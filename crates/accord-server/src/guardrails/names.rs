//! Channel-name heuristics for anti-griefing.
//!
//! Two cheap, dependency-free signals catch the common raid patterns:
//! * **Random-string names** (`xk7f2qz`, `aaaa1111`) — low vowel ratio, high
//!   digit ratio, or very low character variety.
//! * **Low-variance spam** — a new name nearly identical to several recent
//!   channel names (`raid-1`, `raid-2`, `raid-3` …), via normalized edit
//!   distance.
//!
//! These only *flag* (the rate limiter is the hard wall); flagging drives the
//! audit log + `ModAlert` so admins notice a griefer even within rate.

/// Outcome of a name assessment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameVerdict {
    Ok,
    Suspicious { reason: String },
}

/// How many recent names must be near-identical to count as low-variance spam.
const SPAM_CLUSTER: usize = 3;
/// Similarity at/above this (0..1) counts two names as "near-identical".
const SIMILAR: f64 = 0.8;

/// Assess a proposed channel `name` against `recent` channel names.
#[must_use]
pub fn assess_name(name: &str, recent: &[String]) -> NameVerdict {
    let norm = normalize(name);
    if norm.is_empty() {
        return NameVerdict::Ok; // empty handled by validation elsewhere
    }

    if let Some(reason) = looks_random(&norm) {
        return NameVerdict::Suspicious { reason };
    }

    let similar = recent
        .iter()
        .filter(|r| similarity(&norm, &normalize(r)) >= SIMILAR)
        .count();
    if similar >= SPAM_CLUSTER {
        return NameVerdict::Suspicious {
            reason: format!("low-variance name (≈{similar} near-identical recent channels)"),
        };
    }

    NameVerdict::Ok
}

/// Lowercase, trim, collapse internal whitespace.
fn normalize(s: &str) -> String {
    s.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Heuristic random-string detector. Returns a reason when the name looks
/// machine-generated. Tuned to avoid flagging ordinary short words.
fn looks_random(norm: &str) -> Option<String> {
    let letters: Vec<char> = norm.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let digits = norm.chars().filter(|c| c.is_ascii_digit()).count();
    let total = norm.chars().filter(|c| !c.is_whitespace()).count();
    if total < 5 {
        return None; // too short to judge; short names are common and fine
    }

    let digit_ratio = digits as f64 / total as f64;
    if digit_ratio > 0.5 {
        return Some(format!("mostly digits ({}%)", (digit_ratio * 100.0) as u32));
    }

    if letters.len() >= 5 {
        let vowels = letters
            .iter()
            .filter(|c| matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y'))
            .count();
        let vowel_ratio = vowels as f64 / letters.len() as f64;
        if vowel_ratio < 0.15 {
            return Some("no vowels (likely random)".to_owned());
        }
        // Very low character variety relative to length (e.g. "aaaaaa").
        let distinct = {
            let mut v: Vec<char> = letters.clone();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        if letters.len() >= 6 && distinct <= 2 {
            return Some("very low character variety".to_owned());
        }
    }
    None
}

/// Normalized similarity in `0.0..=1.0` (1.0 = identical), from edit distance.
fn similarity(a: &str, b: &str) -> f64 {
    let max = a.chars().count().max(b.chars().count());
    if max == 0 {
        return 1.0;
    }
    let dist = levenshtein(a, b);
    1.0 - (dist as f64 / max as f64)
}

/// Classic Levenshtein edit distance (two-row DP).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}
