# Simulation implementation progress

Checkpoint saved **2026-10-19**, before further implementation changes, at the user's request.

Plan: [Built-in simulation experiments](plans/2026-10-19-simulation-experiments.md).
Usage/semantics: [Simulation experiments](simulation-experiments.md).

## Workspace state

- Repository: `D:\Development\Rust\track-builder`.
- Branch: `codex/simulation-experiments` (created during implementation).
- Changes are uncommitted. No push or PR has been made for this work.
- Preserve pre-existing WIP in the Lab, README and Evolution Lab documentation. It includes
  advice/evidence improvements and files `gather`, `h2h`, `ledger`, `mutation`, `usage`; these
  must not be reverted or swept into an unrelated commit.
- User will handle NVIDIA backend installation. Do not retry downloading/installing its binary.
- `.codegraph` exists: use CodeGraph before locating/understanding indexed code. Read raw files
  for uncovered/stale sections and non-code docs as appropriate.

## Implemented so far (not a final acceptance claim)

1. `Cargo.toml` and lockfile pin `altd_sim` revision
   `ab3ec6799f55aaca5fb286e4fef60276b15f333a`, default features off, embedding `server` APIs enabled.
   Matching resources/licenses copied into `assets/simulation`; no server or GPU download.
2. `src/simulation/mod.rs`: app-owned specification, settings, arms, provenance, metrics,
   observations and progress. Validation fixes population at 1,200 and bounds alternatives.
3. `engine.rs`: raw gzip save conversion preserving f64 parameters and ordering; saved/generated
   track conversion; upstream initialization, bounded driving, checkpoints and metrics.
   Missing grip position requires explicit override; unsupported mapping errors are surfaced.
4. `storage.rs`: dedicated app-data store, hashed immutable specifications/checkpoints, atomic
   progress, exclusive session lock, versioned control messages and pruning of obsolete
   non-boundary checkpoints. Stage/continuation checkpoints retained.
5. `worker.rs` plus `src/bin/simulation-worker.rs`: headless process, versioned stdin request,
   paired settings branches, stage baseline continuation, fresh design training, budget pause,
   cancellation, resumable checkpoints and persisted endpoint observations.
6. `planner.rs`: observed same-vehicle settings, stage/unresolved-dimension ranking, card mapping,
   nearby design candidates and bounds. Requires further correctness review (see gaps).
7. `analysis.rs`: paired comparisons, seed deduplication, source-family accounting, missing-lap
   protection and screening labels. Seed differences are stored in result structures.
8. `cli.rs` and CLI dispatch: prepare/template/create/start/resume/pause/cancel/inspect/list/smoke.
9. `src/gui_simulation.rs`, `main.rs`, minimal `gui_lab.rs` wiring: background preparation/polling,
   campaign review, queue controls, results and supported-card buttons. Compiles; native GUI
   interaction has not been validated. UI implementation needs readability and functional review.
10. README, Evolution Lab docs/spec and new simulation documentation explain isolation, semantics,
    compute controls, attribution and user-managed GPU installation. Historical no-write claims
    were adjusted to distinguish original saves from permitted app-data/keeper writes.

## Verified evidence

Latest focused run: **`cargo test --offline --test simulation`: 12 passed, 0 failed, 1 ignored**.
Ignored test is explicitly hardware-dependent GPU agreement; no GPU success is claimed.

Passing focused coverage:

- Fixed population/default validation.
- CPU checkpoint replay: identical metrics and next-generation checkpoint bytes.
- Full-population campaign and persisted paired results.
- Real saved-weight/mixed-surface track input remains byte-identical after execution.
- Later baseline checkpoint unaffected by alternative mutation changes.
- Missing laps cannot become rate-based lap wins; repeated observations do not create families.
- Nearby design trains from scratch with validated bounds.
- Worker pause/resume metrics match uninterrupted training.
- Actual worker executable protocol and CLI smoke complete.
- Saved parameter/control ordering and inference preserved.
- Malformed shape/sensor mapping and multi-setting arm rejection.
- Session budget pauses with a valid checkpoint and no partial endpoint; cancelled job cannot resume.

Most recent complete suite, before the final three focused tests were added:
**`cargo test --offline`: 25 library + 32 Lab + 8 regression + 9 simulation passed; 1 GPU ignored.**
`cargo check --offline --bins` passed earlier.

**Strict lint is not clean:** `cargo clippy --offline --all-targets -- -D warnings` failed with
36 style diagnostics, mostly in pre-existing code. New diagnostics include nested conditionals
in simulation storage/worker. Do not mass-edit unrelated files to hide this baseline.

## Review status and interruption

The ce-work skill was applied. A ce-simplify-code pipeline pass was delegated for new simulation
files only, with `tests/simulation.rs` protected. It reported that no edits had yet been made,
then the coordinator and remaining review agents failed due to an account usage limit.
There is **no completed simplification result and no completed ce-code-review receipt**.
Do not claim independent review passed. No active review agent remains according to the last
agent inventory. Root continued only to save this plan/status checkpoint after the user's request.

## Remaining work / next actions

1. Resume simplification/review when available, or follow the skill's documented failure path.
   Expand compressed GUI closures and clean lint only in owned new files. Preserve proof tests.
2. Audit suggestion-card mapping carefully: selection combines algorithm/size labels; choose
   the card's actual supported alternative, never an arbitrary different observed value. Ensure
   design-card IDs route correctly and unsupported reasons are clear.
3. Audit exploration ranking and re-ranking after stage/measure changes in the review screen.
   Verify generated sensor alternatives are usable and constrained to observed compatible sensors.
4. Complete UI details: seed consistency visibility, explicit stage progress, practical survival
   units, source/design review, meaningful originating-suggestion navigation, setup/back navigation,
   and queue behavior while another job runs. The UI currently displays IDs for origin links.
5. Audit GPU/runtime allocation failures for the required CPU fallback or resumable pause;
   user-managed installation does not remove adapter error-handling obligations. Hardware run
   remains pending until the user supplies a compatible library.
6. Extend track acceptance beyond the current real-file run: geometry, spawn direction, surfaces
   and actual lap behavior need explicit assertions. Verify population identity before branch
   reproduction, control reuse for overlapping endpoints and failure/cancel edge cases.
7. Add/run UI-level smoke validation. Native GUI end-to-end has not been performed; do not claim
   it has. Confirm worker packaging alongside desktop executable.
8. Final review, targeted retests after fixes, complete-suite check when appropriate, diff/whitespace
   check, documentation consistency, and acceptance checklist audit. Lockfile is present but
   uncommitted; user plan's committed lockfile condition is not yet satisfied.

## Important implementation details for continuation

- Upstream source is available in ignored `target/upstream-sim`.
- `Network::try_from_game_export` is crate-private; adapter validates dimensions/finiteness itself.
- Upstream checkpoints represent the start of a generation. Restore before applying alternative
  settings. Elimination is constructed per session. Math profile is immutable per specification.
- The worker's stage schedule interleaves baseline segments and alternative branches; later baseline
  segments restore their own saved continuation, never an alternative.
- Endpoint evaluation is frozen/no reproduction; restore its starting checkpoint before continuing
  baseline training. Score rate uses common generation elapsed time, not each car's lifespan.
- Default root: `%LOCALAPPDATA%\CatsAiLtdTools\simulation-v1`; test/CLI override:
  `CATS_SIMULATION_ROOT`. Original `.evolution` files are opened read-only.
- Do not use `cargo fmt` over unrelated dirty modules; format named simulation files only.

## Log

### 2026-10-19 — checkpoint before further changes

Saved the full implementation contract and this status log at the user's explicit request.
Confirmed the last simulation run completed with 12 passing tests and one ignored GPU test.
No code changes were made between that request and writing these two documents.
