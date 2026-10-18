//! Pick fit settings automatically for a given course.
//!
//! Each candidate setting is built for real and the finished track is scored against the
//! course *as drawn* (not the exaggerated guide):
//!
//! ```text
//! score = mean distance track -> course          (no detours)
//!       + mean distance course -> track          (no cut corners / lost features)
//!       + 0.8 x 95th-percentile course -> track  (no big local misses, e.g. cut corners)
//!       + 0.4 x worst course -> track            (no lost hairpins)
//!       + 0.6 x |turning - course turning| / course turning
//!                                                (zig-zags add turning, flattened corners remove it)
//! ```
//!
//! The search is coordinate descent: vary one setting over a few values (built in parallel),
//! keep the best, move to the next setting; two passes. About 25-30 builds in total.

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use crate::fit::P;
use crate::game::curve::{bezier, lap_curve, total_turning};
use crate::pipeline::{build_points, BuildParams, Built};

#[derive(Clone, Debug)]
pub struct Score {
    pub total: f64,
    pub track_to_course: f64,
    pub course_to_track: f64,
    pub course_to_track_p95: f64,
    pub course_to_track_max: f64,
    pub turning: f64,
    pub course_turning: f64,
}

pub struct Tuned {
    pub params: BuildParams,
    pub built: Built,
    pub score: Score,
    pub builds: usize,
}

struct Buckets {
    pts: Vec<P>,
    map: HashMap<(i32, i32), Vec<usize>>,
}

impl Buckets {
    fn new(pts: Vec<P>) -> Self {
        let mut map: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, p) in pts.iter().enumerate() {
            map.entry((p.0.floor() as i32, p.1.floor() as i32)).or_default().push(i);
        }
        Buckets { pts, map }
    }
    fn nearest(&self, q: P) -> f64 {
        let (bx, by) = (q.0.floor() as i32, q.1.floor() as i32);
        for reach in [1, 3, 8, 64] {
            let mut best = f64::MAX;
            for dx in -reach..=reach {
                for dy in -reach..=reach {
                    if let Some(v) = self.map.get(&(bx + dx, by + dy)) {
                        for &i in v {
                            let p = self.pts[i];
                            best = best.min(((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt());
                        }
                    }
                }
            }
            if best <= reach as f64 {
                return best;
            }
        }
        f64::MAX
    }
}

fn turning_of(pts: &[P]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let h = |i: usize| {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        (b.1 - a.1).atan2(b.0 - a.0)
    };
    let mut t = 0.0;
    for i in 0..n {
        let mut d = h((i + 1) % n) - h(i);
        while d > std::f64::consts::PI { d -= 2.0 * std::f64::consts::PI; }
        while d < -std::f64::consts::PI { d += 2.0 * std::f64::consts::PI; }
        t += d.abs();
    }
    t / (2.0 * std::f64::consts::PI)
}

/// How well a built track matches the course as drawn. Lower is better.
pub fn score(b: &Built) -> Option<Score> {
    let lap = b.solution.track.lap()?;
    let (ox, oy) = b.solution.offset;
    // track racing line, shifted back into the guide's coordinates
    let mut track = Vec::new();
    for seg in lap_curve(&lap) {
        for i in 0..8 {
            let p = bezier(&seg, i as f64 / 8.0);
            track.push((p.0 + ox as f64, p.1 + oy as f64));
        }
    }
    let course = &b.guide.reference;
    let tb = Buckets::new(track.clone());
    let cb = Buckets::new(course.clone());
    let t2c = track.iter().map(|&q| cb.nearest(q)).sum::<f64>() / track.len() as f64;
    let mut c2t: Vec<f64> = course.iter().map(|&q| tb.nearest(q)).collect();
    let c2t_mean = c2t.iter().sum::<f64>() / c2t.len() as f64;
    c2t.sort_by(|a, b| a.total_cmp(b));
    let p95 = c2t[(c2t.len() as f64 * 0.95) as usize % c2t.len()];
    // the course's own turning, measured at roughly tile resolution so pixel noise doesn't count
    let coarse = crate::fit::smooth(course, 4.0); // reference is sampled every 0.1 tiles -> sigma 0.4 tiles
    let course_turning = turning_of(&coarse);
    let turning = total_turning(&lap);
    let worst = *c2t.last().unwrap_or(&0.0);
    // turning should match the course: more = zig-zags, less = lost corners
    let turn_err = (turning - course_turning).abs() / course_turning.max(1.0);
    Some(Score {
        total: t2c + c2t_mean + 0.8 * p95 + 0.4 * worst + 0.6 * turn_err,
        track_to_course: t2c,
        course_to_track: c2t_mean,
        course_to_track_p95: p95,
        course_to_track_max: worst,
        turning,
        course_turning,
    })
}

type Setter = fn(&mut BuildParams, f64);

/// Search for the settings that give the closest, cleanest match. `progress(done, total)` is
/// called as builds finish (from worker threads).
pub fn auto_tune(line: &[P], base: &BuildParams, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Tuned> {
    let axes: [(&str, Setter, &[f64]); 4] = [
        ("exaggeration", |p, v| p.fit.exaggeration = v, &[0.8, 1.0, 1.3, 1.6, 2.0, 2.5]),
        ("smoothness", |p, v| p.solve.turn_penalty = v, &[0.3, 0.7, 1.0, 1.6, 2.5, 4.0]),
        ("tolerance", |p, v| p.solve.tolerance = v, &[0.05, 0.15, 0.25, 0.4]),
        ("fidelity", |p, v| p.solve.fidelity = v, &[25.0, 45.0, 70.0, 110.0]),
    ];
    let passes = 2;
    let total: usize = 1 + passes * axes.iter().map(|a| a.2.len()).sum::<usize>();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let eval = |p: &BuildParams| -> Option<(Built, Score)> {
        let r = build_points(line, p).ok().and_then(|b| score(&b).map(|s| (b, s)));
        progress(done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, total);
        r
    };
    let mut best_p = base.clone();
    let (mut best_b, mut best_s) = eval(&best_p).ok_or_else(|| anyhow!("couldn't build a track with the current settings"))?;
    let mut builds = 1;
    for _ in 0..passes {
        for (_, set, values) in &axes {
            let cands: Vec<BuildParams> = values
                .iter()
                .map(|&v| {
                    let mut p = best_p.clone();
                    set(&mut p, v);
                    p
                })
                .collect();
            let results: Vec<Option<(Built, Score)>> = std::thread::scope(|sc| {
                let hs: Vec<_> = cands.iter().map(|p| sc.spawn(|| eval(p))).collect();
                hs.into_iter().map(|h| h.join().ok().flatten()).collect()
            });
            builds += cands.len();
            for (p, r) in cands.into_iter().zip(results) {
                if let Some((b, s)) = r {
                    if s.total < best_s.total - 1e-9 {
                        best_p = p;
                        best_b = b;
                        best_s = s;
                    }
                }
            }
        }
    }
    Ok(Tuned { params: best_p, built: best_b, score: best_s, builds })
}
