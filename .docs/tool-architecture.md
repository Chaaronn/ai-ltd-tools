# Track Builder architecture

```
image / SVG ──load──▶ RGB ──trace──▶ centre-line loop (px) ──edit (optional)──▶ ──fit──▶ guide (tile units) ──solve──▶ tiles ──▶ Track ──▶ .track
```

| Module | Job |
|---|---|
| `src/input.rs` | Load PNG/JPG/BMP/WebP/GIF, or render SVG/SVGZ (resvg) onto white at 1600 px on its longest side |
| `src/trace.rs` | Image → ordered loop on the middle of the drawn line, with markers bridged over |
| `src/fit.rs` | Start and direction, smoothing, bend exaggeration, scaling into the grid, per-cell distance and progress |
| `src/solve.rs` | A* search for a closed tile loop that follows the guide |
| `src/game/` | Tile table, connection rules, racing line, lap score, `.track` reading, writing and validation |
| `src/autotune.rs` | *Auto settings*: score a finished track against the drawn course and search the settings |
| `src/pipeline.rs` | `build(traced, params)`, shared by the GUI and the CLI |
| `src/gui.rs` | egui/eframe Windows app |
| `src/bin/track-builder-cli.rs` | `build` and `check` commands, for batch runs and testing |

## Trace

1. Mark *line* pixels: every channel below **line darkness** (default 80), plus saturated red when *include red
   markers* is ticked. Course maps often draw red marker dots on the line, which would otherwise leave gaps.
2. **Opening** (radius 1) removes thin text strokes, then **closing** (radius 3, *gap bridging*) joins small breaks.
3. Label the non-line regions. Every enclosed pocket except the largest one is **filled into the line**. That turns
   ringed markers (a dark outline round a coloured disc) and two-tone lines (dark edges, coloured centre stripe)
   into solid ink.
   The **largest region that doesn't touch the image border** is the inside of the loop.
   Labels, legends, titles and line spurs never change the outline of that region, which is why this beats
   skeletonising.
4. Measure the line width outwards from that region, grow the region by half of it, and trace its outline (Moore
   neighbour tracing). That gives an ordered loop running down the middle of the line.
5. **Bridge over markers** (*bridge over markers on the line*, on by default). Turn numbers, flags and similar markers
   sit on the line and would otherwise pull the traced loop round their edge. A distance transform of the solid line
   finds the "cores": pixels deeper inside the ink than a normal line could be (more than half the width plus 2.5 px).
   Runs of traced points within reach of a core are replaced with a Hermite curve between where the line enters and
   leaves, using the line's direction just outside the run. Runs longer than 10× the line width, or a quarter of the
   loop, are left alone, so genuinely thick sections (such as a filled-in hairpin) aren't flattened.

If tracing fails: the line is probably broken (raise gap bridging) or too light (raise line darkness). A map whose
course isn't drawn as one closed line, such as a dashed line or a satellite photo, needs tracing by hand first.

## Editing the line (GUI)

After tracing, the loop is simplified with Douglas-Peucker (1.2 px tolerance) so the editor has a manageable number of
handles; that is roughly 300 for the TT map, against 2,000 traced pixels. This simplified, editable line is what gets built.

- **Edit line:** drag a handle to move it, double-click the line to insert a point, right-click a handle to delete
  it. Ctrl+Z or *Undo* steps back, and *Reset* returns to the automatic trace.
- **Draw by hand:** for maps that won't trace (satellite photos, dashed lines), click round the course in race order.
- Sparse lines (an average spacing over 2 px) go through a centripetal Catmull-Rom spline before fitting, so handles
  and clicks become smooth curves rather than corners.

## Fit

- **Orient:** the loop starts at the point nearest the start/finish click; *reverse* flips the direction.
- **Smooth:** a 1.5 px gaussian removes pixel stair-steps.
- **Bend exaggeration (default 1.6):** `out = smooth + k × (line − smooth)`, where `smooth` is a gaussian with
  sigma = 4.2% of the course's largest dimension. Large-scale shape is kept, and smaller bends get k times bigger.
  This is what keeps features like Glen Helen visible: one tile on a 50-tile TT is roughly 300 m of real road.
- **Scale** the course so its largest dimension fits *size* tiles (minus a margin), centred.
- **Precompute:** for every cell within 2 tiles of the course, its distance to the course and its progress (arc
  length) along it.

## Solve

A* over states `(cell, entry connection, lap phase, arrival heading)`. A step places one tile, whose entry matches the
state; its exit connection's mate becomes the next state's entry. The arrival heading is included because angled
connections (20°, 45° and 70°) can be crossed at two headings, and the game draws a visible kink wherever consecutive
tiles disagree (see [racing-line-and-lap-score.md](racing-line-and-lap-score.md)).

- **Step cost:**
  ```
  1                                              (per tile: prefer short laps)
  + fidelity  × mean( max(0, d − tolerance)² )   (d = distance from the tile's real racing line to the course,
                                                  sampled at t = 0.2, 0.5, 0.8 on the game's own curve)
  + smoothness × (turn inside the tile + 2 × kink at the join with the previous tile)   [radians]
  + lane-change cost                             (straights that only shift lanes)
  ```
  Scoring the actual line rather than the cell centre is what lets the solver use the side lanes to hug the course.
  The **tolerance** band (0.2 tiles by default) stops it chasing pixel-level drift. It is scaled by the course's
  local *straightness* (`Guide::straightness`): 1 on straights, falling to 0 where the course turns about 35° per
  tile or more, and taking the minimum over ±1 tile so the band also narrows on the approach. Straights get
  straightened, but corners can't be cut for free. Without it, a long straight
  that slopes a few pixels over its length became a zig-zag of tiny lane shifts and 70° V pieces. The **smoothness**
  term charges for every change of direction, including kinks between tiles, so wiggles only appear where the course
  really bends.
- **Progress constraint:** each tile must move along the course between −0.6 and +3.5 tiles. This stops the search
  doubling back or cutting across the loop. The phase flag handles the lap wrapping around.
- **Tile set:** Normal plus Skew (all the angled pieces) by default; Pole, Thin, Wide and Double are excluded so the
  road width stays constant.
- **Goal:** get back to the start cell with the same entry connection after passing halfway round. All 12 normal
  entry connections of the start cell are tried, and the cheapest lap wins.
- The start connection written to the file is the lap's entry at the start tile (type 0, as the game requires).

The TT map (2,000-point trace, about 200 tiles) builds in about 0.5 s on one core.

**Smoothness metric:** `curve::total_turning(lap)` is the total absolute heading change round the lap, in full turns.
A clean loop is 1.0. The CLI prints it, and a regression test (`sloped_straight.svg`) keeps it under 5; the old
zig-zag behaviour scored 8.7.

## Start/finish

Before building, click the course line on the image to choose where the start goes; the green arrow shows the
direction. After building, **click any tile in the Track view** to move the start/finish there
(`Track::set_start_at`; if that tile's entry isn't a plain connection, the next suitable tile is used). **Flip
direction** (`Track::reverse_direction`) makes the cars drive the other way round. The start arrow in the Track
view shows where the cars come in.

## Auto settings

`autotune::auto_tune` picks exaggeration, smoothness, tolerance and fidelity for the course in front of it. Every
candidate is **built for real** and the finished track is scored against the course *as drawn*: the trace with only
pixel noise removed, held in `Guide::reference` in the same grid coordinates. It is not scored against the
exaggerated guide.

```
score = mean gap track → course            (no detours)
      + mean gap course → track            (no cut corners or lost features)
      + 0.8  × 95th-percentile course → track (cut corners)
      + 0.4  × worst course → track        (no lost hairpins)
      + 0.6  × |track turning − course turning| / course turning
```

*Turning* is the total heading change round the lap (`curve::total_turning`). For the course it's measured after
smoothing at about 0.4 tiles, so pixel wobble doesn't count. Zig-zags push the track's turning above the course's,
and flattened corners pull it below, so the score punishes both.

The search is coordinate descent. Each setting is varied over 4–6 values while the others stay fixed, and those
builds run in parallel threads; the best is kept before moving on to the next setting. There are two passes over all
four settings, about 40 builds, which takes a few seconds on a multi-core PC. The result is never worse than the
starting settings, since they are candidate zero. A regression test checks that.

Results on the test courses:

- **TT map:** exaggeration 1.0, smoothness 1.6, fidelity 60. It keeps Ramsey and Creg-ny-Baa and drops the
  zig-zags; turning is 9.5 against the course's 7.1, where the defaults gave 13.6.
- **Sloped straights:** dead-straight runs with single lane shifts; turning 1.9 against 1.8.
- **Marker SVG:** smooth, at 7.5 against 6.9.

The GUI's **Auto settings** button runs this with a progress bar, moves the sliders to the winning values and shows
the match score. The CLI does the same with `--auto`.

## Tuning

All of these have hover tooltips in the app. Tolerance, lane-change cost, line smoothing and bend size are under
**Advanced**.

| Want | Change |
|---|---|
| Straights that wobble or zig-zag | Raise *smoothness* (2–3), or *tolerance* (0.3–0.4) under Advanced |
| More of the small corners | Raise *bend exaggeration* (2.0–2.5). Too high and the overall shape distorts. |
| A smoother, simpler track | Lower *fidelity* (20–40), raise *smoothness*, or set exaggeration below 1. |
| A wobbly trace or hand-drawn line | *line smoothing* 0.005–0.01 under Advanced |
| No diagonal staircases | Keep *angled pieces* on. Without them diagonals become zig-zags. |
| A smaller track | Lower *size*. Fewer tiles means fewer bends survive. |
| "No closed track found" | Lower exaggeration, or check the start point is on the line and the direction is right. |

## Ideas not built yet

- **Edit-mode helpers:** a "smooth selection" brush, and snapping handles back onto the drawn line.
- **Pinned corners:** click a point on the image that the track must pass through, such as Quarterbridge. The
  Python prototype forced a V hairpin (atlas 25) at the course's sharpest point this way.
- **Per-section surfaces:** dirt or ice for chosen stretches.
- **Double-width start/finish straight:** atlas 12/13.
- **A drag-to-edit tile view** in the GUI, re-solving just the edited section.
- **Matching the game's lap score exactly:** reproduce Godot's `Curve2D` baking.
