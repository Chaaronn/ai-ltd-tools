use track_builder::simulation::{CampaignMode, JobSpec, POPULATION};
use track_builder::simulation::{Status, engine::SimulationEngine, storage::Store, worker};

#[test]
fn cpu_checkpoint_replays_population_and_completed_generation() {
    let spec = JobSpec::smoke().unwrap();
    let mut a = SimulationEngine::new(&spec, &spec.input, &spec.baseline, 1729).unwrap();
    a.start(false, &spec.input).unwrap();
    let initial = a.checkpoint().unwrap();
    assert!(a.drive(|| false).unwrap());
    let metrics = a.metrics().unwrap();
    a.reproduce().unwrap();
    let completed = a.checkpoint().unwrap();
    let mut b = SimulationEngine::new(&spec, &spec.input, &spec.baseline, 1729).unwrap();
    b.restore(&initial).unwrap();
    assert!(!b.drive(|| true).unwrap());
    b.restore(&initial).unwrap();
    assert!(b.drive(|| false).unwrap());
    assert_eq!(metrics, b.metrics().unwrap());
    b.reproduce().unwrap();
    assert_eq!(completed, b.checkpoint().unwrap());
}

#[test]
fn full_population_campaign_persists_paired_results() {
    let root = std::env::temp_dir().join(format!("simulation-test-{}", uuid::Uuid::new_v4()));
    let store = Store::at(root.clone());
    let spec = JobSpec::smoke().unwrap();
    let id = store.create(&spec).unwrap();
    let p = worker::run(&store, &id).unwrap();
    assert_eq!(p.status, Status::Completed);
    assert_eq!(p.observations.len(), 2);
    assert_eq!(store.spec(&id).unwrap().population, 1200);
    assert!(worker::run(&store, &id).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn simulation_defaults_are_bounded_and_population_is_fixed() {
    let spec = JobSpec::smoke().unwrap();
    assert_eq!(spec.population, POPULATION);
    assert_eq!(POPULATION, 1200);
    assert_eq!(spec.mode, CampaignMode::Fresh);
    spec.validate().unwrap();
    let mut invalid = spec.clone();
    invalid.population = 32;
    assert!(invalid.validate().is_err());
}

#[test]
fn saved_weights_and_track_are_read_only_and_run_at_fixed_population() {
    use track_builder::simulation::engine;
    let path = std::path::Path::new("tests/fixtures/evolution/Type 2.evolution");
    let track = std::path::Path::new("tests/fixtures/autumn_03_mixed_surfaces.track");
    let before = std::fs::read(path).unwrap();
    let before_track = std::fs::read(track).unwrap();
    let (mut input, baseline, _) = engine::from_save(path, "fixture-family").unwrap();
    engine::set_track(&mut input, Some(track), 1729).unwrap();
    let mut spec = JobSpec::new(input, baseline, 0.2);
    spec.backend = track_builder::simulation::Backend::Cpu;
    let mut engine = SimulationEngine::new(&spec, &spec.input, &spec.baseline, 1729).unwrap();
    engine.start(true, &spec.input).unwrap();
    assert!(engine.drive(|| false).unwrap());
    engine.reproduce().unwrap();
    assert!(!engine.checkpoint().unwrap().is_empty());
    assert_eq!(before, std::fs::read(path).unwrap());
    assert_eq!(before_track, std::fs::read(track).unwrap());
}

#[test]
fn stage_alternatives_cannot_change_later_baseline() {
    let root = std::env::temp_dir().join(format!("simulation-stage-{}", uuid::Uuid::new_v4()));
    let store = Store::at(root.clone());
    let mut spec = JobSpec::smoke().unwrap();
    spec.stages = vec![0, 2];
    let id = store.create(&spec).unwrap();
    let a = worker::run(&store, &id).unwrap();
    spec.alternatives[0].settings.mutation_rate = 0.8;
    let other = store.create(&spec).unwrap();
    let b = worker::run(&store, &other).unwrap();
    assert_eq!(
        store.read_boundary(&id, 1729, 2).unwrap(),
        store.read_boundary(&other, 1729, 2).unwrap()
    );
    let control = |p: &track_builder::simulation::Progress| {
        p.observations
            .iter()
            .filter(|o| o.arm == 0)
            .map(|o| o.metrics.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(control(&a), control(&b));
    assert_eq!(a.observations.len(), 4);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_laps_and_repeated_seeds_do_not_create_independent_wins() {
    use track_builder::simulation::{Measure, Metrics, Observation, analysis};
    let mut spec = JobSpec::smoke().unwrap();
    spec.seeds = vec![1, 2, 3, 4, 5];
    spec.measure = Measure::Lap;
    let mut rows = vec![];
    for seed in &spec.seeds {
        for arm in 0..=1 {
            rows.push(Observation {
                task: 0,
                seed: *seed,
                family: "one-source".into(),
                stage: 0,
                arm,
                metrics: Metrics {
                    lap: None,
                    rate: Some(if arm == 0 { 1.0 } else { 100.0 }),
                    survival: 1.0,
                    lapped: 0,
                },
                backend: "Cpu".into(),
                note: None,
            });
        }
    }
    let comparison = &analysis::comparisons(&spec, &rows)[0];
    assert_eq!(comparison.missing, 5);
    assert_eq!(comparison.wins, 0);
    spec.mode = CampaignMode::SavedWeights;
    spec.measure = Measure::Rate;
    rows.extend(rows.clone());
    let comparison = &analysis::comparisons(&spec, &rows)[0];
    assert_eq!(comparison.trials, 5);
    assert_eq!(comparison.families, 1);
    assert_eq!(comparison.label, "Insufficient data");
    // ... but the seeds still say how consistent the change is for this one network
    assert_eq!((comparison.seeds_better, comparison.seeds_compared), (5, 5));
    assert_eq!(comparison.seed_label, "Consistently better across seeds");
}

#[test]
fn laps_on_one_side_only_count_as_wins_or_losses_without_a_size() {
    use track_builder::simulation::{Measure, Metrics, Observation, analysis};
    let mut spec = JobSpec::smoke().unwrap();
    spec.seeds = vec![1, 2, 3, 4, 5];
    spec.measure = Measure::Lap;
    let lap = |l: Option<f64>| Metrics { lap: l, rate: Some(1.0), survival: 1.0, lapped: l.map_or(0, |_| 10) };
    let mut rows = vec![];
    for seed in &spec.seeds {
        // seeds 1-4: only the alternative laps; seed 5: both lap, alternative 2% faster
        let (a, b) = if *seed < 5 { (None, Some(30.0)) } else { (Some(30.0), Some(29.4)) };
        for (arm, m) in [(0, lap(a)), (1, lap(b))] {
            rows.push(Observation { task: 0, seed: *seed, family: "f".into(), stage: 0, arm, metrics: m, backend: "Cpu".into(), note: None });
        }
    }
    let c = &analysis::comparisons(&spec, &rows)[0];
    assert_eq!((c.alternative_only_laps, c.control_only_laps, c.missing), (4, 0, 0));
    assert_eq!((c.wins, c.losses, c.families), (5, 0, 5));
    assert!((c.median.unwrap() - 2.0).abs() < 1e-9, "only the sized pair sets the median");
    assert_eq!(c.label, "Promising in simulation");
    // and the other way round they're losses
    for r in &mut rows {
        r.arm = 1 - r.arm;
    }
    let c = &analysis::comparisons(&spec, &rows)[0];
    assert_eq!((c.alternative_only_laps, c.control_only_laps, c.wins, c.losses), (0, 4, 0, 5));
}

#[test]
fn saved_networks_use_game_control_order_grip_origin_and_game_settings() {
    use track_builder::simulation::engine;
    let root = std::env::temp_dir().join(format!("simulation-game-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("evolution")).unwrap();
    std::fs::create_dir_all(root.join("settings")).unwrap();
    let save = root.join("evolution/Type 2.evolution");
    std::fs::copy("tests/fixtures/evolution/Type 2.evolution", &save).unwrap();
    // without a settings file the defaults are used and say so
    let (input, settings, _) = engine::from_save(&save, "fixture").unwrap();
    assert!(!settings.adaptive_mutation);
    assert!(input.notes.iter().any(|n| n.contains("couldn't be read")));
    // the grip sensor's empty offset becomes the game's (0,0) without asking
    let prepared = serde_json::to_value(&input).unwrap();
    let grip = prepared["model"]["sensor_layout"]["sensors"].as_array().unwrap().iter().find(|s| s["$type"] == "grip").unwrap().clone();
    assert_eq!(grip["PositionOffset"], serde_json::json!({"X": 0.0, "Y": 0.0}));
    // the game's settings file (gzipped) supplies what saves don't record
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(
        &mut gz,
        br#"{"Evolution":{"IsAdaptiveMutationRate":true,"PreserveParentsMode":2,"PreserveParentsSize":3,"ShouldIdleEliminate":true}}"#,
    )
    .unwrap();
    std::fs::write(root.join("settings/local.settings"), gz.finish().unwrap()).unwrap();
    let (input, settings, _) = engine::from_save(&save, "fixture").unwrap();
    assert!(settings.adaptive_mutation);
    assert_eq!(settings.preserve_parents, "on_custom");
    assert_eq!(settings.preserve_parents_size, 3);
    assert!(settings.eliminate_when_idle);
    assert!(input.notes.iter().any(|n| n.contains("game's settings now")));
    settings.validate().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn alternatives_follow_baseline_edits_but_keep_their_one_change() {
    let spec = JobSpec::smoke().unwrap();
    let old = spec.baseline.clone();
    let mut arm = old.clone();
    arm.mutation_rate = old.mutation_rate + 0.1;
    let mut new = old.clone();
    new.adaptive_mutation = !old.adaptive_mutation;
    new.selection_size = 7;
    let moved = arm.rebase(&old, &new);
    assert_eq!(moved.mutation_rate, arm.mutation_rate);
    assert_eq!(moved.adaptive_mutation, new.adaptive_mutation);
    assert_eq!(moved.selection_size, 7);
    assert_eq!(moved.difference(&new), vec!["mutation_rate".to_string()]);
}

#[test]
fn nearby_designs_are_bounded_and_train_from_scratch() {
    let root = std::env::temp_dir().join(format!("simulation-design-{}", uuid::Uuid::new_v4()));
    let store = Store::at(root.clone());
    let mut spec = JobSpec::smoke().unwrap();
    spec.mode = CampaignMode::Designs;
    spec.alternatives =
        track_builder::simulation::planner::design_alternatives(&spec.input, &spec.baseline, None);
    spec.alternatives.truncate(1);
    spec.validate().unwrap();
    let id = store.create(&spec).unwrap();
    let p = worker::run(&store, &id).unwrap();
    assert_eq!(p.status, Status::Completed);
    assert_eq!(p.observations.len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_pause_resume_matches_uninterrupted_training() {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    use track_builder::simulation::storage::Control;
    let root = std::env::temp_dir().join(format!("simulation-resume-{}", uuid::Uuid::new_v4()));
    let store = Store::at(root.clone());
    let mut spec = JobSpec::smoke().unwrap();
    spec.generations = 12;
    spec.time_limit_seconds = 1.0;
    let id = store.create(&spec).unwrap();
    let worker_store = store.clone();
    let worker_id = id.clone();
    let running = thread::spawn(move || worker::run(&worker_store, &worker_id).unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let p = store.progress(&id).unwrap();
        if p.completed_generations >= 1 {
            store.control(&id, Control::Pause).unwrap();
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let paused = running.join().unwrap();
    assert_eq!(paused.status, Status::Paused);
    assert!(paused.checkpoint.is_some());
    let resumed = worker::run(&store, &id).unwrap();
    let other = store.create(&spec).unwrap();
    let uninterrupted = worker::run(&store, &other).unwrap();
    assert_eq!(
        resumed.completed_generations,
        uninterrupted.completed_generations
    );
    for (a, b) in resumed.observations.iter().zip(&uninterrupted.observations) {
        assert_eq!(a.metrics, b.metrics);
    }
    assert_eq!(resumed.observations.len(), uninterrupted.observations.len());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn versioned_worker_process_and_cli_smoke_complete() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let root = std::env::temp_dir().join(format!("simulation-process-{}", uuid::Uuid::new_v4()));
    let store = Store::at(root.clone());
    let id = store.create(&JobSpec::smoke().unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_simulation-worker"))
        .env("CATS_SIMULATION_ROOT", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        child.stdin.take().unwrap(),
        "{}",
        serde_json::to_string(&worker::Request {
            version: 1,
            job: id.clone()
        })
        .unwrap()
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(store.progress(&id).unwrap().status, Status::Completed);
    let output = Command::new(env!("CARGO_BIN_EXE_track-builder-cli"))
        .env("CATS_SIMULATION_ROOT", &root)
        .args(["simulation", "smoke"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(store.list().unwrap().len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "Requires a user-installed compatible GPU library and supported hardware"]
fn gpu_agrees_with_cpu_for_full_population() {
    use track_builder::simulation::Backend;
    let spec = JobSpec::smoke().unwrap();
    let mut cpu = SimulationEngine::new(&spec, &spec.input, &spec.baseline, 1729).unwrap();
    cpu.start(false, &spec.input).unwrap();
    let initial = cpu.checkpoint().unwrap();
    assert!(cpu.drive(|| false).unwrap());
    let expected = cpu.metrics().unwrap();
    let mut gpu_spec = spec.clone();
    gpu_spec.backend = Backend::Gpu;
    let mut gpu = SimulationEngine::new(&gpu_spec, &spec.input, &spec.baseline, 1729).unwrap();
    gpu.restore(&initial).unwrap();
    assert!(gpu.drive(|| false).unwrap());
    assert!(
        gpu.backend().0.to_lowercase().contains("hip"),
        "GPU test cannot pass through CPU fallback: {:?}",
        gpu.backend()
    );
    assert_eq!(expected, gpu.metrics().unwrap());
}

#[test]
fn adapter_preserves_saved_parameter_and_control_order() {
    use track_builder::simulation::engine;
    let path = std::path::Path::new("tests/fixtures/evolution/Type 2.evolution");
    let (raw, _) = engine::read_gzip(path).unwrap();
    let (input, _, _) = engine::from_save(path, "fixture").unwrap();
    let prepared = serde_json::to_value(input).unwrap();
    assert_eq!(
        raw["NeuralNetwork"]["Weights"],
        prepared["network"]["weights"]
    );
    assert_eq!(
        raw["NeuralNetwork"]["Biases"],
        prepared["network"]["biases"]
    );
    // Independent of the adapter's table: the Lab's names for the game's output enum
    // (Steering 0, Acceleration 1, Handbrake 2, Brake 3, Boost 4).
    let lab_names = track_builder::lab::format::OUTPUTS;
    let expected: Vec<&str> = raw["Outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| match lab_names[a.as_u64().unwrap() as usize] {
            "Steer" => "steering",
            "Accel" => "acceleration",
            "Handbrake" => "handbrake",
            "Brake" => "brake",
            "Boost" => "boost",
            other => panic!("unknown output {other}"),
        })
        .collect();
    let got: Vec<&str> = prepared["network"]["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(got, expected);
    assert_eq!(got[0], "steering", "the game's first output is steering");
    // the simulator's own rally template agrees on steering then acceleration
    let template: serde_json::Value =
        serde_json::from_str(include_str!("../assets/simulation/networks/rally.json")).unwrap();
    assert_eq!(template["outputs"][0].as_str().unwrap().to_lowercase(), "steering");
    assert_eq!(template["outputs"][1].as_str().unwrap().to_lowercase(), "acceleration");
    let network = altd_sim::nn::network::Network::from_game_export(&prepared["network"]);
    let shape = network.shape.clone();
    let mut expected = vec![];
    for (matrix, bias) in raw["NeuralNetwork"]["Weights"]
        .as_array()
        .unwrap()
        .iter()
        .zip(raw["NeuralNetwork"]["Biases"].as_array().unwrap())
    {
        for row in matrix.as_array().unwrap() {
            for v in row.as_array().unwrap() {
                expected.push(v.as_f64().unwrap());
            }
        }
        for v in bias.as_array().unwrap() {
            expected.push(v.as_f64().unwrap());
        }
    }
    let original = altd_sim::nn::network::Network::from_vector(&shape, expected);
    let inputs: Vec<_> = (0..shape[0]).map(|i| i as f64 / shape[0] as f64).collect();
    assert_eq!(
        original.forward(&inputs, altd_sim::math::profile::MathProfile::detect()),
        network.forward(&inputs, altd_sim::math::profile::MathProfile::detect())
    );
}

#[test]
fn malformed_parameters_and_sensor_mappings_are_rejected() {
    use track_builder::simulation::{PreparedInput, engine};
    let input = engine::preset(1, 1729).unwrap();
    let mut data = serde_json::to_value(&input).unwrap();
    data["network"]["shape"] = serde_json::json!([1, 0, 3]);
    assert!(
        engine::validate_input(&serde_json::from_value::<PreparedInput>(data).unwrap()).is_err()
    );
    let mut data = serde_json::to_value(input).unwrap();
    data["network"]["inputs"][0] = serde_json::json!("unsupported_sensor_123");
    assert!(
        engine::validate_input(&serde_json::from_value::<PreparedInput>(data).unwrap()).is_err()
    );
    let mut spec = JobSpec::smoke().unwrap();
    spec.alternatives[0].settings.weight_decay = 0.7;
    assert!(spec.validate().is_err());
}

#[test]
fn budget_pause_and_cancellation_do_not_publish_partial_results() {
    use track_builder::simulation::storage::Control;
    let root = std::env::temp_dir().join(format!("simulation-budget-{}", uuid::Uuid::new_v4()));
    let store = Store::at(root.clone());
    let mut spec = JobSpec::smoke().unwrap();
    spec.generations = 10000;
    spec.budget_seconds = 1;
    spec.time_limit_seconds = 2.0;
    let id = store.create(&spec).unwrap();
    let p = worker::run(&store, &id).unwrap();
    assert_eq!(p.status, Status::Paused);
    assert!(p.observations.is_empty());
    assert!(p.checkpoint.is_some());
    store.control(&id, Control::Cancel).unwrap();
    assert_eq!(store.progress(&id).unwrap().status, Status::Cancelled);
    assert!(worker::run(&store, &id).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn queued_cancellation_survives_another_active_worker() {
    use track_builder::simulation::storage::Control;
    let root = std::env::temp_dir().join(format!("simulation-cancel-{}", uuid::Uuid::new_v4()));
    let store = Store::at(root.clone());
    let id = store.create(&JobSpec::smoke().unwrap()).unwrap();
    let other_worker = store.lock().unwrap();
    store.control(&id, Control::Cancel).unwrap();
    drop(other_worker);
    let p = worker::run(&store, &id).unwrap();
    assert_eq!(p.status, Status::Cancelled);
    assert_eq!(p.completed_generations, 0);
    assert!(p.observations.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn completed_progress_is_not_replaced_by_late_control() {
    use track_builder::simulation::storage::Control;
    let root = std::env::temp_dir().join(format!("simulation-terminal-{}", uuid::Uuid::new_v4()));
    let store = Store::at(root.clone());
    let id = store.create(&JobSpec::smoke().unwrap()).unwrap();
    let p = worker::run(&store, &id).unwrap();
    for control in [Control::Pause, Control::Cancel] {
        store.control(&id, control).unwrap();
        let after = store.progress(&id).unwrap();
        assert_eq!(after.status, Status::Completed);
        assert_eq!(after.completed_generations, p.completed_generations);
        assert_eq!(after.observations.len(), p.observations.len());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn only_finished_jobs_can_be_removed() {
    let root = std::env::temp_dir().join(format!("simulation-remove-{}", uuid::Uuid::new_v4()));
    let store = Store::at(root.clone());
    let spec = JobSpec::smoke().unwrap();
    let id = store.create(&spec).unwrap();
    assert!(store.remove(&id).is_err(), "a queued job can still run");
    let mut p = store.progress(&id).unwrap();
    p.status = Status::Paused;
    store.save_progress(&id, &p).unwrap();
    assert!(store.remove(&id).is_err(), "a paused job can still resume");
    p.status = Status::Cancelled;
    store.save_progress(&id, &p).unwrap();
    store.remove(&id).unwrap();
    assert!(!store.job(&id).unwrap().exists());
    assert!(store.list().unwrap().is_empty());
    assert!(store.remove("../../etc").is_err(), "only job ids name folders");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn every_settings_card_gets_a_one_change_campaign_whatever_the_current_value() {
    use track_builder::lab::{advice, gather, library, verdict};
    use track_builder::simulation::{engine, planner};
    let lib = library::scan(std::path::Path::new("tests/fixtures/evolution"), None, verdict::WINDOW, verdict::MIN_SIDE);
    let source = advice::latest(&lib).unwrap();
    let mut tried = 0;
    for card in gather::cards(&lib, Some(source)).iter().filter(|c| c.id.starts_with("setting:")) {
        let dim = card.id.split(':').nth(2).unwrap();
        if ["Population", "Rewards"].contains(&dim) {
            continue;
        }
        let (input, settings, _) = engine::from_save(&source.path, "fixture").unwrap();
        let mut spec = JobSpec::new(input, settings, 30.0);
        match planner::for_card(&lib, &mut spec, card) {
            Ok(arms) => {
                tried += 1;
                assert_eq!(arms.len(), 1);
                assert_eq!(spec.baseline.difference(&arms[0].settings).len(), 1, "{}: one change only", card.title);
                spec.alternatives = arms;
                spec.validate().unwrap();
            }
            Err(e) => assert!(e.to_string().contains("no value to test"), "{}: {e}", card.title),
        }
    }
    assert!(tried > 0, "the fixtures have settings cards to simulate");
}

#[test]
fn an_untrained_design_starts_from_the_games_settings() {
    use track_builder::simulation::engine;
    let root = std::env::temp_dir().join(format!("simulation-fresh-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("evolution")).unwrap();
    std::fs::create_dir_all(root.join("settings")).unwrap();
    // a save with a design but no generations, like a network made in the game and not trained
    let (mut raw, _) = engine::read_gzip(std::path::Path::new("tests/fixtures/evolution/Type 2.evolution")).unwrap();
    raw["Stats"]["Generations"] = serde_json::json!([]);
    let save = root.join("evolution/New design.evolution");
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, raw.to_string().as_bytes()).unwrap();
    std::fs::write(&save, gz.finish().unwrap()).unwrap();
    // no settings file: a clear error, not a crash
    let e = engine::from_save(&save, "new").unwrap_err().to_string();
    assert!(e.contains("hasn't trained yet"), "{e}");
    let settings = br#"{"Evolution":{"TimeLimit":46,"MutationRate":0.15,"SelectionAlgorithm":1,"CrossoverAlgorithm":0,"SelectionSize":30,"WeightDecay":0,"ShouldEliminate":true,"IsAdaptiveMutationRate":true,"PreserveParentsMode":2,"PreserveParentsSize":3,"ShouldIdleEliminate":false,"Rewards":{"Ratios":[{"Model":{"Metric":0,"Type":0},"Value":100},{"Model":{"Metric":13,"Type":0},"Value":50}]}}}"#;
    std::fs::write(root.join("settings/local.settings"), settings).unwrap();
    let (input, s, time) = engine::from_save(&save, "new").unwrap();
    assert_eq!(time, Some(46.0));
    assert_eq!(s.mutation_rate, 0.15);
    assert_eq!(s.selection_algorithm, "tournament");
    assert_eq!(s.selection_size, 30);
    assert!(s.eliminate_on_wall && s.adaptive_mutation);
    assert_eq!(s.preserve_parents, "on_custom");
    assert_eq!(s.rewards.len(), 2);
    assert!(input.notes.iter().any(|n| n.contains("hasn't trained yet")));
    s.validate().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
