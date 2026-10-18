//! Find a closed loop of tiles that follows the guide.
//!
//! A* over states (cell, entry connection, lap phase). Each step places one tile; its cost is
//! 1 + weight * (mean squared distance from the tile's actual racing line to the course) plus
//! small style penalties. Progress along the course must keep moving forward, which stops the
//! search from wandering or cutting across the loop.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use crate::fit::Guide;
use crate::game::curve::{bezier, tile_bezier};
use crate::game::tiles::{block_type, BlockType, Conn, Side, Surface, BLOCKS};
use crate::game::track::{Cell, TileId, Track};

#[derive(Clone, Debug)]
pub struct SolveParams {
    /// How strongly to stay on the course line (vs. using fewer tiles).
    pub fidelity: f64,
    /// Allow the angled pieces (45/20/70 degree, atlas 21-28).
    pub allow_angled: bool,
    /// Extra cost for a straight that only shifts lanes (avoids pointless wiggles).
    pub wiggle_penalty: f64,
    /// Distance from the course (tiles) that costs nothing. Lets long, slightly sloping
    /// straights stay straight instead of stepping to follow every pixel.
    pub tolerance: f64,
    /// Cost per radian of heading change inside a tile. Higher = smoother, fewer kinks.
    pub turn_penalty: f64,
    pub surface: Surface,
    /// Max backwards / forwards progress per tile, in tiles.
    pub back: f64,
    pub fwd: f64,
}

impl Default for SolveParams {
    fn default() -> Self {
        SolveParams { fidelity: 60.0, allow_angled: true, wiggle_penalty: 1.5, tolerance: 0.2, turn_penalty: 1.0, surface: Surface::Asphalt, back: 0.6, fwd: 3.5 }
    }
}

#[derive(Clone, Debug)]
pub struct Solution {
    pub track: Track,
    pub cost: f64,
    pub mean_dev: f64,
    pub max_dev: f64,
    /// Solver-grid cell that became tile (0, 0) - lets the GUI overlay the course on the track.
    pub offset: (i32, i32),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct State {
    cell: Cell,
    entry: Conn,
    phase: u8,
    /// Direction of travel arriving through `entry`, in whole degrees (NO_DIR at the start).
    /// Angled connections can be crossed at two headings, and the game draws a kink when
    /// they don't match, so the heading is part of the state.
    dir: i16,
}

const NO_DIR: i16 = i16::MIN;

fn heading_deg(v: (f64, f64)) -> i16 {
    (v.1.atan2(v.0).to_degrees().round() as i16).rem_euclid(360)
}

fn angle_between_deg(a: i16, b: i16) -> f64 {
    let d = (a - b).rem_euclid(360) as f64;
    d.min(360.0 - d).to_radians()
}

/// One way through a block.
#[derive(Clone, Copy)]
struct Move {
    block: usize,
    exit: Conn,
    /// heading entering / leaving the tile (degrees)
    in_dir: i16,
    out_dir: i16,
}

#[derive(PartialEq)]
struct Item(f64, State);
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0) // min-heap
    }
}

struct Ctx<'a> {
    g: &'a Guide,
    p: &'a SolveParams,
    /// entry conn -> (block index, exit conn)
    trans: HashMap<Conn, Vec<Move>>,
    /// tile-unit sample points of each block's racing line
    samples: Vec<[(f64, f64); 3]>,
    pen: Vec<f64>,
    dev_cache: HashMap<(Cell, usize), f64>,
}

impl<'a> Ctx<'a> {
    fn new(g: &'a Guide, p: &'a SolveParams) -> Self {
        let mut trans: HashMap<Conn, Vec<Move>> = HashMap::new();
        let mut samples = Vec::new();
        let mut pen = Vec::new();
        for (i, b) in BLOCKS.iter().enumerate() {
            let bt = block_type(b.atlas);
            let ok = bt == BlockType::Normal || (p.allow_angled && bt == BlockType::Skew);
            let seg = tile_bezier(b, b.conns[0]);
            samples.push([bezier(&seg, 0.2), bezier(&seg, 0.5), bezier(&seg, 0.8)]);
            let [a, c] = b.conns;
            let straight = a.side.opposite() == c.side;
            let wiggle = if straight && a.index != c.index && a.kind == 0 && c.kind == 0 { p.wiggle_penalty } else { 0.0 };
            // heading change through the tile, from the curve's end tangents
            let (h0, h1) = ((seg[1].0 - seg[0].0, seg[1].1 - seg[0].1), (seg[3].0 - seg[2].0, seg[3].1 - seg[2].1));
            let turn = (h0.0 * h1.1 - h0.1 * h1.0).atan2(h0.0 * h1.0 + h0.1 * h1.1).abs();
            pen.push(wiggle + p.turn_penalty * turn);
            if !ok {
                continue;
            }
            // forwards (a -> c) along the curve, and backwards
            let fwd_in = heading_deg((seg[1].0 - seg[0].0, seg[1].1 - seg[0].1));
            let fwd_out = heading_deg((seg[3].0 - seg[2].0, seg[3].1 - seg[2].1));
            let back_in = heading_deg((seg[2].0 - seg[3].0, seg[2].1 - seg[3].1));
            let back_out = heading_deg((seg[0].0 - seg[1].0, seg[0].1 - seg[1].1));
            trans.entry(a).or_default().push(Move { block: i, exit: c, in_dir: fwd_in, out_dir: fwd_out });
            trans.entry(c).or_default().push(Move { block: i, exit: a, in_dir: back_in, out_dir: back_out });
        }
        Ctx { g, p, trans, samples, pen, dev_cache: HashMap::new() }
    }

    fn dev2(&mut self, cell: Cell, bi: usize) -> f64 {
        if let Some(&v) = self.dev_cache.get(&(cell, bi)) {
            return v;
        }
        let mut acc = 0.0;
        for s in self.samples[bi] {
            let q = (cell.0 as f64 + s.0, cell.1 as f64 + s.1);
            let (d, tol) = match self.g.nearest(q, 2) {
                Some((d, i)) => (d, self.p.tolerance * self.g.straightness[i]),
                None => (3.0, 0.0),
            };
            let e = (d - tol).max(0.0);
            acc += e * e;
        }
        let v = acc / 3.0;
        self.dev_cache.insert((cell, bi), v);
        v
    }

    /// Plain RMS distance from the course for reporting (ignores the tolerance band).
    fn raw_dev(&self, cell: Cell, bi: usize) -> f64 {
        let mut acc = 0.0;
        for s in self.samples[bi] {
            let d = self.g.nearest((cell.0 as f64 + s.0, cell.1 as f64 + s.1), 2).map(|x| x.0).unwrap_or(3.0);
            acc += d * d;
        }
        (acc / 3.0).sqrt()
    }

    fn progress(&self, cell: Cell, phase: u8) -> Option<f64> {
        let &(_, s) = self.g.cells.get(&cell)?;
        let l = self.g.length;
        Some(if phase == 1 && s < l / 4.0 { s + l } else if phase == 0 && s > 3.0 * l / 4.0 { s - l } else { s })
    }

    fn search(&mut self, start: Cell, entry: Conn) -> Option<(Vec<(Cell, usize, Conn, Conn)>, f64)> {
        let s0 = State { cell: start, entry, phase: 0, dir: NO_DIR };
        let mut heap = BinaryHeap::new();
        let mut cost: HashMap<State, f64> = HashMap::new();
        let mut came: HashMap<State, (State, usize, Conn)> = HashMap::new();
        let mut goal: Option<(f64, State, usize, Conn)> = None;
        cost.insert(s0, 0.0);
        heap.push(Item(0.0, s0));
        let l = self.g.length;
        while let Some(Item(gc, st)) = heap.pop() {
            if gc > *cost.get(&st).unwrap_or(&f64::MAX) {
                continue;
            }
            if let Some((best, ..)) = goal {
                if gc >= best {
                    break;
                }
            }
            let Some(s_cur) = self.progress(st.cell, st.phase) else { continue };
            let opts = self.trans.get(&st.entry).cloned().unwrap_or_default();
            for mv in opts {
                let (bi, exit) = (mv.block, mv.exit);
                let (dx, dy) = exit.side.delta();
                let nc = (st.cell.0 + dx, st.cell.1 + dy);
                let Some(&(_, s_raw)) = self.g.cells.get(&nc) else { continue };
                let mut nph = st.phase;
                let mut s_n = s_raw;
                if st.phase == 0 {
                    if s_n > 3.0 * l / 4.0 {
                        s_n -= l;
                    } else if s_n > l / 2.0 {
                        nph = 1;
                    }
                } else if s_n < l / 4.0 {
                    s_n += l;
                }
                let ds = s_n - s_cur;
                if ds < -self.p.back || ds > self.p.fwd {
                    continue;
                }
                let ne = exit.mate();
                // kink where this tile meets the previous one
                let kink = if st.dir == NO_DIR { 0.0 } else { angle_between_deg(st.dir, mv.in_dir) };
                let step = 1.0 + self.p.fidelity * self.dev2(st.cell, bi) + self.pen[bi] + self.p.turn_penalty * 2.0 * kink;
                let ng = gc + step;
                if nc == start && ne.side == entry.side && ne.index == entry.index && ne.kind == entry.kind && nph == 1 {
                    if goal.is_none_or(|(b, ..)| ng < b) {
                        goal = Some((ng, st, bi, exit));
                    }
                    continue;
                }
                let ns = State { cell: nc, entry: ne, phase: nph, dir: mv.out_dir };
                if ng < *cost.get(&ns).unwrap_or(&f64::MAX) {
                    cost.insert(ns, ng);
                    came.insert(ns, (st, bi, exit));
                    heap.push(Item(ng, ns));
                }
            }
        }
        let (total, last, lbi, lexit) = goal?;
        let mut path = vec![(last.cell, lbi, last.entry, lexit)];
        let mut cur = last;
        while let Some(&(prev, bi, exit)) = came.get(&cur) {
            path.push((prev.cell, bi, prev.entry, exit));
            cur = prev;
        }
        path.reverse();
        Some((path, total))
    }
}

pub fn solve(g: &Guide, p: &SolveParams) -> Option<Solution> {
    let mut ctx = Ctx::new(g, p);
    let start_pt = g.start();
    let start = *g
        .cells
        .keys()
        .min_by(|a, b| {
            let da = (a.0 as f64 + 0.5 - start_pt.0).powi(2) + (a.1 as f64 + 0.5 - start_pt.1).powi(2);
            let db = (b.0 as f64 + 0.5 - start_pt.0).powi(2) + (b.1 as f64 + 0.5 - start_pt.1).powi(2);
            da.total_cmp(&db)
        })?;
    let mut best: Option<(Vec<(Cell, usize, Conn, Conn)>, f64)> = None;
    for side in Side::ALL {
        for index in 0..3 {
            let e = Conn { side, index, kind: 0 };
            if let Some(r) = ctx.search(start, e) {
                if best.as_ref().is_none_or(|b| r.1 < b.1) {
                    best = Some(r);
                }
            }
        }
    }
    let (path, cost) = best?;
    // sanity: one tile per cell
    let mut seen = std::collections::HashSet::new();
    if !path.iter().all(|(c, ..)| seen.insert(*c)) {
        return None;
    }
    let minx = path.iter().map(|x| x.0 .0).min()?;
    let miny = path.iter().map(|x| x.0 .1).min()?;
    let mut tiles = HashMap::new();
    let mut devs = Vec::new();
    for (c, bi, ..) in &path {
        let b = &BLOCKS[*bi];
        tiles.insert((c.0 - minx, c.1 - miny), TileId { atlas: b.atlas + p.surface.offset(), alt: b.alt });
        devs.push(ctx.raw_dev(*c, *bi));
    }
    // start where the course starts, on a plain (normal-type) entry
    let mut gi = 0;
    while path[gi].2.kind != 0 {
        gi = (gi + 1) % path.len();
    }
    let (sc, _, se, _) = path[gi];
    let mut track = Track {
        id: uuid::Uuid::new_v4().to_string(),
        tiles,
        start: (sc.0 - minx, sc.1 - miny),
        start_entry: se,
        lap_score: 0.0,
        extra: Default::default(),
    };
    track.recompute_lap_score();
    let mean_dev = devs.iter().sum::<f64>() / devs.len() as f64;
    let max_dev = devs.iter().cloned().fold(0.0, f64::max);
    Some(Solution { track, cost, mean_dev, max_dev, offset: (minx, miny) })
}
