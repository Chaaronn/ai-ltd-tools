//! Prepare the traced loop for the solver: choose start and direction, exaggerate small
//! bends, scale into the grid, and precompute per-cell distance / progress.

use std::collections::HashMap;

pub type P = (f64, f64);

#[derive(Clone, Debug)]
pub struct FitParams {
    /// Largest track dimension in tiles (the game allows 50).
    pub grid: i32,
    /// 1.0 = faithful; >1 scales up bends that are small compared with the whole course.
    pub exaggeration: f64,
    /// Size of "small" for exaggeration, as a fraction of the course's largest dimension.
    pub detail_scale: f64,
    /// Extra smoothing of the course line before fitting, as a fraction of the course's size
    /// (0 = only remove pixel noise). Irons out wobbly traces and small kinks.
    pub smoothing: f64,
    pub margin: f64,
}

impl Default for FitParams {
    fn default() -> Self {
        FitParams { grid: 50, exaggeration: 1.6, detail_scale: 0.042, smoothing: 0.0, margin: 0.8 }
    }
}

/// Guide in grid units, densely sampled, with arc length.
#[derive(Clone, Debug)]
pub struct Guide {
    pub pts: Vec<P>,
    pub s: Vec<f64>,
    pub length: f64,
    /// Points bucketed per cell for fast nearest queries.
    buckets: HashMap<(i32, i32), Vec<u32>>,
    /// Per cell within reach of the course: (distance to course, progress along it).
    pub cells: HashMap<(i32, i32), (f64, f64)>,
    /// Coarse version for display.
    pub outline: Vec<P>,
    /// The course as drawn (no exaggeration or extra smoothing), in the same grid coordinates.
    /// Used to judge how faithful a finished track is.
    pub reference: Vec<P>,
    /// Per guide point: 1.0 on straights, falling to 0.0 in corners. The solver's tolerance band
    /// is multiplied by this so it straightens straights without letting corners be cut.
    pub straightness: Vec<f64>,
}

fn dist(a: P, b: P) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Resample a closed polyline to (approximately) uniform spacing.
pub fn resample(pts: &[P], spacing: f64) -> Vec<P> {
    let n = pts.len();
    let mut out = Vec::new();
    let mut carry = 0.0;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let seg = dist(a, b);
        let mut t = carry;
        while t < seg {
            let u = t / seg;
            out.push((a.0 + (b.0 - a.0) * u, a.1 + (b.1 - a.1) * u));
            t += spacing;
        }
        carry = t - seg;
    }
    out
}

/// Circular gaussian smoothing of a closed, uniformly spaced polyline (sigma in samples).
pub fn smooth(pts: &[P], sigma: f64) -> Vec<P> {
    let n = pts.len() as i64;
    let r = (3.0 * sigma).ceil() as i64;
    if sigma <= 0.0 || n == 0 {
        return pts.to_vec();
    }
    let w: Vec<f64> = (-r..=r).map(|k| (-(k * k) as f64 / (2.0 * sigma * sigma)).exp()).collect();
    let ws: f64 = w.iter().sum();
    (0..n)
        .map(|i| {
            let (mut x, mut y) = (0.0, 0.0);
            for (j, k) in (-r..=r).enumerate() {
                let p = pts[((i + k).rem_euclid(n)) as usize];
                x += p.0 * w[j];
                y += p.1 * w[j];
            }
            (x / ws, y / ws)
        })
        .collect()
}

/// Rotate the loop so it starts nearest `start`, optionally reversing direction.
pub fn orient(pts: &[P], start: Option<P>, reverse: bool) -> Vec<P> {
    let mut v = pts.to_vec();
    if reverse {
        v.reverse();
    }
    if let Some(s) = start {
        let i = (0..v.len()).min_by(|&a, &b| dist(v[a], s).total_cmp(&dist(v[b], s))).unwrap_or(0);
        v.rotate_left(i);
    }
    v
}

impl Guide {
    pub fn build(loop_px: &[P], p: &FitParams) -> Guide {
        let raw = resample(loop_px, 1.0);
        let (minx, maxx, miny, maxy) = bbox(&raw);
        let span = (maxx - minx).max(maxy - miny).max(1.0);
        let base = smooth(&raw, (span * p.smoothing).max(1.5));
        let mut c = base.clone();
        if (p.exaggeration - 1.0).abs() > 1e-6 {
            let sm = smooth(&base, span * p.detail_scale);
            c = base.iter().zip(&sm).map(|(b, s)| (s.0 + p.exaggeration * (b.0 - s.0), s.1 + p.exaggeration * (b.1 - s.1))).collect();
        }
        let (minx, maxx, miny, maxy) = bbox(&c);
        let span = (maxx - minx).max(maxy - miny).max(1.0);
        let g = p.grid as f64;
        let sc = (g - 2.0 * p.margin) / span;
        let ox = (g - (maxx - minx) * sc) / 2.0;
        let oy = (g - (maxy - miny) * sc) / 2.0;
        let scaled: Vec<P> = c.iter().map(|q| ((q.0 - minx) * sc + ox, (q.1 - miny) * sc + oy)).collect();
        let pts = resample(&scaled, 0.05);
        let mut s = Vec::with_capacity(pts.len());
        let mut acc = 0.0;
        for i in 0..pts.len() {
            s.push(acc);
            acc += dist(pts[i], pts[(i + 1) % pts.len()]);
        }
        let mut buckets: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, q) in pts.iter().enumerate() {
            buckets.entry((q.0.floor() as i32, q.1.floor() as i32)).or_default().push(i as u32);
        }
        let outline = resample(&scaled, 0.25);
        let drawn = smooth(&raw, 1.5);
        let reference = resample(&drawn.iter().map(|q| ((q.0 - minx) * sc + ox, (q.1 - miny) * sc + oy)).collect::<Vec<_>>(), 0.1);
        let straightness = straightness(&pts, 0.05);
        let mut gd = Guide { pts, s, length: acc, buckets, cells: HashMap::new(), outline, reference, straightness };
        for cx in -1..=p.grid {
            for cy in -1..=p.grid {
                if let Some((d, i)) = gd.nearest((cx as f64 + 0.5, cy as f64 + 0.5), 3) {
                    if d <= 2.0 {
                        gd.cells.insert((cx, cy), (d, gd.s[i]));
                    }
                }
            }
        }
        gd
    }

    /// Nearest guide point within `reach` cells: (distance, index).
    pub fn nearest(&self, q: P, reach: i32) -> Option<(f64, usize)> {
        let (bx, by) = (q.0.floor() as i32, q.1.floor() as i32);
        let mut best: Option<(f64, usize)> = None;
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                if let Some(v) = self.buckets.get(&(bx + dx, by + dy)) {
                    for &i in v {
                        let d = dist(self.pts[i as usize], q);
                        if best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, i as usize));
                        }
                    }
                }
            }
        }
        best
    }

    pub fn start(&self) -> P {
        self.pts[0]
    }
}

fn bbox(v: &[P]) -> (f64, f64, f64, f64) {
    v.iter().fold((f64::MAX, f64::MIN, f64::MAX, f64::MIN), |(a, b, c, d), p| (a.min(p.0), b.max(p.0), c.min(p.1), d.max(p.1)))
}

/// Douglas-Peucker simplification of a closed loop: keeps the shape within `tol` pixels with far
/// fewer points (used to give the line editor a manageable number of handles).
pub fn simplify_closed(pts: &[P], tol: f64) -> Vec<P> {
    let n = pts.len();
    if n < 8 {
        return pts.to_vec();
    }
    // split the loop at point 0 and the point farthest from it
    let far = (0..n).max_by(|&a, &b| dist(pts[0], pts[a]).total_cmp(&dist(pts[0], pts[b]))).unwrap();
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[far] = true;
    fn rdp(pts: &[P], idx: &[usize], tol: f64, keep: &mut [bool]) {
        if idx.len() < 3 {
            return;
        }
        let (a, b) = (pts[idx[0]], pts[*idx.last().unwrap()]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        let mut best = (0.0, 0);
        for (k, &i) in idx.iter().enumerate().skip(1).take(idx.len() - 2) {
            let p = pts[i];
            let d = if len < 1e-9 { dist(p, a) } else { ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len };
            if d > best.0 {
                best = (d, k);
            }
        }
        if best.0 > tol {
            keep[idx[best.1]] = true;
            rdp(pts, &idx[..=best.1], tol, keep);
            rdp(pts, &idx[best.1..], tol, keep);
        }
    }
    let first: Vec<usize> = (0..=far).collect();
    let second: Vec<usize> = (far..n).chain(std::iter::once(0)).collect();
    rdp(pts, &first, tol, &mut keep);
    rdp(pts, &second, tol, &mut keep);
    (0..n).filter(|&i| keep[i]).map(|i| pts[i]).collect()
}

/// Smooth closed curve through sparse points (centripetal Catmull-Rom), sampled about every
/// `step` pixels. Hand-drawn or edited lines go through this so clicks become curves, not corners.
pub fn catmull_rom_closed(pts: &[P], step: f64) -> Vec<P> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let mut out = Vec::new();
    for i in 0..n {
        let p0 = pts[(i + n - 1) % n];
        let p1 = pts[i];
        let p2 = pts[(i + 1) % n];
        let p3 = pts[(i + 2) % n];
        let t01 = dist(p0, p1).sqrt().max(1e-6);
        let t12 = dist(p1, p2).sqrt().max(1e-6);
        let t23 = dist(p2, p3).sqrt().max(1e-6);
        // tangents (Barry-Goldman form simplified for centripetal parameterisation)
        let m1 = (
            (p1.0 - p0.0) / t01 - (p2.0 - p0.0) / (t01 + t12) + (p2.0 - p1.0) / t12,
            (p1.1 - p0.1) / t01 - (p2.1 - p0.1) / (t01 + t12) + (p2.1 - p1.1) / t12,
        );
        let m2 = (
            (p2.0 - p1.0) / t12 - (p3.0 - p1.0) / (t12 + t23) + (p3.0 - p2.0) / t23,
            (p2.1 - p1.1) / t12 - (p3.1 - p1.1) / (t12 + t23) + (p3.1 - p2.1) / t23,
        );
        let (m1, m2) = ((m1.0 * t12, m1.1 * t12), (m2.0 * t12, m2.1 * t12));
        let segs = (dist(p1, p2) / step).ceil().max(1.0) as usize;
        for s in 0..segs {
            let t = s as f64 / segs as f64;
            let (t2, t3) = (t * t, t * t * t);
            let (h00, h10, h01, h11) = (2.0 * t3 - 3.0 * t2 + 1.0, t3 - 2.0 * t2 + t, -2.0 * t3 + 3.0 * t2, t3 - t2);
            out.push((
                h00 * p1.0 + h10 * m1.0 + h01 * p2.0 + h11 * m2.0,
                h00 * p1.1 + h10 * m1.1 + h01 * p2.1 + h11 * m2.1,
            ));
        }
    }
    out
}

/// How straight the course is around each point (1 = straight, 0 = a corner turning at
/// least ~35 degrees per tile), from the heading change over a +/-0.5 tile window.
fn straightness(pts: &[P], spacing: f64) -> Vec<f64> {
    let n = pts.len();
    if n < 4 {
        return vec![1.0; n];
    }
    let w = ((0.5 / spacing).round() as usize).clamp(1, n / 4);
    let raw: Vec<f64> = (0..n)
        .map(|i| {
            let (a, b, c) = (pts[(i + n - w) % n], pts[i], pts[(i + w) % n]);
            let (h1, h2) = ((b.1 - a.1).atan2(b.0 - a.0), (c.1 - b.1).atan2(c.0 - b.0));
            let mut d = (h2 - h1).abs();
            if d > std::f64::consts::PI {
                d = 2.0 * std::f64::consts::PI - d;
            }
            let rate = d / (2.0 * w as f64 * spacing); // radians per tile
            (1.0 - rate / 0.6).clamp(0.0, 1.0)
        })
        .collect();
    // take the minimum over a +/-1 tile window so the band shrinks on the approach to a corner too
    let r = (1.0 / spacing) as usize;
    (0..n)
        .map(|i| (0..=2 * r).map(|k| raw[(i + n + k - r) % n]).fold(1.0, f64::min))
        .collect()
}
