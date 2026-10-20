//! Campaign editing and cached queue snapshots; physics runs only in the worker process.
use eframe::egui::{self, Color32, RichText};
use sim::storage::{Control, Store};
use std::{
    path::Path,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};
use track_builder::{
    lab::{
        gather::Card,
        library::{Library, Network},
        overview,
    },
    simulation::{self as sim, *},
};

type Snapshot = Vec<(String, JobSpec, Progress)>;
pub struct SimulationUi {
    open: bool,
    source: String,
    mode: CampaignMode,
    track: String,
    generated: bool,
    track_seed: i64,
    origin_grip: bool,
    spec: Option<JobSpec>,
    candidates: Vec<Arm>,
    selected: Vec<bool>,
    error: Option<String>,
    preparing: Option<Receiver<Result<JobSpec, String>>>,
    polling: Option<Receiver<Result<Snapshot, String>>>,
    jobs: Snapshot,
    last_poll: Instant,
    suggestion: Option<(String, Measure)>,
    requested_card: Option<Card>,
}
impl Default for SimulationUi {
    fn default() -> Self {
        Self {
            open: false,
            source: String::new(),
            mode: CampaignMode::Fresh,
            track: String::new(),
            generated: false,
            track_seed: 1729,
            origin_grip: false,
            spec: None,
            candidates: vec![],
            selected: vec![],
            error: None,
            preparing: None,
            polling: None,
            jobs: vec![],
            last_poll: Instant::now() - Duration::from_secs(10),
            suggestion: None,
            requested_card: None,
        }
    }
}
impl SimulationUi {
    pub fn card(
        &mut self,
        ui: &mut egui::Ui,
        _lib: &Arc<Library>,
        current: Option<&Network>,
        card: &Card,
    ) {
        let dimension = card.id.split(':').nth(2).unwrap_or("");
        let supported = card.id.starts_with("setting:")
            && [
                "Mutation",
                "Selection",
                "Parents kept",
                "Selection size",
                "Crossover",
                "Weight decay",
                "Elimination",
            ]
            .contains(&dimension);
        let design = card.id.starts_with("design:") || card.id.starts_with("feature:");
        let reason = if dimension == "Population" {
            "Population is fixed at 1,200."
        } else {
            "This question needs a different protocol; use a fresh design campaign for architecture or sensor changes."
        };
        if ui
            .add_enabled(
                current.is_some() && (supported || design),
                egui::Button::new(if design {
                    "Explore designs"
                } else {
                    "Simulate"
                }),
            )
            .on_disabled_hover_text(reason)
            .clicked()
        {
            self.source = current.unwrap().file.clone();
            self.mode = if design {
                CampaignMode::Designs
            } else {
                CampaignMode::SavedWeights
            };
            let measure = match card.measure {
                track_builder::lab::compare::Measure::Lap => Measure::Lap,
                track_builder::lab::compare::Measure::Survival => Measure::Survival,
                _ => Measure::Rate,
            };
            self.suggestion = Some((card.id.clone(), measure));
            self.requested_card = Some(card.clone());
            self.spec = None;
            self.open = true;
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.polling
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(jobs) => self.jobs = jobs,
                Err(e) => self.error = Some(e),
            }
            self.polling = None;
        }
        if self.polling.is_none() && self.last_poll.elapsed() > Duration::from_secs(2) {
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = (|| -> anyhow::Result<Snapshot> {
                    let store = Store::default_location()?;
                    let mut jobs = vec![];
                    for id in store.list()? {
                        let spec = store.spec(&id)?;
                        let mut p = store.progress(&id)?;
                        if p.status == Status::Running && !store.active_job(&id) {
                            p.status = Status::Paused;
                            p.note = Some(
                                "Interrupted worker: resume replays the unfinished generation."
                                    .into(),
                            );
                        }
                        jobs.push((id, spec, p));
                    }
                    Ok(jobs)
                })();
                let _ = tx.send(result.map_err(|e| format!("{e:#}")));
                ctx.request_repaint();
            });
            self.polling = Some(rx);
            self.last_poll = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }
    fn prepare(&mut self, lib: &Arc<Library>, ctx: &egui::Context) {
        let source = lib.by_file(&self.source).cloned();
        let family = source
            .as_ref()
            .map(|n| overview::family(lib, n).to_string());
        let mode = self.mode;
        let origin = self.origin_grip;
        let seed = self.track_seed;
        let track = if self.generated {
            None
        } else {
            Some(self.track.clone())
        };
        let card = self.requested_card.clone();
        let suggestion = self.suggestion.clone();
        let lib = lib.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.preparing = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<JobSpec> {
                let (mut input, settings, time) = if let Some(n) = source {
                    sim::engine::from_save(&n.path, family.as_deref().unwrap(), origin)?
                } else {
                    anyhow::ensure!(mode != CampaignMode::SavedWeights, "Choose a saved network");
                    (sim::engine::preset(1, seed)?, Settings::default(), None)
                };
                if let Some(path) = &track {
                    anyhow::ensure!(
                        !path.trim().is_empty(),
                        "Supply a .track file or explicitly select generated geometry"
                    );
                }
                sim::engine::set_track(&mut input, track.as_deref().map(Path::new), seed)?;
                let mut spec = JobSpec::new(input, settings, time.unwrap_or(0.0));
                spec.mode = mode;
                spec.title = match mode {
                    CampaignMode::Fresh => "Fresh-network stage campaign",
                    CampaignMode::SavedWeights => "Experiment initialized from saved weights",
                    CampaignMode::Designs => "Whole-training design comparison",
                }
                .into();
                if mode == CampaignMode::SavedWeights {
                    spec.stages = vec![0];
                }
                if let Some((id, measure)) = suggestion {
                    spec.suggestion_id = Some(id);
                    spec.measure = measure;
                }
                spec.alternatives = if mode == CampaignMode::Designs {
                    sim::planner::design_alternatives(&spec.input, &spec.baseline, Some(&lib))
                } else if let Some(card) = card {
                    sim::planner::for_card(&lib, &spec, &card)?
                } else {
                    sim::planner::explore_settings(&lib, &spec)
                };
                if spec.alternatives.is_empty() && mode == CampaignMode::Fresh {
                    let mut settings = spec.baseline.clone();
                    settings.mutation_rate = if settings.mutation_rate == 0.1 {
                        0.2
                    } else {
                        0.1
                    };
                    spec.alternatives.push(Arm {
                        name: format!("Mutation {} (manual candidate)", settings.mutation_rate),
                        settings,
                        design: None,
                    });
                }
                Ok(spec)
            })();
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, lib: &Arc<Library>) {
        self.poll(ui.ctx());
        if ui.button("New simulation campaign").clicked() {
            self.open = true;
            self.spec = None;
            self.suggestion = None;
            self.requested_card = None;
            self.preparing = None;
            self.error = None;
            if self.source.is_empty() {
                self.source=track_builder::lab::advice::latest(lib).map_or(String::new(),|n|n.file.clone());
            }
        }
        ui.label(RichText::new("Simulation evidence is separate from game observations. Population: 1,200. Game networks are never modified.").small().weak());
        if let Some(rx) = &self.preparing
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(mut spec) => {
                    self.candidates = std::mem::take(&mut spec.alternatives);
                    self.selected = (0..self.candidates.len()).map(|i| i < 3).collect();
                    self.spec = Some(spec);
                }
                Err(e) => self.error = Some(e),
            }
            self.preparing = None;
        }
        if let Some(error) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if self.open {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("Review simulation campaign");
                if self.spec.is_none() {
                    egui::ComboBox::from_id_salt("sim-source")
                        .selected_text(if self.source.is_empty() { "Fresh rally preset" } else { &self.source })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.source, String::new(), "Fresh rally preset");
                            for n in &lib.networks {
                                ui.selectable_value(&mut self.source, n.file.clone(), &n.file);
                            }
                        });
                    if self.suggestion.is_none() {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.mode, CampaignMode::Fresh, "Fresh training / explore settings");
                            ui.selectable_value(&mut self.mode, CampaignMode::SavedWeights, "Saved weights");
                            ui.selectable_value(&mut self.mode, CampaignMode::Designs, "Explore designs");
                        });
                    }
                    ui.checkbox(&mut self.generated, "Use deterministic generated geometry");
                    if self.generated {
                        ui.add(egui::DragValue::new(&mut self.track_seed).prefix("Track seed "));
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(".track path");
                            ui.text_edit_singleline(&mut self.track);
                        });
                    }
                    ui.checkbox(&mut self.origin_grip, "Explicitly use (0,0) for missing grip position descriptors");
                    if ui.add_enabled(self.preparing.is_none(), egui::Button::new("Prepare review")).clicked() {
                        self.prepare(lib, ui.ctx());
                    }
                    if self.preparing.is_some() {
                        ui.spinner();
                    }
                } else if let Some(spec) = self.spec.as_mut() {
                    ui.label(&spec.title);
                    ui.label(format!("{} · {} · population {}", spec.input.source_name, spec.input.track_name, POPULATION));
                    for note in &spec.input.notes {
                        ui.label(RichText::new(note).small());
                    }
                    ui.horizontal(|ui| {
                        ui.label("Primary measure");
                        if spec.suggestion_id.is_none() {
                            for measure in [Measure::Rate, Measure::Lap, Measure::Survival] {
                                ui.selectable_value(&mut spec.measure, measure, measure.label());
                            }
                        } else {
                            ui.label(spec.measure.label());
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut spec.time_limit_seconds).range(0.0..=86400.0).prefix("Generation limit (seconds) "));
                        ui.add(egui::DragValue::new(&mut spec.generations).range(1..=100000).prefix("Comparison generations "));
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut spec.budget_seconds).range(1..=86400).prefix("Session budget (seconds) "));
                        ui.add(egui::DragValue::new(&mut spec.threads).range(1..=256).prefix("CPU threads "));
                    });
                    ui.horizontal(|ui| {
                        for backend in [Backend::Auto, Backend::Cpu, Backend::Gpu] {
                            ui.selectable_value(&mut spec.backend, backend, format!("{backend:?}"));
                        }
                    });
                    ui.label("GPU requires a matching user-installed backend. Unverified or unavailable GPU falls back to CPU; the actual backend and reason are recorded.");
                    ui.horizontal(|ui| {
                        ui.label("Starting families / seeds");
                        let mut count = spec.seeds.len();
                        if ui.add(egui::DragValue::new(&mut count).range(1..=100)).changed() {
                            spec.seeds = (1729..1729 + count as i64).collect();
                        }
                        ui.label(format!("{:?}", spec.seeds));
                    });
                    if spec.mode != CampaignMode::SavedWeights {
                        ui.horizontal(|ui| {
                            ui.label("Branch stages");
                            for stage in [0, 200, 1000, 5000] {
                                let mut selected = spec.stages.contains(&stage);
                                if ui.checkbox(&mut selected, stage.to_string()).changed() {
                                    spec.stages.retain(|s| *s != stage);
                                    if selected {
                                        spec.stages.push(stage);
                                    }
                                    spec.stages.sort();
                                }
                            }
                        });
                    }
                    ui.collapsing("Complete baseline settings and provenance", |ui| {
                        ui.label(sim::engine::design_description(&spec.input));
                        ui.monospace(serde_json::to_string_pretty(&spec.baseline).unwrap_or_default());
                        ui.label(format!("Source SHA256: {}\nTrack SHA256: {}\nEngine: {}\nMath: {}", spec.input.source_hash, spec.input.track_hash, spec.engine_revision, spec.math_profile));
                    });
                    ui.label("Select up to three alternatives (one setting or design feature each)");
                    for (i, arm) in self.candidates.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.selected[i], &arm.name);
                            ui.collapsing("Details", |ui| {
                                ui.monospace(serde_json::to_string_pretty(&arm.settings).unwrap_or_default());
                                if let Some(design)=&arm.design {ui.label(sim::engine::design_description(design));}
                            });
                        });
                    }
                    spec.alternatives = self.candidates.iter().zip(&self.selected)
                        .filter(|(_, selected)| **selected).map(|(arm, _)| arm.clone()).collect();
                    let validation = spec.validate();
                    if let Err(error) = &validation {
                        ui.colored_label(Color32::YELLOW, error.to_string());
                    }
                    if ui.add_enabled(validation.is_ok(), egui::Button::new("Create reviewed experiment")).clicked() {
                        match Store::default_location().and_then(|store| store.create(spec)) {
                            Ok(_) => {
                                self.open = false;
                                self.last_poll = Instant::now() - Duration::from_secs(10);
                            }
                            Err(error) => self.error = Some(format!("{error:#}")),
                        }
                    }
                }
                if self.spec.is_some() && ui.button("Back to setup").clicked() {
                    self.spec=None;
                }
                if ui.button("Close builder").clicked() {
                    self.open = false;
                }
            });
        }
        ui.collapsing(format!("Simulation queue and results ({})", self.jobs.len()), |ui| {
            for (id, spec, progress) in &self.jobs {
                ui.push_id(id, |ui| {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.strong(&spec.title);
                        ui.label(format!("{:?} · {} · task {} · generation {} · {:.1} active minutes",
                            progress.status, progress.backend, progress.task + 1, progress.generation, progress.elapsed_seconds / 60.0));
                        if let Some(task)=sim::worker::tasks(spec).get(progress.task) {
                            ui.label(format!("Seed {} · {} · stage start {} · target generation {}",task.seed,if task.arm==0{"Baseline"}else{&spec.alternatives[task.arm-1].name},task.start,task.end));
                        }
                        if let Some(note) = &progress.note {
                            ui.label(note);
                        }
                        if let Some(error) = &progress.error {
                            ui.colored_label(Color32::LIGHT_RED, error);
                        }
                        if progress.completed_generations > 0 && progress.elapsed_seconds > 0.0 {
                            let rate = progress.completed_generations as f64 / progress.elapsed_seconds;
                            let total: u64 = sim::worker::tasks(spec).iter().map(|task| task.end - task.start).sum();
                            ui.label(format!("Measured {:.2} generations/sec · approx {:.1} minutes remaining (throughput varies)",
                                rate, total.saturating_sub(progress.completed_generations) as f64 / rate / 60.0));
                        }
                        ui.horizontal(|ui| {
                            let action = if !matches!(progress.status, Status::Completed | Status::Cancelled | Status::Running)
                                && ui.button("Start / resume").clicked() {
                                Some(None)
                            } else if progress.status == Status::Running && ui.button("Pause").clicked() {
                                Some(Some(Control::Pause))
                            } else if !matches!(progress.status, Status::Completed | Status::Cancelled)
                                && ui.button("Cancel job").clicked() {
                                Some(Some(Control::Cancel))
                            } else {
                                None
                            };
                            if let Some(control) = action {
                                let result = Store::default_location().and_then(|store| {
                                    if let Some(control) = control {
                                        store.control(id, control)
                                    } else {
                                        sim::worker::spawn(&store, id).map(|_| ())
                                    }
                                });
                                if let Err(error) = result {
                                    self.error = Some(format!("{error:#}"));
                                }
                                self.last_poll = Instant::now() - Duration::from_secs(10);
                            }
                        });
                        for comparison in sim::analysis::comparisons(spec, &progress.observations) {
                            ui.label(format!("{} · stage {} · {} · median {} · {} families, {} trials, {} missing · W/T/L {}/{}/{}",
                                spec.alternatives[comparison.arm - 1].name, comparison.stage, comparison.label,
                                comparison.median.map_or("—".into(), |value| format!("{value:+.2}{}",if spec.measure==Measure::Survival{" percentage points"}else{"%"})),
                                comparison.families, comparison.trials, comparison.missing, comparison.wins, comparison.ties, comparison.losses));
                            ui.collapsing(format!("Paired differences · arm {} stage {}", comparison.arm, comparison.stage), |ui| {
                                for (seed,value) in &comparison.seed_differences {ui.label(format!("Seed {seed}: {value:+.2}"));}
                                for (family, value) in &comparison.differences {
                                    ui.label(format!("{family}: {value:+.2}"));
                                    let (response, painter) = ui.allocate_painter(egui::vec2(280.0, 12.0), egui::Sense::hover());
                                    let mid = response.rect.center();
                                    let x = mid.x + (*value as f32).clamp(-100.0, 100.0) * 1.3;
                                    let color = if *value >= 0.0 { Color32::LIGHT_GREEN } else { Color32::LIGHT_RED };
                                    painter.line_segment([mid, egui::pos2(x, mid.y)], egui::Stroke::new(5.0, color));
                                }
                            });
                        }
                        if spec.mode == CampaignMode::SavedWeights {
                            ui.label("Seeds measure consistency within one source family, not independent source networks.");
                        }
                        if spec.mode == CampaignMode::Designs {
                            ui.label("Whole-training comparison: this does not establish the effect of changing architecture late in training.");
                        }
                        if let Some(suggestion) = &spec.suggestion_id {
                            ui.label(format!("Originating suggestion: {suggestion}"));
                        }
                        ui.label(if spec.mode==CampaignMode::Designs {
                            "Confirm in game: train the compared designs from scratch with matched settings and stage endpoints on unrelated starting networks."
                        }else{"Confirm in game: repeat the paired single-setting test with the same track, time limit and generation budget on unrelated starting networks."});
                        ui.small("Simulation never upgrades game evidence automatically.");
                        ui.small(format!("Job {id}"));
                    });
                });
            }
        });
    }
}
