//! Head-to-head on one track: every network that trained there, lined up by generations on that
//! track, compared on the measures that mean the same thing for every car (best car's average
//! speed, middle car's speed, lap time), at matching time limits, with what differs between them.

use std::collections::{BTreeMap, BTreeSet};

use super::format;
use super::keeper;
use super::library::Library;
use super::overview;
use super::projection;
use super::runs;
use super::verdict;

/// One generation on the track.
#[derive(Clone, Debug)]
pub struct Point {
    /// Generations this network had trained on the track before this one.
    pub on_track: usize,
    /// Best car's average speed (the game's BestAverageSpeed: best score ÷ time).
    pub best_speed: Option<f64>,
    /// Middle car's score per second.
    pub median_rate: Option<f64>,
    pub best_lap: Option<f64>,
    pub time_limit: Option<f64>,
}

/// One network at one time limit.
#[derive(Clone, Debug)]
pub struct AtLimit {
    pub gens: usize,
    /// Generations on the track when this limit started and ended.
    pub from: usize,
    pub to: usize,
    pub best_speed_max: Option<f64>,
    pub median_rate: Option<f64>,
    pub best_lap: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub file: String,
    pub vehicle: &'static str,
    pub design: String,
    /// Reward setups used on this track (which metrics are on).
    pub rewards: BTreeSet<String>,
    pub population: Option<u32>,
    pub points: Vec<Point>,
    /// Generations on the track at the first lap, and the best lap.
    pub first_lap: Option<usize>,
    pub best_lap: Option<f64>,
    /// Forecast best lap 100 generations ahead (networks still on this track).
    pub forecast_100: Option<f64>,
    /// Time-limit steps: (generations on track when it started, limit).
    pub limits: Vec<(usize, f64)>,
    pub at_limit: BTreeMap<u64, AtLimit>,
    /// Copied from another network's save (shares its early history).
    pub copied_from: Option<String>,
}

fn key(limit: f64) -> u64 {
    (limit * 10.0).round() as u64
}

/// Every network with 10+ generations of its own on `track`, most trained first.
pub fn compare(lib: &Library, track: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for n in lib.networks.iter().filter(|n| !keeper::is_keeper_copy(&n.file)) {
        let g = &n.save.gens;
        let keys = runs::track_keys(g);
        let idx: Vec<usize> = (0..g.len()).filter(|&i| keys[i] == track).collect();
        let own = idx.iter().filter(|&&i| i >= n.fork.own_from).count();
        if own < 10 {
            continue;
        }
        let limits_all = verdict::time_limits(g);
        let points: Vec<Point> = idx
            .iter()
            .enumerate()
            .map(|(k, &i)| Point {
                on_track: k,
                best_speed: g[i].best_speed,
                median_rate: g[i].rate(),
                best_lap: g[i].best_lap,
                time_limit: limits_all[i],
            })
            .collect();
        let mut limits: Vec<(usize, f64)> = Vec::new();
        for p in &points {
            if let Some(t) = p.time_limit {
                if limits.last().is_none_or(|l| l.1 != t) {
                    limits.push((p.on_track, t));
                }
            }
        }
        let mut at_limit: BTreeMap<u64, AtLimit> = BTreeMap::new();
        for p in &points {
            let Some(t) = p.time_limit else { continue };
            let e = at_limit.entry(key(t)).or_insert(AtLimit { gens: 0, from: p.on_track, to: p.on_track, best_speed_max: None, median_rate: None, best_lap: None });
            e.gens += 1;
            e.to = p.on_track;
            if let Some(s) = p.best_speed {
                e.best_speed_max = Some(e.best_speed_max.map_or(s, |m: f64| m.max(s)));
            }
            if let Some(l) = p.best_lap {
                e.best_lap = Some(e.best_lap.map_or(l, |m: f64| m.min(l)));
            }
        }
        for (k, e) in at_limit.iter_mut() {
            let mut v: Vec<f64> = points.iter().filter(|p| p.time_limit.map(key) == Some(*k)).filter_map(|p| p.median_rate).collect();
            e.median_rate = runs::median(&mut v);
        }
        let rewards: BTreeSet<String> = n.runs.iter().filter(|r| r.track == track && !r.rewards.is_empty()).map(|r| overview::rewards_key(&r.rewards)).collect();
        let still_here = keys.last().map(String::as_str) == Some(track);
        let forecast_100 = if still_here {
            let drift = projection::pooled_drift(lib, Some(&n.file));
            projection::forecast(n, &drift).map(|f| f.at(100))
        } else {
            None
        };
        out.push(Entry {
            file: n.file.clone(),
            vehicle: format::vehicle_name(n.save.vehicle),
            design: n.save.arch.label(),
            rewards,
            population: idx.last().and_then(|&i| g[i].settings.population),
            first_lap: points.iter().find(|p| p.best_lap.is_some()).map(|p| p.on_track),
            best_lap: points.iter().filter_map(|p| p.best_lap).reduce(f64::min),
            forecast_100,
            limits,
            at_limit,
            copied_from: n.fork.parent.filter(|_| idx.first().is_some_and(|&i| i < n.fork.own_from)).map(|(p, _)| lib.networks[p].file.clone()),
            points,
        });
    }
    out.sort_by(|a, b| b.points.len().cmp(&a.points.len()));
    out
}

/// Time limits two or more entries trained at, in order.
pub fn shared_limits(entries: &[Entry]) -> Vec<f64> {
    let mut count: BTreeMap<u64, usize> = BTreeMap::new();
    for e in entries {
        for k in e.at_limit.keys() {
            *count.entry(*k).or_default() += 1;
        }
    }
    count.into_iter().filter(|(_, c)| *c >= 2).map(|(k, _)| k as f64 / 10.0).collect()
}

/// Plain-English list of what differs between the entries (empty when nothing does).
pub fn differences(entries: &[Entry]) -> Vec<String> {
    let mut out = Vec::new();
    let distinct = |f: &dyn Fn(&Entry) -> String| entries.iter().map(f).collect::<BTreeSet<_>>().len() > 1;
    if distinct(&|e| e.vehicle.to_string()) {
        out.push(format!("Car: {}", entries.iter().map(|e| format!("{} {}", e.file, e.vehicle)).collect::<Vec<_>>().join(", ")));
    }
    if distinct(&|e| e.design.clone()) {
        out.push("Network design differs (inputs, layers or outputs), so a gap may be the network, not the car.".into());
    }
    if distinct(&|e| e.rewards.iter().cloned().collect::<Vec<_>>().join(" / ")) {
        let common: BTreeSet<&String> = entries.iter().skip(1).fold(entries[0].rewards.iter().collect(), |acc, e| acc.intersection(&e.rewards.iter().collect()).copied().collect());
        if common.is_empty() {
            out.push("Rewards differ: speed (score per second) isn't comparable between them; lap time still is.".into());
        } else {
            out.push(format!(
                "Rewards differ at times (all used {} at some point): speed compares fairly only where the rewards matched; lap time always does. {}",
                common.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" / "),
                entries.iter().map(|e| format!("{}: {}", e.file, e.rewards.iter().cloned().collect::<Vec<_>>().join(" / "))).collect::<Vec<_>>().join("; ")
            ));
        }
    }
    if distinct(&|e| e.population.map_or("?".into(), |p| p.to_string())) {
        out.push(format!("Population: {}", entries.iter().map(|e| format!("{} {}", e.file, e.population.map_or("?".into(), |p| p.to_string()))).collect::<Vec<_>>().join(", ")));
    }
    let steps = |e: &Entry| e.limits.iter().map(|l| format!("{:.0}", l.1)).collect::<Vec<_>>().join("→");
    if distinct(&|e| steps(e)) {
        out.push("Time-limit steps differ, so compare at matching time limits (table) rather than matching generations alone.".into());
    }
    out
}
