# Reverse engineering notes

## Method

`AILearnsToDrive.dll` is a plain .NET assembly. We didn't use a full decompiler (ILSpy/ilspycmd), because NuGet
was blocked where this was done. Instead we read the IL with two pip packages:

- `dnfile`: parses .NET metadata (types, fields, methods, string tokens).
- `dncil`: disassembles method bodies into CIL instructions.

`tools/ildump.py` dumps whole types (`python tools/ildump.py <TypeNameFragment> [nometh]`), and `tools/ilmeth.py`
dumps methods by name, including compiler-generated lambdas (`python tools/ilmeth.py "<.cctor>b__48_*"`). Both
read the DLL path from the `GAME_DLL` environment variable; the default is the normal Steam location.

With .NET installed, `ilspycmd -p -o out AILearnsToDrive.dll` gives readable C# and is the easier route next time.

## Classes that matter

| Class | What we took from it |
|---|---|
| `Lib.Tracks.TrackConstants` | `.cctor` builds `TrackTileBlocks` (the full tile table, in lambda `<.cctor>b__48_1`), the 12 connection points (TL…RB with 5×5 sub-grid coordinates), `MaxTileSize` 50×50, `TileSize` 768, the finish overlay tile ids 1011/1012 and the background tile 1010. |
| `Lib.Tracks.TileSet.TrackTileBlock` | `Connections`, `TileSetId`, `Surface = ToSurface(id)`, `Type = ToBlockType(id)`. |
| `Lib.Tracks.TileSet.TrackTileBlockConnection` | `Side`, `Index`, `Type`, `Coords`, `Position`. The constructor's switch gives the side order N, S, W, E. |
| `Lib.Tracks.TileSet.TrackTileBlockConnectionType` | Enum names: Any, Normal, Degrees45, Double, Degrees20, Degrees70. The values used are 0, 1, 2, 3, 4 and 100 for Any (see `IsCompatibleWith`). |
| `Lib.Tracks.TileSet.TrackTileBlockType` | Normal, Pole, Double, Thin, Wide, Skew. |
| `Lib.Tracks.TrackTile.IsCompatibleWith` | Types join if they're equal or either is 100. |
| `Lib.Tracks.TrackUtils` | `GetPath`, `GetPathCurve`, `ComputeTangentParams`, `AngleByType`, `SmoothnessByTypePair`, `GetTrackLapScore`. |
| `Lib.Tracks.Surface.TrackSurfaceConstants` | Atlas offsets 0 / 100 / 200 for asphalt / dirt / ice. |
| `Lib.Repos.Mappers.TrackMapper`, `Lib.Repos.Models.TrackRepositoryModel` | Probably the JSON (de)serialisation of `.track` files, going by the names and the bundled Newtonsoft.Json. Not read in detail. |
| `Lib.Tracks.TrackFactory`, `RandomTrackConfig` | The game's own random track generator. Not used by us, but worth reading for its block-assignment strategy. |

## Reading the tile table out of IL

Each table entry in `<.cctor>b__48_1` follows the same pattern:

```
ldloc.0                      // list
ldc.i4.2; newarr TrackTileBlockConnection
dup; ldc.i4.0; ldsfld LM; ldc.i4.0; callvirt Invoke; stelem   // connection 0 = LM, type 0
dup; ldc.i4.1; ldsfld TL; ldc.i4.0; callvirt Invoke; stelem   // connection 1 = TL, type 0
newobj ConnectionSet(...)
ldc.i4.1; ldarg.1; add       // atlas = 1 + surfaceOffset
ldc.i4.0                     // alternative = 0
newobj TrackTileSetId; newobj TrackTileBlock; callvirt List.Add
```

`TL`, `LM` and the rest are delegates `(type) => new Connection(coords, type)`. `tools/blocks.py` pattern-matches
this into `tools/blocks.json`, and `tools/gen_rust_table.py` writes `src/game/blocks_table.rs`.

## After a game update

1. `pip install dnfile dncil`
2. `python tools/ilmeth.py "<.cctor>b__48_*" > cctor_l.txt`. The lambda number can change; if it does, search
   `ildump.py TrackConstants` output for `stsfld F:TrackTileBlocks` and use the `ldftn` just before it.
3. `python tools/blocks.py`, which writes `blocks.json`. Copy it to `tools/blocks.json`.
4. `python tools/gen_rust_table.py`
5. `cargo test`. The regression tests load real tracks and must still validate.

New tile types (atlas 29+) will need a `block_type` entry in `src/game/tiles.rs`, from `ToBlockType`.

## Verification done

- All 20 tracks found on the dev machine (user, workshop and track-of-the-week, including mixed surfaces) pass
  validation with the decoded table, and their laps close from the stored start connection.
- The calculated lap scores are within 1–7% of the stored values.
- Every tile the user placed in-game matched the shapes we'd inferred earlier from screenshots.
