# Built-in simulation experiments

Evolution Lab runs the pinned `altd_sim` library from
[driving_sim_rs](https://github.com/Zorro909/driving_sim_rs), revision
`ab3ec6799f55aaca5fb286e4fef60276b15f333a`, in `simulation-worker`. The upstream `server`
feature exposes embedding methods; this application never starts its server. Matching vehicle,
model, network and generation resources and their licenses are in `assets/simulation`.
The dependency and all resolved dependencies are pinned in `Cargo.lock`.

## Desktop workflow

Build with `cargo build --release --bins` and keep the worker beside the desktop executable.
Open **Evolution Lab → Simulation → New simulation campaign** (its own tab; Data gathering cards' **Simulate** buttons open it too). Select a saved setup or
the rally preset, choose the mode, and supply an existing `.track` path (including workshop or
user-created files; quotes from Explorer's "Copy as path" are stripped) or explicitly select deterministic generated geometry. A campaign name alone
does not supply track geometry. The generated course is frozen in the specification.

Prepare the review, choose the primary measure, positive generation time limit, stages, seed
count, comparison generations, CPU threads, backend and session budget, and select at most
three alternatives. Every baseline setting (mutation, adaptive mutation, selection and size, crossover,
preserve parents and size, weight decay, elimination) is editable; alternatives keep their one
change and follow the rest of an edited baseline. The review shows the total generations and,
once a job has run on this computer, an estimated time, with each stage's share and a button to
leave it out. Review complete baseline settings and provenance before creating the job.
Jobs are listed newest first, one row each (status, campaign, progress, time left); the selected
job opens below with its start/pause/cancel controls, key numbers and a results table (alternative,
stage, verdict, median, seeds or families better), with the explanation behind **Details**.
Completed, cancelled and failed jobs can be removed one at a time or with **Clear finished**, which
deletes their folders from the simulation store (running, paused and queued jobs can't be removed).
Data gathering cards show the results of campaigns started from them, with **View results**.
Start jobs from the job list. Physics and reproduction run in the worker,
while queue snapshots are read on a background thread. Supported setting cards offer **Simulate**;
unsupported cards explain why they cannot use that protocol.

All arms always contain **1,200** members. Population is never reduced for memory pressure.
Saved files are read once into an immutable specification: later game saves do not change the job.
Ordered sensors, outputs and full-precision parameters are preserved. A save's `Outputs` are the
game's `NeuralNetworkOutputType` indices: 0 steering, 1 acceleration, 2 handbrake, 3 brake,
4 boost (read from AILearnsToDrive.dll; the simulator's own templates use the same order). Unknown
mappings fail with an explanation. Grip sensors with an empty `PositionOffset` get `(0,0)`: the game
always creates them at `Vector2.Zero` and its JSON writer never saves the offset, so every save
has `{}`. Saves don't record adaptive mutation, parent preservation (mode and size) or idle
elimination; these come from the game's `settings/local.settings` next to the evolution folder
(gzipped JSON; `PreserveParentsMode` 0 off, 1 the selected parents, 2 a custom number), with a note.
If that file can't be read the defaults are off, selection-size preservation and off, also noted.

## Experiment semantics

- **Saved weights:** five paired seed trials by default, with 100 additional generations per arm.
  Control and alternative restore the same population/RNG checkpoint and change one setting.
  This is an **experiment initialized from saved weights**, not an exact continuation of the
  game's live population. All seeds remain one source family.
- **Fresh training / settings exploration:** five fresh initialization families by default.
  Baseline checkpoints at completed generations 0, 200, 1,000 and 5,000 support 100-generation
  paired branches. Baseline continuation supplies the control where it matches. Alternatives
  cannot replace or modify the later baseline. Stages and budgets can be reduced before creation.
  Candidate settings come from compatible same-vehicle observations; unresolved playbook
  dimensions are ranked first. The selected list is frozen with the job.
- **Design exploration:** nearby hidden-layer width, insertion/removal, or observed compatible
  non-raycast sensor substitutions. Generated designs have 1–6 hidden layers, 2–32 neurons per
  hidden layer and at most 10,000 parameters. Vehicle, ray geometry and outputs stay fixed.
  Each design starts with fresh weights under matched seed identifiers and identical settings.
  These are **whole-training comparisons**, not evidence of a late-stage architecture intervention.

Endpoints evaluate frozen populations under the same conditions, without reproduction. Measures
are best completed lap, median score divided by generation elapsed seconds, and active share.
Lap comparisons pair seeds where both sides completed a lap. A seed where only the alternative
laps counts as a win, and only the control as a loss; these have no size, so they're shown
separately and left out of the median. Missing laps never become score wins.
Results show paired differences, family medians, win/tie/loss counts, missing pairs, backend
and compatibility notes. Repeated/resumed observations deduplicate by seed, stage and arm.

**Promising in simulation** requires at least five evaluable independent families, improvements
in 80% or more of family comparisons and median improvement of at least 1% for lap/score rate or
two percentage points for survival. Otherwise labels are **Mixed** or **Insufficient data**.
These are screening heuristics, not statistical confidence claims. Saved-weights seeds all start
from one network, so they're one family and that label stays **Insufficient data**; their result is
the seed label instead: **Consistently better/worse across seeds** (80%+ of seeds one way and a
median of at least the practical size), **Mixed across seeds**, or too few seeds, with "x of y seeds
better". That describes consistency for this network, not across networks. Simulation never upgrades real-game advice automatically,
and never enters `lab::Library` or its evidence ledger. Confirm promising changes in the game.

## Compute and recovery

Default threads: logical processors minus two, minimum one. Default session budget: 7,200 seconds
of active worker computation; waiting between sessions is excluded. Each resume receives another
session budget. Only one session can hold the storage lock. Bounded tick batches check control
requests. An interrupted generation replays from its start and publishes no partial metrics.
Generation progress and checkpoints use atomic replacement, with checksum validation. Current
generation and stage/continuation checkpoints remain; superseded non-boundary checkpoints are pruned.
Interrupted jobs appear resumable after restarting. Cancelled and completed jobs are terminal.
Changing the immutable specification requires a new job.

The default directory is `%LOCALAPPDATA%\CatsAiLtdTools\simulation-v1`, outside game save folders.
It stores immutable specifications (input documents, SHA256 hashes, revision, math profile,
effective settings, source family, seeds and stages), progress, generation metrics, checkpoints
and result summaries. Do not delete this directory if you want to resume campaigns.
`CATS_SIMULATION_ROOT` selects an isolated directory for CLI/testing.

Auto and GPU request the upstream compatible backend and use its CPU/GPU agreement checks.
Missing or incompatible backends fall back to CPU with a recorded reason. The user installs
the matching NVIDIA or AMD library and sets `ALTD_GPU_LIB` if needed; this repository does not
download or bundle NVIDIA binaries. Math profile is resolved at creation and fixed on resume.
Throughput and remaining time are estimates from completed work, not a guaranteed speedup.

## CLI

```powershell
cargo build --release --bins
target/release/track-builder-cli simulation smoke
target/release/track-builder-cli simulation prepare --save "network.evolution" --track "course.track" --mode saved --mutation 0.1 > campaign.json
target/release/track-builder-cli simulation prepare --generated-seed 1729 --mode fresh --time-limit 60 --mutation 0.1 > campaign.json
target/release/track-builder-cli simulation create campaign.json
target/release/track-builder-cli simulation start JOB-ID
target/release/track-builder-cli simulation pause JOB-ID
target/release/track-builder-cli simulation resume JOB-ID
target/release/track-builder-cli simulation inspect JOB-ID
target/release/track-builder-cli simulation cancel JOB-ID
target/release/track-builder-cli simulation list
```

`start` and `resume` run in the foreground; issue pause/cancel from another terminal. `prepare`
prints a reviewable specification, which can be edited before creation. Use UTF-8 output when
redirecting from older Windows PowerShell. `template` prints the deliberately tiny smoke specification.
The worker itself takes one newline-delimited version-1 JSON request `{ "version": 1, "job": "UUID" }`
on standard input; durable pause/cancel messages are versioned too.

## Validation

`cargo test --test simulation` exercises 1,200-member CPU campaigns, deterministic replay,
pause/resume equality, stage isolation, saved-file immutability, design training, family accounting,
and the actual worker/CLI process paths. The GPU agreement test is explicitly ignored on CPU-only
machines; run `cargo test --test simulation gpu_agrees_with_cpu -- --ignored` after installing a
compatible library. A real native-window interaction pass and a supported GPU hardware run remain
release validation steps; headless tests do not establish those outcomes.
