//! One-call build used by both the GUI and the CLI.
use anyhow::{anyhow, Result};

use crate::fit::{catmull_rom_closed, orient, FitParams, Guide, P};
use crate::solve::{solve, SolveParams, Solution};
use crate::trace::Traced;

#[derive(Clone, Debug, Default)]
pub struct BuildParams {
    pub fit: FitParams,
    pub solve: SolveParams,
    /// Start/finish in image pixels (nearest point on the traced line is used).
    pub start_px: Option<P>,
    pub reverse: bool,
}

pub struct Built {
    pub solution: Solution,
    pub guide: Guide,
}

pub fn build(traced: &Traced, p: &BuildParams) -> Result<Built> {
    build_points(&traced.points, p)
}

/// Build from any closed course line in image pixels (traced, edited, or drawn by hand).
pub fn build_points(line: &[P], p: &BuildParams) -> Result<Built> {
    if line.len() < 3 {
        return Err(anyhow!("the course line needs at least 3 points"));
    }
    // sparse lines (edited or hand-drawn) become smooth curves through their points
    let n = line.len();
    let perim: f64 = (0..n).map(|i| {
        let (a, b) = (line[i], line[(i + 1) % n]);
        ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
    }).sum();
    let dense = if perim / n as f64 > 2.0 { catmull_rom_closed(line, 1.0) } else { line.to_vec() };
    let pts = orient(&dense, p.start_px, p.reverse);
    let guide = Guide::build(&pts, &p.fit);
    let solution = solve(&guide, &p.solve).ok_or_else(|| {
        anyhow!("no closed track found - try less exaggeration, a lower fidelity, or allowing angled pieces")
    })?;
    Ok(Built { solution, guide })
}
