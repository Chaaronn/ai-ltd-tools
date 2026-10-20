//! Evidence ledger: a daily record of how much clean evidence each open question has, so the
//! Data gathering tab can show progress over time.
//!
//! The third fenced write: one file, `evidence-ledger.json`, in the tool's own settings folder
//! (next to the track names), never the game's folder. At most one entry per day, newest kept,
//! 180 days at most.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};

/// One day's counts: question id -> clean data points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub date: String,
    pub values: BTreeMap<String, f64>,
}

pub const KEEP_DAYS: usize = 180;

pub fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("Cat's AI Ltd Tools").join("evidence-ledger.json"))
}

/// Today's date (UTC) as YYYY-MM-DD.
pub fn today() -> String {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64;
    civil(days)
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
pub fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn load_from(p: &Path) -> Vec<Entry> {
    let Ok(s) = std::fs::read_to_string(p) else { return Vec::new() };
    let Ok(Value::Array(a)) = serde_json::from_str::<Value>(&s) else { return Vec::new() };
    a.iter()
        .filter_map(|e| {
            let date = e.get("date")?.as_str()?.to_string();
            let values = e.get("values")?.as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect();
            Some(Entry { date, values })
        })
        .collect()
}

pub fn load() -> Vec<Entry> {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

/// Add or replace today's entry. Returns the updated ledger (oldest first).
pub fn with_today(mut ledger: Vec<Entry>, today: Entry) -> Vec<Entry> {
    ledger.retain(|e| e.date != today.date);
    ledger.push(today);
    ledger.sort_by(|a, b| a.date.cmp(&b.date));
    let extra = ledger.len().saturating_sub(KEEP_DAYS);
    ledger.drain(..extra);
    ledger
}

/// Write the ledger to `p` (a temp file, then a rename). Refuses any path inside the game's
/// data folder ("AI Learns To Drive").
pub fn save_to(p: &Path, ledger: &[Entry]) -> Result<()> {
    if p.components().any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case("AI Learns To Drive")) {
        bail!("refusing to write inside the game folder: {}", p.display());
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let arr: Vec<Value> = ledger
        .iter()
        .map(|e| {
            let mut o = Map::new();
            o.insert("date".into(), Value::String(e.date.clone()));
            o.insert("values".into(), Value::Object(e.values.iter().map(|(k, v)| (k.clone(), serde_json::json!(v))).collect()));
            Value::Object(o)
        })
        .collect();
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&Value::Array(arr))?).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, p).with_context(|| format!("saving {}", p.display()))?;
    Ok(())
}

/// Record today's counts if they differ from what's stored for today. Returns the ledger.
pub fn record(values: BTreeMap<String, f64>) -> Result<Vec<Entry>> {
    let Some(p) = path() else { bail!("no settings folder on this system") };
    let old = load_from(&p);
    let entry = Entry { date: today(), values };
    if old.iter().any(|e| *e == entry) {
        return Ok(old);
    }
    let new = with_today(old, entry);
    save_to(&p, &new)?;
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(20_745), "2026-10-19");
        assert_eq!(civil(11_016), "2000-02-29");
    }

    #[test]
    fn one_entry_per_day_and_capped() {
        let e = |d: &str, v: f64| Entry { date: d.into(), values: [("x".to_string(), v)].into_iter().collect() };
        let l = with_today(vec![e("2026-10-18", 1.0), e("2026-10-19", 2.0)], e("2026-10-19", 3.0));
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].values["x"], 3.0);
        let many: Vec<Entry> = (0..200).map(|i| e(&civil(i), i as f64)).collect();
        assert_eq!(with_today(many, e(&civil(500), 0.0)).len(), KEEP_DAYS);
    }
}
