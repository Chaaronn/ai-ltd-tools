use super::*;
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}
#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    checksum: String,
    spec: JobSpec,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Control {
    Pause,
    Cancel,
}
#[derive(Serialize, Deserialize)]
struct ControlMessage {
    version: u32,
    control: Control,
}
impl Store {
    pub fn default_location() -> Result<Self> {
        Ok(Self {
            root: dirs::data_local_dir()
                .context("No application data directory")?
                .join("CatsAiLtdTools")
                .join("simulation-v1"),
        })
    }
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn job(&self, id: &str) -> Result<PathBuf> {
        uuid::Uuid::parse_str(id).context("Invalid simulation job ID")?;
        Ok(self.root.join(id))
    }
    pub fn create(&self, spec: &JobSpec) -> Result<String> {
        spec.validate()?;
        fs::create_dir_all(&self.root)?;
        let id = uuid::Uuid::new_v4().to_string();
        let dir = self.job(&id)?;
        fs::create_dir(&dir)?;
        let envelope = Envelope {
            version: VERSION,
            checksum: hash(&serde_json::to_vec(spec)?),
            spec: spec.clone(),
        };
        atomic_json(&dir.join("spec.json"), &envelope)?;
        atomic_json(&dir.join("progress.json"), &Progress::default())?;
        Ok(id)
    }
    pub fn spec(&self, id: &str) -> Result<JobSpec> {
        let e: Envelope = serde_json::from_slice(&fs::read(self.job(id)?.join("spec.json"))?)?;
        ensure!(
            e.version == VERSION && hash(&serde_json::to_vec(&e.spec)?) == e.checksum,
            "Campaign specification integrity failure"
        );
        e.spec.validate()?;
        Ok(e.spec)
    }
    pub fn progress(&self, id: &str) -> Result<Progress> {
        let p: Progress = serde_json::from_slice(&fs::read(self.job(id)?.join("progress.json"))?)?;
        ensure!(p.version == VERSION, "Unsupported progress version");
        Ok(p)
    }
    pub fn save_progress(&self, id: &str, p: &Progress) -> Result<()> {
        atomic_json(&self.job(id)?.join("progress.json"), p)
    }
    pub fn list(&self) -> Result<Vec<String>> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut out = vec![];
        for e in fs::read_dir(&self.root)? {
            let e = e?;
            let id = e.file_name().to_string_lossy().to_string();
            if uuid::Uuid::parse_str(&id).is_ok() && e.path().join("spec.json").is_file() {
                out.push(id);
            }
        }
        out.sort();
        Ok(out)
    }
    /// When the job was created (its specification file's time), for ordering the queue.
    pub fn created(&self, id: &str) -> Option<std::time::SystemTime> {
        fs::metadata(self.job(id).ok()?.join("spec.json")).ok()?.modified().ok()
    }
    /// Delete a finished job's folder (specification, progress, checkpoints, results).
    /// Refuses jobs that could still run: only completed, cancelled or failed ones go.
    pub fn remove(&self, id: &str) -> Result<()> {
        let dir = self.job(id)?;
        ensure!(dir.parent() == Some(self.root.as_path()), "Refusing to remove {}", dir.display());
        let p = self.progress(id)?;
        ensure!(
            matches!(p.status, Status::Completed | Status::Cancelled | Status::Failed),
            "Only completed, cancelled or failed jobs can be removed; pause and cancel it first"
        );
        ensure!(!self.active_job(id), "This job's worker is still running");
        fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))
    }
    pub fn lock(&self) -> Result<File> {
        fs::create_dir_all(&self.root)?;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("worker.lock"))?;
        f.try_lock_exclusive()
            .context("Another simulation worker is running")?;
        Ok(f)
    }
    pub fn active(&self) -> bool {
        self.lock().is_err()
    }
    pub fn active_job(&self, id: &str) -> bool {
        self.active()
            && fs::read(self.root.join("active.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<String>(&b).ok())
                .as_deref()
                == Some(id)
    }
    /// Keep the durable current generation and all immutable branch boundaries.
    /// Called only after progress is atomically committed by the exclusive worker.
    pub fn prune_checkpoints(&self, id: &str, p: &Progress) -> Result<()> {
        let dir = self.job(id)?;
        let mut keep = std::collections::HashSet::new();
        if let Some(cp) = &p.checkpoint {
            keep.insert(cp.clone());
        }
        for e in fs::read_dir(&dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("boundary-") && name.ends_with(".json") {
                keep.insert(serde_json::from_slice::<String>(&fs::read(e.path())?)?);
            }
        }
        let checkpoints = dir.join("checkpoints");
        if checkpoints.exists() {
            for e in fs::read_dir(checkpoints)? {
                let e = e?;
                let path = e.path();
                if path.extension().is_some_and(|e| e == "bin") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        if name.len() == 64
                            && name.bytes().all(|b| b.is_ascii_hexdigit())
                            && !keep.contains(name)
                        {
                            fs::remove_file(path)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn control(&self, id: &str, control: Control) -> Result<()> {
        let p = self.progress(id)?;
        if p.status == Status::Completed || p.status == Status::Cancelled {
            return Ok(());
        }
        atomic_json(
            &self.job(id)?.join("control.json"),
            &ControlMessage {
                version: VERSION,
                control,
            },
        )?;
        if let Ok(_guard) = self.lock() {
            // The worker may have committed a newer generation while the control was written.
            let mut p = self.progress(id)?;
            if matches!(p.status, Status::Completed | Status::Cancelled) {
                return Ok(());
            }
            p.status = if control == Control::Cancel {
                Status::Cancelled
            } else {
                Status::Paused
            };
            self.save_progress(id, &p)?;
        }
        Ok(())
    }
    pub fn requested(&self, id: &str) -> Result<Option<Control>> {
        let path = self.job(id)?.join("control.json");
        if !path.exists() {
            return Ok(None);
        }
        let m: ControlMessage = serde_json::from_slice(&fs::read(path)?)?;
        ensure!(m.version == VERSION, "Unsupported control protocol");
        Ok(Some(m.control))
    }
    pub fn clear_control(&self, id: &str) -> Result<()> {
        let path = self.job(id)?.join("control.json");
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
    pub fn checkpoint(&self, id: &str, bytes: &[u8]) -> Result<String> {
        let name = hash(bytes);
        let dir = self.job(id)?.join("checkpoints");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{name}.bin"));
        if !path.exists() {
            atomic(&path, bytes)?;
        }
        Ok(name)
    }
    pub fn read_checkpoint(&self, id: &str, name: &str) -> Result<Vec<u8>> {
        ensure!(
            name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid checkpoint ID"
        );
        let bytes = fs::read(
            self.job(id)?
                .join("checkpoints")
                .join(format!("{name}.bin")),
        )?;
        ensure!(hash(&bytes) == name, "Checkpoint hash mismatch");
        Ok(bytes)
    }
    pub fn boundary(&self, id: &str, seed: i64, stage: u64, checkpoint: &str) -> Result<()> {
        atomic_json(
            &self.job(id)?.join(format!("boundary-{seed}-{stage}.json")),
            &checkpoint,
        )
    }
    pub fn read_boundary(&self, id: &str, seed: i64, stage: u64) -> Result<String> {
        Ok(serde_json::from_slice(&fs::read(
            self.job(id)?.join(format!("boundary-{seed}-{stage}.json")),
        )?)?)
    }
    pub fn record(&self, id: &str, task: usize, generation: u64, metrics: &Metrics) -> Result<()> {
        let dir = self.job(id)?.join("metrics");
        fs::create_dir_all(&dir)?;
        atomic_json(&dir.join(format!("{task}-{generation}.json")), metrics)
    }
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic(path, &serde_json::to_vec_pretty(value)?)
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        Ok::<_, anyhow::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
