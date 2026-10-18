//! Evolution Lab tests against real saves (tests/fixtures/evolution).
//! Set LAB_SAVES to a folder of saves to also check the whole library (e.g. the game's folder).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use track_builder::lab::format::Save;
use track_builder::lab::library::{self, Library};
use track_builder::lab::{advice, compare, designs, overview, runs, verdict};

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
    let dest = keeper::keeper_path(&dir, "Type 2").unwrap();
    assert!(keeper::write_copy(&src, &dest, 1).is_err(), "refuses when the save changed since it was read");
    keeper::write_copy(&src, &dest, 289).unwrap();
    let orig = Save::load(&src).unwrap();
    let copy = Save::load(&dest).unwrap();
    assert_eq!(copy.name, "Type 2 (best lap)");
    assert_ne!(copy.id, orig.id);
    assert_eq!(copy.id.split('-').nth(2).unwrap().chars().next(), Some('4'), "a version-4 id, so not modded");
    assert_eq!(copy.gens.len(), orig.gens.len());
    assert!(copy.gens.iter().zip(&orig.gens).all(|(a, b)| a.hash == b.hash), "history identical");
    assert_eq!(copy.arch.params(), orig.arch.params());
    let lib = library::scan(&dir, None, verdict::WINDOW, verdict::MIN_SIDE);
    let c = lib.by_file("Type 2 (best lap)").unwrap();
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
            assert!(p.all.iter().all(|ch| ch.n() >= overview::MIN_RUNS));
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
    // design picks are counted per network
    for c in overview::playbook(&lib, None, compare::Measure::Score, 10) {
        for p in c.picks.iter().filter(|p| p.design) {
            assert!(p.all.iter().all(|ch| ch.unit == "networks"));
        }
    }
    let sums = designs::summaries(&lib, compare::Measure::Score);
    assert!(sums.iter().any(|s| s.file == "Faster again 3"));
    // the Type 2 family shares a design, Faster again 3 differs: features compare them
    let feats = designs::features(&lib, compare::Measure::Score);
    assert!(feats.iter().all(|f| !f.with_nets.is_empty() && !f.without_nets.is_empty()));
}

#[test]
fn best_lap_backup_status() {
    let lib = fixture_library();
    // Type 2's whole history lives on in its copies Type 2.1 and 2.2, so its best lap is safe
    let t2 = advice::best_lap(&lib, lib.by_file("Type 2").unwrap()).expect("Type 2 laps");
    match &t2.backup {
        advice::Backup::Safe { copy } => assert!(copy.starts_with("Type 2.")),
        other => panic!("expected safe, got {other:?}"),
    }
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
