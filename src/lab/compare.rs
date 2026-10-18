//! Comparisons across all networks: the mutation × stage grid, pooled verdicts, comparable
//! runs and track switches. Shared (forked) history is always counted once.

use std::collections::{BTreeMap, HashMap};

use super::format::Gen;
use super::library::{Library, Network};
use super::runs::{self, Run, BUCKETS, RANDOM_TRACK};
use super::verdict::{Outcome, Reason};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TrackFilter {
    #[default]
    Any,
    Fixed,
    Random,
}

/// What "progress" means when comparing runs.
///
/// The game's score depends on the reward weights (and its "best average speed" is just best
/// score ÷ generation time), and distance isn't recorded at all. Lap time and survival don't
/// depend on the rewards; score per second doesn't depend on the time limit.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Measure {
    /// % lap-time reduction per 10 generations (runs with laps)
    #[default]
    Lap,
    /// Change in the share of cars still driving at the end, points per 10 generations
    Survival,
    /// % change in median score per simulated second per 10 generations
    Rate,
    /// % median score change per 10 generations (depends on the rewards)
    Score,
}

impl Measure {
    pub const ALL: [Measure; 4] = [Measure::Lap, Measure::Score, Measure::Rate, Measure::Survival];
    pub fn label(self) -> &'static str {
        match self {
            Measure::Lap => "Lap time",
            Measure::Survival => "Survival",
            Measure::Rate => "Score/sec",
            Measure::Score => "Score",
        }
    }
    /// Lower-case name used in sentences.
    pub fn unit(self) -> &'static str {
        match self {
            Measure::Lap => "lap time",
            Measure::Survival => "survival",
            Measure::Rate => "score per second",
            Measure::Score => "score",
        }
    }
    pub fn help(self) -> &'static str {
        match self {
            Measure::Lap => "Best lap time, % faster per 10 generations. The fairest measure: seconds mean the same whatever the rewards. Only runs that complete laps count.",
            Measure::Survival => "Share of cars still driving when the generation ends, change in percentage points per 10 generations. Doesn't depend on rewards, but a longer time limit lowers it.",
            Measure::Rate => "Median score ÷ generation time, % change per 10 generations. Raising the time limit doesn't inflate it, and with a distance-only reward it's the median car's average speed.",
            Measure::Score => "Median score, % change per 10 generations. Depends on the reward weights, so it's only fair between runs with similar rewards.",
        }
    }
    /// Format a gain in this measure's unit.
    pub fn fmt(self, v: f64) -> String {
        match self {
            Measure::Survival => format!("{v:+.1} pts"),
            _ if v.abs() >= 100.0 => format!("{v:+.0}%"),
            _ => format!("{v:+.1}%"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filters {
    pub vehicle: Option<u8>,
    pub track: TrackFilter,
    pub selection: Option<u8>,
    pub crossover: Option<u8>,
    pub min_len: usize,
    pub measure: Measure,
}

impl Default for Filters {
    fn default() -> Self {
        Filters { vehicle: None, track: TrackFilter::Any, selection: None, crossover: None, min_len: 10, measure: Measure::Lap }
    }
}

impl Filters {
    fn network_ok(&self, n: &Network) -> bool {
        self.vehicle.is_none_or(|v| n.save.vehicle == Some(v))
    }
    fn run_ok(&self, r: &Run) -> bool {
        let track_ok = match self.track {
            TrackFilter::Any => true,
            TrackFilter::Fixed => r.track != RANDOM_TRACK,
            TrackFilter::Random => r.track == RANDOM_TRACK,
        };
        track_ok
            && self.selection.is_none_or(|s| r.settings.selection == Some(s))
            && self.crossover.is_none_or(|c| r.settings.crossover == Some(c))
    }
    pub fn gain(&self, r: &Run) -> Option<f64> {
        match self.measure {
            Measure::Score => r.score_gain,
            Measure::Lap => r.lap_gain,
            Measure::Survival => r.survival_gain,
            Measure::Rate => r.rate_gain,
        }
    }
}

pub const STAGES: [&str; NSTAGES] = ["early", "mid", "late", "veteran"];
pub const NSTAGES: usize = 4;

pub fn stage_index(s: Option<f64>) -> Option<usize> {
    STAGES.iter().position(|x| *x == runs::stage_label(s))
}

/// Runs that count: the network's own history only, long enough, matching the filters.
pub fn counted_runs<'a>(lib: &'a Library, f: &Filters) -> impl Iterator<Item = (&'a Network, &'a Run)> + 'a {
    let f = f.clone();
    let f2 = f.clone();
    lib.networks.iter().filter(move |n| f.network_ok(n)).flat_map(move |n| {
        let f = f2.clone();
        n.runs.iter().filter(move |r| r.start >= n.fork.own_from && r.len() >= f.min_len && f.run_ok(r)).map(move |r| (n.as_ref(), r))
    })
}

#[derive(Clone, Debug, Default)]
pub struct Cell {
    /// Sorted gains.
    pub gains: Vec<f64>,
}

impl Cell {
    pub fn n(&self) -> usize {
        self.gains.len()
    }
    pub fn median(&self) -> Option<f64> {
        (!self.gains.is_empty()).then(|| runs::quantile(&self.gains, 0.5))
    }
    pub fn q(&self, q: f64) -> f64 {
        runs::quantile(&self.gains, q)
    }
}

/// `grid[bucket][stage]`, plus runs with an unknown mutation rate.
pub fn grid(lib: &Library, f: &Filters) -> (Vec<[Cell; NSTAGES]>, usize) {
    let mut g: Vec<[Cell; NSTAGES]> = (0..BUCKETS.len()).map(|_| Default::default()).collect();
    let mut unknown = 0;
    for (_, r) in counted_runs(lib, f) {
        let Some(gain) = f.gain(r) else { continue };
        match (runs::bucket(r.settings.mutation), stage_index(r.stage)) {
            (Some(b), Some(s)) => g[b][s].gains.push(gain),
            _ => unknown += 1,
        }
    }
    for row in &mut g {
        for c in row.iter_mut() {
            c.gains.sort_by(f64::total_cmp);
        }
    }
    (g, unknown)
}

#[derive(Clone, Debug, Default)]
pub struct Pooled {
    pub testable: usize,
    pub helped: usize,
    pub hurt: usize,
}

impl Pooled {
    /// Clear results expected by chance (5% of testable changes).
    pub fn expected(&self) -> f64 {
        self.testable as f64 * 0.05
    }
}

pub struct PooledVerdicts {
    /// (change kind, stage) -> counts; the last entry is all stages
    pub cells: BTreeMap<&'static str, [Pooled; NSTAGES + 1]>,
    pub total: Pooled,
    /// How many changes couldn't be tested, by reason.
    pub untestable: Vec<(&'static str, usize)>,
    pub changes: usize,
}

pub fn pooled_verdicts(lib: &Library, f: &Filters) -> PooledVerdicts {
    let mut cells: BTreeMap<&'static str, [Pooled; NSTAGES + 1]> = BTreeMap::new();
    let mut total = Pooled::default();
    let mut reasons: HashMap<&'static str, usize> = HashMap::new();
    let mut changes = 0;
    for n in lib.networks.iter().filter(|n| f.network_ok(n)) {
        for v in n.verdicts.iter().filter(|v| v.at >= n.fork.own_from) {
            if !f.run_ok(&n.runs[v.run]) {
                continue;
            }
            changes += 1;
            if !v.testable() {
                let key = match &v.outcome {
                    Outcome::CantTell(Reason::TooShort(..)) => "too few generations",
                    Outcome::CantTell(Reason::RewardsChanged) => "rewards changed, no laps to judge by",
                    Outcome::CantTell(Reason::TrackChanged) => "track changed",
                    Outcome::CantTell(Reason::TimeLimitChanged) => "time limit changed in the window",
                    _ => "scores missing",
                };
                *reasons.entry(key).or_default() += 1;
                continue;
            }
            let e = cells.entry(v.kind).or_default();
            let add = |p: &mut Pooled| {
                p.testable += 1;
                match v.outcome {
                    Outcome::Helped => p.helped += 1,
                    Outcome::Hurt => p.hurt += 1,
                    _ => {}
                }
            };
            if let Some(s) = stage_index(v.stage) {
                add(&mut e[s]);
            }
            add(&mut e[NSTAGES]);
            add(&mut total);
        }
    }
    let mut untestable: Vec<(&'static str, usize)> = reasons.into_iter().collect();
    untestable.sort_by(|a, b| b.1.cmp(&a.1));
    PooledVerdicts { cells, total, untestable, changes }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub label: String,
    pub gains: Vec<f64>,
    pub clear_helped: usize,
    pub testable: usize,
}

/// Past runs in other networks at the same stage, vehicle and reward metrics as `net`'s latest
/// run, grouped by settings. Groups with fewer than `min_n` runs are left out.
pub fn comparable(lib: &Library, net: &Network, min_n: usize) -> (Vec<Group>, Option<&'static str>) {
    let Some(last) = net.runs.last() else { return (Vec::new(), None) };
    let stage = runs::stage_label(last.stage);
    if stage == "?" {
        return (Vec::new(), None);
    }
    let metrics: Vec<(u8, u8)> = last.rewards.iter().map(|r| (r.metric, r.kind)).collect();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for n in lib.networks.iter().filter(|n| n.path != net.path && n.save.vehicle == net.save.vehicle) {
        for (j, r) in n.runs.iter().enumerate() {
            if r.start < n.fork.own_from || r.len() < 10 || r.stage_label() != stage {
                continue;
            }
            if r.rewards.iter().map(|w| (w.metric, w.kind)).collect::<Vec<_>>() != metrics {
                continue;
            }
            let Some(gain) = r.score_gain else { continue };
            let label = format!(
                "mutation {} · {}·{} · {}",
                runs::bucket(r.settings.mutation).map_or("?", |b| BUCKETS[b].0),
                super::format::selection_name(r.settings.selection),
                r.settings.selection_size.map_or("?".into(), |s| s.to_string()),
                super::format::crossover_name(r.settings.crossover)
            );
            let g = groups.entry(label.clone()).or_insert(Group { label, gains: Vec::new(), clear_helped: 0, testable: 0 });
            g.gains.push(gain);
            if let Some(v) = n.verdicts.iter().find(|v| v.run == j) {
                if v.testable() {
                    g.testable += 1;
                    if v.outcome == Outcome::Helped {
                        g.clear_helped += 1;
                    }
                }
            }
        }
    }
    let mut out: Vec<Group> = groups.into_values().filter(|g| g.gains.len() >= min_n).collect();
    for g in &mut out {
        g.gains.sort_by(f64::total_cmp);
    }
    out.sort_by(|a, b| runs::quantile(&b.gains, 0.5).total_cmp(&runs::quantile(&a.gains, 0.5)));
    out.truncate(5);
    (out, Some(stage))
}

#[derive(Clone, Debug)]
pub struct TrackSwitch {
    pub at: usize,
    pub survival: (Option<f64>, Option<f64>),
    pub lap: (Option<f64>, Option<f64>),
    pub n: (usize, usize),
}

fn med(g: &[Gen], f: impl Fn(&Gen) -> Option<f64>) -> Option<f64> {
    let mut v: Vec<f64> = g.iter().filter_map(f).collect();
    if v.len() < 3 {
        return None;
    }
    runs::median(&mut v)
}

/// For each change of track (not random-track training): survival and best lap over the last
/// 10 generations before and the first 10 after.
pub fn track_switches(net: &Network) -> Vec<TrackSwitch> {
    let g = &net.save.gens;
    let keys = runs::track_keys(g);
    let mut out = Vec::new();
    for j in 1..net.runs.len() {
        let (a, b) = (&net.runs[j - 1], &net.runs[j]);
        if a.track == b.track || a.track == RANDOM_TRACK || b.track == RANDOM_TRACK {
            continue;
        }
        // the same track either side, even if other settings changed within those 10 gens
        let mut lo = a.end;
        while lo > 0 && a.end - lo < 10 && keys[lo - 1] == a.track {
            lo -= 1;
        }
        let mut hi = b.start;
        while hi < g.len() && hi - b.start < 10 && keys[hi] == b.track {
            hi += 1;
        }
        let (before, after) = (&g[lo..a.end], &g[b.start..hi]);
        out.push(TrackSwitch {
            at: b.start,
            survival: (med(before, Gen::survival), med(after, Gen::survival)),
            lap: (med(before, |x| x.best_lap), med(after, |x| x.best_lap)),
            n: (before.len(), after.len()),
        });
    }
    out
}
