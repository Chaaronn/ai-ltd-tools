//! Looking inside a network: which inputs it uses, and whether its weights are still changing.
//!
//! The save stores weights but not activations, so "usage" is estimated from weight sizes:
//! each output starts with importance 1, and importance flows backwards through the layers in
//! proportion to |weight|. A sensor whose weights are all near zero can't influence the car.
//! It's an estimate: an input with a large typical value can matter more than its weights
//! suggest.

use super::format::{Save, Weights};

/// Importance of every neuron, per layer (inputs first), each layer summing to 1.
pub fn importance(w: &Weights) -> Vec<Vec<f64>> {
    let shape = w.shape();
    if shape.len() < 2 {
        return Vec::new();
    }
    let mut imp: Vec<Vec<f64>> = vec![Vec::new(); shape.len()];
    let outs = *shape.last().unwrap_or(&0);
    imp[shape.len() - 1] = vec![1.0 / outs.max(1) as f64; outs];
    for l in (0..w.w.len()).rev() {
        let next = imp[l + 1].clone();
        // each neuron hands its importance back to its inputs in proportion to |weight|
        let col_tot: Vec<f64> = (0..next.len()).map(|j| w.w[l].iter().map(|row| row.get(j).map_or(0.0, |x| x.abs() as f64)).sum()).collect();
        let mut cur: Vec<f64> = w.w[l]
            .iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .filter(|(j, _)| col_tot[*j] > 0.0)
                    .map(|(j, x)| x.abs() as f64 / col_tot[j] * next[j])
                    .sum()
            })
            .collect();
        let tot: f64 = cur.iter().sum();
        if tot > 0.0 {
            cur.iter_mut().for_each(|v| *v /= tot);
        }
        imp[l] = cur;
    }
    imp
}

/// Input usage shares (sum 1) for one output only.
pub fn usage_for_output(w: &Weights, output: usize) -> Vec<f64> {
    let mut only = w.clone();
    // zero the other outputs' incoming weights, then reuse the backward pass
    if let Some(last) = only.w.last_mut() {
        for row in last.iter_mut() {
            for (j, x) in row.iter_mut().enumerate() {
                if j != output {
                    *x = 0.0;
                }
            }
        }
    }
    let imp = importance(&only);
    imp.first().cloned().unwrap_or_default()
}

/// Mean |w| relative change between two networks of the same shape, per layer.
pub fn layer_change(a: &Weights, b: &Weights) -> Option<Vec<f64>> {
    if a.shape() != b.shape() {
        return None;
    }
    Some(
        a.w.iter()
            .zip(&b.w)
            .map(|(la, lb)| {
                let (mut d, mut m, mut n) = (0.0f64, 0.0f64, 0usize);
                for (ra, rb) in la.iter().zip(lb) {
                    for (x, y) in ra.iter().zip(rb) {
                        d += (x - y).abs() as f64;
                        m += x.abs().max(y.abs()) as f64;
                        n += 1;
                    }
                }
                if n == 0 || m == 0.0 { 0.0 } else { d / m }
            })
            .collect(),
    )
}

/// Relative change over all weights.
pub fn total_change(a: &Weights, b: &Weights) -> Option<f64> {
    if a.shape() != b.shape() {
        return None;
    }
    let (mut d, mut m) = (0.0f64, 0.0f64);
    for (la, lb) in a.w.iter().zip(&b.w) {
        for (ra, rb) in la.iter().zip(lb) {
            for (x, y) in ra.iter().zip(rb) {
                d += (x - y).abs() as f64;
                m += x.abs().max(y.abs()) as f64;
            }
        }
    }
    (m > 0.0).then_some(d / m)
}

#[derive(Clone, Debug)]
pub struct Learning {
    /// The snapshot compared with the current network.
    pub from_gen: usize,
    pub to_gen: usize,
    /// Relative weight change per layer, since `from_gen`.
    pub layers: Vec<f64>,
    pub total: f64,
    /// The same, per 10 generations.
    pub per_10: f64,
    /// Change per 10 generations between consecutive snapshots (and the current network):
    /// (from gen, to gen, change per 10 gens), oldest first.
    pub history: Vec<(usize, usize, f64)>,
    /// Input usage at `from_gen`.
    pub usage_then: Vec<f64>,
}

/// Below this change per 10 generations (0.5%) the weights have all but stopped moving.
pub const STILL: f64 = 0.005;

/// Compare the current network with its snapshots.
pub fn learning(save: &Save) -> Option<Learning> {
    let cur = save.weights.as_ref()?;
    let now = save.gens.len();
    let shape = cur.shape();
    let snaps: Vec<&(usize, Weights)> = save.snapshot_weights.iter().filter(|(g, w)| *g < now && w.shape() == shape).collect();
    // the latest snapshot at least 10 generations back, else the oldest available
    let base = snaps.iter().rev().find(|(g, _)| now - g >= 10).or(snaps.first())?;
    let (from_gen, then) = (base.0, &base.1);
    let layers = layer_change(then, cur)?;
    let total = total_change(then, cur)?;
    let span = (now - from_gen).max(1) as f64;
    let mut chain: Vec<(usize, &Weights)> = snaps.iter().map(|(g, w)| (*g, w)).collect();
    chain.push((now, cur));
    let history = chain
        .windows(2)
        .filter_map(|p| {
            let c = total_change(p[0].1, p[1].1)?;
            let gens = (p[1].0 - p[0].0).max(1) as f64;
            Some((p[0].0, p[1].0, c / gens * 10.0))
        })
        .collect();
    Some(Learning {
        from_gen,
        to_gen: now,
        layers,
        total,
        per_10: total / span * 10.0,
        history,
        usage_then: importance(then).first().cloned().unwrap_or_default(),
    })
}

/// Display name of input `i`: "ray −35°" for raycasts, the sensor name otherwise.
pub fn input_name(save: &Save, i: usize) -> String {
    match (save.rays.get(i).copied().flatten(), save.arch.sensors.get(i)) {
        (Some((deg, _)), _) => format!("ray {deg:+.0}°"),
        (None, Some(t)) => super::format::sensor_name(t),
        _ => format!("input {i}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(w: Vec<Vec<Vec<f32>>>) -> Weights {
        Weights { b: w.iter().map(|l| vec![0.0; l[0].len()]).collect(), w }
    }

    #[test]
    fn unused_input_gets_no_usage() {
        // 3 inputs -> 2 hidden -> 1 output; input 2 has zero weights
        let w = net(vec![vec![vec![1.0, 0.5], vec![0.5, 1.0], vec![0.0, 0.0]], vec![vec![1.0], vec![1.0]]]);
        let imp = importance(&w);
        assert_eq!(imp[0].len(), 3);
        assert!(imp[0][2].abs() < 1e-12);
        assert!((imp[0].iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!((imp[0][0] - imp[0][1]).abs() < 1e-9);
    }

    #[test]
    fn change_is_relative() {
        let a = net(vec![vec![vec![1.0, 2.0]]]);
        let b = net(vec![vec![vec![1.1, 2.0]]]);
        let c = total_change(&a, &b).unwrap();
        assert!((c - 0.1 / 3.1).abs() < 1e-6, "{c}");
        assert_eq!(total_change(&a, &a), Some(0.0));
    }
}

/// One point in the network's recent past: a snapshot, or the current network (last).
#[derive(Clone, Debug)]
pub struct SnapPoint {
    pub at: usize,
    /// Relative change from the previous point (None for the first, or a shape change).
    pub change: Option<f64>,
    /// Input usage shares.
    pub usage: Vec<f64>,
    /// Mean |weight| over the whole network.
    pub mean_abs: f64,
    /// Best lap on the network's latest track up to this generation.
    pub best_lap: Option<f64>,
    /// Median score at this generation.
    pub median: Option<f64>,
}

fn mean_abs(w: &Weights) -> f64 {
    let (mut s, mut n) = (0.0f64, 0usize);
    for l in &w.w {
        for r in l {
            for x in r {
                s += x.abs() as f64;
                n += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { s / n as f64 }
}

/// The snapshots (same shape as now) plus the current network, oldest first.
pub fn snapshot_series(save: &Save, track: &str) -> Vec<SnapPoint> {
    let Some(cur) = save.weights.as_ref() else { return Vec::new() };
    let now = save.gens.len();
    let shape = cur.shape();
    let keys = super::runs::track_keys(&save.gens);
    let lap_until = |g: usize| -> Option<f64> {
        save.gens[..g.min(save.gens.len())].iter().zip(&keys).filter(|(_, k)| *k == track).filter_map(|(x, _)| x.best_lap).reduce(f64::min)
    };
    let mut pts: Vec<(usize, &Weights)> = save.snapshot_weights.iter().filter(|(g, w)| *g < now && w.shape() == shape).map(|(g, w)| (*g, w)).collect();
    pts.push((now, cur));
    let mut out: Vec<SnapPoint> = Vec::new();
    for (k, (at, w)) in pts.iter().enumerate() {
        out.push(SnapPoint {
            at: *at,
            change: if k == 0 { None } else { total_change(pts[k - 1].1, w) },
            usage: importance(w).first().cloned().unwrap_or_default(),
            mean_abs: mean_abs(w),
            best_lap: lap_until(*at + 1),
            median: save.gens.get(at.saturating_sub(1).min(now.saturating_sub(1))).and_then(|g| g.median),
        });
    }
    out
}

/// Hidden neurons whose outgoing weights are tiny next to the rest of their layer (under 10%
/// of the layer's average): capacity the network isn't using. (layer, neuron), layers from 1.
pub fn idle_neurons(w: &Weights) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for l in 1..w.w.len() {
        let totals: Vec<f64> = w.w[l].iter().map(|row| row.iter().map(|x| x.abs() as f64).sum()).collect();
        let avg = totals.iter().sum::<f64>() / totals.len().max(1) as f64;
        for (i, t) in totals.iter().enumerate() {
            if avg > 0.0 && *t < 0.1 * avg {
                out.push((l, i));
            }
        }
    }
    out
}

/// Relative change of each output's incoming weights between two networks of the same shape.
pub fn output_change(then: &Weights, now: &Weights) -> Option<Vec<f64>> {
    if then.shape() != now.shape() {
        return None;
    }
    let (a, b) = (then.w.last()?, now.w.last()?);
    let outs = a.first()?.len();
    Some(
        (0..outs)
            .map(|j| {
                let (mut d, mut m) = (0.0f64, 0.0f64);
                for (ra, rb) in a.iter().zip(b) {
                    d += (ra[j] - rb[j]).abs() as f64;
                    m += ra[j].abs().max(rb[j].abs()) as f64;
                }
                if m == 0.0 { 0.0 } else { d / m }
            })
            .collect(),
    )
}

/// Moving (weights changing) × improving (laps or score still gaining): what the combination means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Diagnosis {
    Learning,
    Drifting,
    FineTuning,
    Stuck,
}

/// Generations without a gain before "not improving" (shorter than a stall, on purpose).
pub const NOT_IMPROVING_GENS: usize = 30;

pub fn diagnose(learning: &Learning, since_gain: usize) -> Diagnosis {
    let moving = learning.history.last().map_or(learning.per_10, |h| h.2) >= STILL;
    let improving = since_gain < NOT_IMPROVING_GENS;
    match (moving, improving) {
        (true, true) => Diagnosis::Learning,
        (true, false) => Diagnosis::Drifting,
        (false, true) => Diagnosis::FineTuning,
        (false, false) => Diagnosis::Stuck,
    }
}
