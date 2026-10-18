//! Loading course maps: raster images (PNG, JPG, BMP, WebP, GIF) and vector SVG / SVGZ.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use image::RgbImage;
use resvg::{tiny_skia, usvg};

/// File extensions the app accepts as course maps.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "webp", "gif", "svg", "svgz"];

/// SVGs are rendered so their longest side is this many pixels. Big enough that a thin
/// course line is a few pixels wide; small enough that tracing stays fast.
pub const SVG_RENDER_SIZE: f32 = 1600.0;

pub fn is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"))
}

/// Load any supported course map as an RGB image (SVGs are rendered onto white).
pub fn load_image(path: &Path) -> Result<RgbImage> {
    if is_svg(path) {
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let dir = std::fs::canonicalize(path).ok().and_then(|p| p.parent().map(|p| p.to_path_buf()));
        return render_svg(&data, dir);
    }
    Ok(image::open(path).with_context(|| format!("opening {}", path.display()))?.to_rgb8())
}

/// Render SVG (or gzip-compressed SVGZ) bytes to an RGB image on a white background.
pub fn render_svg(data: &[u8], resources_dir: Option<std::path::PathBuf>) -> Result<RgbImage> {
    let mut opt = usvg::Options { resources_dir, ..usvg::Options::default() };
    // labels are nice in the preview; the tracer strips thin text strokes anyway
    opt.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(data, &opt).map_err(|e| anyhow!("couldn't read SVG: {e}"))?;
    let size = tree.size();
    let scale = SVG_RENDER_SIZE / size.width().max(size.height()).max(1.0);
    let (w, h) = ((size.width() * scale).ceil().max(1.0) as u32, (size.height() * scale).ceil().max(1.0) as u32);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| anyhow!("SVG is too large to render"))?;
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    // the background is opaque white, so the premultiplied RGBA is plain RGB
    let rgb: Vec<u8> = pixmap.data().chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    RgbImage::from_raw(w, h, rgb).ok_or_else(|| anyhow!("SVG render produced a bad buffer"))
}
