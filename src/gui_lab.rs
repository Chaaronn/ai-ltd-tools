//! The Evolution Lab tab: an evaluate-only view of the game's saved networks.
//!
//! Reads the game's evolution folder in a background thread, re-reads changed saves every few
//! seconds, and never writes anything.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2};
use track_builder::lab::compare::{self, Filters, Measure, TrackFilter, STAGES};
use track_builder::lab::format::{self, Gen, Save, CROSSOVER, SELECTION, VEHICLES};
use track_builder::lab::library::{self, Library, Network};
use track_builder::lab::advice::{self, Advice};
use track_builder::lab::designs;
use track_builder::lab::detectability;
use track_builder::lab::netviz;
use track_builder::lab::keeper;
use track_builder::lab::projection;
use track_builder::lab::overview::{self, Strength};
use track_builder::lab::runs::{self, Run, BUCKETS};
use track_builder::lab::verdict::{self, fmt_opt, Outcome, Verdict};

const SCORE: Color32 = Color32::from_rgb(95, 160, 255);
const LAP: Color32 = Color32::from_rgb(255, 160, 60);
const SPEED: Color32 = Color32::from_rgb(60, 200, 190);
const SURVIVAL: Color32 = Color32::from_rgb(150, 210, 110);
const SPREAD: Color32 = Color32::from_rgb(200, 150, 230);
const HELPED: Color32 = Color32::from_rgb(70, 200, 100);
const HURT: Color32 = Color32::from_rgb(235, 80, 80);
const NEUTRAL: Color32 = Color32::from_rgb(150, 150, 150);
const STALL: Color32 = Color32::from_rgba_premultiplied(70, 52, 12, 50);
const REFRESH: Duration = Duration::from_secs(5);

#[derive(PartialEq, Clone, Copy)]
enum View {
    Now,
    Overview,
    Designs,
    Network,
    Compare,
}

#[derive(PartialEq, Clone, Copy)]
enum Metric {
    Score,
    Lap,
    Speed,
}

#[derive(PartialEq, Clone, Copy)]
enum Col {
    Start,
    Gens,
    Mutation,
    Selection,
    Crossover,
    Population,
    Median,
    Gain,
    LapGain,
    PerMin,
    Stability,
    Stage,
}

pub struct Lab {
    dir: PathBuf,
    dir_input: String,
    lib: Option<Arc<Library>>,
    loading: Option<Receiver<Library>>,
    last_check: Instant,
    selected: Option<String>,
    view: View,
    metric: Metric,
    selected_run: Option<usize>,
    sort: (Col, bool),
    filters: Filters,
    window: usize,
    /// Visible generation range on the timeline (None = everything).
    x_range: Option<(f64, f64)>,
    /// Overview scope: None = all tracks, else a track key.
    ov_track: Option<String>,
    ov_measure: Measure,
    /// Network shown on the Now training tab (None = the latest save).
    now_pick: Option<String>,
    /// Timeline range on the Now training tab, and the network it belongs to.
    now_range: Option<(f64, f64)>,
    now_shown: Option<String>,
    /// Choose the opening view once the library has loaded.
    first_load: bool,
    /// Network diagram: colour lines by weight (false) or by change since the snapshot (true).
    net_changes: bool,
    /// Usage for one output only (index into the output neurons), or all.
    usage_output: Option<usize>,
    /// Write a "(best lap)" copy whenever a network in training sets a new best lap.
    keep_best: bool,
    /// What the keeper did this session (newest last).
    kept_log: Vec<(String, Color32)>,
    /// Lap-time target on the Now training tab (kept in memory only).
    target: Option<f64>,
    /// Projection drawn on the timeline (set while drawing the Now training chart).
    projection: Option<projection::Projection>,
    /// Extra generations of empty space after the last one (room for the projection).
    future: usize,
    /// Networks at their best lap with no copy, per library load (keyed by the library's address).
    needs_copy: (usize, std::collections::HashSet<String>),
    /// What the timeline draws.
    chart: ChartOpts,
    /// Reward setups table: show setups with fewer than 3 runs too.
    rewards_all: bool,
    /// Track names being typed (TrackId -> text), and a rename to apply next frame.
    name_drafts: std::collections::HashMap<String, String>,
    rename: Option<(String, String)>,
    rename_error: Option<String>,
    /// Feature evidence for the swap card, per library load and measure.
    features_cache: Option<(usize, Measure, Arc<Vec<designs::Feature>>)>,
}

/// Graph options (kept for the session).
#[derive(Clone, Copy, Debug, PartialEq)]
struct ChartOpts {
    bands: bool,
    stalls: bool,
    verdicts: bool,
    limits: bool,
    rewards: bool,
    best: bool,
    iqr: bool,
    projection: bool,
    milestones: bool,
    fork: bool,
    strips: bool,
    /// Rolling median over this many generations (1 = off).
    smooth: usize,
    /// Chart height multiplier.
    size: f32,
}

impl Default for ChartOpts {
    fn default() -> Self {
        ChartOpts { bands: true, stalls: true, verdicts: true, limits: true, rewards: true, best: true, iqr: true, projection: true, milestones: true, fork: true, strips: true, smooth: 1, size: 1.0 }
    }
}

/// Trailing rolling median over `w` generations (points keep their generation).
fn smooth(pts: &[(usize, f64)], w: usize) -> Vec<(usize, f64)> {
    if w <= 1 {
        return pts.to_vec();
    }
    let mut start = 0;
    let mut buf: Vec<f64> = Vec::new();
    pts.iter()
        .enumerate()
        .map(|(k, &(i, _))| {
            while pts[start].0 + w <= i {
                start += 1;
            }
            buf.clear();
            buf.extend(pts[start..=k].iter().map(|p| p.1));
            buf.sort_by(f64::total_cmp);
            (i, runs::quantile(&buf, 0.5))
        })
        .collect()
}

impl Lab {
    pub fn new(open: Option<&Path>) -> Self {
        let (dir, selected) = match open {
            Some(p) if p.is_file() => (
                p.parent().map(Path::to_path_buf).unwrap_or_default(),
                p.file_stem().map(|s| s.to_string_lossy().into_owned()),
            ),
            Some(p) => (p.to_path_buf(), None),
            None => (library::default_dir().unwrap_or_default(), None),
        };
        let mut lab = Lab {
            dir_input: dir.display().to_string(),
            dir,
            lib: None,
            loading: None,
            last_check: Instant::now(),
            selected,
            view: if open.is_some_and(|p| p.is_file()) { View::Network } else { View::Overview },
            metric: Metric::Score,
            selected_run: None,
            sort: (Col::Start, true),
            filters: Filters::default(),
            window: verdict::WINDOW,
            x_range: None,
            ov_track: None,
            ov_measure: Measure::Lap,
            now_pick: None,
            now_range: None,
            now_shown: None,
            first_load: open.is_none(),
            net_changes: false,
            usage_output: None,
            keep_best: true,
            kept_log: Vec::new(),
            target: None,
            projection: None,
            future: 0,
            needs_copy: (0, Default::default()),
            chart: ChartOpts::default(),
            rewards_all: false,
            name_drafts: Default::default(),
            rename: None,
            rename_error: None,
            features_cache: None,
        };
        lab.rescan(false);
        lab
    }

    /// Re-read the folder in the background. `fresh` drops the cache (e.g. a new folder).
    fn rescan(&mut self, fresh: bool) {
        if self.loading.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let dir = self.dir.clone();
        let prev = if fresh { None } else { self.lib.clone() };
        let window = self.window;
        let min_side = (window / 2).max(5);
        std::thread::spawn(move || {
            let _ = tx.send(library::scan(&dir, prev.as_deref(), window, min_side));
        });
        self.loading = Some(rx);
        self.last_check = Instant::now();
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if let Some((key, name)) = self.rename.take() {
            self.apply_rename(&key, &name);
        }
        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok(lib) => {
                    if self.selected.is_none() {
                        self.selected = lib.networks.iter().filter(|n| n.gens() > 0).max_by(|a, b| a.mtime.total_cmp(&b.mtime)).map(|n| n.file.clone());
                    }
                    // open on Now training when something is training, otherwise on the overview
                    if self.first_load {
                        self.first_load = false;
                        if advice::latest(&lib).and_then(|n| advice::advise(&lib, n)).is_some_and(|a| a.live) {
                            self.view = View::Now;
                        }
                    }
                    let lib = Arc::new(lib);
                    self.lib = Some(lib.clone());
                    self.loading = None;
                    if self.keep_best {
                        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
                        let (kept, errors) = keeper::run(&lib, now);
                        for k in &kept {
                            self.kept_log.push((
                                format!(
                                    "Saved \"{}\": {:.3} s on {} (gen {}){}",
                                    k.copy,
                                    k.seconds,
                                    k.track,
                                    k.generation,
                                    k.previous.map_or(String::new(), |p| format!(", was {p:.3} s"))
                                ),
                                HELPED,
                            ));
                        }
                        for e in errors {
                            self.kept_log.push((format!("Couldn't save a best-lap copy ({e}); will retry"), HURT));
                        }
                        if self.kept_log.len() > 6 {
                            let cut = self.kept_log.len() - 6;
                            self.kept_log.drain(..cut);
                        }
                        if !kept.is_empty() {
                            // pick up the new copies
                            self.rescan(false);
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
                Err(_) => self.loading = None,
            }
        }
        // live refresh: cheap mtime check, full re-read only of changed files
        if self.loading.is_none() && self.last_check.elapsed() >= REFRESH {
            self.last_check = Instant::now();
            if let Some(lib) = &self.lib {
                if library::changed(lib) {
                    self.rescan(false);
                }
            }
        }
        ctx.request_repaint_after(REFRESH);

        egui::Panel::left("lab_list").resizable(true).default_size(270.0).show(ui, |ui| self.list(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            let Some(lib) = self.lib.clone() else {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            };
            if lib.networks.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(format!("No .evolution saves in {}.\nSet the folder on the left.", lib.dir.display()))
                });
                return;
            }
            match self.view {
                View::Now => self.now_view(ui, &lib),
                View::Overview => self.overview_view(ui, &lib),
                View::Designs => self.designs_view(ui, &lib),
                View::Compare => self.compare_view(ui, &lib),
                View::Network => {
                    let net = self.selected.as_deref().and_then(|f| lib.by_file(f)).cloned();
                    match net {
                        Some(n) => self.network_view(ui, &lib, &n),
                        None => {
                            ui.centered_and_justified(|ui| ui.label("Pick a network on the left."));
                        }
                    }
                }
            }
        });
    }

    // ---------------------------------------------------------------- list

    fn list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.heading("Evolution Lab");
        ui.label(RichText::new("Reads your saves and never changes them. Its only write is the optional best-lap copy below.").small().weak());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.dir_input).desired_width(180.0))
                .on_hover_text("The game's evolution folder. Press Enter to use a different folder.");
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.dir = PathBuf::from(self.dir_input.trim());
                self.selected = None;
                self.rescan(true);
            }
            if ui.button("⟳").on_hover_text("Re-read the folder now (it also refreshes by itself every 5 s)").clicked() {
                self.rescan(false);
            }
            if self.loading.is_some() {
                ui.spinner();
            }
        });
        ui.checkbox(&mut self.keep_best, "Keep a best-lap copy automatically").on_hover_text(
            "When a network you're training saves a new best lap, the Lab writes a copy called \"<name> (best lap)\" next to it, and overwrites that copy only when the lap gets faster. It's the Lab's only write: your own saves are never changed. Load the copy in the game if the network gets slower later.",
        );
        for (msg, c) in self.kept_log.iter().rev().take(3) {
            ui.label(RichText::new(msg).small().color(*c));
        }
        ui.add_space(4.0);
        let live = self.lib.as_ref().and_then(|l| advice::latest(l).map(|n| advice::advise(l, n).is_some_and(|a| a.live))).unwrap_or(false);
        let now_text = if live { RichText::new("●  Now training").color(HELPED) } else { RichText::new("○  Now training") };
        if ui
            .selectable_label(self.view == View::Now, now_text)
            .on_hover_text("The network the game saved last, how it's doing, and suggested next settings")
            .clicked()
        {
            self.view = View::Now;
        }
        if ui
            .selectable_label(self.view == View::Overview, "⭐  What's worked best")
            .on_hover_text("Overview: the best-known settings for each training stage, overall and per track, and plain-English findings")
            .clicked()
        {
            self.view = View::Overview;
        }
        if ui
            .selectable_label(self.view == View::Designs, "🧠  Network designs")
            .on_hover_text("Inputs, outputs, layers and size: which designs learned fastest and lapped quickest")
            .clicked()
        {
            self.view = View::Designs;
        }
        if ui
            .selectable_label(self.view == View::Compare, "📊  Compare settings")
            .on_hover_text("Every network's runs side by side: which mutation rates and changes helped, at which stage")
            .clicked()
        {
            self.view = View::Compare;
        }
        ui.separator();
        let Some(lib) = self.lib.clone() else { return };
        let key = Arc::as_ptr(&lib) as usize;
        if self.needs_copy.0 != key {
            // only networks trained recently (within a day of the newest save) are at risk
            let newest = lib.networks.iter().map(|n| n.mtime).fold(0.0, f64::max);
            let set = lib
                .networks
                .iter()
                .filter(|n| newest - n.mtime < 86_400.0)
                .filter(|n| advice::best_lap(&lib, n).is_some_and(|b| b.backup == advice::Backup::Needed))
                .map(|n| n.file.clone())
                .collect();
            self.needs_copy = (key, set);
        }
        let total: usize = lib.networks.iter().map(|n| n.gens()).sum();
        let own: usize = lib.networks.iter().map(|n| n.gens().saturating_sub(n.fork.own_from)).sum();
        ui.label(RichText::new(format!("{} saves · {} generations ({} unique)", lib.networks.len(), thousands(total), thousands(own))).small().weak());
        ui.add_space(2.0);
        // roots newest first, forks nested under their parent
        let mut roots: Vec<usize> = (0..lib.networks.len()).filter(|&i| lib.networks[i].fork.parent.is_none()).collect();
        roots.sort_by(|&a, &b| lib.networks[b].mtime.total_cmp(&lib.networks[a].mtime));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for r in roots {
                self.list_row(ui, &lib, r, 0);
            }
        });
    }

    fn list_row(&mut self, ui: &mut egui::Ui, lib: &Library, i: usize, depth: usize) {
        let n = &lib.networks[i];
        let mut text = RichText::new(format!("{}{}", if depth > 0 { "↳ " } else { "" }, n.file));
        if n.gens() == 0 {
            text = text.weak();
        }
        let selected = self.view == View::Network && self.selected.as_deref() == Some(&n.file);
        let resp = ui
            .horizontal(|ui| {
                ui.add_space(depth as f32 * 14.0);
                let r = ui.selectable_label(selected, text);
                let info = if n.gens() == 0 {
                    "no history".to_string()
                } else if n.gens() == n.fork.own_from {
                    "copy".to_string()
                } else {
                    thousands(n.gens())
                };
                ui.label(RichText::new(info).small().weak());
                if n.error.is_some() {
                    ui.label(RichText::new("!").color(HURT));
                }
                if n.since_gain >= runs::STALL_GENS {
                    ui.label(RichText::new("⏸").color(LAP)).on_hover_text(format!("No gain for {} generations", n.since_gain));
                }
                if self.needs_copy.1.contains(&n.file) {
                    ui.label(RichText::new("🏁").color(LAP)).on_hover_text("At its best lap with no copy saved: save one in the game");
                }
                r
            })
            .inner;
        let resp = resp.on_hover_ui(|ui| {
            ui.label(format!("{} · {} · {} gens", format::vehicle_name(n.save.vehicle), n.save.shape_label(), n.gens()));
            ui.label(format!("Modified {}", ago(n.mtime)));
            if let Some(t) = fork_text(lib, n) {
                ui.label(t);
            }
            if let Some(e) = &n.error {
                ui.colored_label(HURT, e);
            }
        });
        if resp.clicked() {
            self.selected = Some(n.file.clone());
            self.view = View::Network;
            self.selected_run = None;
            self.x_range = None;
        }
        let mut kids: Vec<usize> = (0..lib.networks.len()).filter(|&k| lib.networks[k].fork.parent.map(|p| p.0) == Some(i)).collect();
        kids.sort_by(|&a, &b| lib.networks[a].mtime.total_cmp(&lib.networks[b].mtime));
        for k in kids {
            self.list_row(ui, lib, k, depth + 1);
        }
    }

    // ---------------------------------------------------------------- network view

    fn network_view(&mut self, ui: &mut egui::Ui, lib: &Library, n: &Network) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(&n.file);
            ui.label(
                RichText::new(format!(
                    "{} · {} · {} outputs · {} sensors · {} gens · modified {}",
                    format::vehicle_name(n.save.vehicle),
                    n.save.shape_label(),
                    n.save.outputs.len(),
                    n.save.sensors,
                    thousands(n.gens()),
                    ago(n.mtime)
                ))
                .weak(),
            );
        });
        ui.horizontal_wrapped(|ui| {
            if let Some(t) = fork_text(lib, n) {
                ui.label(RichText::new(format!("⑂ {t}")).color(Color32::from_rgb(180, 180, 230)));
            }
            if n.gens() > 0 {
                if n.since_gain >= runs::STALL_GENS {
                    ui.label(RichText::new(format!("⏸ No gain for {} generations", n.since_gain)).color(LAP).strong())
                        .on_hover_text("The median score hasn't beaten its best on this track and rewards by 1% for this long");
                } else {
                    ui.label(RichText::new(format!("Last gain {} gens ago", n.since_gain)).weak());
                }
                if let Some(k) = n.gens_to_verdict(lib.min_side) {
                    let mut line = format!("· {k} more gens before the last change can get a verdict");
                    let mut tip = "A change needs enough generations either side to tell its effect from noise".to_string();
                    if let Some(plan) = detectability::forward_plan(lib, n) {
                        if let Some(frag) = detectability::format_forward_header(&plan) {
                            line.push(' ');
                            line.push_str(&frag);
                        }
                        let det = detectability::format_forward(&plan);
                        if !det.is_empty() {
                            tip = det;
                        }
                    }
                    ui.label(RichText::new(line).weak()).on_hover_text(tip);
                }
            }
            if let Some(e) = &n.error {
                ui.colored_label(HURT, e);
            }
        });
        if n.gens() == 0 {
            ui.add_space(30.0);
            ui.centered_and_justified(|ui| ui.label("No training history yet."));
            return;
        }
        egui::Panel::right("lab_side").resizable(true).default_size(300.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.side_panel(ui, lib, n));
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.metric, Metric::Score, RichText::new("Score").color(SCORE));
            ui.selectable_value(&mut self.metric, Metric::Lap, RichText::new("Lap time").color(LAP));
            ui.selectable_value(&mut self.metric, Metric::Speed, RichText::new("Score/sec").color(SPEED))
                .on_hover_text("Median score ÷ generation time (thick) and best ÷ time (thin, the game's \"best average speed\"). Raising the time limit doesn't inflate it; with a distance-only reward it's average speed.");
            ui.separator();
            if let Some(k) = self.chart_options(ui, true) {
                self.x_range = Self::range_for(k, n.gens(), 0);
            }
        });
        ui.label(RichText::new("Bands = runs, darker = lower mutation · ▲ helped ▼ hurt ● can't tell · ⏱ time limit raised · ★ rewards changed · scroll to zoom, drag to pan, double-click to reset").small().weak());
        let h = (ui.available_height() * 0.5).clamp(180.0, 420.0) * self.chart.size;
        self.timeline(ui, lib, n, h);
        if self.chart.strips {
            self.strip(ui, n, "Survival", SURVIVAL, Gen::survival, Some((0.0, 1.0)), "Share of cars still driving when the generation ended");
            self.strip(ui, n, "Spread", SPREAD, Gen::spread, None, "(Q3 − Q1) ÷ median: falling = the population is converging");
        }
        ui.add_space(6.0);
        self.run_table(ui, lib, n);
    }

    /// The "Graph options" menu and quick zoom buttons. Returns a range to show: Some(None) = everything,
    /// Some(Some(k)) = the last k generations.
    fn chart_options(&mut self, ui: &mut egui::Ui, network_view: bool) -> Option<Option<usize>> {
        let mut pick = None;
        ui.menu_button("⚙ Graph options", |ui| {
            ui.set_min_width(230.0);
            let c = &mut self.chart;
            ui.label(RichText::new("Show").strong());
            ui.checkbox(&mut c.bands, "Run bands (settings, darker = lower mutation)");
            ui.checkbox(&mut c.stalls, "Stalls (hatched)");
            ui.checkbox(&mut c.verdicts, "Verdict markers ▲ ▼ ●");
            ui.checkbox(&mut c.limits, "Time-limit changes ⏱");
            ui.checkbox(&mut c.rewards, "Reward changes ★");
            ui.checkbox(&mut c.best, "Best line (Score, Score/sec)");
            ui.checkbox(&mut c.iqr, "Middle 50% band (Score)");
            ui.checkbox(&mut c.milestones, "Lap milestones (Lap time)");
            ui.checkbox(&mut c.projection, "Projection and target (Now training)");
            ui.checkbox(&mut c.fork, "Fork point");
            if network_view {
                ui.checkbox(&mut c.strips, "Survival and spread strips");
            }
            ui.separator();
            ui.label(RichText::new("Smoothing").strong());
            ui.horizontal(|ui| {
                for (k, t) in [(1, "Off"), (5, "5"), (10, "10"), (25, "25"), (50, "50")] {
                    ui.selectable_value(&mut c.smooth, k, t);
                }
            });
            ui.label(RichText::new("Rolling median of the main line over this many generations.").small().weak());
            ui.separator();
            ui.label(RichText::new("Height").strong());
            ui.horizontal(|ui| {
                for (k, t) in [(0.7, "Small"), (1.0, "Medium"), (1.5, "Large")] {
                    ui.selectable_value(&mut c.size, k, t);
                }
            });
            ui.separator();
            if ui.button("Reset options").clicked() {
                *c = ChartOpts::default();
            }
        });
        ui.label(RichText::new("Show").weak());
        for (k, t) in [(Some(100), "Last 100"), (Some(200), "200"), (Some(500), "500"), (Some(1000), "1,000"), (None, "All")] {
            if ui.small_button(t).clicked() {
                pick = Some(k);
            }
        }
        pick
    }

    fn range_for(pick: Option<usize>, gens: usize, future: usize) -> Option<(f64, f64)> {
        pick.map(|k| ((gens as f64 - k as f64).max(0.0), (gens + future) as f64))
    }

    fn view_range(&self, n: usize) -> (f64, f64) {
        let full = (0.0, (n + self.future).max(1) as f64);
        match self.x_range {
            Some((a, b)) if b - a >= 5.0 => (a.max(0.0), b.min(full.1)),
            _ => full,
        }
    }

    fn timeline(&mut self, ui: &mut egui::Ui, lib: &Library, n: &Network, height: f32) {
        let g = &n.save.gens;
        let w = ui.available_width();
        let (resp, painter) = ui.allocate_painter(Vec2::new(w, height), Sense::click_and_drag());
        let outer = resp.rect;
        let plot = Rect::from_min_max(outer.min + Vec2::new(54.0, 16.0), outer.max - Vec2::new(8.0, 18.0));
        painter.rect_filled(outer, 4.0, Color32::from_rgb(22, 24, 28));
        let (x0, x1) = self.view_range(g.len());
        // zoom & pan
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                // the chart takes the wheel: don't let the page scroll as well
                ui.input_mut(|i| {
                    i.smooth_scroll_delta = Vec2::ZERO;
                });
                if let Some(p) = resp.hover_pos() {
                    let at = x0 + ((p.x - plot.min.x) / plot.width()) as f64 * (x1 - x0);
                    let k = (-scroll as f64 / 300.0).exp();
                    let (a, b) = (at - (at - x0) * k, at + (x1 - at) * k);
                    self.x_range = Some((a.max(0.0), b.min((g.len() + self.future) as f64)));
                }
            }
        }
        if resp.dragged() {
            let dx = resp.drag_delta().x as f64 / plot.width() as f64 * (x1 - x0);
            let (a, b) = (x0 - dx, x1 - dx);
            let end = (g.len() + self.future) as f64;
            let shift = if a < 0.0 { -a } else if b > end { end - b } else { 0.0 };
            self.x_range = Some((a + shift, b + shift));
        }
        if resp.double_clicked() {
            self.x_range = None;
        }
        let (x0, x1) = self.view_range(g.len());
        let sx = |i: f64| plot.min.x + ((i - x0) / (x1 - x0)) as f32 * plot.width();
        let lo = x0.floor().max(0.0) as usize;
        let hi = (x1.ceil() as usize).min(g.len());

        let opts = self.chart;
        // run bands
        for (j, r) in n.runs.iter().enumerate().filter(|_| opts.bands) {
            if r.end as f64 <= x0 || r.start as f64 >= x1 {
                continue;
            }
            let rect = Rect::from_x_y_ranges(sx(r.start as f64).max(plot.min.x)..=sx(r.end as f64).min(plot.max.x), plot.y_range());
            painter.rect_filled(rect, 0.0, band_color(r.settings.mutation));
            if self.selected_run == Some(j) {
                painter.rect_stroke(rect, 0.0, Stroke::new(1.5, Color32::from_rgb(255, 230, 120)), egui::StrokeKind::Inside);
            }
        }
        // stalls, hatched
        for s in n.stalls.iter().filter(|_| opts.stalls) {
            if s.end as f64 <= x0 || s.start as f64 >= x1 {
                continue;
            }
            let (a, b) = (sx(s.start as f64).max(plot.min.x), sx(s.end as f64).min(plot.max.x));
            let hp = painter.with_clip_rect(Rect::from_x_y_ranges(a..=b, plot.y_range()));
            let lean = plot.height() * 0.4;
            let mut x = a;
            while x < b + lean {
                hp.line_segment([Pos2::new(x, plot.min.y), Pos2::new(x - lean, plot.max.y)], Stroke::new(1.0, STALL));
                x += 9.0;
            }
            painter.line_segment([Pos2::new(a, plot.max.y), Pos2::new(b, plot.max.y)], Stroke::new(3.0, LAP.gamma_multiply(0.6)));
        }
        // fork point
        if opts.fork && n.fork.own_from > 0 && (n.fork.own_from as f64) > x0 && (n.fork.own_from as f64) < x1 {
            let x = sx(n.fork.own_from as f64);
            painter.extend(Shape::dashed_line(&[Pos2::new(x, plot.min.y), Pos2::new(x, plot.max.y)], Stroke::new(1.5, Color32::from_rgb(180, 180, 230)), 5.0, 4.0));
            if let Some((p, _)) = n.fork.parent {
                painter.text(Pos2::new(x + 4.0, plot.max.y - 4.0), Align2::LEFT_BOTTOM, format!("forked from {}", lib.networks[p].file), FontId::proportional(11.0), Color32::from_rgb(180, 180, 230));
            }
        }

        // series and y-range
        let series = |f: &dyn Fn(&Gen) -> Option<f64>| -> Vec<(usize, f64)> { (lo..hi).filter_map(|i| f(&g[i]).map(|v| (i, v))).collect() };
        let (main, colour, band, extra): (Vec<(usize, f64)>, Color32, Vec<(usize, f64, f64)>, Vec<(usize, f64)>) = match self.metric {
            Metric::Score => (
                series(&|x| x.median),
                SCORE,
                (lo..hi).filter_map(|i| Some((i, g[i].q1?, g[i].q3?))).collect(),
                series(&|x| x.best),
            ),
            Metric::Lap => (series(&|x| x.best_lap), LAP, Vec::new(), Vec::new()),
            Metric::Speed => (series(&|x| x.rate()), SPEED, Vec::new(), series(&|x| x.best_speed)),
        };
        let main = smooth(&main, opts.smooth);
        let band = if opts.iqr { band } else { Vec::new() };
        let extra = if opts.best { smooth(&extra, opts.smooth) } else { Vec::new() };
        let show_proj = opts.projection;
        let mut ys: Vec<f64> = main.iter().map(|p| p.1).chain(band.iter().flat_map(|b| [b.1, b.2])).collect();
        // the best score can dwarf the median; let the top 2% of it clip
        let mut best_sorted: Vec<f64> = extra.iter().map(|p| p.1).collect();
        best_sorted.sort_by(f64::total_cmp);
        if !best_sorted.is_empty() {
            ys.push(runs::quantile(&best_sorted, 0.98));
        }
        // keep the projection's plateau and the target in view
        if self.metric == Metric::Lap && !ys.is_empty() && show_proj {
            if let Some(p) = &self.projection {
                if let Some((lo, _, _)) = p.plateau {
                    ys.push(lo);
                }
            }
            if let Some(t) = self.target {
                ys.push(t);
            }
        }
        if ys.is_empty() {
            painter.text(plot.center(), Align2::CENTER_CENTER, match self.metric {
                Metric::Lap => "No laps completed in this range",
                _ => "No data in this range",
            }, FontId::proportional(14.0), NEUTRAL);
        }
        let (mut y0, mut y1) = ys.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
        if !y0.is_finite() {
            (y0, y1) = (0.0, 1.0);
        }
        if y1 - y0 < 1e-9 {
            y1 = y0 + 1.0;
        }
        let pad = (y1 - y0) * 0.05;
        let (y0, y1) = (y0 - pad, y1 + pad);
        let sy = |v: f64| plot.max.y - ((v - y0) / (y1 - y0)) as f32 * plot.height();
        let clamp = |p: Pos2| Pos2::new(p.x, p.y.clamp(plot.min.y, plot.max.y));
        // axes
        for k in 0..=4 {
            let v = y0 + (y1 - y0) * k as f64 / 4.0;
            let y = sy(v);
            painter.line_segment([Pos2::new(plot.min.x, y), Pos2::new(plot.max.x, y)], Stroke::new(0.5, Color32::from_gray(50)));
            painter.text(Pos2::new(plot.min.x - 4.0, y), Align2::RIGHT_CENTER, if self.metric == Metric::Lap { format!("{v:.3}") } else { short_num(v) }, FontId::proportional(10.0), Color32::from_gray(140));
        }
        for t in ticks(x0, x1, 8) {
            let x = sx(t);
            painter.text(Pos2::new(x, plot.max.y + 3.0), Align2::CENTER_TOP, format!("{t:.0}"), FontId::proportional(10.0), Color32::from_gray(140));
        }
        if self.metric == Metric::Lap {
            painter.text(Pos2::new(plot.min.x + 4.0, plot.min.y + 2.0), Align2::LEFT_TOP, "best lap, seconds (lower is better)", FontId::proportional(10.0), LAP);
        }
        // IQR band
        for w2 in band.windows(2) {
            let (a, b) = (w2[0], w2[1]);
            if b.0 != a.0 + 1 {
                continue;
            }
            let pts = vec![
                clamp(Pos2::new(sx(a.0 as f64 + 0.5), sy(a.1))),
                clamp(Pos2::new(sx(b.0 as f64 + 0.5), sy(b.1))),
                clamp(Pos2::new(sx(b.0 as f64 + 0.5), sy(b.2))),
                clamp(Pos2::new(sx(a.0 as f64 + 0.5), sy(a.2))),
            ];
            painter.add(Shape::convex_polygon(pts, SCORE.gamma_multiply(0.18), Stroke::NONE));
        }
        let line = |pts: &[(usize, f64)], c: Color32, width: f32| {
            // break the line at gaps (e.g. generations without a lap)
            let mut seg: Vec<Pos2> = Vec::new();
            let mut last: Option<usize> = None;
            for &(i, v) in pts {
                if last.is_some_and(|l| i > l + 1 && self.metric != Metric::Lap) {
                    painter.line(std::mem::take(&mut seg), Stroke::new(width, c));
                }
                seg.push(clamp(Pos2::new(sx(i as f64 + 0.5), sy(v))));
                last = Some(i);
            }
            if seg.len() > 1 {
                painter.line(seg, Stroke::new(width, c));
            } else if let Some(p) = seg.first() {
                painter.circle_filled(*p, 2.0, c);
            }
        };
        line(&extra, SCORE.gamma_multiply(0.45), 1.0);
        line(&main, colour, 1.8);
        // time-limit steps (players raise it as a network learns more of the track)
        let limits = verdict::time_limits(g);
        let mut prev_limit: Option<f64> = None;
        let mut last_label_x = f32::NEG_INFINITY;
        for (i, lim) in limits.iter().enumerate().filter(|_| opts.limits) {
            if lim.is_some() && *lim != prev_limit {
                if prev_limit.is_some() && (i as f64) >= x0 && (i as f64) <= x1 {
                    let x = sx(i as f64);
                    painter.line_segment([Pos2::new(x, plot.max.y - 14.0), Pos2::new(x, plot.max.y)], Stroke::new(1.0, Color32::from_rgb(200, 200, 120)));
                    if x - last_label_x > 46.0 {
                        painter.text(Pos2::new(x + 2.0, plot.max.y - 2.0), Align2::LEFT_BOTTOM, format!("⏱{} s", fmt_opt(*lim)), FontId::proportional(9.0), Color32::from_rgb(200, 200, 120));
                        last_label_x = x;
                    }
                }
                prev_limit = *lim;
            }
        }
        // reward changes (scores before and after aren't on the same scale)
        if opts.rewards {
            for k in 1..n.runs.len() {
                let (a, b) = (&n.runs[k - 1], &n.runs[k]);
                if a.rewards == b.rewards || (b.start as f64) < x0 || (b.start as f64) > x1 {
                    continue;
                }
                let x = sx(b.start as f64);
                let c = Color32::from_rgb(230, 140, 220);
                painter.extend(Shape::dashed_line(&[Pos2::new(x, plot.min.y), Pos2::new(x, plot.max.y)], Stroke::new(1.0, c), 3.0, 3.0));
                painter.text(Pos2::new(x + 3.0, plot.min.y + 14.0), Align2::LEFT_TOP, "★ rewards", FontId::proportional(10.0), c);
            }
        }
        // projection: fitted curve over its window, dashed into the future, plateau and target lines
        if self.metric == Metric::Lap && show_proj {
            if let Some(p) = &self.projection {
                let end = (g.len() + self.future) as f64;
                let pts: Vec<Pos2> = (0..=120)
                    .map(|k| p.from as f64 + (end - p.from as f64) * k as f64 / 120.0)
                    .filter(|x| *x >= x0 && *x <= x1)
                    .map(|x| clamp(Pos2::new(sx(x + 0.5), sy(p.fit.predict(x)))))
                    .collect();
                if pts.len() > 1 {
                    painter.extend(Shape::dashed_line(&pts, Stroke::new(1.3, Color32::from_rgb(230, 230, 230)), 6.0, 4.0));
                }
                if let Some((_, mid, _)) = p.plateau {
                    let y = sy(mid);
                    if y >= plot.min.y && y <= plot.max.y {
                        let xa = sx(g.len() as f64).max(plot.min.x);
                        painter.extend(Shape::dashed_line(&[Pos2::new(xa, y), Pos2::new(plot.max.x, y)], Stroke::new(1.0, HELPED), 2.0, 3.0));
                        painter.text(Pos2::new(plot.max.x - 4.0, y - 2.0), Align2::RIGHT_BOTTOM, format!("levels off ≈ {mid:.3} s"), FontId::proportional(10.0), HELPED);
                    }
                }
            }
            if let Some(t) = self.target {
                let y = sy(t);
                if y >= plot.min.y && y <= plot.max.y {
                    painter.extend(Shape::dashed_line(&[Pos2::new(plot.min.x, y), Pos2::new(plot.max.x, y)], Stroke::new(1.0, Color32::from_rgb(110, 170, 255)), 8.0, 4.0));
                    painter.text(Pos2::new(plot.min.x + 4.0, y - 2.0), Align2::LEFT_BOTTOM, format!("target {t:.3} s"), FontId::proportional(10.0), Color32::from_rgb(110, 170, 255));
                }
            }
        }
        if self.metric == Metric::Lap && opts.milestones {
            for (k, &(i, l)) in n.lap_milestones.iter().enumerate() {
                if (i as f64) < x0 || (i as f64) > x1 {
                    continue;
                }
                let p = clamp(Pos2::new(sx(i as f64 + 0.5), sy(l)));
                painter.add(Shape::convex_polygon(vec![p + Vec2::new(0.0, -4.0), p + Vec2::new(4.0, 0.0), p + Vec2::new(0.0, 4.0), p + Vec2::new(-4.0, 0.0)], LAP, Stroke::NONE));
                if k == 0 {
                    painter.text(p + Vec2::new(5.0, -5.0), Align2::LEFT_BOTTOM, format!("first lap {l:.3} s"), FontId::proportional(10.0), LAP);
                }
            }
        }
        // verdict markers along the top
        let mut marker_hit: Option<&Verdict> = None;
        let hover = resp.hover_pos();
        for v in n.verdicts.iter().filter(|_| opts.verdicts) {
            if (v.at as f64) < x0 || (v.at as f64) > x1 {
                continue;
            }
            let x = sx(v.at as f64);
            let y = outer.min.y + 8.0;
            match v.outcome {
                Outcome::Helped => painter.add(Shape::convex_polygon(vec![Pos2::new(x, y - 5.0), Pos2::new(x + 5.0, y + 4.0), Pos2::new(x - 5.0, y + 4.0)], HELPED, Stroke::NONE)),
                Outcome::Hurt => painter.add(Shape::convex_polygon(vec![Pos2::new(x - 5.0, y - 4.0), Pos2::new(x + 5.0, y - 4.0), Pos2::new(x, y + 5.0)], HURT, Stroke::NONE)),
                _ if v.testable() => painter.circle_stroke(Pos2::new(x, y), 3.0, Stroke::new(1.2, NEUTRAL)),
                _ => painter.line_segment([Pos2::new(x, y - 2.0), Pos2::new(x, y + 2.0)], Stroke::new(1.0, Color32::from_gray(90))),
            };
            if let Some(h) = hover {
                if (h.x - x).abs() < 5.0 && (h.y - y).abs() < 9.0 {
                    marker_hit = Some(v);
                }
            }
        }
        // hover
        if let Some(h) = hover.filter(|h| plot.expand2(Vec2::new(0.0, 16.0)).contains(*h)) {
            let i = (x0 + ((h.x - plot.min.x) / plot.width()) as f64 * (x1 - x0)).floor().clamp(0.0, (g.len() - 1) as f64) as usize;
            painter.line_segment([Pos2::new(h.x, plot.min.y), Pos2::new(h.x, plot.max.y)], Stroke::new(0.8, Color32::from_gray(160)));
            let run_idx = n.runs.iter().position(|r| i >= r.start && i < r.end);
            let resp2 = resp.clone();
            if let Some(v) = marker_hit {
                resp2.on_hover_ui_at_pointer(|ui| verdict_tooltip(ui, v, lib));
            } else {
                resp2.on_hover_ui_at_pointer(|ui| {
                    gen_tooltip(ui, i, &g[i]);
                    if let Some(j) = run_idx {
                        ui.separator();
                        run_summary(ui, &n.runs[j]);
                    }
                });
            }
            if resp.clicked() {
                self.selected_run = run_idx;
            }
        }
    }

    fn strip(&self, ui: &mut egui::Ui, n: &Network, label: &str, c: Color32, f: fn(&Gen) -> Option<f64>, fixed: Option<(f64, f64)>, help: &str) {
        let g = &n.save.gens;
        let w = ui.available_width();
        let (resp, painter) = ui.allocate_painter(Vec2::new(w, 34.0), Sense::hover());
        let r = resp.rect;
        let plot = Rect::from_min_max(r.min + Vec2::new(54.0, 3.0), r.max - Vec2::new(8.0, 3.0));
        painter.rect_filled(r, 2.0, Color32::from_rgb(22, 24, 28));
        painter.text(Pos2::new(r.min.x + 4.0, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(10.0), c);
        let (x0, x1) = self.view_range(g.len());
        let lo = x0.floor().max(0.0) as usize;
        let hi = (x1.ceil() as usize).min(g.len());
        let pts: Vec<(usize, f64)> = (lo..hi).filter_map(|i| f(&g[i]).map(|v| (i, v))).collect();
        let (y0, y1) = fixed.unwrap_or_else(|| {
            let mut v: Vec<f64> = pts.iter().map(|p| p.1).collect();
            v.sort_by(f64::total_cmp);
            if v.is_empty() { (0.0, 1.0) } else { (runs::quantile(&v, 0.02).min(0.0), runs::quantile(&v, 0.98).max(1e-6)) }
        });
        let line: Vec<Pos2> = pts
            .iter()
            .map(|&(i, v)| {
                let x = plot.min.x + ((i as f64 + 0.5 - x0) / (x1 - x0)) as f32 * plot.width();
                let y = plot.max.y - (((v - y0) / (y1 - y0)) as f32).clamp(0.0, 1.0) * plot.height();
                Pos2::new(x, y)
            })
            .collect();
        if line.len() > 1 {
            painter.line(line, Stroke::new(1.2, c));
        }
        if let Some(h) = resp.hover_pos() {
            let i = (x0 + ((h.x - plot.min.x) / plot.width()) as f64 * (x1 - x0)).floor().clamp(0.0, (g.len().max(1) - 1) as f64) as usize;
            painter.line_segment([Pos2::new(h.x, plot.min.y), Pos2::new(h.x, plot.max.y)], Stroke::new(0.8, Color32::from_gray(160)));
            let val = g.get(i).and_then(f);
            resp.on_hover_ui_at_pointer(|ui| {
                ui.label(format!("Gen {i}: {label} {}", val.map_or("–".into(), |v| format!("{:.0}%", v * 100.0))));
                ui.label(RichText::new(help).small().weak());
            });
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, lib: &Library, n: &Network) {
        ui.add_space(4.0);
        if let Some(a) = advice::advise(lib, n) {
            ui.strong("Suggested next");
            advice_body(ui, &a, false);
            if ui.small_button("Open the overview").clicked() {
                self.view = View::Overview;
                self.ov_track = Some(a.track.clone());
            }
            ui.separator();
        }
        ui.strong("Settings changes");
        let testable = n.verdicts.iter().filter(|v| v.testable()).count();
        let helped = n.verdicts.iter().filter(|v| v.outcome == Outcome::Helped).count();
        let hurt = n.verdicts.iter().filter(|v| v.outcome == Outcome::Hurt).count();
        ui.label(format!("{} changes, {} testable", n.verdicts.len(), testable));
        ui.horizontal(|ui| {
            ui.colored_label(HELPED, format!("▲ {helped} helped"));
            ui.colored_label(HURT, format!("▼ {hurt} hurt"));
            ui.colored_label(NEUTRAL, format!("● {} can't tell", n.verdicts.len() - helped - hurt));
        });
        ui.label(
            RichText::new(format!(
                "A change is clear only if its effect beats {:.1}, the level that random points inside unchanged runs reach 5% of the time ({} points{}). Reward changes are judged on lap time instead, against {:.1}.",
                lib.threshold,
                lib.placebo_points,
                if lib.placebo_points < verdict::MIN_PLACEBO { ", too few to calibrate, so a default is used" } else { "" },
                lib.lap_threshold
            ))
            .small()
            .weak(),
        );
        let clear: Vec<&Verdict> = n.verdicts.iter().filter(|v| matches!(v.outcome, Outcome::Helped | Outcome::Hurt)).collect();
        for v in clear {
            let c = if v.outcome == Outcome::Helped { HELPED } else { HURT };
            let r = ui.add(egui::Label::new(RichText::new(format!("gen {} · {}", v.at, v.change)).color(c)).sense(Sense::click()));
            if r.on_hover_ui(|ui| verdict_tooltip(ui, v, lib)).clicked() {
                self.selected_run = Some(v.run);
                let r = &n.runs[v.run];
                self.x_range = Some(((r.start as f64 - 60.0).max(0.0), (r.start as f64 + 60.0).min(n.gens() as f64)));
            }
        }
        ui.separator();
        ui.strong("Comparable runs");
        let (groups, stage) = compare::comparable(lib, n, 5);
        match stage {
            None => {
                ui.label(RichText::new("The latest run's stage is unknown.").weak());
            }
            Some(st) => {
                ui.label(RichText::new(format!("Other networks' runs at the same stage ({st}), vehicle and reward metrics. Evidence, not advice.")).small().weak());
                if groups.is_empty() {
                    ui.label(RichText::new("Not enough comparable runs yet (need 5 per group).").weak());
                }
                for gr in groups {
                    ui.label(&gr.label);
                    ui.label(
                        RichText::new(format!(
                            "   n={} · median {} per 10 gens (IQR {} to {}) · {} of {} testable changes clearly helped",
                            gr.gains.len(),
                            pct(Some(runs::quantile(&gr.gains, 0.5))),
                            pct(Some(runs::quantile(&gr.gains, 0.25))),
                            pct(Some(runs::quantile(&gr.gains, 0.75))),
                            gr.clear_helped,
                            gr.testable
                        ))
                        .small()
                        .weak(),
                    );
                }
            }
        }
        let switches = compare::track_switches(n);
        if !switches.is_empty() {
            ui.separator();
            ui.strong("Track switches");
            ui.label(RichText::new("Last 10 gens on the old track against the first 10 on the new one.").small().weak());
            for s in switches {
                let surv = match s.survival {
                    (Some(a), Some(b)) => format!("survival {:.0}% → {:.0}%", a * 100.0, b * 100.0),
                    _ => "survival –".into(),
                };
                let lap = match s.lap {
                    (Some(a), Some(b)) => format!(" · lap {a:.3} s → {b:.3} s"),
                    (Some(a), None) => format!(" · lap {a:.3} s → none yet"),
                    _ => String::new(),
                };
                let r = ui.add(egui::Label::new(format!("gen {}: {surv}{lap}", s.at)).sense(Sense::click()));
                if r.clicked() {
                    self.x_range = Some(((s.at as f64 - 60.0).max(0.0), (s.at as f64 + 60.0).min(n.gens() as f64)));
                }
            }
        }
    }

    fn run_table(&mut self, ui: &mut egui::Ui, lib: &Library, n: &Network) {
        let cols: [(Col, &str, f32, &str); 12] = [
            (Col::Start, "Start", 50.0, "First generation of the run"),
            (Col::Gens, "Gens", 42.0, "Generations in the run"),
            (Col::Mutation, "Mutation", 62.0, "Mutation rate"),
            (Col::Selection, "Selection", 96.0, "Selection algorithm · parents kept"),
            (Col::Crossover, "Crossover", 80.0, ""),
            (Col::Population, "Pop", 46.0, "Population size"),
            (Col::Median, "Median start → end", 128.0, "Median score over the first and last 5 generations"),
            (Col::Gain, "Gain/10", 62.0, "Median score change per 10 generations, % of the start"),
            (Col::LapGain, "Lap/10", 58.0, "Best-lap reduction per 10 generations, %"),
            (Col::PerMin, "Gain/min", 62.0, "Median score change per simulated minute"),
            (Col::Stability, "Wobble", 54.0, "Standard deviation of the median within the run, % of its mean. High = thrashing"),
            (Col::Stage, "Stage", 48.0, "How trained the network was at the start of the run: early under 200 generations, mid under 1,000, late under 5,000, veteran after"),
        ];
        let mut order: Vec<usize> = (0..n.runs.len()).collect();
        let key = |r: &Run| -> f64 {
            match self.sort.0 {
                Col::Start => r.start as f64,
                Col::Gens => r.len() as f64,
                Col::Mutation => r.settings.mutation.unwrap_or(-1.0),
                Col::Selection => r.settings.selection.unwrap_or(9) as f64 * 1e4 + r.settings.selection_size.unwrap_or(0) as f64,
                Col::Crossover => r.settings.crossover.unwrap_or(9) as f64,
                Col::Population => r.settings.population.unwrap_or(0) as f64,
                Col::Median => r.end_median.unwrap_or(f64::NEG_INFINITY),
                Col::Gain => r.score_gain.unwrap_or(f64::NEG_INFINITY),
                Col::LapGain => r.lap_gain.unwrap_or(f64::NEG_INFINITY),
                Col::PerMin => r.gain_per_min.unwrap_or(f64::NEG_INFINITY),
                Col::Stability => r.stability.unwrap_or(f64::NEG_INFINITY),
                Col::Stage => r.stage.unwrap_or(-9.0),
            }
        };
        order.sort_by(|&a, &b| {
            let o = key(&n.runs[a]).total_cmp(&key(&n.runs[b]));
            if self.sort.1 { o } else { o.reverse() }
        });
        let verdict_of: std::collections::HashMap<usize, &Verdict> = n.verdicts.iter().map(|v| (v.run, v)).collect();
        let row_h = 18.0;
        ui.horizontal(|ui| {
            ui.strong(format!("Runs ({})", n.runs.len()));
            ui.label(RichText::new("click a header to sort, a row to highlight it").small().weak());
        });
        egui::ScrollArea::horizontal().id_salt("runs_h").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (c, name, w, tip) in cols {
                    let arrow = if self.sort.0 == c { if self.sort.1 { " ▲" } else { " ▼" } } else { "" };
                    let r = ui.add_sized([w, row_h], egui::Button::new(RichText::new(format!("{name}{arrow}")).small().strong()).frame(false));
                    let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
                    if r.clicked() {
                        self.sort = if self.sort.0 == c { (c, !self.sort.1) } else { (c, c == Col::Start) };
                    }
                }
                ui.add_sized([150.0, row_h], egui::Label::new(RichText::new("Change into this run").small().strong()));
                ui.add_sized([220.0, row_h], egui::Label::new(RichText::new("Rewards").small().strong()));
            });
            egui::ScrollArea::vertical().id_salt("runs_v").auto_shrink([false, false]).show_rows(ui, row_h, order.len(), |ui, range| {
                for &j in &order[range] {
                    let r = &n.runs[j];
                    let cells = [
                        r.start.to_string(),
                        r.len().to_string(),
                        fmt_opt(r.settings.mutation),
                        format!("{}·{}", format::selection_name(r.settings.selection), r.settings.selection_size.map_or("?".into(), |x| x.to_string())),
                        format::crossover_name(r.settings.crossover).to_string(),
                        r.settings.population.map_or("?".into(), |x| x.to_string()),
                        format!("{} → {}", r.start_median.map_or("–".into(), short_num), r.end_median.map_or("–".into(), short_num)),
                        pct(r.score_gain),
                        pct(r.lap_gain),
                        r.gain_per_min.map_or("–".into(), |x| format!("{x:+.2}")),
                        r.stability.map_or("–".into(), |x| format!("{:.0}%", x * 100.0)),
                        r.stage_label().to_string(),
                    ];
                    let row = ui.horizontal(|ui| {
                        for (k, (_, _, w, _)) in cols.iter().enumerate() {
                            let mut t = RichText::new(&cells[k]).small();
                            if k == 7 {
                                if let Some(gn) = r.score_gain {
                                    t = t.color(if gn > 0.0 { HELPED } else if gn < 0.0 { HURT } else { NEUTRAL });
                                }
                            }
                            ui.add_sized([*w, row_h], egui::Label::new(t).truncate());
                        }
                        match verdict_of.get(&j) {
                            Some(v) => {
                                let c = match v.outcome {
                                    Outcome::Helped => HELPED,
                                    Outcome::Hurt => HURT,
                                    _ => NEUTRAL,
                                };
                                let short = match v.outcome {
                                    Outcome::Helped => "▲ helped".to_string(),
                                    Outcome::Hurt => "▼ hurt".to_string(),
                                    Outcome::CantTell(verdict::Reason::TooShort(..)) => "too short".to_string(),
                                    Outcome::CantTell(verdict::Reason::RewardsChanged) => "rewards changed, no laps to judge by".to_string(),
                                    Outcome::CantTell(verdict::Reason::TimeLimitChanged) => "time limit changed".to_string(),
                                    Outcome::CantTell(verdict::Reason::TrackChanged) => "track changed".to_string(),
                                    Outcome::CantTell(_) => format!("● can't tell (z {:+.1})", v.z.unwrap_or(0.0)),
                                };
                                let mut tip = format!("{}: {}", v.change, v.label());
                                if let Some(r) = detectability::retrospective(lib, v) {
                                    tip.push_str(". ");
                                    tip.push_str(&detectability::format_retrospective(&r));
                                }
                                ui.add_sized([150.0, row_h], egui::Label::new(RichText::new(short).small().color(c)).truncate()).on_hover_text(tip);
                            }
                            None => {
                                ui.add_sized([150.0, row_h], egui::Label::new(RichText::new("first run").small().weak()));
                            }
                        }
                        ui.add_sized([220.0, row_h], egui::Label::new(RichText::new(format::rewards_label(&r.rewards)).small().weak()).truncate());
                    });
                    let resp = ui.interact(row.response.rect, ui.scope_id().with(("run", j)), Sense::click());
                    if self.selected_run == Some(j) {
                        ui.painter().rect_filled(row.response.rect, 2.0, Color32::from_rgba_premultiplied(60, 55, 20, 40));
                    } else if resp.hovered() {
                        ui.painter().rect_filled(row.response.rect, 2.0, Color32::from_rgba_premultiplied(40, 40, 40, 30));
                    }
                    if resp.clicked() {
                        self.selected_run = Some(j);
                        let span = (r.len() as f64 * 3.0).max(60.0);
                        self.x_range = Some(((r.start as f64 - span / 2.0).max(0.0), (r.end as f64 + span / 2.0).min(n.gens() as f64)));
                    }
                }
            });
        });
    }

    // ---------------------------------------------------------------- now training

    fn now_view(&mut self, ui: &mut egui::Ui, lib: &Library) {
        let latest = advice::latest(lib).map(|n| n.file.clone());
        let file = self.now_pick.clone().or(latest.clone());
        let Some(n) = file.as_deref().and_then(|f| lib.by_file(f)).cloned() else {
            ui.centered_and_justified(|ui| ui.label("No saves with training history yet."));
            return;
        };
        // a different network (e.g. the game started training another one): reset the chart range
        if self.now_shown.as_deref() != Some(n.file.as_str()) {
            self.now_shown = Some(n.file.clone());
            self.now_range = None;
        }
        let Some(a) = advice::advise(lib, &n) else {
            ui.label("This network has no history yet.");
            return;
        };
        // header
        ui.horizontal_wrapped(|ui| {
            if a.live {
                ui.label(RichText::new("●").size(26.0).color(HELPED)).on_hover_text("Saved in the last 15 minutes");
                ui.label(RichText::new("Now training").size(26.0));
            } else {
                ui.label(RichText::new(if self.now_pick.is_some() { "Network" } else { "Last trained" }).size(26.0));
            }
            ui.label(RichText::new(&a.file).size(26.0).strong());
            ui.label(RichText::new(format!("on {} · gen {} · saved {}", a.track_name, thousands(n.gens()), ago(a.mtime))).size(16.0).weak());
        });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("now_pick")
                .width(260.0)
                .selected_text(match &self.now_pick {
                    None => format!("Latest save ({})", latest.as_deref().unwrap_or("-")),
                    Some(f) => f.clone(),
                })
                .show_ui(ui, |ui| {
                    if ui.selectable_label(self.now_pick.is_none(), "Latest save (follows the game)").clicked() {
                        self.now_pick = None;
                        self.now_range = None;
                    }
                    let mut nets: Vec<&Arc<Network>> = lib.networks.iter().filter(|n| n.gens() > n.fork.own_from).collect();
                    nets.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
                    for m in nets {
                        if ui.selectable_label(self.now_pick.as_deref() == Some(&m.file), &m.file).clicked() {
                            self.now_pick = Some(m.file.clone());
                            self.now_range = None;
                        }
                    }
                });
            if ui.button("Open full history").clicked() {
                self.open_network(&n.file.clone(), None, n.gens());
            }
            if ui.button(format!("What worked on {}", a.track_name)).clicked() {
                self.view = View::Overview;
                self.ov_track = Some(a.track.clone());
            }
        });
        ui.add_space(8.0);
        // is the best lap backed up?
        if let Some(b) = advice::best_lap(lib, &n) {
            let (colour, text) = match &b.backup {
                advice::Backup::Needed => (
                    LAP,
                    format!(
                        "🏁 Save a copy now: best lap on {} is {:.3} s (gen {}) and it's only in this save. In the game, save the network under a new name, e.g. \"{} best\", then keep training. If it gets slower later you can load the copy.",
                        b.track_name, b.seconds, b.at, n.file
                    ),
                ),
                advice::Backup::Safe { copy } => (HELPED, format!("✔ Best lap on {} ({:.3} s, gen {}) is safe in \"{}\".", b.track_name, b.seconds, b.at, copy)),
                advice::Backup::Lost { snapshot } => (
                    HURT,
                    format!(
                        "⚠ Slower than its best lap on {} ({:.3} s at gen {}, now {}) and no copy was saved.{} Save a copy next time it sets a best.",
                        b.track_name,
                        b.seconds,
                        b.at,
                        b.recent.map_or("no laps".into(), |r| format!("{r:.3} s")),
                        snapshot.map_or(String::new(), |sn| format!(" The closest snapshot in the save is from gen {sn}."))
                    ),
                ),
            };
            egui::Frame::group(ui.style()).stroke(Stroke::new(1.5, colour)).inner_margin(10.0).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(RichText::new(text).size(15.0).color(colour));
            });
            ui.add_space(8.0);
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // status tiles
            let stalled = a.since_gain >= runs::STALL_GENS;
            let lap_tile = match a.lap {
                (Some(now), Some(before)) => (format!("{now:.3} s"), format!("{:+.3} s in the last 20 gens", now - before)),
                (Some(now), None) => (format!("{now:.3} s"), "best lap on this track".into()),
                _ => {
                    let last = n.save.gens.last();
                    (
                        last.and_then(Gen::rate).map_or("–".into(), |r| format!("{}/s", short_num(r))),
                        format!(
                            "median score per second (score {} in {} s, no laps yet)",
                            last.and_then(|g| g.median).map_or("–".into(), short_num),
                            last.and_then(|g| g.time).map_or("–".into(), |t| format!("{t:.0}"))
                        ),
                    )
                }
            };
            let tiles: [(&str, String, String, Option<Color32>); 4] = [
                (
                    "Stage",
                    capitalise(a.stage.map_or("not yet known", |s| STAGES[s])),
                    format!(
                        "{} gens trained · early < {}, mid < {}, late < {}, veteran after",
                        thousands(n.gens()),
                        runs::MID_GENS,
                        thousands(runs::LATE_GENS),
                        thousands(runs::VETERAN_GENS)
                    ),
                    None,
                ),
                (if a.lap.0.is_some() { "Best lap" } else { "Score/sec" }, lap_tile.0, lap_tile.1, Some(LAP)),
                (
                    "Last real gain",
                    if stalled { format!("Stalled {} gens", a.since_gain) } else { format!("{} gens ago", a.since_gain) },
                    if stalled { "time to change something".into() } else { "score or lap still improving".into() },
                    Some(if stalled { HURT } else { HELPED }),
                ),
                (
                    "Current settings",
                    format!("mutation {}", fmt_opt(a.run.settings.mutation)),
                    format!(
                        "{}·{} · {} · pop {} · for {} gens",
                        format::selection_name(a.run.settings.selection),
                        a.run.settings.selection_size.map_or("?".into(), |x| x.to_string()),
                        format::crossover_name(a.run.settings.crossover),
                        a.run.settings.population.map_or("?".into(), |x| x.to_string()),
                        a.run.len()
                    ),
                    None,
                ),
            ];
            ui.columns(4, |cols| {
                for ((label, big, small, colour), ui) in tiles.iter().zip(cols.iter_mut()) {
                    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(RichText::new(*label).weak());
                        let t = RichText::new(big).size(22.0).strong();
                        ui.label(match colour {
                            Some(c) => t.color(*c),
                            None => t,
                        });
                        ui.label(RichText::new(small).small());
                    });
                }
            });
            // rewards: what the network is being scored on, and when that changed
            let changes: Vec<(usize, &Vec<format::Reward>)> = n
                .runs
                .iter()
                .enumerate()
                .filter(|(k, r)| *k == 0 || n.runs[k - 1].rewards != r.rewards)
                .map(|(_, r)| (r.start, &r.rewards))
                .collect();
            egui::Frame::group(ui.style()).inner_margin(8.0).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Rewards").weak());
                    if a.run.rewards.is_empty() {
                        ui.label(RichText::new("none switched on").weak());
                    }
                    for w in &a.run.rewards {
                        egui::Frame::new().fill(Color32::from_gray(45)).corner_radius(4.0).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
                            ui.label(RichText::new(format::rewards_label(std::slice::from_ref(w))).strong());
                        });
                    }
                    let limit = verdict::time_limits(&n.save.gens).last().copied().flatten();
                    if let Some(t) = limit {
                        ui.label(RichText::new(format!("· time limit {} s", fmt_opt(Some(t)))).weak());
                    }
                    if let Some((at, _)) = changes.last() {
                        ui.label(RichText::new(if *at == 0 { "· unchanged since gen 0".to_string() } else { format!("· since gen {at}") }).weak());
                    }
                });
                if changes.len() > 1 {
                    ui.collapsing(format!("Reward history ({} setups)", changes.len()), |ui| {
                        for (k, (at, r)) in changes.iter().enumerate() {
                            let end = changes.get(k + 1).map_or(n.gens(), |c| c.0);
                            let best = n.save.gens[*at..end].iter().filter_map(|g| g.best_lap).reduce(f64::min);
                            ui.label(
                                RichText::new(format!(
                                    "gens {at}–{}: {}{}",
                                    end - 1,
                                    format::rewards_label(r),
                                    best.map_or(String::new(), |b| format!(" · best lap {b:.3} s"))
                                ))
                                .small(),
                            );
                        }
                        ui.label(RichText::new("Scores aren't comparable across reward setups (the rewards set the scale); lap times are.").small().weak());
                    });
                }
            });
            ui.add_space(10.0);

            // lap-time projection
            let proj = projection::project(&n);
            if let Some(p) = &proj {
                egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(RichText::new("Where the lap time is heading").size(17.0).strong());
                    ui.label(
                        RichText::new(format!(
                            "Fitted to the best lap over gens {}–{}, at the settings used then. A settings change can move it either way.",
                            p.from,
                            p.to - 1
                        ))
                        .small()
                        .weak(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        match p.plateau {
                            Some((lo, mid, hi)) => {
                                ui.label(RichText::new(format!("Levels off around {mid:.3} s")).size(16.0).color(HELPED));
                                ui.label(RichText::new(format!("(range {lo:.3}–{hi:.3} s, {:.3} s below your best)", (p.best - mid).max(0.0))).weak());
                            }
                            None => {
                                ui.label(if p.per_100 > 0.0 {
                            RichText::new("Getting slower lately: no plateau to project").size(16.0).color(HURT)
                        } else {
                            RichText::new("No plateau in sight yet").size(16.0).color(HELPED)
                        });
                            }
                        }
                        ui.label(RichText::new(format!("· recent rate {:+.3} s per 100 gens", p.per_100)).weak());
                    });
                    egui::Grid::new("milestones").spacing([18.0, 3.0]).show(ui, |ui| {
                        for d in [0.05, 0.1, 0.25, 0.5] {
                            let t = p.best - d;
                            ui.label(format!("−{d:.2} s → {t:.3} s"));
                            ui.label(match p.gens_to(t) {
                                Some(g) => RichText::new(format!("about {g:.0} gens")),
                                None => RichText::new("not at this rate").color(HURT),
                            });
                            ui.end_row();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Your target:");
                        let mut on = self.target.is_some();
                        if ui.checkbox(&mut on, "").changed() {
                            self.target = on.then(|| ((p.best - 0.2) * 1000.0).round() / 1000.0);
                        }
                        if let Some(t) = &mut self.target {
                            ui.add(egui::DragValue::new(t).speed(0.001).max_decimals(3).min_decimals(3).suffix(" s"));
                            let t = *t;
                            ui.label(match p.gens_to(t) {
                                Some(g) if g < 0.5 => RichText::new("already beaten").color(HELPED),
                                Some(g) => RichText::new(format!("about {g:.0} gens at this rate")),
                                None => RichText::new("not reachable at this rate: something needs to change").color(HURT),
                            });
                        } else {
                            ui.label(RichText::new("tick to set one (e.g. a leaderboard time)").small().weak());
                        }
                    });
                });
                ui.add_space(10.0);
            }

            // suggestions
            ui.label(RichText::new("Suggested next").size(18.0).strong());
            ui.label(RichText::new("From your saves only, most important first. Nothing is changed in the game.").weak());
            ui.add_space(4.0);
            // the backup prompt is the banner above
            for s in a.suggestions.iter().filter(|s| !s.text.starts_with("Save a copy") && !s.text.starts_with("This network has got slower")) {
                egui::Frame::group(ui.style()).inner_margin(8.0).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        badge(ui, s.strength);
                        let t = RichText::new(&s.text).size(15.0);
                        ui.label(if s.text.starts_with("Stalled") { t.color(HURT) } else { t });
                    });
                    ui.label(RichText::new(&s.why).small().weak());
                });
            }
            ui.add_space(10.0);

            // recent progress
            let mut pick = None;
            ui.horizontal(|ui| {
                ui.label(RichText::new(if a.measure == Measure::Lap { "Lap time, recent generations" } else { "Score per second, recent generations" }).size(18.0).strong());
                ui.separator();
                pick = self.chart_options(ui, false);
            });
            if let Some(k) = pick {
                self.now_range = Self::range_for(k, n.gens(), if proj.is_some() { 60 } else { 0 });
                if k.is_none() {
                    self.now_range = Some((0.0, (n.gens() + if proj.is_some() { 60 } else { 0 }) as f64));
                }
            }
            let (old_metric, old_range, old_run) = (self.metric, self.x_range, self.selected_run);
            self.metric = if a.measure == Measure::Lap { Metric::Lap } else { Metric::Speed };
            self.x_range = self.now_range.or(Some(((n.gens() as f64 - 200.0).max(0.0), n.gens() as f64)));
            self.selected_run = Some(n.runs.len() - 1);
            self.projection = proj.clone();
            self.future = if proj.is_some() { 60 } else { 0 };
            if self.now_range.is_none() && proj.is_some() {
                self.x_range = Some(((n.gens() as f64 - 200.0).max(0.0), (n.gens() + 60) as f64));
            }
            self.timeline(ui, lib, &n, 260.0 * self.chart.size);
            self.projection = None;
            self.future = 0;
            self.now_range = self.x_range;
            (self.metric, self.x_range, self.selected_run) = (old_metric, old_range, old_run);

            ui.add_space(12.0);
            self.inside_network(ui, &n);
        });
    }

    // ---------------------------------------------------------------- swap card

    fn features_for(&mut self, lib: &Arc<Library>, m: Measure) -> Arc<Vec<designs::Feature>> {
        let key = Arc::as_ptr(lib) as usize;
        if let Some((k, mm, f)) = &self.features_cache {
            if *k == key && *mm == m {
                return f.clone();
            }
        }
        let f = Arc::new(designs::features(lib, m));
        self.features_cache = Some((key, m, f.clone()));
        f
    }

    /// For each barely-used sensor, what the network-design evidence says about it and about the
    /// sensors this network doesn't have.
    fn swap_card(&mut self, ui: &mut egui::Ui, n: &Network, low: &[usize], usage: &[f64], even: f64) {
        let Some(lib) = self.lib.clone() else { return };
        let save = &n.save;
        let m = if save.gens.iter().any(|g| g.best_lap.is_some()) { Measure::Lap } else { Measure::Rate };
        let feats = self.features_for(&lib, m);
        let stage = n.runs.last().and_then(|r| compare::stage_index(r.stage)).unwrap_or(0);
        let mine: Vec<String> = save.arch.extras().iter().map(|t| format::sensor_name(t)).collect();
        let inputs: Vec<&designs::Feature> = feats.iter().filter(|f| f.kind == "Input").collect();
        ui.add_space(6.0);
        egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new("Swap candidates for your next network").size(16.0).strong());
            ui.label(
                RichText::new(format!(
                    "Networks with vs without each sensor at the {} stage, by {} gain per 10 gens (one median per network). Usage is estimated from weights, and with/without compares different networks (their tracks, settings and luck differ too), so read this as a shortlist to try, not a verdict.",
                    STAGES[stage],
                    m.unit()
                ))
                .small()
                .weak(),
            );
            let extras = mine.len();
            if extras < 7 {
                ui.label(RichText::new(format!("This network uses {extras} of the 7 non-raycast sensors the game allows, so you could add one instead of swapping.")).small());
            }
            // one line of evidence for a feature at this stage
            let evidence = |ui: &mut egui::Ui, f: Option<&designs::Feature>| match f.and_then(|f| f.evidence(stage).map(|e| (f, e))) {
                Some((f, (pb, st, with_better))) => {
                    let (w, wo) = &f.stages[stage];
                    let (mw, mwo) = (runs::quantile(w, 0.5), runs::quantile(wo, 0.5));
                    let even_odds = pb < 0.6;
                    ui.horizontal(|ui| {
                        badge_small(ui, st);
                        ui.label(
                            RichText::new(format!(
                                "with {} ({} nets) vs without {} ({} nets) · {}",
                                m.fmt(mw),
                                w.len(),
                                m.fmt(mwo),
                                wo.len(),
                                if even_odds {
                                    "about even".to_string()
                                } else {
                                    format!("{} {:.0}% of the time", if with_better { "with beat without" } else { "without beat with" }, pb * 100.0)
                                }
                            ))
                            .small()
                            .color(if even_odds { NEUTRAL } else if with_better { HELPED } else { HURT }),
                        );
                    });
                }
                None => match f {
                    Some(f) => {
                        let (nw, nwo) = (f.stages[stage].0.len(), f.stages[stage].1.len());
                        ui.label(RichText::new(format!("no evidence yet at this stage ({nw} networks with, {nwo} without; needs 2+ each)")).small().weak());
                    }
                    None => {
                        ui.label(RichText::new("every network you've trained has it, so there's nothing to compare it with").small().weak());
                    }
                },
            };
            egui::Grid::new("swap_grid").spacing([12.0, 4.0]).show(ui, |ui| {
                for &i in low {
                    let name = netviz::input_name(save, i);
                    ui.label(RichText::new(format!("{name} ({:.1}× usage)", usage[i] / even)).strong().color(LAP));
                    evidence(ui, inputs.iter().copied().find(|f| f.name == name));
                    ui.end_row();
                }
            });
            // candidates: sensors this network doesn't have, best evidence first
            let mut cands: Vec<(&designs::Feature, Option<(f64, Strength, bool)>)> =
                inputs.iter().copied().filter(|f| !mine.contains(&f.name)).map(|f| (f, f.evidence(stage))).collect();
            let rank = |e: &Option<(f64, Strength, bool)>| match e {
                Some((pb, st, true)) if *pb >= 0.6 => (0, match st { Strength::Strong => 0, Strength::Some => 1, _ => 2 }, (-(pb * 1000.0)) as i64),
                Some((pb, _, true)) => (1, 0, (-(pb * 1000.0)) as i64),
                Some((pb, _, false)) => (2, 0, (pb * 1000.0) as i64),
                None => (3, 0, 0),
            };
            cands.sort_by_key(|c| rank(&c.1));
            ui.add_space(4.0);
            if cands.is_empty() {
                ui.label(RichText::new("Your networks between them haven't used any sensor this one lacks, so there's nothing to compare a swap with.").small().weak());
                return;
            }
            ui.label(RichText::new("Sensors this network doesn't have").small().strong());
            let thin = cands.iter().filter(|c| c.1.is_none()).count();
            egui::Grid::new("swap_cands").striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                for (f, _) in cands.iter().filter(|c| c.1.is_some()).take(6) {
                    ui.label(RichText::new(&f.name).strong());
                    evidence(ui, Some(f));
                    // other stages, compactly
                    let other: Vec<String> = (0..STAGES.len())
                        .filter(|&k| k != stage)
                        .filter_map(|k| f.evidence(k).map(|(_, st, wb)| format!("{} {}{}", STAGES[k], if wb { "▲" } else { "▼" }, match st { Strength::Strong => "●●●", Strength::Some => "●●○", _ => "●○○" })))
                        .collect();
                    ui.label(RichText::new(if other.is_empty() { String::new() } else { other.join("  ") }).small().weak())
                        .on_hover_text("Other stages: ▲ networks with it did better, ▼ without did better; dots = strength");
                    ui.end_row();
                }
            });
            if thin > 0 {
                let names: Vec<&str> = cands.iter().filter(|c| c.1.is_none()).map(|c| c.0.name.as_str()).collect();
                ui.label(RichText::new(format!("No evidence yet at this stage: {}.", names.join(", "))).small().weak());
            }
        });
    }

    // ---------------------------------------------------------------- track names

    /// Save a player's name for a track (empty = forget it) and show it straight away.
    fn apply_rename(&mut self, key: &str, name: &str) {
        let mut all = track_builder::lab::names::load();
        let name = name.trim();
        if name.is_empty() {
            all.remove(key);
        } else {
            all.insert(key.to_string(), name.to_string());
        }
        match track_builder::lab::names::store(&all) {
            Ok(()) => {
                self.rename_error = None;
                if let Some(lib) = &self.lib {
                    let mut l = (**lib).clone();
                    let folder = library::track_names(&l.dir);
                    let mut names = folder;
                    l.user_named = all.keys().cloned().collect();
                    names.extend(all);
                    l.track_names = names;
                    self.lib = Some(Arc::new(l));
                }
                self.name_drafts.remove(key);
            }
            Err(e) => self.rename_error = Some(format!("Couldn't save the name: {e:#}")),
        }
    }

    /// A name box for one track: shown for built-in tracks (unnamed, or named by the player).
    fn name_box(&mut self, ui: &mut egui::Ui, lib: &Library, key: &str, width: f32) {
        if key == runs::RANDOM_TRACK || key.chars().all(|c| c == '0' || c == '-') || (lib.track_names.contains_key(key) && !lib.user_named.contains(key)) {
            return;
        }
        let current = if lib.user_named.contains(key) { lib.track_names.get(key).cloned().unwrap_or_default() } else { String::new() };
        let draft = self.name_drafts.entry(key.to_string()).or_insert(current.clone());
        let r = ui.add_sized([width, 20.0], egui::TextEdit::singleline(draft).hint_text("✎ name it"));
        let changed = draft.trim() != current;
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if changed && (ui.small_button("Save").clicked() || enter) {
            self.rename = Some((key.to_string(), draft.clone()));
        }
    }

    // ---------------------------------------------------------------- reward setups

    fn reward_setups_table(&mut self, ui: &mut egui::Ui, lib: &Library, track: Option<&str>, min_len: usize) {
        let setups = overview::reward_setups(lib, track, min_len);
        if setups.is_empty() {
            return;
        }
        ui.label(RichText::new("Reward setups").size(17.0).strong());
        ui.label(
            RichText::new(
                "Each combination of rewards you've switched on (weights ignored). Always judged on lap time, whatever the measure above, because the rewards set the score's scale. Hover a cell for details.",
            )
            .small()
            .weak(),
        );
        let one_track = track.is_some();
        egui::ScrollArea::horizontal().id_salt("reward_setups").show(ui, |ui| {
            egui::Grid::new("reward_setups_grid").striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                ui.label(RichText::new("Rewards on").small().strong());
                for h in ["Runs", "Networks"] {
                    ui.label(RichText::new(h).small().strong());
                }
                for st in STAGES {
                    ui.label(RichText::new(capitalise(st)).small().strong()).on_hover_text("Median lap-time gain per 10 gens for runs starting at this stage (n = runs)");
                }
                ui.label(RichText::new("First lap").small().strong()).on_hover_text("Generations from starting a track to the first completed lap, for starts that mostly used this setup until then");
                if one_track {
                    ui.label(RichText::new("Fastest lap").small().strong());
                }
                ui.label(RichText::new("Switching to it").small().strong()).on_hover_text("Changes to this setup, judged before/after on lap time");
                ui.end_row();
                // best median per stage, to highlight
                let best: Vec<Option<f64>> = (0..STAGES.len())
                    .map(|k| setups.iter().filter(|s| s.gains[k].len() >= 3).filter_map(|s| s.median_gain(k)).reduce(f64::max))
                    .collect();
                let fastest_first = setups.iter().filter(|s| s.to_first_lap.len() >= 2).filter_map(|s| s.median_first_lap()).reduce(f64::min);
                let shown: Vec<&overview::RewardSetup> =
                    setups.iter().filter(|s| self.rewards_all || s.runs >= 3 || s.to_first_lap.len() + s.never_lapped >= 2).collect();
                for s in shown {
                    ui.label(RichText::new(&s.label).strong());
                    ui.label(RichText::new(s.runs.to_string()).small());
                    ui.label(RichText::new(s.networks.to_string()).small());
                    for k in 0..STAGES.len() {
                        match s.median_gain(k) {
                            Some(g) => {
                                let n = s.gains[k].len();
                                let mut t = RichText::new(format!("{} ({n})", Measure::Lap.fmt(g))).small();
                                t = if n < 3 { t.weak() } else if best[k] == Some(g) { t.color(HELPED).strong() } else if g > 0.0 { t } else { t.color(HURT) };
                                ui.label(t).on_hover_text(format!(
                                    "{} runs: middle half {} to {}{}",
                                    n,
                                    Measure::Lap.fmt(runs::quantile(&s.gains[k], 0.25)),
                                    Measure::Lap.fmt(runs::quantile(&s.gains[k], 0.75)),
                                    if n < 3 { " (under 3 runs: a guess at best)" } else { "" }
                                ));
                            }
                            None => {
                                ui.label(RichText::new("–").weak());
                            }
                        }
                    }
                    let reached = s.to_first_lap.len();
                    match s.median_first_lap() {
                        Some(m) => {
                            let t = RichText::new(format!("{m:.0} gens ({reached}/{})", reached + s.never_lapped)).small();
                            ui.label(if fastest_first == Some(m) { t.color(HELPED).strong() } else { t }).on_hover_text(format!(
                                "Median over {reached} track starts that reached a lap{}. Sorted: {}",
                                if s.never_lapped > 0 { format!("; {} more (20+ gens) never did", s.never_lapped) } else { String::new() },
                                {
                                    let mut v = s.to_first_lap.clone();
                                    v.sort();
                                    v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")
                                }
                            ));
                        }
                        None if s.never_lapped > 0 => {
                            ui.label(RichText::new(format!("never (0/{})", s.never_lapped)).small().color(HURT));
                        }
                        None => {
                            ui.label(RichText::new("–").weak());
                        }
                    }
                    if one_track {
                        ui.label(s.best_lap.as_ref().map_or(RichText::new("–").weak(), |(l, f, g)| RichText::new(format!("{l:.3} s · {f} @ {g}")).small().color(LAP)));
                    }
                    let (h, hu, j) = s.switched_to;
                    ui.label(if j == 0 {
                        RichText::new("–").weak()
                    } else {
                        RichText::new(format!("▲{h} ▼{hu} of {j}")).small().color(if h > hu { HELPED } else if hu > h { HURT } else { NEUTRAL })
                    });
                    ui.end_row();
                }
            });
        });
        let hidden = setups.iter().filter(|s| !(s.runs >= 3 || s.to_first_lap.len() + s.never_lapped >= 2)).count();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Green = best of the setups with 3+ runs at that stage, or the fastest to a first lap. Faint = under 3 runs.").small().weak());
            if hidden > 0 {
                let t = if self.rewards_all { "Hide rarely used setups".to_string() } else { format!("Show {hidden} rarely used setups") };
                if ui.small_button(t).clicked() {
                    self.rewards_all = !self.rewards_all;
                }
            }
        });
        ui.add_space(10.0);
    }

    // ---------------------------------------------------------------- inside the network

    fn inside_network(&mut self, ui: &mut egui::Ui, n: &Network) {
        let save = &n.save;
        let Some(w) = save.weights.as_ref() else { return };
        let imp = netviz::importance(w);
        if imp.is_empty() {
            return;
        }
        let outputs: Vec<String> = save.output_order.iter().map(|&o| format::OUTPUTS.get(o as usize).copied().unwrap_or("?").to_string()).collect();
        if self.usage_output.is_some_and(|o| o >= outputs.len()) {
            self.usage_output = None;
        }
        let usage: Vec<f64> = match self.usage_output {
            Some(o) => netviz::usage_for_output(w, o),
            None => imp[0].clone(),
        };
        let learning = netviz::learning(save);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Inside the network").size(18.0).strong());
            ui.separator();
            ui.label("Usage for");
            egui::ComboBox::from_id_salt("usage_out").selected_text(self.usage_output.map_or("all outputs".to_string(), |o| outputs[o].clone())).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.usage_output, None, "all outputs");
                for (i, o) in outputs.iter().enumerate() {
                    ui.selectable_value(&mut self.usage_output, Some(i), o);
                }
            });
            ui.separator();
            ui.selectable_value(&mut self.net_changes, false, "Weights").on_hover_text("Lines: blue positive, orange negative, brighter = bigger");
            ui.add_enabled_ui(learning.is_some(), |ui| {
                ui.selectable_value(&mut self.net_changes, true, "Changes").on_hover_text("Lines: how much each weight moved since the snapshot below");
            });
        });
        ui.label(
            RichText::new(
                "Usage is estimated from the weights (the save doesn't record how strongly neurons fire), so read it as a guide: an input whose weights are all small can't steer much.",
            )
            .small()
            .weak(),
        );
        let h = 300.0;
        ui.columns(2, |cols| {
            sensor_fan(&mut cols[0], save, &usage, h);
            network_diagram(&mut cols[1], save, w, &imp, &outputs, learning.as_ref(), self.net_changes, h);
        });
        // most and least used, relative to an even share
        let even = 1.0 / usage.len().max(1) as f64;
        let mut order: Vec<usize> = (0..usage.len()).collect();
        order.sort_by(|&a, &b| usage[b].total_cmp(&usage[a]));
        let fmt = |i: usize| format!("{} ({:.1}×)", netviz::input_name(save, i), usage[i] / even);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Most used:").strong());
            ui.label(order.iter().take(3).map(|&i| fmt(i)).collect::<Vec<_>>().join(", "));
            ui.separator();
            ui.label(RichText::new("Least used:").strong());
            ui.label(order.iter().rev().take(3).map(|&i| fmt(i)).collect::<Vec<_>>().join(", "));
        });
        let low: Vec<usize> = order.iter().rev().copied().filter(|&i| usage[i] < even * 0.6 && save.rays.get(i).copied().flatten().is_none()).collect();
        if !low.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{} barely feed the outputs (under 0.6× an even share): candidates to swap for another sensor in your next network. Check the inputs table on the Network designs tab first.",
                    low.iter().map(|&i| netviz::input_name(save, i)).collect::<Vec<_>>().join(", ")
                ))
                .color(LAP),
            );
        }
        ui.label(RichText::new("× = share of the outputs' attention compared with an even split across all inputs.").small().weak());
        if !low.is_empty() {
            self.swap_card(ui, n, &low, &usage, even);
        }

        // still learning?
        ui.add_space(10.0);
        ui.label(RichText::new("Still learning?").size(18.0).strong());
        let Some(l) = learning else {
            ui.label(RichText::new("No earlier snapshot with the same network shape to compare with.").weak());
            return;
        };
        let mutation = n.runs.last().and_then(|r| r.settings.mutation);
        let at_mut = mutation.map_or(String::new(), |m| format!(" (mutation {})", fmt_opt(Some(m))));
        let recent = l.history.last().map_or(l.per_10, |h| h.2);
        let moving = format!("weights moving {:.2}% per 10 gens lately", recent * 100.0);
        let gains = if n.since_gain == 0 { "gained this generation".to_string() } else { format!("last gain {} gens ago", n.since_gain) };
        let (head, text, colour) = match netviz::diagnose(&l, n.since_gain) {
            netviz::Diagnosis::Learning => (
                "Learning",
                format!("Changing and improving: {moving}, {gains}. Leave the settings alone while this lasts."),
                HELPED,
            ),
            netviz::Diagnosis::FineTuning => (
                "Fine-tuning",
                format!(
                    "Small weight changes are still paying off: {moving}, {gains}. Normal later on; if gains dry up, the next step is more exploration, not less."
                ),
                SPEED,
            ),
            netviz::Diagnosis::Drifting => (
                "Drifting",
                format!(
                    "The weights keep changing but nothing is getting better: {moving}, {gains}{at_mut}. {}",
                    if mutation.is_some_and(|m| m <= 0.05) {
                        " New networks keep being adopted, but none are faster. Mutation is already low, so it isn't overshooting: the network has probably got what it can from these rewards and this time limit. Give it a new challenge (a longer time limit, or the suggestion above) rather than lowering mutation further."
                    } else {
                        " New networks keep being adopted, but none are faster, which usually means mutation is too high to refine what it has. Try lowering it a step."
                    }
                ),
                LAP,
            ),
            netviz::Diagnosis::Stuck => (
                "Stuck",
                format!(
                    "The weights have all but stopped and nothing is improving: {moving}, {gains}{at_mut}. The game keeps carrying the same best network because nothing better turns up. Raise mutation to explore more, or change the rewards, time limit or track to give it something new to learn."
                ),
                HURT,
            ),
        };
        ui.horizontal_wrapped(|ui| {
            egui::Frame::new().fill(colour).corner_radius(4.0).inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
                ui.label(RichText::new(head).size(15.0).strong().color(Color32::BLACK));
            });
            ui.label(RichText::new(text).size(15.0).color(colour));
        });
        ui.label(
            RichText::new(format!(
                "Moving = weights changed at least {:.1}% per 10 gens between the last two snapshots. Improving = a gain in the last {} gens.",
                netviz::STILL * 100.0,
                netviz::NOT_IMPROVING_GENS
            ))
            .small()
            .weak(),
        );
        ui.label(RichText::new(format!("Overall since gen {}: {:.2}% ({:.2}% per 10 gens).", l.from_gen, l.total * 100.0, l.per_10 * 100.0)).small());
        ui.columns(2, |cols| {
            // per layer
            let ui = &mut cols[0];
            ui.label(RichText::new(format!("Change per layer since gen {}", l.from_gen)).small().weak());
            let max = l.layers.iter().copied().fold(1e-9, f64::max);
            for (k, c) in l.layers.iter().enumerate() {
                ui.horizontal(|ui| {
                    let name = if k == 0 { "inputs → 1".to_string() } else if k + 1 == l.layers.len() { format!("{k} → outputs") } else { format!("{k} → {}", k + 1) };
                    ui.add_sized([90.0, 14.0], egui::Label::new(RichText::new(name).small()));
                    let (r, p) = ui.allocate_painter(Vec2::new(160.0, 12.0), Sense::hover());
                    p.rect_filled(r.rect, 2.0, Color32::from_gray(40));
                    let wd = (c / max) as f32 * r.rect.width();
                    p.rect_filled(Rect::from_min_size(r.rect.min, Vec2::new(wd.max(1.0), r.rect.height())), 2.0, SPEED);
                    ui.label(RichText::new(format!("{:.2}%", c * 100.0)).small());
                });
            }
            // over time
            let ui = &mut cols[1];
            ui.label(RichText::new("Change per 10 generations, snapshot to snapshot").small().weak());
            let max = l.history.iter().map(|h| h.2).fold(1e-9, f64::max);
            for (a, b, c) in &l.history {
                ui.horizontal(|ui| {
                    ui.add_sized([90.0, 14.0], egui::Label::new(RichText::new(format!("gens {a}–{b}")).small()));
                    let (r, p) = ui.allocate_painter(Vec2::new(160.0, 12.0), Sense::hover());
                    p.rect_filled(r.rect, 2.0, Color32::from_gray(40));
                    let wd = (c / max) as f32 * r.rect.width();
                    p.rect_filled(Rect::from_min_size(r.rect.min, Vec2::new(wd.max(1.0), r.rect.height())), 2.0, if *c < netviz::STILL { HURT } else { SPEED });
                    ui.label(RichText::new(format!("{:.2}%", c * 100.0)).small());
                });
            }
        });
        // usage shifts since the snapshot
        if l.usage_then.len() == imp[0].len() {
            let mut shifts: Vec<(usize, f64)> = imp[0].iter().zip(&l.usage_then).map(|(a, b)| a - b).enumerate().collect();
            shifts.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
            let top: Vec<String> = shifts
                .iter()
                .take(3)
                .filter(|s| s.1.abs() >= 0.002)
                .map(|(i, d)| format!("{} {:+.1} pts", netviz::input_name(save, *i), d * 100.0))
                .collect();
            if !top.is_empty() {
                ui.label(RichText::new(format!("Biggest usage shifts since gen {}: {}", l.from_gen, top.join(", "))).small());
            }
        }

        // snapshot by snapshot
        let track = runs::track_keys(&save.gens).last().cloned().unwrap_or_default();
        let series = netviz::snapshot_series(save, &track);
        if series.len() >= 2 {
            ui.add_space(8.0);
            ui.label(RichText::new("Snapshot by snapshot").strong());
            ui.label(
                RichText::new("The save keeps a copy of the network every so often. \"Same network\" means the game carried the previous best forward unchanged: nothing better turned up in between.")
                    .small()
                    .weak(),
            );
            egui::Grid::new("snap_table").striped(true).spacing([14.0, 3.0]).show(ui, |ui| {
                for h in ["Gen", "Best lap so far", "Median score", "Change", "Mean |w|", ""] {
                    ui.label(RichText::new(h).small().strong());
                }
                ui.end_row();
                let last = series.len() - 1;
                for (k, p) in series.iter().enumerate() {
                    ui.label(if k == last { format!("{} (now)", p.at) } else { p.at.to_string() });
                    ui.label(p.best_lap.map_or("–".into(), |x| format!("{x:.3}s")));
                    ui.label(p.median.map_or("–".into(), |x| format!("{x:.0}")));
                    ui.label(p.change.map_or("–".into(), |c| format!("{:.2}%", c * 100.0)));
                    ui.label(format!("{:.3}", p.mean_abs));
                    let note = match (p.change, k.checked_sub(1).and_then(|j| series[j].best_lap), p.best_lap) {
                        (Some(c), _, _) if c < 1e-6 => RichText::new("same network").color(HURT),
                        (Some(_), Some(a), Some(b)) if b < a - 1e-6 => RichText::new(format!("faster by {:.3}s", a - b)).color(HELPED),
                        (Some(_), _, _) => RichText::new("new network, no faster").color(NEUTRAL),
                        _ => RichText::new(""),
                    };
                    ui.label(note.small());
                    ui.end_row();
                }
            });
            let (a, b) = (series[0].mean_abs, series[series.len() - 1].mean_abs);
            if a > 0.0 && (b / a - 1.0).abs() > 0.1 {
                ui.label(
                    RichText::new(if b > a {
                        format!("Weights are growing ({:+.0}% mean |w| since gen {}): the network is leaning harder on what it has. Weight decay keeps this in check.", (b / a - 1.0) * 100.0, series[0].at)
                    } else {
                        format!("Weights are shrinking ({:+.0}% mean |w| since gen {}), as weight decay pulls unused connections towards zero.", (b / a - 1.0) * 100.0, series[0].at)
                    })
                    .small(),
                );
            }

            // input usage over time
            ui.add_space(6.0);
            ui.label(RichText::new("Input usage over the snapshots").strong());
            let n_in = series.last().map_or(0, |p| p.usage.len());
            let movers: Vec<usize> = {
                let mut d: Vec<(usize, f64)> = (0..n_in)
                    .map(|i| {
                        let vals: Vec<f64> = series.iter().filter_map(|p| p.usage.get(i).copied()).collect();
                        let (lo, hi) = vals.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
                        (i, hi - lo)
                    })
                    .collect();
                d.sort_by(|a, b| b.1.total_cmp(&a.1));
                d.into_iter().take(4).map(|x| x.0).collect()
            };
            let palette = [SCORE, LAP, SURVIVAL, SPREAD];
            let w_avail = ui.available_width().min(900.0);
            let (r, p) = ui.allocate_painter(Vec2::new(w_avail, 150.0), Sense::hover());
            let rect = r.rect.shrink2(Vec2::new(36.0, 12.0));
            p.rect_filled(r.rect, 4.0, Color32::from_gray(22));
            // zoom to the data: usage shares barely move, so a 0-based axis flattens everything
            let even0 = 1.0 / n_in.max(1) as f64;
            let (lo, hi) = series.iter().flat_map(|p| p.usage.iter().copied()).fold((even0, even0), |(lo, hi), v| (lo.min(v), hi.max(v)));
            let pad = ((hi - lo) * 0.08).max(1e-6);
            let (ymin, ymax) = (lo - pad, hi + pad);
            let (g0, g1) = (series[0].at as f64, series[series.len() - 1].at as f64);
            let pos = |g: usize, v: f64| {
                let fx = if g1 > g0 { (g as f64 - g0) / (g1 - g0) } else { 0.5 };
                Pos2::new(rect.left() + fx as f32 * rect.width(), rect.bottom() - ((v - ymin) / (ymax - ymin)) as f32 * rect.height())
            };
            let even = 1.0 / n_in.max(1) as f64;
            p.line_segment([pos(series[0].at, even), pos(series[series.len() - 1].at, even)], Stroke::new(1.0, Color32::from_gray(80)));
            p.text(Pos2::new(r.rect.left() + 4.0, pos(series[0].at, even).y), Align2::LEFT_CENTER, "even", FontId::proportional(10.0), Color32::from_gray(120));
            for i in 0..n_in {
                if movers.contains(&i) {
                    continue;
                }
                let pts: Vec<Pos2> = series.iter().filter_map(|s| s.usage.get(i).map(|v| pos(s.at, *v))).collect();
                p.add(Shape::line(pts, Stroke::new(1.0, Color32::from_gray(70))));
            }
            for (k, &i) in movers.iter().enumerate() {
                let pts: Vec<Pos2> = series.iter().filter_map(|s| s.usage.get(i).map(|v| pos(s.at, *v))).collect();
                for q in &pts {
                    p.circle_filled(*q, 2.5, palette[k]);
                }
                p.add(Shape::line(pts, Stroke::new(2.0, palette[k])));
            }
            for s in &series {
                p.text(Pos2::new(pos(s.at, ymin).x, r.rect.bottom() - 1.0), Align2::CENTER_BOTTOM, s.at.to_string(), FontId::proportional(9.0), Color32::from_gray(120));
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Biggest movers:").small());
                for (k, &i) in movers.iter().enumerate() {
                    let first = series[0].usage.get(i).copied().unwrap_or(0.0);
                    let lastv = series[series.len() - 1].usage.get(i).copied().unwrap_or(0.0);
                    ui.label(RichText::new(format!("● {} {:.1}× → {:.1}×", netviz::input_name(save, i), first / even, lastv / even)).small().color(palette[k]));
                }
                ui.label(RichText::new("(grey: the rest)").small().weak());
            });
        }

        // per output, and spare capacity
        ui.add_space(6.0);
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            let base = save.snapshot_weights.iter().find(|(g, _)| *g == l.from_gen).map(|(_, w)| w);
            if let Some(oc) = base.and_then(|then| netviz::output_change(then, w)) {
                ui.label(RichText::new(format!("Which outputs are learning (change since gen {})", l.from_gen)).strong());
                let max = oc.iter().copied().fold(1e-9, f64::max);
                for (j, c) in oc.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_sized([90.0, 14.0], egui::Label::new(RichText::new(outputs.get(j).cloned().unwrap_or_default()).small()));
                        let (r, p) = ui.allocate_painter(Vec2::new(160.0, 12.0), Sense::hover());
                        p.rect_filled(r.rect, 2.0, Color32::from_gray(40));
                        let wd = (c / max) as f32 * r.rect.width();
                        p.rect_filled(Rect::from_min_size(r.rect.min, Vec2::new(wd.max(1.0), r.rect.height())), 2.0, SPEED);
                        ui.label(RichText::new(format!("{:.2}%", c * 100.0)).small());
                    });
                }
                ui.label(RichText::new("The weights feeding each output. A flat bar means that control's behaviour is settled (or ignored).").small().weak());
            }
            let ui = &mut cols[1];
            ui.label(RichText::new("Spare capacity").strong());
            let idle = netviz::idle_neurons(w);
            let hidden: usize = w.shape().iter().skip(1).rev().skip(1).sum();
            if hidden == 0 {
                ui.label(RichText::new("No hidden layers.").small().weak());
            } else if idle.is_empty() {
                ui.label(RichText::new(format!("All {hidden} hidden neurons pass something on. The network is using its full size; a bigger one might help if it stalls.")).small());
            } else {
                let mut by_layer: Vec<(usize, usize)> = Vec::new();
                for (l, _) in &idle {
                    match by_layer.last_mut() {
                        Some((ll, c)) if ll == l => *c += 1,
                        _ => by_layer.push((*l, 1)),
                    }
                }
                ui.label(
                    RichText::new(format!(
                        "{} of {hidden} hidden neurons are nearly idle ({}): their outgoing weights are under 10% of their layer's average. Room to learn more, so a bigger network isn't the bottleneck yet.",
                        idle.len(),
                        by_layer.iter().map(|(l, c)| format!("{c} in layer {l}")).collect::<Vec<_>>().join(", ")
                    ))
                    .small(),
                );
            }
        });
    }

    // ---------------------------------------------------------------- overview

    fn open_network(&mut self, file: &str, at: Option<usize>, gens: usize) {
        self.selected = Some(file.to_string());
        self.view = View::Network;
        self.selected_run = None;
        self.x_range = at.map(|g| ((g as f64 - 80.0).max(0.0), (g as f64 + 80.0).min(gens as f64)));
    }

    fn overview_view(&mut self, ui: &mut egui::Ui, lib: &Library) {
        let tracks = overview::tracks(lib);
        let min_len = 10;
        ui.heading("What's worked best");
        let runs_n = overview::scoped_runs(lib, self.ov_track.as_deref(), self.ov_measure, min_len).len();
        ui.label(
            RichText::new(format!(
                "From {runs_n} runs of {min_len}+ generations{} (copied history counted once). Gains are compared within a training stage, because early gains are always easier.",
                match &self.ov_track {
                    Some(t) => format!(" on {}", overview::track_name(lib, t).0),
                    None => format!(" across {} tracks", tracks.len()),
                }
            ))
            .weak(),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let current = match &self.ov_track {
                Some(t) => overview::track_name(lib, t).0,
                None => "All tracks".into(),
            };
            egui::ComboBox::from_id_salt("ov_track").width(260.0).selected_text(current).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.ov_track, None, "All tracks");
                for t in &tracks {
                    ui.selectable_value(&mut self.ov_track, Some(t.key.clone()), format!("{}  ·  {} runs · {} gens", t.name, t.runs, thousands(t.gens)));
                }
            });
            if let Some(key) = self.ov_track.clone() {
                self.name_box(ui, lib, &key, 160.0);
            }
            ui.separator();
            measure_picker(ui, &mut self.ov_measure);
            ui.separator();
            for s in [Strength::Strong, Strength::Some, Strength::Weak, Strength::Fact] {
                badge(ui, s);
            }
        });
        if let Some(e) = &self.rename_error {
            ui.colored_label(HURT, e);
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // pointer to the Now training tab
            if let Some(n) = advice::latest(lib) {
                if let Some(a) = advice::advise(lib, n) {
                    ui.horizontal_wrapped(|ui| {
                        if a.live {
                            ui.label(RichText::new("●").color(HELPED));
                        }
                        ui.label(format!("{} {} on {}.", if a.live { "Now training:" } else { "Last trained:" }, a.file, a.track_name));
                        if ui.small_button("Open Now training").clicked() {
                            self.view = View::Now;
                        }
                    });
                    ui.add_space(6.0);
                }
            }
            // stage playbook
            let cards = overview::playbook(lib, self.ov_track.as_deref(), self.ov_measure, min_len);
            stage_grid(ui, cards.len(), |ui, k| {
                let c = &cards[k];
                {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.heading(capitalise(STAGES[c.stage]));
                            ui.label(RichText::new(format!("{} runs", c.runs)).weak());
                        });
                        ui.label(RichText::new(match c.stage {
                            0 => "Runs starting in a network's first 200 generations",
                            1 => "Runs starting between 200 and 1,000 generations",
                            2 => "Runs starting between 1,000 and 5,000 generations",
                            _ => "Runs starting after 5,000 generations",
                        }).small().weak());
                        ui.add_space(4.0);
                        stage_card_body(ui, c, self.ov_measure);
                    });
                }
            });
            ui.add_space(10.0);

            // best on this track
            if let Some(key) = self.ov_track.clone() {
                if let Some(t) = tracks.iter().find(|t| t.key == key) {
                    ui.strong(format!("Best on {}", t.name));
                    if !t.named {
                        ui.label(RichText::new(format!("Trained by: {}. (Built-in track names are stored encrypted in the game files.)", t.networks.join(", "))).small().weak());
                    }
                    match &t.best_lap {
                        Some(b) => {
                            let r = ui.add(egui::Label::new(RichText::new(format!("🏁 Fastest lap {:.3} s by {} at gen {}", b.seconds, b.network, b.at)).strong().color(LAP)).sense(Sense::click()));
                            if r.on_hover_text("Open this network at that generation").clicked() {
                                let gens = lib.by_file(&b.network).map_or(0, |n| n.gens());
                                self.open_network(&b.network.clone(), Some(b.at), gens);
                            }
                            if let Some(n) = lib.by_file(&b.network) {
                                let (steps, hidden) = overview::recipe(n, &key, 5);
                                ui.label(RichText::new(format!("How {} got there (settings over time on this track):", n.file)).small().weak());
                                egui::Grid::new("recipe").striped(true).spacing([12.0, 2.0]).show(ui, |ui| {
                                    let mut prev: Option<f64> = None;
                                    for st in &steps {
                                        // orange where this stretch set a new best lap
                                        let improved = st.best_lap.is_some_and(|l| prev.is_none_or(|p| l < p - 1e-9));
                                        ui.label(RichText::new(format!("gens {}–{}", st.start, st.end - 1)).small());
                                        ui.label(RichText::new(overview::settings_label(&st.settings)).small());
                                        let t = RichText::new(st.best_lap.map_or("no lap yet".into(), |l| format!("best lap {l:.3} s"))).small();
                                        ui.label(if improved { t.color(LAP) } else { t.weak() });
                                        ui.end_row();
                                        if st.best_lap.is_some() {
                                            prev = st.best_lap;
                                        }
                                    }
                                });
                                ui.label(RichText::new("Orange = this stretch set a new best lap.").small().weak());
                                if hidden > 0 {
                                    ui.label(RichText::new(format!("+ {hidden} short stretches under 5 generations not shown")).small().weak());
                                }
                            }
                        }
                        None => {
                            ui.label(RichText::new("No lap completed on this track yet.").weak());
                        }
                    }
                    ui.add_space(6.0);
                }
            }
            let scope = self.ov_track.clone();
            self.reward_setups_table(ui, lib, scope.as_deref(), min_len);
            let best = overview::best_stretches(lib, scope.as_deref(), self.ov_measure, min_len, 5);
            if !best.is_empty() {
                ui.strong("Biggest gains");
                ui.label(RichText::new("The single runs that improved most. Luck plays a part, so read them alongside the stage cards.").small().weak());
                egui::Grid::new("stretches").striped(true).spacing([12.0, 2.0]).show(ui, |ui| {
                    for (n, r, g) in &best {
                        let link = ui.add(egui::Label::new(RichText::new(format!("{} · gens {}–{}", n.file, r.start, r.end - 1)).small()).sense(Sense::click()));
                        if link.on_hover_text("Open this run").clicked() {
                            self.open_network(&n.file.clone(), Some(r.start), n.gens());
                        }
                        ui.label(RichText::new(pct(Some(*g))).small().color(HELPED));
                        ui.label(RichText::new(overview::settings_label(&r.settings)).small());
                        ui.label(RichText::new(format!("{} · {}", r.stage_label(), overview::track_name(lib, &r.track).0)).small().weak());
                        ui.end_row();
                    }
                });
                ui.add_space(10.0);
            }

            // findings
            ui.strong("Findings");
            let fs = overview::findings(lib, self.ov_track.as_deref(), self.ov_measure, min_len);
            if fs.is_empty() {
                ui.label(RichText::new("Nothing to report yet.").weak());
            }
            for f in fs {
                ui.horizontal_wrapped(|ui| {
                    badge(ui, f.strength);
                    let r = ui.add(egui::Label::new(&f.text).sense(if f.link.is_some() { Sense::click() } else { Sense::hover() }));
                    let r = if f.detail.is_empty() { r } else { r.on_hover_text(&f.detail) };
                    if r.clicked() {
                        if let Some((file, at)) = &f.link {
                            let gens = lib.by_file(file).map_or(0, |n| n.gens());
                            self.open_network(&file.clone(), Some(*at), gens);
                        }
                    }
                });
            }

            // tracks table
            if self.ov_track.is_none() {
                ui.add_space(10.0);
                ui.strong("Tracks");
                ui.label(
                    RichText::new(
                        "Click a track to see what worked there. Built-in tracks are packed inside the game, so a save only knows them by an id: type a name to label them (kept in the tool's own settings, not the game's folder). Until then, each shows the network that trained most on it, and hovering lists all of them.",
                    )
                    .small()
                    .weak(),
                );
                egui::Grid::new("tracks").striped(true).spacing([14.0, 3.0]).show(ui, |ui| {
                    for h in ["Track", "Your name", "Gens", "Runs", "Fastest lap", "Set by", "Settings at the time"] {
                        ui.label(RichText::new(h).small().strong());
                    }
                    ui.end_row();
                    for t in &tracks {
                        let name = ui.add(egui::Label::new(RichText::new(&t.name).color(if t.named { Color32::from_gray(230) } else { Color32::from_gray(170) })).sense(Sense::click()));
                        if name.on_hover_text(format!("Trained by {}\nId {}", t.networks.join(", "), t.key)).clicked() {
                            self.ov_track = Some(t.key.clone());
                        }
                        ui.horizontal(|ui| self.name_box(ui, lib, &t.key, 130.0));
                        ui.label(RichText::new(thousands(t.gens)).small());
                        ui.label(RichText::new(t.runs.to_string()).small());
                        match &t.best_lap {
                            Some(b) => {
                                ui.label(RichText::new(format!("{:.3} s", b.seconds)).small().color(LAP));
                                ui.label(RichText::new(format!("{} @ {}", b.network, b.at)).small());
                                ui.label(RichText::new(overview::settings_label(&b.settings)).small().weak());
                            }
                            None => {
                                ui.label(RichText::new("–").small().weak());
                                ui.label("");
                                ui.label("");
                            }
                        }
                        ui.end_row();
                    }
                });
            }
        });
    }

    // ---------------------------------------------------------------- network designs

    fn designs_view(&mut self, ui: &mut egui::Ui, lib: &Library) {
        ui.heading("Network designs");
        ui.label(
            RichText::new(
                "Inputs (sensors), hidden layers, size and outputs. A design is fixed for a whole network, so everything here counts networks, not runs: each network gives one median per stage. With about 20 networks most of this is a guess; it firms up as you train more designs.",
            )
            .weak(),
        );
        ui.horizontal(|ui| {
            measure_picker(ui, &mut self.ov_measure);
            ui.separator();
            for st in [Strength::Strong, Strength::Some, Strength::Weak] {
                badge(ui, st);
            }
        });
        ui.add_space(6.0);
        let m = self.ov_measure;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // design picks per stage
            let cards = overview::playbook(lib, None, m, 10);
            ui.label(RichText::new("Which designs learned fastest, by stage").size(17.0).strong());
            stage_grid(ui, cards.len(), |ui, k| {
                let c = &cards[k];
                {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.heading(capitalise(STAGES[c.stage]));
                        let picks: Vec<&overview::Pick> = c.picks.iter().filter(|p| p.design).collect();
                        if picks.is_empty() {
                            ui.label(RichText::new("Not enough networks to compare yet.").small().weak());
                        } else {
                            stage_rows(ui, c, &picks, m, true);
                        }
                    });
                }
            });
            ui.add_space(12.0);

            // inputs and outputs, with vs without
            ui.label(RichText::new("Inputs and outputs: networks with vs without").size(17.0).strong());
            ui.label(RichText::new("Median gain per 10 gens of networks that have it against networks that don't, at each stage. Every network has a few raycasts plus 7 other sensors, so the choice is which 7.").small().weak());
            let feats = designs::features(lib, m);
            egui::Grid::new("features").striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["", "Feature", "With", "Without", "Early", "Mid", "Late", "Veteran"] {
                    ui.label(RichText::new(h).small().strong());
                }
                ui.end_row();
                for f in &feats {
                    ui.label(RichText::new(f.kind).small().weak());
                    ui.label(capitalise(&f.name));
                    ui.label(RichText::new(format!("{} nets", f.with_nets.len())).small()).on_hover_text(f.with_nets.join(", "));
                    ui.label(RichText::new(format!("{} nets", f.without_nets.len())).small()).on_hover_text(f.without_nets.join(", "));
                    for s in 0..STAGES.len() {
                        let (w, wo) = &f.stages[s];
                        match f.evidence(s) {
                            None => {
                                ui.label(RichText::new("–").small().weak()).on_hover_text("Needs 2+ networks on each side at this stage");
                            }
                            Some((p, st, with_better)) => {
                                let (mw, mwo) = (runs::quantile(w, 0.5), runs::quantile(wo, 0.5));
                                let c = if st == Strength::Weak { NEUTRAL } else if with_better { HELPED } else { HURT };
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("{} vs {}", m.fmt(mw), m.fmt(mwo))).small().color(c));
                                    badge_small(ui, st);
                                })
                                .response
                                .on_hover_text(format!(
                                    "With: median {} over {} networks. Without: {} over {}. Networks {} it out-gained those {} {:.0}% of the time.",
                                    m.fmt(mw),
                                    w.len(),
                                    m.fmt(mwo),
                                    wo.len(),
                                    if with_better { "with" } else { "without" },
                                    if with_better { "without" } else { "with" },
                                    p * 100.0
                                ));
                            }
                        }
                    }
                    ui.end_row();
                }
            });
            ui.add_space(12.0);

            // same track, different designs
            let h2h = designs::head_to_head(lib);
            if !h2h.is_empty() {
                ui.label(RichText::new("Same track, different designs").size(17.0).strong());
                ui.label(RichText::new("Fastest lap each network reached, and how many generations it spent on the track. More training time usually means a faster lap, so read the two together.").small().weak());
                for h in &h2h {
                    ui.add_space(4.0);
                    ui.strong(&h.track);
                    egui::Grid::new(("h2h", &h.track)).striped(true).spacing([14.0, 2.0]).show(ui, |ui| {
                        for (file, arch, lap, gens) in &h.rows {
                            let r = ui.add(egui::Label::new(RichText::new(file).small()).sense(Sense::click()));
                            if r.on_hover_text("Open this network").clicked() {
                                let g = lib.by_file(file).map_or(0, |n| n.gens());
                                self.open_network(&file.clone(), None, g);
                            }
                            ui.label(RichText::new(format!("{lap:.3} s")).small().color(LAP));
                            ui.label(RichText::new(format!("{} gens", thousands(*gens))).small().weak());
                            ui.label(RichText::new(arch.label()).small());
                            ui.end_row();
                        }
                    });
                }
                ui.add_space(12.0);
            }

            // every network
            ui.label(RichText::new("All networks").size(17.0).strong());
            ui.label(RichText::new("Hover the inputs to see every sensor. Gains are medians of the network's own runs of 10+ generations.").small().weak());
            let sums = designs::summaries(lib, m);
            egui::Grid::new("designs_all").striped(true).spacing([12.0, 3.0]).show(ui, |ui| {
                for h in ["Network", "Vehicle", "Inputs", "Hidden", "Params", "Outputs", "Own gens", "Early", "Mid", "Late", "Veteran", "Fastest lap"] {
                    ui.label(RichText::new(h).small().strong());
                }
                ui.end_row();
                for s in &sums {
                    let r = ui.add(egui::Label::new(RichText::new(&s.file).small()).sense(Sense::click()));
                    if r.on_hover_text("Open this network").clicked() {
                        let g = lib.by_file(&s.file).map_or(0, |n| n.gens());
                        self.open_network(&s.file.clone(), None, g);
                    }
                    ui.label(RichText::new(format::vehicle_name(s.vehicle)).small());
                    ui.label(RichText::new(format!("{} rays + {}", s.arch.raycasts(), s.arch.sensors.len() - s.arch.raycasts())).small())
                        .on_hover_text(s.arch.extras().iter().map(|e| format::sensor_name(e)).collect::<Vec<_>>().join(", "));
                    ui.label(RichText::new(s.arch.hidden_label()).small());
                    ui.label(RichText::new(s.arch.params().to_string()).small());
                    ui.label(RichText::new(s.arch.outputs_label()).small());
                    ui.label(RichText::new(thousands(s.own_gens)).small());
                    for k in 0..STAGES.len() {
                        let t = match s.gain[k] {
                            Some(g) => RichText::new(m.fmt(g)).small().color(if g > 0.0 { HELPED } else { HURT }),
                            None => RichText::new("–").small().weak(),
                        };
                        ui.label(t).on_hover_text(format!("{} runs", s.runs[k]));
                    }
                    ui.label(RichText::new(s.fastest.as_ref().map_or("–".into(), |(t, l)| format!("{l:.3} s on {t}"))).small());
                    ui.end_row();
                }
            });
        });
    }

    // ---------------------------------------------------------------- compare view

    fn compare_view(&mut self, ui: &mut egui::Ui, lib: &Library) {
        ui.heading("Compare settings");
        ui.label(RichText::new("Every network's runs, with history shared by copied saves counted once.").weak());
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let f = &mut self.filters;
            combo(ui, "Vehicle", &mut f.vehicle, &VEHICLES);
            egui::ComboBox::from_id_salt("track")
                .selected_text(match f.track {
                    TrackFilter::Any => "Any track",
                    TrackFilter::Fixed => "Fixed tracks",
                    TrackFilter::Random => "Random tracks",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.track, TrackFilter::Any, "Any track");
                    ui.selectable_value(&mut f.track, TrackFilter::Fixed, "Fixed tracks");
                    ui.selectable_value(&mut f.track, TrackFilter::Random, "Random tracks");
                });
            combo(ui, "Selection", &mut f.selection, &SELECTION);
            combo(ui, "Crossover", &mut f.crossover, &CROSSOVER);
            ui.label("Min run length");
            ui.add(egui::DragValue::new(&mut f.min_len).range(2..=500)).on_hover_text("Shorter runs are too noisy to compare");
            ui.separator();
            measure_picker(ui, &mut f.measure);
            ui.separator();
            ui.label("Before/after window");
            let mut w = self.window;
            egui::ComboBox::from_id_salt("window").selected_text(format!("{w} gens")).show_ui(ui, |ui| {
                for x in [10, 20, 40] {
                    ui.selectable_value(&mut w, x, format!("{x} gens"));
                }
            });
            if w != self.window {
                self.window = w;
                self.rescan(false);
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.strong(format!("{} gain per 10 generations, by mutation rate and stage", self.filters.measure.label()));
            ui.label(RichText::new("Box = middle half of the runs, line = median, whiskers = 10th–90th percentile. High rates look better early mostly because early gains are easy, so compare within a column.").small().weak());
            let (grid, unknown) = compare::grid(lib, &self.filters);
            let mut all: Vec<f64> = grid.iter().flat_map(|r| r.iter()).flat_map(|c| c.gains.iter().copied()).collect();
            all.sort_by(f64::total_cmp);
            let (lo, hi) = if all.is_empty() { (-10.0, 10.0) } else { (runs::quantile(&all, 0.05).min(0.0), runs::quantile(&all, 0.95).max(0.0)) };
            let cell_w = ((ui.available_width() - 110.0) / 3.0).clamp(160.0, 320.0);
            egui::Grid::new("cmp_grid").spacing([6.0, 4.0]).show(ui, |ui| {
                ui.label("");
                for s in STAGES {
                    ui.add_sized([cell_w, 18.0], egui::Label::new(RichText::new(s).strong()));
                }
                ui.end_row();
                for (b, row) in grid.iter().enumerate() {
                    ui.add_sized([100.0, 44.0], egui::Label::new(format!("mutation {}", BUCKETS[b].0)));
                    for c in row {
                        box_cell(ui, c, lo, hi, cell_w);
                    }
                    ui.end_row();
                }
            });
            if unknown > 0 {
                ui.label(RichText::new(format!("{unknown} runs left out: unknown mutation rate or stage.")).small().weak());
            }
            ui.add_space(12.0);
            ui.strong("Did the change help? Pooled before/after verdicts");
            let p = compare::pooled_verdicts(lib, &self.filters);
            ui.label(
                RichText::new(format!(
                    "Threshold |z| > {:.1}, from {} placebo points (reward changes: judged on lap time, |z| > {:.1} from {} points): by construction about 5% of testable changes look clear by chance, so compare each count with the ≈ figure. Changes are usually made when a run has flattened, so read these as \"associated with\", not \"caused\".",
                    lib.threshold, lib.placebo_points, lib.lap_threshold, lib.lap_placebo_points
                ))
                .small()
                .weak(),
            );
            egui::Grid::new("pooled").striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                ui.strong("Change");
                for s in STAGES.iter().chain(["all stages"].iter()) {
                    ui.strong(*s);
                }
                ui.end_row();
                for (kind, cells) in &p.cells {
                    ui.label(*kind);
                    for c in cells {
                        pooled_cell(ui, c);
                    }
                    ui.end_row();
                }
                ui.strong("All");
                for _ in 0..STAGES.len() {
                    ui.label("");
                }
                pooled_cell(ui, &p.total);
                ui.end_row();
            });
            let untestable: usize = p.untestable.iter().map(|x| x.1).sum();
            ui.add_space(4.0);
            ui.label(format!(
                "{} changes in total: {} testable, {} not ({}).",
                p.changes,
                p.total.testable,
                untestable,
                p.untestable.iter().map(|(r, n)| format!("{n} {r}")).collect::<Vec<_>>().join(", ")
            ));
            let short = p.untestable.iter().find(|x| x.0 == "too few generations").map_or(0, |x| x.1);
            if short * 2 > p.changes {
                ui.label(
                    RichText::new(format!(
                        "Most changes come too soon after the previous one to be judged. Keeping settings for at least {} generations either side of a change makes it testable.",
                        lib.min_side
                    ))
                    .color(LAP),
                );
            }
        });
    }
}

// -------------------------------------------------------------------- helpers

fn combo(ui: &mut egui::Ui, label: &str, value: &mut Option<u8>, names: &[&str]) {
    egui::ComboBox::from_id_salt(label)
        .selected_text(value.map_or(format!("Any {}", label.to_lowercase()), |v| names[v as usize].to_string()))
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, format!("Any {}", label.to_lowercase()));
            for (i, n) in names.iter().enumerate() {
                ui.selectable_value(value, Some(i as u8), *n);
            }
        });
}

fn box_cell(ui: &mut egui::Ui, c: &compare::Cell, lo: f64, hi: f64, w: f32) {
    let (resp, p) = ui.allocate_painter(Vec2::new(w, 44.0), Sense::hover());
    let r = resp.rect;
    p.rect_filled(r, 3.0, Color32::from_rgb(28, 30, 34));
    let x = |v: f64| r.min.x + 8.0 + (((v - lo) / (hi - lo)) as f32).clamp(0.0, 1.0) * (w - 16.0);
    let zero = x(0.0);
    p.line_segment([Pos2::new(zero, r.min.y + 4.0), Pos2::new(zero, r.max.y - 14.0)], Stroke::new(1.0, Color32::from_gray(70)));
    if c.n() < 5 {
        p.text(r.center(), Align2::CENTER_CENTER, format!("too few runs (n={})", c.n()), FontId::proportional(11.0), Color32::from_gray(100));
        return;
    }
    let y = r.min.y + 15.0;
    let col = if c.median().unwrap_or(0.0) >= 0.0 { SCORE } else { HURT };
    p.line_segment([Pos2::new(x(c.q(0.1)), y), Pos2::new(x(c.q(0.9)), y)], Stroke::new(1.0, col.gamma_multiply(0.7)));
    p.rect_filled(Rect::from_x_y_ranges(x(c.q(0.25))..=x(c.q(0.75)).max(x(c.q(0.25)) + 1.0), (y - 6.0)..=(y + 6.0)), 2.0, col.gamma_multiply(0.5));
    let m = x(c.median().unwrap_or(0.0));
    p.line_segment([Pos2::new(m, y - 8.0), Pos2::new(m, y + 8.0)], Stroke::new(2.0, Color32::WHITE));
    p.text(Pos2::new(r.min.x + 6.0, r.max.y - 3.0), Align2::LEFT_BOTTOM, format!("n={} · median {}", c.n(), pct(c.median())), FontId::proportional(10.0), Color32::from_gray(170));
    resp.on_hover_text(format!(
        "{} runs\nmedian {}\nmiddle half {} to {}\n10th–90th percentile {} to {}",
        c.n(),
        pct(c.median()),
        pct(Some(c.q(0.25))),
        pct(Some(c.q(0.75))),
        pct(Some(c.q(0.1))),
        pct(Some(c.q(0.9)))
    ));
}

fn pooled_cell(ui: &mut egui::Ui, c: &compare::Pooled) {
    if c.testable == 0 {
        ui.label(RichText::new("–").weak());
        return;
    }
    ui.horizontal(|ui| {
        ui.colored_label(HELPED, format!("▲{}", c.helped));
        ui.colored_label(HURT, format!("▼{}", c.hurt));
        ui.label(RichText::new(format!("of {} (≈{:.1})", c.testable, c.expected())).weak());
    })
    .response
    .on_hover_text(format!(
        "{} testable changes: {} clearly helped, {} clearly hurt. About {:.1} would look clear by chance alone.",
        c.testable,
        c.helped,
        c.hurt,
        c.expected()
    ));
}

fn gen_tooltip(ui: &mut egui::Ui, i: usize, g: &Gen) {
    ui.strong(format!("Generation {i}"));
    let row = |ui: &mut egui::Ui, k: &str, v: String| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(k).weak());
            ui.label(v);
        });
    };
    let n = |v: Option<f64>| v.map_or("–".into(), short_num);
    row(ui, "Median", format!("{} (IQR {} – {})", n(g.median), n(g.q1), n(g.q3)));
    row(ui, "Best / average / worst", format!("{} / {} / {}", n(g.best), n(g.average), n(g.worst)));
    row(ui, "Best lap", g.best_lap.map_or("–".into(), |l| format!("{l:.3} s")));

    row(ui, "Survival", g.survival().map_or("–".into(), |s| format!("{:.0}% ({} of {})", s * 100.0, g.active.unwrap_or(0), g.settings.population.unwrap_or(0))));
    row(ui, "Simulated time", g.time.map_or("–".into(), |t| format!("{t:.0} s")));
    row(ui, "Score per second", g.rate().map_or("–".into(), |r| format!("{} median · {} best", short_num(r), g.best_speed.map_or("–".into(), short_num))));
}

fn run_summary(ui: &mut egui::Ui, r: &Run) {
    ui.strong(format!("Run: gens {}–{} ({})", r.start, r.end - 1, r.len()));
    ui.label(format!(
        "mutation {} · {}·{} · crossover {} · pop {} · time limit {} s at the start · weight decay {} · elimination {}",
        fmt_opt(r.settings.mutation),
        format::selection_name(r.settings.selection),
        r.settings.selection_size.map_or("?".into(), |x| x.to_string()),
        format::crossover_name(r.settings.crossover),
        r.settings.population.map_or("?".into(), |x| x.to_string()),
        fmt_opt(r.settings.time_limit),
        fmt_opt(r.settings.weight_decay),
        match r.settings.elimination {
            Some(true) => "on",
            Some(false) => "off",
            None => "?",
        }
    ));
    ui.label(RichText::new(format!("rewards: {}", format::rewards_label(&r.rewards))).small().weak());
    ui.label(format!(
        "gain {} per 10 gens · lap {} · stage {} · wobble {}",
        pct(r.score_gain),
        pct(r.lap_gain),
        r.stage_label(),
        r.stability.map_or("–".into(), |x| format!("{:.0}%", x * 100.0))
    ));
}

fn verdict_tooltip(ui: &mut egui::Ui, v: &Verdict, lib: &Library) {
    let c = match v.outcome {
        Outcome::Helped => HELPED,
        Outcome::Hurt => HURT,
        _ => NEUTRAL,
    };
    ui.strong(format!("Gen {}: {}", v.at, if v.change.is_empty() { "settings changed" } else { &v.change }));
    ui.colored_label(c, v.label());
    if let (Some(b), Some(a), Some(z)) = (v.before, v.after, v.z) {
        if v.by_lap {
            ui.label(RichText::new("Judged on lap time: the rewards set the score's scale, but a lap time means the same whatever the rewards.").small());
            ui.label(format!("Best lap trend {:+.4} → {:+.4} s per gen (negative = getting faster)", b.slope, a.slope));
            ui.label(format!("z = {z:+.1} (clear above ±{:.1}, lap-time calibration) · {} gens before, {} after", lib.lap_threshold, v.n_before, v.n_after));
        } else {
            ui.label(format!("Median slope {:+.3} → {:+.3} per gen", b.slope, a.slope));
            ui.label(format!("z = {z:+.1} (clear above ±{:.1}) · {} gens before, {} after", lib.threshold, v.n_before, v.n_after));
        }
    }
    if let Some(r) = detectability::retrospective(lib, v) {
        ui.label(detectability::format_retrospective(&r));
    }
    if let Some(l) = v.lap_change {
        ui.label(format!("Best lap {} {:.3} s (lap time always falls over time, so read with care)", if l < 0.0 { "faster by" } else { "slower by" }, l.abs()));
    }
    ui.label(RichText::new(format!("Stage before: {}", runs::stage_label(v.stage))).weak());
}

fn fork_text(lib: &Library, n: &Network) -> Option<String> {
    let (p, k) = n.fork.parent?;
    let parent = &lib.networks[p];
    let mut s = if n.fork.parent_is_prefix {
        format!("Copied from {} at gen {k}", parent.file)
    } else {
        format!("Shares its first {k} gens with {}", parent.file)
    };
    if n.gens() == k {
        s.push_str(", no generations of its own yet");
    } else {
        s.push_str(&format!(", {} of its own since", thousands(n.gens() - k)));
    }
    if parent.save.shape != n.save.shape {
        s.push_str(&format!(" (network shape changed: {} → {})", parent.save.shape_label(), n.save.shape_label()));
    } else if parent.save.id != n.save.id && n.gens() == k {
        s.push_str(" (weights differ)");
    }
    Some(s)
}

fn band_color(m: Option<f64>) -> Color32 {
    let Some(m) = m else { return Color32::from_rgb(40, 40, 40) };
    // log scale 0.0005 .. 0.5
    let t = ((m.max(0.0005).ln() - 0.0005f64.ln()) / (0.5f64.ln() - 0.0005f64.ln())).clamp(0.0, 1.0) as f32;
    let lerp = |a: f32, b: f32| (a + (b - a) * t) as u8;
    Color32::from_rgb(lerp(26.0, 62.0), lerp(30.0, 74.0), lerp(38.0, 104.0))
}

fn pct(v: Option<f64>) -> String {
    match v {
        None => "–".into(),
        Some(x) if x.abs() >= 100.0 => format!("{x:+.0}%"),
        Some(x) => format!("{x:+.1}%"),
    }
}

fn short_num(v: f64) -> String {
    let a = v.abs();
    if a >= 10000.0 {
        format!("{:.1}k", v / 1000.0)
    } else if a >= 100.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

fn thousands(n: usize) -> String {
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

fn ticks(a: f64, b: f64, max: usize) -> Vec<f64> {
    let span = (b - a).max(1.0);
    let raw = span / max as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag).max(1.0);
    let mut t = (a / step).ceil() * step;
    let mut out = Vec::new();
    while t <= b {
        out.push(t);
        t += step;
    }
    out
}

fn ago(mtime: f64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let s = (now - mtime).max(0.0);
    if mtime <= 0.0 {
        "unknown".into()
    } else if s < 90.0 {
        "just now".into()
    } else if s < 5400.0 {
        format!("{:.0} min ago", s / 60.0)
    } else if s < 129600.0 {
        format!("{:.0} h ago", s / 3600.0)
    } else {
        format!("{:.0} days ago", s / 86400.0)
    }
}

fn badge(ui: &mut egui::Ui, s: Strength) {
    let (c, t) = match s {
        Strength::Strong => (HELPED, "●●● strong"),
        Strength::Some => (Color32::from_rgb(200, 200, 90), "●●○ some evidence"),
        Strength::Weak => (NEUTRAL, "●○○ guess"),
        Strength::Fact => (Color32::from_rgb(110, 170, 255), "■ fact"),
    };
    ui.label(RichText::new(t).small().color(c)).on_hover_text(match s {
        Strength::Strong => "8+ runs, and a run with this setting beat one with the runner-up at least 70% of the time",
        Strength::Some => "5+ runs, and it beat the runner-up at least 60% of the time",
        Strength::Weak => "Too few runs, or too close to the runner-up (under 1 point per 10 gens, or under 60% of the time) to rely on",
        Strength::Fact => "Measured directly, not a comparison",
    });
}

/// The rows of a stage card: one per setting, best value first, with its evidence.
/// Lay out stage cards in as many columns as fit (4 across on a wide window, else 2×2 or a
/// single column), so their contents never run into the next card.
fn stage_grid(ui: &mut egui::Ui, n: usize, mut card: impl FnMut(&mut egui::Ui, usize)) {
    let per = ((ui.available_width() / 420.0).floor() as usize).clamp(1, 4);
    let per = if per == 3 { 2 } else { per }; // 3 + 1 looks lopsided
    for row in (0..n).collect::<Vec<_>>().chunks(per) {
        ui.columns(per, |cols| {
            for (&k, ui) in row.iter().zip(cols.iter_mut()) {
                card(ui, k);
            }
        });
        ui.add_space(4.0);
    }
}

fn stage_card_body(ui: &mut egui::Ui, c: &overview::StageCard, m: Measure) {
    if let Some(best) = c.flat {
        ui.label(
            RichText::new(format!(
                "No mutation rate moved this stage (best median {} per 10 gens). Changing rewards, the track or the network is more likely to help.",
                pct(Some(best))
            ))
            .small()
            .color(LAP),
        );
        ui.add_space(2.0);
    }
    if c.picks.is_empty() {
        ui.label(RichText::new("Not enough runs to compare yet (needs 3+ runs each for at least two values of a setting).").small().weak());
        return;
    }
    for design in [false, true] {
        let picks: Vec<&overview::Pick> = c.picks.iter().filter(|p| p.design == design).collect();
        if picks.is_empty() {
            continue;
        }
        if design {
            ui.add_space(3.0);
            ui.label(RichText::new("Network design (counted per network)").small().weak());
        }
        stage_rows(ui, c, &picks, m, design);
    }
}

fn stage_rows(ui: &mut egui::Ui, c: &overview::StageCard, picks: &[&overview::Pick], m: Measure, design: bool) {
    let value_w = (ui.available_width() - 210.0).max(80.0);
    egui::Grid::new(("card", c.stage, c.runs, design)).spacing([8.0, 3.0]).show(ui, |ui| {
        for p in picks {
            ui.label(RichText::new(p.dimension).small().weak());
            ui.scope(|ui| {
                ui.set_max_width(value_w);
                ui.add(egui::Label::new(RichText::new(&p.best.label).strong()).wrap());
            });
            let g = p.best.median();
            ui.label(RichText::new(m.fmt(g)).small().color(if g > 0.0 { HELPED } else { HURT }));
            let r = ui.horizontal(|ui| badge_small(ui, p.strength)).response;
            r.on_hover_ui(|ui| {
                ui.strong(format!("{}: median {} gain per 10 gens", p.dimension, m.unit()));
                for ch in &p.all {
                    ui.label(format!("{:10}  {}  ({} {})", ch.label, m.fmt(ch.median()), ch.n(), ch.unit));
                }
                if let (Some(b), Some(ru)) = (p.beats, &p.runner_up) {
                    let what = if p.design { "network" } else { "run" };
                    ui.label(RichText::new(format!("A {} {what} out-gained a {} {what} {:.0}% of the time.", p.best.label, ru.label, b * 100.0)).weak());
                }
            });
            ui.end_row();
        }
    });
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Current state and the suggestions for one network.
fn advice_body(ui: &mut egui::Ui, a: &Advice, wide: bool) {
    let stage = a.stage.map_or("unknown", |s| STAGES[s]);
    let lap = match a.lap {
        (Some(now), Some(before)) => format!(" · best lap {now:.3} s ({:+.3} s in 20 gens)", now - before),
        (Some(now), None) => format!(" · best lap {now:.3} s"),
        _ => String::new(),
    };
    ui.label(
        RichText::new(format!(
            "{} stage · current settings for {} gens: {}{lap}",
            capitalise(stage),
            a.run.len(),
            overview::settings_label(&a.run.settings)
        ))
        .small(),
    );
    ui.label(
        RichText::new(format!(
            "Judged by {}. Suggestions come from your saves only; nothing is changed in the game.",
            match a.measure {
                Measure::Lap => "lap time (this track has laps)",
                Measure::Score => "score (no laps yet)",
                Measure::Rate => "score per second (no laps yet; unaffected by the time limit)",
                _ => a.measure.unit(),
            }
        ))
        .small()
        .weak(),
    );
    ui.add_space(3.0);
    for s in &a.suggestions {
        ui.horizontal_wrapped(|ui| {
            if wide {
                badge(ui, s.strength);
            }
            let t = RichText::new(&s.text);
            let t = if s.strength == Strength::Fact && s.text.starts_with("Stalled") { t.color(LAP) } else { t };
            let r = ui.label(t);
            let r = if wide { r } else { r.on_hover_ui(|ui| badge(ui, s.strength)) };
            r.on_hover_text(&s.why);
        });
    }
}

fn badge_small(ui: &mut egui::Ui, s: Strength) {
    let (c, t) = match s {
        Strength::Strong => (HELPED, "●●●"),
        Strength::Some => (Color32::from_rgb(200, 200, 90), "●●○"),
        Strength::Weak => (NEUTRAL, "●○○"),
        Strength::Fact => (Color32::from_rgb(110, 170, 255), "■"),
    };
    ui.label(RichText::new(t).small().color(c)).on_hover_text(s.label());
}

/// Pick what "progress" means.
fn measure_picker(ui: &mut egui::Ui, m: &mut Measure) {
    for x in Measure::ALL {
        ui.selectable_value(m, x, x.label()).on_hover_text(x.help());
    }
}

fn usage_colour(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| (a + (b - a) * t) as u8;
    Color32::from_rgb(lerp(80.0, 70.0), lerp(80.0, 220.0), lerp(90.0, 110.0))
}

/// Top-down car with its raycasts at their real angles and lengths, coloured by usage, and the
/// other sensors as bars.
fn sensor_fan(ui: &mut egui::Ui, save: &Save, usage: &[f64], h: f32) {
    let w = ui.available_width();
    let (resp, p) = ui.allocate_painter(Vec2::new(w, h * 0.68), Sense::hover());
    let r = resp.rect;
    p.rect_filled(r, 4.0, Color32::from_rgb(22, 24, 28));
    let max_use = usage.iter().copied().fold(1e-9, f64::max);
    let car = Pos2::new(r.center().x, r.max.y - 26.0);
    let max_len = save.rays.iter().flatten().map(|x| x.1).fold(1.0, f32::max);
    let radius = (r.height() - 40.0).min(r.width() / 2.0 - 30.0);
    let mut hover: Option<String> = None;
    for (i, ray) in save.rays.iter().enumerate() {
        let Some((deg, len)) = ray else { continue };
        let u = usage.get(i).copied().unwrap_or(0.0);
        let t = (u / max_use) as f32;
        let a = deg.to_radians();
        let end = car + Vec2::new(a.sin(), -a.cos()) * radius * (len / max_len);
        p.line_segment([car, end], Stroke::new(1.0 + 4.0 * t, usage_colour(t)));
        p.text(end + Vec2::new(a.sin(), -a.cos()) * 10.0, Align2::CENTER_CENTER, format!("{:.0}%", u * 100.0), FontId::proportional(9.0), usage_colour(t));
        if let Some(hp) = resp.hover_pos() {
            if seg_distance(hp, car, end) < 6.0 {
                hover = Some(format!("Raycast {deg:+.0}°, length {len:.0}: usage {:.1}%", u * 100.0));
            }
        }
    }
    // the car
    p.rect_filled(Rect::from_center_size(car, Vec2::new(14.0, 26.0)), 3.0, Color32::from_gray(200));
    p.rect_filled(Rect::from_center_size(car + Vec2::new(0.0, -8.0), Vec2::new(10.0, 6.0)), 1.0, Color32::from_gray(90));
    p.text(r.left_top() + Vec2::new(6.0, 4.0), Align2::LEFT_TOP, "Raycasts (forward is up)", FontId::proportional(10.0), Color32::from_gray(150));
    if let Some(t) = hover {
        resp.on_hover_text(t);
    }
    // other sensors
    let max_extra = save.rays.iter().enumerate().filter(|(_, r)| r.is_none()).map(|(i, _)| usage.get(i).copied().unwrap_or(0.0)).fold(1e-9, f64::max).max(max_use);
    for (i, ray) in save.rays.iter().enumerate() {
        if ray.is_some() {
            continue;
        }
        let u = usage.get(i).copied().unwrap_or(0.0);
        ui.horizontal(|ui| {
            ui.add_sized([120.0, 14.0], egui::Label::new(RichText::new(netviz::input_name(save, i)).small()).truncate());
            let (rr, pp) = ui.allocate_painter(Vec2::new((ui.available_width() - 50.0).max(40.0), 11.0), Sense::hover());
            pp.rect_filled(rr.rect, 2.0, Color32::from_gray(40));
            let t = (u / max_extra) as f32;
            pp.rect_filled(Rect::from_min_size(rr.rect.min, Vec2::new((t * rr.rect.width()).max(1.0), rr.rect.height())), 2.0, usage_colour((u / max_use) as f32));
            ui.label(RichText::new(format!("{:.1}%", u * 100.0)).small());
        });
    }
}

fn seg_distance(q: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((q - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - q).length()
}

/// The network as columns of neurons. Lines are coloured by weight (or by how much each weight
/// changed since the comparison snapshot); input dots are sized by usage.
#[allow(clippy::too_many_arguments)]
fn network_diagram(ui: &mut egui::Ui, save: &Save, w: &track_builder::lab::format::Weights, imp: &[Vec<f64>], outputs: &[String], learning: Option<&netviz::Learning>, changes: bool, h: f32) {
    let width = ui.available_width();
    let (resp, p) = ui.allocate_painter(Vec2::new(width, h), Sense::hover());
    let r = resp.rect;
    p.rect_filled(r, 4.0, Color32::from_rgb(22, 24, 28));
    let shape = w.shape();
    let inner = Rect::from_min_max(r.min + Vec2::new(96.0, 24.0), r.max - Vec2::new(72.0, 12.0));
    let col_x = |l: usize| inner.min.x + inner.width() * l as f32 / (shape.len() - 1).max(1) as f32;
    let node_y = |l: usize, i: usize| {
        let n = shape[l];
        if n <= 1 { inner.center().y } else { inner.min.y + inner.height() * i as f32 / (n - 1) as f32 }
    };
    let then: Option<&track_builder::lab::format::Weights> = learning.and_then(|l| save.snapshot_weights.iter().find(|(g, _)| *g == l.from_gen).map(|(_, w)| w));
    for (l, layer) in w.w.iter().enumerate() {
        let max = layer.iter().flatten().map(|x| x.abs()).fold(1e-9, f32::max);
        let dmax = then.map(|t| {
            layer
                .iter()
                .zip(&t.w[l])
                .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
                .fold(1e-9, f32::max)
        });
        for (i, row) in layer.iter().enumerate() {
            for (j, x) in row.iter().enumerate() {
                let (c, a) = if changes {
                    let d = then.map_or(0.0, |t| (x - t.w[l][i][j]).abs());
                    (Color32::from_rgb(240, 220, 90), d / dmax.unwrap_or(1.0))
                } else {
                    (if *x >= 0.0 { SCORE } else { LAP }, x.abs() / max)
                };
                if a < 0.04 {
                    continue;
                }
                p.line_segment([Pos2::new(col_x(l), node_y(l, i)), Pos2::new(col_x(l + 1), node_y(l + 1, j))], Stroke::new(0.4 + 1.6 * a, c.gamma_multiply(0.15 + 0.85 * a)));
            }
        }
    }
    let mut hover: Option<String> = None;
    for (l, &n) in shape.iter().enumerate() {
        let lmax = imp.get(l).map_or(1e-9, |v| v.iter().copied().fold(1e-9, f64::max));
        for i in 0..n {
            let c = Pos2::new(col_x(l), node_y(l, i));
            let u = imp.get(l).and_then(|v| v.get(i)).copied().unwrap_or(0.0);
            let rad = 2.5 + 3.5 * (u / lmax) as f32;
            p.circle_filled(c, rad, if l == 0 { usage_colour((u / lmax) as f32) } else { Color32::from_gray(190) });
            if l == 0 {
                p.text(c - Vec2::new(8.0, 0.0), Align2::RIGHT_CENTER, netviz::input_name(save, i), FontId::proportional(9.0), Color32::from_gray(160));
            } else if l + 1 == shape.len() {
                p.text(c + Vec2::new(8.0, 0.0), Align2::LEFT_CENTER, outputs.get(i).cloned().unwrap_or_default(), FontId::proportional(10.0), Color32::from_gray(210));
            }
            if let Some(hp) = resp.hover_pos() {
                if (hp - c).length() < rad + 3.0 {
                    hover = Some(if l == 0 {
                        format!("{}: usage {:.1}%", netviz::input_name(save, i), u * 100.0)
                    } else if l + 1 == shape.len() {
                        format!("Output: {}", outputs.get(i).cloned().unwrap_or_default())
                    } else {
                        format!("Hidden layer {l}, neuron {}: {:.1}% of the layer's importance", i + 1, u * 100.0)
                    });
                }
            }
        }
    }
    p.text(r.left_top() + Vec2::new(6.0, 2.0), Align2::LEFT_TOP, if changes { "Lines: change since the snapshot" } else { "Lines: blue +, orange −" }, FontId::proportional(10.0), Color32::from_gray(150));
    if let Some(t) = hover {
        resp.on_hover_text(t);
    }
}
