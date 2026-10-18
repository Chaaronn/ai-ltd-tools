//! Minimum detectable effect (information-cost) for pending TooShort holds and WithinNoise
//! verdicts. Numbers are hidden until placebo calibration is thick enough (`MIN_PLACEBO`).

use super::format::Gen;
use super::library::{Library, Network};
use super::runs::Run;
use super::verdict::{self, Fit, Outcome, Reason, Verdict, MIN_PLACEBO};

/// One projected (or current) after-side length and its MDE in player-facing units per 10 gens.
#[derive(Clone, Debug)]
pub struct ForwardRow {
    pub length: usize,
    pub gens_to_add: usize,
    /// Absolute MDE in the judgment measure, scaled to “per 10 generations”.
    pub mde_per_10: f64,
    /// True when after-side noise was projected (or borrowed from before).
    pub estimated: bool,
}

#[derive(Clone, Debug)]
pub struct ForwardPlan {
    pub by_lap: bool,
    pub n_after: usize,
    pub min_side: usize,
    pub window: usize,
    pub rows: Vec<ForwardRow>,
    /// After side already at the verdict window — waiting cannot improve detectability.
    pub at_window: bool,
}

#[derive(Clone, Debug)]
pub struct Retrospective {
    pub by_lap: bool,
    pub mde_per_10: f64,
    pub n_before: usize,
    pub n_after: usize,
}

/// `T · √(se_b² + se_a²)` — slope-change units matching `slope_change` / `lap_slope_change`.
pub fn mde_slope(threshold: f64, se_b: f64, se_a: f64) -> f64 {
    threshold * (se_b * se_b + se_a * se_a + 1e-12).sqrt()
}

fn sxx(n: usize) -> f64 {
    let nf = n as f64;
    nf * (nf * nf - 1.0) / 12.0
}

/// Project after-side SE to length `n_prime` using KTD6 (hold variance inflation from `fit`).
fn project_se(fit: &Fit, n_current: usize, n_prime: usize) -> f64 {
    if n_prime < 3 {
        return fit.se;
    }
    let sxx_n = sxx(n_current.max(3));
    if fit.sd < 1e-15 || sxx_n < 1e-15 {
        return fit.se;
    }
    let k = fit.se * sxx_n.sqrt() / fit.sd;
    k * fit.sd / sxx(n_prime).sqrt()
}

fn thick(lib: &Library, by_lap: bool) -> bool {
    if by_lap {
        lib.lap_placebo_points >= MIN_PLACEBO
    } else {
        lib.placebo_points >= MIN_PLACEBO
    }
}

fn threshold_for(lib: &Library, by_lap: bool) -> f64 {
    if by_lap {
        lib.lap_threshold
    } else {
        lib.threshold
    }
}

fn per_10(mde_slope: f64) -> f64 {
    mde_slope * 10.0
}

/// Last verdict is a pending TooShort on the final run with an adequate before side.
pub fn pending_too_short<'a>(n: &'a Network) -> Option<&'a Verdict> {
    let v = n.verdicts.last()?;
    if v.run != n.runs.len().saturating_sub(1) {
        return None;
    }
    match v.outcome {
        Outcome::CantTell(Reason::TooShort(nb, _na, need)) if nb >= need => Some(v),
        _ => None,
    }
}

/// Would this change be judged on lap time? (same rule as `verdicts`: rewards change, not track.)
fn would_be_by_lap(prev: &Run, cur: &Run) -> bool {
    prev.rewards != cur.rewards && prev.track == cur.track
}

/// Window bounds mirroring `verdict::verdicts` for run pair `(prev, cur)`.
fn window_bounds(gens: &[Gen], prev: &Run, cur: &Run, window: usize) -> (usize, usize, usize, usize) {
    let limits = verdict::time_limits(gens);
    let mut lo = prev.end.saturating_sub(window).max(prev.start);
    let mut hi = (cur.start + window).min(cur.end);
    let (lap_lo, lap_hi) = (lo, hi);
    while lo + 1 < prev.end && limits[lo].is_some() && limits[prev.end - 1].is_some() && limits[lo] != limits[prev.end - 1] {
        lo += 1;
    }
    let first_after = limits[cur.start];
    if let Some(k) = (cur.start..hi).find(|&k| limits[k].is_some() && first_after.is_some() && limits[k] != first_after) {
        hi = k;
    }
    (lo, hi, lap_lo, lap_hi)
}

/// Forward detectability for a pending after-short hold, or `None` when hidden / inapplicable.
pub fn forward_plan(lib: &Library, n: &Network) -> Option<ForwardPlan> {
    let v = pending_too_short(n)?;
    let j = v.run;
    if j == 0 || j >= n.runs.len() {
        return None;
    }
    let (prev, cur) = (&n.runs[j - 1], &n.runs[j]);
    let by_lap = would_be_by_lap(prev, cur);
    if !thick(lib, by_lap) {
        return None;
    }
    let window = lib.window;
    let min_side = lib.min_side;
    let na = v.n_after;
    if na >= window {
        return Some(ForwardPlan {
            by_lap,
            n_after: na,
            min_side,
            window,
            rows: Vec::new(),
            at_window: true,
        });
    }

    let gens = &n.save.gens;
    let (lo, hi, lap_lo, lap_hi) = window_bounds(gens, prev, cur, window);
    let limits = verdict::time_limits(gens);

    // Score path: a hold that would become TimeLimitChanged is not worth pricing.
    if !by_lap && verdict::limit_changes(&limits, lo, hi) {
        return None;
    }

    let before_slice = if by_lap {
        &gens[lap_lo..prev.end]
    } else {
        &gens[lo..prev.end]
    };
    let after_end = if by_lap { lap_hi.min(cur.end) } else { hi.min(cur.end) };
    let after_slice = &gens[cur.start..after_end];

    let before_y = if by_lap {
        verdict::lap_series(before_slice, 3)?
    } else {
        verdict::medians(before_slice)?
    };
    let before_fit = verdict::ols(&before_y)?;

    // Lap path with no laps yet on the after side cannot earn a lap verdict by waiting alone.
    if by_lap {
        let after_have = after_slice.iter().filter(|g| g.best_lap.is_some()).count();
        if after_have == 0 {
            return None;
        }
    }

    let t = threshold_for(lib, by_lap);
    let se_b = before_fit.se;

    let mut lengths: Vec<usize> = Vec::new();
    if na >= 3 {
        lengths.push(na);
    }
    if min_side > na && min_side <= window {
        lengths.push(min_side);
    }
    if window > na && !lengths.contains(&window) {
        lengths.push(window);
    }
    lengths.sort_unstable();
    lengths.dedup();

    let (noise_fit, n_noise) = if na >= min_side {
        let y = if by_lap {
            verdict::lap_series(after_slice, 3)
        } else {
            verdict::medians(after_slice)
        };
        match y.and_then(|y| verdict::ols(&y)) {
            Some(f) => (f, na.max(3)),
            None => (before_fit, v.n_before.max(3)),
        }
    } else {
        (before_fit, v.n_before.max(3))
    };

    let mut rows = Vec::new();
    for &len in &lengths {
        let se_a = if len == na && na >= 3 {
            let y = if by_lap {
                verdict::lap_series(after_slice, 3)
            } else {
                verdict::medians(after_slice)
            };
            y.and_then(|y| verdict::ols(&y)).map(|f| f.se).unwrap_or_else(|| project_se(&noise_fit, n_noise, len))
        } else {
            project_se(&noise_fit, n_noise, len)
        };
        let mde = mde_slope(t, se_b, se_a);
        rows.push(ForwardRow {
            length: len,
            gens_to_add: len.saturating_sub(na),
            mde_per_10: per_10(mde),
            estimated: len != na || na < min_side,
        });
    }

    Some(ForwardPlan {
        by_lap,
        n_after: na,
        min_side,
        window,
        rows,
        at_window: false,
    })
}

/// Retrospective MDE for a WithinNoise verdict with stored fits.
pub fn retrospective(lib: &Library, v: &Verdict) -> Option<Retrospective> {
    if !matches!(v.outcome, Outcome::CantTell(Reason::WithinNoise)) {
        return None;
    }
    if !thick(lib, v.by_lap) {
        return None;
    }
    let (b, a) = (v.before?, v.after?);
    let mde = mde_slope(threshold_for(lib, v.by_lap), b.se, a.se);
    Some(Retrospective {
        by_lap: v.by_lap,
        mde_per_10: per_10(mde),
        n_before: v.n_before,
        n_after: v.n_after,
    })
}

fn unit_label(by_lap: bool) -> &'static str {
    if by_lap {
        "s"
    } else {
        "score"
    }
}

fn fmt_mde(mde_per_10: f64, by_lap: bool) -> String {
    let u = unit_label(by_lap);
    if by_lap {
        format!("about {mde_per_10:.3} {u} per 10 gens")
    } else {
        format!("about {mde_per_10:.2} {u} per 10 gens")
    }
}

/// Copy for header / advice when a forward plan is available.
pub fn format_forward(plan: &ForwardPlan) -> String {
    if plan.at_window {
        return "Waiting longer will not improve detectability for this verdict shape (after side already at the window).".into();
    }
    let u = unit_label(plan.by_lap);
    let mut parts = Vec::new();
    for r in &plan.rows {
        let approx = if r.estimated { " ~" } else { "" };
        if r.gens_to_add == 0 {
            parts.push(format!(
                "now (~{} gens after):{approx} {} {}",
                r.length,
                fmt_mde(r.mde_per_10, plan.by_lap).trim_start_matches("about "),
                if plan.by_lap { "(lap)" } else { "(score)" }
            ));
        } else {
            parts.push(format!(
                "+{} gens →{approx} {:.3} {}/10 gens",
                r.gens_to_add,
                r.mde_per_10,
                u
            ));
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    format!(
        "Detectability (estimate): {}. Smaller effects than that would not be flagged.",
        parts.join("; ")
    )
}

/// Short fragment for the header line (after “k more gens…”).
pub fn format_forward_header(plan: &ForwardPlan) -> Option<String> {
    if plan.at_window {
        return Some("· waiting longer will not improve detectability".into());
    }
    let best = plan.rows.iter().find(|r| r.gens_to_add > 0).or_else(|| plan.rows.first())?;
    Some(format!(
        "· detect ~{:.3} {}/10 gens{}",
        best.mde_per_10,
        unit_label(plan.by_lap),
        if best.estimated { " (est.)" } else { "" }
    ))
}

/// Append to advice “why” / text for the wait suggestion.
pub fn format_forward_advice(plan: &ForwardPlan) -> String {
    format_forward(plan)
}

pub fn format_retrospective(r: &Retrospective) -> String {
    format!(
        "A change smaller than {} would not have been flagged ({} gens before, {} after).",
        fmt_mde(r.mde_per_10, r.by_lap),
        r.n_before,
        r.n_after
    )
}
