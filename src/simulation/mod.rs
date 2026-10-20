//! Isolated simulation evidence. Never writes game saves or feeds the observed Lab library.
pub mod analysis;
pub mod cli;
pub mod engine;
pub mod planner;
pub mod storage;
pub mod worker;

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const POPULATION: usize = 1200;
pub const VERSION: u32 = 1;
pub const ENGINE_REVISION: &str = "ab3ec6799f55aaca5fb286e4fef60276b15f333a";
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CampaignMode {
    Fresh,
    SavedWeights,
    Designs,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backend {
    Auto,
    Cpu,
    Gpu,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Measure {
    Lap,
    Rate,
    Survival,
}
impl Measure {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lap => "Lap time",
            Self::Rate => "Score/sec",
            Self::Survival => "Survival",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reward {
    pub metric: String,
    pub weight: i64,
    #[serde(rename = "type")]
    pub kind: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub population: usize,
    pub selection_algorithm: String,
    pub selection_size: usize,
    pub crossover: String,
    pub mutation_rate: f64,
    pub adaptive_mutation: bool,
    pub weight_decay: f64,
    pub preserve_parents: String,
    pub preserve_parents_size: usize,
    pub rewards: Vec<Reward>,
    pub eliminate_on_wall: bool,
    pub eliminate_when_idle: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            population: POPULATION,
            selection_algorithm: "best".into(),
            selection_size: 3,
            crossover: "none".into(),
            mutation_rate: 0.2,
            adaptive_mutation: false,
            weight_decay: 0.0005,
            preserve_parents: "on_selection_size".into(),
            preserve_parents_size: 3,
            rewards: vec![Reward {
                metric: "total_score".into(),
                weight: 100,
                kind: "default".into(),
            }],
            eliminate_on_wall: false,
            eliminate_when_idle: false,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.population == POPULATION,
            "Population must remain {POPULATION}"
        );
        ensure!(
            (1..=POPULATION).contains(&self.selection_size),
            "Invalid selection size"
        );
        ensure!(
            self.preserve_parents_size <= POPULATION,
            "Invalid preserved parent count"
        );
        ensure!(
            self.mutation_rate.is_finite() && (0.0..=10.0).contains(&self.mutation_rate),
            "Invalid mutation rate"
        );
        ensure!(
            self.weight_decay.is_finite() && (0.0..=1.0).contains(&self.weight_decay),
            "Invalid weight decay"
        );
        ensure!(
            ["best", "tournament", "roulette"].contains(&self.selection_algorithm.as_str()),
            "Unsupported selection"
        );
        ensure!(
            ["none", "single_point", "uniform"].contains(&self.crossover.as_str()),
            "Unsupported crossover"
        );
        ensure!(
            ["off", "on_selection_size", "on_custom"].contains(&self.preserve_parents.as_str()),
            "Unsupported preservation mode"
        );
        ensure!(!self.rewards.is_empty(), "At least one reward is required");
        for r in &self.rewards {
            ensure!(
                altd_sim::training::evolution::METRIC_NAMES.contains(&r.metric.as_str())
                    && ["default", "average"].contains(&r.kind.as_str()),
                "Unsupported reward"
            );
        }
        Ok(())
    }
    pub(crate) fn evolution(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap();
        v.as_object_mut().unwrap().remove("eliminate_on_wall");
        v.as_object_mut().unwrap().remove("eliminate_when_idle");
        v
    }
    pub fn difference(&self, other: &Self) -> Vec<String> {
        let a = serde_json::to_value(self).unwrap();
        let b = serde_json::to_value(other).unwrap();
        a.as_object()
            .unwrap()
            .iter()
            .filter(|(k, v)| b.get(*k) != Some(*v))
            .map(|(k, _)| k.clone())
            .collect()
    }
}

/// Ordered and full-precision inputs; only the engine adapter interprets these documents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedInput {
    pub source_name: String,
    pub source_hash: String,
    pub source_family: String,
    pub track_name: String,
    pub track_hash: String,
    pub vehicle: u8,
    pub notes: Vec<String>,
    pub(crate) network: Value,
    pub(crate) model: Value,
    pub(crate) scene: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Arm {
    pub name: String,
    pub settings: Settings,
    /// Present only for a fresh design experiment.
    pub design: Option<PreparedInput>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    pub version: u32,
    pub engine_revision: String,
    pub title: String,
    pub mode: CampaignMode,
    pub population: usize,
    pub input: PreparedInput,
    pub baseline: Settings,
    pub alternatives: Vec<Arm>,
    pub stages: Vec<u64>,
    pub seeds: Vec<i64>,
    pub generations: u64,
    pub time_limit_seconds: f64,
    pub budget_seconds: u64,
    pub threads: usize,
    pub backend: Backend,
    pub math_profile: String,
    pub measure: Measure,
    pub suggestion_id: Option<String>,
}
impl JobSpec {
    pub fn new(input: PreparedInput, baseline: Settings, time_limit_seconds: f64) -> Self {
        Self {
            version: VERSION,
            engine_revision: ENGINE_REVISION.into(),
            title: "Fresh-network campaign".into(),
            mode: CampaignMode::Fresh,
            population: POPULATION,
            input,
            baseline,
            alternatives: vec![],
            stages: vec![0, 200, 1000, 5000],
            seeds: vec![1729, 1730, 1731, 1732, 1733],
            generations: 100,
            time_limit_seconds,
            budget_seconds: 7200,
            threads: std::thread::available_parallelism()
                .map_or(1, |n| n.get().saturating_sub(2).max(1)),
            backend: Backend::Auto,
            math_profile: altd_sim::math::profile::MathProfile::detect().to_string(),
            measure: Measure::Rate,
            suggestion_id: None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == VERSION && self.engine_revision == ENGINE_REVISION,
            "Unsupported campaign or simulator revision"
        );
        ensure!(
            self.population == POPULATION,
            "Population must remain {POPULATION}"
        );
        ensure!(
            self.time_limit_seconds.is_finite()
                && self.time_limit_seconds > 0.0
                && self.time_limit_seconds <= 86400.0,
            "Supply a positive time limit, at most 24 hours"
        );
        ensure!(
            self.generations > 0
                && self.generations <= 100_000
                && self.threads > 0
                && self.budget_seconds > 0,
            "Invalid compute budget"
        );
        ensure!(
            !self.stages.is_empty()
                && self.stages.windows(2).all(|w| w[0] < w[1])
                && self.stages.iter().all(|s| *s <= 100_000),
            "Stages must be sorted and unique"
        );
        ensure!(
            !self.seeds.is_empty()
                && self.seeds.len() <= 100
                && self
                    .seeds
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == self.seeds.len(),
            "Choose 1–100 unique seeds"
        );
        ensure!(
            self.mode != CampaignMode::SavedWeights || self.stages == [0],
            "Saved-weight experiments start at stage 0"
        );
        ensure!(
            !self.alternatives.is_empty() && self.alternatives.len() <= 3,
            "Choose 1–3 alternatives"
        );
        self.baseline.validate()?;
        self.math_profile
            .parse::<altd_sim::math::profile::MathProfile>()
            .map_err(anyhow::Error::msg)?;
        engine::validate_input(&self.input)?;
        let mut names = std::collections::BTreeSet::new();
        for arm in &self.alternatives {
            ensure!(
                !arm.name.is_empty() && names.insert(&arm.name),
                "Alternative names must be unique"
            );
            arm.settings.validate()?;
            if self.mode == CampaignMode::Designs {
                ensure!(
                    arm.settings == self.baseline,
                    "Design experiments must preserve training settings"
                );
                let input = arm
                    .design
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Missing alternative design"))?;
                engine::validate_input(input)?;
                planner::validate_design_change(&self.input, input)?;
            } else {
                ensure!(
                    arm.design.is_none(),
                    "Settings experiments cannot change design"
                );
                let diff = self.baseline.difference(&arm.settings);
                ensure!(
                    diff.len() == 1
                        && [
                            "mutation_rate",
                            "selection_algorithm",
                            "selection_size",
                            "crossover",
                            "weight_decay",
                            "eliminate_on_wall",
                            "eliminate_when_idle"
                        ]
                        .contains(&diff[0].as_str()),
                    "Each arm must change exactly one supported setting"
                );
            }
        }
        if self.mode == CampaignMode::SavedWeights && self.input.network.get("weights").is_none() {
            bail!("Saved-weight experiments need a weighted save");
        }
        Ok(())
    }
    pub fn smoke() -> Result<Self> {
        let input = engine::preset(1, 1729)?;
        let mut s = Self::new(input, Settings::default(), 0.2);
        s.backend = Backend::Cpu;
        s.seeds = vec![1729];
        s.stages = vec![0];
        s.generations = 1;
        let mut alternative = s.baseline.clone();
        alternative.mutation_rate = 0.1;
        s.alternatives.push(Arm {
            name: "Mutation 0.1".into(),
            settings: alternative,
            design: None,
        });
        Ok(s)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub lap: Option<f64>,
    pub rate: Option<f64>,
    pub survival: f64,
    pub lapped: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub task: usize,
    pub seed: i64,
    pub family: String,
    pub stage: u64,
    pub arm: usize,
    pub metrics: Metrics,
    pub backend: String,
    pub note: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Running,
    Paused,
    Cancelled,
    Failed,
    Completed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Progress {
    pub version: u32,
    pub status: Status,
    pub task: usize,
    pub generation: u64,
    pub checkpoint: Option<String>,
    pub elapsed_seconds: f64,
    pub completed_generations: u64,
    pub backend: String,
    pub note: Option<String>,
    pub observations: Vec<Observation>,
    pub error: Option<String>,
}
impl Default for Progress {
    fn default() -> Self {
        Self {
            version: VERSION,
            status: Status::Queued,
            task: 0,
            generation: 0,
            checkpoint: None,
            elapsed_seconds: 0.0,
            completed_generations: 0,
            backend: "pending".into(),
            note: None,
            observations: vec![],
            error: None,
        }
    }
}
