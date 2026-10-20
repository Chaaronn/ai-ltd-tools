use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comparison {
    pub arm: usize,
    pub stage: u64,
    pub families: usize,
    pub trials: usize,
    pub missing: usize,
    pub wins: usize,
    pub ties: usize,
    pub losses: usize,
    /// Median of the families' paired differences that have a size (one-sided laps excluded).
    pub median: Option<f64>,
    pub label: String,
    pub differences: Vec<(String, f64)>,
    pub seed_differences: Vec<(i64, f64)>,
    /// Lap time only: seeds where only the alternative / only the control completed a lap.
    /// Each is a clear win or loss with no size; counted in wins/losses, not in the median.
    #[serde(default)]
    pub alternative_only_laps: usize,
    #[serde(default)]
    pub control_only_laps: usize,
    /// Seeds where the alternative did better (including laps only it completed).
    #[serde(default)]
    pub seeds_better: usize,
    /// Seeds with a usable comparison (paired or one-sided).
    #[serde(default)]
    pub seeds_compared: usize,
    /// Agreement across seeds. For saved-weights campaigns this is the result to read: all seeds
    /// start from one network, so they can't count as independent families.
    #[serde(default)]
    pub seed_label: String,
}

/// One seed's comparison of control `a` with alternative `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paired {
    /// Positive = the alternative did better (% or percentage points).
    Value(f64),
    /// Lap time: only the alternative completed a lap.
    AlternativeOnly,
    /// Lap time: only the control completed a lap.
    ControlOnly,
}

fn lapped(m: &Metrics) -> bool {
    m.lapped > 0 && m.lap.is_some_and(|l| l > 0.0 && l.is_finite())
}

pub fn paired(measure: Measure, a: &Metrics, b: &Metrics) -> Option<Paired> {
    if measure == Measure::Lap {
        return match (lapped(a), lapped(b)) {
            (true, true) => difference(measure, a, b).map(Paired::Value),
            (false, true) => Some(Paired::AlternativeOnly),
            (true, false) => Some(Paired::ControlOnly),
            (false, false) => None,
        };
    }
    difference(measure, a, b).map(Paired::Value)
}

pub fn difference(measure: Measure, a: &Metrics, b: &Metrics) -> Option<f64> {
    match measure {
        Measure::Lap => a
            .lap
            .zip(b.lap)
            .filter(|(x, y)| *x > 0.0 && *y > 0.0 && a.lapped > 0 && b.lapped > 0)
            .map(|(a, b)| (a - b) / a * 100.0),
        Measure::Rate => a
            .rate
            .zip(b.rate)
            .filter(|(a, _)| a.abs() > 1e-9)
            .map(|(a, b)| (b - a) / a.abs() * 100.0),
        Measure::Survival => Some((b.survival - a.survival) * 100.0),
    }
    .filter(|x| x.is_finite())
}

/// Sort key that puts one-sided results beyond every sized difference.
fn rank(p: Paired) -> f64 {
    match p {
        Paired::Value(v) => v,
        Paired::AlternativeOnly => f64::INFINITY,
        Paired::ControlOnly => f64::NEG_INFINITY,
    }
}

pub fn comparisons(spec: &JobSpec, observations: &[Observation]) -> Vec<Comparison> {
    let mut out = vec![];
    // Durable identity deduplicates replayed/resumed observations before matching pairs.
    let rows: BTreeMap<_, _> = observations
        .iter()
        .map(|o| ((o.seed, o.stage, o.arm), o))
        .collect();
    let practical = if spec.measure == Measure::Survival {
        2.0
    } else {
        1.0
    };
    for stage in &spec.stages {
        for arm in 1..=spec.alternatives.len() {
            let mut grouped: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            let (mut missing, mut trials) = (0, 0);
            let (mut alternative_only, mut control_only) = (0, 0);
            let mut seed_differences = vec![];
            let mut seed_ranks = vec![];
            for seed in &spec.seeds {
                let pair = rows
                    .get(&(*seed, *stage, 0))
                    .zip(rows.get(&(*seed, *stage, arm)));
                let Some(p) = pair.and_then(|(a, b)| paired(spec.measure, &a.metrics, &b.metrics)) else {
                    missing += 1;
                    continue;
                };
                trials += 1;
                match p {
                    Paired::Value(d) => seed_differences.push((*seed, d)),
                    Paired::AlternativeOnly => alternative_only += 1,
                    Paired::ControlOnly => control_only += 1,
                }
                seed_ranks.push(rank(p));
                let family = if spec.mode == CampaignMode::SavedWeights {
                    spec.input.source_family.clone()
                } else {
                    format!("seed:{seed}")
                };
                grouped.entry(family).or_default().push(rank(p));
            }
            // a family's result is the median of its seeds (infinite = one-sided lap)
            let family_results: Vec<(String, f64)> = grouped
                .into_iter()
                // +inf and -inf average to NaN: an even split counts as a tie
                .filter_map(|(f, mut v)| crate::lab::runs::median(&mut v).map(|m| (f, if m.is_nan() { 0.0 } else { m })))
                .collect();
            let all: Vec<f64> = family_results.iter().map(|(_, v)| *v).collect();
            let overall = crate::lab::runs::median(&mut all.clone()).filter(|m| !m.is_nan());
            let wins = all.iter().filter(|v| **v > 1e-9).count();
            let losses = all.iter().filter(|v| **v < -1e-9).count();
            let mut sized: Vec<f64> = all.iter().copied().filter(|v| v.is_finite()).collect();
            let median = crate::lab::runs::median(&mut sized);
            let label = if all.len() < 5 {
                "Insufficient data"
            } else if wins * 5 >= all.len() * 4 && overall.is_some_and(|m| m >= practical) {
                "Promising in simulation"
            } else {
                "Mixed"
            };
            let seeds_better = seed_ranks.iter().filter(|v| **v > 1e-9).count();
            let seeds_worse = seed_ranks.iter().filter(|v| **v < -1e-9).count();
            let seed_median = crate::lab::runs::median(&mut seed_ranks.clone()).filter(|m| !m.is_nan());
            let seed_label = if seed_ranks.len() < 3 {
                "Too few seeds to judge"
            } else if seeds_better * 5 >= seed_ranks.len() * 4 && seed_median.is_some_and(|m| m >= practical) {
                "Consistently better across seeds"
            } else if seeds_worse * 5 >= seed_ranks.len() * 4 && seed_median.is_some_and(|m| m <= -practical) {
                "Consistently worse across seeds"
            } else {
                "Mixed across seeds"
            };
            out.push(Comparison {
                arm,
                stage: *stage,
                families: all.len(),
                trials,
                missing,
                wins,
                ties: all.len() - wins - losses,
                losses,
                median,
                label: label.into(),
                differences: family_results.into_iter().filter(|(_, v)| v.is_finite()).collect(),
                seed_differences,
                alternative_only_laps: alternative_only,
                control_only_laps: control_only,
                seeds_better,
                seeds_compared: seed_ranks.len(),
                seed_label: seed_label.into(),
            });
        }
    }
    out
}
