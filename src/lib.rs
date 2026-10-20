//! Track Builder for "AI Learns To Drive": turn a course map image into a `.track` file.
//!
//! Input: PNG / JPG / BMP / WebP / GIF or SVG ([`input`]).
//! Pipeline: [`trace`] (image -> centre-line loop) -> [`fit`] (orient, exaggerate, scale) ->
//! [`solve`] (tiles) -> [`game::track::Track`] (validate, score, save).
pub mod autotune;
pub mod fit;
pub mod game;
pub mod input;
pub mod lab;
pub mod pipeline;
pub mod solve;
pub mod simulation;
pub mod trace;
