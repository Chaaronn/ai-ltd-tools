//! Finding saves that were copied from another save and trained on separately ("forks").
//!
//! A copy gets a new `Id`, so forks are found by comparing generation records: if the first
//! `k` records of two saves are identical, they share those `k` generations of history.

/// What one save shares with the others.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ForkInfo {
    /// The save this one was copied from, and how many generations they share.
    pub parent: Option<(usize, usize)>,
    /// True when the parent's whole history is a prefix of this one (a real copy point); false
    /// when the two diverged and the common ancestor isn't saved.
    pub parent_is_prefix: bool,
    /// Generations from here on are this save's own; earlier ones are counted at another save.
    pub own_from: usize,
}

/// Number of identical leading records.
pub fn shared_prefix(a: &[u64], b: &[u64]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// `hashes[i]` are save i's record hashes, `mtimes[i]` its modification time (seconds).
///
/// Saves are put in order of (history length, mtime). Each save's shared history is counted
/// once, at the first save in that order that has it, so copies never double the evidence.
pub fn detect(hashes: &[Vec<u64>], mtimes: &[f64]) -> Vec<ForkInfo> {
    let n = hashes.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| hashes[a].len().cmp(&hashes[b].len()).then(mtimes[a].total_cmp(&mtimes[b])));
    let mut pos = vec![0; n];
    for (p, &i) in order.iter().enumerate() {
        pos[i] = p;
    }
    let mut out = vec![ForkInfo::default(); n];
    for &f in &order {
        if hashes[f].is_empty() {
            continue;
        }
        // (shared, is_prefix, candidate)
        let mut best: Option<(usize, bool, usize)> = None;
        for &g in &order[..pos[f]] {
            let k = shared_prefix(&hashes[f], &hashes[g]);
            if k == 0 {
                continue;
            }
            let prefix = k == hashes[g].len();
            let better = match best {
                None => true,
                // more shared history wins; on a tie, a true prefix beats a sibling and the
                // earlier save (in order) is kept
                Some((bk, bp, _)) => k > bk || (k == bk && prefix && !bp),
            };
            if better {
                best = Some((k, prefix, g));
            }
        }
        if let Some((k, prefix, g)) = best {
            out[f] = ForkInfo { parent: Some((g, k)), parent_is_prefix: prefix, own_from: k };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families() {
        // a: root; b = a + more; c = a + other; d = b exactly (copied, no training); e unrelated
        let a = vec![1, 2, 3];
        let b = vec![1, 2, 3, 4, 5];
        let c = vec![1, 2, 3, 9];
        let d = b.clone();
        let e = vec![7, 8];
        let f = detect(&[a, b, c, d, e], &[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(f[0].parent, None);
        assert_eq!(f[1].parent, Some((0, 3)));
        assert_eq!(f[2].parent, Some((0, 3)));
        assert_eq!(f[3].parent, Some((1, 5)));
        assert_eq!(f[3].own_from, 5);
        assert_eq!(f[4].parent, None);
        assert!(f[1].parent_is_prefix);
    }

    #[test]
    fn siblings_without_saved_ancestor() {
        let f = detect(&[vec![1, 2, 5], vec![1, 2, 6, 7]], &[2.0, 1.0]);
        assert_eq!(f[0].parent, None);
        assert_eq!(f[1].parent, Some((0, 2)));
        assert!(!f[1].parent_is_prefix);
    }
}
