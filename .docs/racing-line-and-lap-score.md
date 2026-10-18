# Racing line and lap score

The game doesn't keep any per-tile road drawing for the cars. It builds a single closed **Godot `Curve2D`** through the
connection points (`TrackUtils.GetPath` → `GetPathCurve`). Cars, the curvature sensor and `LapScore` all use this
curve, and Track Builder copies it exactly (`src/game/curve.rs`).

## Geometry

- Tiles are 768 × 768 px (`TrackConstants.TileSize`).
- Connection points sit on a 5 × 5 sub-grid of each tile, so they're at 0, 192, 384, 576 and 768 px. A
  connection's coordinates are `(0, i+1)` on the left side, `(4, i+1)` on the right, `(i+1, 0)` on the top and
  `(i+1, 4)` on the bottom, where `i` is the lane index. That puts lanes at **1/4, 1/2 and 3/4** of the edge.
- The normal of a connection is `-ToVector(side)`, which points into the tile.

## Path points

`GetPath(track, startCoords, startConnection)` walks the lap and adds **one point per tile: that tile's entry
connection** (position, inward normal, connection type). The next tile's entry is this tile's exit, mirrored. So
consecutive points are always the two ends of the road through one tile.

## Bezier handles (`GetPathCurve`)

For each point `k`, with previous point `p` and next point `n`:

```
out_k = dir_out * |P_k - P_n| * s_out     where (dir_out, s_out) = ComputeTangentParams(normal_k,  P_n - P_k, type_k, type_n)
in_k  = dir_in  * |P_k - P_p| * s_in      where (dir_in,  s_in)  = ComputeTangentParams(-normal_k, P_p - P_k, type_k, type_p)
```

`ComputeTangentParams(normal, delta, typeA, typeB)`:

- `s = SmoothnessByTypePair[(typeA, typeB)]`, defaulting to **0.5**. The table holds 0.2 for every pair going from
  {normal, double, any} to {45°, 20°, 70°}.
- If `typeA` is in `AngleByType`, the handle direction is the normal rotated by that angle, turned towards `delta`
  (the sign of `cross(normal, delta)`):
  - 20° → `atan2(128, 384)` = 18.43°
  - 45° → π/4
  - 70° → `atan2(384, 128)` = 71.57°
- Otherwise the handle points straight along the normal.

The curve through a tile only depends on that tile, because both handle lengths use the distance between its own two
connection points. That lets the builder score tiles one at a time (`curve::tile_bezier`).

## Lap score

```
LapScore = Curve2D.GetBakedLength() * 0.013020833333333334      // PixelsToScore = px / 76.8
```

That works out to about 10 per tile of straight road. Track Builder samples each cubic 40 times instead of using
Godot's baking. On 20 real tracks it comes within 1–7% of the game's stored value (Godot's baking and the closing
segment differ slightly). The game recalculates `LapScore` when you save in the editor, so this only matters for
the first load.
