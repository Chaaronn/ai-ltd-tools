# The `.track` file format

A `.track` file is **gzip-compressed UTF-8 JSON** with no other wrapping. Example (decompressed, reformatted):

```json
{
  "Id": "1ee55f26-7ab4-48fd-8844-30b8510f6485",
  "Tiles": {
    "Vector2i(5, 5)": { "Block": { "TileSetId": { "AtlasId": 10, "AlternativeId": 0 } }, "Coords": "Vector2i(5, 5)" },
    "Vector2i(6, 5)": { "Block": { "TileSetId": { "AtlasId": 10, "AlternativeId": 0 } }, "Coords": "Vector2i(6, 5)" }
  },
  "StartTileCoords": "Vector2i(5, 5)",
  "StartTileConnection": { "Side": 3, "Index": 0, "Type": 0 },
  "Challenges": [],
  "StarsToUnlock": 0,
  "LapScore": 288.9257303873698,
  "SteamWorkshopId": null,
  "Race": null,
  "Size": "Vector2i(9, 6)"
}
```

## Fields

| Field | Meaning |
|---|---|
| `Id` | A GUID. Use a new one per track; challenge progress files are keyed by it. |
| `Tiles` | An object keyed by `"Vector2i(x, y)"`, using Godot's `Vector2i` text form. `Coords` repeats the key. x grows to the right and y grows downwards. Real tracks start at (0, 0), but negative values parse fine. |
| `Block.TileSetId.AtlasId` | The tile type: 1–28 asphalt, 101–128 dirt, 201–228 ice. See [tiles.md](tiles.md). |
| `Block.TileSetId.AlternativeId` | Which orientation of that tile: 0–1, 0–3 or 0–7 depending on the tile. The meaning comes **only** from the table. There is no general rotation rule, because each tile's codes were listed by hand. |
| `StartTileCoords` | The tile the start/finish line sits on. |
| `StartTileConnection` | `{Side, Index, Type}`: the connection on the start tile that cars **drive in through**. The finish line is drawn there, and the car direction comes from it. |
| `Side` | **N=0, S=1, W=2, E=3** (`TrackTileBlockConnection` constructor: side 0 is top, 1 bottom, 2 left, 3 right). |
| `Index` | The lane along that side: 0 = top/left, 1 = centre, 2 = bottom/right. |
| `Type` | The connection type (0 normal, 1 45°, 2 double, 3 20°, 4 70°). Use 0; the finish line can only go on normal or any connections (`FinishCompatibleTypes = {0, 100}`). |
| `Challenges` | Challenge definitions (an empty array for user tracks). |
| `StarsToUnlock` | 0 for user tracks. |
| `LapScore` | The length of the racing line ÷ 76.8. See [racing-line-and-lap-score.md](racing-line-and-lap-score.md). The game recalculates it on save, so a rough value loads fine. |
| `SteamWorkshopId` | null unless the track is published. |
| `Race` | null for user tracks. |
| `Size` | `"Vector2i(w, h)"`, the bounding box. Older files have null; the game fills it in when it saves. |

## Rules a valid track must follow

1. **Every connection is matched.** For each tile and each of its two connections, the tile next to it on that side
   must have a connection on the opposite side, in the same lane, with a compatible type (equal, or either one 100).
2. **One single loop.** Following the road from the start connection must visit every tile and come back to the
   start.
3. **One tile per cell.** There are no crossing pieces.
4. **At most 50 × 50.**
5. **The start connection is a real connection of the start tile, with type 0.**

The game is lenient when it loads. Tracks that broke rule 1 (wrong connection type) or rule 5 (an invalid start
connection) still loaded, but the road is drawn wrong at the faulty joins. When one of those tracks was edited and re-saved in the game, the
start had moved; whether the game moved it or the player did is unknown.
Track Builder's `validate()` checks all five rules; `track-builder-cli check <file>` runs it.

## Writing

Write the keys in the order above; that's what the game writes, though it probably doesn't care. Compress with
normal gzip. Save as `%APPDATA%\AI Learns To Drive\tracks\<Name>.track`, where the file name is the display name.
