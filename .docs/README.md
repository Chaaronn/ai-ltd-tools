# Docs: AI Learns To Drive, and how Track Builder works

Everything here was learned by reading the game's files: its compiled C# code and the tracks it saves. Nothing is
guessed unless a page says so.

| Page | What's in it |
|---|---|
| [game-overview.md](game-overview.md) | Engine, install and save folders, what each data folder holds |
| [track-file-format.md](track-file-format.md) | The `.track` format, field by field, including the start connection and size rules |
| [tiles.md](tiles.md) | The complete tile table: every atlas id, rotation code, connection and connection type |
| [racing-line-and-lap-score.md](racing-line-and-lap-score.md) | How the game builds the driving line through the tiles, and how `LapScore` is calculated |
| [reverse-engineering.md](reverse-engineering.md) | How the above was extracted, which classes matter, and how to re-extract after a game update |
| [tool-architecture.md](tool-architecture.md) | How Track Builder turns an image into a track, with tuning notes |
| [evolution-lab-spec.md](evolution-lab-spec.md) | Design doc for Evolution Lab: analysing saved training runs to find the settings that work, with the real-data analysis behind it |
| [evolution-lab.md](evolution-lab.md) | How the built Evolution Lab works: modules, the exact calculations, tests |
| [history-and-lessons.md](history-and-lessons.md) | What we tried before we had the game code, what went wrong, and why |

## Key facts at a glance

- **Format:** a `.track` file is gzip-compressed JSON. Tiles are keyed by `"Vector2i(x, y)"`. Each tile stores
  `AtlasId` (the tile type, plus 100 for dirt or 200 for ice) and `AlternativeId` (which rotation or mirror of it).
- **Tiles:** there are 28 tile types and 138 orientations in total, all in [tiles.md](tiles.md).
- **Lanes:** each tile edge has 3 lanes, placed at 1/4, 1/2 and 3/4 along it. Tiles are 768 px square in the game.
- **Connection types:** normal (0), 45° (1), double width (2), 20° (3), 70° (4), and any (100, only used by the
  finish line). A join needs the same side pair, the same lane and the same type.
- **Sides:** the game numbers them **N=0, S=1, W=2, E=3**.
- **Start:** `StartTileConnection` is the connection cars drive *in through* on the start tile.
- **Size:** 50 × 50 tiles at most (`TrackConstants.MaxTileSize`).
- **Lap score:** `LapScore` = length of the racing line in pixels ÷ 76.8. The game recalculates it when it saves a
  track.
