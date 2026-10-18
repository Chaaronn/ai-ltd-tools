"""Regenerate src/game/blocks_table.rs from tools/blocks.json (run from the repo root)."""
import json
B = json.load(open("tools/blocks.json"))
NAME = {"TL":("N",0),"TM":("N",1),"TR":("N",2),"BL":("S",0),"BM":("S",1),"BR":("S",2),
        "LT":("W",0),"LM":("W",1),"LB":("W",2),"RT":("E",0),"RM":("E",1),"RB":("E",2)}
rows = []
for b in B:
    cs = ["c(Side::%s, %d, %d)" % (NAME[c][0], NAME[c][1], t) for c, t in b["conns"]]
    rows.append("    Block { atlas: %d, alt: %d, conns: [%s, %s] }, // %s-%s" % (b["atlas"], b["alt"], cs[0], cs[1], b["conns"][0][0], b["conns"][1][0]))
src = """//! Generated from the game's `TrackConstants` static constructor (AILearnsToDrive.dll).
//! Do not edit by hand: see `.docs/tiles.md` and `tools/gen_rust_table.py`.
use super::tiles::{Block, Side, c};

/// Every placeable block on the asphalt surface (atlas 1..=28). Dirt = atlas + 100, ice = atlas + 200.
pub const BLOCKS: &[Block] = &[
""" + "\n".join(rows) + "\n];\n"
open("src/game/blocks_table.rs", "w").write(src)
print(len(rows), "blocks written")
