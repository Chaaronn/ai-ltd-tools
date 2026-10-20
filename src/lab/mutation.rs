//! What the game's mutation settings actually do, read from `AILearnsToDrive.dll` (Oct 2026).
//!
//! - Every child gets Gaussian noise on **every** weight and bias, every generation: the mutation
//!   rate is a noise size, not a chance of mutating. Weights between layers of `a` and `b` units
//!   get sd = rate × √(2 / (a + b)) ("Xavier"); biases get sd = 0.1 × rate.
//! - **Adaptive mutation rate** multiplies the rate by a fixed factor from the network's size
//!   (`MutationNormalization.GetXavierFactor`), so bigger networks get gentler mutation. It
//!   doesn't change during training.
//! - Saves record only the rate as set, not whether adaptive mutation was on. The game's own
//!   settings file says whether it's on now, which is all the Lab can know.

use std::path::Path;

use super::format::Arch;

/// The factor adaptive mutation applies to the rate for a network design (1.0 when unknown).
pub fn adaptive_factor(arch: &Arch) -> f64 {
    if arch.shape.len() < 2 {
        return 1.0;
    }
    let p = arch.params() as f64;
    let l = arch.shape.len() as f64;
    let d = 0.262982 + 0.050982 * (p + 1.0).ln() - 0.000139 * l - 0.000278 * l * l;
    if d <= 0.0 {
        return 1.0;
    }
    let f = 0.516777 / d;
    if f > 0.0 { f.clamp(0.1, 10.0) } else { 1.0 }
}

/// Whether "Adaptive mutation rate" is on in the game's settings now
/// (`<game folder>/settings/local.settings`, gzipped JSON next to the evolution folder). Read-only.
pub fn adaptive_setting(evolution_dir: &Path) -> Option<bool> {
    game_settings(evolution_dir)?.pointer("/Evolution/IsAdaptiveMutationRate")?.as_bool()
}

/// The game's settings file (`<game folder>/settings/local.settings`, next to the evolution
/// folder): the current evolution settings, including ones saves don't record. Read-only.
pub fn game_settings(evolution_dir: &Path) -> Option<serde_json::Value> {
    use std::io::Read;
    let path = evolution_dir.parent()?.join("settings").join("local.settings");
    let bytes = std::fs::read(path).ok()?;
    // the game gzips it, like its saves; accept plain JSON too
    let mut text = String::new();
    if flate2::read::GzDecoder::new(&bytes[..]).read_to_string(&mut text).is_err() {
        text = String::from_utf8(bytes).ok()?;
    }
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// "≈ 0.17 used" style note for a rate on a design, when adaptive mutation is on and the
/// factor moves the rate noticeably. `None` otherwise.
pub fn used_note(rate: Option<f64>, arch: &Arch, adaptive: Option<bool>) -> Option<String> {
    let rate = rate?;
    if adaptive != Some(true) {
        return None;
    }
    let f = adaptive_factor(arch);
    if (f - 1.0).abs() < 0.02 {
        return None;
    }
    Some(format!("≈ {} used", short_rate(rate * f)))
}

/// Longer explanation for hovers.
pub fn explain(arch: &Arch, adaptive: Option<bool>) -> String {
    let f = adaptive_factor(arch);
    let state = match adaptive {
        Some(true) => "is on in the game now",
        Some(false) => "is off in the game now",
        None => "couldn't be read from the game's settings",
    };
    format!(
        "Adaptive mutation rate {state}. When on, the game multiplies the rate by ×{f:.2} for this network ({} weights and biases): bigger networks get gentler mutation. Saves don't record whether it was on, so older runs may have used the rate as set. Mutation adds noise to every weight of every child each generation; the rate sets the size of that noise.",
        thousands(arch.params())
    )
}

fn short_rate(r: f64) -> String {
    if r >= 0.1 { format!("{r:.2}") } else if r >= 0.01 { format!("{r:.3}") } else { format!("{r:.4}") }
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arch(shape: &[u32]) -> Arch {
        Arch { shape: shape.to_vec(), sensors: Vec::new(), outputs: Vec::new() }
    }

    #[test]
    fn factors_match_the_game_formula() {
        // 20-16-16-12-11-5: 1,015 params -> about 0.85; small networks stay near 1
        assert!((adaptive_factor(&arch(&[20, 16, 16, 12, 11, 5])) - 0.85).abs() < 0.01);
        assert!((adaptive_factor(&arch(&[18, 6, 4, 5, 5])) - 0.98).abs() < 0.01);
        assert_eq!(adaptive_factor(&arch(&[])), 1.0);
        // bigger networks always get less
        assert!(adaptive_factor(&arch(&[20, 64, 64, 5])) < adaptive_factor(&arch(&[20, 16, 5])));
    }

    #[test]
    fn note_only_when_on_and_it_matters() {
        let big = arch(&[20, 16, 16, 12, 11, 5]);
        assert_eq!(used_note(Some(0.2), &big, Some(true)).as_deref(), Some("≈ 0.17 used"));
        assert_eq!(used_note(Some(0.2), &big, Some(false)), None);
        assert_eq!(used_note(Some(0.2), &big, None), None);
        assert_eq!(used_note(Some(0.2), &arch(&[18, 6, 4, 5, 5]), Some(true)), None);
    }

}
