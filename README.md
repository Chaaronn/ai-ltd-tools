# Cat's AI Ltd Tools

Tools for **AI Learns To Drive**: the **Track Builder** and the **Evolution Lab**.

The Track Builder turns a picture of a race circuit into a track for **AI Learns To Drive**, saved straight into the game.

## Use it

1. `cargo run --release` (or run `target\release\track-builder.exe`).
2. Drop a course map (PNG, JPG, BMP, WebP, GIF or **SVG**) onto the window, or click **Open image / track...**. The
   traced course shows in blue. Turn-number markers and flags on the line are bridged over automatically.
   - If nothing is traced, change **line darkness** or **gap bridging**, or use **Draw by hand**.
   - Click **Edit line** to fix the trace: drag points, double-click to add, right-click to delete, Ctrl+Z to undo.
3. Click the course line where the start/finish should be. A green arrow shows the race direction; tick
   **reverse race direction** if it points the wrong way.
4. Press **Build track**, or **Auto settings** to let it try about 40 combinations and keep the one that matches the
   map best (the sliders move to the chosen values). The **Track** tab shows the result, drawn with the game's own racing line, with the
   original course overlaid in yellow.
   - **Bend exaggeration** keeps small corners visible. **Fidelity** trades closeness to the course against
     simplicity. **Smoothness** removes zig-zags. More settings are under **Advanced**, and every control has a
     hover tooltip.
   - Start in the wrong place? Click any tile in the **Track** view to move the start/finish there, and use **Flip
     direction** to reverse the race direction.
5. Give it a name and press **Save to game**. It's written to `%APPDATA%\AI Learns To Drive\tracks\<name>.track`
   and appears in the game's track list.

You can also open an existing `.track` file to preview and check it.

## Evolution Lab

The **Evolution Lab** tab (top of the window) reads your saved networks from
`%APPDATA%\AI Learns To Drive\evolution` and shows which training settings worked. It never
changes your saves. Its one write is optional: a "<name> (best lap)" copy of a network
whenever it sets a new best lap, overwritten only by a faster one, so a later regression can't lose it.

- **Rewards:** reward changes get a before/after verdict on lap time (scores aren't comparable across rewards), and
  What's worked best has a reward setups table: lap gain by stage, generations to a first lap, fastest lap.
- **Track names:** built-in tracks show the network that trained most on them, and you can name them (stored in
  the tool's settings, not the game's folder).
- **Swap candidates:** when a sensor is barely used, Now training lists sensors to try instead, with the evidence
  from your other networks at the same stage.
- **Inside the network** on Now training: the car's raycasts and sensors coloured by how much the network uses them
  (estimated from its weights), the network drawn layer by layer, and whether it's still learning: a Learning / Fine-tuning / Drifting / Stuck
  verdict with advice, snapshot-by-snapshot changes, input usage over time, which outputs are learning, and idle neurons.
- **Lap-time projection** on Now training: where the lap time levels off, how many generations −0.1 s (or your
  own target) would take, and the curve drawn ahead on the chart.
- **Now training** (its own tab; the Lab opens on it while the game is training): tells you to save a copy in the
  game when the network is at its best lap and no copy holds it, so a later regression can't lose it. Then the save the game wrote last, its track and stage, and suggested next
  settings: whether to wait or change, what the evidence says for this stage and track, and what worked for this
  network before. Every network's side panel has the same suggestions. Nothing is changed in the game.
- **What's worked best** (opens first): for each training stage (early / mid / late / veteran: under 200, 1,000, 5,000 generations, and beyond), the best mutation rate,
  selection, parents kept, crossover and population, each marked strong evidence, some evidence or a guess. Scope it
  to all tracks or one track, and pick the measure: lap time (default), score, score per second or survival (score
  depends on your reward weights; the game doesn't record distance or a real speed). A track shows its fastest lap and the settings history
  that got there. Below that: the biggest single gains, plain-English findings, and a table of every track.
- **Network designs:** inputs (raycasts and which other sensors), hidden layers, size and outputs. Which designs
  learned fastest at each stage, each input and output with vs without, and networks of different designs on the
  same track. Counted per network, since a design is fixed for a whole network.
- **Network list:** every save, with copies of another save nested under it (found by matching training history).
  ⏸ marks a network that has had no gain for 50+ generations.
- **Timeline:** score (median, middle-half band, best), lap time or speed for every generation. Background bands are
  runs of unchanged settings (darker = lower mutation), hatching marks stalls, and the markers along the top give
  each settings change a verdict: ▲ clearly helped, ▼ clearly hurt, ● can't tell (hover for why). Scroll to zoom,
  drag to pan, double-click to reset. Survival and spread strips sit underneath.
- **Run table:** every run with its settings, start → end median, gains per 10 generations and per simulated minute,
  wobble and stage. Click a header to sort, a row to zoom to it.
- **Compare settings:** gains by mutation rate and training stage across all networks, and pooled verdicts by type of
  change, each against the number expected by chance. Filters for vehicle, track, selection, crossover and run
  length.
- It refreshes by itself every 5 seconds while the game trains.

Dropping a `.evolution` file onto the window (or passing one on the command line) opens it in the Lab.

## Build

Needs Rust 1.95 or later (egui 0.37). Run `rustup update stable` if `cargo` complains about the version.

```
cargo build --release
cargo test
```

Two binaries:

- `track-builder`: the Windows app.
- `track-builder-cli`: `build <image|svg> <out.track> [--start X,Y] [--reverse] [--grid N] [--exaggeration F] [--fidelity F] [--no-angled] [--surface asphalt|dirt|ice] [--debug-trace trace.png] [--auto]`,
  `check <file.track>...` to validate tracks and compare lap scores, and `lab [folder] [--window N] [--runs SAVE] [--overview] [--advise [SAVE]]`
  to print the Evolution Lab summary (or one save's run table).

## How it works

See [.docs/](.docs/README.md). It covers everything learned about the game (file format, all 138 tile
orientations, connection rules, the racing-line maths, the `.evolution` save format) and the design of both tools.
