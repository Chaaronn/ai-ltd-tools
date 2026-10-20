//! The Overview: which settings have worked best, overall and per track, as stage playbooks
//! and plain-English findings, each with how sure it is.

use std::collections::{BTreeMap, HashMap};

use super::compare::{self, Filters, Measure, STAGES};
use super::format::{self, Gen, Settings};
use super::library::{Library, Network};
use super::runs::{self, Run, BUCKETS, RANDOM_TRACK};
use super::verdict::fmt_opt;

/// How much to trust a claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strength {
    /// Measured directly (a best lap, a count): not a comparison.
    Fact,
    Strong,
    Some,
    Weak,
}

impl Strength {
    pub fn label(self) -> &'static str {
        match self {
            Strength::Fact => "fact",
            Strength::Strong => "strong evidence",
            Strength::Some => "some evidence",
            Strength::Weak => "weak: a guess",
        }
    }
}

/// One setting value and how its runs did.
#[derive(Clone, Debug)]
pub struct Choice {
    pub label: String,
    /// Sorted family medians (each network is reduced to a median first).
    pub gains: Vec<f64>,
    /// Evidence unit: "families".
    pub unit: &'static str,
    pub runs: usize,
    pub networks: usize,
}

impl Choice {
    pub fn n(&self) -> usize {
        self.gains.len()
    }
    pub fn median(&self) -> f64 {
        runs::quantile(&self.gains, 0.5)
    }
    pub fn counts(&self) -> String {
        format!("{} families · {} networks · {} runs", self.n(), self.networks, self.runs)
    }
}

/// Root of the detected ancestry tree, including siblings whose ancestor is not saved.
pub fn family<'a>(lib: &'a Library, n: &'a Network) -> &'a str {
    let mut root = n;
    while let Some((parent, _)) = root.fork.parent {
        root = &lib.networks[parent];
    }
    &root.file
}

/// The best value of one setting at one stage.
#[derive(Clone, Debug)]
pub struct Pick {
    pub dimension: &'static str,
    /// A network-design feature, not a training setting.
    pub design: bool,
    pub best: Choice,
    pub runner_up: Option<Choice>,
    /// Share of (best, runner-up) family-median pairs where the best gained more.
    pub beats: Option<f64>,
    pub strength: Strength,
    /// Every value with enough runs, best first.
    pub all: Vec<Choice>,
}

#[derive(Clone, Debug)]
pub struct StageCard {
    pub stage: usize,
    pub runs: usize,
    pub picks: Vec<Pick>,
    /// No mutation rate gained meaningfully (best median below 1% per 10 gens).
    pub flat: Option<f64>,
}

/// Smallest gap in median gain (percentage points per 10 gens) that counts as a difference.
pub const PRACTICAL: f64 = 1.0;

/// Minimum runs for a setting value to be considered.
pub const MIN_RUNS: usize = 3;

/// Probability that a random run from `a` gained more than one from `b` (ties count half).
pub fn beats(a: &[f64], b: &[f64]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.5;
    }
    let mut s = 0.0;
    for x in a {
        for y in b {
            s += if x > y { 1.0 } else if x == y { 0.5 } else { 0.0 };
        }
    }
    s / (a.len() * b.len()) as f64
}

pub fn strength(best: usize, runner: Option<usize>, p: Option<f64>) -> Strength {
    match (runner, p) {
        (Some(r), Some(p)) if best >= 8 && r >= 5 && p >= 0.70 => Strength::Strong,
        (Some(r), Some(p)) if best >= 5 && r >= MIN_RUNS && p >= 0.60 => Strength::Some,
        _ => Strength::Weak,
    }
}

/// A setting (or network-design feature) the playbook compares.
pub struct Dim {
    pub name: &'static str,
    /// Design features require at least two contributing networks to be shown.
    /// All dimensions use family medians for ranking and strength.
    pub per_network: bool,
    /// Only compared by lap time: changing the rewards rescales the score, so score-based gains
    /// can't be compared across reward setups.
    pub lap_only: bool,
    pub key: fn(&Run) -> Option<String>,
}

/// What "Selection size" means (read from the game's code, Oct 2026).
pub const SELECTION_SIZE_NOTE: &str = "Selection size: how many of the top cars are picked as parents to breed the next generation. With the game's \"Preserve parents\" set to keep the selected parents, those cars also carry over unchanged, so it's also the number kept; set to a fixed size, only that many are kept; set to none, no parents are kept. Saves don't record the Preserve parents setting.";

/// Notes shown with a playbook dimension.
pub fn dim_note(name: &str) -> Option<&'static str> {
    match name {
        "Selection size" => Some(SELECTION_SIZE_NOTE),
        "Mutation" => Some("Mutation bands compare the rate as set. With adaptive mutation on, the game scales it by ×0.85 to ×1.0 depending on network size (bigger networks get less), smaller than the gap between bands."),
        _ => None,
    }
}

/// The name a dimension had in ledger ids (kept so the evidence ledger's history carries on).
pub fn dim_id(name: &str) -> &str {
    if name == "Selection size" { "Parents kept" } else { name }
}

pub fn dims() -> [Dim; 10] {
    [
        Dim { name: "Mutation", per_network: false, lap_only: false, key: |r| runs::bucket(r.settings.mutation).map(|b| BUCKETS[b].0.to_string()) },
        Dim { name: "Selection", per_network: false, lap_only: false, key: |r| r.settings.selection.map(|x| format::selection_name(Some(x)).to_string()) },
        Dim {
            name: "Selection size",
            per_network: false,
            lap_only: false,
            key: |r| {
                r.settings.selection_size.map(|n| match n {
                    0..=5 => "≤ 5".to_string(),
                    6..=15 => "6–15".to_string(),
                    16..=30 => "16–30".to_string(),
                    _ => "> 30".to_string(),
                })
            },
        },
        Dim { name: "Rewards", per_network: false, lap_only: true, key: |r| (!r.rewards.is_empty()).then(|| rewards_key(&r.rewards)) },
        Dim { name: "Crossover", per_network: false, lap_only: false, key: |r| r.settings.crossover.map(|x| format::crossover_name(Some(x)).to_string()) },
        Dim {
            name: "Population",
            per_network: false,
            lap_only: false,
            key: |r| {
                r.settings.population.map(|n| match n {
                    0..=600 => "≤ 600".to_string(),
                    601..=1000 => "601–1000".to_string(),
                    _ => "> 1000".to_string(),
                })
            },
        },
        Dim {
            name: "Raycasts",
            per_network: true,
            lap_only: false,
            key: |r| {
                (!r.arch.sensors.is_empty()).then(|| match r.arch.raycasts() {
                    0..=8 => "≤ 8".to_string(),
                    9..=11 => "9–11".to_string(),
                    _ => "12+".to_string(),
                })
            },
        },
        Dim { name: "Outputs", per_network: true, lap_only: false, key: |r| (!r.arch.outputs.is_empty()).then(|| r.arch.outputs_label()) },
        Dim {
            name: "Hidden layers",
            per_network: true,
            lap_only: false,
            key: |r| {
                (r.arch.shape.len() >= 2).then(|| match r.arch.hidden().len() {
                    0..=3 => "≤ 3".to_string(),
                    4 => "4".to_string(),
                    5 => "5".to_string(),
                    _ => "6+".to_string(),
                })
            },
        },
        Dim {
            name: "Network size",
            per_network: true,
            lap_only: false,
            key: |r| {
                (r.arch.shape.len() >= 2).then(|| match r.arch.params() {
                    0..=299 => "< 300 params".to_string(),
                    300..=599 => "300–599 params".to_string(),
                    600..=899 => "600–899 params".to_string(),
                    _ => "900+ params".to_string(),
                })
            },
        },
    ]
}

/// The playbook's label for a run's value of a setting (e.g. "0.13–0.20" for mutation 0.15).
/// Which reward metrics are switched on, ignoring their weights: "Distance + Speed (avg)".
pub fn rewards_key(r: &[format::Reward]) -> String {
    r.iter().map(|w| format!("{}{}", format::metric_name(w.metric), if w.kind == 1 { " (avg)" } else { "" })).collect::<Vec<_>>().join(" + ")
}

pub fn dim_label(dim: &str, r: &Run) -> Option<String> {
    dims().iter().find(|d| d.name == dim).and_then(|d| (d.key)(r))
}

pub fn is_design(dim: &str) -> bool {
    dims().iter().any(|d| d.name == dim && d.per_network)
}

/// Runs counted for the overview: own history, long enough, optionally one track.
pub fn scoped_runs<'a>(lib: &'a Library, track: Option<&'a str>, measure: Measure, min_len: usize) -> Vec<(&'a Network, &'a Run, f64)> {
    let f = Filters { min_len, measure, ..Filters::default() };
    compare::counted_runs(lib, &f)
        .filter(|(_, r)| track.is_none_or(|t| r.track == t))
        .filter_map(|(n, r)| f.gain(r).map(|g| (n, r, g)))
        .collect()
}

/// Stage playbook: for each stage, the best value of each setting.
pub fn playbook(lib: &Library, track: Option<&str>, measure: Measure, min_len: usize) -> [StageCard; compare::NSTAGES] {
    let all = scoped_runs(lib, track, measure, min_len);
    std::array::from_fn(|stage| {
        let rs: Vec<&(&Network, &Run, f64)> = all.iter().filter(|(_, r, _)| compare::stage_index(r.stage) == Some(stage)).collect();
        let mut picks = Vec::new();
        let mut flat = None;
        for d in dims() {
            if d.lap_only && measure != Measure::Lap {
                continue;
            }
            let dim = d.name;
            // Collapse repeated runs within each network, then related networks within a family.
            // Raw counts remain visible, but do not multiply a family's weight or confidence.
            let mut per: BTreeMap<(String, String, String), Vec<f64>> = BTreeMap::new();
            for (n, r, g) in &rs {
                if let Some(k) = (d.key)(r) {
                    per.entry((k, family(lib, n).into(), n.file.clone())).or_default().push(*g);
                }
            }
            let mut grouped: BTreeMap<String, BTreeMap<String, Vec<f64>>> = BTreeMap::new();
            let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
            for ((label, family, _), mut gains) in per {
                let count = counts.entry(label.clone()).or_default();
                count.0 += gains.len();
                count.1 += 1;
                grouped.entry(label).or_default().entry(family).or_default().push(runs::median(&mut gains).unwrap());
            }
            let mut choices = Vec::new();
            for (label, families) in grouped {
                let (raw_runs, networks) = counts[&label];
                // Keep thin comparisons visible as weak evidence instead of hiding the question.
                if (d.per_network && networks < 2) || (!d.per_network && raw_runs < MIN_RUNS) { continue; }
                let mut gains: Vec<f64> = families.into_values().filter_map(|mut g| runs::median(&mut g)).collect();
                gains.sort_by(f64::total_cmp);
                choices.push(Choice { label, gains, unit: "families", runs: raw_runs, networks });
            }
            if choices.is_empty() {
                continue;
            }
            choices.sort_by(|a, b| b.median().total_cmp(&a.median()));
            if dim == "Mutation" && choices.len() >= 2 && choices[0].median() < 1.0 {
                flat = Some(choices[0].median());
            }
            // a setting that never varied here says nothing
            if choices.len() < 2 {
                continue;
            }
            let best = choices[0].clone();
            let runner_up = choices.get(1).cloned();
            let p = runner_up.as_ref().map(|r| beats(&best.gains, &r.gains));
            let mut strength = strength(best.n(), runner_up.as_ref().map(Choice::n), p);
            // a gap under 1 point per 10 gens isn't worth acting on, however consistent
            if runner_up.as_ref().is_some_and(|r| best.median() - r.median() < PRACTICAL) {
                strength = Strength::Weak;
            }
            picks.push(Pick { dimension: dim, design: d.per_network, best, runner_up, beats: p, strength, all: choices });
        }
        StageCard { stage, runs: rs.len(), picks, flat }
    })
}

/// A track that some network trained on.
#[derive(Clone, Debug)]
pub struct TrackInfo {
    pub key: String,
    pub name: String,
    pub named: bool,
    pub gens: usize,
    pub networks: Vec<String>,
    pub runs: usize,
    pub best_lap: Option<BestLap>,
}

#[derive(Clone, Debug)]
pub struct BestLap {
    pub seconds: f64,
    pub network: String,
    pub at: usize,
    pub settings: Settings,
}

pub fn settings_label(s: &Settings) -> String {
    format!(
        "mutation {} · {}·{} · {} · pop {}",
        fmt_opt(s.mutation),
        format::selection_name(s.selection),
        s.selection_size.map_or("?".into(), |x| x.to_string()),
        format::crossover_name(s.crossover),
        s.population.map_or("?".into(), |x| x.to_string())
    )
}

pub fn track_name(lib: &Library, key: &str) -> (String, bool) {
    if key == RANDOM_TRACK {
        return ("Random tracks".into(), true);
    }
    if key.chars().all(|c| c == '0' || c == '-') {
        return ("Unsaved track".into(), false);
    }
    match lib.track_names.get(key) {
        Some(n) => (n.clone(), true),
        None => match lib.track_hints.get(key) {
            Some(net) => (format!("Built-in {} · {net}'s track", &key[..key.len().min(8)]), false),
            None => (format!("Built-in track {}", &key[..key.len().min(8)]), false),
        },
    }
}

/// Every track with training history (own history only), most-trained first.
pub fn tracks(lib: &Library) -> Vec<TrackInfo> {
    let mut map: HashMap<String, TrackInfo> = HashMap::new();
    for n in &lib.networks {
        let keys = runs::track_keys(&n.save.gens);
        for (i, g) in n.save.gens.iter().enumerate().skip(n.fork.own_from) {
            let key = &keys[i];
            if key.is_empty() {
                continue;
            }
            let e = map.entry(key.clone()).or_insert_with(|| {
                let (name, named) = track_name(lib, key);
                TrackInfo { key: key.clone(), name, named, gens: 0, networks: Vec::new(), runs: 0, best_lap: None }
            });
            e.gens += 1;
            if !e.networks.contains(&n.file) {
                e.networks.push(n.file.clone());
            }
            if let Some(l) = lap_of(g) {
                if key != RANDOM_TRACK && e.best_lap.as_ref().is_none_or(|b| l < b.seconds) {
                    e.best_lap = Some(BestLap { seconds: l, network: n.file.clone(), at: i, settings: settings_at(n, i) });
                }
            }
        }
        for r in n.runs.iter().filter(|r| r.start >= n.fork.own_from && r.len() >= 10) {
            if let Some(e) = map.get_mut(&r.track) {
                e.runs += 1;
            }
        }
    }
    let mut v: Vec<TrackInfo> = map.into_values().collect();
    v.sort_by(|a, b| b.gens.cmp(&a.gens));
    v
}

fn lap_of(g: &Gen) -> Option<f64> {
    g.best_lap
}

fn settings_at(n: &Network, i: usize) -> Settings {
    n.runs.iter().find(|r| i >= r.start && i < r.end).map(|r| r.settings.clone()).unwrap_or_else(|| n.save.gens[i].settings.clone())
}

/// The runs that gained most (in scope).
pub fn best_stretches<'a>(lib: &'a Library, track: Option<&'a str>, measure: Measure, min_len: usize, k: usize) -> Vec<(&'a Network, &'a Run, f64)> {
    let mut v = scoped_runs(lib, track, measure, min_len);
    v.sort_by(|a, b| b.2.total_cmp(&a.2));
    v.truncate(k);
    v
}

/// One stretch of unchanged settings in a network's history on one track.
#[derive(Clone, Debug)]
pub struct Step {
    pub start: usize,
    pub end: usize,
    pub settings: Settings,
    /// Best lap reached by the end of this step (on this track).
    pub best_lap: Option<f64>,
}

/// How a network trained on a track: its settings over time (reward tweaks don't split steps).
/// Steps shorter than `min_len` are left out and counted in the second value.
pub fn recipe(n: &Network, track: &str, min_len: usize) -> (Vec<Step>, usize) {
    let mut steps: Vec<Step> = Vec::new();
    let mut best = f64::INFINITY;
    for r in n.runs.iter().filter(|r| r.track == track) {
        for g in &n.save.gens[r.start..r.end] {
            if let Some(l) = g.best_lap {
                best = best.min(l);
            }
        }
        let lap = best.is_finite().then_some(best);
        match steps.last_mut() {
            Some(s) if s.settings == r.settings && s.end == r.start => {
                s.end = r.end;
                s.best_lap = lap;
            }
            _ => steps.push(Step { start: r.start, end: r.end, settings: r.settings.clone(), best_lap: lap }),
        }
    }
    let total = steps.len();
    let kept: Vec<Step> = steps.into_iter().filter(|s| s.end - s.start >= min_len).collect();
    let hidden = total - kept.len();
    (kept, hidden)
}

/// A plain-English conclusion.
#[derive(Clone, Debug)]
pub struct Finding {
    pub strength: Strength,
    pub text: String,
    pub detail: String,
    /// Network and generation to open when clicked.
    pub link: Option<(String, usize)>,
}

fn measure_unit(m: Measure) -> &'static str {
    m.unit()
}


fn pick_finding(prefix: &str, stage: usize, p: &Pick, m: Measure) -> Finding {
    let what = if p.dimension == "Mutation" { format!("mutation {}", p.best.label) } else { format!("{} {}", p.dimension.to_lowercase(), p.best.label) };
    let mut text = format!(
        "{prefix}{} stage: {what} did best ({} {} per 10 gens; {})",
        stage_word(prefix, stage),
        m.fmt(p.best.median()),
        measure_unit(m),
        p.best.counts()
    );
    if let Some(r) = &p.runner_up {
        text.push_str(&format!(", ahead of {} ({}, {})", r.label, m.fmt(r.median()), r.counts()));
    }
    text.push('.');
    let detail = match p.beats {
        Some(b) => format!("A family median with {what} out-gained a {} family median in {:.0}% of pairs. Families are inferred from shared saved history; this is observational evidence.", p.runner_up.as_ref().map_or("", |r| r.label.as_str()), b * 100.0),
        None => String::new(),
    };
    Finding { strength: p.strength, text, detail, link: None }
}

fn stage_word(prefix: &str, stage: usize) -> String {
    if prefix.is_empty() { capitalise(STAGES[stage]) } else { format!("the {}", STAGES[stage]) }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Findings, most reliable first.
pub fn findings(lib: &Library, track: Option<&str>, measure: Measure, min_len: usize) -> Vec<Finding> {
    let mut out = Vec::new();
    let prefix = match track {
        Some(t) => format!("On {}, ", track_name(lib, t).0),
        None => String::new(),
    };
    let cards = playbook(lib, track, measure, min_len);
    for c in &cards {
        for p in &c.picks {
            if c.flat.is_some() && p.dimension == "Mutation" {
                continue;
            }
            if p.strength != Strength::Weak || p.dimension == "Mutation" {
                out.push(pick_finding(&prefix, c.stage, p, measure));
            }
        }
        if let Some(best) = c.flat {
            out.push(Finding {
                strength: if c.runs >= 15 { Strength::Some } else { Strength::Weak },
                text: format!(
                    "{prefix}{} stage: no mutation rate made a real difference (the best median was {} per 10 gens). Changing rewards, the track or the network is more likely to move it.",
                    stage_word(&prefix, c.stage),
                    measure.fmt(best)
                ),
                detail: format!("{} runs at this stage.", c.runs),
                link: None,
            });
        }
    }
    if track.is_none() {
        // pooled before/after verdicts that stand out from chance
        let p = compare::pooled_verdicts(lib, &Filters::default());
        for (kind, cells) in &p.cells {
            let a = &cells[compare::NSTAGES];
            if a.testable < 5 {
                continue;
            }
            let e = a.expected();
            if a.helped >= 2 && a.helped as f64 >= 2.0 * e {
                out.push(Finding {
                    strength: Strength::Some,
                    text: format!("{} changes clearly helped {} times out of {} that could be judged (about {:.1} expected by chance).", capitalise(kind), a.helped, a.testable, e),
                    detail: format!("{} clearly hurt.", a.hurt),
                    link: None,
                });
            }
            if a.hurt >= 2 && a.hurt as f64 >= 2.0 * e {
                out.push(Finding {
                    strength: Strength::Some,
                    text: format!("{} changes clearly hurt {} times out of {} that could be judged (about {:.1} expected by chance).", capitalise(kind), a.hurt, a.testable, e),
                    detail: format!("{} clearly helped.", a.helped),
                    link: None,
                });
            }
        }
        let short = p.untestable.iter().find(|x| x.0 == "too few generations").map_or(0, |x| x.1);
        if p.changes > 0 && short * 2 > p.changes {
            out.push(Finding {
                strength: Strength::Fact,
                text: format!(
                    "{:.0}% of your settings changes came within {} generations of the previous one, too soon to judge. Holding settings longer would show which changes work.",
                    short as f64 * 100.0 / p.changes as f64,
                    lib.min_side
                ),
                detail: format!("{} of {} changes.", short, p.changes),
                link: None,
            });
        }
        let mut stalled: Vec<&Network> = lib.networks.iter().map(|n| n.as_ref()).filter(|n| n.since_gain >= runs::STALL_GENS && n.gens() > n.fork.own_from).collect();
        stalled.sort_by(|a, b| b.since_gain.cmp(&a.since_gain));
        if !stalled.is_empty() {
            out.push(Finding {
                strength: Strength::Fact,
                text: format!(
                    "Stalled right now: {}.",
                    stalled.iter().take(6).map(|n| format!("{} ({} gens without a gain)", n.file, n.since_gain)).collect::<Vec<_>>().join(", ")
                ),
                detail: "No 1% gain in the median on the current track and rewards.".into(),
                link: stalled.first().map(|n| (n.file.clone(), n.gens().saturating_sub(1))),
            });
        }
    }
    // fastest lap (track scope only; the tracks table lists them all)
    for t in tracks(lib).into_iter().filter(|t| track.is_some_and(|k| k == t.key)) {
        if let Some(b) = &t.best_lap {
            out.push(Finding {
                strength: Strength::Fact,
                text: format!("Fastest lap on {}: {:.3} s by {} at gen {}, with {}.", t.name, b.seconds, b.network, b.at, settings_label(&b.settings)),
                detail: format!("{} networks trained here, {} generations.", t.networks.len(), t.gens),
                link: Some((b.network.clone(), b.at)),
            });
        }
    }
    out.sort_by_key(|f| match f.strength {
        Strength::Strong => 0,
        Strength::Some => 1,
        Strength::Fact => 2,
        Strength::Weak => 3,
    });
    out
}

/// The playbook card that applies to a network now: its latest run's stage, on its track if
/// that track has evidence, otherwise across all tracks.
pub fn for_network<'a>(lib: &'a Library, n: &Network, measure: Measure, min_len: usize) -> Option<(StageCard, Option<String>)> {
    let last = n.runs.last()?;
    let stage = compare::stage_index(last.stage)?;
    let track_card = playbook(lib, Some(&last.track), measure, min_len)[stage].clone();
    if track_card.picks.iter().any(|p| p.strength != Strength::Weak) {
        return Some((track_card, Some(track_name(lib, &last.track).0)));
    }
    Some((playbook(lib, None, measure, min_len)[stage].clone(), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_counts_pairs() {
        assert_eq!(beats(&[2.0, 3.0], &[1.0]), 1.0);
        assert_eq!(beats(&[1.0], &[1.0]), 0.5);
        assert!((beats(&[1.0, 3.0], &[2.0]) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn strength_needs_runs_and_separation() {
        assert_eq!(strength(10, Some(10), Some(0.8)), Strength::Strong);
        assert_eq!(strength(6, Some(4), Some(0.65)), Strength::Some);
        assert_eq!(strength(3, Some(3), Some(0.9)), Strength::Weak);
        assert_eq!(strength(20, None, None), Strength::Weak);
    }
}

/// One combination of rewards (which metrics are on; weights ignored) across the library.
#[derive(Clone, Debug)]
pub struct RewardSetup {
    pub label: String,
    /// Runs (own history, 10+ gens) and networks that used it.
    pub runs: usize,
    pub networks: usize,
    pub gens: usize,
    /// Lap-time gain per 10 gens, per stage: the runs' gains (sorted).
    pub gains: [Vec<f64>; compare::NSTAGES],
    /// Fastest lap with this setup (only meaningful on one track): (seconds, network, gen).
    pub best_lap: Option<(f64, String, usize)>,
    /// Generations from starting a track to its first lap, for (network, track) starts that
    /// mostly used this setup before the first lap.
    pub to_first_lap: Vec<usize>,
    /// Starts that mostly used this setup and never reached a lap (with at least 20 gens).
    pub never_lapped: usize,
    /// Switching to this setup, judged on lap time: (helped, hurt, judged).
    pub switched_to: (usize, usize, usize),
}

impl RewardSetup {
    pub fn median_gain(&self, stage: usize) -> Option<f64> {
        (!self.gains[stage].is_empty()).then(|| runs::quantile(&self.gains[stage], 0.5))
    }
    pub fn median_first_lap(&self) -> Option<f64> {
        let mut v: Vec<f64> = self.to_first_lap.iter().map(|&x| x as f64).collect();
        runs::median(&mut v)
    }
}

/// Every reward setup used, most-used first. `track` limits it to one track key.
pub fn reward_setups(lib: &Library, track: Option<&str>, min_len: usize) -> Vec<RewardSetup> {
    let mut out: BTreeMap<String, RewardSetup> = BTreeMap::new();
    let mut nets: BTreeMap<String, std::collections::HashSet<String>> = BTreeMap::new();
    let new = |label: &str| RewardSetup {
        label: label.to_string(),
        runs: 0,
        networks: 0,
        gens: 0,
        gains: Default::default(),
        best_lap: None,
        to_first_lap: Vec::new(),
        never_lapped: 0,
        switched_to: (0, 0, 0),
    };
    let f = Filters { min_len, measure: Measure::Lap, ..Filters::default() };
    for (n, r) in compare::counted_runs(lib, &f).filter(|(_, r)| track.is_none_or(|t| r.track == t)) {
        if r.rewards.is_empty() {
            continue;
        }
        let key = rewards_key(&r.rewards);
        let e = out.entry(key.clone()).or_insert_with(|| new(&key));
        e.runs += 1;
        e.gens += r.len();
        nets.entry(key).or_default().insert(n.file.clone());
        if let (Some(s), Some(g)) = (compare::stage_index(r.stage), f.gain(r)) {
            e.gains[s].push(g);
        }
        if track.is_some() {
            if let Some((i, l)) = n.save.gens[r.start..r.end].iter().enumerate().filter_map(|(i, g)| g.best_lap.map(|l| (i, l))).min_by(|a, b| a.1.total_cmp(&b.1)) {
                if e.best_lap.as_ref().is_none_or(|b| l < b.0) {
                    e.best_lap = Some((l, n.file.clone(), r.start + i));
                }
            }
        }
    }
    // generations to a first lap, per (network, track) start inside the network's own history
    for n in lib.networks.iter().filter(|n| !super::keeper::is_keeper_copy(&n.file)) {
        let g = &n.save.gens;
        let keys = runs::track_keys(g);
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (start, k) in keys.iter().enumerate() {
            if !seen.insert(k.as_str()) || start < n.fork.own_from || track.is_some_and(|t| t != k) {
                continue;
            }
            let on_track: Vec<usize> = (start..g.len()).filter(|&i| keys[i] == *k).collect();
            let first = on_track.iter().position(|&i| g[i].best_lap.is_some());
            let before: &[usize] = match first {
                Some(p) => &on_track[..p.max(1)],
                None if on_track.len() >= 20 => &on_track,
                None => continue,
            };
            // the setup used for most of the generations before the first lap
            let mut count: HashMap<String, usize> = HashMap::new();
            for &i in before {
                let r = n.runs.iter().find(|r| i >= r.start && i < r.end);
                if let Some(r) = r.filter(|r| !r.rewards.is_empty()) {
                    *count.entry(rewards_key(&r.rewards)).or_default() += 1;
                }
            }
            let Some((key, _)) = count.into_iter().max_by_key(|x| x.1) else { continue };
            let e = out.entry(key.clone()).or_insert_with(|| new(&key));
            match first {
                Some(p) => e.to_first_lap.push(p),
                None => e.never_lapped += 1,
            }
        }
        // reward switches judged on lap time
        for v in n.verdicts.iter().filter(|v| v.by_lap && v.at >= n.fork.own_from) {
            let r = &n.runs[v.run];
            if track.is_some_and(|t| r.track != t) {
                continue;
            }
            let e = out.entry(rewards_key(&r.rewards)).or_insert_with(|| new(&rewards_key(&r.rewards)));
            e.switched_to.2 += 1;
            match v.outcome {
                super::verdict::Outcome::Helped => e.switched_to.0 += 1,
                super::verdict::Outcome::Hurt => e.switched_to.1 += 1,
                _ => {}
            }
        }
    }
    let mut v: Vec<RewardSetup> = out
        .into_values()
        .map(|mut s| {
            s.networks = nets.get(&s.label).map_or(0, |x| x.len());
            s.gains.iter_mut().for_each(|g| g.sort_by(f64::total_cmp));
            s
        })
        .filter(|s| s.runs > 0 || !s.to_first_lap.is_empty() || s.never_lapped > 0)
        .collect();
    v.sort_by(|a, b| b.runs.cmp(&a.runs).then(b.gens.cmp(&a.gens)));
    v
}
