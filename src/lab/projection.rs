//! Lap-time projection: fit the best-lap curve of a network on its track and estimate where it
//! levels off and how many generations a given time would take.
//!
//! Series: the current lap pace (best lap over the trailing 5 generations) on the track, from the
//! slowest point of the latest 200 generations onwards. Model: `y(x) = a + b·e^(−x/τ)`; for each
//! τ on a log grid, `a` and `b` come from least squares and the τ with the smallest error wins.
//! The main fit uses the latest 60% of the series; `a` is the plateau, and fits over the whole
//! series and the last 60 generations give its range. When the main fit is effectively a
//! straight line, or puts the plateau above the current best, there's no plateau in sight and
//! the recent straight-line rate is used instead.
//!
//! Replayed over the player's history, that curve on its own did worse than assuming no change
//! (and its "levelled off" call was usually wrong), so the app shows a [`Forecast`] instead: half
//! the typical improvement other networks made over the same number of generations, half this
//! network's curve shrunk to 20% of its predicted gain. See `.docs/evolution-lab.md`.

use super::keeper;
use super::library::{Library, Network};
use super::runs;

/// Default window: the latest this-many generations on the track.
pub const WINDOW: usize = 200;
/// Fewer distinct lap improvements than this and there's nothing to fit.
pub const MIN_IMPROVEMENTS: usize = 4;
/// No faster lap for this many generations counts as already levelled off.
pub const FLAT_GENS: usize = 40;

#[derive(Clone, Debug)]
pub struct Fit {
    pub a: f64,
    pub b: f64,
    pub tau: f64,
    /// Generation that x = 0 refers to.
    pub origin: usize,
    /// The fit's τ hit the top of the grid: the curve is still close to a straight line.
    pub linear: bool,
}

impl Fit {
    pub fn predict(&self, at: f64) -> f64 {
        self.a + self.b * (-(at - self.origin as f64) / self.tau).exp()
    }
}

#[derive(Clone, Debug)]
pub struct Projection {
    /// Generations used: [from, to).
    pub from: usize,
    pub to: usize,
    pub best: f64,
    /// Generation where the current pace first reached `best`.
    pub best_at: usize,
    pub fit: Fit,
    /// Plateau estimate and its range across windows; None when no plateau is in sight.
    pub plateau: Option<(f64, f64, f64)>,
    /// Recent straight-line rate: seconds per 100 generations (negative = getting faster).
    pub per_100: f64,
}

impl Projection {
    /// Generations from now until the best lap reaches `target`, if it's expected to.
    pub fn gens_to(&self, target: f64) -> Option<f64> {
        if target >= self.best {
            return Some(0.0);
        }
        let now = self.to as f64;
        if !self.fit.linear && self.fit.b <= 0.0 {
            return None; // levelled off
        }
        if !self.fit.linear && self.fit.b > 0.0 {
            let (lo, _, _) = self.plateau?;
            // don't promise anything at or beyond the most optimistic plateau
            if target <= lo.min(self.fit.a) {
                return None;
            }
            let x = -self.fit.tau * ((target - self.fit.a) / self.fit.b).ln();
            let g = x + self.fit.origin as f64 - now;
            return (g.is_finite() && g >= 0.0).then_some(g);
        }
        // linear: only extrapolate a little way
        if self.per_100 >= 0.0 {
            return None;
        }
        let g = (self.best - target) / -self.per_100 * 100.0;
        (g <= 1000.0).then_some(g)
    }
}

fn fit_exp(xs: &[f64], ys: &[f64], origin: usize) -> Option<Fit> {
    let n = xs.len() as f64;
    let span = xs.last()? - xs.first()?;
    if span <= 0.0 {
        return None;
    }
    let taus: Vec<f64> = (0..60).map(|i| 2.0 * (span * 20.0 / 2.0f64).powf(i as f64 / 59.0)).collect();
    let mut best: Option<(f64, Fit)> = None;
    for (k, &tau) in taus.iter().enumerate() {
        let z: Vec<f64> = xs.iter().map(|x| (-x / tau).exp()).collect();
        let (mz, my) = (z.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
        let szz: f64 = z.iter().map(|v| (v - mz).powi(2)).sum();
        if szz < 1e-15 {
            continue;
        }
        let b = z.iter().zip(ys).map(|(zi, yi)| (zi - mz) * (yi - my)).sum::<f64>() / szz;
        let a = my - b * mz;
        if b < 0.0 {
            continue;
        }
        let sse: f64 = z.iter().zip(ys).map(|(zi, yi)| (a + b * zi - yi).powi(2)).sum();
        if best.as_ref().is_none_or(|(s, _)| sse < *s) {
            best = Some((sse, Fit { a, b, tau, origin, linear: k == taus.len() - 1 }));
        }
    }
    best.map(|b| b.1)
}

/// The network's current lap pace on `track` over the latest `window` generations:
/// (gen, best lap over the trailing 5 generations). A trailing minimum follows the current
/// network (including any regression after a reward or settings change), where an all-time
/// best would sit flat.
fn series(n: &Network, track: &str, window: usize) -> Vec<(usize, f64)> {
    let g = &n.save.gens;
    let keys = runs::track_keys(g);
    let on: Vec<usize> = (0..g.len()).filter(|&i| keys[i] == track).collect();
    let start = on.len().saturating_sub(window);
    let mut out = Vec::new();
    for k in start..on.len() {
        let lo = k.saturating_sub(4);
        let m = on[lo..=k].iter().filter_map(|&i| g[i].best_lap).fold(f64::INFINITY, f64::min);
        if m.is_finite() {
            out.push((on[k], m));
        }
    }
    // fit from the slowest point onwards, so the curve is the current improvement phase
    if let Some(peak) = out.iter().enumerate().max_by(|a, b| a.1 .1.total_cmp(&b.1 .1)).map(|(i, _)| i) {
        if out.len() - peak >= 30 {
            out.drain(..peak);
        }
    }
    out
}

/// Projection for the network's latest track, or None without enough lap history.
pub fn project(n: &Network) -> Option<Projection> {
    let run = n.runs.last()?;
    if run.track == runs::RANDOM_TRACK {
        return None;
    }
    let s = series(n, &run.track, WINDOW);
    let improvements = s.windows(2).filter(|p| p[1].1 < p[0].1 - 1e-9).count();
    if s.len() < 15 || improvements < MIN_IMPROVEMENTS {
        return None;
    }
    let fit_on = |part: &[(usize, f64)]| -> Option<Fit> {
        if part.len() < 15 {
            return None;
        }
        let origin = part[0].0;
        let xs: Vec<f64> = part.iter().map(|p| (p.0 - origin) as f64).collect();
        let ys: Vec<f64> = part.iter().map(|p| p.1).collect();
        fit_exp(&xs, &ys, origin)
    };
    // the main fit leans on the recent part (the early drop after a regression is steep and
    // would otherwise decide the plateau); the whole phase and the last 60 gens give the range
    let recent = &s[s.len() * 2 / 5..];
    let fit = fit_on(if recent.len() >= 15 { recent } else { &s })?;
    let (from, to) = (fit.origin, s.last()?.0 + 1);
    let best = s.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let tail: Vec<&(usize, f64)> = s.iter().rev().take(50).collect();
    let per_100 = match (tail.last(), tail.first()) {
        (Some(a), Some(b)) if b.0 > a.0 => (b.1 - a.1) / (b.0 - a.0) as f64 * 100.0,
        _ => 0.0,
    };
    // no improvement for a while: it has already levelled off at these settings
    let best_at = s.iter().find(|p| p.1 <= best + 1e-9).map_or(0, |p| p.0);
    if to - best_at >= FLAT_GENS {
        let flat = Fit { a: best, b: 0.0, tau: 1.0, origin: from, linear: false };
        return Some(Projection { from, to, best, best_at, fit: flat, plateau: Some((best, best, best)), per_100 });
    }
    // a plateau only counts if the main fit bends and doesn't claim we're already past it
    let plateau = if fit.linear || fit.a > best + 1e-6 {
        None
    } else {
        let mut ests = vec![fit.a];
        for f in [fit_on(&s), fit_on(&s[s.len().saturating_sub(60)..])].into_iter().flatten() {
            if !f.linear && f.a <= best + 1e-6 {
                ests.push(f.a);
            }
        }
        let lo = ests.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = ests.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Some((lo, fit.a, hi))
    };
    let mut fit = fit;
    if plateau.is_none() {
        fit.linear = true;
    }
    Some(Projection { from, to, best, best_at, fit, plateau, per_100 })
}

// ------------------------------------------------------------------ forecast

/// Horizons (generations ahead) the pooled improvement is measured at.
pub const HORIZONS: [usize; 6] = [0, 25, 50, 100, 150, 200];
/// Weight of the pooled improvement in the forecast (the rest is this network's shrunk curve).
pub const DRIFT_WEIGHT: f64 = 0.5;
/// Share of the curve's predicted gain kept (the raw curve over-promises).
pub const CURVE_SHRINK: f64 = 0.2;
/// The forecast reaches this far ahead.
pub const MAX_AHEAD: usize = 200;

/// Typical relative improvement of the best-so-far lap over each horizon, pooled across
/// networks: (horizon, median fraction, points).
#[derive(Clone, Debug, Default)]
pub struct Drift {
    pub points: Vec<(usize, f64, usize)>,
}

impl Drift {
    /// Fraction improved after `h` generations (interpolated; flat beyond the last horizon).
    pub fn at(&self, h: f64) -> f64 {
        let p = &self.points;
        if p.is_empty() {
            return 0.0;
        }
        for w in p.windows(2) {
            let (a, b) = (w[0], w[1]);
            if h <= b.0 as f64 {
                let t = ((h - a.0 as f64) / (b.0 - a.0).max(1) as f64).clamp(0.0, 1.0);
                return a.1 + (b.1 - a.1) * t;
            }
        }
        p.last().unwrap().1
    }
}

/// Best lap so far at each generation on `track` (None until the first lap there).
fn best_so_far(n: &Network, keys: &[String], track: &str) -> Vec<Option<f64>> {
    let mut cur: Option<f64> = None;
    n.save
        .gens
        .iter()
        .zip(keys)
        .map(|(g, k)| {
            if k == track {
                if let Some(l) = g.best_lap {
                    cur = Some(cur.map_or(l, |c: f64| c.min(l)));
                }
            }
            cur
        })
        .collect()
}

/// Sample points from networks' own history: (network, gen t, track, best at t−1, gens since that
/// best, best at t−1+h for each horizon where the track stays the same).
fn samples(lib: &Library) -> Vec<(String, usize, f64, usize, Vec<(usize, f64)>)> {
    let mut out = Vec::new();
    for n in lib.networks.iter().filter(|n| !keeper::is_keeper_copy(&n.file) && n.gens() > n.fork.own_from) {
        let g = &n.save.gens;
        if !g.iter().any(|x| x.best_lap.is_some()) {
            continue;
        }
        let keys = runs::track_keys(g);
        let mut cache: std::collections::HashMap<String, Vec<Option<f64>>> = std::collections::HashMap::new();
        let mut t = (n.fork.own_from + 40).max(60);
        while t < g.len() {
            let track = keys[t - 1].clone();
            if track == runs::RANDOM_TRACK || track.is_empty() {
                t += 10;
                continue;
            }
            let bsf = cache.entry(track.clone()).or_insert_with(|| best_so_far(n, &keys, &track));
            let Some(b) = bsf[t - 1] else {
                t += 10;
                continue;
            };
            let since = (0..t).rev().find(|&i| keys[i] == track && g[i].best_lap.is_some_and(|l| l <= b + 1e-9)).map_or(0, |i| t - 1 - i);
            let ahead: Vec<(usize, f64)> = HORIZONS
                .iter()
                .skip(1)
                .copied()
                .filter(|&h| t - 1 + h < g.len() && (t..t + h).all(|i| keys[i] == track))
                .filter_map(|h| bsf[t - 1 + h].map(|x| (h, x)))
                .collect();
            out.push((n.file.clone(), t, b, since, ahead));
            t += 10;
        }
    }
    out
}

/// Pooled improvement over each horizon, leaving out network `exclude` (the one being forecast).
pub fn pooled_drift(lib: &Library, exclude: Option<&str>) -> Drift {
    let s = samples(lib);
    let mut points = vec![(0usize, 0.0, 0usize)];
    for &h in HORIZONS.iter().skip(1) {
        let mut v: Vec<f64> = s
            .iter()
            .filter(|x| Some(x.0.as_str()) != exclude)
            .filter_map(|x| x.4.iter().find(|a| a.0 == h).map(|a| (x.2 - a.1) / x.2))
            .collect();
        let n = v.len();
        if let Some(m) = runs::median(&mut v) {
            points.push((h, m.max(0.0), n));
        }
    }
    Drift { points }
}

/// How often networks with no new best for a while went on to set one: buckets of generations
/// since the best, (label, faster by 0.5%+ within 100 gens, points).
pub fn faster_odds(lib: &Library, exclude: Option<&str>) -> Vec<(&'static str, usize, usize, std::ops::Range<usize>)> {
    let s = samples(lib);
    [("under 10", 0..10), ("10–39", 10..40), ("40 or more", 40..usize::MAX)]
        .into_iter()
        .map(|(label, r)| {
            let pts: Vec<f64> = s
                .iter()
                .filter(|x| Some(x.0.as_str()) != exclude && r.contains(&x.3))
                .filter_map(|x| x.4.iter().find(|a| a.0 == 100).map(|a| (x.2 - a.1) / x.2))
                .collect();
            (label, pts.iter().filter(|&&f| f >= 0.005).count(), pts.len(), r)
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct Forecast {
    /// Best lap so far, and the generation index the forecast starts from (the next generation).
    pub best: f64,
    pub from: usize,
    /// Generations since the best lap was set.
    pub since_best: usize,
    /// (generations ahead, forecast best lap), every 5 generations up to `MAX_AHEAD`.
    pub curve: Vec<(usize, f64)>,
    /// The curve fit the forecast leans on (still shown in the panel's small print).
    pub projection: Projection,
}

impl Forecast {
    /// Forecast best lap `h` generations ahead.
    pub fn at(&self, h: usize) -> f64 {
        let k = (h / 5).min(self.curve.len() - 1);
        self.curve[k].1
    }
    /// Generations until the forecast reaches `target`, if within `MAX_AHEAD`.
    pub fn gens_to(&self, target: f64) -> Option<usize> {
        if target >= self.best {
            return Some(0);
        }
        self.curve.iter().find(|p| p.1 <= target).map(|p| p.0)
    }
}

/// The forecast for `n`'s current track. `drift` should leave `n` out (see `pooled_drift`).
pub fn forecast(n: &Network, drift: &Drift) -> Option<Forecast> {
    let p = project(n)?;
    let g = &n.save.gens;
    let keys = runs::track_keys(g);
    let track = &n.runs.last()?.track;
    let best = g.iter().zip(&keys).filter(|(_, k)| *k == track).filter_map(|(x, _)| x.best_lap).reduce(f64::min)?;
    let since_best = (0..g.len()).rev().find(|&i| keys[i] == *track && g[i].best_lap.is_some_and(|l| l <= best + 1e-9)).map_or(0, |i| g.len() - 1 - i);
    let now = g.len();
    let curve_at = |h: usize| -> f64 {
        let at = (now - 1 + h) as f64;
        let raw = if p.fit.linear || p.plateau.is_none() { best + p.per_100.min(0.0) * h as f64 / 100.0 } else { p.fit.predict(at).min(best) };
        best + (raw.min(best) - best) * CURVE_SHRINK
    };
    let mut run = best;
    let curve = (0..=MAX_AHEAD / 5)
        .map(|k| {
            let h = k * 5;
            let v = DRIFT_WEIGHT * best * (1.0 - drift.at(h as f64)) + (1.0 - DRIFT_WEIGHT) * curve_at(h);
            run = run.min(v); // the best so far can't get worse
            (h, run)
        })
        .collect();
    Some(Forecast { best, from: now, since_best, curve, projection: p })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_an_exponential() {
        let xs: Vec<f64> = (0..200).map(|x| x as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|x| 44.0 + 2.0 * (-x / 50.0).exp()).collect();
        let f = fit_exp(&xs, &ys, 0).unwrap();
        assert!((f.a - 44.0).abs() < 0.02, "plateau {}", f.a);
        assert!((f.tau - 50.0).abs() / 50.0 < 0.1, "tau {}", f.tau);
        assert!(!f.linear);
    }

    #[test]
    fn straight_line_has_no_plateau() {
        let xs: Vec<f64> = (0..100).map(|x| x as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|x| 50.0 - 0.01 * x).collect();
        let f = fit_exp(&xs, &ys, 0).unwrap();
        assert!(f.linear || f.a < 45.0, "a straight line shouldn't look like it's levelling off: {f:?}");
    }
}
