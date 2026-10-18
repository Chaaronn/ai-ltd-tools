//! Names the player gives to built-in tracks. The game's own tracks are packed inside the game
//! (encrypted), so a save only knows a built-in track by its `TrackId`. These names live in the
//! tool's own settings folder, never in the game's folder.
//!
//! The second fenced exception to "the Lab never writes": this module writes one file,
//! `track-names.json`, under the tool's config folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// %APPDATA%\Cat's AI Ltd Tools\track-names.json
pub fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("Cat's AI Ltd Tools").join("track-names.json"))
}

/// TrackId -> name. Empty if the file is missing or unreadable.
pub fn load() -> HashMap<String, String> {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

pub fn load_from(p: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Write the names to `p` (atomically: a temp file, then a rename). Refuses any path inside the
/// game's data folder ("AI Learns To Drive").
pub fn save_to(p: &Path, names: &HashMap<String, String>) -> Result<()> {
    if p.components().any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case("AI Learns To Drive")) {
        bail!("refusing to write inside the game folder: {}", p.display());
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut sorted: Vec<(&String, &String)> = names.iter().filter(|(_, v)| !v.trim().is_empty()).collect();
    sorted.sort();
    let map: serde_json::Map<String, serde_json::Value> = sorted.into_iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.trim().to_string()))).collect();
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&map)?).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, p).with_context(|| format!("saving {}", p.display()))?;
    Ok(())
}

pub fn store(names: &HashMap<String, String>) -> Result<()> {
    let Some(p) = path() else { bail!("no settings folder on this system") };
    save_to(&p, names)
}
