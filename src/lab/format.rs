//! Reading the game's `.evolution` saves (gzip-compressed JSON). Read-only: nothing here can
//! write a file.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use serde_json::Value;

/// `VehicleType` in the game.
pub const VEHICLES: [&str; 4] = ["Formula", "Rally", "Snowmobile", "Truck"];
/// `SelectionAlgorithm`.
pub const SELECTION: [&str; 3] = ["Best", "Tournament", "Roulette"];
/// `CrossoverAlgorithm`.
pub const CROSSOVER: [&str; 3] = ["None", "SinglePoint", "Uniform"];
/// `RewardMetric`.
pub const METRICS: [&str; 14] = [
    "Score",
    "Speed",
    "Abs steering",
    "Drifting",
    "Distance",
    "Racing line",
    "Momentum changes",
    "Throttle changes",
    "Braking",
    "Wall distance",
    "Centre distance",
    "Novelty",
    "Collisions",
    "Best lap",
];

pub fn vehicle_name(v: Option<u8>) -> &'static str {
    v.and_then(|v| VEHICLES.get(v as usize).copied()).unwrap_or("?")
}
pub fn selection_name(v: Option<u8>) -> &'static str {
    v.and_then(|v| SELECTION.get(v as usize).copied()).unwrap_or("?")
}
pub fn crossover_name(v: Option<u8>) -> &'static str {
    v.and_then(|v| CROSSOVER.get(v as usize).copied()).unwrap_or("?")
}
pub fn metric_name(m: u8) -> &'static str {
    METRICS.get(m as usize).copied().unwrap_or("?")
}

/// One reward weight that is switched on (non-zero).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reward {
    pub metric: u8,
    /// 0 Default, 1 Average
    pub kind: u8,
    pub value: f64,
}

/// The settings that define a run. Two generations with equal `Settings` (and the same rewards
/// and track) belong to the same run.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub mutation: Option<f64>,
    pub selection: Option<u8>,
    pub selection_size: Option<u32>,
    pub crossover: Option<u8>,
    pub weight_decay: Option<f64>,
    pub elimination: Option<bool>,
    pub population: Option<u32>,
    /// The generation time limit (simulated seconds). Not saved directly: a generation that
    /// ends with cars still driving ran to the limit, so its `Time` is the limit.
    pub time_limit: Option<f64>,
}

/// One generation record from `Stats.Generations`.
#[derive(Clone, Debug)]
pub struct Gen {
    pub settings: Settings,
    /// Non-zero reward weights, sorted.
    pub rewards: Vec<Reward>,
    pub active: Option<u32>,
    /// Simulated seconds.
    pub time: Option<f64>,
    pub best: Option<f64>,
    pub average: Option<f64>,
    pub median: Option<f64>,
    pub q1: Option<f64>,
    pub q3: Option<f64>,
    pub worst: Option<f64>,
    pub best_speed: Option<f64>,
    pub best_lap: Option<f64>,
    pub track: Option<String>,
    /// Hash of the raw record, used to find saves copied from one another.
    pub hash: u64,
}

impl Gen {
    /// Share of cars still running at the end of the generation.
    pub fn survival(&self) -> Option<f64> {
        match (self.active, self.settings.population) {
            (Some(a), Some(p)) if p > 0 => Some(a as f64 / p as f64),
            _ => None,
        }
    }
    /// Median score per simulated second. Unaffected by raising the time limit (a longer
    /// generation scores more just by lasting longer); with a distance-only reward it's the
    /// median car's average speed.
    pub fn rate(&self) -> Option<f64> {
        match (self.median, self.time) {
            (Some(m), Some(t)) if t > 0.0 => Some(m / t),
            _ => None,
        }
    }
    /// (Q3 - Q1) / |median|
    pub fn spread(&self) -> Option<f64> {
        match (self.q1, self.q3, self.median) {
            (Some(a), Some(b), Some(m)) if m.abs() > 1e-9 => Some((b - a) / m.abs()),
            _ => None,
        }
    }
    pub fn rewards_label(&self) -> String {
        rewards_label(&self.rewards)
    }
}

pub fn rewards_label(r: &[Reward]) -> String {
    if r.is_empty() {
        return "none".into();
    }
    r.iter()
        .map(|w| format!("{}{} {}", metric_name(w.metric), if w.kind == 1 { " (avg)" } else { "" }, fmt_num(w.value)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") }
}

/// What the tool reads from one save.
#[derive(Clone, Debug)]
pub struct Save {
    pub id: String,
    pub name: String,
    pub vehicle: Option<u8>,
    pub shape: Vec<u32>,
    pub outputs: Vec<u8>,
    pub sensors: usize,
    pub gens: Vec<Gen>,
    /// The network design now.
    pub arch: Arch,
    /// Designs recorded in the save's snapshots, by generation (ascending).
    pub snapshots: Vec<(usize, Arch)>,
    /// The current network's weights.
    pub weights: Option<Weights>,
    /// Weights in the save's snapshots, by generation (ascending).
    pub snapshot_weights: Vec<(usize, Weights)>,
    /// Output types in neuron order (as saved, not sorted).
    pub output_order: Vec<u8>,
    /// Per sensor, in input order: (angle in degrees, length) for raycasts, None otherwise.
    pub rays: Vec<Option<(f32, f32)>>,
}

/// A network's parameters: `w[l][i][j]` is the weight from neuron `i` of layer `l` to neuron
/// `j` of layer `l + 1`; `b[l][j]` is the bias of neuron `j` of layer `l + 1`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Weights {
    pub w: Vec<Vec<Vec<f32>>>,
    pub b: Vec<Vec<f32>>,
}

impl Weights {
    fn parse(nn: &Value) -> Option<Weights> {
        let num = |x: &Value| x.as_f64().map(|v| v as f32);
        let w = nn
            .get("Weights")?
            .as_array()?
            .iter()
            .map(|l| l.as_array().map(|rows| rows.iter().map(|r| r.as_array().map(|c| c.iter().filter_map(num).collect()).unwrap_or_default()).collect()))
            .collect::<Option<Vec<Vec<Vec<f32>>>>>()?;
        let b = nn
            .get("Biases")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|l| l.as_array().map(|c| c.iter().filter_map(num).collect()).unwrap_or_default()).collect())
            .unwrap_or_default();
        (!w.is_empty()).then_some(Weights { w, b })
    }
    /// Layer sizes implied by the weights.
    pub fn shape(&self) -> Vec<usize> {
        let mut s: Vec<usize> = self.w.iter().map(Vec::len).collect();
        if let Some(last) = self.w.last().and_then(|l| l.first()) {
            s.push(last.len());
        }
        s
    }
}

/// `NeuralNetworkOutputType`, short.
pub const OUTPUTS: [&str; 5] = ["Steer", "Accel", "Handbrake", "Brake", "Boost"];

/// A network design: its inputs (sensors), layers and outputs.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Arch {
    pub shape: Vec<u32>,
    /// Sensor `$type`s in order (e.g. "raycast", "speed", "wheelAngle").
    pub sensors: Vec<String>,
    pub outputs: Vec<u8>,
}

impl Arch {
    fn parse(nn: &Value, sensors: Option<&Value>, outputs: Option<&Value>) -> Arch {
        Arch {
            shape: nn
                .get("Shape")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as u32).collect())
                .unwrap_or_default(),
            sensors: sensors
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|x| x.get("$type").and_then(Value::as_str).unwrap_or("?").to_string()).collect())
                .unwrap_or_default(),
            outputs: outputs
                .and_then(Value::as_array)
                .map(|a| {
                    let mut o: Vec<u8> = a.iter().filter_map(Value::as_u64).map(|x| x as u8).collect();
                    o.sort_unstable();
                    o
                })
                .unwrap_or_default(),
        }
    }
    pub fn raycasts(&self) -> usize {
        self.sensors.iter().filter(|s| *s == "raycast").count()
    }
    /// Sensor types other than raycasts, sorted.
    pub fn extras(&self) -> Vec<String> {
        let mut v: Vec<String> = self.sensors.iter().filter(|s| *s != "raycast").cloned().collect();
        v.sort();
        v.dedup();
        v
    }
    pub fn hidden(&self) -> &[u32] {
        if self.shape.len() >= 2 { &self.shape[1..self.shape.len() - 1] } else { &[] }
    }
    pub fn hidden_neurons(&self) -> u32 {
        self.hidden().iter().sum()
    }
    /// Weights plus biases.
    pub fn params(&self) -> u64 {
        self.shape.windows(2).map(|w| (w[0] as u64 + 1) * w[1] as u64).sum()
    }
    pub fn outputs_label(&self) -> String {
        if self.outputs.is_empty() {
            return "?".into();
        }
        self.outputs.iter().map(|&o| OUTPUTS.get(o as usize).copied().unwrap_or("?")).collect::<Vec<_>>().join(" + ")
    }
    pub fn hidden_label(&self) -> String {
        if self.hidden().is_empty() { "none".into() } else { self.hidden().iter().map(|x| x.to_string()).collect::<Vec<_>>().join("-") }
    }
    /// One line: "9 rays + 7 sensors → 9-6-8-6 → Steer + Accel + Brake + Boost (351 params)"
    pub fn label(&self) -> String {
        format!(
            "{} rays + {} sensors → {} → {} ({} params)",
            self.raycasts(),
            self.sensors.len() - self.raycasts(),
            self.hidden_label(),
            self.outputs_label(),
            self.params()
        )
    }
}

/// "velocitySide" -> "velocity side"
pub fn sensor_name(t: &str) -> String {
    let mut out = String::new();
    for c in t.chars() {
        if c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}
fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

fn parse_gen(v: &Value) -> Gen {
    let mut h = DefaultHasher::new();
    v.to_string().hash(&mut h);
    let mut rewards: Vec<Reward> = v
        .pointer("/Rewards/Ratios")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let value = f(x, "Value")?;
                    (value != 0.0).then(|| Reward {
                        metric: x.pointer("/Model/Metric").and_then(Value::as_u64).unwrap_or(0) as u8,
                        kind: x.pointer("/Model/Type").and_then(Value::as_u64).unwrap_or(0) as u8,
                        value,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    rewards.sort_by(|a, b| (a.metric, a.kind).cmp(&(b.metric, b.kind)).then(a.value.total_cmp(&b.value)));
    Gen {
        settings: Settings {
            mutation: f(v, "MutationRate"),
            selection: u(v, "SelectionAlgorithm").map(|x| x as u8),
            selection_size: u(v, "SelectionSize").map(|x| x as u32),
            crossover: u(v, "CrossoverAlgorithm").map(|x| x as u8),
            weight_decay: f(v, "WeightDecay"),
            elimination: v.get("WasEliminationEnabled").and_then(Value::as_bool),
            population: u(v, "AgentsCount").map(|x| x as u32),
            time_limit: match (u(v, "ActiveCount"), f(v, "Time")) {
                (Some(a), Some(t)) if a > 0 && t > 0.0 => Some((t * 10.0).round() / 10.0),
                _ => None,
            },
        },
        rewards,
        active: u(v, "ActiveCount").map(|x| x as u32),
        time: f(v, "Time"),
        best: f(v, "BestScore"),
        average: f(v, "AverageScore"),
        median: f(v, "MedianScore"),
        q1: f(v, "FirstQuartileScore"),
        q3: f(v, "ThirdQuartileScore"),
        worst: f(v, "WorstScore"),
        best_speed: f(v, "BestAverageSpeed"),
        best_lap: f(v, "BestLapTime").filter(|x| *x > 0.0),
        track: v.get("TrackId").and_then(Value::as_str).map(String::from),
        hash: h.finish(),
    }
}

impl Save {
    pub fn from_bytes(bytes: &[u8]) -> Result<Save> {
        let mut s = String::new();
        GzDecoder::new(bytes).read_to_string(&mut s).context("not a gzip .evolution file")?;
        let v: Value = serde_json::from_str(&s).context("not valid JSON (the game may still be writing it)")?;
        if !v.is_object() {
            return Err(anyhow!("save is not a JSON object"));
        }
        let gens = v
            .pointer("/Stats/Generations")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(parse_gen).collect())
            .unwrap_or_default();
        let nn = v.get("NeuralNetwork").cloned().unwrap_or(Value::Null);
        Ok(Save {
            id: v.get("Id").and_then(Value::as_str).unwrap_or_default().into(),
            name: v.get("Name").and_then(Value::as_str).unwrap_or_default().into(),
            vehicle: u(&v, "Type").map(|x| x as u8),
            shape: nn
                .get("Shape")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as u32).collect())
                .unwrap_or_default(),
            outputs: v
                .get("Outputs")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as u8).collect())
                .unwrap_or_default(),
            sensors: v.get("Sensors").and_then(Value::as_array).map_or(0, Vec::len),
            gens,
            arch: Arch::parse(&nn, v.get("Sensors"), v.get("Outputs")),
            weights: Weights::parse(&nn),
            snapshot_weights: {
                let mut sw: Vec<(usize, Weights)> = v
                    .get("Snapshots")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| Some((x.get("GenerationIndex")?.as_u64()? as usize, Weights::parse(x.get("NeuralNetwork")?)?)))
                            .collect()
                    })
                    .unwrap_or_default();
                sw.sort_by_key(|s| s.0);
                sw
            },
            output_order: v
                .get("Outputs")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as u8).collect())
                .unwrap_or_default(),
            rays: v
                .get("Sensors")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| match (x.get("Degrees").and_then(Value::as_f64), x.get("Length").and_then(Value::as_f64)) {
                            (Some(d), Some(l)) => Some((d as f32, l as f32)),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            snapshots: {
                let mut snaps: Vec<(usize, Arch)> = v
                    .get("Snapshots")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| {
                                let i = x.get("GenerationIndex")?.as_u64()? as usize;
                                let nn = x.get("NeuralNetwork")?;
                                Some((i, Arch::parse(nn, x.get("Sensors"), x.get("OutputTypes"))))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                snaps.sort_by_key(|s| s.0);
                snaps
            },
        })
    }

    /// Opens the file read-only.
    pub fn load(path: &Path) -> Result<Save> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Save::from_bytes(&bytes)
    }

    /// The design in use at generation `i`: the first snapshot at or after it, else the current
    /// network. (Snapshots are every 10–100 gens, so a design change is placed approximately.)
    pub fn arch_at(&self, i: usize) -> &Arch {
        self.snapshots.iter().find(|(g, _)| *g >= i).map_or(&self.arch, |(_, a)| a)
    }

    pub fn shape_label(&self) -> String {
        self.shape.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("-")
    }
}
