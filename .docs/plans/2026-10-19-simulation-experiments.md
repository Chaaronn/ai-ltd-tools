# Built-in simulation experiments for Evolution Lab

Status: implementation in progress; not a completed release.

This records the user-approved implementation plan. The subsequent instruction **“I'll sort
NVIDIA backend myself”** changes packaging ownership: do not download/install or bundle NVIDIA
binaries on the user's behalf. Keep the adapter, verification, fallback and hardware test hooks.
See [progress log](../simulation-implementation-progress.md) for actual implementation and evidence.

## Objective and boundaries

Embed a pinned Rust dependency on `driving_sim_rs` inside our own headless worker. No Drive Lab
UI, external simulator server, or import/export of simulator-trained networks into the game.
Support both experiments initialized from saved weights and fresh-network training campaigns.
Population is **always 1,200**, including controls, alternatives, initialization and resumed jobs.
Simulation evidence stays separate from observed game history, `lab::Library` and its ledger.

## 1. Simulation engine and execution

### Dependency and compatibility

- Pin the inspected upstream implementation to an immutable Git revision; retain the lockfile.
  Record the revision in every experiment. The inspected pin is
  `ab3ec6799f55aaca5fb286e4fef60276b15f333a` (v1.3.0).
- Use upstream physics, sensors, evolution, initialization and checkpoint implementations
  directly. Preserve required attribution and matching vehicle/model resources.
- Enable `server` only for embedding APIs; never instantiate its server.
- Keep upstream conversions behind a `SimulationEngine` adapter. UI and analysis use
  application-owned specifications, progress and result types.
- Preserve ordered sensors, outputs, weights, biases, rewards, elimination and math profile.
  Reject unsupported mappings with an actionable explanation. Never silently substitute a
  different vehicle or sensor.
- Explicitly configure all training settings; do not inherit scratch-training defaults or
  implicit mutation schedules.

### Worker and compute

- Dedicated executable, controlled through versioned local messages, owns the simulator and
  artifacts. Desktop receives progress/results. CLI exposes start, pause, resume, cancel, inspect.
- One active simulation session, internally parallel population. Default threads: logical
  processors minus two, minimum one.
- Auto/CPU/GPU selection. Auto prefers a verified GPU; verify compatibility and CPU/GPU agreement
  before accepting GPU results. Show and record actual backend and fallback reasons.
- Target RTX 4060 Ti through a matching user-installed NVIDIA library; support upstream-compatible
  AMD libraries through the adapter. User owns NVIDIA installation, superseding original bundling.
- Never shrink population for memory pressure. Fall back to CPU when supported; otherwise pause
  with an explanation. Fix math profile at creation and preserve it on resume.

### Persistence and recovery

- Dedicated application-data storage outside game folders and the observed-save scanner.
- Immutable specification, source hashes, effective settings, seed identities, simulator revision,
  generation metrics, checkpoints and summaries.
- Two-hour active-compute budget per campaign session by default; pause resumably when exhausted.
- Bounded tick batches keep stop requests responsive. Replay interrupted generations from their
  start; never publish partial results.
- Checkpoint every completed generation and stage boundary, atomically replacing progress while
  preserving the last valid checkpoint. Interrupted jobs become resumable after app restart.

## 2. Experiment modes

### Track and baseline selection

- Accept supported existing `.track` files, including user-created/workshop tracks, or deterministic
  generated tracks frozen to one geometry.
- Convert using upstream saved-track geometry support; validate spawn, direction, surfaces and
  lap completion. Built-in campaign names alone are insufficient; require usable geometry.
- Default vehicle, architecture, ordered sensors, outputs and training settings to the selected
  save. Fresh starts retain setup, discard weights. Always override population to 1,200.
- Require positive generation time limit: copy effective saved limit if available, otherwise
  require explicit input. Keep it fixed across compared branches.

### Existing-network experiments

- “Simulate” on a supported card prepares control and one alternative from an immutable source copy.
- Build 1,200 members using saved weights and upstream initialization. Copy the complete population
  and RNG checkpoint to both branches before applying the single named setting change.
- Default five paired seed trials and 100 additional generations per branch.
- Label **Experiment initialized from saved weights**, not an exact continuation of the game's
  full live population/RNG state.
- Population suggestions cannot be simulated. Design suggestions open fresh-design workflow.
  Unsupported suggestions remain visible with a reason.

### Fresh-network stage campaigns

Default five independently initialized starting families on the selected track. For each:

1. Initialize fresh weights using upstream behavior.
2. Follow unchanged baseline training configuration.
3. Save branching checkpoints at completed generations **0, 200, 1,000 and 5,000**.
4. Compare control and selected alternatives for 100 generations from each checkpoint.
5. Continue the baseline lineage toward the next checkpoint.

Alternatives never become later baselines. Reuse baseline continuation as control only when
configuration/checkpoint match exactly. Users may select fewer stages or change generation
budget before creation. Unfinished stages remain pending at the session budget boundary.

### Bounded settings exploration

- Candidates from same-vehicle playbook comparisons and observed settings.
- Vary one dimension at a time: mutation, selection, crossover, decay or elimination.
- Exclude population, vehicle, architecture, rewards and time limit from automatic exploration.
- Rank unresolved evidence and relevance to selected stage. Freeze candidates at campaign start;
  default at most three alternatives. Reuse identical controls.
- Specification changes create new experiments, never rewrite completed evidence.

### Nearby design exploration

- Begin with selected design or another existing same-vehicle design. At most three alternatives,
  one feature change each: hidden width, one hidden layer insertion/removal, or non-raycast sensor
  substitution.
- Bound generated designs to 1–6 hidden layers, 2–32 neurons per layer, 10,000 parameters.
- Preserve ray geometry, vehicle and output controls. Replacement sensors must be upstream-supported
  and observed in compatible same-vehicle saves. Validate and report GPU limitations before running.
- Train each from scratch with five matched initialization seed identifiers and identical settings.
  Different architectures receive compatible fresh weights, not identical-population claims.
- Compare at matching endpoints. Label **whole-training design comparisons**, not proof that a
  late-stage architecture change causes improvement.

## 3. Analysis, evidence and desktop UI

### Provenance

Application-owned types cover specs, arms, source provenance, stage checkpoints, progress and
paired results. Record track/input hashes, vehicle/design, effective settings, source family,
initialization seed, ancestry, target measure, budget, revision, math profile, actual backend,
completion status and compatibility/fallback notes.

Repeated branches/resumes/designs sharing a seed block do not add independent families.
Populations derived from one saved network remain one source family. Keep simulation records
out of the observed library and evidence ledger.

### Evaluation

- Lock primary measure before training: suggestion measure or user choice; fresh default score/sec.
  Lap comparisons require completed laps, with no silent score fallback.
- Evaluate frozen populations at endpoints under identical track/spawn/time conditions without
  reproduction. Record lap completion, best lap, median score/sec and survival.
- Pair alternatives with controls; report per-family differences, median improvement, win/tie/loss
  counts and missing data, not comparisons against unrelated historical runs.
- Labels: **Insufficient data**, **Mixed**, **Promising in simulation**.
- Promising requires five evaluable independent families, at least 80% positive paired comparisons,
  and median practical improvement of 1% lap/rate or two percentage points survival.
- These are screening heuristics, not statistical confidence claims. Existing-save trials show
  seed consistency without implying independent source networks. Never automatically promote
  real-game advice to strong evidence.

### Desktop experience

- Data gathering: supported-card Simulate, New simulation campaign with fresh/settings/design
  workflows, review screen for fixed population, track, baseline, alternatives, stages, seeds,
  time limit and compute budget.
- Persistent queue: backend, generation, stage, elapsed time, pause/resume/cancel.
- Result tables/plots comparing alternatives to controls, originating suggestion links and proposed
  real-game confirmation test.
- Measured throughput and estimated remaining work after benchmarking; no guaranteed speedup claim.

## 4. Delivery order

1. Pinned dependency, attribution, worker protocol and CPU smoke test.
2. Save/track adapters, fixed population and deterministic checkpoint/resume.
3. Fresh paired comparisons and isolated result storage.
4. Existing-save initialization and suggestion-card integration.
5. Stage campaigns and bounded settings exploration.
6. Nearby design exploration and family-aware analysis.
7. GPU verification/fallback/performance measurement; NVIDIA installation user-managed.
8. Complete UI, recovery, documentation and end-to-end validation.

## Acceptance checklist

- [ ] Original saves byte-identical; all simulation writes inside experiment storage.
- [ ] Population 1,200 through initialization, branching, settings changes and resume.
- [ ] Identical CPU checkpoint/configuration/seed gives identical supported results.
- [ ] Pause/resume equals uninterrupted execution; failed/cancelled partial generations never publish.
- [ ] Track conversion preserves geometry, spawn direction, surfaces and lap behavior.
- [ ] Saved conversion preserves input/output order and inference.
- [ ] Branches start with identical populations and differ only in requested setting.
- [ ] Stage alternatives cannot change later baseline checkpoints.
- [ ] Missing laps cannot produce score-based lap wins.
- [ ] Resumes/repeated branches cannot inflate independence.
- [ ] Generated designs bounded; incompatible sensors fail clearly.
- [ ] GPU hardware test validates supported installed path; CPU-only runs explicitly skip it.
- [ ] Full 1,200-member smoke campaign through GUI and CLI without blocking UI.
- [ ] README, Evolution Lab docs and spec explain semantics, accounting, track requirements,
  controls, provenance and simulation-versus-game distinction.

Checklist items remain unchecked here until the final acceptance audit; partial proof and exact
test outcomes are maintained in the progress log. Release completion requires both modes running
end-to-end, reproducible interruption recovery and untouched playable networks. Do not describe
the whole release as complete merely because the CPU core passes tests.
