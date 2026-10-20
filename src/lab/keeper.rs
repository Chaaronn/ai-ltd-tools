//! Best-lap keeper: when a network being trained saves a new best lap, write a copy of that
//! save as "<name> (best lap · <track>).evolution", one copy per network and track, overwriting
//! a copy only with a faster lap on the same track. (Copies were once one per network,
//! "<name> (best lap).evolution"; switching tracks then replaced the old track's best. Those
//! older copies are still recognised, and count for the track their saved network was on.)
//! Which track a copy holds is read from the copy itself (its last generation), not its name,
//! so renaming a track never orphans a copy.
//!
//! This is the Lab's main write, and it's fenced in:
//! - it only ever writes files whose name is a keeper name (checked by `keeper_path` and again
//!   by `write_copy`);
//! - it never modifies, renames or deletes the game's own saves;
//! - the copy is the save's own JSON with a new `Name` and a new random `Id` (a version-4 GUID,
//!   so the game doesn't treat it as modded), gzip-compressed like the game's files;
//! - it writes to a temporary file first and renames it into place, so the game never sees a
//!   half-written copy.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde_json::Value;

use super::advice;
use super::format::Save;
use super::library::{Library, Network};

/// Every keeper copy's name contains this, followed by ")" or " · <track>)".
pub const MARK: &str = " (best lap";
/// The older one-per-network suffix.
pub const SUFFIX: &str = " (best lap)";
/// Only networks saved this recently are being trained (seconds).
pub const ACTIVE_SECONDS: f64 = 15.0 * 60.0;

/// True for files the keeper made (file stem, without ".evolution").
pub fn is_keeper_copy(file: &str) -> bool {
    file.ends_with(')') && file.contains(MARK)
}

/// The save a keeper copy was made from.
pub fn source_of(file: &str) -> Option<&str> {
    if !is_keeper_copy(file) {
        return None;
    }
    file.rfind(MARK).map(|i| &file[..i])
}

/// A track name made safe for a Windows file name.
pub fn track_label(name: &str) -> String {
    let s: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(40).collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "track".into() } else { s }
}

/// Where a new copy of `file` for a track labelled `label` goes. Refuses anything that wouldn't
/// be a keeper copy.
pub fn keeper_path(dir: &Path, file: &str, label: &str) -> Result<PathBuf> {
    if is_keeper_copy(file) || file.is_empty() || file.contains(['/', '\\']) || label.contains(['/', '\\']) {
        bail!("not a source save: {file}");
    }
    let p = dir.join(format!("{file}{MARK} · {}).evolution", track_label(label)));
    let stem = p.file_stem().and_then(|n| n.to_str()).unwrap_or_default();
    if source_of(stem) != Some(file) || p.parent() != Some(dir) {
        bail!("refusing to write {}", p.display());
    }
    Ok(p)
}

/// The track key a save's network was last trained on (what a keeper copy holds).
fn last_track(save: &Save) -> Option<String> {
    super::runs::track_keys(&save.gens).pop()
}

/// The existing keeper copy of network `file` for `track`, if there is one.
pub fn copy_for<'a>(lib: &'a Library, file: &str, track: &str) -> Option<&'a Network> {
    lib.networks.iter().filter(|o| source_of(&o.file) == Some(file)).find(|o| last_track(&o.save).as_deref() == Some(track)).map(|o| &**o)
}

#[derive(Clone, Debug)]
pub struct Kept {
    pub source: String,
    pub copy: String,
    pub seconds: f64,
    pub track: String,
    pub generation: usize,
    /// The previous copy's best lap on that track, if there was one.
    pub previous: Option<f64>,
}

/// Best lap on `track` in a save (its own history included).
fn best_on(save: &Save, track: &str) -> Option<f64> {
    let keys = super::runs::track_keys(&save.gens);
    save.gens.iter().zip(&keys).filter(|(_, k)| *k == track).filter_map(|(g, _)| g.best_lap).reduce(f64::min)
}

/// What the keeper would do for one network: Some((copy path, previous copy's lap)) when a
/// copy should be written now.
pub fn due(lib: &Library, n: &Network, now: f64) -> Option<(PathBuf, Option<f64>)> {
    if is_keeper_copy(&n.file) || n.error.is_some() || now - n.mtime > ACTIVE_SECONDS {
        return None;
    }
    let b = advice::best_lap(lib, n)?;
    // the network in the file must be the one that set the best: the latest generation did
    if b.at + 1 != n.gens() {
        return None;
    }
    let track = &n.runs.last()?.track;
    // only a copy holding this track is ever replaced; copies for other tracks are left alone
    let (path, previous) = match copy_for(lib, &n.file, track) {
        Some(o) => (o.path.clone(), best_on(&o.save, track)),
        None => {
            let mut p = keeper_path(&lib.dir, &n.file, &b.track_name).ok()?;
            // a copy with that name already holds another track (e.g. a renamed track): use the id
            if lib.networks.iter().any(|o| o.path == p) {
                p = keeper_path(&lib.dir, &n.file, &format!("{} {}", b.track_name, &track[..track.len().min(8)])).ok()?;
                if lib.networks.iter().any(|o| o.path == p) {
                    return None;
                }
            }
            (p, None)
        }
    };
    if previous.is_some_and(|p| p <= b.seconds) {
        return None;
    }
    Some((path, previous))
}

/// Copy `source` to `dest` with a new name and id. `expect_gens` guards against the game
/// having saved again since the Lab read it.
pub fn write_copy(source: &Path, dest: &Path, expect_gens: usize) -> Result<()> {
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if !name.ends_with(".evolution") || !is_keeper_copy(name.trim_end_matches(".evolution")) {
        bail!("refusing to write {}", dest.display());
    }
    let bytes = std::fs::read(source).with_context(|| format!("reading {}", source.display()))?;
    let mut text = String::new();
    GzDecoder::new(&bytes[..]).read_to_string(&mut text).context("not a gzip save")?;
    let mut v: Value = serde_json::from_str(&text).context("save isn't valid JSON (the game may be writing it)")?;
    let gens = v.pointer("/Stats/Generations").and_then(Value::as_array).map_or(0, Vec::len);
    if gens != expect_gens {
        bail!("the save changed since it was read ({gens} generations, expected {expect_gens})");
    }
    let obj = v.as_object_mut().context("save isn't a JSON object")?;
    let copy_name = name.trim_end_matches(".evolution").to_string();
    obj.insert("Name".into(), Value::String(copy_name));
    obj.insert("Id".into(), Value::String(uuid::Uuid::new_v4().to_string()));
    let out = serde_json::to_string(&v)?;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(out.as_bytes())?;
    let gz = enc.finish()?;
    let tmp = dest.with_extension("evolution.tmp");
    std::fs::write(&tmp, &gz).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, dest).with_context(|| format!("replacing {}", dest.display()))?;
    Ok(())
}

/// Check every network and write the copies that are due. Returns what was written and any
/// errors (a failed copy is retried on the next refresh).
pub fn run(lib: &Library, now: f64) -> (Vec<Kept>, Vec<String>) {
    let mut kept = Vec::new();
    let mut errors = Vec::new();
    for n in &lib.networks {
        let Some((dest, previous)) = due(lib, n, now) else { continue };
        let Some(b) = advice::best_lap(lib, n) else { continue };
        match write_copy(&n.path, &dest, n.gens()) {
            Ok(()) => kept.push(Kept {
                source: n.file.clone(),
                copy: dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                seconds: b.seconds,
                track: b.track_name.clone(),
                generation: b.at,
                previous,
            }),
            Err(e) => errors.push(format!("{}: {e:#}", n.file)),
        }
    }
    (kept, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_keeper_names_are_writable() {
        let d = Path::new("/tmp/x");
        assert!(keeper_path(d, "snow nosw", "Autumn 06").unwrap().ends_with("snow nosw (best lap · Autumn 06).evolution"));
        assert!(keeper_path(d, "snow nosw (best lap)", "Autumn 06").is_err(), "no copies of copies");
        assert!(keeper_path(d, "snow nosw (best lap · Autumn 06)", "x").is_err());
        assert!(keeper_path(d, "../evil", "x").is_err());
        assert!(keeper_path(d, "a", "../../evil").is_err());
        assert!(write_copy(Path::new("a.evolution"), Path::new("/tmp/x/real save.evolution"), 0).is_err());
        assert!(write_copy(Path::new("a.evolution"), Path::new("/tmp/x/real (best lap) save.evolution"), 0).is_err());
    }

    #[test]
    fn names_round_trip() {
        assert_eq!(source_of("Type 2 (best lap)"), Some("Type 2"), "old-style copies still recognised");
        assert_eq!(source_of("Type 2 (best lap · Autumn 06)"), Some("Type 2"));
        assert_eq!(source_of("Type 2"), None);
        assert!(!is_keeper_copy("my (best lap) notes"));
        assert_eq!(track_label(r#"Las: Vegas/"Night"?"#), "Las Vegas Night");
        assert_eq!(track_label("..."), "track");
    }
}
