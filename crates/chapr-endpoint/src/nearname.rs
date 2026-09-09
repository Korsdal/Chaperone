// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Is this new directory name almost the name of one that already exists?
//!
//! ## Why a guard at all
//!
//! `chapr_mkdir` exists because having to create output directories by hand is
//! friction, and friction pushes work outside Chaperone. But careless directory
//! creation is how a share ends up holding `Reports`, `reports` and `Repotrs`
//! side by side — and **invariant 5 keys all coordination by canonical path**, so
//! those are three distinct, permanent keys that will never reconcile. Leases,
//! version history and conflicts all fork along them.
//!
//! ## Deterministic, because the alternative is not checkable
//!
//! The obvious design is to ask the model to look for similar names and involve
//! the human if it finds any. That is non-deterministic: the same call can pass
//! today and fail tomorrow, and nothing can be tested. This module answers the
//! question with arithmetic instead — normalise, then measure edit distance — so
//! the outcome is a property of the inputs alone. The model's job is reduced to
//! reporting a refusal, which it cannot get subtly wrong.
//!
//! ## What it actually catches
//!
//! Case is **not** the interesting axis. The SMB grammar already casefolds
//! (invariant 5, [`crate::pathgrammar`]) and NTFS will not hold `Reports` and
//! `reports` at once, so that pair cannot arise on the backend this ships
//! against. What does arise:
//!
//! - **transpositions and typos** — `Repotrs` for `Reports`, one operation away;
//! - **spacing and separators** — `Q1 Reports`, `Q1-Reports`, `Q1_Reports`;
//! - **near-identical plurals** — `Report` beside `Reports`.
//!
//! So the comparison strips separators and punctuation, casefolds, and then
//! allows a small edit distance. A short name gets a tighter budget: at distance
//! 2, three-letter names would collide with half the alphabet.

/// Similarity threshold, by length of the normalised candidate.
///
/// Distance 1 for short names, 2 from eight characters up. Fixed rather than
/// proportional because a proportional budget grows without bound on long names,
/// and two directory names differing by four edits are different names.
fn budget(len: usize) -> usize {
    match len {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

/// Casefold and drop the characters people vary without meaning to.
///
/// Separators, punctuation and whitespace all go, so `Q1 Reports`, `q1-reports`
/// and `Q1_Reports` normalise to one string. Digits and letters are kept.
fn normalise(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Damerau-Levenshtein distance (optimal string alignment), capped by `max`.
///
/// Transposition matters here more than it usually does: `Repotrs` is one
/// transposition from `Reports`, and plain Levenshtein scores it 2 — the same as
/// two unrelated substitutions. The cap lets the walk stop early on names that
/// are obviously unrelated.
fn distance_within(a: &str, b: &str, max: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    // Three full-length rows, rotated. `prev2` starts full-length rather than
    // empty even though row 1 never reads it: the rotation below moves it into
    // `cur`, so an empty one becomes an empty `cur` on the second row and every
    // index panics. Cheaper to allocate than to reason about.
    let mut prev2: Vec<usize> = vec![0; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                cur[j] = cur[j].min(prev2[j - 2] + 1);
            }
        }
        // Rotate: the row just computed becomes `prev`, the old `prev` becomes
        // `prev2` (which the transposition term reads), and the retired `prev2`
        // buffer is reused as `cur`.
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    let d = prev[b.len()];
    (d <= max).then_some(d)
}

/// Do two names differ **only in their digits**?
///
/// `2026` and `2027` are one edit apart and both deliberate; so are `Q1` and
/// `Q2`, `v1` and `v2`, `Reports2025` and `Reports2026`. Numbering is how people
/// legitimately make sibling directories, and a guard that refuses the second
/// year of a deployment would be turned off within a week — which is worse than
/// no guard, because the typo protection goes with it.
///
/// So: strip the digits from both. If what remains is identical and the digits
/// are not, the difference is deliberate numbering rather than a slip. A typo in
/// the letters (`Repotrs`) still fails this test and is still caught.
fn differs_only_in_digits(a: &str, b: &str) -> bool {
    let letters = |s: &str| s.chars().filter(|c| !c.is_numeric()).collect::<String>();
    a != b && letters(a) == letters(b)
}

/// Which of `existing` are close enough to `candidate` to be worth refusing over.
///
/// Returns the matches in the order they were given, so a refusal lists them the
/// way the directory listing did. An **exact** normalised match is always
/// reported, whatever the length budget — `Reports` against an existing
/// `reports` is the strongest case there is, even though both normalise to a
/// distance of zero on a short name whose budget is zero.
pub fn similar_names<'a>(
    candidate: &str,
    existing: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let target = normalise(candidate);
    if target.is_empty() {
        return Vec::new();
    }
    let max = budget(target.len());
    existing
        .into_iter()
        .filter(|name| {
            let other = normalise(name);
            if other.is_empty() {
                return false;
            }
            if other == target {
                return true;
            }
            if differs_only_in_digits(&target, &other) {
                return false;
            }
            distance_within(&target, &other, max).is_some()
        })
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transposition_is_caught() {
        assert_eq!(similar_names("Repotrs", ["Reports"]), vec!["Reports"]);
    }

    #[test]
    fn case_and_separators_are_not_differences() {
        for existing in ["reports", "REPORTS", "Q1 Reports", "q1-reports"] {
            let candidate = if existing.contains('1') {
                "Q1_Reports"
            } else {
                "Reports"
            };
            assert_eq!(
                similar_names(candidate, [existing]),
                vec![existing.to_string()],
                "{candidate} vs {existing}"
            );
        }
    }

    #[test]
    fn a_singular_beside_a_plural_is_caught() {
        assert_eq!(similar_names("Report", ["Reports"]), vec!["Reports"]);
    }

    /// The guard has to stay out of the way of ordinary work, or it gets turned
    /// off. Genuinely different names must pass.
    #[test]
    fn unrelated_names_pass() {
        let existing = ["Reports", "Tenders", "Archive", "2026"];
        for candidate in ["Proposals", "Invoices", "Drafts", "2027"] {
            assert!(
                similar_names(candidate, existing).is_empty(),
                "{candidate} was wrongly flagged"
            );
        }
    }

    /// A tight budget on short names, or three-letter directories collide with
    /// everything: `abc` vs `xyz` is distance 3, but `abc` vs `abd` is 1.
    #[test]
    fn short_names_get_a_tighter_budget() {
        assert!(
            similar_names("abc", ["abd"]).is_empty(),
            "3 chars: no budget"
        );
        assert_eq!(
            similar_names("draft", ["drafts"]),
            vec!["drafts"],
            "5 chars: 1"
        );
        // 9 characters, budget 2: two deletions away and still caught.
        assert_eq!(
            similar_names("proposals", ["propsal"]),
            vec!["propsal"],
            "9 chars: 2"
        );
        // Three edits is a different name, whatever the length.
        assert!(
            similar_names("quarterlies", ["quarterly"]).is_empty(),
            "distance 3 is not a near-duplicate"
        );
    }

    /// The false-positive family that would have got this guard switched off:
    /// numbered siblings. `2026` and `2027` are one edit apart and both
    /// deliberate.
    #[test]
    fn numbered_siblings_are_deliberate_not_typos() {
        assert!(similar_names("2027", ["2026"]).is_empty());
        assert!(similar_names("Q2", ["Q1"]).is_empty());
        assert!(similar_names("Reports2026", ["Reports2025"]).is_empty());
        assert!(similar_names("Reports2026", ["Reports"]).is_empty());
        // But a letter typo is still a typo, digits present or not.
        assert_eq!(
            similar_names("Repotrs2026", ["Reports2026"]),
            vec!["Reports2026"]
        );
    }

    #[test]
    fn an_exact_normalised_match_is_always_reported() {
        // Budget for a 3-char name is 0, but this is not a near miss — it is the
        // same name with different punctuation.
        assert_eq!(similar_names("a-b-c", ["abc"]), vec!["abc"]);
    }

    #[test]
    fn every_match_is_listed_in_listing_order() {
        let existing = ["Reports", "Unrelated", "reports "];
        assert_eq!(
            similar_names("Report", existing),
            vec!["Reports".to_string(), "reports ".to_string()]
        );
    }

    #[test]
    fn a_name_with_no_alphanumerics_flags_nothing() {
        assert!(similar_names("___", ["Reports"]).is_empty());
    }
}
