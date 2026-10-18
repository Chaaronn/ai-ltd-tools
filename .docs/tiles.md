# Tile table

Decoded from `TrackConstants..cctor` in `AILearnsToDrive.dll` (lambda `<.cctor>b__48_1`). The same table is
compiled into the app as `src/game/blocks_table.rs`.

Connection names are the game's: first letter = side (**T**op, **B**ottom, **L**eft, **R**ight), second = position
along that side (**L**eft/**M**iddle/**R**ight for top/bottom, **T**op/**M**iddle/**B**ottom for left/right).
So `LM` = west side, centre lane; `TR` = north side, right lane (lane index 2).

Connection type is shown in brackets when it is not *normal*. Two tiles only join if the touching connections are on
opposite sides, in the same lane, with the same type (or either is type 100 = *any*, used only by the finish line).

| Atlas | Block type | What it is | Alternatives (alt: connections) |
|---|---|---|---|
| 1 | Normal | Corner: centre lane ↔ outer lane | 0: LM – TL<br>1: TR – RM<br>2: LM – BL<br>3: LT – TM<br>4: RM – BR<br>5: LB – BM<br>6: TM – RT<br>7: BM – RB |
| 2 | Normal | Corner: centre lane to centre lane | 0: LM – TM<br>1: TM – RM<br>2: LM – BM<br>3: BM – RM |
| 3 | Normal | Tight corner hugging a tile corner | 0: LT – TL<br>1: TR – RT<br>2: BR – RB<br>3: LB – BL |
| 4 | Normal | Corner: centre lane ↔ far outer lane (wide) | 0: LM – TR<br>1: TL – RM<br>2: LM – BR<br>3: TM – LB<br>4: BL – RM<br>5: LT – BM<br>6: TM – RB<br>7: BM – RT |
| 5 | Normal | Straight, centre lane | 0: LM – RM<br>1: TM – BM |
| 6 | Normal | Straight with half-lane shift | 0: LM – RT<br>1: LT – RM<br>2: LM – RB<br>3: TM – BL<br>4: LB – RM<br>5: TL – BM<br>6: TM – BR<br>7: BM – TR |
| 7 | Normal | Corner: outer lane ↔ outer lane (medium) | 0: LT – BL<br>1: RT – BR<br>2: TL – LB<br>3: TL – RT<br>4: TR – RB<br>5: BL – RB<br>6: LT – TR<br>7: LB – BR |
| 8 | Normal | Wide sweeping corner, outer lanes | 0: LT – BR<br>1: LB – TR<br>2: TL – RB<br>3: BL – RT |
| 9 | Normal | Straight with full lane shift (S-bend) | 0: LT – RB<br>1: LB – RT<br>2: BL – TR<br>3: TL – BR |
| 10 | Normal | Straight, outer lane | 0: LT – RT<br>1: LB – RB<br>2: TL – BL<br>3: TR – BR |
| 11 | Pole | Straight, centre lane, with pole | 0: LM – RM<br>1: TM – BM |
| 12 | Double | Straight, double width | 0: LM (double) – RM (double)<br>1: TM (double) – BM (double) |
| 13 | Double | Transition normal ↔ double width | 0: LM – RM (double)<br>1: TM – BM (double)<br>2: LM (double) – RM<br>3: TM (double) – BM |
| 14 | Thin | Thin straight, centre lane | 0: LM – RM<br>1: TM – BM |
| 15 | Thin | Thin straight, centre lane | 0: LM – RM<br>1: TM – BM |
| 16 | Pole | Straight, centre lane, with pole | 0: LM – RM<br>1: TM – BM |
| 17 | Wide | Wide straight, centre lane | 0: LM – RM<br>1: TM – BM |
| 18 | Pole | Straight, outer lane, with pole | 0: LT – RT<br>1: LB – RB<br>2: TL – BL<br>3: TR – BR |
| 19 | Thin | Thin straight, outer lane | 0: LT – RT<br>1: LB – RB<br>2: TL – BL<br>3: TR – BR |
| 20 | Thin | Thin straight, outer lane | 0: LT – RT<br>1: LB – RB<br>2: TL – BL<br>3: TR – BR |
| 21 | Skew | Curve from normal into 45° diagonal | 0: BM (45°) – RM<br>1: LM (45°) – BM<br>2: TM (45°) – LM<br>3: RM (45°) – TM<br>4: TM (45°) – RM<br>5: LM (45°) – TM<br>6: BM (45°) – LM<br>7: RM (45°) – BM |
| 22 | Skew | 45° diagonal straight | 0: LM (45°) – TM (45°)<br>1: TM (45°) – RM (45°)<br>2: BM (45°) – LM (45°)<br>3: RM (45°) – BM (45°) |
| 23 | Pole | Straight, outer lane, with pole | 0: LT – RT<br>1: LB – RB<br>2: TL – BL<br>3: TR – BR |
| 24 | Skew | Transition 70° ↔ 20° | 0: LM (70°) – TL (20°)<br>1: TR (20°) – RM (70°)<br>2: LM (70°) – BL (20°)<br>3: LT (20°) – TM (70°)<br>4: RM (70°) – BR (20°)<br>5: LB (20°) – BM (70°)<br>6: TM (70°) – RT (20°)<br>7: BM (70°) – RB (20°) |
| 25 | Skew | Transition normal ↔ 70° (half of a V hairpin) | 0: LM (70°) – TL<br>1: TR – RM (70°)<br>2: LM (70°) – BL<br>3: LT – TM (70°)<br>4: RM (70°) – BR<br>5: LB – BM (70°)<br>6: TM (70°) – RT<br>7: BM (70°) – RB |
| 26 | Skew | 20° shallow diagonal straight | 0: LM (20°) – RT (20°)<br>1: LT (20°) – RM (20°)<br>2: LM (20°) – RB (20°)<br>3: TM (20°) – BL (20°)<br>4: LB (20°) – RM (20°)<br>5: TL (20°) – BM (20°)<br>6: TM (20°) – BR (20°)<br>7: BM (20°) – TR (20°) |
| 27 | Skew | Transition normal → 20° | 0: LM – RT (20°)<br>1: LT (20°) – RM<br>2: LM – RB (20°)<br>3: TM – BL (20°)<br>4: LB (20°) – RM<br>5: TL (20°) – BM<br>6: TM – BR (20°)<br>7: BM – TR (20°) |
| 28 | Skew | Transition 20° → normal | 0: LM (20°) – RT<br>1: LT – RM (20°)<br>2: LM (20°) – RB<br>3: TM (20°) – BL<br>4: LB – RM (20°)<br>5: TL – BM (20°)<br>6: TM (20°) – BR<br>7: BM (20°) – TR |

## Surfaces

The surface is encoded in the atlas id (`TrackTileBlock.ToSurface`):

| Atlas range | Surface |
|---|---|
| 1–28 | Asphalt |
| 101–128 | Dirt |
| 201–228 | Ice |

Connections are identical across surfaces; only the drawing and the car physics differ. Surfaces can be mixed in one
track (the *Autumn 03* track of the week uses all three).

## Block types

`TrackTileBlock.ToBlockType(atlas % 100)`: 1–10 Normal, 11/16/18/23 Pole, 12/13 Double, 14/15/19/20 Thin,
17 Wide, 21/22/24–28 Skew. The builder only uses Normal and Skew tiles so the road width stays constant.

## Special tile set ids

- `1010.0` — background tile (`BgTileSetId`).
- `1011.0–3`, `1012.0–7` — start/finish line overlays, chosen per start connection (`FinishTileSetIdByConnection`).
  These are drawn by the game; they are never stored in a `.track` file.
