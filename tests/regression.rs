//! Regression tests against real tracks made in the game, plus an end-to-end build.
use std::path::Path;

use track_builder::game::track::Track;
use track_builder::pipeline::{build, BuildParams};
use track_builder::trace::{trace, TraceParams};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

#[test]
fn real_tracks_validate_and_score_close_to_the_game() {
    for f in ["oulton_park.track", "test_lots_of_corners.track", "iom_tt_user_edit.track", "autumn_03_mixed_surfaces.track"] {
        let t = Track::load(&fixture(f)).unwrap();
        let issues = t.validate();
        assert!(issues.is_empty(), "{f}: {:?}", issues.iter().map(|i| i.to_string()).collect::<Vec<_>>());
        let lap = t.lap().unwrap();
        let score = track_builder::game::curve::lap_score(&lap);
        let err = (score - t.lap_score).abs() / t.lap_score;
        assert!(err < 0.08, "{f}: calculated {score:.1} vs file {:.1}", t.lap_score);
    }
}

#[test]
fn round_trip_preserves_tiles() {
    let t = Track::load(&fixture("oulton_park.track")).unwrap();
    let back = Track::from_bytes(&t.to_bytes().unwrap()).unwrap();
    assert_eq!(t.tiles, back.tiles);
    assert_eq!(t.start, back.start);
    assert_eq!(t.start_entry, back.start_entry);
}

#[test]
fn builds_isle_of_man_from_the_map() {
    let img = image::open(fixture("iom_tt_map.png")).unwrap().to_rgb8();
    let traced = trace(&img, &TraceParams::default()).unwrap();
    let p = BuildParams { start_px: Some((488.0, 887.0)), ..Default::default() };
    let b = build(&traced, &p).unwrap();
    let t = &b.solution.track;
    assert!(t.validate().is_empty());
    let (w, h) = t.size();
    assert!(w <= 50 && h <= 50 && t.tiles.len() > 150, "{} tiles {w}x{h}", t.tiles.len());
    assert!(b.solution.mean_dev < 0.3, "mean deviation {}", b.solution.mean_dev);
}

#[test]
fn svg_with_markers_on_the_line() {
    // two-tone line, grey halo, 18 numbered ring markers, a chequered flag and an arrow
    let img = track_builder::input::load_image(&fixture("markers_test.svg")).unwrap();
    assert_eq!(img.width().max(img.height()), track_builder::input::SVG_RENDER_SIZE as u32);
    let traced = trace(&img, &TraceParams::default()).unwrap();
    assert!(traced.markers_removed >= 12, "only bridged {} markers", traced.markers_removed);
    let b = build(&traced, &BuildParams::default()).unwrap();
    assert!(b.solution.track.validate().is_empty());
    assert!(b.solution.mean_dev < 0.3);
}

#[test]
fn edited_sparse_line_builds() {
    // what the editor hands over: a simplified (sparse) version of the trace
    let img = image::open(fixture("iom_tt_map.png")).unwrap().to_rgb8();
    let traced = trace(&img, &TraceParams::default()).unwrap();
    let sparse = track_builder::fit::simplify_closed(&traced.points, 1.2);
    assert!(sparse.len() < traced.points.len() / 3, "{} of {}", sparse.len(), traced.points.len());
    let p = BuildParams { start_px: Some((488.0, 887.0)), ..Default::default() };
    let b = track_builder::pipeline::build_points(&sparse, &p).unwrap();
    assert!(b.solution.track.validate().is_empty());
}

#[test]
fn gently_sloping_straights_stay_straight() {
    // a rectangle-ish circuit whose long straights drift a few pixels over their length;
    // before join-kink scoring the solver zig-zagged along them (total turning ~8.7 laps)
    let img = track_builder::input::load_image(&fixture("sloped_straight.svg")).unwrap();
    let traced = trace(&img, &TraceParams::default()).unwrap();
    let mut p = BuildParams { start_px: Some((450.0, 463.0)), ..Default::default() };
    p.fit.exaggeration = 1.0;
    let b = build(&traced, &p).unwrap();
    let lap = b.solution.track.lap().unwrap();
    let turning = track_builder::game::curve::total_turning(&lap);
    assert!(turning < 5.0, "track is wiggly: {turning:.2} turns");
}

#[test]
fn auto_settings_never_do_worse_than_defaults() {
    let img = track_builder::input::load_image(&fixture("sloped_straight.svg")).unwrap();
    let traced = trace(&img, &TraceParams::default()).unwrap();
    let p = BuildParams { start_px: Some((450.0, 463.0)), ..Default::default() };
    let default_score = track_builder::autotune::score(&build(&traced, &p).unwrap()).unwrap().total;
    let tuned = track_builder::autotune::auto_tune(&traced.points, &p, &|_, _| {}).unwrap();
    assert!(tuned.score.total <= default_score + 1e-9, "{} > {}", tuned.score.total, default_score);
    assert!(tuned.built.solution.track.validate().is_empty());
}

#[test]
fn start_can_be_moved_and_reversed() {
    let mut t = Track::load(&fixture("oulton_park.track")).unwrap();
    let lap = t.lap().unwrap();
    let target = lap[10].cell;
    assert!(t.set_start_at(target));
    assert!(t.validate().is_empty());
    let first = t.lap().unwrap()[1].cell;
    assert!(t.reverse_direction());
    assert!(t.validate().is_empty());
    let rev = t.lap().unwrap();
    assert_eq!(rev.len(), lap.len());
    // driving the other way, the tile after the start is now the one that used to come before it
    assert_ne!(rev[1].cell, first);
    assert!(!t.set_start_at((-99, -99)));
}
