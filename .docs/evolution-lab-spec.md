# Evolution Lab: design document

**Status:** built (v0.1–v0.3, October 2026); see [evolution-lab.md](evolution-lab.md) for the implementation · **Owner:** Matt · **Home:** a second tab in Track Builder (`track-builder` repo)

Evolution Lab is an **evaluate-only** analysis tool for the game's saved neural-network populations. It answers one
question: **"which training settings actually work, at which stage of training?"** Every generation the game has ever
trained is already logged inside its save files, so the tool reads that log, splits it into runs of unchanged
settings, and compares those runs fairly.

> **Hard constraint: the tool can't make anything, only evaluate.** It never creates, copies, edits or deletes a file,
> in the game folder or anywhere else, with one exception the player asked for later: the best-lap copy (O-10). There are no exports, no archive, no notes file and no on-disk cache. Everything
> it shows is computed in memory from the saves as they are, and disappears when the app closes.

---

## 1. Problem statement

Training in AI Learns To Drive is trial and error. You pick a mutation rate, selection method, crossover and reward
weights, run some generations, and judge by eye whether it helped. The in-game charts show one network at a time and
don't label when settings changed. There is also no way to compare "0.2 for 100 generations then 0.19 for 50" against
what happened in other networks. Lessons get lost, and good settings are found by luck and then forgotten.

The data to answer this already exists. On the dev machine, 32 saved networks hold **15,393 logged generations**.
About 4,000 of those are duplicates, because several saves are copies of another save that kept training (see §5,
*Forks*). That leaves **11,391 unique generations in 655 runs of unchanged settings**. Nothing reads them.

The analysis in §9 also shows the data is noisier than it looks. Most settings were changed again within a few
generations, and a naive before/after comparison "finds" an effect almost as often at random points as at real
settings changes. The tool's second job is therefore to be honest about **what the data can and can't tell you**.

## 2. Goals

1. **See what happened.** For any saved network, show score, lap time and speed over every generation, with each
   settings change visible, in one view and in under 2 seconds.
2. **Compare settings fairly.** Rank settings (mutation rate first, then selection, crossover and selection size) by
   how much they improve a network, *controlling for how trained it already was and which track and rewards it used*.
3. **Judge each change honestly.** For every settings change, say whether it made a clear difference, or that the
   data can't tell, measured against how often the same test fires at random.
4. **Spot stalls.** Show when a network has stopped improving, and for how long.
5. **Zero setup, zero output.** No recording step, no game changes and no files written: it works on existing saves
   and updates as training continues.

## 3. Non-goals

| Not doing | Why |
|---|---|
| Controlling the game or running training automatically | Needs hooks into the game; risky, and the game owns the training loop. Read-only keeps saves safe. |
| Creating files: exports, CSV, archives, notes, caches | The tool evaluates. The one exception, added at the player's request, is the best-lap copy (O-10), which only ever writes "(best lap)" files. |
| Applying settings to the game | It suggests next settings with their evidence (see O-5), but never changes anything; the decision stays with the player. |
| Per-car or per-run replays | Only per-generation summary stats are saved, not individual cars. |
| Editing or repairing `.evolution` files | Read-only by design, so a bug can never corrupt a population. |
| A "distance travelled" metric | It isn't stored. Distance only exists inside the score when the *TotalActualDistance* reward is on. `BestAverageSpeed` is best score ÷ generation time, not a real speed. Comparisons use lap time, survival or generation length instead (O-8). |
| Cloud sync or sharing between players | v1 is local; see P2. |
| Recommending settings with ML | Too little data for a model to beat simple stage-matched statistics; revisit with more history. |

## 4. What the save files contain (verified)

`%APPDATA%\AI Learns To Drive\evolution\<name>.evolution` is gzip-compressed JSON, like `.track` files.

```text
{
  "Id", "Name", "Type", "Description", "Color", "Skin", "OpponentId", "SteamWorkshopId", "StarsToUnlock",
  "NeuralNetwork": { "Shape": [20,16,14,10,8,5], "InputShape", "OutputShape", "NeuronsCount", "TotalParams",
                     "Weights", "Biases" },
  "Sensors":  [ { "$type": "raycast", "Length": 252.3, "Degrees": -95 }, ... ],
  "Outputs":  [0,1,2,...],                       // NeuralNetworkOutputType
  "Snapshots": [ { "GenerationIndex", "NeuralNetwork", "Sensors", "OutputTypes", "VehicleType", "VehicleSkin" } ],  // up to 6
  "Stats": { "Generations": [ <one record per generation, oldest first> ] }
}
```

**One generation record:**

| Field | Meaning |
|---|---|
| `AgentsCount` | Population size (e.g. 1200) |
| `ActiveCount` | Cars still running when the generation ended |
| `Time` | Simulated seconds the generation ran |
| `WasEliminationEnabled` / `WasEliminationDisabled` | Elimination setting |
| `MutationRate` | e.g. 0.2 (null in 900 old records) |
| `SelectionAlgorithm` | 0 Best, 1 Tournament, 2 Roulette |
| `SelectionSize` | Parents kept |
| `CrossoverAlgorithm` | 0 None, 1 SinglePoint, 2 Uniform |
| `WeightDecay` | |
| `Rewards.Ratios[]` | `{Model:{Metric, Type}, Value}`: weight of each reward metric |
| `Rewards.Active[]` | Which metrics are switched on |
| `BestScore`, `AverageScore`, `MedianScore`, `FirstQuartileScore`, `ThirdQuartileScore`, `WorstScore` | Score distribution |
| `BestAverageSpeed` | |
| `BestLapTime` | Seconds; null until a car completes a lap (10,757 of 15,393 records are null) |
| `TrackId` | GUID of the track trained on (random-track training gives a new id every generation) |

**Enums (from `AILearnsToDrive.dll`):**

- `RewardMetric`: TotalScore, TotalSpeed, TotalAbsSteering, TotalDriftingAmount, TotalActualDistance,
  TotalRacingLineEfficiency, TotalMomentumChangeFrequency, TotalThrottleChangeFrequency, TotalBrakingFrequency,
  TotalDistanceFromWall, TotalDistanceFromCenter, NetworkNovelty, CollisionCount, BestLapPerformance.
- `RewardType`: Default, Average.
- `VehicleType`: Formula, Rally, Snowmobile, Truck.
- `NeuralNetworkOutputType`: Steering, Acceleration, Handbrake, Brake, Boost.

**Caveats.** Scores aren't comparable across different reward weights or tracks. Old saves have nulls. A network's
history ends wherever its save was overwritten or deleted. A copied save gets a **new `Id`** and carries the full
generation history of the save it came from, so `Id` can't be used to spot copies (see *Forks* below).

## 5. Core concepts

- **Network:** one `.evolution` file.
- **Run:** a stretch of consecutive generations where *mutation rate, selection algorithm, selection size, crossover,
  weight decay, elimination, population size, reward weights and track* all stay the same. When any of them changes, a new run starts.
  Random-track training (a new `TrackId` every generation) is treated as one "random" track, so it doesn't split into
  one-generation runs.
- **Stage:** how trained the network already was when the run started: the run's starting median score as a
  fraction of the best median score that network ever reached on the same track and rewards. The stages are early
  (<33%), mid (33–66%) and late (>66%). This is the key to fair comparison. Today's data shows high mutation rates
  "winning" (+24.6% per 10 generations at 0.21–0.3, against +1% at 0.08–0.12), but only because high rates are used
  early, when gains are easy.
- **Gain:** a run's improvement per 10 generations:
  - *score gain*: median of the last 5 generations minus median of the first 5, as a percentage of the start, per
    10 generations
  - *lap gain*: % lap-time reduction per 10 generations, when the run has laps
  - *best-score gain*
  - *stability*: the standard deviation of the median score within the run. High values mean the run is thrashing.
  - *gain per simulated minute*: the same gains divided by the summed `Time` of the run's generations, since a
    generation can last 10 s or 80 s.
- **Lap time first.** Where a run has laps, lap time is the headline measure. It's in seconds, so it compares across
  reward setups, while score doesn't. Score is the fallback before the first lap.
- **Fork:** a save whose first *k* generation records are byte-identical to another save's. It was copied from that
  save at generation *k* and trained on separately. Forks are found by comparing record hashes, never by name or `Id`.
  The shared generations are counted **once** in every comparison, at the oldest save, so copies don't double the
  evidence. The timeline shows a "forked from *X* at gen *k*" marker, and the network list nests forks under their
  parent.
- **Before/after test:** at every settings change where the track and rewards stay the same, fit a straight line to
  the median score over the last *N* generations before the change and the first *N* after (default *N* = 20, at
  least 10 each side). The effect is the change in slope, as a z-score whose standard error is widened for the
  autocorrelation between neighbouring generations.
- **Placebo calibration:** the same test is run at fake change points inside runs where nothing changed. The verdict
  threshold is the 95th percentile of those placebo |z| values, *not* a textbook 2. A change only counts as **clear**
  if it beats what random points produce 95% of the time. Otherwise the verdict is "can't tell", and it says why: too
  few generations, rewards changed at the same time, or within noise.
- **Stall:** 50 or more generations without the median beating its best on that track and rewards by 1%. Shown as a
  hatched span on the timeline, with a running counter on the latest generation ("no gain for 84 gens").
- **Survival rate:** `ActiveCount ÷ AgentsCount`, the share of cars still driving when the generation ended. It shows
  whether gains come from more cars surviving or from the best cars getting faster.
- **Spread:** `(Q3 − Q1) ÷ median`. A collapsing spread means the population has converged and further mutation
  changes mostly add noise; a rising one means it's diversifying.

## 6. Users and stories

**Primary: a player who trains networks seriously (Matt).**

1. As a player, I want to open a network and see its whole training history with every settings change marked, so I
   can tell which change caused a jump or a collapse.
2. As a player, I want a table of every run in a network (settings, length, start and end score, gain), so I can
   spot the runs that paid off.
3. As a player, I want to compare mutation rates across all my networks *at the same training stage*, so I know what
   to use next for a network that's, say, 70% trained.
4. As a player, I want to filter comparisons by vehicle, track, reward setup and population size, so I'm comparing
   like with like.
5. As a player, I want the view to refresh while the game is training, so I can watch the effect of a change without
   reopening anything.
6. As a player, I want to see when a lap was first completed and how lap time fell, so I can judge real driving
   progress and not just score.
7. As a player, I want each settings change to carry a verdict ("clearly helped", "clearly hurt", "can't tell"), so I
   don't read meaning into noise.
8. As a player, I want to be told when a network has stalled, so I stop burning hours on a run that isn't moving.
9. As a player, I want copies of a network recognised as copies, so their shared history doesn't count twice.

**Edge cases:**

- A network with 0 generations (5 of the 32 saves) shows "No training history yet", not an error.
- A fork with no training since it was copied (today, `snow nosw.weekly.boost` has the same 1,207 records as
  `snow nosw.weekly` but different weights) is shown as "copy of *X*, no generations of its own yet".
- A change where the rewards changed too gets the verdict "can't tell: rewards changed", since the score scale moved.
- A save that's half-written while the game saves it is retried on the next refresh, never shown as corrupt.
- A run with no laps shows lap gain as "–", not 0.
- Null `MutationRate` in old saves goes into the "unknown" bucket and is left out of comparisons by default.

## 7. Requirements

### P0: must have

| # | Requirement | Acceptance criteria |
|---|---|---|
| P0-1 | **Library scan:** list every `.evolution` in the game's evolution folder with name, vehicle, network shape, generations, last modified. | Given the folder has 32 saves, when the Lab tab opens, then all 32 are listed within 2 s, sorted by last modified, and empty saves are marked "no history". |
| P0-2 | **Parser:** read the format in §4, tolerate nulls and unknown fields. | All 32 saves on the dev machine parse; unknown keys are ignored; nulls become "missing", never 0. |
| P0-3 | **Run segmentation** as defined in §5. | "Faster again 3" (1,311 gens) produces the runs listed in §9; random-track saves don't split per generation. |
| P0-4 | **Network timeline:** charts of median (with the quartile band), best and average score; best lap time; best speed; all against generation, with runs shown as coloured background bands labelled by mutation rate. | Hovering a generation shows all its fields; hovering a band shows the run's settings and gains; it renders 1,500 generations without visible lag. |
| P0-5 | **Run table** for the selected network: start gen, length, mutation, selection (+size), crossover, rewards summary, start and end median, score gain, lap gain, stability. | Sortable by any column; clicking a row highlights that band on the timeline. |
| P0-6 | **Settings comparison:** every run (≥10 generations by default) across all networks, grouped by mutation-rate bucket and split by stage, showing count, median gain and spread. Fork-shared generations count once. | The bucket × stage grid shows *n* in every cell; cells with n < 5 are greyed and labelled "too few runs". |
| P0-7 | **Evaluate-only:** the tool writes nothing, anywhere. | Saves are opened read-only; the `lab` module has no file-write, create, copy or rename calls (checked by a test that greps the source); no cache or settings file is created. |
| P0-8 | **Fork detection** as defined in §5. | On the dev machine it finds exactly the 4 families in §9.3 with the stated divergence points, and nests the forks under their parents. |
| P0-9 | **Before/after verdicts** with placebo calibration, as defined in §5. | Every change point gets "clearly helped", "clearly hurt" or "can't tell" with the reason; the threshold is recomputed from the loaded saves; the change tooltip shows slopes before and after, *N* each side and the placebo threshold. |
| P0-10 | **Stall marker** as defined in §5. | Stalls of 50+ generations are hatched on the timeline; the latest generation shows the running count. |

### P1: nice to have

| # | Requirement | Acceptance criteria |
|---|---|---|
| P1-1 | **Live refresh:** re-read a changed save on a timer (5 s) or file-watch. | When the game saves a new generation, the timeline extends within 10 s. |
| P1-2 | **Filters** for comparison: vehicle, track (or random), reward profile, population size, selection algorithm, crossover, minimum run length. | Filters combine; the grid's counts update. |
| P1-3 | **"Comparable runs" panel:** for the selected network's latest generation, find past runs at the same stage, vehicle and reward profile and show how each settings group did. Evidence, not advice. | Shows at most 5 groups, each with *n*, spread and how many of its changes were clear; nothing is shown when n < 5. |
| P1-4 | **Lap milestones:** mark the first lap and each new best lap on the timeline. | Markers appear where `BestLapTime` first becomes non-null and wherever it drops. |
| P1-5 | **Survival and spread charts:** survival rate and spread as two thin strips under the timeline. | They share the timeline's x-axis and hover. |
| P1-6 | **Pooled verdicts:** in the comparison view, pool before/after effects by change type (mutation down, mutation up, crossover, population) and stage, with the count of clear results against the count expected by chance. | Each cell shows "*a* clear of *n* (≈*e* by chance)". |
| P1-7 | **Track transfer:** for networks trained on more than one track, compare the first generations on the new track with the end of the old one. | Shown per track switch, as lap-time or survival change; marked "can't tell" when scores aren't comparable. |

### Added after v0.3: the overview

Players asked for a summary of what has worked best, not just the evidence. Built as the Lab's opening view
("What's worked best"), per track and overall:

| # | Requirement | Acceptance criteria |
|---|---|---|
| O-1 | **Stage playbook:** for each stage, the best value of each setting with an evidence label (strong / some / guess). | Picks need 3+ runs per value; a gap under 1 point per 10 gens is always a guess; a flat stage says so. |
| O-2 | **Per-track scope:** the same playbook for one track, its fastest lap and the settings history that led to it. | Track names resolve from the player's track folders; built-in tracks fall back to an id and the networks that used them. |
| O-3 | **Findings:** plain-English conclusions, most reliable first, each labelled. | Never claims more than the evidence label allows; facts are marked as facts. |
| O-4 | **Suggestions in the network view:** what to do next for that network. | Uses the track's evidence when there is some, otherwise all tracks. |
| O-5 | **Now training:** detect the save being trained (newest mtime) and suggest its next settings. | Timing advice first (stalled / wait / still improving); every suggestion carries its evidence label and why; lap time is the measure on a fixed track with laps. |
| O-7 | **Network designs:** compare inputs, outputs, layers and size, counted per network. | Designs come from the save and its snapshots; design rows in every playbook; a with/without table per input and output; same-track comparisons. |
| O-8 | **Reward-independent measures:** compare by lap time (default), survival or generation length; score is kept as the second choice. | Every comparison view has the measure picker; suggestions use lap time, else survival. |
| O-9 | **Keep the best network:** prompt to save a copy in the game when the network is at its best lap and no copy holds it. | Safe / save now / lost, from record histories; the Lab still writes nothing. |
| O-10 | **Best-lap keeper:** write "<name> (best lap).evolution" when a network in training sets a new best lap, overwriting it only with a faster one. | Never touches other files; checked name; atomic replace; new v4 Id; copies shown under their source. |
| O-11 | **Lap-time projection:** plateau, range and generations to a target. | Shown on Now training with the curve dashed ahead on the chart; "levelled off" after 40 gens without a faster lap. |
| O-12 | **Stage by generations trained** (early < 200, mid < 1,000, late < 5,000, veteran after) and **time-limit aware** comparisons. | A new network is early; raising the time limit never makes it look more trained or create a fake gain; verdict windows stop at time-limit changes. |
| O-13 | **Inside the network:** sensor and network views with estimated input usage, and "still learning?" from weight changes between snapshots. | Below the chart on Now training; usage labelled as an estimate; stopped below 0.5% change per 10 gens. |
| O-14 | **Still learning, expanded:** a Learning / Fine-tuning / Drifting / Stuck diagnosis with advice, snapshot table, input usage over time, per-output change and idle neurons. | Drifting at low mutation doesn't advise lowering it; best lap in the table never gets slower; tested on the fixtures. |
| O-6 | **Lap-based stage and stall** (stage superseded by O-12) where laps exist. | A reward tweak no longer makes a well-trained network look early, and a falling lap time isn't called a stall. |

### P2: future

- **Snapshot diff:** compare a network's saved snapshots (shape, sensors, outputs) to explain jumps.
- **Track difficulty predictor (parked):** guess how hard a track is from its tiles by comparing first-lap
  generations across networks. Needs far more track variety than today's saves.
- **Statistical model:** once there are about 500 or more long runs, fit gain against settings and stage.

Dropped because they create files: CSV export, notes, a history archive and sharing.

## 8. Design

### Layout (Lab tab)

```
┌ Networks ────────────┐┌ Faster again 3 · Formula · 16-5-5-5-4-3-5 · 1,311 gens ───────────────┐
│ ▸ Faster again 3     ││ [Score] [Lap time] [Speed]          runs coloured by mutation rate    │
│   Fastest 3 terrain  ││  ▓▓0.2▓▓|░░░0.13░░░|▓0.2▓| ... |▒0.005▒|▒▒▒0.001▒▒▒                  │
│   snow nosw.weekly   ││  median ─── best ─── IQR band ░                                        │
│   Eagle1   ...       │├ Runs ──────────────────────────────────────────────────────────────────┤
│                      ││ start  len  mut   sel        xover   start→end   gain/10   lap gain    │
│ [Compare settings]   ││  29     77  0.13  Best·10    None    66.6→120.7  +10.5%    –           │
└──────────────────────┘└────────────────────────────────────────────────────────────────────────┘
```

The **Compare** view swaps the right-hand side for a grid: mutation-rate bucket down the side, stage across the top,
each cell a small box plot of gains with *n*, plus the filters from P1-2 above it.

### Visual rules

- Use the app's existing egui dark theme. Charts come from `egui_plot`.
- Score in blue, lap time in orange, speed in teal; they're also distinguishable by lightness. Run bands use a
  sequential scale from low (light) to high (dark) mutation rate, never hue alone.
- Every number shows its sample size where it's an aggregate.

### Architecture

```
src/lab/
  format.rs    serde structs for .evolution (Stats/Generations/Snapshots; unknown fields ignored)
  library.rs   scan folder, parse in a background thread, in-memory cache by (path, mtime)
  forks.rs     record hashes, shared prefixes, fork families, dedupe ranges
  runs.rs      segmentation, stage, gains, stability, stalls, survival, spread
  verdict.rs   before/after slopes, autocorrelation-widened SE, placebo threshold
  compare.rs   bucketing, filters, aggregates (median, IQR, n), pooled verdicts
src/gui_lab.rs the tab: list, timeline, run table, compare grid
```

- Parsing is about 1 MB of JSON per big save. Do it in a background thread, keep results in memory keyed by path and
  mtime, and only re-parse changed files. Nothing is persisted.
- The placebo threshold is recomputed whenever the library changes (a few thousand fake change points; well under a
  second).
- Bucket edges: ≤0.02, 0.03–0.07, 0.08–0.12, 0.13–0.20, 0.21–0.30, >0.30. They're configurable later.
- The game folder comes from the same `dirs::config_dir()` logic as the tracks folder.
- Tests: fixture saves (one big, one empty, one with nulls) with known run boundaries.

## 9. Real data

### 9.1 Worked example: "Faster again 3"

Runs of 15 or more generations, as the tool segments them (`track-builder-cli lab --runs "Faster again 3"`). Reward
weight tweaks start a new run, which is why the first run ends at gen 21.

| Start | Gens | Mut | Selection | Crossover | Median start→end | Best lap | Stage |
|---|---|---|---|---|---|---|---|
| 0 | 21 | 0.2 | Best·10 | None | 5.7 → 14.6 | – | mid |
| 29 | 77 | 0.13 | Best·10 | None | 43.3 → 62.0 | 57.7 s | mid |
| 172 | 20 | 0.2 | Best·5 | None | 15.7 → 43.8 | 20.2 s | early |
| 295 | 49 | 0.01 | Best·5 | None | 71.0 → 71.0 | 25.0 s | late |
| 506 | 90 | 0.09 | Tournament·10 | None | 28.5 → 41.6 | 5.9 s | mid |
| 655 | 305 | 0.2 | Best·30 | SinglePoint | 27.9 → 29.6 | – | early |
| 1092 | 38 | 0.03 | Best·30 | Uniform | 113.3 → 126.3 | 41.3 s | late |
| 1130 | 78 | 0.005 | Best·30 | Uniform | 140.7 → 143.6 | 40.3 s | late |
| 1208 | 103 | 0.0005 | Best·30 | Uniform | 143.3 → 143.6 | 40.2 s | late |

The table makes visible what the in-game chart hides. The 305-generation run at 0.2 barely moved (+0.2% per 10
gens), and the runs at 0.005 and 0.0005 at the end were flat. None of these changes gets a clear verdict: most came
1–8 generations after the previous change, or changed the rewards at the same time.

### 9.2 Before/after test on real data (all 32 saves, read-only, October 2026)

The test in §5 was run on every save, with fork-shared generations counted once.

**Where the change points go.** Of about 945 settings changes, 565 left fewer than 10 generations on one side, 268
changed the rewards at the same time and 22 changed the track. Only **90 changes are testable at *N* = 20** (58 after
removing fork duplicates). Run lengths are short: the median run is **4 generations**, 70% are under 10 and only 9%
reach 40.

**Noise.** The median score wobbles by about 14% of its level from one generation to the next within a 20-generation
window, with mild autocorrelation (lag-1 ≈ 0.25).

**Placebo comparison** (fake change points inside unchanged runs):

| Window *N* | Placebo with \|z\| > 2 | 95th-percentile placebo \|z\| | Real changes tested | Real changes beyond the placebo threshold |
|---|---|---|---|---|
| 10 | 30% | 4.0 | 122 | 9 (7%) |
| 20 | 28% | 4.4 | 58 | 6 (10%) |
| 40 | 25% | 4.5 | 27 | 1 (4%) |

**What it means:**

- **A plain z > 2 rule would be badly wrong.** It fires at 28% of random points. Hence placebo calibration in P0-9.
- **After calibration, few single changes are clear.** At *N* = 20, 6 of 58 (about twice the 5% expected by chance);
  at *N* = 40 there's no better than chance, because only long, stable runs qualify. Most changes will rightly read
  "can't tell".
- **Clear results at *N* = 20:** Type 2.2 gen 120 (mutation down, early: slope −0.26 → +0.81 per gen, still clear at
  *N* = 40), the snow nosw family at gen 762 (mutation down, early), Blank Mk1.200 gen 400 (mutation up, just after the fork),
  Fastest 3 terrain gen 122 and Rally for all gen 272 (mutation up, mid/late, ending a decline), and Jurgio Mk 1.3
  gen 11 (weight-decay change, clearly *worse*: slope 0.71 → 0.08).
- **The pooled picture is more useful than single verdicts.** Before calibration, real changes leaned positive (20%
  clearly up, 3% clearly down) where placebo points were balanced (13% up, 15% down). Changes help on average more
  than doing nothing. Part of that is regression from stalls, since changes are usually made when a run has
  flattened, so the tool must label pooled results as "associated with", not "caused".
- **Lap time trends can't be judged the same way.** Best lap only ever falls, so 13–14% of placebo windows are also
  ">0.5 s faster after". At *N* = 20 real changes beat that (7 of 21, 33%), but lap verdicts need the same placebo
  baseline.

**Consequences for the design:** placebo-calibrated verdicts (P0-9), pooled verdicts by change type and stage (P1-6),
a "can't tell" reason on every change, and a nudge in the run table showing how many generations a run would need
for a verdict ("10 more gens for a verdict").

### 9.3 Forks found in the saves

Matching generation records across files finds four families. None share an `Id`.

| Parent | Fork | Shared gens | Note |
|---|---|---|---|
| snow nosw (737) | snow nosw.weekly2 (1,080) | 737 | weekly2 continued for 343 more |
| snow nosw.weekly2 | snow nosw.weekly (1,207) | 1,080 | weekly is weekly2 plus 127 more |
| snow nosw.weekly | snow nosw.weekly.boost (1,207) | 1,207 | identical history, different weights and `Id`; no generations of its own yet |
| Type 2 (289) | Type 2.1 (322), Type 2.2 (604) | 289 | two branches from the same point |
| Blank Mk1.Marina (400) | Blank Mk1.200 (537) | 400 | 200 continued for 137 more |

The Jurgio Mk 1.2 and Mk 1.3 saves share no history; they're separate networks of different shapes. Counted naively,
these families add 4,002 duplicate generations (26% of the total) and would have tripled the weight of the
snow nosw changes in every comparison.

### 9.4 Stalls, survival and spread

- **Stalls of 50+ generations** happen in 12 of the 21 saves with real history. The longest are Test Random 1 (532),
  SnowFastest (407), Jurgio Mk 1.3 (268), Eagle1 (225) and Ice and fast (212).
- **Survival** is usually low: the median share of cars still running at the end is under 10% in most saves. Type 2.2
  and tuyv are the exceptions, at about 55%.
- **Spread** (IQR ÷ median) ranges from about 0.5 (Test Random 1) to 1.7 (Eagle1). That's wide enough to tell
  converged populations from diverse ones.

## 10. Success metrics

Leading indicators, in the first two weeks of use:

- The Lab is opened in at least 50% of training sessions.
- Time to answer "what did my last settings change do?" falls from minutes of guessing to under 10 seconds, including
  an honest "can't tell" when that's the answer.
- 100% of saves on the machine parse without errors, and all 4 fork families are detected.
- The verdict false-alarm rate on placebo points stays at 5% (by construction; checked by a test).

Lagging indicators, over one to two months:

- **Median run length rises from 4 generations to 15 or more.** Today most changes can't be judged because settings
  change again too soon; seeing "10 more gens for a verdict" should change that habit.
- The share of testable changes with a clear verdict rises above 10%.
- Stalls of 50+ generations are cut short sooner: the median stall length halves.
- Fewer generations to a target lap time on a new track, compared with past networks on similar tracks. Target: 25%
  fewer.

## 11. Open questions

| Question | Who | Blocking? |
|---|---|---|
| Is the run definition right? Should a reward-weight tweak start a new run, or only big changes? Today 199 changes (own history only) can't be tested because rewards changed with them; the tool currently splits on any reward tweak. | Matt (player) | Yes, for P0-3 |
| Is *N* = 20 the right default window? *N* = 10 tests twice as many changes but is noisier; *N* = 40 qualifies too few. Possibly let the window adapt to run length. | Matt / data | No |
| Should lap-time verdicts get their own placebo baseline (lap time only ever falls)? Proposed: yes. | Data | No |
| How should random-track training be compared: as its own "track", or only against other random runs? | Matt | No |
| Does the game rewrite the whole file on every save (safe to read mid-training) or append? Check by watching mtime and size during a training session. | Engineering | No (retry covers it) |
| Should the stage be based on lap time, where laps exist, instead of score? | Matt / data | No |
| Should the stall threshold (50 gens, 1%) scale with population size or generation time? | Matt | No |

## 12. Phasing

1. **v0.1 (P0-1 to P0-5, P0-7, P0-8):** library, parser, fork detection, timeline and run table for one network,
   writing nothing. This is the "see what happened" goal.
2. **v0.2 (P0-6, P0-9, P0-10, P1-1, P1-4):** before/after verdicts, stalls, cross-network comparison with stages,
   live refresh, lap markers.
3. **v0.3 (P1-2, P1-3, P1-5, P1-6, P1-7):** filters, comparable runs, survival and spread, pooled verdicts, track
   transfer.
4. **Later (P2):** snapshot diff, then the parked ideas once there's more data.

There are no external dependencies. Everything needed is in the save files and the existing app.
