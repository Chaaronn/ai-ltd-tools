//! Scanning the game's evolution folder and analysing every save. Files are only ever opened
//! for reading; results live in memory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use super::format::Save;
use super::forks::{self, ForkInfo};
use super::runs::{self, Run, Stall};
use super::verdict::{self, Verdict};

/// One save and everything worked out from it.
#[derive(Clone, Debug)]
pub struct Network {
    pub path: PathBuf,
    /// File name without `.evolution`.
    pub file: String,
    pub mtime: f64,
    pub save: Arc<Save>,
    /// Set when the latest read failed (e.g. the game was mid-save) and an older copy is shown,
    /// or nothing could be read at all.
    pub error: Option<String>,
    pub runs: Vec<Run>,
    pub stalls: Vec<Stall>,
    /// Generations since the median last made a real gain.
    pub since_gain: usize,
    pub lap_milestones: Vec<(usize, f64)>,
    pub verdicts: Vec<Verdict>,
    pub fork: ForkInfo,
}

impl Network {
    pub fn gens(&self) -> usize {
        self.save.gens.len()
    }
    /// Generations the latest run still needs before its start can get a verdict.
    pub fn gens_to_verdict(&self, min_side: usize) -> Option<usize> {
        let last = self.runs.last()?;
        if self.runs.len() < 2 {
            return None;
        }
        (last.len() < min_side).then(|| min_side - last.len())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Library {
    pub dir: PathBuf,
    pub networks: Vec<Arc<Network>>,
    pub threshold: f64,
    pub placebo_points: usize,
    /// The same for verdicts judged on lap time (reward changes).
    pub lap_threshold: f64,
    pub lap_placebo_points: usize,
    pub window: usize,
    pub min_side: usize,
    /// Track `Id` -> name, from the game's track folders (built-in tracks aren't readable),
    /// overlaid with the names the player gave in the Lab (`names`).
    pub track_names: HashMap<String, String>,
    /// Ids the player named (a subset of `track_names`).
    pub user_named: std::collections::HashSet<String>,
    /// Track `Id` -> the network with most of its own generations there (a hint for unnamed tracks).
    pub track_hints: HashMap<String, String>,
    /// "Adaptive mutation rate" in the game's settings now (see `mutation`).
    pub adaptive_mutation: Option<bool>,
}

impl Library {
    pub fn by_file(&self, file: &str) -> Option<&Arc<Network>> {
        self.networks.iter().find(|n| n.file == file)
    }
    pub fn mtimes(&self) -> HashMap<PathBuf, f64> {
        self.networks.iter().map(|n| (n.path.clone(), n.mtime)).collect()
    }
}

/// The game's evolution folder: %APPDATA%\AI Learns To Drive\evolution
pub fn default_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("AI Learns To Drive").join("evolution"))
}

/// `.evolution` files in `dir` with their modification times.
pub fn list(dir: &Path) -> Vec<(PathBuf, f64)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<(PathBuf, f64)> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("evolution")))
        .map(|p| {
            let t = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0.0, |d| d.as_secs_f64());
            (p, t)
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// True if the folder's files or their times differ from what `lib` was built from.
pub fn changed(lib: &Library) -> bool {
    let now = list(&lib.dir);
    let old = lib.mtimes();
    now.len() != old.len() || now.iter().any(|(p, t)| old.get(p) != Some(t))
}

/// Read and analyse every save in `dir`. Saves whose mtime matches `previous` are reused
/// without re-reading. `window` / `min_side` configure the before/after test.
pub fn scan(dir: &Path, previous: Option<&Library>, window: usize, min_side: usize) -> Library {
    let cached: HashMap<PathBuf, Arc<Network>> =
        previous.map(|l| l.networks.iter().map(|n| (n.path.clone(), n.clone())).collect()).unwrap_or_default();
    let mut loaded: Vec<(PathBuf, String, f64, Arc<Save>, Option<String>)> = Vec::new();
    for (path, mtime) in list(dir) {
        let file = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let old = cached.get(&path);
        if let Some(o) = old.filter(|o| o.mtime == mtime && o.error.is_none()) {
            loaded.push((path, file, mtime, o.save.clone(), None));
            continue;
        }
        match Save::load(&path) {
            Ok(s) => loaded.push((path, file, mtime, Arc::new(s), None)),
            Err(e) => {
                let msg = format!("{e:#}");
                match old {
                    // keep showing the last good copy and retry on the next refresh
                    Some(o) => loaded.push((path, file, o.mtime, o.save.clone(), Some(format!("{msg} (showing the last good read)")))),
                    None => loaded.push((path, file, 0.0, Arc::new(empty_save()), Some(msg))),
                }
            }
        }
    }
    let mut lib = analyse(dir, loaded, window, min_side);
    let mut names = track_names(dir);
    let user = super::names::load();
    lib.user_named = user.keys().cloned().collect();
    names.extend(user);
    lib.track_names = names;
    lib.adaptive_mutation = super::mutation::adaptive_setting(dir);
    lib
}

/// Names of the tracks saved next to the evolution folder: `tracks`, `track_of_the_week` and
/// `workshop_tracks`. Read-only; files that don't parse are skipped.
pub fn track_names(evolution_dir: &Path) -> HashMap<String, String> {
    use std::io::Read;
    let mut out = HashMap::new();
    let Some(root) = evolution_dir.parent() else { return out };
    for sub in ["tracks", "track_of_the_week", "workshop_tracks"] {
        let Ok(rd) = std::fs::read_dir(root.join(sub)) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("track")) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&p) else { continue };
            let mut s = String::new();
            if flate2::read::GzDecoder::new(&bytes[..]).read_to_string(&mut s).is_err() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else { continue };
            if let (Some(id), Some(name)) = (v.get("Id").and_then(|x| x.as_str()), p.file_stem()) {
                out.insert(id.to_string(), name.to_string_lossy().into_owned());
            }
        }
    }
    out
}

fn empty_save() -> Save {
    Save {
        id: String::new(),
        name: String::new(),
        vehicle: None,
        shape: Vec::new(),
        outputs: Vec::new(),
        sensors: 0,
        gens: Vec::new(),
        arch: Default::default(),
        snapshots: Vec::new(),
        weights: None,
        snapshot_weights: Vec::new(),
        output_order: Vec::new(),
        rays: Vec::new(),
    }
}

/// Analysis for saves already in memory (also used by tests).
pub fn analyse(dir: &Path, loaded: Vec<(PathBuf, String, f64, Arc<Save>, Option<String>)>, window: usize, min_side: usize) -> Library {
    // best-lap copies made by the keeper hang off their source, and never become anyone's parent
    let keeper: Vec<bool> = loaded.iter().map(|l| super::keeper::is_keeper_copy(&l.1)).collect();
    let hashes: Vec<Vec<u64>> = loaded
        .iter()
        .zip(&keeper)
        .map(|(l, k)| if *k { Vec::new() } else { l.3.gens.iter().map(|g| g.hash).collect() })
        .collect();
    let mtimes: Vec<f64> = loaded.iter().map(|l| l.2).collect();
    let mut fork = forks::detect(&hashes, &mtimes);
    for (i, l) in loaded.iter().enumerate().filter(|(i, _)| keeper[*i]) {
        let source = super::keeper::source_of(&l.1).unwrap_or_default();
        let len = l.3.gens.len();
        if let Some(j) = loaded.iter().position(|o| o.1 == source) {
            fork[i] = ForkInfo { parent: Some((j, len)), parent_is_prefix: true, own_from: len };
        } else {
            fork[i] = ForkInfo { parent: None, parent_is_prefix: false, own_from: len };
        }
    }
    let mut nets: Vec<Network> = loaded
        .into_iter()
        .zip(fork)
        .map(|((path, file, mtime, save, error), fork)| {
            let runs = runs::segment(&save);
            let (stalls, since_gain) = runs::stalls(&save);
            let lap_milestones = runs::lap_milestones(&save);
            Network { path, file, mtime, save, error, runs, stalls, since_gain, lap_milestones, verdicts: Vec::new(), fork }
        })
        .collect();
    let placebo: Vec<f64> =
        nets.iter().flat_map(|n| verdict::placebo_z(&n.save.gens, &n.runs, n.fork.own_from, window, min_side)).collect();
    let (threshold, placebo_points) = verdict::threshold(placebo);
    let lap_placebo: Vec<f64> =
        nets.iter().flat_map(|n| verdict::placebo_z_lap(&n.save.gens, &n.runs, n.fork.own_from, window, min_side)).collect();
    let (lap_threshold, lap_placebo_points) = verdict::threshold(lap_placebo);
    // the network that trained most on each track (own history), as a hint for unnamed tracks
    let mut most: HashMap<String, (String, usize)> = HashMap::new();
    for n in &nets {
        if super::keeper::is_keeper_copy(&n.file) {
            continue;
        }
        let mut per: HashMap<&str, usize> = HashMap::new();
        for g in n.save.gens.iter().skip(n.fork.own_from) {
            if let Some(t) = g.track.as_deref() {
                *per.entry(t).or_default() += 1;
            }
        }
        for (t, c) in per {
            let e = most.entry(t.to_string()).or_insert((n.file.clone(), 0));
            if c > e.1 {
                *e = (n.file.clone(), c);
            }
        }
    }
    let track_hints: HashMap<String, String> = most.into_iter().map(|(k, v)| (k, v.0)).collect();
    for n in &mut nets {
        n.verdicts = verdict::verdicts(&n.save.gens, &n.runs, threshold, lap_threshold, window, min_side);
    }
    Library {
        dir: dir.to_path_buf(),
        networks: nets.into_iter().map(Arc::new).collect(),
        threshold,
        placebo_points,
        lap_threshold,
        lap_placebo_points,
        window,
        min_side,
        track_names: HashMap::new(),
        user_named: Default::default(),
        track_hints,
        adaptive_mutation: None,
    }
}
