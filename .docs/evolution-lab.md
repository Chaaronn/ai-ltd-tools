# Evolution Lab: how it's built

The design and the reasoning are in [evolution-lab-spec.md](evolution-lab-spec.md). This page describes the code as
built.

**Evaluate-only, with one fenced exception.** Nothing in `src/lab/` or `src/gui_lab.rs` writes, creates, copies,
renames or deletes a file, except the best-lap keeper (`src/lab/keeper.rs`), which writes only
"<name> (best lap).evolution" copies (see below).
Saves are read with `std::fs::read`; all results live in memory. `tests/lab.rs::lab_never_writes_files` fails the
build if a write call appears in those files.

```
%APPDATA%\AI Learns To Drive\evolution\*.evolution
        │  read-only, background thread, re-read only when mtime changes
        ▼
format.rs ──▶ forks.rs ──▶ runs.rs ──▶ verdict.rs ──▶ compare.rs ──▶ gui_lab.rs
 (parse)     (copies)    (runs, stage,  (before/after,   (grid, pooled,   (Lab tab)
                          gains, stalls) placebo)         comparable runs)
```

| Module | Job |
|---|---|
| `src/lab/format.rs` | `.evolution` → `Save { name, vehicle, shape, outputs, sensors, gens }`. Each generation keeps its settings, non-zero reward weights, scores, lap, speed, survival inputs, track, and a hash of the raw record. Nulls stay `None`. |
| `src/lab/forks.rs` | Finds copies by identical leading records (`ForkInfo { parent, parent_is_prefix, own_from }`). |
| `src/lab/runs.rs` | Run segmentation, stage, gains, wobble, stalls, lap milestones, mutation buckets. |
| `src/lab/verdict.rs` | Slope fits, the before/after z-score, placebo threshold, verdicts with reasons. |
| `src/lab/compare.rs` | Filters, mutation × stage grid, pooled verdicts, comparable runs, track switches. |
| `src/lab/designs.rs` | Network designs: per-network summaries, inputs/outputs with vs without, same-track comparisons. |
| `src/lab/overview.rs` | "What's worked best": stage playbooks (overall and per track), evidence strength, findings, tracks, fastest laps and the settings history behind them. |
| `src/lab/library.rs` | Folder scan, mtime cache, change detection, and `analyse()` tying it all together. |
| `src/gui_lab.rs` | The tab: network tree, timeline with strips, side panel, run table, compare view. |
| `track-builder-cli lab` | The same analysis printed to the terminal (`--runs SAVE` for one save's run table). |

## Calculations

**Run.** Consecutive generations with the same mutation rate, selection algorithm, selection size, crossover,
weight decay, elimination, population, non-zero reward weights and track. Old records with a null mutation rate
inherit the previous generation's settings. A save whose distinct `TrackId`s exceed 20% of its generations (and has
10 or more) is random-track training: all its tracks count as one.

**Stage.** From generations trained before the run starts (inherited history included): **early** under 200,
**mid** 200–1,000, **late** 1,000–5,000, **veteran** after 5,000 (the fraction keeps rising to 20,000). Earlier versions placed the stage by
score or lap time against the network's best, but that jumped whenever the rewards, the time limit or the track
changed (a 4-generation network, or one that had just had its time limit raised, looked "late"). Generations are
crude (population size and track difficulty matter too) but predictable.

**Time limit.** Not saved directly; a generation that ends with cars still driving ran to the limit, so its `Time`
is the limit (carried over generations where every car was eliminated first). Players raise it step by step as a
network learns more of the track, so it doesn't split runs. It does matter for scores: a longer generation scores
more just by lasting longer. So:

- a before/after window stops at a time-limit change on either side (and the verdict is "can't tell" if a side
  gets too short); placebo windows do the same;
- stalls restart their comparison at each new time limit, and use score per second;
- "Score/sec" (median score ÷ generation time) is a measure and a chart; with a distance-only reward it's average
  speed. The timeline marks each raise with ⏱.

On the dev machine this removed a false result: Type 2's mutation drop at gen 120 had read as the clearest "helped",
but the time limit went 16 → 18 → 30 → 35 → 38 s in the next 15 generations.

**Gains** (per run):

- *score gain* = (median of last 5 medians − median of first 5) ÷ |first| × 100 ÷ length × 10, as % per 10 gens
- *lap gain* = (first best lap − last best lap) ÷ first × 100 ÷ length × 10
- *gain per minute* = (last 5 − first 5) ÷ (summed `Time` ÷ 60)
- *wobble* = standard deviation of the run's medians ÷ |mean|

**Stall.** 50+ generations in which the median never beats its best on that track and rewards by 1% *and* the best
lap on that track never improves by 0.1% (measured against the last lap that counted as a gain, so slow creeping
improvements add up). (With a best-lap reward the median can drop while laps keep getting
faster, so score alone reported false stalls.) The header
shows the running count ("no gain for N generations").

**Forks.** Saves are ordered by (history length, mtime). Each save's parent is the earlier save sharing the longest
prefix of identical records, preferring one whose whole history is that prefix. `own_from` = shared length: runs,
changes and placebo points before it are counted at the parent only.

**Before/after verdict** (each change between consecutive runs):

1. Can't tell if the track or the rewards changed too.
2. Window: up to *N* generations of the previous run before the change and of the new run after it (*N* = 20 by
   default; 10 or 40 in the Compare view). Each side needs *N*/2 (at least 5), otherwise "can't tell: only k gens".
3. Fit a least-squares line to the medians each side. Standard error = residual SD ÷ √Sxx, widened by
   √(n ÷ n_eff) with n_eff = n(1 − r₁)/(1 + r₁), r₁ = lag-1 autocorrelation of the residuals (clamped 0–0.95).
4. z = (slope_after − slope_before) ÷ √(se_before² + se_after²).
5. Threshold = 95th percentile of |z| at **placebo points**: every *N*/2 generations inside unchanged runs, across
   all saves (own history only). With fewer than 50 placebo points it falls back to 4.4.
6. Helped if z > threshold, hurt if z < −threshold, otherwise "can't tell: within noise".

On the dev machine's 32 saves: threshold 4.1 from 631 placebo points; 637 changes, 58 testable, 5 clearly helped and
2 clearly hurt (about 2.9 expected by chance). 363 changes came too soon after the previous one, 199 changed the
rewards too and 17 the track. The clear ones match the analysis in the spec (§9.2), e.g. Type 2 gen 120: slope
−0.262 → +0.807 per gen, z = +4.9.

**Comparable runs.** Runs from other saves with the same vehicle, stage label and reward metrics as the selected
save's latest run, at least 10 generations long, grouped by mutation bucket, selection and crossover. Only groups
with 5+ runs are shown.

**Track switches.** Where a fixed track changes: median survival and best lap over the last 10 generations on the
old track and the first 10 on the new one.

## Overview ("What's worked best")

The Lab opens on this view. Scope: all tracks, or one track (picked from the list or by clicking a row of the
tracks table). Measure: score gain or lap-time gain (% per 10 generations).

**Stage playbook.** For each stage (early / mid / late), runs of 10+ generations (own history only) are grouped by
each setting: mutation bucket, selection algorithm, parents kept (≤ 5, 6–15, 16–30, > 30), crossover and population
(≤ 600, 601–1000, > 1000). Values with fewer than 3 runs are ignored, and a setting needs two such values to say
anything. The best value is the one with the highest median gain. Its evidence:

- **beats** = the chance that a random run with the best value out-gained a random run with the runner-up (ties
  count half)
- **strong**: best has 8+ runs, runner-up 5+, beats ≥ 70%
- **some evidence**: best has 5+ runs, runner-up 3+, beats ≥ 60%
- **guess**: anything less, or a gap under 1 point per 10 gens however consistent

If the best mutation bucket's median is under +1% per 10 gens, the card says the stage is **flat**: no mutation
rate moved it, so rewards, track or network are the levers left.

**Best on a track** (track scope): the fastest lap, who set it and when, and that network's settings over time on
the track (consecutive runs with the same settings merged, reward tweaks ignored, stretches under 5 generations
hidden). Orange marks the stretches that set a new best lap.

**Biggest gains**: the five runs with the highest gain in scope.

**Findings**, most reliable first: strong and some-evidence playbook picks (and the mutation pick for each stage,
even when it's only a guess); flat stages; change types whose clear "helped" or "hurt" count is at least 2 and at
least twice what chance predicts; the share of changes that came too soon to judge; stalled networks; and, in track
scope, the fastest lap. Facts are measured directly and aren't comparisons.

**Track names** come from the `.track` files in `tracks`, `track_of_the_week` and `workshop_tracks` next to the
evolution folder, matched on `Id`. The game's built-in tracks are packed in its encrypted `.pck`, so they show as
"Built-in track <id>" with the networks that used them. The all-zero id is "Unsaved track", and random-track
training is "Random tracks".

The network view's side panel starts with the playbook card for that network's current stage: from its own track
when that track has some-evidence or strong picks, otherwise across all tracks.

On the dev machine, all tracks, score gain: early-stage picks are mostly guesses (0.21–0.30 mutation has the highest
median, +33% over 8 runs, but beats 0.13–0.20 only 55% of the time); keeping ≤ 5 parents and a population of
601–1000 early have some evidence; the late stage is flat (best +0.2%).

## Now training and suggestions (`src/lab/advice.rs`)

The save the game wrote most recently is the one being trained; it counts as **now training** if it was saved in
the last 15 minutes. It has its own **Now training** tab (the Lab opens on it while the game is training; a
picker shows any other network instead): status tiles for stage, best lap, last real gain and current settings,
the suggestions with their reasons written out, and the last 200 generations' lap time (or score). Every network's
side panel has the same suggestions for that network. Suggestions are shown, never applied: nothing is written to the game.

Judged by lap time when the current run is on a fixed track with laps, otherwise by score. In order:

1. **Timing** (always first): stalled (time to change something), or the current settings have run fewer than
   *N*/2 generations (wait so the last change can be judged), or still improving (no need to change).
2. **Playbook** for the network's current stage, from its own track first (some evidence or better) and then all
   tracks: "Try <setting> <value> (now <value>)" where the best-known value differs, "Keep …" where it already
   matches, each with its evidence label and why. All-tracks mutation picks are shown even as guesses. A flat stage
   adds "change rewards, track or network rather than mutation".
3. **Own history:** the stretch on this track where the network's lap time fell fastest per generation, and whether
   the current settings match it (a hint: one network only).
4. **Past clear verdicts** in this network (up to 3).

Example (snow nosw.weekly.boost on Autumn 06, gen 1,340, late stage by lap time): wait 3 more generations; strong
evidence for a population of 600 or less at the late stage (it beat > 1000 82% of the time across 14 runs); a guess
at mutation 0.03–0.07; and its own laps fell fastest at mutation 0.06 (gens 845–852).

`track-builder-cli lab --advise [SAVE]` prints the same (latest save by default).

## Network designs (`src/lab/designs.rs`)

A save's design is read from `NeuralNetwork.Shape`, `Sensors` (`$type` of each) and `Outputs`, and each snapshot
has its own copy. A generation's design is the first snapshot at or after it (else the current network), so a design
change is placed to within a snapshot interval (10–100 gens), and a design change starts a new run.

- **Inputs:** raycasts plus other sensors. Every save so far has 7 non-raycast sensors chosen from 12 types (speed,
  wheel angle, track curvature, side/front velocity and acceleration, angular velocity, grip, correct direction,
  boost capacity, distance from wall), so "which inputs" means "which 7".
- **Params** = Σ (inputs + 1) × outputs over the layers, which matches the game's `TotalParams`.
- **Outputs:** Steer, Accel, Handbrake, Brake, Boost.

Design features are fixed per network, so they're counted **per network**: each network's runs at a stage collapse
to one median before comparing, and evidence labels use network counts. The playbook has four design rows (raycasts
≤ 8 / 9–11 / 12+, outputs, hidden layers, network size in params), shown under each stage card. The **Network
designs** tab shows those picks per stage; every input and output with the networks that have it against those
without, per stage; tracks lapped by networks of different designs (fastest lap and generations spent there); and
a table of every network's design and gains. Suggestions add "for your next network" when a design pick has some
evidence or better and differs from the current network.

Lap times are shown to 3 decimal places throughout, the precision the game records.

## What counts as progress (`compare::Measure`)

What the save records per generation, checked in `AILearnsToDrive.dll`:

- **Score** depends on the reward weights, so it's only fair between runs with similar rewards.
- **"Best average speed"** is `BestScore ÷ Time` (`EvolutionStatsGeneration.get_BestAverageSpeed`), so it's score
  again, not a speed. The timeline no longer charts it; the tooltip calls it "score per second".
- **Distance isn't recorded.** The game measures it (`AgentStats.TotalActualDistance`, 0.1 × velocity length per
  tick) but only as a reward input, never in the save.
- **Best lap time**, **survival** (`ActiveCount ÷ AgentsCount`) and **generation length** (`Time`, simulated seconds)
  don't depend on the rewards.

So comparisons can use four measures (lap time by default, score kept as the second choice):

| Measure | Gain per 10 gens | Notes |
|---|---|---|
| Lap time | % faster (first vs last best lap in the run) | Fairest; only runs with laps |
| Survival | change in percentage points (median of first 5 vs last 5) | Works before the first lap |
| Score/sec | % change in median score ÷ `Time` | Not inflated by raising the time limit; average speed with a distance-only reward |
| Score | % change in median score | Depends on the reward weights |

Suggestions use lap time when the run has laps, otherwise score per second. The before/after verdicts still use the median
score, which is fair there because a verdict needs the same rewards on both sides.

## Keeping the best network (`advice::best_lap`)

Saving every generation (which the Lab needs, since only saved generations are in the file) overwrites the network,
so a network that later gets slower loses its best version. The Lab checks the current track's best lap:

- **Safe:** another save's history includes the generation that set it (a copy saved at or after that generation
  shares at least that many records).
- **Save a copy now:** no copy holds it and the latest 5 generations are within 0.1% of it, so the network in the
  file is still that good. The Now training tab shows an orange banner, and the network list marks recently trained
  networks with 🏁.
- **Lost:** no copy, and the network has got slower since. The banner names the closest snapshot still in the save.

The copy is made in the game (save under a new name); the Lab only tells you when. Copies are recognised as forks,
so their shared history is counted once.

## Lap-time projection (`src/lab/projection.rs`)

The Now training tab estimates where the lap time is heading on the current track.

- **Series:** the current lap pace (best lap over the trailing 5 generations), from the slowest point of the latest
  200 generations onwards, so a regression after a reward or settings change starts the curve fresh.
- **Model:** `a + b·e^(−x/τ)`, with τ on a log grid and `a`, `b` by least squares. The main fit uses the latest 60%
  of the series; fits over the whole series and the last 60 generations give the plateau's range.
- **No plateau** when the main fit is effectively a straight line, or puts the plateau above the current best; then
  the recent straight-line rate (seconds per 100 generations) projects at most 1,000 generations ahead.
- **Levelled off** when no faster lap has come for 40 generations: nothing more is expected at these settings.
- The tab shows the plateau and range, generations to −0.05 / −0.1 / −0.25 / −0.5 s, an optional target time (in
  memory only), and the fitted curve dashed into the next 60 generations on the chart. Suggestions add a line about
  it.

## Best-lap keeper (`src/lab/keeper.rs`)

On by default ("Keep a best-lap copy automatically" in the Lab's side panel). After each refresh, for every network
saved in the last 15 minutes whose **latest** generation set its best lap on its track:

1. If "<name> (best lap).evolution" exists and its best lap on that track is as fast or faster, nothing happens.
2. Otherwise the save is re-read; if its generation count still matches what the Lab analysed, its JSON is written
   back with `Name` = "<name> (best lap)" and a new version-4 `Id` (so the game doesn't mark it modded), gzip-
   compressed, to a `.tmp` file that is then renamed over the copy.

Copies of copies are never made, and the destination name is checked before every write. The Lab shows copies under
their source (never the other way round), counts their history once, ignores them when picking the network that's
"now training", and the best-lap banner reports the best as safe once a copy holds it.

The network saved in a file is whatever the game saves (its best network by score), so the copy is the network as
it stood when the best lap was set, not necessarily the exact car that drove it.

## Inside the network (`src/lab/netviz.rs`)

Shown on Now training below the chart. The save holds the network's weights and biases (`NeuralNetwork.Weights[l][i][j]`
= neuron `i` of layer `l` to neuron `j` of layer `l + 1`), the sensors in input order (raycasts with `Degrees` and
`Length`), the outputs in neuron order, and 6 snapshots of earlier weights.

- **Usage (estimated):** each output starts with importance 1 (or only the chosen output, via "Usage for"); each
  neuron hands its importance back to its inputs in proportion to |weight| ÷ the neuron's total incoming |weight|;
  each layer is normalised to sum to 1. Activations aren't saved, so this is a guide: an input whose weights are all
  small can't steer much, but one with large typical values can matter more than its weights suggest.
- **Sensor view:** a top-down car with each raycast at its real angle (0° forward, negative left) and relative
  length, coloured and thickened by usage; the other sensors as bars. Most- and least-used inputs are listed as a
  multiple of an even share, and non-raycast sensors under 0.6× are flagged as candidates to swap in the next
  network.
- **Network view:** layers as columns; lines blue for positive and orange for negative weights, brighter and
  thicker for bigger ones (lines under 4% of the layer's largest are skipped). "Changes" colours lines by how much
  each weight moved since the comparison snapshot.
- **Still learning?** Relative change = Σ|Δw| ÷ Σ max(|w_then|, |w_now|), against the latest snapshot at least 10
  generations back: overall, per layer, and per 10 generations between consecutive snapshots. Under 0.5% per 10
  generations counts as stopped: the game is carrying the same best network forward because nothing better turns up
  (Faster again 3 hasn't changed at all since gen 1300). Biggest usage shifts since the snapshot are listed too.
- **Diagnosis:** moving (change between the last two snapshots ≥ 0.5% per 10 gens) × improving (a gain in the last
  30 generations, shorter than a stall on purpose):
  - **Learning** (moving, improving): leave the settings alone.
  - **Fine-tuning** (still, improving): small changes still paying off; normal later on.
  - **Drifting** (moving, not improving): new networks adopted that are no faster. With mutation above 0.05 the
    advice is to lower it a step; at or below 0.05 it isn't overshooting, so the advice is a new challenge (longer
    time limit, or the playbook suggestion) instead.
  - **Stuck** (still, not improving): raise mutation, or change rewards, time limit or track.
- **Snapshot by snapshot:** each snapshot (same shape) plus now: best lap so far on the current track, median score,
  change from the previous snapshot, mean |w|, and "same network" / "faster by" / "new network, no faster". A mean
  |w| shift over 10% gets a line (growing: leaning harder on what it has; shrinking: weight decay at work).
- **Input usage over the snapshots:** usage share per input per snapshot, y axis zoomed to the data; the four inputs
  that moved most are coloured, the rest grey, with an "even share" line.
- **Which outputs are learning:** relative change of each output's incoming weights since the comparison snapshot.
- **Spare capacity:** hidden neurons whose outgoing |w| total is under 10% of their layer's average count as nearly
  idle. Idle neurons mean size isn't the bottleneck; none idle means a bigger network might help if it stalls.

`track-builder-cli lab --inside SAVE` prints the usage table and the change history.

## GUI notes

- **Track names.** The game's built-in tracks are packed inside the game's `.pck`, which is encrypted, so a save
  only knows them by `TrackId` (the game's settings, challenge and log files don't name them either). Unnamed
  tracks now show the network that trained most on them ("Built-in c75c00e2 · Type 2's track"; hover for all
  networks and the full id). The Tracks table (and the box next to the track picker) lets the player type a name.
  Names are saved by `lab::names` to `%APPDATA%\Cat's AI Ltd Tools\track-names.json` (a temp file then a rename),
  the Lab's second fenced write: it refuses any path inside a folder called "AI Learns To Drive", and the source
  test checks it has exactly one write, one rename and one create_dir_all. Player names override names from track
  files; clearing the box forgets the name.
- **Swap card** (Now training, under the usage bars, when a non-raycast sensor is under 0.6× an even share): for
  each flagged sensor, the networks-with vs networks-without evidence at the network's current stage; then the
  sensors this network doesn't have, best evidence first (with-better and at least 60% "beats", then about even,
  then without-better), with n per side, strength dots, and a compact ▲/▼ for the other stages. "About even" when
  the beat rate is under 60%; "no evidence yet" under 2 networks a side; "every network you've trained has it" when
  there's nothing to compare. Judged on lap time if the network has laps, else score per second. Notes how many of
  the game's 7 non-raycast sensor slots are used (room to add rather than swap). The weight-estimate and
  confounding caveats stay on the card.

- **Graph options** (⚙ menu by each timeline, kept for the session): toggle run bands, stalls, verdict markers,
  time-limit and reward-change markers, best line, middle-50% band, lap milestones, projection/target, fork point
  and the survival/spread strips; rolling-median smoothing (off/5/10/25/50 gens); chart height; quick ranges (last
  100/200/500/1,000, all). Scrolling over a chart zooms it and no longer scrolls the page.
- **Rewards** on Now training: the reward weights switched on, the time limit, when the rewards last changed, and a
  reward history with the best lap under each setup. Reward changes are marked ★ on the charts. The overview's
  playbook compares reward setups (which metrics are on, weights ignored) by lap time only, since rewards rescale
  the score.
- **Reward changes get verdicts on lap time.** A reward change used to be "can't tell" because the rewards set the
  score's scale. A lap time means the same whatever the rewards, so a change of rewards (same track) is now judged
  on the best-lap series: slope before vs after, z positive when laps get faster more quickly, against its own
  placebo threshold (best-lap series inside unchanged runs; 5.07 from 181 points on the current library, vs 3.84
  for scores). Lap windows don't stop at time-limit changes. Each side needs 10+ generations, 80% of them with a lap
  (gaps filled with the previous lap); otherwise "rewards changed, no laps to judge by". On the current saves: 9
  reward changes judged (1 clearly hurt), 8 without laps, 186 too short: most reward changes come within 10
  generations of the previous change.
- **Reward setups table** (What's worked best, under the stage cards): one row per combination of rewards (weights
  ignored): runs, networks, median lap gain per 10 gens at each stage, generations from starting a track to its
  first lap (for starts that mostly used that setup until then; "reached/total", with starts of 20+ gens that never
  lapped counted), the fastest lap with it (one track only), and the lap-time verdicts for switching to it. Setups
  with under 3 runs and under 2 track starts are hidden behind a button.

- The library is scanned on a background thread. Every 5 s the UI thread lists the folder's mtimes (cheap); if any
  changed, a new scan re-parses only those files. If a read fails (the game mid-save), the last good copy stays on
  screen with a "!" and is retried next time.
- Charts are drawn with the egui painter (no plot crate). Zoom is kept per view; picking a run in the table zooms
  to it.

## Tests

`tests/lab.rs`, using five real saves in `tests/fixtures/evolution` (the Type 2 family, an empty save and
"Faster again 3"):

- parsing (generation counts, vehicle, shape)
- forks (Type 2.1 / 2.2 forked from Type 2 at 289)
- run segmentation (the long runs of "Faster again 3")
- calibrated verdicts (Type 2.2's gen-120 mutation drop is "clearly helped")
- the empty save, the comparison grid, and the no-write source check
- the overview (picks sorted, minimum runs, the 1-point rule, tracks and the fastest network's settings history)

Set `LAB_SAVES` to a folder to also check a whole library:
`LAB_SAVES="%APPDATA%\AI Learns To Drive\evolution" cargo test --test lab -- --nocapture`.
Unit tests in `forks.rs` and `verdict.rs` cover fork families, siblings, slope recovery and a clear synthetic
change.

## Analysis: can "switching to rewards X at this stage" be suggested? (Oct 2026)

One-off analysis of the 33 saves (not part of the app). Question: after a reward switch, does lap-time progress
change, compared with ordinary points in training with no reward change nearby (placebo, same stage)?

- 203 reward switches on the same track in own history. Setups are held a median of **7 generations**; 90 of 214
  lasted under 5.
- Only **47** switches have laps on both sides (5+ lap gens before, 10+ after). Of those, **26** had a time-limit
  change within ±5 gens and 8 changed other settings at the same time. 20 of the 47 are one network (Faster again 3),
  often 3 to 5 switches inside 10 generations (e.g. gens 987–996), so their effects can't be separated.
- **6 clean switches** (no time-limit or settings change nearby, held 10+ gens, no other reward change within 10).
  Nothing can be concluded per metric from 6.
- Per metric, nothing is consistent. Weak hints only: adding Best lap (9 switches, 6 networks) sat slightly above
  typical (slope percentile median 77, best lap over the next 50 gens percentile 74); removing Score (7 switches,
  4 networks) sat below typical on slope in all 7 (percentile median 13). Adding Speed (avg) disagreed between the
  two measures.
- Time-limit raises almost never happen alone (1 with nothing else changing within 10 gens), so whether a longer
  limit itself speeds up best laps (more laps per car, more chances) can't be checked yet.
- The early stage has almost no laps (5 placebo points), so early-stage reward advice would have to use score per
  second or generations to a first lap instead.

Decision: don't build the suggestion yet. It would mostly be reporting noise and time-limit effects. What would
make it possible: holding a reward setup (and the time limit) for 10+ generations after a change. Re-run when there
are 30+ clean switches.
