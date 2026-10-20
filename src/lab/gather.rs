//! Suggested data gathering: which experiments would firm up the advice most, how training
//! habits feed (or starve) the evidence, and how accurate the predictions have been so far.
//!
//! Everything here is read-only analysis. Cards describe what to do in the game; nothing is
//! changed for the player.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::compare::{self, Measure, STAGES};
use super::keeper;
use super::library::{Library, Network};
use super::overview::{self, Strength};
use super::projection;
use super::runs;
use super::verdict::{self, Outcome};

/// Generations a clean test needs each side of a change.
fn need(lib: &Library) -> usize {
    lib.min_side.max(1)
}

fn own_networks(lib: &Library) -> impl Iterator<Item = &Arc<Network>> {
    lib.networks.iter().filter(|n| !keeper::is_keeper_copy(&n.file) && n.gens() > n.fork.own_from)
}

// ------------------------------------------------------------------ habits (3)

/// How training habits feed the evidence, counted over the networks' own history.
#[derive(Clone, Debug, Default)]
pub struct Habits {
    /// Settings, reward or track changes.
    pub changes: usize,
    /// Changes that could be judged (10+ gens each side, nothing confounding).
    pub judged: usize,
    /// Changes followed by another change within 10 generations.
    pub too_soon: usize,
    /// Changes made within 5 generations of a time-limit change.
    pub with_time_limit: usize,
    /// Changes that altered more than one setting at once.
    pub several: usize,
    /// Time-limit raises with no other change within 10 generations either side, and laps on both.
    pub isolated_limit_raises: usize,
    pub limit_raises: usize,
    /// Reward changes judged on lap time with no time-limit change within 10 gens.
    pub clean_reward_switches: usize,
    pub reward_switches: usize,
    /// Networks that trained on random tracks and then on one fixed track for 30+ gens.
    pub random_then_fixed: usize,
}

pub fn habits(lib: &Library) -> Habits {
    let mut h = Habits::default();
    let need = need(lib);
    for n in own_networks(lib) {
        let g = &n.save.gens;
        let limits = verdict::time_limits(g);
        let starts: Vec<usize> = n.runs.iter().skip(1).map(|r| r.start).collect();
        let limit_change_near = |at: usize, w: usize| -> bool {
            let lo = at.saturating_sub(w).max(1);
            let hi = (at + w).min(g.len());
            (lo..hi).any(|i| limits[i].is_some() && limits[i - 1].is_some() && limits[i] != limits[i - 1])
        };
        for v in n.verdicts.iter().filter(|v| v.at >= n.fork.own_from) {
            h.changes += 1;
            if v.testable() {
                h.judged += 1;
            }
            if v.n_after < need && v.run + 1 < n.runs.len() {
                h.too_soon += 1;
            }
            if limit_change_near(v.at, 5) {
                h.with_time_limit += 1;
            }
            if v.kind == "several" {
                h.several += 1;
            }
            let (a, b) = (&n.runs[v.run - 1], &n.runs[v.run]);
            if a.rewards != b.rewards && a.track == b.track {
                h.reward_switches += 1;
                if v.by_lap && v.testable() && !limit_change_near(v.at, need) {
                    h.clean_reward_switches += 1;
                }
            }
        }
        for i in n.fork.own_from.max(1)..g.len() {
            if let (Some(a), Some(b)) = (limits[i - 1], limits[i]) {
                if b > a {
                    h.limit_raises += 1;
                    let quiet = starts.iter().all(|&s| s.abs_diff(i) >= need);
                    let alone = (i.saturating_sub(need).max(1)..(i + need).min(g.len()))
                        .filter(|&j| j != i)
                        .all(|j| limits[j].is_none() || limits[j - 1].is_none() || limits[j] == limits[j - 1]);
                    // and laps on both sides, so its effect on lap time can be seen
                    let laps = |r: std::ops::Range<usize>| r.filter(|&j| g[j].best_lap.is_some()).count();
                    let lapped = laps(i.saturating_sub(need)..i) >= need / 2 && laps(i..(i + need).min(g.len())) >= need / 2;
                    if quiet && alone && lapped {
                        h.isolated_limit_raises += 1;
                    }
                }
            }
        }
        let keys = runs::track_keys(g);
        if let Some(first_random) = keys.iter().position(|k| k == runs::RANDOM_TRACK) {
            let fixed_after = keys[first_random..].iter().filter(|k| *k != runs::RANDOM_TRACK && !k.is_empty()).count();
            if fixed_after >= 30 {
                h.random_then_fixed += 1;
            }
        }
    }
    h
}

// ------------------------------------------------------------------ experiment queue (1)

#[derive(Clone, Debug)]
pub struct Card {
    /// Outcome used to rank and judge this experiment.
    pub measure: Measure,
    /// Stable id for the evidence ledger.
    pub id: String,
    pub title: String,
    /// Which advice this would firm up or overturn.
    pub why: String,
    /// What to do in the game.
    pub protocol: String,
    /// What the result would mean.
    pub outcomes: String,
    /// The evidence so far, in words.
    pub evidence: String,
    /// Clean data points now, and the count the next strength level (or a useful answer) needs.
    pub have: usize,
    pub target: usize,
    /// What `have` counts (families per side, agreeing families, or confound-specific counts).
    pub unit: String,
    pub strength: Option<Strength>,
    /// Rough minimum generations of training it takes.
    pub cost_gens: usize,
    /// Can be done on the network being trained right now.
    pub now: bool,
    pub score: f64,
    /// Families with direct A/B results: (for the pick, for the runner-up, within noise).
    /// Each family counts at most once per category; conflicts remain on both sides.
    pub direct: Option<(usize, usize, usize)>,
}

/// Direct A/B results settle a question when this many families agree and none disagree.
pub const DIRECT_TARGET: usize = 3;

/// The measure a stage's questions are judged on: lap time where most runs there have laps,
/// otherwise score per second (early on, networks often can't finish a lap yet).
pub fn stage_measure(lib: &Library, stage: usize) -> Measure {
    let count = |m: Measure| overview::scoped_runs(lib, None, m, need(lib)).iter().filter(|(_, r, _)| compare::stage_index(r.stage) == Some(stage)).count();
    let (lap, rate) = (count(Measure::Lap), count(Measure::Rate));
    if lap >= 10 && lap * 2 >= rate { Measure::Lap } else { Measure::Rate }
}

/// Clean before/after verdicts that switched `dim` between `a` and `b` at `stage`, changing
/// nothing else: (for a, for b, within noise).
fn direct_evidence(lib: &Library, measure: Measure) -> Vec<(&Network, Vec<verdict::Verdict>)> {
    let placebo = own_networks(lib)
        .flat_map(|n| verdict::placebo_z_for_measure(&n.save.gens, &n.runs, n.fork.own_from, lib.window, need(lib), measure))
        .collect();
    let threshold = verdict::threshold(placebo).0;
    own_networks(lib).map(|n| {
        (n.as_ref(), verdict::verdicts_for_measure(&n.save.gens, &n.runs, measure, threshold, lib.window, need(lib)))
    }).collect()
}

fn direct_tests(lib: &Library, evidence: &[(&Network, Vec<verdict::Verdict>)], stage: usize, dim: &str, a: &str, b: &str) -> (usize, usize, usize) {
    let (mut fa, mut fb, mut noise) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for (n, verdicts) in evidence {
        for v in verdicts.iter().filter(|v| v.at >= n.fork.own_from && v.testable() && v.kind != "several") {
            if compare::stage_index(v.stage) != Some(stage) {
                continue;
            }
            let (before, after) = (&n.runs[v.run - 1], &n.runs[v.run]);
            let (Some(lb), Some(la)) = (overview::dim_label(dim, before), overview::dim_label(dim, after)) else { continue };
            let pair = (lb.as_str(), la.as_str());
            if pair != (a, b) && pair != (b, a) {
                continue;
            }
            // the after value won if it helped, the before value if it hurt
            match (&v.outcome, la == a) {
                (Outcome::Helped, true) | (Outcome::Hurt, false) => { fa.insert(overview::family(lib, n)); },
                (Outcome::Helped, false) | (Outcome::Hurt, true) => { fb.insert(overview::family(lib, n)); },
                _ => { noise.insert(overview::family(lib, n)); },
            }
        }
    }
    // A family with conflicting results stays on both sides, preventing false agreement.
    (fa.len(), fb.len(), noise.len())
}

/// Runs each side needs for the next strength level.
fn runs_needed(best: usize, runner: usize, s: Strength) -> (usize, usize, &'static str) {
    match s {
        Strength::Weak | Strength::Fact => (5usize.saturating_sub(best), overview::MIN_RUNS.saturating_sub(runner), "some evidence"),
        Strength::Some => (8usize.saturating_sub(best), 5usize.saturating_sub(runner), "strong"),
        Strength::Strong => (0, 0, "strong"),
    }
}

/// The open questions, most valuable first. `current` is the network being trained (its stage
/// makes matching questions doable now).
pub fn cards(lib: &Library, current: Option<&Network>) -> Vec<Card> {
    let need = need(lib);
    let mut out: Vec<Card> = Vec::new();
    let cur_stage = current.and_then(|n| n.runs.last()).and_then(|r| compare::stage_index(r.stage));
    let cur_name = current.map(|n| n.file.clone());
    let evidence = [(Measure::Lap, direct_evidence(lib, Measure::Lap)), (Measure::Rate, direct_evidence(lib, Measure::Rate))];
    // settings questions from the playbook (all tracks), each stage judged on what it can measure
    let books: [(Measure, [overview::StageCard; compare::NSTAGES]); 2] =
        [(Measure::Lap, overview::playbook(lib, None, Measure::Lap, need)), (Measure::Rate, overview::playbook(lib, None, Measure::Rate, need))];
    for stage_i in 0..compare::NSTAGES {
        let m = stage_measure(lib, stage_i);
        let card = &books.iter().find(|b| b.0 == m).unwrap().1[stage_i];
        for p in card.picks.iter().filter(|p| !p.design) {
            let Some(r) = &p.runner_up else { continue };
            let (kb, kr, next) = runs_needed(p.best.n(), r.n(), p.strength);
            let (da, db, dn) = direct_tests(lib, &evidence.iter().find(|e| e.0 == m).unwrap().1, card.stage, p.dimension, &p.best.label, &r.label);
            // settled: strong across networks, or enough direct tests all pointing one way
            if p.strength == Strength::Strong || (da >= DIRECT_TARGET && db == 0) || (db >= DIRECT_TARGET && da == 0) {
                continue;
            }
            let close = p.best.median() - r.median() < overview::PRACTICAL;
            let more = kb + kr;
            let stage = STAGES[card.stage];
            let now = cur_stage == Some(card.stage);
            let judged = if m == Measure::Lap { "lap time".to_string() } else { "score per second (most networks at this stage aren't lapping yet)".to_string() };
            let title = format!("{stage} stage: is {} {} really better than {}?", p.dimension.to_lowercase(), p.best.label, r.label);
            let why = format!(
                "What's worked best currently picks {} {} at the {stage} stage ({}, judged on {judged}). Every suggestion for this stage that mentions {} rests on it.",
                p.dimension.to_lowercase(),
                p.best.label,
                p.strength.label(),
                p.dimension.to_lowercase()
            );
            let target_runs = if more == 0 { 2 } else { more };
            let protocol = format!(
                "{}Run {} {} for {need}+ generations, then {} for {need}+, changing nothing else ({}, track and time limit the same, and no time-limit change in the {need} gens before the switch either). Judge the change in improvement rate using {judged}. Missing data does not count as a direct test; {DIRECT_TARGET} agreeing families with no opposing result settle it. Repeat on unrelated networks; another fork or run from the same family adds no independent evidence.",
                match (&cur_name, now) {
                    (Some(c), true) => format!("On {c} (it's at the {stage} stage): "),
                    _ => format!("On a network at the {stage} stage: "),
                },
                p.dimension.to_lowercase(),
                r.label,
                p.best.label,
                if p.dimension == "Rewards" { "other settings" } else { "rewards, other settings" }
            );
            let outcomes = if close {
                format!("The two are within {:.0} point per 10 gens today. More unrelated families tell you whether {} matters at all here; if it doesn't, stop tuning it and spend the effort elsewhere.", overview::PRACTICAL, p.dimension.to_lowercase())
            } else {
                format!(
                    "If {} keeps beating {}, the pick becomes {next}. If not, the current advice was luck and the suggestion goes away.",
                    p.best.label, r.label
                )
            };
            let direct = if da + db + dn == 0 {
                format!("No usable direct tests for {} yet; insufficient data is excluded, not counted as within noise.", m.unit())
            } else {
                format!("Families with direct tests: {} better {da}, {} better {db}, within noise {dn}. A family can appear on both sides when its results conflict.", p.best.label, r.label)
            };
            let evidence = format!(
                "Across families ({}): {} {}: median {} per 10 gens ({}); {}: {} ({}); {} beat {} in {:.0}% of family pairs. {direct}",
                if m == Measure::Lap { "lap time" } else { "score per second" },
                p.dimension,
                p.best.label,
                m.fmt(p.best.median()),
                p.best.counts(),
                r.label,
                m.fmt(r.median()),
                r.counts(),
                p.best.label,
                r.label,
                p.beats.unwrap_or(0.5) * 100.0
            );
            let weak = if p.strength == Strength::Weak { 1.0 } else { 0.6 };
            let gap = ((p.best.median() - r.median()).abs() / 2.0).min(2.0);
            // a question that's had direct tests is closer to an answer: a little less to gain
            let score = weak * (1.0 + gap) * if now { 2.0 } else { 1.0 } / (target_runs as f64).sqrt() / (1.0 + 0.25 * (da + db + dn) as f64);
            out.push(Card {
                measure: m,
                id: format!("setting:{stage}:{}:families:{}", overview::dim_id(p.dimension), m.unit()),
                title,
                why,
                protocol,
                outcomes,
                evidence,
                // enough runs already but no clear answer: only direct tests can settle it now
                have: if more == 0 { da.max(db) } else { p.best.n().min(r.n()) },
                target: if more == 0 { DIRECT_TARGET } else { p.best.n().min(r.n()) + kb.max(kr) },
                unit: if more == 0 { "agreeing families".into() } else { "families on the smaller side".into() },
                strength: Some(p.strength),
                cost_gens: target_runs * need,
                now,
                score,
                direct: Some((da, db, dn)),
            });
        }
    }
    // the confounds only habits can fix
    let h = habits(lib);
    out.push(Card {
        measure: Measure::Lap,
        id: "confound:time-limit".into(),
        title: "Does raising the time limit, on its own, speed up lap times?".into(),
        why: "You raise the time limit often, and almost always alongside other changes, so its effect is mixed into every other result (and verdicts on score have to stop at each raise).".into(),
        protocol: format!("Next time you raise the time limit, change nothing else for {need} generations before and after."),
        outcomes: "If laps speed up after a lone raise, the time limit is doing work the tool currently credits to other settings; if not, raises can be timed purely around when the cars reach the limit.".into(),
        evidence: format!("{} of your {} time-limit raises had nothing else change within {need} generations and laps on both sides.", h.isolated_limit_raises, h.limit_raises),
        have: h.isolated_limit_raises,
        target: 10,
        unit: "lone raises".into(),
        strength: None,
        cost_gens: 2 * need,
        now: current.is_some(),
        direct: None,
        score: 1.6 * (1.0 - (h.isolated_limit_raises as f64 / 10.0).min(1.0)),
    });
    out.push(Card {
        measure: Measure::Lap,
        id: "confound:rewards".into(),
        title: "Which reward changes speed up lap times?".into(),
        why: "Reward changes are judged on lap time, but most come within a few generations of another change, so almost none can be judged.".into(),
        protocol: format!("When you change rewards, keep the time limit and every other setting the same, and hold the new rewards for {need}+ generations. Make sure the network was already lapping before the change."),
        outcomes: "Around 30 clean switches would let the tool say which reward changes help at which stage (the analysis behind 'switching to rewards X' suggestions).".into(),
        evidence: format!("{} of {} reward changes on a fixed track were clean (judged on lap time, no time-limit change nearby).", h.clean_reward_switches, h.reward_switches),
        have: h.clean_reward_switches,
        target: 30,
        unit: "clean switches".into(),
        strength: None,
        cost_gens: 2 * need,
        now: current.is_some(),
        direct: None,
        score: 1.3 * (1.0 - (h.clean_reward_switches as f64 / 30.0).min(1.0)),
    });
    out.push(Card {
        measure: Measure::Lap,
        id: "confound:transfer".into(),
        title: "Does training on random tracks first make a network faster on a new track?".into(),
        why: "No network has gone from random tracks to a fixed track, so the tool can't say whether pre-training is worth it.".into(),
        protocol: "Fork a random-track network (Test Random 1, say) onto a track another network learned from scratch, use the same time-limit steps, and train 100+ generations.".into(),
        outcomes: "Compare lap times at the same generation count on that track: faster means pre-training pays; the same means start new networks directly on the target track.".into(),
        evidence: format!("{} networks trained on random tracks and then on a fixed track.", h.random_then_fixed),
        have: h.random_then_fixed,
        target: 1,
        unit: "networks".into(),
        strength: None,
        cost_gens: 100,
        now: false,
        direct: None,
        score: if h.random_then_fixed == 0 { 0.9 } else { 0.0 },
    });
    out.retain(|c| c.have < c.target || c.direct.is_some());
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

// ------------------------------------------------------------------ prediction check (4)

/// How well the lap-time forecast would have done, replayed over past history.
#[derive(Clone, Debug, Default)]
pub struct Backtest {
    pub horizon: usize,
    /// Points checked (network, generation) and the networks they came from.
    pub points: usize,
    pub networks: usize,
    /// Median absolute error, % of lap: the forecast, "no change" (best lap stays as it is),
    /// and the raw curve fit the app used to show.
    pub err_forecast: f64,
    pub err_naive: f64,
    pub err_old: f64,
    /// Mean absolute error of the forecast and of "no change" (big misses count more).
    pub mean_forecast: f64,
    pub mean_naive: f64,
    /// Share of points where the forecast was closer than "no change".
    pub forecast_closer: f64,
    /// Share within 1% of what happened.
    pub within_1pct: f64,
    /// 80% of misses were smaller than this (%).
    pub p80: f64,
    /// Median signed error, % (negative = predicted faster than happened: too optimistic).
    pub bias: f64,
}

/// A copy of `n` cut to its first `t` generations, re-analysed.
fn truncated(n: &Network, t: usize) -> Network {
    let mut save = (*n.save).clone();
    save.gens.truncate(t);
    let runs = runs::segment(&save);
    let mut m = n.clone();
    m.save = Arc::new(save);
    m.runs = runs;
    m
}

/// Replays the forecast at every 20th generation: target = the best lap so far 50 and 100
/// generations later (same track throughout). Each network is forecast with the pooled
/// improvement of the *other* networks, so it never sees its own future.
pub fn backtest(lib: &Library) -> (Vec<Backtest>, Vec<(&'static str, usize, usize)>) {
    const STEP: usize = 20;
    let horizons = [50usize, 100];
    // (forecast err %, naive err %, old err %, signed %)
    let mut rows: Vec<Vec<(f64, f64, f64, f64)>> = vec![Vec::new(); horizons.len()];
    let mut nets: Vec<BTreeSet<String>> = vec![BTreeSet::new(); horizons.len()];
    for n in own_networks(lib) {
        let g = &n.save.gens;
        if !g.iter().any(|x| x.best_lap.is_some()) {
            continue;
        }
        let drift = projection::pooled_drift(lib, Some(&n.file));
        let keys = runs::track_keys(g);
        let mut t = (n.fork.own_from + 40).max(60);
        while t + 50 <= g.len() {
            let track = &keys[t - 1];
            if track == runs::RANDOM_TRACK || track.is_empty() {
                t += STEP;
                continue;
            }
            let cut = truncated(n, t);
            if let Some(f) = projection::forecast(&cut, &drift) {
                let p = &f.projection;
                for (k, &h) in horizons.iter().enumerate() {
                    let at = t - 1 + h;
                    if at >= g.len() || (t..=at).any(|i| keys[i] != *track) {
                        continue;
                    }
                    let Some(actual) = g[..=at].iter().zip(&keys).filter(|(_, kk)| *kk == track).filter_map(|(x, _)| x.best_lap).reduce(f64::min) else { continue };
                    let old = if p.fit.linear || p.plateau.is_none() { f.best + p.per_100.min(0.0) * h as f64 / 100.0 } else { p.fit.predict(at as f64).min(f.best) };
                    let predicted = f.at(h);
                    let e = |x: f64| (x - actual) / actual * 100.0;
                    rows[k].push((e(predicted).abs(), e(f.best).abs(), e(old).abs(), e(predicted)));
                    nets[k].insert(n.file.clone());
                }
            }
            t += STEP;
        }
    }
    let med = |mut v: Vec<f64>| runs::median(&mut v).unwrap_or(f64::NAN);
    let out = horizons
        .iter()
        .enumerate()
        .map(|(k, &h)| {
            let r = &rows[k];
            let n = r.len().max(1) as f64;
            let mut abs: Vec<f64> = r.iter().map(|x| x.0).collect();
            abs.sort_by(f64::total_cmp);
            Backtest {
                horizon: h,
                points: r.len(),
                networks: nets[k].len(),
                err_forecast: med(r.iter().map(|x| x.0).collect()),
                err_naive: med(r.iter().map(|x| x.1).collect()),
                err_old: med(r.iter().map(|x| x.2).collect()),
                mean_forecast: r.iter().map(|x| x.0).sum::<f64>() / n,
                mean_naive: r.iter().map(|x| x.1).sum::<f64>() / n,
                forecast_closer: r.iter().filter(|x| x.0 < x.1 - 1e-12).count() as f64 / n,
                within_1pct: r.iter().filter(|x| x.0 <= 1.0).count() as f64 / n,
                p80: if abs.is_empty() { f64::NAN } else { runs::quantile(&abs, 0.8) },
                bias: med(r.iter().map(|x| x.3).collect()),
            }
        })
        .collect();
    let odds = projection::faster_odds(lib, None).into_iter().map(|(l, k, n, _)| (l, k, n)).collect();
    (out, odds)
}

/// Verdicts that turned out "clear": how many there are, against how many chance alone gives.
pub fn verdict_noise(lib: &Library) -> (usize, usize, f64) {
    let (mut clear, mut testable) = (0usize, 0usize);
    for n in own_networks(lib) {
        for v in n.verdicts.iter().filter(|v| v.at >= n.fork.own_from && v.testable()) {
            testable += 1;
            if matches!(v.outcome, Outcome::Helped | Outcome::Hurt) {
                clear += 1;
            }
        }
    }
    (clear, testable, testable as f64 * 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use super::super::format::Save;

    fn experiment() -> Save {
        let mut save = Save::load(Path::new("tests/fixtures/evolution/Type 2.evolution")).unwrap();
        let template = save.gens[0].clone();
        save.gens = (0usize..40).map(|i| {
            let mut g = template.clone();
            let trend = i.saturating_sub(20) as f64;
            g.hash = i as u64 + 1;
            g.track = Some("test-track".into());
            g.settings.mutation = Some(if i < 20 { 0.1 } else { 0.2 });
            g.settings.time_limit = Some(60.0);
            g.settings.population = Some(100);
            g.time = Some(30.0);
            g.median = Some(100.0 + trend * 10.0);
            g.best_lap = Some(40.0 + trend);
            g.active = Some(50 + trend as u32);
            g
        }).collect();
        save.snapshots.clear();
        save.snapshot_weights.clear();
        save
    }

    fn library(save: Save) -> Library {
        super::super::library::analyse(Path::new("in-memory"), vec![("test.evolution".into(), "test".into(), 1.0, Arc::new(save), None)], 20, 10)
    }

    #[test]
    fn direct_tests_follow_the_question_when_scores_and_laps_disagree() {
        let lib = library(experiment());
        let n = &lib.networks[0];
        assert_eq!(n.verdicts[0].outcome, Outcome::Helped);
        let stage = compare::stage_index(n.runs[0].stage).unwrap();
        let a = overview::dim_label("Mutation", &n.runs[0]).unwrap();
        let b = overview::dim_label("Mutation", &n.runs[1]).unwrap();
        assert_ne!(a, b);
        assert_eq!(direct_tests(&lib, &direct_evidence(&lib, Measure::Lap), stage, "Mutation", &a, &b), (1, 0, 0));
        assert_eq!(direct_tests(&lib, &direct_evidence(&lib, Measure::Rate), stage, "Mutation", &a, &b), (0, 1, 0));
    }

    #[test]
    fn missing_laps_never_fall_back_to_score_or_count_as_noise() {
        let mut save = experiment();
        for g in &mut save.gens[20..] { g.best_lap = None; }
        let lib = library(save);
        let evidence = direct_evidence(&lib, Measure::Lap);
        assert_eq!(evidence[0].1[0].outcome, Outcome::CantTell(verdict::Reason::MissingMeasure(Measure::Lap)));
        assert!(!evidence[0].1[0].testable());
        let n = &lib.networks[0];
        let stage = compare::stage_index(n.runs[0].stage).unwrap();
        let a = overview::dim_label("Mutation", &n.runs[0]).unwrap();
        let b = overview::dim_label("Mutation", &n.runs[1]).unwrap();
        assert_eq!(direct_tests(&lib, &evidence, stage, "Mutation", &a, &b), (0, 0, 0));
    }

    #[test]
    fn rate_uses_elapsed_time_and_survival_uses_active_share() {
        let mut save = experiment();
        for g in &mut save.gens { g.time = g.median.map(|s| s / 10.0); }
        let lib = library(save);
        assert_eq!(direct_evidence(&lib, Measure::Rate)[0].1[0].outcome, Outcome::CantTell(verdict::Reason::WithinNoise));
        assert_eq!(direct_evidence(&lib, Measure::Survival)[0].1[0].outcome, Outcome::Helped);
    }

    fn repeated_runs(families: usize, related: bool) -> Library {
        let mut lib = library(experiment());
        let mut source = (*lib.networks[0]).clone();
        let template = source.runs[0].clone();
        source.runs = (0..13).map(|i| {
            let mut run = template.clone();
            run.start = i * 10;
            run.end = run.start + 10;
            run.settings.mutation = Some(if i < 8 { 0.1 } else { 0.2 });
            run.lap_gain = Some(if i < 8 { 10.0 } else { 0.0 });
            run
        }).collect();
        lib.networks = (0..families).map(|i| {
            let mut n = source.clone();
            n.file = format!("network-{i}");
            if related && i > 0 { n.fork.parent = Some((i - 1, 1)); }
            Arc::new(n)
        }).collect();
        lib
    }

    #[test]
    fn repeated_runs_and_related_networks_cannot_manufacture_strong_evidence() {
        for (networks, related, expected_families, strength) in [
            (1, false, 1, Strength::Weak),
            (8, true, 1, Strength::Weak),
            (5, false, 5, Strength::Some),
            (8, false, 8, Strength::Strong),
        ] {
            let lib = repeated_runs(networks, related);
            let book = overview::playbook(&lib, None, Measure::Lap, 10);
            let pick = book[0].picks.iter().find(|p| p.dimension == "Mutation").unwrap();
            assert_eq!(pick.strength, strength);
            assert_eq!(pick.best.n(), expected_families);
            assert_eq!(pick.best.networks, networks);
            assert_eq!(pick.best.runs, networks * 8);
            assert_eq!(pick.best.median(), 10.0);
        }
    }

    #[test]
    fn repeated_direct_tests_and_forks_count_once_per_family() {
        let mut lib = library(experiment());
        let source = (*lib.networks[0]).clone();
        lib.networks = (0..3).map(|i| {
            let mut n = source.clone();
            n.file = format!("fork-{i}");
            if i > 0 { n.fork.parent = Some((0, 1)); }
            Arc::new(n)
        }).collect();
        let mut evidence = direct_evidence(&lib, Measure::Lap);
        // Repeating an identical result in the same family adds no independent vote.
        let v = evidence[0].1[0].clone();
        evidence[0].1.extend([v.clone(), v.clone()]);
        let stage = compare::stage_index(source.runs[0].stage).unwrap();
        let a = overview::dim_label("Mutation", &source.runs[0]).unwrap();
        let b = overview::dim_label("Mutation", &source.runs[1]).unwrap();
        assert_eq!(direct_tests(&lib, &evidence, stage, "Mutation", &a, &b), (1, 0, 0));
        let mut contrary = v;
        contrary.outcome = Outcome::Helped;
        evidence[0].1.push(contrary);
        assert_eq!(direct_tests(&lib, &evidence, stage, "Mutation", &a, &b), (1, 1, 0));
    }
}
