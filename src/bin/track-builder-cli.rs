//! Command line front end (handy for batch runs and testing).
//!
//!   track-builder-cli build <image|svg> <out.track> [--start X,Y] [--reverse] [--grid N]
//!                     [--exaggeration F] [--fidelity F] [--no-angled] [--surface asphalt|dirt|ice]
//!                     [--debug-trace trace.png] [--auto]
//!   track-builder-cli check <file.track>...
//!   track-builder-cli lab [evolution folder] [--window N] [--runs SAVE] [--overview] [--advise [SAVE]] [--inside SAVE]   (Evolution Lab summary, read-only)
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use track_builder::game::tiles::Surface;
use track_builder::game::track::Track;
use track_builder::pipeline::{build, BuildParams};
use track_builder::trace::{trace, TraceParams};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => {
            for f in &args[1..] {
                let t = Track::load(&PathBuf::from(f))?;
                let issues = t.validate();
                let lap = t.lap();
                let calc = lap.as_ref().map(|l| track_builder::game::curve::lap_score(l)).unwrap_or(0.0);
                let (w, h) = t.size();
                println!(
                    "{f}: {} tiles, {w}x{h}, lap score file {:.1} / calculated {:.1}, {}",
                    t.tiles.len(),
                    t.lap_score,
                    calc,
                    if issues.is_empty() { "OK".to_string() } else { format!("{} issue(s)", issues.len()) }
                );
                for i in issues.iter().take(10) {
                    println!("   - {i}");
                }
            }
            Ok(())
        }
        Some("lab") => lab(&args[1..]),
        Some("build") => {
            let img_path = args.get(1).context("missing image")?;
            let out = args.get(2).context("missing output path")?;
            let mut p = BuildParams::default();
            let mut debug_trace: Option<String> = None;
            let mut auto = false;
            let mut i = 3;
            while i < args.len() {
                let val = || args.get(i + 1).cloned().context("missing value");
                match args[i].as_str() {
                    "--start" => {
                        let v = val()?;
                        let (x, y) = v.split_once(',').context("--start X,Y")?;
                        p.start_px = Some((x.trim().parse()?, y.trim().parse()?));
                        i += 1;
                    }
                    "--reverse" => p.reverse = true,
                    "--auto" => auto = true,
                    "--debug-trace" => { debug_trace = Some(val()?); i += 1; }
                    "--grid" => { p.fit.grid = val()?.parse()?; i += 1; }
                    "--exaggeration" => { p.fit.exaggeration = val()?.parse()?; i += 1; }
                    "--fidelity" => { p.solve.fidelity = val()?.parse()?; i += 1; }
                    "--tolerance" => { p.solve.tolerance = val()?.parse()?; i += 1; }
                    "--turn-penalty" => { p.solve.turn_penalty = val()?.parse()?; i += 1; }
                    "--wiggle-penalty" => { p.solve.wiggle_penalty = val()?.parse()?; i += 1; }
                    "--smoothing" => { p.fit.smoothing = val()?.parse()?; i += 1; }
                    "--no-angled" => p.solve.allow_angled = false,
                    "--surface" => {
                        p.solve.surface = match val()?.as_str() { "dirt" => Surface::Dirt, "ice" => Surface::Ice, _ => Surface::Asphalt };
                        i += 1;
                    }
                    other => bail!("unknown option {other}"),
                }
                i += 1;
            }
            let img = track_builder::input::load_image(std::path::Path::new(img_path))?;
            let t0 = std::time::Instant::now();
            let traced = trace(&img, &TraceParams::default())?;
            if let Some(dbg) = &debug_trace {
                // traced line in red over the input, for checking the tracer
                let mut out = img.clone();
                for &(x, y) in &traced.points {
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1)] {
                        let (px, py) = (x as i64 + dx, y as i64 + dy);
                        if px >= 0 && py >= 0 && (px as u32) < out.width() && (py as u32) < out.height() {
                            out.put_pixel(px as u32, py as u32, image::Rgb([255, 0, 0]));
                        }
                    }
                }
                out.save(dbg)?;
            }
            println!("traced {} points, line width {:.1}px ({:?})", traced.points.len(), traced.line_width, t0.elapsed());
            let b = if auto {
                let t = track_builder::autotune::auto_tune(&traced.points, &p, &|_, _| {})?;
                let sc = &t.score;
                println!(
                    "auto: {} builds -> exaggeration {:.2}, smoothness {:.2}, tolerance {:.2}, fidelity {:.0} | score {:.3} (track->course {:.3}, course->track {:.3}, p95 {:.3}, turning {:.2} vs course {:.2})",
                    t.builds, t.params.fit.exaggeration, t.params.solve.turn_penalty, t.params.solve.tolerance, t.params.solve.fidelity,
                    sc.total, sc.track_to_course, sc.course_to_track, sc.course_to_track_p95, sc.turning, sc.course_turning
                );
                t.built
            } else {
                let b = build(&traced, &p)?;
                if let Some(sc) = track_builder::autotune::score(&b) {
                    println!("score {:.3} (track->course {:.3}, course->track {:.3}, p95 {:.3}, turning {:.2} vs course {:.2})",
                        sc.total, sc.track_to_course, sc.course_to_track, sc.course_to_track_p95, sc.turning, sc.course_turning);
                }
                b
            };
            let s = &b.solution;
            let (w, h) = s.track.size();
            println!(
                "{} tiles, {w}x{h}, mean deviation {:.2} tiles, max {:.2}, turning {:.1} laps, lap score {:.1} ({:?})",
                s.track.tiles.len(), s.mean_dev, s.max_dev,
                s.track.lap().map(|l| track_builder::game::curve::total_turning(&l)).unwrap_or(0.0),
                s.track.lap_score, t0.elapsed()
            );
            let issues = s.track.validate();
            if !issues.is_empty() {
                bail!("built track failed validation: {}", issues[0]);
            }
            s.track.save(&PathBuf::from(out))?;
            println!("saved {out}");
            Ok(())
        }
        _ => {
            eprintln!("usage: track-builder-cli build <image> <out.track> [options] | check <file.track>... | lab [folder] [--window N]");
            Ok(())
        }
    }
}

/// Evolution Lab summary of a folder of saves. Prints only; writes nothing.
fn lab(args: &[String]) -> Result<()> {
    use track_builder::lab::{compare, library, verdict::Outcome};
    let mut dir = library::default_dir();
    let mut window = track_builder::lab::verdict::WINDOW;
    let mut show_runs: Option<String> = None;
    let mut overview = false;
    let mut inside: Option<String> = None;
    let mut advise: Option<Option<String>> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--inside" {
            inside = Some(args.get(i + 1).context("--inside needs a save name")?.clone());
            i += 2;
        } else if args[i] == "--advise" {
            let name = args.get(i + 1).filter(|a| !a.starts_with("--")).cloned();
            i += if name.is_some() { 2 } else { 1 };
            advise = Some(name);
        } else if args[i] == "--overview" {
            overview = true;
            i += 1;
        } else if args[i] == "--runs" {
            show_runs = Some(args.get(i + 1).context("--runs needs a save name")?.clone());
            i += 2;
        } else if args[i] == "--window" {
            window = args.get(i + 1).context("--window needs a number")?.parse()?;
            i += 2;
        } else {
            dir = Some(PathBuf::from(&args[i]));
            i += 1;
        }
    }
    let dir = dir.context("no evolution folder")?;
    let lib = library::scan(&dir, None, window, (window / 2).max(5));
    if let Some(name) = inside {
        use track_builder::lab::netviz;
        let n = lib.by_file(&name).context("no such save")?;
        let w = n.save.weights.as_ref().context("no weights")?;
        let imp = netviz::importance(w);
        let mut order: Vec<usize> = (0..imp[0].len()).collect();
        order.sort_by(|&a, &b| imp[0][b].total_cmp(&imp[0][a]));
        println!("{}: input usage (estimated from weights)", n.file);
        for i in order {
            println!("  {:22} {:5.1}%", netviz::input_name(&n.save, i), imp[0][i] * 100.0);
        }
        if let Some(l) = netviz::learning(&n.save) {
            println!(
                "weights changed {:.2}% since gen {} ({:.2}% per 10 gens); per layer {:?}",
                l.total * 100.0,
                l.from_gen,
                l.per_10 * 100.0,
                l.layers.iter().map(|x| format!("{:.2}%", x * 100.0)).collect::<Vec<_>>()
            );
            for (a, b, c) in &l.history {
                println!("  gens {a}-{b}: {:.2}% per 10 gens", c * 100.0);
            }
        }
        return Ok(());
    }
    if let Some(name) = advise {
        use track_builder::lab::advice;
        let n = match &name {
            Some(f) => lib.by_file(f).map(|n| n.as_ref()),
            None => advice::latest(&lib),
        }
        .context("no such save")?;
        let a = advice::advise(&lib, n).context("no history")?;
        println!(
            "{} ({}) on {}, {} stage, gen {}: {}",
            a.file,
            if a.live { "training now" } else { "latest save" },
            a.track_name,
            a.stage.map_or("?", |s| compare::STAGES[s]),
            n.gens(),
            track_builder::lab::overview::settings_label(&a.run.settings)
        );
        for s in &a.suggestions {
            println!("  [{}] {}\n        {}", s.strength.label(), s.text, s.why);
        }
        return Ok(());
    }
    if overview {
        use track_builder::lab::{compare::Measure, overview as ov};
        for (scope, key) in std::iter::once(("All tracks".to_string(), None)).chain(ov::tracks(&lib).into_iter().filter(|t| t.runs >= 5).map(|t| (t.name.clone(), Some(t.key.clone())))) {
            println!("\n== {scope}");
            for c in ov::playbook(&lib, key.as_deref(), Measure::Lap, 10) {
                println!("  {} ({} runs){}", compare::STAGES[c.stage], c.runs, c.flat.map(|f| format!(" FLAT best {f:+.1}%")).unwrap_or_default());
                for p in &c.picks {
                    println!(
                        "     {:12} {:10} {:+7.1}% n={:<3} vs {} ({}) beats {:.0}%  [{}]",
                        p.dimension,
                        p.best.label,
                        p.best.median(),
                        p.best.n(),
                        p.runner_up.as_ref().map_or("-".into(), |r| format!("{} {:+.1}%", r.label, r.median())),
                        p.runner_up.as_ref().map_or(0, |r| r.n()),
                        p.beats.unwrap_or(0.0) * 100.0,
                        p.strength.label()
                    );
                }
            }
        }
        println!("\n== Findings");
        for f in ov::findings(&lib, None, Measure::Lap, 10) {
            println!("  [{}] {}", f.strength.label(), f.text);
        }
        return Ok(());
    }
    if let Some(name) = show_runs {
        use track_builder::lab::format;
        let n = lib.by_file(&name).context("no save with that name")?;
        println!("start  gens  mutation  selection       crossover    median start->end   gain/10   best lap  stage  change");
        for (j, r) in n.runs.iter().enumerate() {
            let v = n.verdicts.iter().find(|v| v.run == j).map(|v| v.label()).unwrap_or_default();
            println!(
                "{:5} {:5}  {:8}  {:14}  {:11}  {:>8} -> {:<8}  {:>8}  {:>8}  {:5}  {}",
                r.start,
                r.len(),
                track_builder::lab::verdict::fmt_opt(r.settings.mutation),
                format!("{}·{}", format::selection_name(r.settings.selection), r.settings.selection_size.unwrap_or(0)),
                format::crossover_name(r.settings.crossover),
                r.start_median.map_or("-".into(), |x| format!("{x:.1}")),
                r.end_median.map_or("-".into(), |x| format!("{x:.1}")),
                r.score_gain.map_or("-".into(), |x| format!("{x:+.1}%")),
                r.best_lap.map_or("-".into(), |x| format!("{x:.3}s")),
                r.stage_label(),
                v
            );
        }
        return Ok(());
    }
    println!("{}: {} saves, placebo threshold |z| > {:.2} from {} points (window {window})", dir.display(), lib.networks.len(), lib.threshold, lib.placebo_points);
    for n in &lib.networks {
        let fork = n.fork.parent.map(|(p, k)| format!(" (fork of {} at {k})", lib.networks[p].file)).unwrap_or_default();
        let clear: Vec<String> = n
            .verdicts
            .iter()
            .filter(|v| matches!(v.outcome, Outcome::Helped | Outcome::Hurt))
            .map(|v| format!("{}{} {}", if v.outcome == Outcome::Helped { "+" } else { "-" }, v.at, v.kind))
            .collect();
        println!(
            "  {:28} {:5} gens {:4} runs {:3} stalls, last gain {:4} gens ago{}{}{}",
            n.file,
            n.gens(),
            n.runs.len(),
            n.stalls.len(),
            n.since_gain,
            fork,
            if clear.is_empty() { String::new() } else { format!(" clear: {}", clear.join(", ")) },
            n.error.as_ref().map(|e| format!(" ERROR {e}")).unwrap_or_default()
        );
    }
    let p = compare::pooled_verdicts(&lib, &compare::Filters::default());
    println!("changes {} (shared history once): testable {}, helped {}, hurt {} (~{:.1} by chance); untestable {:?}", p.changes, p.total.testable, p.total.helped, p.total.hurt, p.total.expected(), p.untestable);
    Ok(())
}
