//! Sole adapter to the pinned simulator. All source files are opened read-only.
use super::*;
use altd_sim::{
    math::profile::MathProfile,
    training::session::{Session, SessionOptions},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::{io::Read, path::Path};

fn template(vehicle: u8) -> Result<Value> {
    let data = match vehicle {
        0 => include_str!("../../assets/simulation/scenes/formula_template.json"),
        1 => include_str!("../../assets/simulation/scenes/rally_template.json"),
        2 => include_str!("../../assets/simulation/scenes/snowmobile_template.json"),
        3 => include_str!("../../assets/simulation/scenes/truck_template.json"),
        _ => bail!("Unsupported vehicle {vehicle}"),
    };
    Ok(serde_json::from_str(data)?)
}
fn generated(vehicle: u8, seed: i64) -> Result<Value> {
    let settings = serde_json::from_str(include_str!(
        "../../assets/simulation/random_track_settings.json"
    ))?;
    let (_, scene) = altd_sim::track::training_tracks::training_scene_at(
        &template(vehicle)?,
        &settings,
        None,
        seed,
        0,
        0,
        MathProfile::detect(),
    )
    .map_err(anyhow::Error::msg)?;
    Ok(scene)
}
pub fn preset(vehicle: u8, seed: i64) -> Result<PreparedInput> {
    let (network, model) = if vehicle == 1 {
        (
            include_str!("../../assets/simulation/networks/rally.json"),
            include_str!("../../assets/simulation/models/rally.json"),
        )
    } else {
        (
            include_str!("../../assets/simulation/networks/formula.json"),
            include_str!("../../assets/simulation/models/formula.json"),
        )
    };
    let mut network: Value = serde_json::from_str(network)?;
    network["shape"] = json!([
        network["inputs"]
            .as_array()
            .context("Missing inputs")?
            .len(),
        16,
        16,
        network["outputs"]
            .as_array()
            .context("Missing outputs")?
            .len()
    ]);
    let scene = generated(vehicle, seed)?;
    Ok(PreparedInput { source_name:"Fresh preset".into(),source_hash:hash(network.to_string().as_bytes()),source_family:"fresh".into(), track_name:format!("Generated track {seed}"),track_hash:hash(scene.to_string().as_bytes()),vehicle,notes:vec!["Explicit baseline defaults: adaptive mutation off, preserve selection-size parents, idle elimination off.".into()],network,model:serde_json::from_str(model)?,scene })
}
pub fn read_gzip(path: &Path) -> Result<(Value, String)> {
    let bytes = std::fs::read(path)?;
    let mut text = String::new();
    flate2::read::GzDecoder::new(bytes.as_slice())
        .read_to_string(&mut text)
        .context("Expected a gzip game file")?;
    Ok((serde_json::from_str(&text)?, hash(&bytes)))
}
pub fn set_track(input: &mut PreparedInput, path: Option<&Path>, seed: i64) -> Result<()> {
    if let Some(path) = path {
        let track = crate::game::track::Track::load(path)?;
        ensure!(
            track.validate().is_empty(),
            "Track is not a valid closed drivable course"
        );
        let (raw, _) = read_gzip(path)?;
        input.scene = altd_sim::track::random_track::saved_track_scene(
            &raw,
            &path.file_stem().unwrap_or_default().to_string_lossy(),
            &template(input.vehicle)?,
            MathProfile::detect(),
        )
        .map_err(anyhow::Error::msg)?;
        input.track_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into();
    } else {
        input.scene = generated(input.vehicle, seed)?;
        input.track_name = format!("Generated track {seed}");
    }
    input.track_hash = hash(input.scene.to_string().as_bytes());
    Ok(())
}
pub fn from_save(
    path: &Path,
    family: &str,
    explicit_origin_grip: bool,
) -> Result<(PreparedInput, Settings, Option<f64>)> {
    let (raw, source_hash) = read_gzip(path)?;
    let vehicle = raw["Type"].as_u64().context("Missing vehicle")?;
    ensure!(vehicle <= 3, "Unsupported vehicle");
    let sensors = raw["Sensors"]
        .as_array()
        .context("Missing ordered sensors")?;
    let mut sensors = sensors.clone();
    let mut notes=vec!["Save does not contain the complete game population/RNG. Adaptive mutation is explicitly off, preservation uses selection size, idle elimination is off; review these defaults.".into()];
    for sensor in &mut sensors {
        if sensor["$type"] == "grip"
            && sensor["PositionOffset"]
                .as_object()
                .is_some_and(|o| o.is_empty())
        {
            ensure!(
                explicit_origin_grip,
                "Grip PositionOffset is missing. Explicitly choose origin grip position in the campaign builder, or supply a complete sensor descriptor."
            );
            sensor["PositionOffset"] = json!({"X":0.0,"Y":0.0});
            notes.push(
                "User explicitly selected grip position (0,0) for a missing descriptor.".into(),
            );
        }
    }
    let names: Vec<String> = sensors
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}:{i}", s["$type"].as_str().unwrap_or("unknown")))
        .collect();
    let outputs = raw["Outputs"]
        .as_array()
        .context("Missing ordered controls")?
        .iter()
        .map(|v| {
            let i = v.as_u64().context("Invalid output")? as usize;
            ["acceleration", "steering", "brake", "handbrake", "boost"]
                .get(i)
                .copied()
                .context("Unsupported output")
        })
        .collect::<Result<Vec<_>>>()?;
    let nn = &raw["NeuralNetwork"];
    let network = json!({"shape":nn["Shape"],"weights":nn["Weights"],"biases":nn["Biases"],"inputs":names,"outputs":outputs});
    let model = json!({"sensor_layout":{"names":names,"sensors":sensors}});
    let gens = raw
        .pointer("/Stats/Generations")
        .and_then(Value::as_array)
        .context("Missing training history; use a preset for an empty network")?;
    let last = gens
        .last()
        .context("Empty training history; use a preset")?;
    let select = last["SelectionAlgorithm"]
        .as_u64()
        .context("Missing selection")? as usize;
    let cross = last["CrossoverAlgorithm"]
        .as_u64()
        .context("Missing crossover")? as usize;
    let metrics = [
        "total_score",
        "total_speed",
        "total_abs_steering",
        "total_drifting_amount",
        "total_actual_distance",
        "total_racing_line_efficiency",
        "total_momentum_change_frequency",
        "total_throttle_change_frequency",
        "total_braking_frequency",
        "total_distance_from_wall",
        "total_distance_from_center",
        "network_novelty",
        "collision_count",
        "best_lap_performance",
    ];
    let mut rewards = Vec::new();
    for r in last
        .pointer("/Rewards/Ratios")
        .and_then(Value::as_array)
        .context("Missing rewards")?
    {
        let weight = r["Value"].as_f64().context("Invalid reward weight")?;
        if weight == 0.0 {
            continue;
        }
        ensure!(
            weight.is_finite() && weight.fract() == 0.0 && weight.abs() < i64::MAX as f64,
            "Only integer reward weights are supported"
        );
        let metric = r
            .pointer("/Model/Metric")
            .and_then(Value::as_u64)
            .context("Missing reward metric")? as usize;
        let kind = r
            .pointer("/Model/Type")
            .and_then(Value::as_u64)
            .context("Missing reward type")?;
        ensure!(kind <= 1, "Unsupported reward type");
        rewards.push(Reward {
            metric: metrics
                .get(metric)
                .context("Unsupported reward metric")?
                .to_string(),
            weight: weight as i64,
            kind: if kind == 0 { "default" } else { "average" }.into(),
        });
    }
    let settings = Settings {
        selection_algorithm: ["best", "tournament", "roulette"]
            .get(select)
            .context("Unsupported selection")?
            .to_string(),
        selection_size: last["SelectionSize"]
            .as_u64()
            .context("Missing selection size")? as usize,
        crossover: ["none", "single_point", "uniform"]
            .get(cross)
            .context("Unsupported crossover")?
            .to_string(),
        mutation_rate: last["MutationRate"].as_f64().context("Missing mutation")?,
        weight_decay: last["WeightDecay"].as_f64().context("Missing decay")?,
        eliminate_on_wall: last["WasEliminationEnabled"]
            .as_bool()
            .context("Missing elimination")?,
        rewards,
        ..Settings::default()
    };
    let time = gens
        .iter()
        .rev()
        .find(|g| g["ActiveCount"].as_u64().is_some_and(|v| v > 0))
        .and_then(|g| g["Time"].as_f64())
        .filter(|t| t.is_finite() && *t > 0.0)
        .map(|t| (t * 10.0).round() / 10.0);
    let scene = generated(vehicle as u8, 1729)?;
    let input = PreparedInput {
        source_name: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        source_hash,
        source_family: family.into(),
        vehicle: vehicle as u8,
        track_name: "Generated track 1729".into(),
        track_hash: hash(scene.to_string().as_bytes()),
        notes,
        network,
        model,
        scene,
    };
    validate_input(&input)?;
    settings.validate()?;
    Ok((input, settings, time))
}
pub fn validate_input(input: &PreparedInput) -> Result<()> {
    ensure!(
        hash(input.scene.to_string().as_bytes()) == input.track_hash,
        "Track geometry hash mismatch"
    );
    let shape: Vec<usize> =
        serde_json::from_value(input.network["shape"].clone()).context("Missing network shape")?;
    ensure!(shape.len() >= 2 && !shape.contains(&0), "Invalid shape");
    ensure!(
        shape.iter().all(|n| *n <= 4096)
            && shape.windows(2).map(|w| (w[0] + 1) * w[1]).sum::<usize>() <= 1_000_000,
        "Network exceeds memory safety limit"
    );
    ensure!(
        input.network["inputs"].as_array().map(Vec::len) == shape.first().copied()
            && input.network["outputs"].as_array().map(Vec::len) == shape.last().copied(),
        "Sensor/output shape mismatch"
    );
    altd_sim::training::SensorLayout::try_from_exports(
        &input.network,
        &input.model,
        MathProfile::detect(),
    )
    .map_err(anyhow::Error::msg)?;
    if input.network.get("weights").is_some() {
        let weights: Vec<Vec<Vec<f64>>> = serde_json::from_value(input.network["weights"].clone())
            .context("Malformed saved weights")?;
        let biases: Vec<Vec<f64>> = serde_json::from_value(input.network["biases"].clone())
            .context("Malformed saved biases")?;
        ensure!(
            weights.len() == shape.len() - 1 && biases.len() == weights.len(),
            "Parameter layer count differs from shape"
        );
        for (i, (matrix, bias)) in weights.iter().zip(&biases).enumerate() {
            ensure!(
                matrix.len() == shape[i] && bias.len() == shape[i + 1],
                "Parameter dimensions differ at layer {i}"
            );
            ensure!(
                matrix
                    .iter()
                    .all(|row| row.len() == shape[i + 1] && row.iter().all(|v| v.is_finite()))
                    && bias.iter().all(|v| v.is_finite()),
                "Invalid weights or biases at layer {i}"
            );
        }
    }
    Ok(())
}

pub fn design_description(input:&PreparedInput)->String {
    let shape:Vec<usize>=serde_json::from_value(input.network["shape"].clone()).unwrap_or_default();
    let parameters:usize=shape.windows(2).map(|w|(w[0]+1)*w[1]).sum();
    format!("Layers {shape:?} · {parameters} parameters\nOrdered inputs: {}\nOrdered outputs: {}",input.network["inputs"],input.network["outputs"])
}

pub struct SimulationEngine {
    session: Session,
    limit: u64,
    input: PreparedInput,
    options: Value,
    fallback_note: Option<String>,
}
impl SimulationEngine {
    pub fn new(
        spec: &JobSpec,
        input: &PreparedInput,
        settings: &Settings,
        seed: i64,
    ) -> Result<Self> {
        settings.validate()?;
        validate_input(input)?;
        let options = json!({"population":POPULATION,"seed":seed,"batchCount":1,"statsPhase":0,
            "settings":settings.evolution(),"eliminateOnWall":settings.eliminate_on_wall,
            "eliminateWhenIdle":settings.eliminate_when_idle,"mathProfile":spec.math_profile,
            "backend":if spec.backend==Backend::Cpu{"cpu"}else{"hip"}});
        let parsed = SessionOptions::from_json(&options.to_string()).map_err(anyhow::Error::msg)?;
        let session = Session::new(&input.scene, &input.network, &input.model, parsed)
            .map_err(anyhow::Error::msg)?;
        Ok(Self {
            session,
            limit: (spec.time_limit_seconds * 60.0).round().max(1.0) as u64,
            input: input.clone(),
            options,
            fallback_note: None,
        })
    }
    pub fn start(&mut self, saved: bool, input: &PreparedInput) -> Result<()> {
        if saved {
            self.session.start().map_err(anyhow::Error::msg)?;
        } else {
            let shape: Vec<usize> = serde_json::from_value(input.network["shape"].clone())?;
            self.session
                .start_with_shape(&shape)
                .map_err(anyhow::Error::msg)?;
        }
        self.check_population()
    }
    fn check_population(&self) -> Result<()> {
        ensure!(
            self.session.runner.settings.population == POPULATION
                && self.session.runner.agents.len() == POPULATION,
            "Checkpoint population must be {POPULATION}"
        );
        Ok(())
    }
    pub fn checkpoint(&self) -> Result<Vec<u8>> {
        self.check_population()?;
        self.session.checkpoint_bytes().map_err(anyhow::Error::msg)
    }
    pub fn restore(&mut self, bytes: &[u8]) -> Result<()> {
        self.session
            .restore_checkpoint_bytes(bytes)
            .map_err(anyhow::Error::msg)?;
        self.check_population()
    }
    pub fn settings(&mut self, settings: &Settings) -> Result<()> {
        settings.validate()?;
        self.session
            .set_evolution_settings(&settings.evolution().to_string())
            .map_err(anyhow::Error::msg)?;
        self.options["settings"] = settings.evolution();
        self.check_population()
    }
    pub fn backend(&self) -> (String, Option<String>) {
        (
            format!("{:?}", self.session.backend()),
            self.fallback_note
                .clone()
                .or_else(|| self.session.backend_note().map(str::to_owned)),
        )
    }
    /// Returns false for a cancelled partial generation; caller restores the last checkpoint.
    pub fn drive(&mut self, mut stop: impl FnMut() -> bool) -> Result<bool> {
        match self.drive_batches(&mut stop) {
            Err(error) if self.session.backend() == altd_sim::training::session::Backend::Hip => {
                // Discard all partial GPU work and replay the same population/RNG on CPU.
                let checkpoint = self.checkpoint()?;
                self.options["backend"] = json!("cpu");
                let options = SessionOptions::from_json(&self.options.to_string())
                    .map_err(anyhow::Error::msg)?;
                let mut cpu = Session::new(
                    &self.input.scene,
                    &self.input.network,
                    &self.input.model,
                    options,
                )
                .map_err(anyhow::Error::msg)?;
                cpu.restore_checkpoint_bytes(&checkpoint)
                    .map_err(anyhow::Error::msg)?;
                self.session = cpu;
                self.fallback_note = Some(format!(
                    "GPU execution failed ({error:#}); replayed the generation on CPU at population {POPULATION}."
                ));
                self.check_population()?;
                self.drive_batches(&mut stop)
            }
            result => result,
        }
    }
    fn drive_batches(&mut self, stop: &mut impl FnMut() -> bool) -> Result<bool> {
        let safe = self.limit.saturating_sub(18);
        while self.session.runner.tick < safe {
            if stop() {
                return Ok(false);
            }
            let n = (safe - self.session.runner.tick).min(60);
            let done = self.session.advance(n, true).map_err(anyhow::Error::msg)?;
            if done < n {
                self.session.sync().map_err(anyhow::Error::msg)?;
                return Ok(true);
            }
        }
        if stop() {
            return Ok(false);
        }
        self.session
            .advance_generation(self.limit)
            .map_err(anyhow::Error::msg)?;
        self.session.sync().map_err(anyhow::Error::msg)?;
        Ok(true)
    }
    pub fn reproduce(&mut self) -> Result<()> {
        self.session.next_generation().map_err(anyhow::Error::msg)?;
        self.check_population()
    }
    pub fn metrics(&mut self) -> Result<Metrics> {
        self.session.sync().map_err(anyhow::Error::msg)?;
        let summary = serde_json::to_value(self.session.generation_summary())?;
        let elapsed = self.session.runner.tick as f64 / 60.0;
        let mut rates: Vec<f64> = self
            .session
            .runner
            .agents
            .iter()
            .filter_map(|a| {
                (elapsed > 0.0)
                    .then_some(a.stats.total_score / elapsed)
                    .filter(|x| x.is_finite())
            })
            .collect();
        Ok(Metrics {
            lap: summary["lapTime"]
                .as_f64()
                .filter(|x| x.is_finite() && *x > 0.0),
            rate: crate::lab::runs::median(&mut rates),
            survival: summary["active"]
                .as_u64()
                .ok_or_else(|| anyhow!("Missing active count"))? as f64
                / POPULATION as f64,
            lapped: summary["lapped"].as_u64().unwrap_or(0) as usize,
        })
    }
}
