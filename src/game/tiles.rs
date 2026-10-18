//! Tile model, matching the game's `TrackTileBlock` / `TrackTileBlockConnection`.

pub use super::blocks_table::BLOCKS;

/// Tile side. Discriminants are the game's own enum values (used in `StartTileConnection.Side`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Side {
    N = 0,
    S = 1,
    W = 2,
    E = 3,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::N, Side::S, Side::W, Side::E];

    pub fn opposite(self) -> Side {
        match self {
            Side::N => Side::S,
            Side::S => Side::N,
            Side::W => Side::E,
            Side::E => Side::W,
        }
    }

    /// Grid step towards the neighbour on this side (y grows downwards).
    pub fn delta(self) -> (i32, i32) {
        match self {
            Side::N => (0, -1),
            Side::S => (0, 1),
            Side::W => (-1, 0),
            Side::E => (1, 0),
        }
    }

    pub fn from_game(v: i64) -> Option<Side> {
        Some(match v {
            0 => Side::N,
            1 => Side::S,
            2 => Side::W,
            3 => Side::E,
            _ => return None,
        })
    }

    pub fn to_game(self) -> i64 {
        self as i64
    }
}

/// Connection type (`TrackTileBlockConnectionType`). Two connections join only if the types match
/// (or either is `ANY`).
pub mod kind {
    pub const NORMAL: u8 = 0;
    pub const DEG45: u8 = 1;
    pub const DOUBLE: u8 = 2;
    /// "Degrees20": a 1-in-3 slope, atan(128/384) = 18.4 deg.
    pub const DEG20: u8 = 3;
    /// "Degrees70": atan(384/128) = 71.6 deg.
    pub const DEG70: u8 = 4;
    pub const ANY: u8 = 100;
}

/// One end of the road inside a tile: which side, which lane (0..=2, top/left first) and its type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Conn {
    pub side: Side,
    pub index: u8,
    pub kind: u8,
}

pub const fn c(side: Side, index: u8, kind: u8) -> Conn {
    Conn { side, index, kind }
}

impl Conn {
    /// The connection the neighbouring tile must have to join this one.
    pub fn mate(self) -> Conn {
        Conn { side: self.side.opposite(), ..self }
    }

    /// Position in tile units (0..1). The game places lanes on a 5x5 sub-grid: 1/4, 1/2, 3/4.
    pub fn pos(self) -> (f64, f64) {
        let t = (self.index as f64 + 1.0) / 4.0;
        match self.side {
            Side::N => (t, 0.0),
            Side::S => (t, 1.0),
            Side::W => (0.0, t),
            Side::E => (1.0, t),
        }
    }

    /// Normal pointing into the tile through this connection.
    pub fn inward(self) -> (f64, f64) {
        let (dx, dy) = self.side.delta();
        (-dx as f64, -dy as f64)
    }
}

pub fn compatible(a: u8, b: u8) -> bool {
    a == b || a == kind::ANY || b == kind::ANY
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Block {
    pub atlas: u16,
    pub alt: u16,
    pub conns: [Conn; 2],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockType {
    Normal,
    Pole,
    Double,
    Thin,
    Wide,
    Skew,
}

/// `TrackTileBlock.ToBlockType` (atlas % 100).
pub fn block_type(atlas: u16) -> BlockType {
    match atlas % 100 {
        1..=10 => BlockType::Normal,
        11 | 16 | 18 | 23 => BlockType::Pole,
        12 | 13 => BlockType::Double,
        14 | 15 | 19 | 20 => BlockType::Thin,
        17 => BlockType::Wide,
        21 | 22 | 24..=28 => BlockType::Skew,
        _ => BlockType::Normal,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Surface {
    #[default]
    Asphalt,
    Dirt,
    Ice,
}

impl Surface {
    pub const ALL: [Surface; 3] = [Surface::Asphalt, Surface::Dirt, Surface::Ice];
    /// `TrackSurfaceConstants`: atlas offsets.
    pub fn offset(self) -> u16 {
        match self {
            Surface::Asphalt => 0,
            Surface::Dirt => 100,
            Surface::Ice => 200,
        }
    }
    pub fn of_atlas(atlas: u16) -> Surface {
        match atlas {
            100..=199 => Surface::Dirt,
            200..=299 => Surface::Ice,
            _ => Surface::Asphalt,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Surface::Asphalt => "Asphalt",
            Surface::Dirt => "Dirt",
            Surface::Ice => "Ice",
        }
    }
}

/// Look up a block by its file ids (any surface).
pub fn lookup(atlas: u16, alt: u16) -> Option<&'static Block> {
    let base = atlas % 100;
    BLOCKS.iter().find(|b| b.atlas == base && b.alt == alt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_complete() {
        assert_eq!(BLOCKS.len(), 138);
        for b in BLOCKS {
            assert_ne!(b.conns[0].side, b.conns[1].side, "{b:?}");
        }
    }

    #[test]
    fn known_entries() {
        // values cross-checked against screenshots of real tracks
        let b = lookup(10, 0).unwrap();
        assert_eq!(b.conns, [c(Side::W, 0, 0), c(Side::E, 0, 0)]);
        let v = lookup(125, 0).unwrap(); // dirt V-hairpin half
        assert_eq!(v.conns[0], c(Side::W, 1, kind::DEG70));
        assert_eq!(block_type(22), BlockType::Skew);
    }
}
