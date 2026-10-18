# History and lessons

How we got here, in case a future change runs into the same traps.

1. **Reading tiles from a palette screenshot (wrong).** The first model came from a screenshot of the tile palette:
   9 pieces, 3 lanes at 1/6, 1/2 and 5/6, and rotations only. It produced a plausible TT, but it couldn't reproduce
   Oulton Park from that track's own file. Several pieces were misread, and the palette groups pieces visually in ways
   that don't match the atlas ids.
2. **Guessing what `AlternativeId` means (partly wrong).** We tried treating it as Godot's standard flip/transpose
   flags. That fitted some tiles and not others. The truth is that every tile's orientations are a hand-made list,
   so no general rule exists.
3. **Calibrating from screenshots (worked, slowly).** Pairing a `.track` file with a screenshot of the same track,
   then measuring where the road crosses each tile edge, gave each `(atlas, alt)` its connections. This was enough to
   build valid tracks, but it missed connection *types*, the real lane positions (1/4, 1/2, 3/4) and the side
   numbering.
4. **Reading the game code (authoritative).** The DLL contains the whole table, the join rules, the racing line and
   the score formula. Lessons:
   - **Connection types matter.** Our v3 TT joined 45° ends to normal ends in 44 places. The game loaded it, but the
     road is drawn wrong at those joins.
   - **Side numbering:** N=0, S=1, W=2, E=3, not N/E/S/W. Our early files had invalid start connections that the
     game tolerated.
   - **The start connection is where cars come in**, not where they leave.
5. **Following the course closely.**
   - Hand-picked guide points missed small bends. Tracing the map automatically fixed that.
   - Scoring tiles by their cell centre made the solver ignore lanes. Scoring the real racing line inside each tile
     fixed that.
   - Some bends are smaller than a tile at 50 × 50. Bend exaggeration (keep the big shape, scale up the small
     detail) keeps them recognisable.
6. **Validating against the game's own files.** Every rule was checked against tracks the game itself wrote. That's
   now built into the regression tests in `tests/`.

Prototype history: the solver began in Python (scratch scripts). The Rust version reproduces its results; the Python
v4 and the Rust output for the TT differ only by minor tile choices.
