//! The app's look: one dark palette, one accent, IBM Plex Sans for text and IBM Plex Mono for
//! numbers. Colours are named by role so screens don't pick their own.
use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Vec2};
use std::sync::Arc;

const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Window background.
pub const BG: Color32 = rgb(0x15171b);
/// Side panels and the top bar.
pub const PANEL: Color32 = rgb(0x181a1f);
/// Inputs and inset tracks.
pub const SUNKEN: Color32 = rgb(0x121418);
/// Hovered rows.
pub const HOVER: Color32 = rgb(0x20242a);
/// Selected rows and the current tab.
pub const SELECTED: Color32 = rgb(0x262b33);
/// Panel edges and table header rules.
pub const BORDER: Color32 = rgb(0x262a31);
/// Row dividers.
pub const DIVIDER: Color32 = rgb(0x22262c);

pub const TEXT: Color32 = rgb(0xe4e7eb);
/// Secondary text (row labels in the sidebar).
pub const TEXT2: Color32 = rgb(0xb7bdc6);
/// Descriptions and sublines (7:1 on BG).
pub const MUTED: Color32 = rgb(0x9aa1ab);
/// Column headers and counts (5:1 on BG).
pub const DIM: Color32 = rgb(0x8a919c);

/// The one accent: primary actions, progress, selection.
pub const ACCENT: Color32 = rgb(0x7fb2ff);
/// Text on the accent.
pub const ON_ACCENT: Color32 = rgb(0x0f1a2b);

/// Status: good / running.
pub const GOOD: Color32 = rgb(0x8fe0ad);
pub const GOOD_DOT: Color32 = rgb(0x5cc98a);
/// Status: waiting / paused.
pub const WARN: Color32 = rgb(0xf0c977);
pub const WARN_DOT: Color32 = rgb(0xe0b45a);
/// Status: worse / failed.
pub const BAD: Color32 = rgb(0xf0a39b);

pub fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Install the fonts and colours. Call once at start-up.
pub fn apply(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "plex-sans", include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"));
    add(&mut fonts, "plex-sans-medium", include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"));
    add(&mut fonts, "plex-sans-semibold", include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"));
    add(&mut fonts, "plex-mono", include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"));
    // egui's own fonts stay as fallbacks for symbols Plex doesn't have
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mono_fallback = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let family = |first: &str, rest: &[String]| std::iter::once(first.to_string()).chain(rest.iter().cloned()).collect::<Vec<_>>();
    fonts.families.insert(FontFamily::Proportional, family("plex-sans", &fallback));
    fonts.families.insert(FontFamily::Monospace, family("plex-mono", &mono_fallback));
    fonts.families.insert(FontFamily::Name("medium".into()), family("plex-sans-medium", &fallback));
    fonts.families.insert(FontFamily::Name("semibold".into()), family("plex-sans-semibold", &fallback));
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.extreme_bg_color = SUNKEN;
    v.faint_bg_color = rgb(0x191b20);
    v.code_bg_color = SUNKEN;
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARN;
    v.error_fg_color = BAD;
    v.weak_text_color = Some(MUTED);
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = rgb(0x2b3f5c);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    let r = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = BG;
    w.noninteractive.weak_bg_fill = BG;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = rgb(0x20242a);
    w.inactive.weak_bg_fill = rgb(0x20242a);
    w.inactive.bg_stroke = Stroke::new(1.0, rgb(0x2c313a));
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT2);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = SELECTED;
    w.hovered.weak_bg_fill = SELECTED;
    w.hovered.bg_stroke = Stroke::new(1.0, rgb(0x3a404a));
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = rgb(0x2e3540);
    w.active.weak_bg_fill = rgb(0x2e3540);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.0, TEXT);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;
    w.open.bg_fill = SELECTED;
    w.open.weak_bg_fill = SELECTED;
    w.open.corner_radius = r;
    ctx.set_visuals(v);

    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.spacing.interact_size.y = 24.0;
        use egui::TextStyle::*;
        s.text_styles.insert(Heading, semibold(22.0));
        s.text_styles.insert(Body, sans(14.0));
        s.text_styles.insert(Button, sans(13.5));
        s.text_styles.insert(Small, sans(11.5));
        s.text_styles.insert(Monospace, mono(13.0));
    });
}

/// A small coloured dot, for status.
pub fn dot(ui: &mut egui::Ui, colour: Color32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, colour);
    resp
}

/// An uppercase column header in the table style.
pub fn header(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase()).font(sans(11.0)).color(DIM)
}

/// A thin horizontal rule.
pub fn rule(ui: &mut egui::Ui, colour: Color32) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 0.0, colour);
}

/// The primary action button (accent fill).
pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.to_string()).font(medium(13.5)).color(ON_ACCENT))
        .fill(ACCENT)
        .corner_radius(CornerRadius::same(8))
        .min_size(Vec2::new(0.0, 34.0))
}
