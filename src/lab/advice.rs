//! Suggestions for a network's next settings, from the evidence the Lab already has: the
//! stage playbook (its track first, then all tracks), whether it's still improving, its own
//! history on the track, and its past verdicts. Suggestions are shown, never applied.

use std::time::{SystemTime, UNIX_EPOCH};

use super::compare::{self, Measure, STAGES};
use super::detectability;
use super::library::{Library, Network};
use super::overview::{self, Strength};
use super::runs::{self, Run};
use super::verdict::{self, Outcome};

/// A save modified within this many seconds counts as "training now".
pub const LIVE_SECONDS: f64 = 15.0 * 60.0;

#[derive(Clone, Debug)]
pub struct Suggestion {
    pub strength: Strength,
    pub text: String,
    pub why: String,
}

#[derive(Clone, Debug)]
pub struct Advice {
    pub file: String,
    pub mtime: f64,
    pub live: bool,
    pub track: String,
    pub track_name: String,
    pub stage: Option<usize>,
    pub run: Run,
    pub since_gain: usize,
    /// Median change per generation over the last 20 generations.
    pub slope: Option<f64>,
    /// Best lap now and 20 generations ago (on this track).
    pub lap: (Option<f64>, Option<f64>),
    pub measure: Measure,
    pub suggestions: Vec<Suggestion>,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The save the game wrote most recently (with history of its own; the Lab's best-lap copies
/// don't count).
pub fn latest(lib: &Library) -> Option<&Network> {
    lib.networks
        .iter()
        .filter(|n| n.gens() > 0 && n.error.is_none() && !super::keeper::is_keeper_copy(&n.file))
        .max_by(|a, b| a.mtime.total_cmp(&b.mtime))
        .map(|n| n.as_ref())
}

/// Value label of one setting (or design feature), as the playbook groups it.
fn label(dim: &str, r: &Run) -> Option<String> {
    overview::dim_label(dim, r)
}


/// Is the network's best lap on its current track safe in a copy?
#[derive(Clone, Debug, PartialEq)]
pub enum Backup {
    /// Another save of this network (a copy or fork) holds a network that laps this track at
    /// least as fast: its last generation is on this track with a lap within `SAME_LAP`.
    Safe { copy: String },
    /// The current network is (still) as fast as the best and no copy holds it: save one now.
    Needed,
    /// The network has got slower since its best and no copy was saved, so the best weights are
    /// gone (unless a snapshot is close).
    Lost { snapshot: Option<usize> },
}

#[derive(Clone, Debug)]
pub struct BestLap {
    pub seconds: f64,
    /// Generation that set it.
    pub at: usize,
    pub track_name: String,
    /// Best lap over the latest 5 generations.
    pub recent: Option<f64>,
    pub backup: Backup,
}

/// Counts as "still as fast as the best" within this fraction (0.1%).
pub const SAME_LAP: f64 = 0.001;

/// The best lap on the network's current track, and whether a copy holds it.
pub fn best_lap(lib: &Library, n: &Network) -> Option<BestLap> {
    let run = n.runs.last()?;
    if run.track == runs::RANDOM_TRACK {
        return None;
    }
    let g = &n.save.gens;
    let keys = runs::track_keys(g);
    let (at, seconds) = g
        .iter()
        .enumerate()
        .filter(|(i, _)| keys[*i] == run.track)
        .filter_map(|(i, x)| x.best_lap.map(|l| (i, l)))
        .min_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))?;
    let recent = g.iter().rev().take(5).filter_map(|x| x.best_lap).reduce(f64::min);
    let mine: Vec<u64> = g.iter().map(|x| x.hash).collect();
    // A copy protects the best lap only if the network it saved is that fast: a save holds the
    // network as of its last generation, so that generation must be on this track with a lap at
    // least as good. Sharing the history that recorded the lap isn't enough: a copy saved after
    // the network got slower keeps the record but not the weights.
    let copy = lib
        .networks
        .iter()
        .filter(|o| o.path != n.path && !o.save.gens.is_empty())
        .filter(|o| {
            let theirs: Vec<u64> = o.save.gens.iter().map(|x| x.hash).collect();
            super::forks::shared_prefix(&mine, &theirs) > 0
        })
        .filter(|o| {
            let k = runs::track_keys(&o.save.gens);
            k.last() == Some(&run.track) && o.save.gens.last().and_then(|x| x.best_lap).is_some_and(|l| l <= seconds * (1.0 + SAME_LAP))
        })
        .max_by(|a, b| a.mtime.total_cmp(&b.mtime))
        .map(|o| o.file.clone());
    let backup = match copy {
        Some(c) => Backup::Safe { copy: c },
        None if recent.is_some_and(|r| r <= seconds * (1.0 + SAME_LAP)) => Backup::Needed,
        None => Backup::Lost {
            snapshot: n.save.snapshots.iter().map(|s| s.0).filter(|&s| s >= at).min(),
        },
    };
    Some(BestLap { seconds, at, track_name: overview::track_name(lib, &run.track).0, recent, backup })
}

pub fn advise(lib: &Library, n: &Network) -> Option<Advice> {
    let run = n.runs.last()?.clone();
    let g = &n.save.gens;
    let stage = compare::stage_index(run.stage);
    let (track_name, _) = overview::track_name(lib, &run.track);
    let keys = runs::track_keys(g);
    let last20: Vec<f64> = g.iter().rev().take(20).filter_map(|x| x.median).collect::<Vec<_>>().into_iter().rev().collect();
    let slope = verdict::ols(&last20).map(|f| f.slope);
    let best_lap_until = |end: usize| -> Option<f64> {
        g[..end].iter().zip(&keys).filter(|(_, k)| **k == run.track).filter_map(|(x, _)| x.best_lap).reduce(f64::min)
    };
    let lap = (best_lap_until(g.len()), best_lap_until(g.len().saturating_sub(20)));
    // lap time is the fair measure on a fixed track once laps exist; score otherwise
    let has_laps = g[run.start..].iter().any(|x| x.best_lap.is_some());
    let measure = if has_laps && run.track != runs::RANDOM_TRACK { Measure::Lap } else { Measure::Rate };
    let mut out: Vec<Suggestion> = Vec::new();
    let min_side = lib.min_side;

    // 0. is the best lap safe? (saving every generation overwrites a network that later regresses)
    if let Some(b) = best_lap(lib, n) {
        match &b.backup {
            Backup::Needed => out.push(Suggestion {
                strength: Strength::Fact,
                text: format!(
                    "Save a copy now: this network is at its best lap on {} ({:.3} s, gen {}) and no copy holds it. In the game, save it under a new name (e.g. \"{} best\") before training on.",
                    b.track_name, b.seconds, b.at, n.file
                ),
                why: "Saving every generation overwrites the network, so if it gets slower later the best one is gone. The Lab recognises the copy, so its history isn't counted twice.".into(),
            }),
            Backup::Lost { snapshot } => out.push(Suggestion {
                strength: Strength::Fact,
                text: format!(
                    "This network has got slower since its best lap on {} ({:.3} s at gen {}, now {}), and no copy was saved.{}",
                    b.track_name,
                    b.seconds,
                    b.at,
                    b.recent.map_or("no laps".into(), |r| format!("{r:.3} s")),
                    match snapshot {
                        Some(sn) => format!(" The save's snapshot from gen {sn} is the closest to it."),
                        None => String::new(),
                    }
                ),
                why: "Next time it sets a best lap, save a copy under a new name.".into(),
            }),
            Backup::Safe { .. } => {}
        }
    }
    // 1. timing: is it time to change anything?
    // A historical stall can predate the latest change. Give that change its observation
    // window before using the stall to recommend another intervention.
    if run.len() < min_side && n.runs.len() > 1 {
        let mut why = format!("A change needs {min_side} generations either side to be judged against noise.");
        if let Some(plan) = detectability::forward_plan(lib, n) {
            let det = detectability::format_forward_advice(&plan);
            if !det.is_empty() {
                why = format!("{why} {det}");
            }
        }
        out.push(Suggestion {
            strength: Strength::Fact,
            text: format!(
                "These settings have only run {} generations. Give them {} more before changing again, so you can tell whether the last change worked.",
                run.len(),
                min_side - run.len()
            ),
            why,
        });
    } else if n.since_gain >= runs::STALL_GENS {
        out.push(Suggestion {
            strength: Strength::Fact,
            text: format!("Stalled for {} generations: no meaningful gain yet, so consider changing something.", n.since_gain),
            why: format!("The median hasn't beaten its best on this track and rewards by 1% in that time. The current settings have run for at least {min_side} generations; the stall may also include earlier settings."),
        });
    } else {
        let lap_note = match lap {
            (Some(a), Some(b)) if a < b - 1e-6 => format!(" Best lap {b:.3} s → {a:.3} s over the last 20 generations."),
            _ => String::new(),
        };
        out.push(Suggestion {
            strength: Strength::Fact,
            text: format!("Still improving: last real gain {} generations ago.{lap_note} No need to change yet.", n.since_gain),
            why: "Changing settings while a network is still gaining makes it hard to tell what helped.".into(),
        });
    }

    // 1b. where is the lap time heading? (the forecast: see projection::forecast)
    let drift = super::projection::pooled_drift(lib, Some(&n.file));
    if let Some(f) = super::projection::forecast(n, &drift) {
        let (a50, a100) = (f.at(50), f.at(100));
        let odds = super::projection::faster_odds(lib, Some(&n.file));
        let spot = odds.iter().find(|o| o.3.contains(&f.since_best)).filter(|o| o.2 >= 5 && f.since_best >= 40);
        out.push(Suggestion {
            strength: Strength::Fact,
            text: match spot {
                Some((_, k, m, _)) => format!(
                    "No new best lap for {} generations. That's not necessarily the end: in your other networks, a lap 0.5%+ faster came within 100 gens {k} of {m} times from that spot, often after a change. Forecast on current form: {a100:.3} s in 100 gens (best {:.3} s).",
                    f.since_best, f.best
                ),
                None => format!("Lap time forecast: about {a50:.3} s in 50 gens and {a100:.3} s in 100 (best now {:.3} s).", f.best),
            },
            why: format!(
                "Half your other networks' typical gain over the same generations ({:.2}% per 100), half this network's own curve shrunk to {:.0}%. Replayed over your history it beats assuming no change most of the time; see Data gathering.",
                drift.at(100.0) * 100.0,
                super::projection::CURVE_SHRINK * 100.0
            ),
        });
    }

    // 2. the playbook for this stage: this track first, then all tracks
    if let Some(st) = stage {
        let track_card = overview::playbook(lib, Some(&run.track), measure, 10)[st].clone();
        let all_card = overview::playbook(lib, None, measure, 10)[st].clone();
        let mut seen: Vec<&'static str> = Vec::new();
        for (card, scope) in [(&track_card, track_name.as_str()), (&all_card, "all tracks")] {
            for p in &card.picks {
                if seen.contains(&p.dimension) || (p.strength == Strength::Weak && scope != "all tracks") {
                    continue;
                }
                let Some(cur) = label(p.dimension, &run) else { continue };
                let cur_choice = p.all.iter().find(|c| c.label == cur);
                let unit = measure.unit();
                let why = format!(
                    "On {scope}, at the {} stage, {} {} gained a median {} {unit} per 10 gens over {} {}{}.{}",
                    STAGES[st],
                    p.dimension.to_lowercase(),
                    p.best.label,
                    measure.fmt(p.best.median()),
                    p.best.n(),
                    p.best.unit,
                    match cur_choice {
                        Some(c) if c.label != p.best.label => format!(", against {} for {} ({} {})", measure.fmt(c.median()), c.label, c.n(), c.unit),
                        _ => String::new(),
                    },
                    p.beats.map_or(String::new(), |b| format!(" It beat the runner-up {:.0}% of the time.", b * 100.0))
                );
                if p.design {
                    // a design can't be changed mid-run: it's advice for the next network
                    if cur != p.best.label && p.strength != Strength::Weak {
                        out.push(Suggestion {
                            strength: p.strength,
                            text: format!("For your next network: {} {} has learned faster at this stage (this one has {}).", p.dimension.to_lowercase(), p.best.label, cur),
                            why: format!("{why} Counted per detected family, so repeated runs and related saves do not multiply the evidence."),
                        });
                        seen.push(p.dimension);
                    }
                    continue;
                }
                if cur == p.best.label {
                    if p.strength != Strength::Weak || p.dimension == "Mutation" {
                        out.push(Suggestion { strength: p.strength, text: format!("Keep {} at {}: it's the best-known choice at this stage.", p.dimension.to_lowercase(), cur), why });
                        seen.push(p.dimension);
                    }
                } else if p.strength != Strength::Weak || p.dimension == "Mutation" {
                    out.push(Suggestion {
                        strength: p.strength,
                        text: format!("Try {} {} (now {}).", p.dimension.to_lowercase(), p.best.label, cur),
                        why,
                    });
                    seen.push(p.dimension);
                }
            }
            if let Some(best) = card.flat {
                out.push(Suggestion {
                    strength: if card.runs >= 15 { Strength::Some } else { Strength::Weak },
                    text: format!(
                        "Mutation changes have barely moved {}-stage networks on {scope} (best median {} per 10 gens). If it stalls, change the rewards, the track or the network rather than the mutation rate.",
                        STAGES[st],
                        measure.fmt(best)
                    ),
                    why: format!("{} runs at this stage.", card.runs),
                });
                break;
            }
        }
    }

    // 3. this network's own history on the track: where did its laps improve fastest?
    let (steps, _) = overview::recipe(n, &run.track, 5);
    let mut prev: Option<f64> = None;
    let mut best_step: Option<(f64, &overview::Step, f64)> = None;
    for s in &steps {
        if let (Some(p), Some(l)) = (prev, s.best_lap) {
            let rate = (p - l) / (s.end - s.start) as f64;
            if rate > 0.0 && best_step.as_ref().is_none_or(|b| rate > b.0) {
                best_step = Some((rate, s, p - l));
            }
        }
        if s.best_lap.is_some() {
            prev = s.best_lap;
        }
    }
    if let Some((_, s, gained)) = best_step {
        let same = s.settings == run.settings;
        out.push(Suggestion {
            strength: Strength::Weak,
            text: if same {
                format!("Your current settings are the ones that cut this network's lap time fastest on {track_name} (gens {}–{}).", s.start, s.end - 1)
            } else {
                format!(
                    "This network's lap time fell fastest on {track_name} at {} (gens {}–{}, {:.3} s faster). Worth going back to if it stalls.",
                    overview::settings_label(&s.settings),
                    s.start,
                    s.end - 1,
                    gained
                )
            },
            why: "From this network's own history only, so it's a hint rather than evidence.".into(),
        });
    }

    // 4. its past clear verdicts
    for v in n.verdicts.iter().filter(|v| matches!(v.outcome, Outcome::Helped | Outcome::Hurt)).rev().take(3) {
        out.push(Suggestion {
            strength: Strength::Fact,
            text: format!("Earlier in this network, {} at gen {} clearly {}.", v.change, v.at, if v.outcome == Outcome::Helped { "helped" } else { "hurt" }),
            why: if v.by_lap {
                format!("Before/after test on lap time: z = {:+.1}, threshold {:.1}.", v.z.unwrap_or(0.0), lib.lap_threshold)
            } else {
                format!("Before/after test: z = {:+.1}, threshold {:.1}.", v.z.unwrap_or(0.0), lib.threshold)
            },
        });
    }

    let order = |s: &Strength| match s {
        Strength::Fact => 1,
        Strength::Strong => 0,
        Strength::Some => 2,
        Strength::Weak => 3,
    };
    // keep the timing note first, then strongest evidence
    // backup prompt (if any) and the timing note stay first, then strongest evidence
    let lead = out.iter().take_while(|s| s.strength == Strength::Fact && (s.text.starts_with("Save a copy") || s.text.starts_with("This network has got slower"))).count() + 1;
    let mut rest = out.split_off(lead.min(out.len()));
    rest.sort_by_key(|s| order(&s.strength));
    out.extend(rest);
    let rest = out;

    Some(Advice {
        file: n.file.clone(),
        mtime: n.mtime,
        live: now() - n.mtime < LIVE_SECONDS,
        track: run.track.clone(),
        track_name,
        stage,
        run,
        since_gain: n.since_gain,
        slope,
        lap,
        measure,
        suggestions: rest,
    })
}
