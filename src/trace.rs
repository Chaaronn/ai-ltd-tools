//! Turn a course map image into an ordered centre-line loop (pixel coordinates).
//!
//! Approach: threshold the drawn line, clean it up morphologically, then take the largest
//! enclosed background region (the inside of the loop) and trace its outline. Using the
//! enclosed region rather than a skeleton makes text labels, legends and spurs irrelevant:
//! they never change the outline of the inside of the loop.

use anyhow::{bail, Result};
use image::RgbImage;

#[derive(Clone, Debug)]
pub struct TraceParams {
    /// A pixel is "line" if all its channels are below this.
    pub dark_threshold: u8,
    /// Also treat saturated red pixels (markers drawn on the line) as line.
    pub include_red: bool,
    /// Radius of the opening that removes thin text strokes.
    pub open_radius: i32,
    /// Radius of the closing that bridges small gaps in the line.
    pub close_radius: i32,
    /// Stop markers drawn on the line (numbered circles, flags...) bending the traced line.
    pub remove_markers: bool,
}

impl Default for TraceParams {
    fn default() -> Self {
        TraceParams { dark_threshold: 80, include_red: true, open_radius: 1, close_radius: 3, remove_markers: true }
    }
}

#[derive(Clone, Debug)]
pub struct Traced {
    /// Ordered loop, roughly one point per pixel, on the middle of the drawn line.
    pub points: Vec<(f64, f64)>,
    pub line_width: f64,
    /// Number of places where a marker on the line was bridged over.
    pub markers_removed: usize,
    pub width: u32,
    pub height: u32,
}

struct Mask {
    w: i32,
    h: i32,
    d: Vec<bool>,
}

impl Mask {
    fn get(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h && self.d[(y * self.w + x) as usize]
    }
    fn idx(&self, x: i32, y: i32) -> usize {
        (y * self.w + x) as usize
    }
    /// Separable square-ish disk: run a min/max filter with a circular footprint.
    fn morph(&self, r: i32, dilate: bool) -> Mask {
        if r <= 0 {
            return Mask { w: self.w, h: self.h, d: self.d.clone() };
        }
        let offs: Vec<(i32, i32)> = (-r..=r)
            .flat_map(|dy| (-r..=r).map(move |dx| (dx, dy)))
            .filter(|(dx, dy)| (dx * dx + dy * dy) as f64 <= (r as f64 + 0.5).powi(2))
            .collect();
        let mut d = vec![false; self.d.len()];
        for y in 0..self.h {
            for x in 0..self.w {
                let v = if dilate {
                    offs.iter().any(|(dx, dy)| self.get(x + dx, y + dy))
                } else {
                    offs.iter().all(|(dx, dy)| {
                        let (xx, yy) = (x + dx, y + dy);
                        // outside the image counts as background for erosion
                        self.get(xx, yy)
                    })
                };
                d[self.idx(x, y)] = v;
            }
        }
        Mask { w: self.w, h: self.h, d }
    }
}

pub fn trace(img: &RgbImage, p: &TraceParams) -> Result<Traced> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let mut d = Vec::with_capacity((w * h) as usize);
    for px in img.pixels() {
        let [r, g, b] = px.0;
        let dark = r.max(g).max(b) < p.dark_threshold;
        let red = p.include_red && r > 180 && g < 120 && b < 120;
        d.push(dark || red);
    }
    let raw = Mask { w, h, d };
    let opened = raw.morph(p.open_radius, false).morph(p.open_radius, true);
    let line = opened.morph(p.close_radius, true).morph(p.close_radius, false);

    // label background regions (4-connected); keep the largest one not touching the border
    let mut label = vec![u32::MAX; line.d.len()];
    let mut best: Option<(u32, usize)> = None;
    let mut next = 0u32;
    let mut borders: Vec<bool> = Vec::new();
    let mut stack = Vec::new();
    for sy in 0..h {
        for sx in 0..w {
            let si = line.idx(sx, sy);
            if line.d[si] || label[si] != u32::MAX {
                continue;
            }
            let (mut area, mut border) = (0usize, false);
            label[si] = next;
            stack.push((sx, sy));
            while let Some((x, y)) = stack.pop() {
                area += 1;
                if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                    border = true;
                }
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w || ny >= h {
                        continue;
                    }
                    let ni = line.idx(nx, ny);
                    if !line.d[ni] && label[ni] == u32::MAX {
                        label[ni] = next;
                        stack.push((nx, ny));
                    }
                }
            }
            borders.push(border);
            if !border && best.is_none_or(|(_, a)| area > a) {
                best = Some((next, area));
            }
            next += 1;
        }
    }
    let Some((hole_label, hole_area)) = best else {
        bail!("couldn't find a closed loop in the image - try a different darkness threshold");
    };
    if hole_area < 500 {
        bail!("the enclosed area is tiny ({hole_area} px) - the line is probably broken somewhere");
    }
    let mut hole = Mask { w, h, d: label.iter().map(|&l| l == hole_label).collect() };
    // "solid" line: also fill every other enclosed pocket. That turns ringed markers (a dark
    // outline round a coloured disc) and two-tone lines (dark edges, coloured centre) solid.
    let line = Mask {
        w,
        h,
        d: (0..line.d.len())
            .map(|i| line.d[i] || (label[i] != u32::MAX && label[i] != hole_label && !borders[label[i] as usize]))
            .collect(),
    };

    // estimate the line width by measuring how far the line extends outward from the hole
    let outline = moore_trace(&hole);
    let mut runs: Vec<f64> = Vec::new();
    let n = outline.len();
    for i in (0..n).step_by((n / 200).max(1)) {
        let a = outline[(i + n - 3) % n];
        let b = outline[(i + 3) % n];
        let (tx, ty) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
        let len = (tx * tx + ty * ty).sqrt().max(1e-6);
        let (p0, p1) = (outline[i].0 as f64, outline[i].1 as f64);
        // try both normals; keep the one that enters the line
        for sgn in [1.0, -1.0] {
            let (nx, ny) = (-ty / len * sgn, tx / len * sgn);
            let mut k = 1.0;
            while k < 60.0 && line.get((p0 + nx * k).round() as i32, (p1 + ny * k).round() as i32) {
                k += 1.0;
            }
            if k > 1.5 && k < 60.0 {
                runs.push(k - 1.0);
                break;
            }
        }
    }
    runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let line_width = runs.get(runs.len() / 2).copied().unwrap_or(4.0);

    // grow the hole into the line by half its width so the outline sits on the line's centre
    for _ in 0..((line_width / 2.0).round() as i32) {
        let grown = hole.morph(1, true);
        for i in 0..hole.d.len() {
            hole.d[i] = hole.d[i] || (grown.d[i] && line.d[i]);
        }
    }
    let pts = moore_trace(&hole);
    if pts.len() < 50 {
        bail!("traced outline is too short");
    }
    let mut points: Vec<(f64, f64)> = pts.into_iter().map(|(x, y)| (x as f64 + 0.5, y as f64 + 0.5)).collect();
    let markers_removed = if p.remove_markers { bridge_markers(&line, line_width, &mut points) } else { 0 };
    Ok(Traced {
        points,
        line_width,
        markers_removed,
        width: w as u32,
        height: h as u32,
    })
}

/// Moore-neighbour boundary trace of the region `m` (8-connected), clockwise on screen.
fn moore_trace(m: &Mask) -> Vec<(i32, i32)> {
    // start: first region pixel in raster order (its west neighbour is outside)
    let start = (0..m.h).flat_map(|y| (0..m.w).map(move |x| (x, y))).find(|&(x, y)| m.get(x, y));
    let Some(start) = start else { return vec![] };
    // neighbours clockwise starting from west
    const NB: [(i32, i32); 8] = [(-1, 0), (-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1)];
    let mut out = vec![start];
    let mut cur = start;
    let mut back = 0usize; // direction index pointing to the last background pixel examined
    let limit = (m.w * m.h * 4) as usize;
    for _ in 0..limit {
        let mut found = None;
        for k in 1..=8 {
            let dir = (back + k) % 8;
            let (dx, dy) = NB[dir];
            if m.get(cur.0 + dx, cur.1 + dy) {
                found = Some(dir);
                break;
            }
        }
        let Some(dir) = found else { break }; // isolated pixel
        let (dx, dy) = NB[dir];
        let nxt = (cur.0 + dx, cur.1 + dy);
        // backtrack direction: from the new pixel, point at the cell we came from, then step back
        back = (dir + 4 + 2) % 8; // previous background neighbour relative to the new pixel
        back = (back + 8 - 1) % 8;
        if nxt == start && out.len() > 2 {
            break;
        }
        out.push(nxt);
        cur = nxt;
    }
    out
}

/// Two-pass chamfer distance (3-4 weights, result in pixels) from every pixel to the nearest
/// pixel where `inside(i)` is false.
fn distance_to_outside(w: i32, h: i32, inside: impl Fn(usize) -> bool) -> Vec<f32> {
    let big = 1e9f32;
    let mut d: Vec<f32> = (0..(w * h) as usize).map(|i| if inside(i) { big } else { 0.0 }).collect();
    let at = |x: i32, y: i32| (y * w + x) as usize;
    for y in 0..h {
        for x in 0..w {
            let i = at(x, y);
            if d[i] == 0.0 {
                continue;
            }
            let mut v = d[i];
            if x > 0 { v = v.min(d[at(x - 1, y)] + 3.0); }
            if y > 0 {
                v = v.min(d[at(x, y - 1)] + 3.0);
                if x > 0 { v = v.min(d[at(x - 1, y - 1)] + 4.0); }
                if x < w - 1 { v = v.min(d[at(x + 1, y - 1)] + 4.0); }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = at(x, y);
            if d[i] == 0.0 {
                continue;
            }
            let mut v = d[i];
            if x < w - 1 { v = v.min(d[at(x + 1, y)] + 3.0); }
            if y < h - 1 {
                v = v.min(d[at(x, y + 1)] + 3.0);
                if x < w - 1 { v = v.min(d[at(x + 1, y + 1)] + 4.0); }
                if x > 0 { v = v.min(d[at(x - 1, y + 1)] + 4.0); }
            }
            d[i] = v;
        }
    }
    d.iter_mut().for_each(|v| *v /= 3.0);
    d
}

/// Markers are places where the (solid) line is much thicker than normal. Find them with a
/// distance transform, then replace the stretches of the traced line that pass them with a
/// smooth curve between where the line enters and leaves. Returns how many were bridged.
fn bridge_markers(line: &Mask, line_width: f64, pts: &mut [(f64, f64)]) -> usize {
    let (w, h) = (line.w, line.h);
    let half = (line_width / 2.0).max(1.0) as f32;
    let depth = distance_to_outside(w, h, |i| line.d[i]);
    // cores: pixels deeper inside the line than a normal line could ever be
    let core_r = half + 2.5;
    let is_core: Vec<bool> = depth.iter().map(|&v| v > core_r).collect();
    if !is_core.iter().any(|&c| c) {
        return 0;
    }
    // distance from each pixel to the nearest core pixel
    let to_core = distance_to_outside(w, h, |i| !is_core[i]);
    // the core radius at each core pixel tells us how big the marker is; a point on the traced line
    // is "affected" when it is within (marker radius + a little) of a core
    let n = pts.len();
    let affected: Vec<bool> = pts
        .iter()
        .map(|&(x, y)| {
            let (xi, yi) = (x as i32, y as i32);
            if xi < 0 || yi < 0 || xi >= w || yi >= h {
                return false;
            }
            to_core[(yi * w + xi) as usize] <= core_r + half + 3.0
        })
        .collect();
    if affected.iter().all(|&a| a) {
        return 0;
    }
    // walk runs of affected points (with wrap-around), starting from an unaffected point
    let start = affected.iter().position(|&a| !a).unwrap();
    let mut bridged = 0;
    let mut k = 0;
    let max_chord = (line_width * 10.0).max(40.0);
    while k < n {
        let i = (start + k) % n;
        if !affected[i] {
            k += 1;
            continue;
        }
        let mut len = 0;
        while len < n && affected[(i + len) % n] {
            len += 1;
        }
        let a = (i + n - 1) % n; // last good point before the run
        let b = (i + len) % n; // first good point after
        let (pa, pb) = (pts[a], pts[b]);
        let chord = ((pb.0 - pa.0).powi(2) + (pb.1 - pa.1).powi(2)).sqrt();
        if chord <= max_chord && len < n / 4 {
            // Hermite curve using the line's direction just outside the run
            let ta = tangent_at(pts, a, -1, 6);
            let tb = tangent_at(pts, b, 1, 6);
            let m = chord;
            for j in 0..len {
                let t = (j + 1) as f64 / (len + 1) as f64;
                let (h00, h10, h01, h11) = (2.0 * t * t * t - 3.0 * t * t + 1.0, t * t * t - 2.0 * t * t + t, -2.0 * t * t * t + 3.0 * t * t, t * t * t - t * t);
                pts[(i + j) % n] = (
                    h00 * pa.0 + h10 * m * ta.0 + h01 * pb.0 + h11 * m * tb.0,
                    h00 * pa.1 + h10 * m * ta.1 + h01 * pb.1 + h11 * m * tb.1,
                );
            }
            bridged += 1;
        }
        k += len;
    }
    bridged
}

/// Unit direction of travel at `i`, looking `span` points back (dir = -1) or ahead (dir = 1).
fn tangent_at(pts: &[(f64, f64)], i: usize, dir: i64, span: usize) -> (f64, f64) {
    let n = pts.len() as i64;
    let j = ((i as i64 + dir * span as i64).rem_euclid(n)) as usize;
    let (p, q) = if dir < 0 { (pts[j], pts[i]) } else { (pts[i], pts[j]) };
    let (dx, dy) = (q.0 - p.0, q.1 - p.1);
    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
    (dx / l, dy / l)
}
