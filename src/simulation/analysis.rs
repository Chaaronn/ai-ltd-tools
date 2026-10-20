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
    pub median: Option<f64>,
    pub label: String,
    pub differences: Vec<(String, f64)>,
    pub seed_differences: Vec<(i64, f64)>,
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
pub fn comparisons(spec: &JobSpec, observations: &[Observation]) -> Vec<Comparison> {
    let mut out = vec![];
    // Durable identity deduplicates replayed/resumed observations before matching pairs.
    let rows: BTreeMap<_, _> = observations
        .iter()
        .map(|o| ((o.seed, o.stage, o.arm), o))
        .collect();
    for stage in &spec.stages {
        for arm in 1..=spec.alternatives.len() {
            let mut grouped: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            let mut missing = 0;
            let mut trials = 0;
            let mut seed_differences = vec![];
            for seed in &spec.seeds {
                let pair = rows
                    .get(&(*seed, *stage, 0))
                    .zip(rows.get(&(*seed, *stage, arm)));
                if let Some((a, b)) = pair {
                    if let Some(d) = difference(spec.measure, &a.metrics, &b.metrics) {
                        trials += 1;
                        seed_differences.push((*seed, d));
                        let family = if spec.mode == CampaignMode::SavedWeights {
                            spec.input.source_family.clone()
                        } else {
                            format!("seed:{seed}")
                        };
                        grouped.entry(family).or_default().push(d);
                    } else {
                        missing += 1;
                    }
                } else {
                    missing += 1;
                }
            }
            let differences: Vec<_> = grouped
                .into_iter()
                .filter_map(|(f, mut v)| crate::lab::runs::median(&mut v).map(|m| (f, m)))
                .collect();
            let mut values: Vec<_> = differences.iter().map(|(_, v)| *v).collect();
            let median = crate::lab::runs::median(&mut values);
            let wins = values.iter().filter(|v| **v > 1e-9).count();
            let losses = values.iter().filter(|v| **v < -1e-9).count();
            let practical = if spec.measure == Measure::Survival {
                2.0
            } else {
                1.0
            };
            let label = if values.len() < 5 {
                "Insufficient data"
            } else if wins * 5 >= values.len() * 4 && median.is_some_and(|m| m >= practical) {
                "Promising in simulation"
            } else {
                "Mixed"
            };
            out.push(Comparison {
                arm,
                stage: *stage,
                families: values.len(),
                trials,
                missing,
                wins,
                ties: values.len() - wins - losses,
                losses,
                median,
                label: label.into(),
                differences,
                seed_differences,
            });
        }
    }
    out
}
