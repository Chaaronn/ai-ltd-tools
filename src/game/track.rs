//! Reading, validating and writing the game's `.track` files (gzip-compressed JSON).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde_json::{json, Map, Value};

use super::curve;
use super::tiles::{compatible, lookup, Block, Conn, Side, Surface};

pub type Cell = (i32, i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileId {
    pub atlas: u16,
    pub alt: u16,
}

impl TileId {
    pub fn block(self) -> Option<&'static Block> {
        lookup(self.atlas, self.alt)
    }
}

/// A track in memory. `extra` keeps fields we don't model (Challenges, Race, ...) so a
/// read-modify-write round trip doesn't lose anything.
#[derive(Clone, Debug)]
pub struct Track {
    pub id: String,
    pub tiles: HashMap<Cell, TileId>,
    pub start: Cell,
    /// The connection cars drive in through on the start tile.
    pub start_entry: Conn,
    pub lap_score: f64,
    pub extra: Map<String, Value>,
}

#[derive(Clone, Copy, Debug)]
pub struct LapStep {
    pub cell: Cell,
    pub block: &'static Block,
    pub entry: Conn,
    pub exit: Conn,
}

#[derive(Debug, Clone)]
pub enum Issue {
    UnknownTile(Cell, TileId),
    OpenEnd(Cell, Conn),
    Mismatch(Cell, Conn, Cell),
    BadStart,
    NotOneLoop { lap: usize, tiles: usize },
    TooBig(i32, i32),
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Issue::UnknownTile(c, t) => write!(f, "unknown tile {}.{} at {:?}", t.atlas, t.alt, c),
            Issue::OpenEnd(c, k) => write!(f, "road leaves {:?} on {:?} lane {} with nothing there", c, k.side, k.index),
            Issue::Mismatch(c, k, n) => write!(f, "{:?} {:?} lane {} type {} doesn't join {:?}", c, k.side, k.index, k.kind, n),
            Issue::BadStart => write!(f, "start connection isn't on the start tile"),
            Issue::NotOneLoop { lap, tiles } => write!(f, "lap covers {lap} of {tiles} tiles"),
            Issue::TooBig(w, h) => write!(f, "track is {w}x{h}, the game allows 50x50"),
        }
    }
}

pub const MAX_SIZE: i32 = 50;

fn parse_vec2i(s: &str) -> Result<Cell> {
    let inner = s
        .trim()
        .strip_prefix("Vector2i(")
        .and_then(|r| r.strip_suffix(')'))
        .ok_or_else(|| anyhow!("bad Vector2i: {s}"))?;
    let mut it = inner.split(',').map(|p| p.trim().parse::<i32>());
    match (it.next(), it.next()) {
        (Some(Ok(x)), Some(Ok(y))) => Ok((x, y)),
        _ => bail!("bad Vector2i: {s}"),
    }
}

fn fmt_vec2i(c: Cell) -> String {
    format!("Vector2i({}, {})", c.0, c.1)
}

impl Track {
    pub fn load(path: &Path) -> Result<Track> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Track> {
        let mut s = String::new();
        GzDecoder::new(bytes).read_to_string(&mut s).context("not a gzip .track file")?;
        let v: Value = serde_json::from_str(&s)?;
        let mut obj = v.as_object().cloned().ok_or_else(|| anyhow!("track is not a JSON object"))?;
        let mut tiles = HashMap::new();
        if let Some(Value::Object(t)) = obj.remove("Tiles") {
            for (_, tile) in t {
                let coords = parse_vec2i(tile["Coords"].as_str().unwrap_or(""))?;
                let id = &tile["Block"]["TileSetId"];
                tiles.insert(
                    coords,
                    TileId {
                        atlas: id["AtlasId"].as_u64().unwrap_or(0) as u16,
                        alt: id["AlternativeId"].as_u64().unwrap_or(0) as u16,
                    },
                );
            }
        }
        let start = parse_vec2i(obj.remove("StartTileCoords").and_then(|v| v.as_str().map(String::from)).as_deref().unwrap_or("Vector2i(0, 0)"))?;
        let sc = obj.remove("StartTileConnection").unwrap_or(json!({}));
        let start_entry = Conn {
            side: Side::from_game(sc["Side"].as_i64().unwrap_or(0)).unwrap_or(Side::N),
            index: sc["Index"].as_u64().unwrap_or(0) as u8,
            kind: sc["Type"].as_u64().unwrap_or(0) as u8,
        };
        let id = obj.remove("Id").and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let lap_score = obj.remove("LapScore").and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(Track { id, tiles, start, start_entry, lap_score, extra: obj })
    }

    pub fn bounds(&self) -> (Cell, Cell) {
        let xs = self.tiles.keys().map(|c| c.0);
        let ys = self.tiles.keys().map(|c| c.1);
        (
            (xs.clone().min().unwrap_or(0), ys.clone().min().unwrap_or(0)),
            (xs.max().unwrap_or(0), ys.max().unwrap_or(0)),
        )
    }

    pub fn size(&self) -> (i32, i32) {
        let ((x0, y0), (x1, y1)) = self.bounds();
        (x1 - x0 + 1, y1 - y0 + 1)
    }

    /// Follow the road from the start tile. `None` if the start connection is invalid or the
    /// road breaks before getting back to the start.
    pub fn lap(&self) -> Option<Vec<LapStep>> {
        walk(&self.tiles, self.start, self.start_entry)
    }

    /// Everything the game would object to (or that would make the track misbehave).
    pub fn validate(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        for (&cell, &id) in &self.tiles {
            let Some(b) = id.block() else {
                issues.push(Issue::UnknownTile(cell, id));
                continue;
            };
            for k in b.conns {
                let (dx, dy) = k.side.delta();
                let n = (cell.0 + dx, cell.1 + dy);
                match self.tiles.get(&n).and_then(|t| t.block()) {
                    None => issues.push(Issue::OpenEnd(cell, k)),
                    Some(nb) => {
                        let ok = nb.conns.iter().any(|m| m.side == k.side.opposite() && m.index == k.index && compatible(m.kind, k.kind));
                        if !ok {
                            issues.push(Issue::Mismatch(cell, k, n));
                        }
                    }
                }
            }
        }
        match self.lap() {
            None => issues.push(Issue::BadStart),
            Some(l) if l.len() != self.tiles.len() => issues.push(Issue::NotOneLoop { lap: l.len(), tiles: self.tiles.len() }),
            _ => {}
        }
        let (w, h) = self.size();
        if w > MAX_SIZE || h > MAX_SIZE {
            issues.push(Issue::TooBig(w, h));
        }
        issues
    }

    /// Move the start/finish to `cell` (keeping the race direction). If that tile's entry
    /// isn't a plain connection (the game requires type 0), the next suitable tile is used.
    /// Returns false if `cell` isn't on the lap.
    pub fn set_start_at(&mut self, cell: Cell) -> bool {
        let Some(lap) = self.lap() else { return false };
        let Some(i) = lap.iter().position(|s| s.cell == cell) else { return false };
        for k in 0..lap.len() {
            let s = &lap[(i + k) % lap.len()];
            if s.entry.kind == 0 {
                self.start = s.cell;
                self.start_entry = s.entry;
                return true;
            }
        }
        false
    }

    /// Make the cars drive the other way round, keeping the start where it is if possible.
    pub fn reverse_direction(&mut self) -> bool {
        let Some(lap) = self.lap() else { return false };
        let i = lap.iter().position(|s| s.cell == self.start).unwrap_or(0);
        // going backwards, each tile is entered through its old exit
        for k in 0..lap.len() {
            let s = &lap[(i + lap.len() - k) % lap.len()];
            if s.exit.kind == 0 {
                self.start = s.cell;
                self.start_entry = s.exit;
                return true;
            }
        }
        false
    }

    pub fn recompute_lap_score(&mut self) {
        if let Some(l) = self.lap() {
            self.lap_score = (curve::lap_score(&l) * 1000.0).round() / 1000.0;
        }
    }

    pub fn set_surface(&mut self, s: Surface) {
        for t in self.tiles.values_mut() {
            t.atlas = t.atlas % 100 + s.offset();
        }
    }

    /// Serialise in the same key order the game writes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut tiles = Map::new();
        let mut cells: Vec<_> = self.tiles.iter().collect();
        cells.sort_by_key(|(c, _)| (c.1, c.0));
        for (&c, id) in cells {
            let k = fmt_vec2i(c);
            tiles.insert(
                k.clone(),
                json!({"Block": {"TileSetId": {"AtlasId": id.atlas, "AlternativeId": id.alt}}, "Coords": k}),
            );
        }
        let (w, h) = self.size();
        let mut o = Map::new();
        o.insert("Id".into(), json!(self.id));
        o.insert("Tiles".into(), Value::Object(tiles));
        o.insert("StartTileCoords".into(), json!(fmt_vec2i(self.start)));
        o.insert(
            "StartTileConnection".into(),
            json!({"Side": self.start_entry.side.to_game(), "Index": self.start_entry.index, "Type": self.start_entry.kind}),
        );
        let defaults: [(&str, Value); 5] = [
            ("Challenges", json!([])),
            ("StarsToUnlock", json!(0)),
            ("SteamWorkshopId", Value::Null),
            ("Race", Value::Null),
            ("Size", Value::Null),
        ];
        for (k, d) in defaults {
            if k == "Size" {
                o.insert("LapScore".into(), json!(self.lap_score));
                o.insert(k.into(), json!(fmt_vec2i((w, h))));
                continue;
            }
            o.insert(k.into(), self.extra.get(k).cloned().unwrap_or(d));
        }
        for (k, v) in &self.extra {
            if !o.contains_key(k) {
                o.insert(k.clone(), v.clone());
            }
        }
        let text = serde_json::to_string(&Value::Object(o))?;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(text.as_bytes())?;
        Ok(enc.finish()?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_bytes()?).with_context(|| format!("writing {}", path.display()))
    }
}

pub fn walk(tiles: &HashMap<Cell, TileId>, start: Cell, entry: Conn) -> Option<Vec<LapStep>> {
    let mut out = Vec::new();
    let (mut cell, mut e) = (start, entry);
    for _ in 0..=tiles.len() {
        let b = tiles.get(&cell)?.block()?;
        let entry_conn = *b.conns.iter().find(|k| k.side == e.side && k.index == e.index && compatible(k.kind, e.kind))?;
        let exit = if b.conns[0] == entry_conn { b.conns[1] } else { b.conns[0] };
        out.push(LapStep { cell, block: b, entry: entry_conn, exit });
        let (dx, dy) = exit.side.delta();
        cell = (cell.0 + dx, cell.1 + dy);
        e = exit.mate();
        if cell == start {
            return Some(out);
        }
    }
    None
}

/// The game's default tracks folder: %APPDATA%\AI Learns To Drive\tracks
pub fn game_tracks_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("AI Learns To Drive").join("tracks"))
}
