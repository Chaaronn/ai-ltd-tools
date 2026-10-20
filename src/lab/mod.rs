//! Evolution Lab: an evaluate-only analysis of the game's saved networks (`.evolution` files).
//!
//! Nothing in this module writes, creates, copies, renames or deletes a file, with these fenced
//! exceptions: `keeper` writes "<name> (best lap · <track>).evolution" copies (and only those), `names` writes
//! the player's track names and `ledger` a daily evidence record, both to the tool's own settings folder. A test
//! (`tests/lab.rs`) checks the source for write calls.
pub mod advice;
pub mod compare;
pub mod designs;
pub mod detectability;
pub mod format;
pub mod forks;
pub mod gather;
pub mod h2h;
pub mod keeper;
pub mod ledger;
pub mod library;
pub mod mutation;
pub mod names;
pub mod netviz;
pub mod overview;
pub mod projection;
pub mod runs;
pub mod usage;
pub mod verdict;
