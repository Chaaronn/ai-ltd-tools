//! The Windows app: drop a course map, click start/finish, build, save into the game.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

/// A finished background job.
struct Done {
    built: Built,
    /// settings chosen by auto-tune (None for a plain build)
    params: Option<BuildParams>,
    note: String,
}

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, Vec2};
use track_builder::fit::P;
use track_builder::game::curve::{bezier, lap_curve};
use track_builder::game::tiles::Surface;
use track_builder::game::track::{game_tracks_dir, Track};
use track_builder::fit::simplify_closed;
use track_builder::pipeline::{build_points, BuildParams, Built};
use track_builder::trace::{trace, TraceParams, Traced};

#[derive(PartialEq, Clone, Copy)]
enum EditMode {
    /// clicks set the start/finish
    Off,
    /// drag / insert / delete points on the course line
    Edit,
    /// click points one by one to draw the course by hand
    Draw,
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Builder,
    Lab,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    Image,
    Track,
}

pub struct App {
    image_path: Option<PathBuf>,
    image: Option<image::RgbImage>,
    texture: Option<TextureHandle>,
    trace_params: TraceParams,
    traced: Option<Traced>,
    /// The course line the build uses (image pixels): the trace, simplified, plus user edits.
    line: Option<Vec<P>>,
    edit_mode: EditMode,
    drag_idx: Option<usize>,
    undo: Vec<Vec<P>>,
    params: BuildParams,
    building: Option<Receiver<Result<Done, String>>>,
    /// (builds finished, builds planned) while auto-tuning
    progress: Arc<(AtomicUsize, AtomicUsize)>,
    /// short description of the last result's quality
    quality: Option<String>,
    built: Option<Built>,
    loaded_track: Option<Track>,
    view: View,
    name: String,
    overwrite: bool,
    status: String,
    error: Option<String>,
    path_input: String,
    mode: Mode,
    /// Created the first time the Evolution Lab tab is opened.
    lab: Option<crate::gui_lab::Lab>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // the app is designed dark; don't follow a light system theme
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut app = App {
            image_path: None,
            image: None,
            texture: None,
            trace_params: TraceParams::default(),
            traced: None,
            line: None,
            edit_mode: EditMode::Off,
            drag_idx: None,
            undo: Vec::new(),
            params: BuildParams::default(),
            building: None,
            progress: Arc::new((AtomicUsize::new(0), AtomicUsize::new(0))),
            quality: None,
            built: None,
            loaded_track: None,
            view: View::Image,
            name: "My Track".into(),
            overwrite: false,
            status: "Drop a course map (PNG, JPG or SVG) onto the window, or click Open image.".into(),
            error: None,
            path_input: String::new(),
            mode: Mode::Builder,
            lab: None,
        };
        // a file passed on the command line (or dropped onto the .exe) opens straight away
        if let Some(p) = std::env::args_os().nth(1) {
            app.open_path(&cc.egui_ctx, Path::new(&p));
        }
        app
    }

    fn open_path(&mut self, ctx: &egui::Context, path: &Path) {
        // a saved network opens in the Evolution Lab
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("evolution")) {
            self.lab = Some(crate::gui_lab::Lab::new(Some(path)));
            self.mode = Mode::Lab;
            return;
        }
        self.error = None;
        if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("track")) {
            match Track::load(path) {
                Ok(t) => {
                    let issues = t.validate();
                    self.status = format!(
                        "Loaded {} - {} tiles, {}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        t.tiles.len(),
                        if issues.is_empty() { "valid".to_string() } else { format!("{} issue(s): {}", issues.len(), issues[0]) }
                    );
                    if let Some(stem) = path.file_stem() {
                        self.name = format!("{} (copy)", stem.to_string_lossy());
                    }
                    self.loaded_track = Some(t);
                    self.built = None;
                    self.view = View::Track;
                }
                Err(e) => self.error = Some(format!("{e:#}")),
            }
            return;
        }
        match track_builder::input::load_image(path) {
            Ok(rgb) => {
                let rgba = image::DynamicImage::ImageRgb8(rgb.clone()).to_rgba8();
                let ci = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
                self.texture = Some(ctx.load_texture("map", ci, egui::TextureOptions::LINEAR));
                if let Some(stem) = path.file_stem() {
                    self.name = stem.to_string_lossy().into_owned();
                }
                self.image = Some(rgb);
                self.image_path = Some(path.to_path_buf());
                self.params.start_px = None;
                self.built = None;
                self.loaded_track = None;
                self.view = View::Image;
                self.retrace();
            }
            Err(e) => self.error = Some(format!("Couldn't open image: {e:#}")),
        }
    }

    fn retrace(&mut self) {
        let Some(img) = &self.image else { return };
        match trace(img, &self.trace_params) {
            Ok(t) => {
                let markers = if t.markers_removed > 0 { format!(", bridged {} marker(s)", t.markers_removed) } else { String::new() };
                self.status = format!("Traced the course{markers}. Click on the line to set start/finish, then Build. Use Edit line to fix anything.");
                self.line = Some(simplify_closed(&t.points, 1.2));
                self.undo.clear();
                self.traced = Some(t);
                self.error = None;
            }
            Err(e) => {
                self.traced = None;
                self.line = None;
                self.error = Some(format!("{e:#}\nYou can still draw the course by hand (Draw by hand)."));
            }
        }
    }

    fn start_build(&mut self) {
        let Some(line) = self.line.clone().filter(|l| l.len() >= 3) else { return };
        self.edit_mode = EditMode::Off;
        let p = self.params.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = build_points(&line, &p).map_err(|e| format!("{e:#}")).map(|built| {
                let note = track_builder::autotune::score(&built).map(|s| quality_note(&s)).unwrap_or_default();
                Done { built, params: None, note }
            });
            let _ = tx.send(r);
        });
        self.building = Some(rx);
        self.status = "Building...".into();
        self.error = None;
    }

    fn start_auto(&mut self) {
        let Some(line) = self.line.clone().filter(|l| l.len() >= 3) else { return };
        self.edit_mode = EditMode::Off;
        let p = self.params.clone();
        let (tx, rx) = channel();
        let prog = self.progress.clone();
        prog.0.store(0, Ordering::Relaxed);
        prog.1.store(1, Ordering::Relaxed);
        let ctx_prog = prog.clone();
        std::thread::spawn(move || {
            let report = move |done: usize, total: usize| {
                ctx_prog.0.store(done, Ordering::Relaxed);
                ctx_prog.1.store(total, Ordering::Relaxed);
            };
            let r = track_builder::autotune::auto_tune(&line, &p, &report).map_err(|e| format!("{e:#}")).map(|t| Done {
                note: format!("Auto: best of {} builds. {}", t.builds, quality_note(&t.score)),
                built: t.built,
                params: Some(t.params),
            });
            let _ = tx.send(r);
        });
        self.building = Some(rx);
        self.status = "Finding the best settings for this course...".into();
        self.error = None;
    }

    fn current_track(&self) -> Option<&Track> {
        self.built.as_ref().map(|b| &b.solution.track).or(self.loaded_track.as_ref())
    }

    fn save_to(&mut self, path: PathBuf) {
        let Some(t) = self.current_track() else { return };
        let mut t = t.clone();
        t.set_surface(self.params.solve.surface);
        match t.save(&path) {
            Ok(()) => self.status = format!("Saved {}", path.display()),
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("1. Course map");
        ui.horizontal(|ui| {
            if ui.button("Open image / track...").on_hover_text("Open a course map (PNG, JPG, BMP, WebP, GIF, SVG) to trace, or an existing .track file to preview and check").clicked() {
                if let Some(p) = pick_file() {
                    self.open_path(ctx, &p);
                }
            }
        });
        if cfg!(not(windows)) {
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.path_input);
                if ui.button("Load").clicked() {
                    let p = PathBuf::from(self.path_input.trim());
                    self.open_path(ctx, &p);
                }
            });
        }
        if let Some(p) = &self.image_path {
            ui.label(egui::RichText::new(p.file_name().unwrap_or_default().to_string_lossy()).weak());
        }
        let mut retrace = false;
        retrace |= ui
            .add(egui::Slider::new(&mut self.trace_params.dark_threshold, 30..=200).text("line darkness"))
            .on_hover_text("Pixels darker than this count as the course line. Raise it if the line is grey, coloured or faint; lower it if text and shading are being picked up.")
            .changed();
        retrace |= ui
            .checkbox(&mut self.trace_params.include_red, "include red markers")
            .on_hover_text("Also count bright red as part of the line. Many maps put red dots or kerbs on the course; untick if red text or areas get traced.")
            .changed();
        retrace |= ui
            .add(egui::Slider::new(&mut self.trace_params.close_radius, 0..=8).text("gap bridging"))
            .on_hover_text("Joins small breaks in the drawn line (in pixels). Raise it if tracing says the loop isn't closed; too high can merge parts of the course that run close together.")
            .changed();
        retrace |= ui
            .checkbox(&mut self.trace_params.remove_markers, "bridge over markers on the line")
            .on_hover_text("Turn numbers, flags and other blobs sitting on the line are skipped over with a smooth curve instead of being traced around.")
            .changed();
        if retrace {
            self.retrace();
        }
        ui.checkbox(&mut self.params.reverse, "reverse race direction")
            .on_hover_text("Flip which way the cars drive. The green arrow on the map shows the current direction.");
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Course line").strong());
        ui.horizontal_wrapped(|ui| {
            let has_line = self.line.is_some();
            if ui.add_enabled(has_line, egui::Button::new("Edit line").selected(self.edit_mode == EditMode::Edit)).on_hover_text("Fix the traced line by hand: drag points, double-click to add, right-click to delete").clicked() {
                self.edit_mode = if self.edit_mode == EditMode::Edit { EditMode::Off } else { EditMode::Edit };
                self.view = View::Image;
            }
            if self.edit_mode == EditMode::Draw {
                if ui.button("Finish drawing").on_hover_text("Close the loop and stop adding points").clicked() {
                    self.edit_mode = EditMode::Off;
                }
            } else if ui.add_enabled(self.image.is_some(), egui::Button::new("Draw by hand")).on_hover_text("Ignore the trace and click round the course yourself, in race order. Good for photos or maps that won't trace.").clicked() {
                if let Some(l) = self.line.take() {
                    self.undo.push(l);
                }
                self.line = Some(Vec::new());
                self.edit_mode = EditMode::Draw;
                self.view = View::Image;
            }
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo")).on_hover_text("Undo the last line edit (Ctrl+Z)").clicked() {
                self.undo_step();
            }
            if ui.add_enabled(self.traced.is_some(), egui::Button::new("Reset")).on_hover_text("Back to the automatic trace").clicked() {
                if let Some(l) = self.line.take() {
                    self.undo.push(l);
                }
                self.line = self.traced.as_ref().map(|t| simplify_closed(&t.points, 1.2));
                self.edit_mode = EditMode::Off;
            }
        });
        let help = match self.edit_mode {
            EditMode::Off => "Click the line to set start/finish.",
            EditMode::Edit => "Drag points to move them. Double-click the line to add a point. Right-click a point to delete it. Ctrl+Z undoes.",
            EditMode::Draw => "Click around the course in race order. The loop closes itself; press Finish drawing when done.",
        };
        ui.label(egui::RichText::new(help).weak());
        ui.separator();

        ui.heading("2. Fit");
        ui.add(egui::Slider::new(&mut self.params.fit.grid, 10..=50).text("size (tiles)"))
            .on_hover_text("How many tiles the longest side of the track spans. The game allows up to 50. Bigger keeps more detail.");
        ui.add(egui::Slider::new(&mut self.params.fit.exaggeration, 0.5..=3.0).text("bend exaggeration"))
            .on_hover_text("1.0 = true to the map. Above 1 makes small bends bigger so they survive the coarse grid; below 1 flattens them.");
        ui.add(egui::Slider::new(&mut self.params.solve.fidelity, 5.0..=150.0).text("fidelity"))
            .on_hover_text("How hard the track tries to follow the course line. Higher = closer to the map but more kinks; lower = simpler, smoother track.");
        ui.add(egui::Slider::new(&mut self.params.solve.turn_penalty, 0.0..=5.0).text("smoothness"))
            .on_hover_text("Cost of every change of direction. Raise it to get rid of zig-zags and needless kinks; lower it to let the track chase small bends.");
        ui.checkbox(&mut self.params.solve.allow_angled, "use angled pieces (45/20/70 deg)")
            .on_hover_text("Allow the diagonal and transition tiles. Without them diagonals become staircases of corners.");
        egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
            ui.add(egui::Slider::new(&mut self.params.solve.tolerance, 0.0..=0.6).text("tolerance (tiles)"))
                .on_hover_text("Distance from the course that counts as 'on the line' for free. Lets long, gently sloping straights stay dead straight instead of stepping to follow every pixel.");
            ui.add(egui::Slider::new(&mut self.params.solve.wiggle_penalty, 0.0..=6.0).text("lane-change cost"))
                .on_hover_text("Extra cost for straights that only shift sideways a lane. Raise it if straights look wobbly.");
            ui.add(egui::Slider::new(&mut self.params.fit.smoothing, 0.0..=0.03).text("line smoothing"))
                .on_hover_text("Irons out wobbles in the traced line before fitting (as a fraction of the course size). Use a little for hand-drawn or noisy traces.");
            ui.add(egui::Slider::new(&mut self.params.fit.detail_scale, 0.01..=0.1).text("bend size"))
                .on_hover_text("What counts as a 'small' bend for exaggeration, as a fraction of the course size. Larger exaggerates bigger features too.");
            if ui.button("Reset settings").on_hover_text("Put every fit setting back to its default").clicked() {
                let keep = (self.params.start_px, self.params.reverse, self.params.solve.surface);
                self.params = BuildParams::default();
                (self.params.start_px, self.params.reverse, self.params.solve.surface) = keep;
            }
        });
        egui::ComboBox::from_label("surface")
            .selected_text(self.params.solve.surface.label())
            .show_ui(ui, |ui| {
                for s in Surface::ALL {
                    ui.selectable_value(&mut self.params.solve.surface, s, s.label());
                }
            })
            .response
            .on_hover_text("Road surface for the whole track. Dirt and ice change how the cars grip.");
        let can_build = self.line.as_ref().is_some_and(|l| l.len() >= 3) && self.building.is_none() && self.edit_mode != EditMode::Draw;
        ui.horizontal(|ui| {
            if ui.add_enabled(can_build, egui::Button::new("Build track").min_size(Vec2::new(150.0, 30.0))).on_hover_text("Fit tiles to the course line with the settings above. Takes about a second.").clicked() {
                self.start_build();
            }
            if ui
                .add_enabled(can_build, egui::Button::new("Auto settings").min_size(Vec2::new(120.0, 30.0)))
                .on_hover_text("Try a few dozen combinations of exaggeration, smoothness, tolerance and fidelity, and keep the track that matches the course map best: close to the line, no lost corners, no zig-zags. The sliders are updated to the winning settings. Takes a few seconds.")
                .clicked()
            {
                self.start_auto();
            }
        });
        if self.building.is_some() {
            let (done, total) = (self.progress.0.load(Ordering::Relaxed), self.progress.1.load(Ordering::Relaxed));
            ui.horizontal(|ui| {
                ui.spinner();
                if total > 1 {
                    ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(format!("{done} / {total} builds")).desired_width(220.0));
                } else {
                    ui.label("solving...");
                }
            });
        }
        if let Some(b) = &self.built {
            let s = &b.solution;
            let (w, h) = s.track.size();
            ui.label(format!("{} tiles, {w} x {h}", s.track.tiles.len()));
            ui.label(format!("average distance from course: {:.2} tiles", s.mean_dev))
                .on_hover_text("How far the track's racing line is from the (exaggerated) course line, on average");
            ui.label(format!("lap score: {:.0}", s.track.lap_score));
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Start/finish:").weak());
                if ui.button("Flip direction").on_hover_text("Make the cars drive the other way round the track").clicked() {
                    if let Some(t) = self.current_track_mut() {
                        t.reverse_direction();
                    }
                }
            })
            .response
            .on_hover_text("Click any tile in the Track view to move the start/finish there");
            if let Some(q) = &self.quality {
                ui.label(egui::RichText::new(q).weak()).on_hover_text("Match score against the course as drawn (lower is better): average gaps either way, worst miss, and how much the track turns compared with the course. Zig-zags add turning; flattened corners remove it.");
            }
        }
        ui.separator();

        ui.heading("3. Save");
        ui.horizontal(|ui| {
            ui.label("name");
            ui.text_edit_singleline(&mut self.name).on_hover_text("Track name shown in the game (also the file name)");
        });
        let dir = game_tracks_dir();
        let has_track = self.current_track().is_some();
        let target = dir.as_ref().map(|d| d.join(format!("{}.track", sanitize(&self.name))));
        let exists = target.as_ref().is_some_and(|t| t.exists());
        if exists {
            ui.checkbox(&mut self.overwrite, "a track with this name exists - overwrite");
        }
        if ui.add_enabled(has_track && target.is_some() && (!exists || self.overwrite), egui::Button::new("Save to game")).on_hover_text("Write the track into the game's tracks folder. It shows up in the game's track list under this name.").clicked() {
            if let Some(t) = target.clone() {
                self.save_to(t);
            }
        }
        if let Some(d) = &dir {
            ui.label(egui::RichText::new(d.display().to_string()).weak().small());
        }
        if ui.add_enabled(has_track, egui::Button::new("Save as...")).on_hover_text("Save the .track file somewhere else").clicked() {
            if let Some(p) = save_file(&self.name) {
                self.save_to(p);
            }
        }
        ui.separator();
        ui.label(&self.status);
        if let Some(e) = &self.error {
            ui.colored_label(Color32::from_rgb(255, 120, 110), e);
        }
    }

    fn undo_step(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.line = Some(prev);
            self.drag_idx = None;
        }
    }

    fn image_view(&mut self, ui: &mut egui::Ui) {
        let (Some(tex), Some(img)) = (&self.texture, &self.image) else {
            ui.centered_and_justified(|ui| ui.label("Drop a course map here (PNG, JPG or SVG)"));
            return;
        };
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let avail = ui.available_size();
        let scale = (avail.x / iw).min(avail.y / ih);
        let (resp, painter) = ui.allocate_painter(Vec2::new(iw * scale, ih * scale), Sense::click_and_drag());
        let rect = resp.rect;
        painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        let to_screen = |p: P| Pos2::new(rect.min.x + p.0 as f32 * scale, rect.min.y + p.1 as f32 * scale);
        let to_image = |q: Pos2| (((q.x - rect.min.x) / scale) as f64, ((q.y - rect.min.y) / scale) as f64);
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let nearest_handle = |line: &[P], q: Pos2| -> Option<usize> {
            (0..line.len())
                .map(|i| (i, to_screen(line[i]).distance(q)))
                .filter(|(_, d)| *d < 9.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };

        // ---- interaction
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
            self.undo_step();
        }
        match self.edit_mode {
            EditMode::Off => {
                if resp.clicked() {
                    if let Some(pos) = pointer {
                        self.params.start_px = Some(to_image(pos));
                        self.status = "Start/finish set. Build when ready (tick 'reverse' if the arrow points the wrong way).".into();
                    }
                }
            }
            EditMode::Draw => {
                if resp.clicked() {
                    if let (Some(pos), Some(line)) = (pointer, self.line.as_mut()) {
                        self.undo.push(line.clone());
                        line.push(to_image(pos));
                    }
                }
            }
            EditMode::Edit => {
                if let Some(line) = self.line.as_mut() {
                    if resp.drag_started() {
                        // pick the handle under where the button went down, not where the pointer is now
                        let origin = ui.input(|i| i.pointer.press_origin()).or(pointer);
                        if let Some(pos) = origin {
                            self.drag_idx = nearest_handle(line, pos);
                            if self.drag_idx.is_some() {
                                self.undo.push(line.clone());
                            }
                        }
                    }
                    if let (Some(i), Some(pos)) = (self.drag_idx, pointer) {
                        if resp.dragged() && i < line.len() {
                            line[i] = to_image(pos);
                        }
                    }
                    if resp.drag_stopped() {
                        self.drag_idx = None;
                    }
                    if resp.secondary_clicked() {
                        if let Some(i) = pointer.and_then(|pos| nearest_handle(line, pos)) {
                            if line.len() > 3 {
                                self.undo.push(line.clone());
                                line.remove(i);
                            }
                        }
                    }
                    if resp.double_clicked() {
                        if let Some(pos) = pointer {
                            // insert into the closest segment
                            let q = to_image(pos);
                            let n = line.len();
                            let seg = (0..n).min_by(|&a, &b| seg_dist(q, line[a], line[(a + 1) % n]).total_cmp(&seg_dist(q, line[b], line[(b + 1) % n])));
                            if let Some(a) = seg {
                                self.undo.push(line.clone());
                                line.insert(a + 1, q);
                            }
                        }
                    }
                }
            }
        }

        // ---- drawing
        let blue = Color32::from_rgb(0, 170, 255);
        if let Some(line) = &self.line {
            let pts: Vec<Pos2> = line.iter().map(|&p| to_screen(p)).collect();
            if self.edit_mode == EditMode::Draw {
                if pts.len() >= 2 {
                    painter.add(egui::Shape::line(pts.clone(), Stroke::new(2.0, blue)));
                }
                if let (Some(last), Some(h)) = (pts.last(), resp.hover_pos()) {
                    painter.line_segment([*last, h], Stroke::new(1.0, blue.gamma_multiply(0.6)));
                    if let Some(first) = pts.first() {
                        painter.line_segment([h, *first], Stroke::new(1.0, blue.gamma_multiply(0.3)));
                    }
                }
            } else if pts.len() >= 2 {
                painter.add(egui::Shape::closed_line(pts.clone(), Stroke::new(2.0, blue)));
            }
            if self.edit_mode != EditMode::Off {
                let hover = resp.hover_pos().and_then(|q| nearest_handle(line, q));
                for (i, p) in pts.iter().enumerate() {
                    let hot = Some(i) == hover || Some(i) == self.drag_idx;
                    painter.circle(*p, if hot { 5.5 } else { 3.5 }, if hot { Color32::YELLOW } else { Color32::WHITE }, Stroke::new(1.0, Color32::BLACK));
                }
            }
            if self.params.start_px.is_some() && line.len() >= 3 && self.edit_mode != EditMode::Draw {
                let o = track_builder::fit::orient(line, self.params.start_px, self.params.reverse);
                let a = to_screen(o[0]);
                // arrow towards a point a little further along
                let target = o.iter().skip(1).map(|&p| to_screen(p)).find(|p| p.distance(a) > 20.0).unwrap_or(to_screen(o[1]));
                painter.circle_filled(a, 6.0, Color32::from_rgb(40, 220, 80));
                painter.arrow(a, (target - a).normalized() * 28.0, Stroke::new(3.0, Color32::from_rgb(40, 220, 80)));
            }
        }
    }

    /// Draws the track; returns the tile that was clicked, if any.
    fn track_view(&self, ui: &mut egui::Ui) -> Option<(i32, i32)> {
        let Some(track) = self.current_track() else {
            ui.centered_and_justified(|ui| ui.label("Build a track to preview it here"));
            return None;
        };
        let ((x0, y0), _) = track.bounds();
        let (w, h) = track.size();
        let avail = ui.available_size();
        let cell = ((avail.x - 20.0) / (w as f32 + 2.0)).min((avail.y - 20.0) / (h as f32 + 2.0)).max(2.0);
        let (resp, painter) = ui.allocate_painter(Vec2::new(cell * (w as f32 + 2.0), cell * (h as f32 + 2.0)), Sense::click());
        let r = resp.rect;
        let clicked = if resp.clicked() {
            resp.interact_pointer_pos().map(|q| {
                (((q.x - r.min.x) / cell - 1.0).floor() as i32 + x0, ((q.y - r.min.y) / cell - 1.0).floor() as i32 + y0)
            })
        } else {
            None
        };
        if resp.hovered() {
            resp.clone().on_hover_text("Click a tile on the track to move the start/finish there");
        }
        painter.rect_filled(r, 0.0, Color32::from_rgb(125, 143, 158));
        let grid = Stroke::new(1.0, Color32::from_rgb(102, 120, 136));
        for i in 0..=(w + 2) {
            let x = r.min.x + i as f32 * cell;
            painter.line_segment([Pos2::new(x, r.min.y), Pos2::new(x, r.max.y)], grid);
        }
        for j in 0..=(h + 2) {
            let y = r.min.y + j as f32 * cell;
            painter.line_segment([Pos2::new(r.min.x, y), Pos2::new(r.max.x, y)], grid);
        }
        let to_screen = |p: P| Pos2::new(r.min.x + ((p.0 - x0 as f64 + 1.0) as f32) * cell, r.min.y + ((p.1 - y0 as f64 + 1.0) as f32) * cell);
        let course: Option<Vec<Pos2>> = self.built.as_ref().map(|b| {
            let (ox, oy) = b.solution.offset;
            b.guide.outline.iter().map(|&(x, y)| to_screen((x - ox as f64 + x0 as f64, y - oy as f64 + y0 as f64))).collect()
        });
        if let Some(lap) = track.lap() {
            let mut pts = Vec::new();
            for seg in lap_curve(&lap) {
                for i in 0..12 {
                    pts.push(to_screen(bezier(&seg, i as f64 / 12.0)));
                }
            }
            painter.add(egui::Shape::closed_line(pts.clone(), Stroke::new(cell * 0.34, Color32::WHITE)));
            painter.add(egui::Shape::closed_line(pts, Stroke::new(cell * 0.27, Color32::from_rgb(79, 97, 112))));
            if let Some(c) = course {
                painter.add(egui::Shape::closed_line(c, Stroke::new(1.5, Color32::from_rgb(255, 211, 77))));
            }
            let s = track.start;
            let c = to_screen((s.0 as f64 + 0.5, s.1 as f64 + 0.5));
            let green = Color32::from_rgb(40, 220, 80);
            painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(cell * 0.9)), 0.0, Stroke::new(2.0, green), egui::StrokeKind::Inside);
            // arrow: where the cars come in, pointing the way they drive
            let (px, py) = track.start_entry.pos();
            let (nx, ny) = track.start_entry.inward();
            let a = to_screen((s.0 as f64 + px, s.1 as f64 + py));
            painter.arrow(a - Vec2::new(nx as f32, ny as f32) * cell * 0.8, Vec2::new(nx as f32, ny as f32) * cell * 1.6, Stroke::new(3.0, green));
        } else {
            for (&(x, y), _) in &track.tiles {
                let c = to_screen((x as f64 + 0.5, y as f64 + 0.5));
                painter.rect_filled(Rect::from_center_size(c, Vec2::splat(cell * 0.8)), 0.0, Color32::from_rgb(173, 95, 104));
            }
        }
        clicked
    }

    fn current_track_mut(&mut self) -> Option<&mut Track> {
        match self.built.as_mut() {
            Some(b) => Some(&mut b.solution.track),
            None => self.loaded_track.as_mut(),
        }
    }

    fn move_start(&mut self, cell: (i32, i32)) {
        if let Some(t) = self.current_track_mut() {
            if t.set_start_at(cell) {
                let s = t.start;
                self.status = format!("Start/finish moved to tile {}, {}.", s.0, s.1);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        // dropped files
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(p) = dropped.first() {
            self.open_path(ctx, p);
        }
        // finished build?
        if let Some(rx) = &self.building {
            match rx.try_recv() {
                Ok(Ok(done)) => {
                    let b = done.built;
                    if let Some(p) = done.params {
                        self.params = p;
                    }
                    self.quality = Some(done.note);
                    self.progress.1.store(0, Ordering::Relaxed);
                    self.status = "Built - check the Track tab, then save.".into();
                    self.built = Some(b);
                    self.loaded_track = None;
                    self.building = None;
                    self.view = View::Track;
                }
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.building = None;
                    self.status = "Build failed.".into();
                }
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }

        egui::Panel::top("mode").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Cat's AI Ltd Tools").strong().size(15.0));
                ui.separator();
                ui.selectable_value(&mut self.mode, Mode::Builder, RichText::new("🛣  Track Builder").strong())
                    .on_hover_text("Turn a course map into a track");
                ui.selectable_value(&mut self.mode, Mode::Lab, RichText::new("🧬  Evolution Lab").strong())
                    .on_hover_text("See which training settings worked, from your saved networks (read-only)");
            });
        });
        if self.mode == Mode::Lab {
            self.lab.get_or_insert_with(|| crate::gui_lab::Lab::new(None)).ui(ui);
            return;
        }
        egui::Panel::left("controls").resizable(false).exact_size(330.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui, ctx));
        });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.view, View::Image, "Course image");
                ui.selectable_value(&mut self.view, View::Track, "Track");
            });
            ui.separator();
            match self.view {
                View::Image => self.image_view(ui),
                View::Track => {
                    if let Some(c) = self.track_view(ui) {
                        self.move_start(c);
                    }
                }
            }
        });
    }
}

fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "Untitled".into() } else { s }
}

#[cfg(windows)]
fn pick_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Course map or track", &[track_builder::input::IMAGE_EXTENSIONS, &["track"]].concat())
        .pick_file()
}

#[cfg(not(windows))]
fn pick_file() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn save_file(name: &str) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new().add_filter("Track", &["track"]).set_file_name(format!("{}.track", sanitize(name)));
    if let Some(dir) = game_tracks_dir() {
        d = d.set_directory(dir);
    }
    d.save_file()
}

#[cfg(not(windows))]
fn save_file(_name: &str) -> Option<PathBuf> {
    None
}

fn seg_dist(q: P, a: P, b: P) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 < 1e-12 { 0.0 } else { (((q.0 - a.0) * dx + (q.1 - a.1) * dy) / l2).clamp(0.0, 1.0) };
    let (px, py) = (a.0 + t * dx, a.1 + t * dy);
    ((q.0 - px).powi(2) + (q.1 - py).powi(2)).sqrt()
}

fn quality_note(s: &track_builder::autotune::Score) -> String {
    format!(
        "Match score {:.2}: average gap {:.2} tiles, worst {:.2}; turns {:.1}x vs course {:.1}x",
        s.total,
        (s.track_to_course + s.course_to_track) / 2.0,
        s.course_to_track_max,
        s.turning,
        s.course_turning
    )
}
