//! Learning ASUS-specific opcodes from the monitor.
//!
//! For each named value the user selects it in the monitor's own OSD; the
//! register whose reading differs for every value is the feature's opcode.
//! The pure functions here are shared by the CLI wizard and the GUI.

use std::collections::{BTreeMap, BTreeSet};

use crate::features::Feature;
use crate::manager::{DisplayState, Manager};

/// Opcode -> current value for every register the monitor answers.
pub type Scan = BTreeMap<u8, u16>;

/// Registers that change on their own (frequency counters, "active control")
/// and are never a candidate.
pub const ALWAYS_NOISY: &[u8] = &[0x52, 0xAC, 0xAE, 0xC0];

/// Registers that differ between two scans taken with nothing touched.
pub fn noisy_codes(a: &Scan, b: &Scan) -> BTreeSet<u8> {
    let mut noisy: BTreeSet<u8> = ALWAYS_NOISY.iter().copied().collect();
    for code in a.keys().chain(b.keys()) {
        if a.get(code) != b.get(code) {
            noisy.insert(*code);
        }
    }
    noisy
}

/// Did any steady register differ between two scans?
pub fn changed(before: &Scan, after: &Scan, noisy: &BTreeSet<u8>) -> bool {
    before
        .keys()
        .chain(after.keys())
        .filter(|c| !noisy.contains(c))
        .any(|c| before.get(c) != after.get(c))
}

/// Registers whose value is different for every named state.
pub fn candidates(shots: &[(String, Scan)], noisy: &BTreeSet<u8>) -> Vec<u8> {
    if shots.len() < 2 {
        return Vec::new();
    }
    let mut common: BTreeSet<u8> = shots[0].1.keys().copied().collect();
    for (_, s) in &shots[1..] {
        common = common.intersection(&s.keys().copied().collect()).copied().collect();
    }
    common
        .into_iter()
        .filter(|c| !noisy.contains(c))
        .filter(|c| {
            let vals: BTreeSet<u16> = shots.iter().map(|(_, s)| s[c]).collect();
            vals.len() == shots.len()
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing changed in any step: the OSD item does not exist here.
    NothingChanged,
    /// Only one value changed: cannot tell registers apart.
    NeedMore,
    /// Values changed but no single register tracked them.
    NoRegister,
    /// One or more registers matched.
    Found(Vec<u8>),
}

/// An in-progress learn run (one feature on one display).
pub struct LearnSession {
    pub feature: &'static Feature,
    pub names: Vec<String>,
    codes: Vec<u8>,
    noisy: BTreeSet<u8>,
    prev: Scan,
    pub shots: Vec<(String, Scan)>,
}

impl LearnSession {
    /// Scans the monitor (slow: every unanswered register costs a timeout).
    pub fn begin(mgr: &Manager, state: &DisplayState, feature: &'static Feature, names: Vec<String>) -> LearnSession {
        Self::begin_with_progress(mgr, state, feature, names, &mut |_, _| {})
    }

    /// Like [`begin`], reporting `(done, total)` while the full scan runs.
    pub fn begin_with_progress(
        mgr: &Manager,
        state: &DisplayState,
        feature: &'static Feature,
        names: Vec<String>,
        progress: &mut dyn FnMut(usize, usize),
    ) -> LearnSession {
        let first = mgr.scan_progress(state, None, progress);
        let codes: Vec<u8> = first.keys().copied().collect();
        let second = mgr.scan(state, Some(&codes));
        let noisy = noisy_codes(&first, &second);
        LearnSession { feature, names, codes, noisy, prev: first, shots: Vec::new() }
    }

    /// Capture the monitor's state for `name`. Returns false when nothing
    /// changed since the previous step (the value is ignored, e.g. because the
    /// monitor's menu does not have it).
    pub fn capture(&mut self, mgr: &Manager, state: &DisplayState, name: &str) -> bool {
        let shot = mgr.scan(state, Some(&self.codes));
        if !changed(&self.prev, &shot, &self.noisy) {
            return false;
        }
        self.prev = shot.clone();
        self.shots.push((name.to_string(), shot));
        true
    }

    pub fn outcome(&self) -> Outcome {
        match self.shots.len() {
            0 => Outcome::NothingChanged,
            1 => Outcome::NeedMore,
            _ => {
                let found = candidates(&self.shots, &self.noisy);
                if found.is_empty() {
                    Outcome::NoRegister
                } else {
                    Outcome::Found(found)
                }
            }
        }
    }

    /// Values learned for `vcp`.
    pub fn values(&self, vcp: u8) -> BTreeMap<String, u16> {
        self.shots.iter().map(|(n, s)| (n.clone(), s[&vcp])).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(pairs: &[(u8, u16)]) -> Scan {
        pairs.iter().copied().collect()
    }

    #[test]
    fn picks_the_register_that_tracks_the_osd() {
        let base = scan(&[(0x10, 50), (0x52, 8), (0xDC, 13), (0xE6, 0)]);
        let noisy = noisy_codes(&base, &scan(&[(0x10, 51), (0x52, 8), (0xDC, 13), (0xE6, 0)]));
        assert!(noisy.contains(&0x10), "0x10 drifted between idle scans");
        let shots = vec![
            ("Level 0".to_string(), scan(&[(0x10, 50), (0x52, 8), (0xDC, 13), (0xE6, 0)])),
            ("Level 1".to_string(), scan(&[(0x10, 50), (0x52, 9), (0xDC, 13), (0xE6, 1)])),
            ("Level 2".to_string(), scan(&[(0x10, 50), (0x52, 10), (0xDC, 13), (0xE6, 2)])),
        ];
        assert_eq!(candidates(&shots, &noisy), vec![0xE6]);
        assert!(candidates(&shots[..1], &noisy).is_empty());
    }

    #[test]
    fn ignores_steps_where_only_noise_moved() {
        let base = scan(&[(0xE6, 0), (0x52, 8)]);
        let noisy: BTreeSet<u8> = [0x52].into_iter().collect();
        assert!(!changed(&base, &scan(&[(0xE6, 0), (0x52, 9)]), &noisy));
        assert!(changed(&base, &scan(&[(0xE6, 1), (0x52, 8)]), &noisy));
    }

    #[test]
    fn a_register_that_repeats_a_value_is_not_a_candidate() {
        let noisy = BTreeSet::new();
        let shots = vec![
            ("a".to_string(), scan(&[(0xE6, 1)])),
            ("b".to_string(), scan(&[(0xE6, 2)])),
            ("c".to_string(), scan(&[(0xE6, 1)])),
        ];
        assert!(candidates(&shots, &noisy).is_empty());
    }
}
