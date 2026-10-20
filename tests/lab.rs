//! Evolution Lab tests against real saves (tests/fixtures/evolution).
//! Set LAB_SAVES to a folder of saves to also check the whole library (e.g. the game's folder).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use track_builder::lab::format::Save;
use track_builder::lab::library::{self, Library};
use track_builder::lab::verdict::Fit;
use track_builder::lab::{advice, compare, designs, detectability, overview, runs, verdict};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/evolution")
}

/// Load the fixtures with fixed mtimes (copying files resets them).
fn fixture_library() -> Library {
    let order = [("Type 2", 1.0), ("Type 2.1", 2.0), ("Type 2.2", 3.0), ("Blank Mk1", 4.0), ("Faster again 3", 5.0)];
    let loaded = order
        .iter()
        .map(|&(name, t)| {
            let p = fixtures().join(format!("{name}.evolution"));
            let s = Save::load(&p).unwrap();
            (p, name.to_string(), t, Arc::new(s), None)
        })
        .collect();
    library::analyse(&fixtures(), loaded, verdict::WINDOW, verdict::MIN_SIDE)
}

#[test]
fn parses_saves() {
    let lib = fixture_library();
    let g = |n: &str| lib.by_file(n).unwrap().gens();
    assert_eq!(g("Type 2"), 289);
    assert_eq!(g("Type 2.1"), 322);
    assert_eq!(g("Type 2.2"), 604);
    assert_eq!(g("Blank Mk1"), 0);
    assert_eq!(g("Faster again 3"), 1311);
    let f = lib.by_file("Faster again 3").unwrap();
    assert_eq!(f.save.vehicle, Some(0));
    assert_eq!(f.save.shape, vec![16, 5, 5, 5, 4, 3, 5]);
}

#[test]
fn finds_forks() {
    let lib = fixture_library();
    let idx = |n: &str| lib.networks.iter().position(|x| x.file == n).unwrap();
    let t2 = idx("Type 2");
    for fork in ["Type 2.1", "Type 2.2"] {
        let n = lib.by_file(fork).unwrap();
        assert_eq!(n.fork.parent, Some((t2, 289)), "{fork}");
        assert!(n.fork.parent_is_prefix);
        assert_eq!(n.fork.own_from, 289);
    }
    assert_eq!(lib.by_file("Type 2").unwrap().fork.parent, None);
    assert_eq!(lib.by_file("Faster again 3").unwrap().fork.parent, None);
}

#[test]
fn runs_match_the_spec_example() {
    let lib = fixture_library();
    let n = lib.by_file("Faster again 3").unwrap();
    let long: Vec<(usize, usize, f64)> =
        n.runs.iter().filter(|r| r.len() >= 15).map(|r| (r.start, r.len(), r.settings.mutation.unwrap())).collect();
    // reward-weight tweaks start a new run (spec §5), e.g. at gen 21 and 983
    let expect = [(0, 21, 0.2), (29, 77, 0.13), (506, 90, 0.09), (655, 305, 0.2), (1130, 78, 0.005), (1208, 103, 0.0005)];
    for (s, l, m) in expect {
        assert!(long.iter().any(|&(a, b, c)| a == s && b == l && (c - m).abs() < 1e-6), "missing run {s}+{l} at {m}: {long:?}");
    }
    // runs tile the history
    assert_eq!(n.runs.first().unwrap().start, 0);
    assert_eq!(n.runs.last().unwrap().end, 1311);
    assert!(n.runs.windows(2).all(|w| w[0].end == w[1].start));
    assert_eq!(n.verdicts.len(), n.runs.len() - 1);
}

#[test]
fn verdicts_are_calibrated() {
    let lib = fixture_library();
    assert!(lib.threshold > 2.0, "placebo threshold {}", lib.threshold);
    // Type 2's mutation drop at gen 120 once looked like the clearest win in the data, but the
    // time limit went 16 → 18 → 30 → 35 → 38 s straight after it, so the score jump can't be
    // credited to the mutation change
    let n = lib.by_file("Type 2").unwrap();
    let limits = verdict::time_limits(&n.save.gens);
    assert_eq!((limits[120], limits[121], limits[125]), (Some(16.0), Some(18.0), Some(30.0)));
    let v = n.verdicts.iter().find(|v| v.at == 120).expect("change at 120");
    assert_eq!(v.kind, "mutation ↓");
    assert!(
        matches!(v.outcome, verdict::Outcome::CantTell(verdict::Reason::TooShort(..) | verdict::Reason::TimeLimitChanged)),
        "{v:?}"
    );
    // shared history is pooled once: Type 2.1 / 2.2 changes before gen 289 are skipped
    let p = compare::pooled_verdicts(&lib, &compare::Filters::default());
    let all: usize = lib.networks.iter().map(|n| n.verdicts.len()).sum();
    assert!(p.changes < all);
}

#[test]
fn empty_save_is_fine() {
    let lib = fixture_library();
    let n = lib.by_file("Blank Mk1").unwrap();
    assert!(n.runs.is_empty() && n.verdicts.is_empty() && n.stalls.is_empty() && n.error.is_none());
}

#[test]
fn comparison_grid_counts_runs() {
    let lib = fixture_library();
    for m in compare::Measure::ALL {
        let f = compare::Filters { measure: m, ..compare::Filters::default() };
        let (grid, _) = compare::grid(&lib, &f);
        let n: usize = grid.iter().flat_map(|r| r.iter()).map(|c| c.n()).sum();
        let direct = compare::counted_runs(&lib, &f)
            .filter(|(_, r)| f.gain(r).is_some() && r.stage.is_some() && runs::bucket(r.settings.mutation).is_some())
            .count();
        assert_eq!(n, direct, "{m:?}");
        assert!(n > 10, "{m:?}: {n} runs");
    }
    // lap time is the default: it doesn't depend on the rewards
    assert_eq!(compare::Filters::default().measure, compare::Measure::Lap);
}

/// Evaluate-only: the Lab code must not contain anything that writes to disk.
#[test]
fn lab_never_writes_files() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![root.join("gui_lab.rs")];
    for e in std::fs::read_dir(root.join("lab")).unwrap() {
        files.push(e.unwrap().path());
    }
    let banned = [
        "File::create", "fs::write", "OpenOptions", "create_dir", "fs::rename", "remove_file", "remove_dir", "fs::copy",
        "set_permissions", "hard_link", "symlink", "save_file", ".save(",
    ];
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        // the keeper is the one fenced exception: it writes only "(best lap)" copies
        if f.file_name().is_some_and(|n| n == "keeper.rs") {
            assert_eq!(src.matches("std::fs::write(").count(), 1, "keeper writes in exactly one place");
            assert_eq!(src.matches("std::fs::rename(").count(), 1);
            for b in ["remove_file", "remove_dir", "fs::copy", "OpenOptions", "File::create"] {
                assert!(!src.contains(b), "keeper.rs contains `{b}`");
            }
            assert!(src.contains("refusing to write"), "keeper must check the destination name");
            continue;
        }
        // track names and the evidence ledger: one settings file each in the tool's own folder
        if f.file_name().is_some_and(|n| n == "names.rs" || n == "ledger.rs") {
            assert_eq!(src.matches("std::fs::write(").count(), 1);
            assert_eq!(src.matches("std::fs::rename(").count(), 1);
            assert_eq!(src.matches("create_dir_all(").count(), 1);
            for b in ["remove_file", "remove_dir", "fs::copy", "OpenOptions", "File::create"] {
                assert!(!src.contains(b), "names.rs contains `{b}`");
            }
            assert!(src.contains("refusing to write inside the game folder"), "{} must refuse the game folder", f.display());
            continue;
        }
        for b in banned {
            assert!(!src.contains(b), "{} contains `{b}`", f.display());
        }
    }
}

/// The keeper copies a save under "(best lap)" with a new name and id, and the Lab then treats
/// it as a copy of its source.
#[test]
fn keeper_writes_a_valid_copy() {
    use track_builder::lab::keeper;
    let dir = std::env::temp_dir().join(format!("tb-keeper-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("Type 2.evolution");
    std::fs::copy(fixtures().join("Type 2.evolution"), &src).unwrap();
    let dest = keeper::keeper_path(&dir, "Type 2", "Track A").unwrap();
    assert!(keeper::write_copy(&src, &dest, 1).is_err(), "refuses when the save changed since it was read");
    keeper::write_copy(&src, &dest, 289).unwrap();
    let orig = Save::load(&src).unwrap();
    let copy = Save::load(&dest).unwrap();
    assert_eq!(copy.name, "Type 2 (best lap · Track A)");
    assert_ne!(copy.id, orig.id);
    assert_eq!(copy.id.split('-').nth(2).unwrap().chars().next(), Some('4'), "a version-4 id, so not modded");
    assert_eq!(copy.gens.len(), orig.gens.len());
    assert!(copy.gens.iter().zip(&orig.gens).all(|(a, b)| a.hash == b.hash), "history identical");
    assert_eq!(copy.arch.params(), orig.arch.params());
    let lib = library::scan(&dir, None, verdict::WINDOW, verdict::MIN_SIDE);
    let c = lib.by_file("Type 2 (best lap · Track A)").unwrap();
    let parent = c.fork.parent.map(|(p, _)| lib.networks[p].file.clone());
    assert_eq!(parent.as_deref(), Some("Type 2"), "the copy hangs off its source");
    assert_eq!(lib.by_file("Type 2").unwrap().fork.parent, None, "the source is never a fork of its copy");
    std::fs::remove_dir_all(&dir).ok();
}

/// Whole-library check, when LAB_SAVES points at a folder of saves.
#[test]
fn whole_library_if_available() {
    let Ok(dir) = std::env::var("LAB_SAVES") else { return };
    let lib = library::scan(Path::new(&dir), None, verdict::WINDOW, verdict::MIN_SIDE);
    for n in &lib.networks {
        assert!(n.error.is_none(), "{}: {:?}", n.file, n.error);
    }
    let families: Vec<(String, String, usize)> = lib
        .networks
        .iter()
        .filter_map(|n| n.fork.parent.map(|(p, k)| (n.file.clone(), lib.networks[p].file.clone(), k)))
        .collect();
    eprintln!("{} saves, threshold {:.2} from {} placebo points", lib.networks.len(), lib.threshold, lib.placebo_points);
    for f in &families {
        eprintln!("  fork {} <- {} at {}", f.0, f.1, f.2);
    }
    let p = compare::pooled_verdicts(&lib, &compare::Filters::default());
    eprintln!("  changes {}, testable {}, helped {}, hurt {}, untestable {:?}", p.changes, p.total.testable, p.total.helped, p.total.hurt, p.untestable);
    for n in &lib.networks {
        for v in n.verdicts.iter().filter(|v| v.at > n.fork.own_from && matches!(v.outcome, verdict::Outcome::Helped | verdict::Outcome::Hurt)) {
            eprintln!("  {} gen {} {} {:?} z={:.1}", n.file, v.at, v.kind, v.outcome, v.z.unwrap());
        }
    }
}

#[test]
fn overview_playbook_and_findings() {
    let lib = fixture_library();
    let cards = overview::playbook(&lib, None, compare::Measure::Score, 10);
    let total: usize = cards.iter().map(|c| c.runs).sum();
    assert!(total > 10, "{total} runs in the playbook");
    for c in &cards {
        for p in &c.picks {
            // best first, and every value shown has enough runs
            assert!(p.all.windows(2).all(|w| w[0].median() >= w[1].median()));
            assert!(p.all.iter().all(|ch| if p.design { ch.networks >= 2 } else { ch.runs >= overview::MIN_RUNS }));
            assert!(p.all.iter().all(|ch| ch.n() <= ch.networks && ch.networks <= ch.runs));
            if let Some(r) = &p.runner_up {
                if p.best.median() - r.median() < overview::PRACTICAL {
                    assert_eq!(p.strength, overview::Strength::Weak);
                }
            }
        }
    }
    let tracks = overview::tracks(&lib);
    assert!(!tracks.is_empty());
    // each fixed track with a lap knows who set it
    let with_lap = tracks.iter().filter(|t| t.best_lap.is_some()).count();
    assert!(with_lap > 0);
    let fs = overview::findings(&lib, None, compare::Measure::Score, 10);
    assert!(!fs.is_empty());
    // the recipe of the fastest network covers its history on that track in order
    let t = tracks.iter().find(|t| t.best_lap.is_some()).unwrap();
    let b = t.best_lap.as_ref().unwrap();
    let n = lib.by_file(&b.network).unwrap();
    let (steps, _) = overview::recipe(n, &t.key, 1);
    assert!(steps.windows(2).all(|w| w[0].end <= w[1].start));
    assert!(steps.iter().any(|s| s.start <= b.at && b.at < s.end));
}

#[test]
fn advice_for_the_latest_network() {
    let lib = fixture_library();
    // "Faster again 3" has the newest fixture mtime
    let n = advice::latest(&lib).unwrap();
    assert_eq!(n.file, "Faster again 3");
    let a = advice::advise(&lib, n).unwrap();
    assert!(!a.suggestions.is_empty());
    // a backup prompt may come first; then the timing note (stalled / too soon / still improving)
    let first = &a.suggestions.iter().find(|s| !s.text.starts_with("Save a copy") && !s.text.starts_with("This network has got slower")).unwrap().text;
    assert!(first.starts_with("Stalled") || first.starts_with("These settings") || first.starts_with("Still improving"), "{first}");
    // on a fixed track with laps, advice is judged by lap time and the stage comes from laps
    assert_eq!(a.measure, compare::Measure::Lap);
    assert_eq!(a.stage, Some(2), "Faster again 3's last run starts after 1,000 generations");
    // fixtures have mtimes in 1970, so nothing is "live"
    assert!(!a.live);
}

#[test]
fn network_designs() {
    let lib = fixture_library();
    let f = lib.by_file("Faster again 3").unwrap();
    let a = &f.save.arch;
    assert_eq!(a.shape, vec![16, 5, 5, 5, 4, 3, 5]);
    assert_eq!(a.params(), 204, "the game's TotalParams");
    assert_eq!(a.raycasts(), 9);
    assert_eq!(a.sensors.len(), 16);
    assert_eq!(a.outputs_label(), "Steer + Accel + Handbrake + Brake + Boost");
    assert_eq!(a.hidden_label(), "5-5-5-4-3");
    // every run knows its design
    assert!(f.runs.iter().all(|r| r.arch.shape.len() >= 2));
    // design picks are counted per detected family
    for c in overview::playbook(&lib, None, compare::Measure::Score, 10) {
        for p in c.picks.iter().filter(|p| p.design) {
            assert!(p.all.iter().all(|ch| ch.unit == "families" && ch.n() <= ch.networks && ch.networks <= ch.runs));
        }
    }
    let sums = designs::summaries(&lib, compare::Measure::Score);
    assert!(sums.iter().any(|s| s.file == "Faster again 3"));
    // the Type 2 family shares a design, Faster again 3 differs: features compare them
    let feats = designs::features(&lib, compare::Measure::Score);
    assert!(feats.iter().all(|f| !f.with_nets.is_empty() && !f.without_nets.is_empty()));
}

#[test]
fn recent_settings_change_takes_priority_over_an_old_stall() {
    for min_side in [10, 15] {
        for elapsed in [1, min_side - 1, min_side] {
            let mut save = Save::load(&fixtures().join("Type 2.evolution")).unwrap();
            let template = save.gens[0].clone();
            save.gens = (0..60 + elapsed).map(|i| {
                let mut g = template.clone();
                g.hash = i as u64 + 1;
                g.track = Some("timing-test".into());
                g.median = Some(100.0);
                g.time = Some(30.0);
                g.best_lap = None;
                g.settings.time_limit = Some(30.0);
                g.settings.mutation = Some(if i < 60 { 0.1 } else { 0.2 });
                g
            }).collect();
            save.snapshots.clear();
            save.snapshot_weights.clear();
            let lib = library::analyse(Path::new("in-memory"), vec![("timing.evolution".into(), "timing".into(), 1.0, Arc::new(save), None)], 20, min_side);
            let n = &lib.networks[0];
            assert!(n.since_gain >= runs::STALL_GENS);
            assert_eq!(n.runs.last().unwrap().len(), elapsed);
            let advice = advice::advise(&lib, n).unwrap();
            let timing = &advice.suggestions[0];
            if elapsed < min_side {
                assert!(timing.text.starts_with("These settings"), "{}", timing.text);
                assert!(timing.text.contains(&format!("Give them {} more", min_side - elapsed)));
                assert!(!advice.suggestions.iter().any(|s| s.text.starts_with("Stalled")));
            } else {
                assert!(timing.text.starts_with("Stalled"), "{}", timing.text);
            }
        }
    }
}

#[test]
fn best_lap_backup_status() {
    let lib = fixture_library();
    // Type 2.1 and 2.2 share all of Type 2's history but went on to other tracks: they keep the
    // record of Type 2's best lap, not its weights, so they don't protect it
    let t2 = advice::best_lap(&lib, lib.by_file("Type 2").unwrap()).expect("Type 2 laps");
    assert!(!matches!(t2.backup, advice::Backup::Safe { .. }), "{:?}", t2.backup);
    // Faster again 3 has no copies: either save one now, or it has slowed since its best
    let f = advice::best_lap(&lib, lib.by_file("Faster again 3").unwrap()).unwrap();
    assert!(matches!(f.backup, advice::Backup::Needed | advice::Backup::Lost { .. }));
    assert_eq!(f.seconds, 40.162, "best lap to the game's 3 decimals");
}

#[test]
fn stage_follows_generations_trained() {
    assert_eq!(runs::stage_label(Some(runs::stage_of(0))), "early");
    assert_eq!(runs::stage_label(Some(runs::stage_of(199))), "early");
    assert_eq!(runs::stage_label(Some(runs::stage_of(200))), "mid");
    assert_eq!(runs::stage_label(Some(runs::stage_of(999))), "mid");
    assert_eq!(runs::stage_label(Some(runs::stage_of(1000))), "late");
    assert_eq!(runs::stage_label(Some(runs::stage_of(4999))), "late");
    assert_eq!(runs::stage_label(Some(runs::stage_of(5000))), "veteran");
    assert!(runs::stage_of(20000) <= 1.0 && runs::stage_of(100_000) <= 1.0);
    let lib = fixture_library();
    for r in &lib.by_file("Type 2.2").unwrap().runs {
        assert_eq!(r.stage_label(), if r.start < 200 { "early" } else if r.start < 1000 { "mid" } else if r.start < 5000 { "late" } else { "veteran" });
    }
}

#[test]
fn still_learning_diagnosis() {
    use track_builder::lab::format::Save;
    use track_builder::lab::netviz::{self, Diagnosis};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/evolution");
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let save = Save::load(&e.path()).unwrap();
        let Some(l) = netviz::learning(&save) else { continue };
        let track = track_builder::lab::runs::track_keys(&save.gens).last().cloned().unwrap_or_default();
        let series = netviz::snapshot_series(&save, &track);
        assert!(series.len() >= 2);
        assert_eq!(series.last().unwrap().at, save.gens.len());
        assert!(series[0].change.is_none());
        // best lap so far never gets slower
        let laps: Vec<f64> = series.iter().filter_map(|p| p.best_lap).collect();
        assert!(laps.windows(2).all(|w| w[1] <= w[0] + 1e-9), "{laps:?}");
        let w = save.weights.as_ref().unwrap();
        let hidden: usize = w.shape().iter().skip(1).rev().skip(1).sum();
        assert!(netviz::idle_neurons(w).len() <= hidden);
        let then = &save.snapshot_weights.iter().find(|(g, _)| *g == l.from_gen).unwrap().1;
        assert_eq!(netviz::output_change(then, w).unwrap().len(), *w.shape().last().unwrap());
        // the four combinations
        assert_eq!(netviz::diagnose(&l, 0) == Diagnosis::Learning || netviz::diagnose(&l, 0) == Diagnosis::FineTuning, true);
        assert!(matches!(netviz::diagnose(&l, 1000), Diagnosis::Drifting | Diagnosis::Stuck));
    }
}

#[test]
fn reward_changes_are_judged_on_lap_time() {
    use track_builder::lab::verdict::{Outcome, Reason};
    let dir = std::env::var("LAB_SAVES").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/evolution").to_string());
    let lib = library::scan(Path::new(&dir), None, verdict::WINDOW, verdict::MIN_SIDE);
    assert!(lib.lap_threshold > 1.0);
    let (mut judged, mut no_laps, mut short) = (0, 0, 0);
    for n in &lib.networks {
        for v in n.verdicts.iter().filter(|v| v.at >= n.fork.own_from) {
            let rewards = n.runs[v.run - 1].rewards != n.runs[v.run].rewards && n.runs[v.run - 1].track == n.runs[v.run].track;
            // only reward changes (on the same track) are judged by lap
            assert_eq!(v.by_lap, rewards && v.z.is_some(), "{} gen {}", n.file, v.at);
            if rewards {
                match &v.outcome {
                    Outcome::CantTell(Reason::RewardsChanged) => no_laps += 1,
                    Outcome::CantTell(Reason::TooShort(..)) => short += 1,
                    _ => {
                        judged += 1;
                        if std::env::var("LAB_SAVES").is_ok() {
                            eprintln!("  {} gen {}: {} ({:+.1})", n.file, v.at, v.label(), v.z.unwrap_or(0.0));
                        }
                    }
                }
            }
        }
    }
    eprintln!("reward changes: {judged} judged on lap time, {no_laps} without laps, {short} too short; lap threshold {:.2}", lib.lap_threshold);
}

#[test]
fn reward_setups_add_up() {
    use track_builder::lab::overview;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/evolution");
    let lib = library::scan(Path::new(dir), None, verdict::WINDOW, verdict::MIN_SIDE);
    let setups = overview::reward_setups(&lib, None, 10);
    let f = compare::Filters { min_len: 10, measure: compare::Measure::Lap, ..compare::Filters::default() };
    let runs = compare::counted_runs(&lib, &f).filter(|(_, r)| !r.rewards.is_empty()).count();
    assert_eq!(setups.iter().map(|s| s.runs).sum::<usize>(), runs);
    for s in &setups {
        assert!(s.networks <= s.runs.max(1) || s.runs == 0);
        assert!(s.switched_to.0 + s.switched_to.1 <= s.switched_to.2);
    }
    // labels are unique
    let mut labels: Vec<&str> = setups.iter().map(|s| s.label.as_str()).collect();
    labels.sort();
    labels.dedup();
    assert_eq!(labels.len(), setups.len());
}

#[test]
fn track_names_round_trip_and_stay_out_of_the_game_folder() {
    use track_builder::lab::names;
    let dir = std::env::temp_dir().join(format!("tb-names-{}", std::process::id()));
    let p = dir.join("track-names.json");
    let mut m = std::collections::HashMap::new();
    m.insert("c75c00e2-cf27-4848-a6f0-f71a275d9f55".to_string(), "  Monza  ".to_string());
    m.insert("blank".to_string(), " ".to_string());
    names::save_to(&p, &m).unwrap();
    let back = names::load_from(&p);
    assert_eq!(back.len(), 1);
    assert_eq!(back["c75c00e2-cf27-4848-a6f0-f71a275d9f55"], "Monza");
    assert!(names::save_to(&dir.join("AI Learns To Drive").join("x.json"), &m).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

/// Faster again 3 truncated just after the reward-change run at 1208 starts (lap-path pending hold).
fn truncated_pending_library() -> Library {
    let order = [("Type 2", 1.0), ("Type 2.1", 2.0), ("Type 2.2", 3.0), ("Blank Mk1", 4.0), ("Faster again 3", 5.0)];
    let loaded = order
        .iter()
        .map(|&(name, t)| {
            let p = fixtures().join(format!("{name}.evolution"));
            let mut s = Save::load(&p).unwrap();
            if name == "Faster again 3" {
                s.gens.truncate(1208 + 5);
            }
            (p, name.to_string(), t, Arc::new(s), None)
        })
        .collect();
    library::analyse(&fixtures(), loaded, verdict::WINDOW, verdict::MIN_SIDE)
}

#[test]
fn detectability_mde_matches_formula() {
    let b = Fit { slope: 0.0, se: 0.2, sd: 1.0, mean: 0.0 };
    let a = Fit { slope: 0.0, se: 0.3, sd: 1.0, mean: 0.0 };
    let t = 4.0;
    let expect = t * (0.2f64.powi(2) + 0.3f64.powi(2) + 1e-12).sqrt();
    assert!((detectability::mde_slope(t, b.se, a.se) - expect).abs() < 1e-12);
}

#[test]
fn detectability_forward_on_pending_after_short() {
    let lib = truncated_pending_library();
    assert!(lib.lap_placebo_points >= verdict::MIN_PLACEBO, "lap points {}", lib.lap_placebo_points);
    let n = lib.by_file("Faster again 3").unwrap();
    assert_eq!(n.gens(), 1213);
    let v = detectability::pending_too_short(n).expect("pending TooShort-after");
    assert!(matches!(v.outcome, verdict::Outcome::CantTell(verdict::Reason::TooShort(nb, na, need)) if nb >= need && na < need));

    let plan = detectability::forward_plan(&lib, n).expect("forward plan");
    assert!(plan.by_lap, "reward change at 1208 is judged on lap time");
    assert!(!plan.at_window);
    assert!(!plan.rows.is_empty());
    assert!(plan.rows.iter().any(|r| r.gens_to_add > 0), "{:?}", plan.rows);
    assert!(plan.rows.iter().all(|r| r.length <= plan.window));
    let mdes: Vec<f64> = plan.rows.iter().map(|r| r.mde_per_10).collect();
    assert!(mdes.windows(2).all(|w| w[1] <= w[0] + 1e-9), "MDE should not rise with longer holds: {mdes:?}");

    let fmt = detectability::format_forward(&plan);
    assert!(fmt.contains("Detectability"), "{fmt}");
    let advice = advice::advise(&lib, n).unwrap();
    let wait = advice.suggestions.iter().find(|s| s.text.starts_with("These settings")).expect("wait suggestion");
    assert!(wait.why.contains("Detectability") || wait.why.contains("detect"), "{}", wait.why);
}

#[test]
fn detectability_hidden_when_placebo_thin() {
    let mut lib = truncated_pending_library();
    lib.lap_placebo_points = 10;
    lib.placebo_points = 10;
    let n = lib.by_file("Faster again 3").unwrap();
    assert!(detectability::forward_plan(&lib, n).is_none());

    let thick = fixture_library();
    let v = thick
        .by_file("Type 2.2")
        .unwrap()
        .verdicts
        .iter()
        .find(|v| v.at == 528 && matches!(v.outcome, verdict::Outcome::CantTell(verdict::Reason::WithinNoise)))
        .expect("WithinNoise at 528");
    let mut thin = thick.clone();
    thin.placebo_points = 10;
    thin.lap_placebo_points = 10;
    assert!(detectability::retrospective(&thin, v).is_none());
}

#[test]
fn detectability_at_window_formatter() {
    let plan = detectability::ForwardPlan {
        by_lap: false,
        n_after: 20,
        min_side: 10,
        window: 20,
        rows: Vec::new(),
        at_window: true,
    };
    let s = detectability::format_forward(&plan);
    assert!(s.contains("will not improve detectability"), "{s}");
    let h = detectability::format_forward_header(&plan).unwrap();
    assert!(h.contains("will not improve detectability"), "{h}");
}

#[test]
fn detectability_retrospective_within_noise() {
    let lib = fixture_library();
    assert!(lib.placebo_points >= verdict::MIN_PLACEBO);
    assert!(lib.lap_placebo_points >= verdict::MIN_PLACEBO);

    let score = lib
        .by_file("Type 2.2")
        .unwrap()
        .verdicts
        .iter()
        .find(|v| v.at == 528 && matches!(v.outcome, verdict::Outcome::CantTell(verdict::Reason::WithinNoise)))
        .expect("score WithinNoise");
    assert!(!score.by_lap);
    let r = detectability::retrospective(&lib, score).expect("score retrospective");
    let s = detectability::format_retrospective(&r);
    assert!(s.contains("would not have been flagged"), "{s}");
    assert!(s.contains("score"), "{s}");

    let lap = lib
        .by_file("Faster again 3")
        .unwrap()
        .verdicts
        .iter()
        .find(|v| {
            (v.at == 1092 || v.at == 1208)
                && v.by_lap
                && matches!(v.outcome, verdict::Outcome::CantTell(verdict::Reason::WithinNoise))
        })
        .expect("lap WithinNoise");
    let r = detectability::retrospective(&lib, lap).expect("lap retrospective");
    let s = detectability::format_retrospective(&r);
    assert!(s.contains("would not have been flagged"), "{s}");
    assert!(s.contains(" s "), "{s}");
}

#[test]
fn detectability_short_before_has_no_forward_plan() {
    let lib = fixture_library();
    // Type 2.1 last change is TooShort with short before (1, 20, 10) style — not after-short pending.
    let n = lib.by_file("Type 2.1").unwrap();
    let last = n.verdicts.last().expect("verdicts");
    if let verdict::Outcome::CantTell(verdict::Reason::TooShort(nb, _na, need)) = last.outcome {
        assert!(nb < need, "fixture last TooShort should be short-before");
    }
    assert!(detectability::pending_too_short(n).is_none());
    assert!(detectability::forward_plan(&lib, n).is_none());
}

/// Build a library from in-memory saves (name, save, mtime).
fn mem_library(saves: Vec<(&str, Save, f64)>) -> Library {
    let loaded = saves.into_iter().map(|(name, s, t)| (fixtures().join(format!("{name}.evolution")), name.to_string(), t, Arc::new(s), None)).collect();
    library::analyse(&fixtures(), loaded, verdict::WINDOW, verdict::MIN_SIDE)
}

/// A copy saved after the network got slower keeps the record of the best lap but not the
/// network that set it, so it doesn't count as a backup; a copy saved at the best does.
#[test]
fn backup_needs_the_best_network_not_just_its_record() {
    // Faster again 3 up to its best lap, then 30 slower generations (12 s on a 10 s best, say)
    let orig = Save::load(&fixtures().join("Faster again 3.evolution")).unwrap();
    let probe = mem_library(vec![("FA3", orig.clone(), 1.0)]);
    let b = advice::best_lap(&probe, probe.by_file("FA3").unwrap()).unwrap();
    let mut full = orig.clone();
    full.gens.truncate(b.at + 1);
    for k in 0..30u64 {
        let mut g = full.gens[b.at].clone();
        g.best_lap = Some(b.seconds * 1.2);
        g.hash = 0xBAD0_0000 + k;
        full.gens.push(g);
    }
    let mut slow = full.clone();
    slow.gens.truncate(b.at + 20);
    let mut at_best = full.clone();
    at_best.gens.truncate(b.at + 1);
    let lib = mem_library(vec![("FA3", full.clone(), 1.0), ("later copy", slow, 2.0)]);
    let r = advice::best_lap(&lib, lib.by_file("FA3").unwrap()).unwrap();
    assert_eq!(r.at, b.at);
    assert!(!matches!(r.backup, advice::Backup::Safe { .. }), "a copy running slower isn't a backup: {:?}", r.backup);
    let lib = mem_library(vec![("FA3", full, 1.0), ("copy at best", at_best, 2.0)]);
    let r = advice::best_lap(&lib, lib.by_file("FA3").unwrap()).unwrap();
    assert_eq!(r.backup, advice::Backup::Safe { copy: "copy at best".into() });
}

/// After switching tracks, a new best on the new track gets its own copy; the old track's copy
/// is never the one replaced.
#[test]
fn keeper_keeps_one_copy_per_track() {
    use track_builder::lab::keeper;
    let t2 = Save::load(&fixtures().join("Type 2.evolution")).unwrap();
    let mut t22 = Save::load(&fixtures().join("Type 2.2.evolution")).unwrap();
    let keys = runs::track_keys(&t22.gens);
    let (old_track, new_track) = (keys[0].clone(), keys.last().unwrap().clone());
    assert_ne!(old_track, new_track, "Type 2.2 switched tracks");
    // cut Type 2.2 at its best lap on the new track, so the latest generation set it
    let probe = mem_library(vec![("Type 2.2", t22.clone(), 1.0)]);
    let at = advice::best_lap(&probe, probe.by_file("Type 2.2").unwrap()).unwrap().at;
    t22.gens.truncate(at + 1);
    // an old-style copy made while it was on the first track (Type 2's history)
    let lib = mem_library(vec![("Type 2.2", t22, 100.0), ("Type 2.2 (best lap)", t2, 50.0)]);
    let n = lib.by_file("Type 2.2").unwrap();
    let old = keeper::copy_for(&lib, "Type 2.2", &old_track).expect("the old copy holds the first track");
    assert_eq!(old.file, "Type 2.2 (best lap)");
    assert!(keeper::copy_for(&lib, "Type 2.2", &new_track).is_none());
    let (dest, previous) = keeper::due(&lib, n, 100.0).expect("a new best on the new track is due");
    assert_ne!(dest, old.path, "the first track's copy must not be overwritten");
    assert!(dest.file_name().unwrap().to_string_lossy().starts_with("Type 2.2 (best lap · "));
    assert_eq!(previous, None);
}

/// Data gathering: habits add up, every card is still open and points at real evidence, and
/// the prediction replay produces sane numbers.
#[test]
fn data_gathering_is_consistent() {
    use track_builder::lab::gather;
    let lib = fixture_library();
    let h = gather::habits(&lib);
    assert!(h.judged <= h.changes && h.too_soon <= h.changes && h.with_time_limit <= h.changes);
    assert!(h.isolated_limit_raises <= h.limit_raises && h.clean_reward_switches <= h.reward_switches);
    let cards = gather::cards(&lib, advice::latest(&lib));
    assert!(!cards.is_empty());
    assert!(cards.windows(2).all(|w| w[0].score >= w[1].score), "most valuable first");
    for c in &cards {
        assert!(c.have < c.target, "{}: a settled question isn't a card", c.id);
        assert!(!c.protocol.is_empty() && !c.evidence.is_empty());
    }
    // direct tests can't outnumber the clean single-setting changes at that stage
    for card in cards.iter().filter(|c| c.direct.is_some()) {
        let testable = lib.networks.iter().map(|n| {
            verdict::verdicts_for_measure(&n.save.gens, &n.runs, card.measure, verdict::FALLBACK_THRESHOLD, lib.window, lib.min_side)
                .iter().filter(|v| v.testable() && v.kind != "several").count()
        }).sum::<usize>();
        let c = card.direct.unwrap();
        assert!(c.0 + c.1 + c.2 <= testable);
        assert!(card.id.ends_with(card.measure.unit()));
    }
    let ids: std::collections::BTreeSet<&str> = cards.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids.len(), cards.len(), "ids are unique (they key the ledger)");
    let (bt, odds) = gather::backtest(&lib);
    assert_eq!(bt.len(), 2);
    for b in &bt {
        if b.points > 0 {
            assert!(b.err_forecast.is_finite() && b.err_naive.is_finite() && b.err_old.is_finite());
            assert!((0.0..=1.0).contains(&b.forecast_closer) && (0.0..=1.0).contains(&b.within_1pct));
        }
    }
    assert!(odds.iter().all(|(_, k, n)| k <= n));
}

#[test]
fn ledger_round_trips_and_stays_out_of_the_game_folder() {
    use track_builder::lab::ledger::{self, Entry};
    let dir = std::env::temp_dir().join(format!("tb-ledger-{}", std::process::id()));
    let p = dir.join("evidence-ledger.json");
    let e = Entry { date: "2026-10-19".into(), values: [("habit:judged".to_string(), 32.0)].into_iter().collect() };
    ledger::save_to(&p, &[e.clone()]).unwrap();
    assert_eq!(ledger::load_from(&p), vec![e.clone()]);
    assert!(ledger::save_to(&dir.join("AI Learns To Drive").join("x.json"), &[e]).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

/// The forecast never predicts a slower best lap, makes no wild promises, and reaches a target
/// no later than it says.
#[test]
fn forecast_is_monotone_and_bounded() {
    use track_builder::lab::projection;
    let lib = fixture_library();
    let n = lib.by_file("Faster again 3").unwrap();
    let drift = projection::pooled_drift(&lib, Some(&n.file));
    assert!(drift.points.windows(2).all(|w| w[0].0 < w[1].0));
    assert_eq!(drift.at(0.0), 0.0);
    let f = projection::forecast(n, &drift).expect("Faster again 3 has lap history");
    assert!(f.curve.windows(2).all(|w| w[1].1 <= w[0].1 + 1e-12), "best so far never gets worse");
    assert!(f.curve.iter().all(|p| p.1 <= f.best + 1e-12));
    assert!((f.at(0) - f.best).abs() < 1e-9);
    // no wild promises: the old curve could extrapolate far below anything ever seen
    assert!(f.at(200) >= f.best * 0.9, "{} v best {}", f.at(200), f.best);
    if let Some(g) = f.gens_to(f.at(100)) {
        assert!(g <= 100);
    }
}

/// Head-to-head: entries line up by generations on the track and only networks with their own
/// training there count.
#[test]
fn head_to_head_lines_up_by_generations_on_track() {
    use track_builder::lab::h2h;
    let lib = fixture_library();
    for t in overview::tracks(&lib) {
        let es = h2h::compare(&lib, &t.key);
        for e in &es {
            assert!(e.points.len() >= 10);
            assert!(e.points.iter().enumerate().all(|(k, p)| p.on_track == k));
            assert!(e.at_limit.values().map(|a| a.gens).sum::<usize>() <= e.points.len());
            if let (Some(f), Some(b)) = (e.forecast_100, e.best_lap) {
                assert!(f <= b + 1e-9);
            }
        }
        if es.len() < 2 {
            assert!(h2h::shared_limits(&es).is_empty());
        }
    }
}

/// The adaptive-mutation flag is read (never written) from the game's settings file next to the
/// evolution folder.
#[test]
fn adaptive_mutation_setting_is_read_from_the_game_settings() {
    use track_builder::lab::mutation;
    let root = std::env::temp_dir().join(format!("lab-mutation-{}", std::process::id()));
    let evo = root.join("evolution");
    std::fs::create_dir_all(&evo).unwrap();
    std::fs::create_dir_all(root.join("settings")).unwrap();
    assert_eq!(mutation::adaptive_setting(&evo), None);
    std::fs::write(root.join("settings/local.settings"), r#"{"Evolution":{"IsAdaptiveMutationRate":true}}"#).unwrap();
    assert_eq!(mutation::adaptive_setting(&evo), Some(true));
    std::fs::write(root.join("settings/local.settings"), r#"{"Evolution":{"IsAdaptiveMutationRate":false}}"#).unwrap();
    assert_eq!(mutation::adaptive_setting(&evo), Some(false));
    // the game's own file is gzipped
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, br#"{"Evolution":{"IsAdaptiveMutationRate":true}}"#).unwrap();
    std::fs::write(root.join("settings/local.settings"), gz.finish().unwrap()).unwrap();
    assert_eq!(mutation::adaptive_setting(&evo), Some(true));
    let _ = std::fs::remove_dir_all(&root);
}
