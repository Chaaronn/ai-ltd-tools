//! The before/after test at every settings change, calibrated against fake ("placebo") change
//! points inside runs where nothing changed. See `.docs/evolution-lab-spec.md` §5 and §9.2.

use super::format::{Gen, Settings};
use super::runs::Run;
use super::compare::Measure;

/// Generations either side of a change (default window) and the minimum needed each side.
pub const WINDOW: usize = 20;
pub const MIN_SIDE: usize = 10;
/// Used until there are enough placebo points to calibrate.
pub const FALLBACK_THRESHOLD: f64 = 4.4;
pub const MIN_PLACEBO: usize = 50;

#[derive(Clone, Copy, Debug)]
pub struct Fit {
    pub slope: f64,
    /// Standard error, widened for lag-1 autocorrelation.
    pub se: f64,
    pub sd: f64,
    pub mean: f64,
}

/// Least-squares line through `y` against 0..n.
pub fn ols(y: &[f64]) -> Option<Fit> {
    let n = y.len();
    if n < 3 {
        return None;
    }
    let nf = n as f64;
    let mx = (nf - 1.0) / 2.0;
    let my = y.iter().sum::<f64>() / nf;
    let sxx: f64 = (0..n).map(|i| (i as f64 - mx).powi(2)).sum();
    let slope = y.iter().enumerate().map(|(i, v)| (i as f64 - mx) * (v - my)).sum::<f64>() / sxx;
    let res: Vec<f64> = y.iter().enumerate().map(|(i, v)| v - (my + slope * (i as f64 - mx))).collect();
    let ss: f64 = res.iter().map(|e| e * e).sum();
    let sd = (ss / (nf - 2.0).max(1.0)).sqrt();
    let r1 = if ss > 1e-12 { (0..n - 1).map(|i| res[i] * res[i + 1]).sum::<f64>() / ss } else { 0.0 };
    let r1 = r1.clamp(0.0, 0.95);
    let neff = (nf * (1.0 - r1) / (1.0 + r1)).max(3.0);
    let se = sd / sxx.sqrt() * (nf / neff).sqrt();
    Some(Fit { slope, se, sd, mean: my })
}

/// z-score of the change in slope from `before` to `after`.
pub fn slope_change(before: &[f64], after: &[f64]) -> Option<(Fit, Fit, f64)> {
    let b = ols(before)?;
    let a = ols(after)?;
    let z = (a.slope - b.slope) / (b.se * b.se + a.se * a.se + 1e-12).sqrt();
    Some((b, a, z))
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    /// Not enough generations: (before, after, needed each side)
    TooShort(usize, usize, usize),
    RewardsChanged,
    /// The generation time limit changed inside the before/after window.
    TimeLimitChanged,
    TrackChanged,
    MissingScores,
    MissingMeasure(Measure),
    WithinNoise,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Helped,
    Hurt,
    CantTell(Reason),
}

#[derive(Clone, Debug)]
pub struct Verdict {
    /// First generation with the new settings.
    pub at: usize,
    /// Index of the run that starts here.
    pub run: usize,
    pub change: String,
    /// Change type for pooling: "mutation ↓", "crossover", "several", ...
    pub kind: &'static str,
    pub outcome: Outcome,
    pub z: Option<f64>,
    pub before: Option<Fit>,
    pub after: Option<Fit>,
    pub n_before: usize,
    pub n_after: usize,
    pub stage: Option<f64>,
    /// Median best-lap change (seconds, negative = faster), when both sides have laps.
    pub lap_change: Option<f64>,
    /// Judged on lap time instead of score (reward changes: the rewards set the score's scale,
    /// but a lap time means the same whatever the rewards). z is then positive for faster laps.
    pub by_lap: bool,
}

impl Verdict {
    pub fn testable(&self) -> bool {
        self.z.is_some()
    }
    pub fn label(&self) -> String {
        match &self.outcome {
            Outcome::Helped => "clearly helped".into(),
            Outcome::Hurt => "clearly hurt".into(),
            Outcome::CantTell(r) => format!("can't tell: {}", reason_text(&r)),
        }
    }
}

pub fn reason_text(r: &Reason) -> String {
    match r {
        Reason::TooShort(b, a, need) => {
            if *b < *need && *a < *need {
                format!("only {b} gens before and {a} after (need {need} each)")
            } else if *b < *need {
                format!("only {b} gens before (need {need})")
            } else {
                format!("only {a} gens after (need {need})")
            }
        }
        Reason::RewardsChanged => "rewards changed, and there aren't laps on both sides to judge it by lap time (scores aren't comparable)".into(),
        Reason::TimeLimitChanged => "the time limit changed within the window, so scores aren't comparable".into(),
        Reason::TrackChanged => "track changed too".into(),
        Reason::MissingScores => "scores missing".into(),
        Reason::MissingMeasure(m) => format!("insufficient {} data", m.unit()),
        Reason::WithinNoise => "within noise".into(),
    }
}

/// Short description of what changed between two settings.
pub fn describe_change(a: &Settings, b: &Settings) -> (String, &'static str) {
    let mut parts = Vec::new();
    let mut kind = "other";
    let arrow = |x: Option<f64>, y: Option<f64>| if y.unwrap_or(0.0) < x.unwrap_or(0.0) { "↓" } else { "↑" };
    if a.mutation != b.mutation {
        parts.push(format!("mutation {} → {}", fmt_opt(a.mutation), fmt_opt(b.mutation)));
        kind = if b.mutation.unwrap_or(0.0) < a.mutation.unwrap_or(0.0) { "mutation ↓" } else { "mutation ↑" };
    }
    if a.selection != b.selection || a.selection_size != b.selection_size {
        parts.push(format!(
            "selection {}·{} → {}·{}",
            super::format::selection_name(a.selection),
            a.selection_size.map_or("?".into(), |x| x.to_string()),
            super::format::selection_name(b.selection),
            b.selection_size.map_or("?".into(), |x| x.to_string())
        ));
        kind = if parts.len() == 1 { "selection" } else { "several" };
    }
    if a.crossover != b.crossover {
        parts.push(format!("crossover {} → {}", super::format::crossover_name(a.crossover), super::format::crossover_name(b.crossover)));
        kind = if parts.len() == 1 { "crossover" } else { "several" };
    }
    if a.weight_decay != b.weight_decay {
        parts.push(format!("weight decay {} → {}", fmt_opt(a.weight_decay), fmt_opt(b.weight_decay)));
        kind = if parts.len() == 1 { "weight decay" } else { "several" };
    }
    if a.elimination != b.elimination {
        parts.push(format!("elimination {}", if b.elimination == Some(true) { "on" } else { "off" }));
        kind = if parts.len() == 1 { "elimination" } else { "several" };
    }
    if a.population != b.population {
        parts.push(format!(
            "population {} → {}",
            a.population.map_or("?".into(), |x| x.to_string()),
            b.population.map_or("?".into(), |x| x.to_string())
        ));
        kind = if parts.len() == 1 {
            if arrow(a.population.map(f64::from), b.population.map(f64::from)) == "↓" { "population ↓" } else { "population ↑" }
        } else {
            "several"
        };
    }
    (parts.join(", "), kind)
}

pub fn fmt_opt(v: Option<f64>) -> String {
    match v {
        None => "?".into(),
        Some(x) => {
            let s = format!("{x:.4}");
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        }
    }
}

pub(crate) fn medians(g: &[Gen]) -> Option<Vec<f64>> {
    g.iter().map(|x| x.median).collect()
}

/// The time limit in force at each generation (carried over generations where no car survived
/// to show it).
pub fn time_limits(g: &[Gen]) -> Vec<Option<f64>> {
    let mut cur = None;
    g.iter()
        .map(|x| {
            if x.settings.time_limit.is_some() {
                cur = x.settings.time_limit;
            }
            cur
        })
        .collect()
}

/// True if the time limit changes anywhere in `lo..hi`: scores before and after aren't on the
/// same footing (a longer generation scores more just by lasting longer).
pub(crate) fn limit_changes(limits: &[Option<f64>], lo: usize, hi: usize) -> bool {
    limits[lo..hi].windows(2).any(|w| w[0].is_some() && w[1].is_some() && w[0] != w[1])
}

/// Best lap per generation, gaps filled with the previous lap. None unless at least `min`
/// generations, and 80% of them, have a lap.
pub fn lap_series(g: &[Gen], min: usize) -> Option<Vec<f64>> {
    let have = g.iter().filter(|x| x.best_lap.is_some()).count();
    if have < min.max(3) || (have as f64) < 0.8 * g.len() as f64 {
        return None;
    }
    let mut prev = g.iter().find_map(|x| x.best_lap)?;
    Some(
        g.iter()
            .map(|x| {
                if let Some(l) = x.best_lap {
                    prev = l;
                }
                prev
            })
            .collect(),
    )
}

/// Slope change judged on lap time: z is positive when laps get faster more quickly after.
pub fn lap_slope_change(before: &[f64], after: &[f64]) -> Option<(Fit, Fit, f64)> {
    slope_change(before, after).map(|(b, a, z)| (b, a, -z))
}

fn lap_median(g: &[Gen]) -> Option<f64> {
    let mut v: Vec<f64> = g.iter().filter_map(|x| x.best_lap).collect();
    if v.len() < 5 {
        return None;
    }
    super::runs::median(&mut v)
}

/// A verdict for every change between consecutive runs.
/// `lap_threshold` is used for changes judged on lap time (reward changes).
pub fn verdicts(gens: &[Gen], runs: &[Run], threshold: f64, lap_threshold: f64, window: usize, min_side: usize) -> Vec<Verdict> {
    evaluate(gens, runs, threshold, lap_threshold, window, min_side, None)
}

/// Judge a planned experiment on its declared measure, without falling back to score.
pub fn verdicts_for_measure(gens: &[Gen], runs: &[Run], measure: Measure, threshold: f64, window: usize, min_side: usize) -> Vec<Verdict> {
    evaluate(gens, runs, threshold, threshold, window, min_side, Some(measure))
}

fn measure_series(gens: &[Gen], measure: Measure, min: usize) -> Option<Vec<f64>> {
    let values: Vec<f64> = match measure {
        Measure::Lap => lap_series(gens, min)?.into_iter().map(|v| -v).collect(),
        Measure::Score => medians(gens)?,
        Measure::Rate => gens.iter().map(Gen::rate).collect::<Option<Vec<_>>>()?,
        Measure::Survival => gens.iter().map(Gen::survival).collect::<Option<Vec<_>>>()?,
    };
    values.iter().all(|v| v.is_finite()).then_some(values)
}

fn evaluate(gens: &[Gen], runs: &[Run], threshold: f64, lap_threshold: f64, window: usize, min_side: usize, measure: Option<Measure>) -> Vec<Verdict> {
    let limits = time_limits(gens);
    let mut out = Vec::new();
    for j in 1..runs.len() {
        let (a, b) = (&runs[j - 1], &runs[j]);
        let (mut change, mut kind) = describe_change(&a.settings, &b.settings);
        let rewards_changed = a.rewards != b.rewards;
        let track_changed = a.track != b.track;
        if a.arch != b.arch {
            change = if change.is_empty() { "network design".into() } else { format!("{change}, network design") };
            kind = if kind == "other" { "network design" } else { "several" };
        }
        if track_changed {
            kind = "rewards/track";
        } else if rewards_changed {
            kind = if change.is_empty() { "rewards" } else { "several" };
        }
        if rewards_changed {
            change = if change.is_empty() { "rewards".into() } else { format!("{change}, rewards") };
        }
        if track_changed {
            change = if change.is_empty() { "track".into() } else { format!("{change}, track") };
        }
        let mut lo = a.end.saturating_sub(window).max(a.start);
        let mut hi = (b.start + window).min(b.end);
        // lap-time windows don't stop at time-limit changes: a lap time doesn't depend on the limit
        let (lap_lo, lap_hi) = (lo, hi);
        // stop each side at a change of time limit, so both sides are on the same footing
        while lo + 1 < a.end && limits[lo].is_some() && limits[a.end - 1].is_some() && limits[lo] != limits[a.end - 1] {
            lo += 1;
        }
        let first_after = limits[b.start];
        if let Some(k) = (b.start..hi).find(|&k| limits[k].is_some() && first_after.is_some() && limits[k] != first_after) {
            hi = k;
        }
        let (nb, na) = (a.end - lo, hi - b.start);
        let mut v = Verdict {
            at: b.start,
            run: j,
            change,
            kind,
            outcome: Outcome::CantTell(Reason::WithinNoise),
            z: None,
            before: None,
            after: None,
            n_before: nb,
            n_after: na,
            stage: a.stage,
            lap_change: None,
            by_lap: false,
        };
        v.outcome = if track_changed {
            Outcome::CantTell(Reason::TrackChanged)
        } else if let Some(m) = measure {
            v.by_lap = m == Measure::Lap;
            if nb < min_side || na < min_side {
                Outcome::CantTell(Reason::TooShort(nb, na, min_side))
            } else if limit_changes(&limits, lo, hi) {
                Outcome::CantTell(Reason::TimeLimitChanged)
            } else if rewards_changed && matches!(m, Measure::Score | Measure::Rate) {
                Outcome::CantTell(Reason::RewardsChanged)
            } else if let Some((fb, fa, z)) = measure_series(&gens[lo..a.end], m, min_side)
                .zip(measure_series(&gens[b.start..hi], m, min_side))
                .and_then(|(yb, ya)| slope_change(&yb, &ya))
            {
                v.z = Some(z);
                v.before = Some(fb);
                v.after = Some(fa);
                if z > threshold { Outcome::Helped } else if z < -threshold { Outcome::Hurt } else { Outcome::CantTell(Reason::WithinNoise) }
            } else {
                Outcome::CantTell(Reason::MissingMeasure(m))
            }
        } else if rewards_changed {
            let (nb, na) = (a.end - lap_lo, lap_hi - b.start);
            v.n_before = nb;
            v.n_after = na;
            if nb < min_side || na < min_side {
                Outcome::CantTell(Reason::TooShort(nb, na, min_side))
            } else {
                match (lap_series(&gens[lap_lo..a.end], min_side), lap_series(&gens[b.start..lap_hi], min_side)) {
                    (Some(yb), Some(ya)) => match lap_slope_change(&yb, &ya) {
                        Some((fb, fa, z)) => {
                            v.z = Some(z);
                            v.before = Some(fb);
                            v.after = Some(fa);
                            v.by_lap = true;
                            if let (Some(lb), Some(la)) = (lap_median(&gens[lap_lo..a.end]), lap_median(&gens[b.start..lap_hi])) {
                                v.lap_change = Some(la - lb);
                            }
                            if z > lap_threshold {
                                Outcome::Helped
                            } else if z < -lap_threshold {
                                Outcome::Hurt
                            } else {
                                Outcome::CantTell(Reason::WithinNoise)
                            }
                        }
                        None => Outcome::CantTell(Reason::RewardsChanged),
                    },
                    _ => Outcome::CantTell(Reason::RewardsChanged),
                }
            }
        } else if nb < min_side || na < min_side {
            Outcome::CantTell(Reason::TooShort(nb, na, min_side))
        } else if limit_changes(&limits, lo, hi) {
            Outcome::CantTell(Reason::TimeLimitChanged)
        } else {
            match (medians(&gens[lo..a.end]), medians(&gens[b.start..hi])) {
                (Some(yb), Some(ya)) => match slope_change(&yb, &ya) {
                    Some((fb, fa, z)) => {
                        v.z = Some(z);
                        v.before = Some(fb);
                        v.after = Some(fa);
                        if let (Some(lb), Some(la)) = (lap_median(&gens[lo..a.end]), lap_median(&gens[b.start..hi])) {
                            v.lap_change = Some(la - lb);
                        }
                        if z > threshold {
                            Outcome::Helped
                        } else if z < -threshold {
                            Outcome::Hurt
                        } else {
                            Outcome::CantTell(Reason::WithinNoise)
                        }
                    }
                    None => Outcome::CantTell(Reason::MissingScores),
                },
                _ => Outcome::CantTell(Reason::MissingScores),
            }
        };
        out.push(v);
    }
    out
}

/// |z| of fake change points inside unchanged runs, starting at generation `own_from` (so
/// history shared with another save isn't counted twice).
pub fn placebo_z(gens: &[Gen], runs: &[Run], own_from: usize, window: usize, min_side: usize) -> Vec<f64> {
    let limits = time_limits(gens);
    let mut out = Vec::new();
    for r in runs {
        let start = r.start.max(own_from);
        if r.end < start + 2 * min_side {
            continue;
        }
        let mut c = start + min_side;
        while c + min_side <= r.end {
            let lo = c.saturating_sub(window).max(start);
            let hi = (c + window).min(r.end);
            let mut lo = lo;
            while lo + 1 < c && limits[lo].is_some() && limits[c - 1].is_some() && limits[lo] != limits[c - 1] {
                lo += 1;
            }
            let hi = (c..hi).find(|&k| limits[k].is_some() && limits[c].is_some() && limits[k] != limits[c]).unwrap_or(hi);
            if c - lo < min_side || hi - c < min_side {
                c += min_side;
                continue;
            }
            if let (Some(yb), Some(ya)) = (medians(&gens[lo..c]), medians(&gens[c..hi])) {
                if let Some((_, _, z)) = slope_change(&yb, &ya) {
                    out.push(z.abs());
                }
            }
            c += min_side;
        }
    }
    out
}

/// Placebo |z| for lap-time verdicts: fake change points inside unchanged runs, judged on the
/// best-lap series (no time-limit truncation, like the real lap verdicts).
pub fn placebo_z_lap(gens: &[Gen], runs: &[Run], own_from: usize, window: usize, min_side: usize) -> Vec<f64> {
    let mut out = Vec::new();
    for r in runs {
        let start = r.start.max(own_from);
        if r.end < start + 2 * min_side {
            continue;
        }
        let mut c = start + min_side;
        while c + min_side <= r.end {
            let lo = c.saturating_sub(window).max(start);
            let hi = (c + window).min(r.end);
            if let (Some(yb), Some(ya)) = (lap_series(&gens[lo..c], min_side), lap_series(&gens[c..hi], min_side)) {
                if let Some((_, _, z)) = lap_slope_change(&yb, &ya) {
                    out.push(z.abs());
                }
            }
            c += min_side;
        }
    }
    out
}

/// Placebo calibration for the experiment's measure, using fixed-time-limit windows.
pub fn placebo_z_for_measure(gens: &[Gen], runs: &[Run], own_from: usize, window: usize, min_side: usize, measure: Measure) -> Vec<f64> {
    let limits = time_limits(gens);
    let mut out = Vec::new();
    let min_side = min_side.max(3);
    for r in runs {
        let start = r.start.max(own_from);
        for c in ((start + min_side)..=r.end.saturating_sub(min_side)).step_by(min_side) {
            let lo = c.saturating_sub(window).max(start);
            let hi = (c + window).min(r.end);
            if c - lo < min_side || hi - c < min_side || limit_changes(&limits, lo, hi) {
                continue;
            }
            if let Some((_, _, z)) = measure_series(&gens[lo..c], measure, min_side)
                .zip(measure_series(&gens[c..hi], measure, min_side))
                .and_then(|(yb, ya)| slope_change(&yb, &ya))
            {
                out.push(z.abs());
            }
        }
    }
    out
}

/// The 95th percentile of placebo |z|, or the fallback when there are too few points.
pub fn threshold(mut placebo: Vec<f64>) -> (f64, usize) {
    let n = placebo.len();
    if n < MIN_PLACEBO {
        return (FALLBACK_THRESHOLD, n);
    }
    placebo.sort_by(f64::total_cmp);
    (placebo[((0.95 * n as f64) as usize).min(n - 1)], n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ols_recovers_slope() {
        let y: Vec<f64> = (0..20).map(|i| 3.0 + 0.5 * i as f64).collect();
        let f = ols(&y).unwrap();
        assert!((f.slope - 0.5).abs() < 1e-9);
        assert!(f.sd < 1e-9);
    }

    #[test]
    fn clear_change_is_detected() {
        let before: Vec<f64> = (0..20).map(|i| 10.0 + if i % 2 == 0 { 0.3 } else { -0.3 }).collect();
        let after: Vec<f64> = (0..20).map(|i| 10.0 + 1.0 * i as f64 + if i % 2 == 0 { 0.3 } else { -0.3 }).collect();
        let (_, _, z) = slope_change(&before, &after).unwrap();
        assert!(z > 10.0, "z = {z}");
    }
}
