# Game overview

**AI Learns To Drive** is on Steam; its install folder is called *AI Learns To Race*. You build a track out of tiles,
and a population of neural-network cars evolves to drive it.

## Engine

- Godot 4 with **C# (.NET 8)** scripting. The game code is a normal .NET assembly, so it can be read directly.
- Install folder: `C:\Program Files (x86)\Steam\steamapps\common\AI Learns To Race\`
  - `AI Learns To Drive.exe`: the Godot runtime.
  - `AI Learns To Drive.pck`: Godot resources (scenes, textures, tile set art), about 94 MB.
  - `data_AILearnsToDrive_windows_x86_64\AILearnsToDrive.dll`: **all the game logic** (about 2 MB). This is where
    the tile table, file format and racing line come from.
  - Notable libraries next to it: GodotSharp, Newtonsoft.Json (serialisation), MathNet.Numerics, Google.Protobuf,
    LiveChartsCore (charts), QuadTrees, Serilog.

## Save data

Everything lives in `%APPDATA%\AI Learns To Drive\` (`C:\Users\<you>\AppData\Roaming\AI Learns To Drive\`):

| Folder | Contents |
|---|---|
| `tracks\` | **Your tracks.** One `<name>.track` file per track; the file name is the name shown in the game. Track Builder saves here. |
| `workshop_tracks\` | Tracks downloaded from the Steam Workshop (same format). |
| `track_of_the_week\` | Weekly official tracks (same format; some mix surfaces). |
| `temp_workshop\` | Scratch space for workshop uploads. |
| `challenges\` | `<guid>.progress`: per-track challenge progress. |
| `evolution\`, `evolution_race\`, `workshop_evolution\` | Saved neural-network populations. |
| `settings\local.settings` | Game settings. |
| `logs\`, `shader_cache\`, `vulkan\` | Engine housekeeping. |

The game reads `tracks\` when the menu opens, so a file Track Builder saves there appears in the track list without
any import step. Restart the game, or reopen the menu, if it doesn't show up straight away.

## Other systems spotted in the code (not used by Track Builder)

- **Random track generator:** `TrackFactory.GenerateRandomTrackInternal`, with `AssignBlocksAllBacktrack`,
  `AssignBlocksSimple` and `GetCandidateBlocks`, configured by `RandomTrackConfig` (including
  `AllowDoubleConnections`).
- **Challenges:** around 30 classes under `Lib.Tracks.Challenges` (lap in time, limited neurons, drift in time,
  elimination, and so on). They're stored in a track's `Challenges` array.
- **Leaderboards and track of the week:** `Lib.Repos.TrackOfTheWeek.*`, which talks to an online API.
- **Sensors:** `TrackCurvatureSensor` lets cars sense how sharply the track ahead curves.
- **Raycasting:** `Lib.Tracks.Raycast.BspTreeRaycaster`, the cars' distance sensors.
