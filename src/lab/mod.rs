//! Evolution Lab: an evaluate-only analysis of the game's saved networks (`.evolution` files).
//!
//! Nothing in this module writes, creates, copies, renames or deletes a file, with one fenced
//! exceptions: `keeper` writes "<name> (best lap).evolution" copies (and only those), and `names` writes
//! the player's track names to the tool's own settings folder. A test
//! (`tests/lab.rs`) checks the source for write calls.
pub mod advice;
pub mod compare;
pub mod designs;
pub mod format;
pub mod forks;
pub mod keeper;
pub mod library;
pub mod names;
pub mod netviz;
pub mod overview;
pub mod projection;
pub mod runs;
pub mod verdict;
