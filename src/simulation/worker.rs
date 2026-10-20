use super::{
    engine::SimulationEngine,
    storage::{Control, Store},
    *,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    process::{Child, Command, Stdio},
    time::Instant,
};

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub job: String,
}
#[derive(Clone, Debug)]
pub struct Task {
    pub seed: i64,
    pub arm: usize,
    pub start: u64,
    pub end: u64,
    pub baseline: bool,
}
pub fn tasks(spec: &JobSpec) -> Vec<Task> {
    let mut out = vec![];
    let end = spec.stages.last().copied().unwrap_or(0) + spec.generations;
    for seed in &spec.seeds {
        if spec.mode == CampaignMode::Designs {
            out.push(Task {
                seed: *seed,
                arm: 0,
                start: 0,
                end,
                baseline: true,
            });
            for arm in 1..=spec.alternatives.len() {
                out.push(Task {
                    seed: *seed,
                    arm,
                    start: 0,
                    end,
                    baseline: false,
                });
            }
        } else {
            let mut baseline_start = 0;
            for stage in &spec.stages {
                let end = stage + spec.generations;
                out.push(Task {
                    seed: *seed,
                    arm: 0,
                    start: baseline_start,
                    end,
                    baseline: true,
                });
                for arm in 1..=spec.alternatives.len() {
                    out.push(Task {
                        seed: *seed,
                        arm,
                        start: *stage,
                        end,
                        baseline: false,
                    });
                }
                baseline_start = end;
            }
        }
    }
    out
}
pub fn spawn(store: &Store, id: &str) -> Result<Child> {
    store.spec(id)?;
    ensure!(
        !store.active(),
        "Another simulation session is active; pause it first"
    );
    let exe = std::env::current_exe()?.with_file_name(if cfg!(windows) {
        "simulation-worker.exe"
    } else {
        "simulation-worker"
    });
    ensure!(
        exe.is_file(),
        "Build/package simulation-worker beside the application (cargo build --bins)"
    );
    let mut cmd = Command::new(exe);
    cmd.env("CATS_SIMULATION_ROOT", &store.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    // The user's ALTD_GPU_LIB wins. Otherwise use an optional library beside the executable.
    let mut child = cmd.spawn()?;
    serde_json::to_writer(
        child.stdin.as_mut().context("Worker input unavailable")?,
        &Request {
            version: VERSION,
            job: id.into(),
        },
    )?;
    child.stdin.as_mut().unwrap().write_all(b"\n")?;
    child.stdin.take();
    Ok(child)
}
pub fn run(store: &Store, id: &str) -> Result<Progress> {
    let _guard = store.lock()?;
    storage::atomic_json(&store.root.join("active.json"), &id)?;
    let spec = store.spec(id)?;
    let mut p = store.progress(id)?;
    ensure!(
        !matches!(p.status, Status::Cancelled | Status::Completed),
        "Job is already terminal"
    );
    if store.requested(id)? == Some(Control::Cancel) {
        p.status = Status::Cancelled;
        store.save_progress(id, &p)?;
        return Ok(p);
    }
    store.clear_control(id)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(spec.threads)
        .build()?;
    let start = Instant::now();
    let elapsed_before = p.elapsed_seconds;
    p.status = Status::Running;
    p.error = None;
    store.save_progress(id, &p)?;
    let result = pool.install(|| execute(store, id, &spec, &mut p, start, elapsed_before));
    p.elapsed_seconds = elapsed_before + start.elapsed().as_secs_f64();
    if result.is_err() || p.status != Status::Completed {
        // Generations after the last checkpoint weren't written: resume from what's on disk.
        if let Ok(saved) = store.progress(id) {
            p.task = saved.task;
            p.generation = saved.generation;
            p.checkpoint = saved.checkpoint;
            p.completed_generations = saved.completed_generations;
            p.observations = saved.observations;
        }
    }
    if let Err(error) = result {
        p.status = Status::Failed;
        p.error = Some(format!("{error:#}"));
        store.save_progress(id, &p)?;
        return Err(error);
    }
    store.save_progress(id, &p)?;
    storage::atomic_json(
        &store.job(id)?.join("results.json"),
        &analysis::comparisons(&spec, &p.observations),
    )?;
    Ok(p)
}
/// Longest stretch between checkpoints while training (seconds of wall time).
pub const CHECKPOINT_SECONDS: f64 = 15.0;

/// Pause/cancel/budget check, reading the control file at most every quarter second.
#[derive(Default)]
struct StopCheck {
    last: Option<(Instant, bool)>,
}
impl StopCheck {
    fn check(&mut self, store: &Store, id: &str, start: Instant, budget: u64) -> bool {
        if start.elapsed().as_secs() >= budget {
            return true;
        }
        if let Some((at, answer)) = self.last
            && at.elapsed().as_secs_f64() < 0.25
        {
            return answer;
        }
        let answer = store.requested(id).ok().flatten().is_some();
        self.last = Some((Instant::now(), answer));
        answer
    }
}

fn execute(
    store: &Store,
    id: &str,
    spec: &JobSpec,
    p: &mut Progress,
    start: Instant,
    elapsed_before: f64,
) -> Result<()> {
    let work = tasks(spec);
    let mut stop = StopCheck::default();
    let mut last_write = Instant::now();
    let mut pending: Vec<(usize, u64, Metrics)> = vec![];
    p.perf = None;
    while p.task < work.len() {
        let task = &work[p.task];
        let arm = task.arm.checked_sub(1).map(|i| &spec.alternatives[i]);
        let input = arm.and_then(|a| a.design.as_ref()).unwrap_or(&spec.input);
        let settings = arm.map(|a| &a.settings).unwrap_or(&spec.baseline);
        // Start/restore with the settings that created the checkpoint, then apply the tested change.
        let creating = if p.checkpoint.is_some() {
            settings
        } else {
            &spec.baseline
        };
        let mut engine = SimulationEngine::new(spec, input, creating, task.seed)?;
        if let Some(cp) = &p.checkpoint {
            engine.restore(&store.read_checkpoint(id, cp)?)?;
        } else {
            if (task.baseline && task.start > 0)
                || (!task.baseline && spec.mode != CampaignMode::Designs)
            {
                let cp = store.read_boundary(id, task.seed, task.start)?;
                engine.restore(&store.read_checkpoint(id, &cp)?)?;
            } else {
                engine.start(spec.mode == CampaignMode::SavedWeights, input)?;
            }
            p.generation = task.start;
            p.checkpoint = Some(store.checkpoint(id, &engine.checkpoint()?)?);
            store.save_progress(id, p)?;
        }
        engine.settings(settings)?;
        // Elimination is fixed at Session construction; alternate elimination requires a fresh
        // session with the same population checkpoint but its own physical elimination rules.
        if creating.eliminate_on_wall != settings.eliminate_on_wall
            || creating.eliminate_when_idle != settings.eliminate_when_idle
        {
            let bytes = engine.checkpoint()?;
            drop(engine);
            engine = SimulationEngine::new(spec, input, settings, task.seed)?;
            engine.restore(&bytes)?;
            engine.settings(settings)?;
        }
        loop {
            let requested = store.requested(id)?;
            if requested.is_some() || start.elapsed().as_secs() >= spec.budget_seconds {
                p.status = if requested == Some(Control::Cancel) {
                    Status::Cancelled
                } else {
                    Status::Paused
                };
                return Ok(());
            }
            if task.baseline && (spec.stages.contains(&p.generation) || p.generation == task.end) {
                store.boundary(id, task.seed, p.generation, p.checkpoint.as_ref().unwrap())?;
            }
            let endpoint = if spec.mode == CampaignMode::Designs || task.baseline {
                spec.stages
                    .iter()
                    .copied()
                    .find(|s| s + spec.generations == p.generation)
            } else {
                (p.generation == task.end).then_some(task.start)
            };
            if let Some(stage) = endpoint {
                if !p
                    .observations
                    .iter()
                    .any(|o| o.seed == task.seed && o.arm == task.arm && o.stage == stage)
                {
                    if !engine.drive(|| {
                        store.requested(id).ok().flatten().is_some()
                            || start.elapsed().as_secs() >= spec.budget_seconds
                    })? {
                        continue;
                    }
                    let metrics = engine.metrics()?;
                    let (backend, note) = engine.backend();
                    p.observations.push(Observation {
                        task: p.task,
                        seed: task.seed,
                        family: if spec.mode == CampaignMode::SavedWeights {
                            spec.input.source_family.clone()
                        } else {
                            format!("seed:{}", task.seed)
                        },
                        stage,
                        arm: task.arm,
                        metrics,
                        backend: backend.clone(),
                        note: note.clone(),
                    });
                    p.backend = backend;
                    p.note = note;
                    p.elapsed_seconds = elapsed_before + start.elapsed().as_secs_f64();
                    store.save_progress(id, p)?;
                    // Frozen evaluation is discarded; continuation starts from the same boundary/RNG.
                    engine.restore(&store.read_checkpoint(id, p.checkpoint.as_ref().unwrap())?)?;
                    engine.settings(settings)?;
                }
            }
            if p.generation >= task.end {
                break;
            }
            let t0 = Instant::now();
            if !engine.drive(|| stop.check(store, id, start, spec.budget_seconds))? {
                continue;
            }
            let ticks = engine.ticks() as f64;
            let metrics = engine.metrics()?;
            let t1 = Instant::now();
            let (backend, note) = engine.backend();
            engine.reproduce()?;
            let t2 = Instant::now();
            p.generation += 1;
            p.completed_generations += 1;
            p.backend = backend;
            p.note = note;
            pending.push((p.task, p.generation, metrics));
            // A checkpoint is written when something needs this exact generation (a stage
            // boundary, an evaluation point, the end of the task), when the user may stop it,
            // and otherwise every CHECKPOINT_SECONDS. Between writes the progress on disk stays at
            // the last checkpoint, so an interruption replays at most that many seconds.
            let g = p.generation;
            let exact = g >= task.end
                || (task.baseline && spec.stages.contains(&g))
                || (spec.mode == CampaignMode::Designs || task.baseline) && spec.stages.iter().any(|s| s + spec.generations == g);
            let due = exact
                || last_write.elapsed().as_secs_f64() >= CHECKPOINT_SECONDS
                || stop.check(store, id, start, spec.budget_seconds);
            if due {
                p.checkpoint = Some(store.checkpoint(id, &engine.checkpoint()?)?);
                for (task, generation, m) in pending.drain(..) {
                    store.record(id, task, generation, &m)?;
                }
                p.elapsed_seconds = elapsed_before + start.elapsed().as_secs_f64();
                store.save_progress(id, p)?;
                store.prune_checkpoints(id, p)?;
                last_write = Instant::now();
            }
            let t3 = Instant::now();
            // (these timings reach the queue display with the next checkpoint write)
            p.perf.get_or_insert_with(Perf::default).add(
                ticks,
                (t1 - t0).as_secs_f64(),
                (t2 - t1).as_secs_f64(),
                (t3 - t2).as_secs_f64(),
                due,
            );
        }
        p.task += 1;
        p.generation = 0;
        p.checkpoint = None;
        store.save_progress(id, p)?;
    }
    p.status = Status::Completed;
    Ok(())
}
