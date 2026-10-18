//! Splitting a network's history into runs of unchanged settings, and the per-run measures:
//! stage, gains, stability, stalls.

use std::collections::{HashMap, HashSet};

use super::format::{Arch, Gen, Reward, Save, Settings};

/// A save trains on "random tracks" when its track changes almost every generation.
pub const RANDOM_TRACK: &str = "random";
/// Stall: this many generations without the median score per second beating its best by
/// [`STALL_GAIN`] (and no faster lap).
pub const STALL_GENS: usize = 50;
pub const STALL_GAIN: f64 = 0.01;
/// A best lap this much faster (0.1%) also counts as a gain.
pub const STALL_LAP_GAIN: f64 = 0.001;

#[derive(Clone, Debug)]
pub struct Run {
    /// First generation (index into the save's generations).
    pub start: usize,
    /// One past the last generation.
    pub end: usize,
    pub settings: Settings,
    pub rewards: Vec<Reward>,
    pub track: String,
    /// How trained the network was at the start, from generations trained (see [`stage_of`]).
    pub stage: Option<f64>,
    pub start_median: Option<f64>,
    pub end_median: Option<f64>,
    /// % change of the median per 10 generations.
    pub score_gain: Option<f64>,
    /// % lap-time reduction per 10 generations.
    pub lap_gain: Option<f64>,
    /// Change in median survival (share of cars still driving), percentage points per 10 gens.
    pub survival_gain: Option<f64>,
    /// % change in median score per simulated second per 10 gens.
    pub rate_gain: Option<f64>,
    /// Median score change per simulated minute.
    pub gain_per_min: Option<f64>,
    /// Standard deviation of the median within the run, relative to its mean.
    pub stability: Option<f64>,
    pub best_lap: Option<f64>,
    pub sim_seconds: f64,
    /// Network design during the run.
    pub arch: Arch,
}

impl Run {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
    pub fn stage_label(&self) -> &'static str {
        stage_label(self.stage)
    }
}

/// Stage boundaries in generations trained: early below [`MID_GENS`], mid below [`LATE_GENS`],
/// late below [`VETERAN_GENS`], veteran after. Generations are the one measure that doesn't jump
/// when the rewards, the time limit or the track change.
pub const MID_GENS: usize = 200;
pub const LATE_GENS: usize = 1000;
/// Veteran from this many generations.
pub const VETERAN_GENS: usize = 5000;

/// Stage as a 0..1 fraction from generations trained: each stage is a quarter (early 0–0.25
/// for 0–200 gens, mid to 0.5 at 1,000, late to 0.75 at 5,000, veteran beyond, reaching 1 at
/// 20,000), so labels and comparisons line up.
pub fn stage_of(gens: usize) -> f64 {
    let g = gens as f64;
    let seg = |lo: f64, hi: f64, base: f64| base + 0.25 * ((g - lo) / (hi - lo)).clamp(0.0, 1.0);
    if gens < MID_GENS {
        seg(0.0, MID_GENS as f64, 0.0)
    } else if gens < LATE_GENS {
        seg(MID_GENS as f64, LATE_GENS as f64, 0.25)
    } else if gens < VETERAN_GENS {
        seg(LATE_GENS as f64, VETERAN_GENS as f64, 0.5)
    } else {
        seg(VETERAN_GENS as f64, 20_000.0, 0.75)
    }
}

pub fn stage_label(s: Option<f64>) -> &'static str {
    match s {
        None => "?",
        Some(s) if s < 0.25 => "early",
        Some(s) if s < 0.5 => "mid",
        Some(s) if s < 0.75 => "late",
        Some(_) => "veteran",
    }
}

/// A stretch with no meaningful gain. `end` is exclusive; `ongoing` if it runs to the last generation.
#[derive(Clone, Debug)]
pub struct Stall {
    pub start: usize,
    pub end: usize,
    pub ongoing: bool,
}

pub fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

pub fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let p = q * (sorted.len() - 1) as f64;
    let (i, t) = (p.floor() as usize, p.fract());
    if i + 1 < sorted.len() { sorted[i] * (1.0 - t) + sorted[i + 1] * t } else { sorted[i] }
}

/// The track key for each generation, with random-track training folded into one key.
pub fn track_keys(gens: &[Gen]) -> Vec<String> {
    let distinct: HashSet<&str> = gens.iter().filter_map(|g| g.track.as_deref()).collect();
    let random = gens.len() >= 10 && distinct.len() as f64 > 0.2 * gens.len() as f64;
    gens.iter()
        .map(|g| if random { RANDOM_TRACK.to_string() } else { g.track.clone().unwrap_or_default() })
        .collect()
}

/// Settings for each generation, with missing (old, null) settings carried over from the
/// previous generation so they don't split runs.
fn effective_settings(gens: &[Gen]) -> Vec<Settings> {
    let mut out: Vec<Settings> = Vec::with_capacity(gens.len());
    for g in gens {
        let mut s = if g.settings.mutation.is_none() { out.last().cloned().unwrap_or_else(|| g.settings.clone()) } else { g.settings.clone() };
        // the time limit is only visible when some cars survive to it; otherwise keep the last one
        if s.time_limit.is_none() {
            s.time_limit = out.last().and_then(|p| p.time_limit).or(g.settings.time_limit);
        }
        out.push(s);
    }
    out
}

/// Settings equal for run purposes. The time limit is left out: players raise it step by step as
/// a network learns more of the track (a curriculum, not an experiment), and score per second
/// already cancels its effect.
pub fn same_settings(a: &Settings, b: &Settings) -> bool {
    Settings { time_limit: None, ..a.clone() } == Settings { time_limit: None, ..b.clone() }
}

pub fn same_key(a: &Gen, ta: &str, b: &Gen, tb: &str) -> bool {
    ta == tb && a.rewards == b.rewards
}

/// Split into runs and measure each.
pub fn segment(save: &Save) -> Vec<Run> {
    let g = &save.gens;
    if g.is_empty() {
        return Vec::new();
    }
    let tracks = track_keys(g);
    let settings = effective_settings(g);
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    let mut a = 0;
    for i in 1..=g.len() {
        if i == g.len() || !same_settings(&settings[i], &settings[a]) || g[i].rewards != g[a].rewards || tracks[i] != tracks[a] || save.arch_at(i) != save.arch_at(a) {
            bounds.push((a, i));
            a = i;
        }
    }
    bounds
        .into_iter()
        .map(|(a, b)| {
            let meds: Vec<f64> = g[a..b].iter().filter_map(|x| x.median).collect();
            let head = median(&mut meds.iter().take(5).copied().collect::<Vec<_>>());
            let tail = median(&mut meds.iter().rev().take(5).copied().collect::<Vec<_>>());
            let len = (b - a) as f64;
            let score_gain = match (head, tail) {
                (Some(h), Some(t)) if b - a >= 2 && h.abs() > 1e-9 => Some((t - h) / h.abs() * 100.0 / len * 10.0),
                _ => None,
            };
            let laps: Vec<f64> = g[a..b].iter().filter_map(|x| x.best_lap).collect();
            let lap_gain = match (laps.first(), laps.last()) {
                (Some(&f), Some(&l)) if laps.len() >= 2 => Some((f - l) / f * 100.0 / len * 10.0),
                _ => None,
            };
            let ends = |f: &dyn Fn(&Gen) -> Option<f64>| -> Option<(f64, f64)> {
                let v: Vec<f64> = g[a..b].iter().filter_map(f).collect();
                if v.len() < 2 {
                    return None;
                }
                Some((median(&mut v.iter().take(5).copied().collect::<Vec<_>>())?, median(&mut v.iter().rev().take(5).copied().collect::<Vec<_>>())?))
            };
            let survival_gain = ends(&|x: &Gen| x.survival()).map(|(h, t)| (t - h) * 100.0 / len * 10.0);
            let rate_gain = ends(&|x: &Gen| x.rate()).and_then(|(h, t)| (h.abs() > 1e-9).then(|| (t - h) / h.abs() * 100.0 / len * 10.0));
            let sim: f64 = g[a..b].iter().filter_map(|x| x.time).sum();
            let gain_per_min = match (head, tail) {
                (Some(h), Some(t)) if sim > 0.0 && b - a >= 2 => Some((t - h) / (sim / 60.0)),
                _ => None,
            };
            let stability = if meds.len() >= 3 {
                let mean = meds.iter().sum::<f64>() / meds.len() as f64;
                let var = meds.iter().map(|m| (m - mean).powi(2)).sum::<f64>() / (meds.len() - 1) as f64;
                (mean.abs() > 1e-9).then(|| var.sqrt() / mean.abs())
            } else {
                None
            };
            // generations trained before this run (inherited history included)
            let stage = Some(stage_of(a));
            Run {
                start: a,
                end: b,
                settings: settings[a].clone(),
                rewards: g[a].rewards.clone(),
                track: tracks[a].clone(),
                stage,
                start_median: head,
                end_median: tail,
                score_gain,
                lap_gain,
                survival_gain,
                rate_gain,
                gain_per_min,
                stability,
                best_lap: laps.iter().copied().reduce(f64::min),
                sim_seconds: sim,
                arch: save.arch_at(a).clone(),
            }
        })
        .collect()
}

/// Stretches of [`STALL_GENS`]+ generations where neither the median beat its best (for that
/// track and rewards) by [`STALL_GAIN`] nor the best lap on that track improved by
/// [`STALL_LAP_GAIN`].
pub fn stalls(save: &Save) -> (Vec<Stall>, usize) {
    let g = &save.gens;
    let tracks = track_keys(g);
    let mut best: HashMap<(String, String), f64> = HashMap::new();
    let mut best_lap: HashMap<String, f64> = HashMap::new();
    let limits = super::verdict::time_limits(g);
    let mut since_start = 0usize;
    let mut since = 0usize;
    let mut out = Vec::new();
    for (i, x) in g.iter().enumerate() {
        let lap_improved = match x.best_lap.filter(|_| tracks[i] != RANDOM_TRACK) {
            Some(l) => {
                let e = best_lap.entry(tracks[i].clone()).or_insert(f64::INFINITY);
                // compare with the last lap that counted as a gain, so a run of tiny
                // improvements adds up instead of each one falling under the threshold
                let better = l < *e * (1.0 - STALL_LAP_GAIN);
                if better {
                    *e = l;
                }
                better
            }
            None => false,
        };
        // score per second: a longer time limit alone isn't a gain
        let Some(m) = x.rate() else {
            since += 1;
            continue;
        };
        // a new time limit starts afresh: score per second shifts when generations get longer
        let k = (tracks[i].clone(), format!("{:?}{:?}", x.rewards, limits[i]));
        let improved = lap_improved
            || match best.get(&k) {
                None => true,
                Some(&b) => m > b + STALL_GAIN * b.abs().max(1e-6),
            };
        if improved {
            if since >= STALL_GENS {
                out.push(Stall { start: since_start, end: i, ongoing: false });
            }
            let e = best.entry(k).or_insert(m);
            *e = e.max(m);
            since = 0;
            since_start = i + 1;
        } else {
            since += 1;
        }
    }
    if since >= STALL_GENS {
        out.push(Stall { start: since_start, end: g.len(), ongoing: true });
    }
    (out, since)
}

/// Generations where the best lap first appears or improves.
pub fn lap_milestones(save: &Save) -> Vec<(usize, f64)> {
    let mut best = f64::INFINITY;
    let mut out = Vec::new();
    for (i, g) in save.gens.iter().enumerate() {
        if let Some(l) = g.best_lap {
            if l < best - 1e-9 {
                best = l;
                out.push((i, l));
            }
        }
    }
    out
}

/// Mutation-rate buckets used in the comparison grid.
pub const BUCKETS: [(&str, f64); 6] =
    [("≤ 0.02", 0.025), ("0.03–0.07", 0.075), ("0.08–0.12", 0.125), ("0.13–0.20", 0.205), ("0.21–0.30", 0.305), ("> 0.30", f64::INFINITY)];

pub fn bucket(m: Option<f64>) -> Option<usize> {
    let m = m?;
    BUCKETS.iter().position(|&(_, hi)| m < hi)
}
