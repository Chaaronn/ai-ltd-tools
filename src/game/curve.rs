//! The racing line exactly as the game builds it (`TrackUtils.GetPath` / `GetPathCurve`),
//! plus the lap score (`GetTrackLapScore` -> `PixelsToScore`).

use super::tiles::{kind, Block, Conn};
use super::track::LapStep;

pub const TILE_PX: f64 = 768.0;
/// `PixelsToScore`: pixels * 0.013020833 (= 1 / 76.8).
pub const PX_TO_SCORE: f64 = 0.013020833333333334;

pub type P = (f64, f64);

fn angle_for(kind_: u8) -> Option<f64> {
    match kind_ {
        kind::DEG20 => Some((128f64).atan2(384.0)),
        kind::DEG45 => Some(std::f64::consts::FRAC_PI_4),
        kind::DEG70 => Some((384f64).atan2(128.0)),
        _ => None,
    }
}

/// `SmoothnessByTypePair`: 0.2 when leaving a normal/double/any connection towards an angled one,
/// otherwise 0.5.
fn smoothness(a: u8, b: u8) -> f64 {
    let from_plain = matches!(a, kind::NORMAL | kind::DOUBLE | kind::ANY);
    let to_angled = matches!(b, kind::DEG20 | kind::DEG45 | kind::DEG70);
    if from_plain && to_angled { 0.2 } else { 0.5 }
}

/// `ComputeTangentParams`: direction (normal, rotated for angled connection types) and smoothness.
fn tangent(normal: P, delta: P, a: u8, b: u8) -> (P, f64) {
    let s = smoothness(a, b);
    match angle_for(a) {
        None => (normal, s),
        Some(ang) => {
            let cross = normal.0 * delta.1 - normal.1 * delta.0;
            let ang = ang * cross.signum();
            let (sn, cs) = ang.sin_cos();
            ((normal.0 * cs - normal.1 * sn, normal.0 * sn + normal.1 * cs), s)
        }
    }
}

fn add(a: P, b: P) -> P { (a.0 + b.0, a.1 + b.1) }
fn sub(a: P, b: P) -> P { (a.0 - b.0, a.1 - b.1) }
fn mul(a: P, k: f64) -> P { (a.0 * k, a.1 * k) }
fn dist(a: P, b: P) -> f64 { ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() }

pub fn bezier(seg: &[P; 4], t: f64) -> P {
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    (
        w[0] * seg[0].0 + w[1] * seg[1].0 + w[2] * seg[2].0 + w[3] * seg[3].0,
        w[0] * seg[0].1 + w[1] * seg[1].1 + w[2] * seg[2].1 + w[3] * seg[3].1,
    )
}

/// The curve inside one tile, in tile units, from `from` to the other connection.
/// Only depends on the tile itself because the game's handles use the distance between
/// consecutive path points, which are this tile's two connections.
pub fn tile_bezier(block: &Block, from: Conn) -> [P; 4] {
    let to = if block.conns[0] == from { block.conns[1] } else { block.conns[0] };
    let (pa, pb) = (from.pos(), to.pos());
    let d = dist(pa, pb);
    let (ta, sa) = tangent(from.inward(), sub(pb, pa), from.kind, to.kind);
    let (tb, sb) = tangent(to.inward(), sub(pa, pb), to.kind, from.kind);
    [pa, add(pa, mul(ta, d * sa)), add(pb, mul(tb, d * sb)), pb]
}

/// Whole-lap curve in tile units (one cubic per tile).
pub fn lap_curve(lap: &[LapStep]) -> Vec<[P; 4]> {
    lap.iter()
        .map(|s| {
            let seg = tile_bezier(s.block, s.entry);
            let o = (s.cell.0 as f64, s.cell.1 as f64);
            [add(seg[0], o), add(seg[1], o), add(seg[2], o), add(seg[3], o)]
        })
        .collect()
}

pub fn lap_length_px(lap: &[LapStep]) -> f64 {
    let mut len = 0.0;
    for seg in lap_curve(lap) {
        let mut prev = seg[0];
        for i in 1..=40 {
            let p = bezier(&seg, i as f64 / 40.0);
            len += dist(prev, p);
            prev = p;
        }
    }
    len * TILE_PX
}

pub fn lap_score(lap: &[LapStep]) -> f64 {
    lap_length_px(lap) * PX_TO_SCORE
}

/// Total absolute heading change round the lap, in full turns. A simple loop with no wiggles
/// is 1.0; every zig-zag adds to it. Handy for judging how smooth a track is.
pub fn total_turning(lap: &[LapStep]) -> f64 {
    let mut pts = Vec::new();
    for seg in lap_curve(lap) {
        for i in 0..10 {
            pts.push(bezier(&seg, i as f64 / 10.0));
        }
    }
    let n = pts.len();
    let heading = |i: usize| {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        (b.1 - a.1).atan2(b.0 - a.0)
    };
    let mut total = 0.0;
    for i in 0..n {
        let mut d = heading((i + 1) % n) - heading(i);
        while d > std::f64::consts::PI { d -= 2.0 * std::f64::consts::PI; }
        while d < -std::f64::consts::PI { d += 2.0 * std::f64::consts::PI; }
        total += d.abs();
    }
    total / (2.0 * std::f64::consts::PI)
}
