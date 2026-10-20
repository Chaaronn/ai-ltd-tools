//! Campaign editing and cached queue snapshots; physics runs only in the worker process.
use crate::theme;
use eframe::egui::{self, Color32, RichText, Vec2};
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
type QueueSnapshot = (Snapshot, Vec<String>);

fn load_queue(store: &Store) -> anyhow::Result<QueueSnapshot> {
    let mut jobs = vec![];
    let mut errors = vec![];
    let mut ids = store.list()?;
    // newest first
    ids.sort_by_key(|id| std::cmp::Reverse(store.created(id)));
    for id in ids {
        let result = (|| -> anyhow::Result<(JobSpec, Progress)> {
            Ok((store.spec(&id)?, store.progress(&id)?))
        })();
        match result {
            Ok((spec,mut p))=>{
                if p.status==Status::Running && !store.active_job(&id){p.status=Status::Paused;p.note=Some("Interrupted worker: resume replays the unfinished generation.".into());}
                jobs.push((id,spec,p));
            }
            Err(error)=>errors.push(format!("Job {id} could not be loaded: {error:#}. Its files are preserved in {} for recovery.",store.root.display())),
        }
    }
    Ok((jobs, errors))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_job_does_not_hide_healthy_queue_entries() {
        let root = std::env::temp_dir().join(format!("simulation-queue-{}", uuid::Uuid::new_v4()));
        let store = Store::at(root.clone());
        let spec = JobSpec::smoke().unwrap();
        let good = store.create(&spec).unwrap();
        let bad = store.create(&spec).unwrap();
        std::fs::write(
            store.job(&bad).unwrap().join("progress.json"),
            b"partial JSON",
        )
        .unwrap();
        let (jobs, errors) = load_queue(&store).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0, good);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains(&bad));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn review_and_results_render_during_full_population_campaign() {
        let root = std::env::temp_dir().join(format!("simulation-ui-{}", uuid::Uuid::new_v4()));
        let store = Store::at(root.clone());
        let spec = JobSpec::smoke().unwrap();
        let id = store.create(&spec).unwrap();
        let worker_store = store.clone();
        let worker_id = id.clone();
        let running =
            std::thread::spawn(move || sim::worker::run(&worker_store, &worker_id).unwrap());
        let context = egui::Context::default();
        let mut view = SimulationUi {
            open: true,
            spec: Some(spec.clone()),
            candidates: spec.alternatives.clone(),
            selected: vec![true],
            last_poll: Instant::now(),
            ..Default::default()
        };
        let library = Arc::new(Library::default());
        let mut frames = 0;
        while !running.is_finished() || frames < 2 {
            let output = context.run_ui(egui::RawInput::default(), |ui| view.ui(ui, &library));
            assert!(!output.shapes.is_empty());
            frames += 1;
            assert!(frames < 10000, "Worker did not finish");
        }
        assert_eq!(running.join().unwrap().status, Status::Completed);
        view.jobs = load_queue(&store).unwrap().0;
        view.open = false;
        let _ = context.run_ui(egui::RawInput::default(), |ui| view.ui(ui, &library));
        assert_eq!(view.jobs[0].2.observations.len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
pub struct SimulationUi {
    open: bool,
    source: String,
    mode: CampaignMode,
    track: String,
    generated: bool,
    track_seed: i64,
    spec: Option<JobSpec>,
    candidates: Vec<Arm>,
    selected: Vec<bool>,
    error: Option<String>,
    preparing: Option<Receiver<Result<JobSpec, String>>>,
    polling: Option<Receiver<Result<QueueSnapshot, String>>>,
    queue_errors: Vec<String>,
    jobs: Snapshot,
    last_poll: Instant,
    suggestion: Option<(String, Measure)>,
    requested_card: Option<Card>,
    /// The job shown in the detail panel.
    picked: Option<String>,
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
            spec: None,
            candidates: vec![],
            selected: vec![],
            error: None,
            preparing: None,
            polling: None,
            queue_errors: vec![],
            jobs: vec![],
            last_poll: Instant::now() - Duration::from_secs(10),
            suggestion: None,
            requested_card: None,
            picked: None,
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
    ) -> bool {
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
            self.error = None;
            return true;
        }
        false
    }
    /// Keep the queue snapshot fresh (also called from other tabs that show results).
    pub fn refresh(&mut self, ctx: &egui::Context) {
        self.poll(ctx);
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.polling
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok((jobs, errors)) => {
                    self.jobs = jobs;
                    self.queue_errors = errors;
                }
                Err(e) => self.error = Some(e),
            }
            self.polling = None;
        }
        if self.polling.is_none() && self.last_poll.elapsed() > Duration::from_secs(2) {
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = (|| -> anyhow::Result<QueueSnapshot> {
                    let store = Store::default_location()?;
                    load_queue(&store)
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
        let seed = self.track_seed;
        // "Copy as path" in Explorer wraps the path in quotes
        let track = if self.generated {
            None
        } else {
            Some(self.track.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string())
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
                if let Some(n) = &source {
                    anyhow::ensure!(
                        !(mode == CampaignMode::SavedWeights && n.gens() == 0),
                        "{} hasn't trained yet, so it has no saved weights worth testing. Choose Fresh training / explore settings to train its design from scratch on the track.",
                        n.file
                    );
                }
                let (mut input, settings, time) = if let Some(n) = source {
                    sim::engine::from_save(&n.path, family.as_deref().unwrap())?
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
                    sim::planner::for_card(&lib, &mut spec, &card)?
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
        let new = ui
            .horizontal(|ui| {
                ui.heading("Simulation");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.add(theme::primary("+  New campaign")).clicked())
                    .inner
            })
            .inner;
        ui.label(RichText::new("Screen settings and designs in a copy of the game's physics. Results stay here and never change the advice; confirm anything promising in the game.").color(theme::MUTED));
        ui.add_space(8.0);
        if new {
            self.open = true;
            self.spec = None;
            self.suggestion = None;
            self.requested_card = None;
            self.preparing = None;
            self.error = None;
            if self.source.is_empty() {
                self.source = track_builder::lab::advice::latest(lib)
                    .map_or(String::new(), |n| n.file.clone());
            }
        }
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
                                let label = if n.gens() == 0 { format!("{} (not trained yet: design only)", n.file) } else { n.file.clone() };
                                ui.selectable_value(&mut self.source, n.file.clone(), label);
                            }
                        });
                    if self.suggestion.is_none() {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.mode, CampaignMode::Fresh, "Fresh training / explore settings");
                            ui.selectable_value(&mut self.mode, CampaignMode::SavedWeights, "Saved weights");
                            ui.selectable_value(&mut self.mode, CampaignMode::Designs, "Explore designs");
                        });
                    }
                    if lib.by_file(&self.source).is_some_and(|n| n.gens() == 0) {
                        if self.mode == CampaignMode::SavedWeights {
                            self.mode = CampaignMode::Fresh;
                        }
                        ui.label(RichText::new("Not trained yet: its design trains from scratch on the track, starting from the game's current settings.").font(theme::sans(12.5)).color(theme::MUTED));
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
                    ui.add_space(4.0);
                    ui.label(RichText::new("Baseline settings").strong());
                    ui.label(RichText::new("Read from the save where it records them and from the game's settings file otherwise (see the notes above). Every alternative changes one thing from these.").small().weak());
                    let before = spec.baseline.clone();
                    settings_editor(ui, &mut spec.baseline, &spec.input);
                    if spec.baseline != before {
                        for (i, arm) in self.candidates.iter_mut().enumerate() {
                            arm.settings = arm.settings.rebase(&before, &spec.baseline);
                            if arm.design.is_none() && arm.settings == spec.baseline {
                                self.selected[i] = false;
                            }
                        }
                    }
                    ui.add_space(4.0);
                    estimate(ui, spec, &self.jobs);
                    ui.collapsing("Complete baseline settings and provenance", |ui| {
                        ui.label(sim::engine::design_description(&spec.input));
                        ui.monospace(serde_json::to_string_pretty(&spec.baseline).unwrap_or_default());
                        ui.label(format!("Source SHA256: {}\nTrack SHA256: {}\nEngine: {}\nMath: {}", spec.input.source_hash, spec.input.track_hash, spec.engine_revision, spec.math_profile));
                    });
                    ui.label("Select up to three alternatives (one setting or design feature each)");
                    for (i, arm) in self.candidates.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if arm.design.is_none() && arm.settings == spec.baseline {
                                ui.add_enabled(false, egui::Checkbox::new(&mut false, format!("{} (now the same as the baseline)", arm.name)));
                                return;
                            }
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
        self.queue(ui);
    }

    /// Jobs whose campaign came from this Data gathering card, newest first: a short result for
    /// each, and a button that opens it. Returns true when the user asks to see one.
    pub fn card_results(&mut self, ui: &mut egui::Ui, card: &Card) -> bool {
        let mut open = None;
        let related: Vec<&(String, JobSpec, Progress)> = self.jobs.iter().filter(|(_, s, _)| s.suggestion_id.as_deref() == Some(card.id.as_str())).take(3).collect();
        if related.is_empty() {
            return false;
        }
        egui::Frame::new().fill(theme::SUNKEN).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new("Simulated").font(theme::medium(12.5)).color(theme::TEXT2));
            for (id, spec, progress) in related {
                let comps = sim::analysis::comparisons(spec, &progress.observations);
                ui.horizontal_wrapped(|ui| {
                    let (label, tone) = status_style(progress.status);
                    theme::dot(ui, tone);
                    ui.label(RichText::new(label).font(theme::sans(12.0)).color(tone));
                    let shown: Vec<String> = comps.iter().filter(|c| c.median.is_some() || c.alternative_only_laps + c.control_only_laps > 0).take(3).map(|c| {
                        format!("{} at stage {}: {} ({})", arm_name(spec, c.arm), c.stage, median_text(spec, c), better_text(spec, c))
                    }).collect();
                    ui.label(RichText::new(if shown.is_empty() { "no results yet".to_string() } else { shown.join(" · ") }).font(theme::sans(12.5)));
                    if ui.small_button("View results").clicked() {
                        open = Some(id.clone());
                    }
                });
            }
        });
        if let Some(id) = open {
            self.picked = Some(id);
            self.open = false;
            return true;
        }
        false
    }

    /// The job list (one row each) and the selected job's detail panel.
    fn queue(&mut self, ui: &mut egui::Ui) {
        for error in &self.queue_errors {
            ui.colored_label(theme::BAD, error);
        }
        let count = |f: &dyn Fn(Status) -> bool| self.jobs.iter().filter(|(_, _, p)| f(p.status)).count();
        let running = count(&|s| s == Status::Running);
        let waiting = count(&|s| matches!(s, Status::Paused | Status::Queued));
        let finished = count(&|s| matches!(s, Status::Completed | Status::Cancelled | Status::Failed));
        ui.horizontal(|ui| {
            for (n, label, fg, bg) in [
                (running, "running", theme::GOOD, Color32::from_rgb(0x1f, 0x2a, 0x22)),
                (waiting, "paused or queued", theme::WARN, Color32::from_rgb(0x2c, 0x27, 0x17)),
                (finished, "finished", theme::MUTED, Color32::from_rgb(0x22, 0x25, 0x2b)),
            ] {
                egui::Frame::new().fill(bg).corner_radius(255).inner_margin(egui::Margin::symmetric(10, 3)).show(ui, |ui| {
                    ui.label(RichText::new(format!("{n} {label}")).font(theme::sans(12.0)).color(fg));
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if finished > 0
                    && ui
                        .button(format!("Clear finished ({finished})"))
                        .on_hover_text("Delete completed, cancelled and failed jobs and their results from this computer")
                        .clicked()
                {
                    let ids: Vec<String> = self.jobs.iter().filter(|(_, _, p)| matches!(p.status, Status::Completed | Status::Cancelled | Status::Failed)).map(|(id, _, _)| id.clone()).collect();
                    for id in ids {
                        self.remove(&id);
                    }
                }
            });
        });
        ui.add_space(10.0);
        if self.jobs.is_empty() {
            ui.label(RichText::new("No campaigns yet. Start one with New campaign, or from a card on Data gathering.").color(theme::MUTED));
            return;
        }
        if self.picked.as_ref().is_none_or(|p| !self.jobs.iter().any(|(id, _, _)| id == p)) {
            self.picked = self
                .jobs
                .iter()
                .find(|(_, _, p)| p.status == Status::Running)
                .or(self.jobs.first())
                .map(|(id, _, _)| id.clone());
        }
        // columns: status, campaign, progress, time left
        let w = ui.available_width();
        let cols = [0.0, 120.0, (w * 0.55).max(380.0), w - 130.0];
        let (hr, _) = ui.allocate_exact_size(Vec2::new(w, 26.0), egui::Sense::hover());
        for (x, text) in cols.iter().zip(["STATUS", "CAMPAIGN", "PROGRESS", "TIME LEFT"]) {
            ui.painter().text(hr.left_center() + Vec2::new(12.0 + x, 0.0), egui::Align2::LEFT_CENTER, text, theme::sans(11.0), theme::DIM);
        }
        theme::rule(ui, theme::BORDER);
        let mut clicked = None;
        for (id, spec, progress) in &self.jobs {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 54.0), egui::Sense::click());
            let picked = self.picked.as_deref() == Some(id.as_str());
            let p = ui.painter();
            if picked {
                p.rect_filled(rect, 0.0, theme::HOVER);
                p.rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())), 0.0, theme::ACCENT);
            } else if resp.hovered() {
                p.rect_filled(rect, 0.0, Color32::from_rgb(0x1b, 0x1e, 0x23));
            }
            p.rect_filled(egui::Rect::from_min_size(rect.left_bottom() - Vec2::new(0.0, 1.0), Vec2::new(w, 1.0)), 0.0, theme::DIVIDER);
            let x = |i: usize| rect.left() + 12.0 + cols[i];
            let (label, tone) = status_style(progress.status);
            p.circle_filled(egui::pos2(x(0) + 4.0, rect.center().y), 4.0, tone);
            p.text(egui::pos2(x(0) + 14.0, rect.center().y), egui::Align2::LEFT_CENTER, label, theme::sans(12.5), tone);
            let title_w = cols[2] - cols[1] - 24.0;
            let title = clip(ui, &job_title(spec), theme::medium(14.0), title_w);
            p.text(egui::pos2(x(1), rect.center().y - 9.0), egui::Align2::LEFT_CENTER, title, theme::medium(14.0), theme::TEXT);
            let sub = clip(ui, &job_subtitle(spec), theme::sans(12.0), title_w);
            p.text(egui::pos2(x(1), rect.center().y + 10.0), egui::Align2::LEFT_CENTER, sub, theme::sans(12.0), theme::MUTED);
            let (done, total) = work(spec, progress);
            let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            let bar_w = (cols[3] - cols[2] - 80.0).max(60.0);
            let bar = egui::Rect::from_min_size(egui::pos2(x(2), rect.center().y - 2.0), Vec2::new(bar_w, 4.0));
            p.rect_filled(bar, 2.0, theme::BORDER);
            let colour = match progress.status {
                Status::Running => theme::ACCENT,
                Status::Paused | Status::Queued => theme::WARN_DOT,
                Status::Completed => theme::GOOD_DOT,
                _ => Color32::from_rgb(0x4a, 0x50, 0x5a),
            };
            if frac > 0.0 {
                p.rect_filled(egui::Rect::from_min_size(bar.min, Vec2::new(bar_w * frac.min(1.0), 4.0)), 2.0, colour);
            }
            p.text(egui::pos2(bar.right() + 10.0, rect.center().y), egui::Align2::LEFT_CENTER, format!("{:.0}%", frac * 100.0), theme::mono(12.0), theme::TEXT2);
            p.text(egui::pos2(x(3), rect.center().y), egui::Align2::LEFT_CENTER, time_left(spec, progress), theme::sans(12.5), theme::MUTED);
            if resp.clicked() {
                clicked = Some(id.clone());
            }
        }
        if let Some(id) = clicked {
            self.picked = Some(id);
        }
        ui.add_space(18.0);
        if let Some(id) = self.picked.clone()
            && let Some((_, spec, progress)) = self.jobs.iter().find(|(j, _, _)| *j == id).cloned()
        {
            self.detail(ui, &id, &spec, &progress);
        }
    }

    fn remove(&mut self, id: &str) {
        match Store::default_location().and_then(|store| store.remove(id)) {
            Ok(()) => {
                self.jobs.retain(|(j, _, _)| j != id);
                if self.picked.as_deref() == Some(id) {
                    self.picked = None;
                }
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
        self.last_poll = Instant::now() - Duration::from_secs(10);
    }

    fn control(&mut self, id: &str, control: Option<Control>) {
        let result = Store::default_location().and_then(|store| match control {
            Some(control) => store.control(id, control),
            None => sim::worker::spawn(&store, id).map(|_| ()),
        });
        if let Err(error) = result {
            self.error = Some(format!("{error:#}"));
        }
        self.last_poll = Instant::now() - Duration::from_secs(10);
    }

    fn detail(&mut self, ui: &mut egui::Ui, id: &str, spec: &JobSpec, progress: &Progress) {
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(24, 20))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(job_title(spec)).font(theme::semibold(17.0)));
                        ui.label(
                            RichText::new(format!(
                                "{} · {} · {} · {} · {:.1} active minutes",
                                spec.input.source_name,
                                spec.input.track_name,
                                spec.measure.label().to_lowercase(),
                                progress.backend,
                                progress.elapsed_seconds / 60.0
                            ))
                            .font(theme::sans(12.5))
                            .color(theme::MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let finished = matches!(progress.status, Status::Completed | Status::Cancelled | Status::Failed);
                        if finished {
                            if ui.button("Remove").on_hover_text("Delete this job and its results from this computer").clicked() {
                                self.remove(id);
                            }
                        } else {
                            if ui.button("Cancel").clicked() {
                                self.control(id, Some(Control::Cancel));
                            }
                            if progress.status == Status::Running {
                                if ui.button("Pause").clicked() {
                                    self.control(id, Some(Control::Pause));
                                }
                            } else if ui.add(theme::primary(if progress.completed_generations > 0 { "Resume" } else { "Start" })).clicked() {
                                self.control(id, None);
                            }
                        }
                    });
                });
                ui.add_space(12.0);
                // key numbers
                let tasks = sim::worker::tasks(spec);
                let task = tasks.get(progress.task);
                let seed_no = task.and_then(|t| spec.seeds.iter().position(|s| *s == t.seed)).map_or("—".to_string(), |i| format!("{} of {}", i + 1, spec.seeds.len()));
                let generation = task.map_or("—".to_string(), |t| format!("{} / {}", thousands(progress.generation), thousands(t.end)));
                let arm = task.map_or("—".to_string(), |t| if t.arm == 0 { "Baseline".to_string() } else { arm_name(spec, t.arm) });
                let speed = if progress.completed_generations > 0 && progress.elapsed_seconds > 0.0 {
                    format!("{:.2} gen/s", progress.completed_generations as f64 / progress.elapsed_seconds)
                } else {
                    "—".to_string()
                };
                let finished = matches!(progress.status, Status::Completed | Status::Cancelled | Status::Failed);
                let (_, total) = work(spec, progress);
                let stats: Vec<(&str, String)> = if finished {
                    vec![
                        ("Seeds", spec.seeds.len().to_string()),
                        ("Generations", format!("{} of {}", thousands(work(spec, progress).0), thousands(total))),
                        ("Speed", speed),
                        ("Status", status_style(progress.status).0.to_string()),
                    ]
                } else {
                    vec![("Seed", seed_no), ("Running", arm), ("Generation", generation), ("Speed", speed), ("Time left", time_left(spec, progress))]
                };
                ui.horizontal(|ui| {
                    for (label, value) in stats {
                        ui.vertical(|ui| {
                            ui.label(theme::header(label));
                            ui.label(RichText::new(value).font(theme::mono(14.0)));
                        });
                        ui.add_space(28.0);
                    }
                });
                if let Some(perf) = progress.perf.as_ref().filter(|p| p.generations > 0) {
                    let ms = |s: f64| if s >= 1.0 { format!("{s:.2} s") } else { format!("{:.0} ms", s * 1000.0) };
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Recent generations").font(theme::sans(12.5)).color(theme::TEXT2));
                        ui.label(
                            RichText::new(format!(
                                "{} each: driving {} ({:.1} simulated s, {}× real time) · breeding {} · saving {}",
                                ms(perf.per_generation()),
                                ms(perf.physics),
                                perf.ticks / 60.0,
                                perf.sim_speed().map_or("—".into(), |v| format!("{v:.0}")),
                                ms(perf.reproduce),
                                ms(perf.saving),
                            ))
                            .font(theme::mono(12.0))
                            .color(theme::MUTED),
                        )
                        .on_hover_text(
                            "Generations per second falls as the cars learn to survive: each generation then simulates more seconds before every car is out or the time limit ends it. The real-time multiple is the speed that doesn't depend on that.",
                        );
                    });
                }
                if let Some(note) = &progress.note {
                    ui.label(RichText::new(note).font(theme::sans(12.5)).color(theme::WARN));
                }
                if let Some(error) = &progress.error {
                    ui.colored_label(theme::BAD, error);
                }
                ui.add_space(14.0);
                ui.label(RichText::new("Results").font(theme::semibold(14.0)));
                ui.add_space(4.0);
                let comps = sim::analysis::comparisons(spec, &progress.observations);
                let saved = spec.mode == CampaignMode::SavedWeights;
                egui::Grid::new(("sim-results", id)).num_columns(5).spacing([28.0, 8.0]).min_col_width(60.0).show(ui, |ui| {
                    for h in ["Alternative", "Stage", "Verdict", "Median", if saved { "Seeds better" } else { "Families better" }] {
                        ui.label(theme::header(h));
                    }
                    ui.end_row();
                    for c in &comps {
                        ui.label(arm_name(spec, c.arm));
                        ui.label(RichText::new(thousands(c.stage)).font(theme::mono(13.0)).color(theme::MUTED));
                        let verdict = if saved {
                            c.seed_label.clone()
                        } else if c.label == "Insufficient data" {
                            format!("Too few families yet ({} of 5)", c.families)
                        } else {
                            c.label.clone()
                        };
                        let verdict = if c.trials == 0 { "No results yet".to_string() } else { verdict };
                        ui.label(RichText::new(verdict).color(theme::TEXT2));
                        let colour = match c.median {
                            Some(m) if m > 1e-9 => theme::GOOD,
                            Some(m) if m < -1e-9 => theme::BAD,
                            _ => theme::DIM,
                        };
                        ui.label(RichText::new(median_text(spec, c)).font(theme::mono(13.0)).color(colour));
                        ui.label(RichText::new(better_text(spec, c)).font(theme::mono(13.0)));
                        ui.end_row();
                    }
                });
                if comps.iter().all(|c| c.trials == 0) {
                    ui.label(RichText::new(if matches!(progress.status, Status::Cancelled | Status::Failed) {
                        "No results: it stopped before the first comparison finished."
                    } else {
                        "No comparisons finished yet. Results appear as each seed's branches complete."
                    }).color(theme::DIM));
                }
                let one_sided: usize = comps.iter().map(|c| c.alternative_only_laps + c.control_only_laps).sum();
                if one_sided > 0 {
                    ui.label(RichText::new("Some seeds lapped on one side only: those count as a win or loss with no size, so they're left out of the median.").font(theme::sans(12.0)).color(theme::MUTED));
                }
                ui.add_space(10.0);
                egui::CollapsingHeader::new(RichText::new("Details and how to confirm in the game").color(theme::TEXT2)).id_salt(("sim-details", id)).show(ui, |ui| {
                    ui.label(RichText::new(match spec.mode {
                        CampaignMode::SavedWeights => "All seeds start from the same saved network, so they show how consistent the change is for this network, not across networks.",
                        CampaignMode::Fresh => "Each seed is a fresh network, so each counts as an independent family. Promising in simulation needs five families, 80% of them better and a median of at least 1% (2 points for survival).",
                        CampaignMode::Designs => "Whole-training comparison: each design trains from scratch. It doesn't show the effect of changing the design late in training.",
                    }).color(theme::MUTED));
                    ui.label(RichText::new(if spec.mode == CampaignMode::Designs {
                        "To confirm: train the compared designs from scratch in the game with matched settings, on unrelated starting networks."
                    } else {
                        "To confirm: repeat the single-setting switch in the game with the same track, time limit and generation budget, on unrelated starting networks."
                    }).color(theme::MUTED));
                    for c in comps.iter().filter(|c| !c.seed_differences.is_empty()) {
                        ui.label(RichText::new(format!(
                            "{} · stage {}: {}",
                            arm_name(spec, c.arm),
                            c.stage,
                            c.seed_differences.iter().map(|(seed, v)| format!("seed {seed} {v:+.2}")).collect::<Vec<_>>().join(", ")
                        )).font(theme::mono(12.0)).color(theme::TEXT2));
                    }
                    if let Some(s) = &spec.suggestion_id {
                        ui.label(RichText::new(format!("Started from a Data gathering card ({s}).")).font(theme::sans(12.0)).color(theme::DIM));
                    }
                    ui.label(RichText::new(format!("Simulation never changes the game evidence. Job {id}.")).font(theme::sans(11.5)).color(theme::DIM));
                });
            });
    }
}

fn status_style(status: Status) -> (&'static str, Color32) {
    match status {
        Status::Running => ("Running", theme::GOOD),
        Status::Paused => ("Paused", theme::WARN),
        Status::Queued => ("Ready", theme::WARN),
        Status::Completed => ("Done", theme::TEXT2),
        Status::Cancelled => ("Cancelled", theme::DIM),
        Status::Failed => ("Failed", theme::BAD),
    }
}

fn arm_name(spec: &JobSpec, arm: usize) -> String {
    spec.alternatives.get(arm.wrapping_sub(1)).map_or("?".into(), |a| a.name.clone())
}

fn job_title(spec: &JobSpec) -> String {
    match spec.alternatives.as_slice() {
        [one] => format!("{} vs baseline", one.name),
        [] => spec.title.clone(),
        many => format!("{} alternatives: {}", many.len(), many.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")),
    }
}

fn job_subtitle(spec: &JobSpec) -> String {
    let mode = match spec.mode {
        CampaignMode::SavedWeights => "From saved weights".to_string(),
        CampaignMode::Fresh => format!("Fresh networks · stages {}", spec.stages.iter().map(|s| thousands(*s)).collect::<Vec<_>>().join(", ")),
        CampaignMode::Designs => "Design comparison".to_string(),
    };
    format!("{mode} · {} · {}", spec.input.source_name, spec.input.track_name)
}

/// Generations done and in total.
fn work(spec: &JobSpec, progress: &Progress) -> (u64, u64) {
    let total: u64 = sim::worker::tasks(spec).iter().map(|t| t.end - t.start).sum();
    let done = if progress.status == Status::Completed { total } else { progress.completed_generations.min(total) };
    (done, total)
}

fn time_left(spec: &JobSpec, progress: &Progress) -> String {
    match progress.status {
        Status::Completed => return "done".into(),
        Status::Cancelled | Status::Failed => return "—".into(),
        _ => {}
    }
    let (done, total) = work(spec, progress);
    if done == 0 || progress.elapsed_seconds <= 0.0 {
        return "—".into();
    }
    let secs = (total - done) as f64 * progress.elapsed_seconds / done as f64;
    if secs < 90.0 {
        "≈ 1 min".into()
    } else if secs < 3600.0 {
        format!("≈ {:.0} min", secs / 60.0)
    } else {
        format!("≈ {:.1} h", secs / 3600.0)
    }
}

fn median_text(spec: &JobSpec, c: &sim::analysis::Comparison) -> String {
    match c.median {
        Some(v) => format!("{v:+.2}{}", if spec.measure == Measure::Survival { " pts" } else { "%" }),
        None if c.alternative_only_laps > 0 && c.control_only_laps == 0 => "laps only with it".into(),
        None if c.control_only_laps > 0 && c.alternative_only_laps == 0 => "laps only without".into(),
        None => "—".into(),
    }
}

fn better_text(spec: &JobSpec, c: &sim::analysis::Comparison) -> String {
    if c.trials == 0 {
        return "—".into();
    }
    if spec.mode == CampaignMode::SavedWeights {
        format!("{} of {}", c.seeds_better, c.seeds_compared)
    } else {
        format!("{} of {}", c.wins, c.families)
    }
}

/// Shorten text to fit `width`, ending in an ellipsis.
fn clip(ui: &egui::Ui, text: &str, font: egui::FontId, width: f32) -> String {
    let fits = |t: &str| ui.fonts_mut(|f| f.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x) <= width;
    if fits(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let t: String = chars[..mid].iter().collect::<String>() + "…";
        if fits(&t) { lo = mid } else { hi = mid - 1 }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

fn choice(ui: &mut egui::Ui, id: &str, value: &mut String, options: &[(&str, &str)]) {
    let shown = options.iter().find(|(k, _)| *k == value.as_str()).map_or(value.clone(), |(_, l)| l.to_string());
    egui::ComboBox::from_id_salt(id).selected_text(shown).show_ui(ui, |ui| {
        for (k, l) in options {
            ui.selectable_value(value, k.to_string(), *l);
        }
    });
}

/// The baseline training settings, all editable.
fn settings_editor(ui: &mut egui::Ui, s: &mut Settings, input: &PreparedInput) {
    egui::Grid::new("sim-baseline").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.label("Mutation rate");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut s.mutation_rate).range(0.0..=10.0).speed(0.005).max_decimals(4));
            ui.checkbox(&mut s.adaptive_mutation, "adaptive")
                .on_hover_text("The game's \"Adaptive mutation rate\": scales the rate by a fixed factor from the network's size.");
            if s.adaptive_mutation {
                let arch = track_builder::lab::format::Arch { shape: input.shape(), ..Default::default() };
                let f = track_builder::lab::mutation::adaptive_factor(&arch);
                ui.label(RichText::new(format!("≈ {:.3} used (×{f:.2} for this network's size)", s.mutation_rate * f)).small().weak());
            }
        });
        ui.end_row();
        ui.label("Selection");
        ui.horizontal(|ui| {
            choice(ui, "sim-selection", &mut s.selection_algorithm, &[("best", "Best"), ("tournament", "Tournament"), ("roulette", "Roulette")]);
            ui.add(egui::DragValue::new(&mut s.selection_size).range(1..=POPULATION).prefix("size "))
                .on_hover_text("How many of the top cars are picked as parents.");
        });
        ui.end_row();
        ui.label("Crossover");
        choice(ui, "sim-crossover", &mut s.crossover, &[("none", "None"), ("single_point", "Single point"), ("uniform", "Uniform")]);
        ui.end_row();
        ui.label("Preserve parents");
        ui.horizontal(|ui| {
            choice(ui, "sim-preserve", &mut s.preserve_parents, &[("off", "Off"), ("on_selection_size", "The selected parents"), ("on_custom", "A custom number")]);
            if s.preserve_parents == "on_custom" {
                ui.add(egui::DragValue::new(&mut s.preserve_parents_size).range(0..=POPULATION).prefix("keep "));
            }
        })
        .response
        .on_hover_text("How many parents carry over to the next generation unchanged.");
        ui.end_row();
        ui.label("Weight decay");
        ui.add(egui::DragValue::new(&mut s.weight_decay).range(0.0..=1.0).speed(0.0001).max_decimals(5));
        ui.end_row();
        ui.label("Elimination");
        ui.horizontal(|ui| {
            ui.checkbox(&mut s.eliminate_on_wall, "on hitting a wall");
            ui.checkbox(&mut s.eliminate_when_idle, "when idle");
        });
        ui.end_row();
        ui.label("Rewards");
        ui.label(
            s.rewards
                .iter()
                .map(|r| format!("{} {}{}", r.metric.replace('_', " "), r.weight, if r.kind == "average" { " (average)" } else { "" }))
                .collect::<Vec<_>>()
                .join(" · "),
        )
        .on_hover_text("From the save. Reward changes aren't simulated as alternatives yet.");
        ui.end_row();
        ui.label("Population");
        ui.label(format!("{POPULATION} (fixed)"));
        ui.end_row();
    });
}

/// How much work the campaign is, and roughly how long it takes on this computer.
fn estimate(ui: &mut egui::Ui, spec: &mut JobSpec, jobs: &Snapshot) {
    let gens = |spec: &JobSpec| sim::worker::tasks(spec).iter().map(|t| t.end - t.start).sum::<u64>();
    let total = gens(spec);
    // simulated seconds per wall second per thread, from work this computer has finished
    let mut rates: Vec<f64> = jobs
        .iter()
        .filter(|(_, _, p)| p.completed_generations > 0 && p.elapsed_seconds > 5.0)
        .map(|(_, s, p)| p.completed_generations as f64 * s.time_limit_seconds / p.elapsed_seconds / s.threads.max(1) as f64)
        .collect();
    let rate = track_builder::lab::runs::median(&mut rates);
    let (limit, threads) = (spec.time_limit_seconds, spec.threads.max(1) as f64);
    let duration = move |gens: u64| -> Option<f64> { rate.map(|r| gens as f64 * limit / (r * threads)) };
    let hours = |s: f64| if s < 3600.0 { format!("{:.0} min", s / 60.0) } else { format!("{:.1} h", s / 3600.0) };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(format!(
            "Work: {} generations of {POPULATION} cars across {} seed(s) and {} arm(s).",
            thousands(total),
            spec.seeds.len(),
            spec.alternatives.len() + 1
        )).strong());
        match duration(total) {
            Some(d) => ui.label(format!(
                "About {} of computing on {} thread(s), from the speed of earlier jobs here (rough: generations that end early, when every car is eliminated, are quicker; the session budget pauses it every {}).",
                hours(d),
                spec.threads,
                hours(spec.budget_seconds as f64)
            )),
            None => ui.label("No speed measured on this computer yet. Run a small job first (one seed, stages 0 and 200) to get an estimate for bigger ones."),
        };
        if spec.mode != CampaignMode::SavedWeights && spec.stages.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Each stage's share:").small());
                let mut drop = None;
                for &stage in &spec.stages {
                    let mut without = spec.clone();
                    without.stages.retain(|s| *s != stage);
                    let saved = total - gens(&without);
                    let label = match duration(saved) {
                        Some(d) => format!("stage {stage}: {} gens, {}", thousands(saved), hours(d)),
                        None => format!("stage {stage}: {} gens", thousands(saved)),
                    };
                    if ui.small_button(format!("✕ {label}")).on_hover_text("Leave this stage out").clicked() {
                        drop = Some(stage);
                    }
                }
                if let Some(stage) = drop {
                    spec.stages.retain(|s| *s != stage);
                }
            });
        }
    });
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
