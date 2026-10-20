// Hide the console window on Windows release builds (the CLI is a separate binary).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod gui;
mod gui_lab;
mod gui_simulation;
mod theme;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Cat's AI Ltd Tools")
            .with_inner_size([1280.0, 820.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Cat's AI Ltd Tools", options, Box::new(|cc| Ok(Box::new(gui::App::new(cc)))))
}
