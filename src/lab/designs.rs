//! Comparing network designs: inputs (sensors), layers, size and outputs.
//!
//! A design is fixed for a whole network (or for long stretches of it), so every comparison
//! here collapses runs per network, then networks per detected family, at each stage.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::compare::{self, Measure, NSTAGES};
use super::format::{self, Arch, OUTPUTS};
use super::library::{Library, Network};
use super::overview::{self, beats, strength, Strength};
use super::runs::{self, RANDOM_TRACK};

/// One network and how fast it learned at each stage.
#[derive(Clone, Debug)]
pub struct NetSummary {
    pub file: String,
    pub vehicle: Option<u8>,
    pub arch: Arch,
    pub own_gens: usize,
    /// Median gain per 10 gens of its own runs (10+ gens) at early / mid / late.
    pub gain: [Option<f64>; NSTAGES],
    pub runs: [usize; NSTAGES],
    /// Fastest lap on its most-lapped fixed track: (track name, seconds).
    pub fastest: Option<(String, f64)>,
}

fn stage_gains(n: &Network, measure: Measure) -> ([Vec<f64>; NSTAGES], Vec<(Arch, [Vec<f64>; NSTAGES])>) {
    let f = compare::Filters { measure, ..compare::Filters::default() };
    let mut all: [Vec<f64>; NSTAGES] = Default::default();
    let mut by_arch: Vec<(Arch, [Vec<f64>; NSTAGES])> = Vec::new();
    for r in n.runs.iter().filter(|r| r.start >= n.fork.own_from && r.len() >= 10) {
        let (Some(s), Some(g)) = (compare::stage_index(r.stage), f.gain(r)) else { continue };
        all[s].push(g);
        match by_arch.iter_mut().find(|(a, _)| *a == r.arch) {
            Some((_, v)) => v[s].push(g),
            None => {
                let mut v: [Vec<f64>; NSTAGES] = Default::default();
                v[s].push(g);
                by_arch.push((r.arch.clone(), v));
            }
        }
    }
    (all, by_arch)
}

pub fn summaries(lib: &Library, measure: Measure) -> Vec<NetSummary> {
    let mut out = Vec::new();
    for n in lib.networks.iter().filter(|n| n.gens() > n.fork.own_from) {
        let (all, _) = stage_gains(n, measure);
        let runs: [usize; NSTAGES] = std::array::from_fn(|k| all[k].len());
        let gain: [Option<f64>; NSTAGES] = std::array::from_fn(|k| runs::median(&mut all[k].clone()));
        // most-lapped fixed track
        let keys = runs::track_keys(&n.save.gens);
        let mut laps: HashMap<&str, (usize, f64)> = HashMap::new();
        for (g, k) in n.save.gens.iter().zip(&keys).skip(n.fork.own_from) {
            if let Some(l) = g.best_lap.filter(|_| k != RANDOM_TRACK) {
                let e = laps.entry(k.as_str()).or_insert((0, f64::INFINITY));
                e.0 += 1;
                e.1 = e.1.min(l);
            }
        }
        let fastest = laps.into_iter().max_by_key(|(_, (c, _))| *c).map(|(k, (_, l))| (overview::track_name(lib, k).0, l));
        out.push(NetSummary {
            file: n.file.clone(),
            vehicle: n.save.vehicle,
            arch: n.save.arch.clone(),
            own_gens: n.gens() - n.fork.own_from,
            gain,
            runs,
            fastest,
        });
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.own_gens));
    out
}

/// One input or output, and how networks with it compared with networks without it.
#[derive(Clone, Debug)]
pub struct Feature {
    /// "Input" or "Output"
    pub kind: &'static str,
    pub name: String,
    pub with_nets: Vec<String>,
    pub without_nets: Vec<String>,
    /// Per stage: (family medians with, family medians without), sorted.
    pub stages: [(Vec<f64>, Vec<f64>); NSTAGES],
}

impl Feature {
    /// (beats, strength) at one stage: does "with" out-gain "without"?
    pub fn evidence(&self, stage: usize) -> Option<(f64, Strength, bool)> {
        let (w, wo) = &self.stages[stage];
        if w.len() < 2 || wo.len() < 2 {
            return None;
        }
        let p = beats(w, wo);
        let with_better = p >= 0.5;
        let (best, other, pb) = if with_better { (w, wo, p) } else { (wo, w, 1.0 - p) };
        let gap = runs::quantile(best, 0.5) - runs::quantile(other, 0.5);
        let s = if gap < overview::PRACTICAL { Strength::Weak } else { strength(best.len(), Some(other.len()), Some(pb)) };
        Some((pb, s, with_better))
    }
}

/// Every non-raycast sensor type and every output that some (but not all) networks use.
pub fn features(lib: &Library, measure: Measure) -> Vec<Feature> {
    // per network and design: medians per stage
    let mut rows: Vec<(String, Arch, [Option<f64>; NSTAGES])> = Vec::new();
    for n in lib.networks.iter().filter(|n| n.gens() > n.fork.own_from) {
        let (_, by_arch) = stage_gains(n, measure);
        for (a, v) in by_arch {
            let m: [Option<f64>; NSTAGES] = std::array::from_fn(|k| runs::median(&mut v[k].clone()));
            rows.push((n.file.clone(), a, m));
        }
    }
    let mut names: BTreeSet<(&'static str, String)> = BTreeSet::new();
    for (_, a, _) in &rows {
        for e in a.extras() {
            names.insert(("Input", e));
        }
        for &o in &a.outputs {
            names.insert(("Output", OUTPUTS.get(o as usize).copied().unwrap_or("?").to_string()));
        }
    }
    let has = |a: &Arch, kind: &str, name: &str| -> bool {
        if kind == "Input" {
            a.sensors.iter().any(|s| s == name)
        } else {
            a.outputs.iter().any(|&o| OUTPUTS.get(o as usize).copied() == Some(name))
        }
    };
    let mut out = Vec::new();
    for (kind, name) in names {
        let mut f = Feature { kind, name: name.clone(), with_nets: Vec::new(), without_nets: Vec::new(), stages: Default::default() };
        let mut per: BTreeMap<(usize, bool, String, String), Vec<f64>> = BTreeMap::new();
        for (file, a, m) in &rows {
            let w = has(a, kind, &name);
            let list = if w { &mut f.with_nets } else { &mut f.without_nets };
            if !list.contains(file) {
                list.push(file.clone());
            }
            for s in 0..NSTAGES {
                if let Some(v) = m[s] {
                    let n = lib.by_file(file).unwrap();
                    per.entry((s, w, overview::family(lib, n).into(), file.clone())).or_default().push(v);
                }
            }
        }
        if f.with_nets.is_empty() || f.without_nets.is_empty() {
            continue;
        }
        let mut families: BTreeMap<(usize, bool, String), Vec<f64>> = BTreeMap::new();
        for ((s, w, family, _), mut values) in per {
            families.entry((s, w, family)).or_default().push(runs::median(&mut values).unwrap());
        }
        for ((s, w, _), mut values) in families {
            let v = runs::median(&mut values).unwrap();
            if w { f.stages[s].0.push(v) } else { f.stages[s].1.push(v) }
        }
        for s in 0..NSTAGES {
            f.stages[s].0.sort_by(f64::total_cmp);
            f.stages[s].1.sort_by(f64::total_cmp);
        }
        if kind == "Input" {
            f.name = format::sensor_name(&f.name);
        }
        out.push(f);
    }
    out
}

/// Networks with different designs that lapped the same fixed track.
#[derive(Clone, Debug)]
pub struct HeadToHead {
    pub track: String,
    /// (network, design, fastest lap, generations on this track), fastest first.
    pub rows: Vec<(String, Arch, f64, usize)>,
}

pub fn head_to_head(lib: &Library) -> Vec<HeadToHead> {
    let mut per: BTreeMap<String, Vec<(String, Arch, f64, usize)>> = BTreeMap::new();
    for n in lib.networks.iter().filter(|n| n.gens() > n.fork.own_from) {
        let keys = runs::track_keys(&n.save.gens);
        let mut best: HashMap<&str, (f64, usize)> = HashMap::new();
        for (g, k) in n.save.gens.iter().zip(&keys).skip(n.fork.own_from) {
            if k == RANDOM_TRACK || k.is_empty() {
                continue;
            }
            let e = best.entry(k.as_str()).or_insert((f64::INFINITY, 0));
            e.1 += 1;
            if let Some(l) = g.best_lap {
                e.0 = e.0.min(l);
            }
        }
        for (k, (l, gens)) in best {
            if l.is_finite() {
                per.entry(k.to_string()).or_default().push((n.file.clone(), n.save.arch.clone(), l, gens));
            }
        }
    }
    let mut out: Vec<HeadToHead> = per
        .into_iter()
        .filter(|(_, v)| v.len() >= 2 && v.iter().any(|r| r.1 != v[0].1))
        .map(|(k, mut rows)| {
            rows.sort_by(|a, b| a.2.total_cmp(&b.2));
            HeadToHead { track: overview::track_name(lib, &k).0, rows }
        })
        .collect();
    out.sort_by_key(|h| std::cmp::Reverse(h.rows.len()));
    out
}
