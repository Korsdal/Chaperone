// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Path canonicalisation (concept §5.1) — the construction site for
//! [`CanonicalPath`].
//!
//! Invariant 5: all coordination state is keyed by canonical path, so two users
//! naming the same file differently **must** land on the same key. This is the
//! one place allowed to mint a [`CanonicalPath`] from raw input — everywhere
//! else the value is already canonical and trusted.
//!
//! Three stages, in order:
//!
//! 1. **Root resolution** ([`PathGrammar::to_rooted`], E-022) — the impure step.
//!    A mapped drive letter becomes the UNC name of the share it points at, so
//!    `Z:\` on one laptop and `P:\` on another key the same file identically.
//!    Needs the live OS mount table, which is why it is separate from stage 2.
//! 2. **Normalisation** ([`PathGrammar::normalize`]) — pure string work: NFC,
//!    casefold, separator unification, separator collapse.
//! 3. **Confinement** (E-025, closes I-010) — the canonical path must sit inside
//!    a configured coordinated root, if any are configured.
//!
//! ## What is still deferred
//!
//! - **DFS resolution** to the underlying server/share. Not needed for the first
//!   deployment (the customer confirmed no DFS namespace and no DFS-R), so it is
//!   no longer bundled with the drive-letter work it was originally filed with.
//! - `.` and `..` are still not resolved (I-009). Full Unicode casefold is still
//!   approximated by lowercase in [`crate::pathgrammar::WinGrammar`].

use crate::mount::{self, MountTable};
use crate::pathgrammar::PathGrammar;
use chapr_proto::{CanonicalPath, ChaprError};
use std::sync::OnceLock;

/// The coordinated roots, fixed once at start-up.
///
/// A process-global rather than a parameter on [`canonicalize`], deliberately.
/// The check exists so a path *cannot* reach the filesystem without passing it,
/// and there are a dozen canonicalisation sites across `ops`/`read`/`write`;
/// threading a value through all of them makes the guard something a future call
/// site can forget, which is precisely the property a confinement check must not
/// have. The value is immutable after start-up, so the global carries no
/// scheduling risk — and the interesting logic lives in the pure
/// [`is_within_roots`], which is what the tests exercise.
static ROOTS: OnceLock<Vec<CanonicalPath>> = OnceLock::new();

/// Fix the coordinated roots for this process. Called once from `main`.
///
/// Returns `Err` if they were already set — a second call means two places
/// disagree about the deployment's scope, and silently keeping the first would
/// hide that.
pub fn set_coordinated_roots(roots: Vec<CanonicalPath>) -> Result<(), Vec<CanonicalPath>> {
    ROOTS.set(roots)
}

/// The configured roots, or an empty slice when confinement is off.
pub fn coordinated_roots() -> &'static [CanonicalPath] {
    ROOTS.get().map(|v| v.as_slice()).unwrap_or(&[])
}

/// Is `path` inside one of `roots`?
///
/// **An empty `roots` means yes**: confinement is opt-in, so an endpoint with no
/// configured root behaves exactly as it did before E-025. That is a deliberate
/// compatibility choice, and `main` warns at start-up when it applies.
///
/// The boundary check is a separator, not a bare prefix: `\\srv\share2` must not
/// count as inside `\\srv\share`. Both sides are already canonical here, so this
/// is a plain comparison — no case or Unicode work left to do.
pub fn is_within_roots(path: &str, roots: &[CanonicalPath], sep: char) -> bool {
    if roots.is_empty() {
        return true;
    }
    roots.iter().any(|root| {
        let root = root.as_str();
        path == root
            || (path.len() > root.len()
                && path.starts_with(root)
                && path[root.len()..].starts_with(sep))
    })
}

/// Canonicalise a raw path into a [`CanonicalPath`] using `grammar` for the
/// backend-specific rules, this process's real mount table, and the configured
/// coordinated roots.
pub fn canonicalize(raw: &str, grammar: &dyn PathGrammar) -> Result<CanonicalPath, ChaprError> {
    canonicalize_in(raw, grammar, mount::default_mounts(), coordinated_roots())
}

/// [`canonicalize`] with the mount table and roots supplied explicitly.
///
/// Used by tests — the real mount table needs a live Windows session with actual
/// drive mappings — and by `main`, which must canonicalise the roots themselves
/// before any root exists to check them against.
pub fn canonicalize_in(
    raw: &str,
    grammar: &dyn PathGrammar,
    mounts: &dyn MountTable,
    roots: &[CanonicalPath],
) -> Result<CanonicalPath, ChaprError> {
    let invalid = |reason: String| ChaprError::InvalidPath {
        raw: raw.to_string(),
        reason,
    };

    if raw.trim().is_empty() {
        return Err(invalid("empty path".into()));
    }

    let rooted = grammar.to_rooted(raw, mounts).map_err(invalid)?;
    let out = grammar.normalize(&rooted).map_err(invalid)?;
    if out.is_empty() {
        return Err(invalid("path has no body after normalisation".into()));
    }

    if !is_within_roots(&out, roots, grammar.sep()) {
        return Err(invalid(format!(
            "resolves to {out:?}, which is outside this endpoint's coordinated \
             root(s): {}",
            roots
                .iter()
                .map(|r| r.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    Ok(CanonicalPath::new_unchecked(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mount::{MountTable, NoMountTable};
    use crate::pathgrammar::{PosixGrammar, WinGrammar};

    /// A table with one drive mapping.
    struct FakeMounts {
        drive: &'static str,
        share: &'static str,
    }

    impl MountTable for FakeMounts {
        fn universal_name(&self, local_path: &str) -> Result<Option<String>, String> {
            match local_path.strip_prefix(self.drive) {
                Some(rest) => Ok(Some(format!("{}{}", self.share, rest))),
                None => Ok(None),
            }
        }
    }

    struct BrokenMounts;

    impl MountTable for BrokenMounts {
        fn universal_name(&self, _local_path: &str) -> Result<Option<String>, String> {
            Err("the network provider is unavailable".into())
        }
    }

    fn c(s: &str) -> String {
        canonicalize_in(s, &WinGrammar, &NoMountTable, &[])
            .unwrap()
            .into_inner()
    }

    #[test]
    fn forward_and_back_slashes_agree() {
        assert_eq!(c("//srv/share/dir/a.md"), c("\\\\srv\\share\\dir\\a.md"));
    }

    #[test]
    fn trailing_separators_are_stripped() {
        assert_eq!(c("\\\\srv\\share\\dir\\"), "\\\\srv\\share\\dir");
        assert_eq!(c("\\\\srv\\share\\dir\\\\"), "\\\\srv\\share\\dir");
    }

    #[test]
    fn repeated_separators_collapse_but_unc_prefix_survives() {
        assert_eq!(c("\\\\srv\\\\share\\a.md"), "\\\\srv\\share\\a.md");
        assert!(c("\\\\srv\\share\\a.md").starts_with("\\\\"));
    }

    #[test]
    fn casefold_makes_casings_equal() {
        assert_eq!(c("\\\\SRV\\Share\\Report.MD"), c("\\\\srv\\share\\report.md"));
    }

    #[test]
    fn nfc_makes_composed_and_decomposed_equal() {
        let composed = "\\\\srv\\share\\caf\u{e9}.md";
        let decomposed = "\\\\srv\\share\\caf\u{65}\u{301}.md";
        assert_eq!(c(composed), c(decomposed));
    }

    #[test]
    fn empty_or_blank_is_invalid() {
        for bad in ["", "   "] {
            assert!(matches!(
                canonicalize_in(bad, &WinGrammar, &NoMountTable, &[]),
                Err(ChaprError::InvalidPath { .. })
            ));
        }
    }

    #[test]
    fn canonicalisation_is_idempotent() {
        let once = c("//SRV/Share//dir/a.md/");
        let twice = canonicalize_in(&once, &WinGrammar, &NoMountTable, &[])
            .unwrap()
            .into_inner();
        assert_eq!(once, twice);
    }

    // ---- E-022: drive letter → UNC ---------------------------------------

    #[test]
    fn two_laptops_with_different_drive_letters_key_the_same_file() {
        // The defect this closes. Both laptops have the same share mapped under a
        // different letter; before resolution they produced `z:\…` and `p:\…` —
        // two lease keys, and a torn-file marker written under one invisible
        // under the other.
        let laptop_a = FakeMounts { drive: "Z:", share: "\\\\FILESRV\\AICollab" };
        let laptop_b = FakeMounts { drive: "P:", share: "\\\\FILESRV\\AICollab" };

        let from_a = canonicalize_in("Z:\\Tenders\\bid.docx", &WinGrammar, &laptop_a, &[]).unwrap();
        let from_b = canonicalize_in("P:\\Tenders\\bid.docx", &WinGrammar, &laptop_b, &[]).unwrap();
        assert_eq!(from_a, from_b);
        assert_eq!(from_a.as_str(), "\\\\filesrv\\aicollab\\tenders\\bid.docx");
    }

    #[test]
    fn a_mapped_drive_agrees_with_the_unc_spelling_of_the_same_file() {
        // The other half of invariant 5: one user typing the UNC path and another
        // using the mapped drive must also agree.
        let mounts = FakeMounts { drive: "Z:", share: "\\\\FILESRV\\AICollab" };
        let via_drive = canonicalize_in("Z:\\Tenders\\bid.docx", &WinGrammar, &mounts, &[]).unwrap();
        let via_unc = canonicalize_in(
            "\\\\FILESRV\\AICollab\\Tenders\\bid.docx",
            &WinGrammar,
            &mounts,
            &[],
        )
        .unwrap();
        assert_eq!(via_drive, via_unc);
    }

    #[test]
    fn a_genuinely_local_path_is_left_alone() {
        // Not an error: a local path cannot be shared between laptops, so it has
        // no aliasing risk, and the live-smoke harness runs on local temp trees.
        let out = canonicalize_in("C:\\temp\\scratch.md", &WinGrammar, &NoMountTable, &[]).unwrap();
        assert_eq!(out.as_str(), "c:\\temp\\scratch.md");
    }

    #[test]
    fn a_relative_path_is_refused_loudly() {
        // The "reject when we cannot resolve" half of D-030: a relative path means
        // something different per working directory, so it can never be keyed.
        for bad in ["bid.docx", "Tenders\\bid.docx", "Z:bid.docx"] {
            let err = canonicalize_in(bad, &WinGrammar, &NoMountTable, &[]).unwrap_err();
            match err {
                ChaprError::InvalidPath { reason, .. } => {
                    assert!(
                        reason.contains("not an absolute path"),
                        "{bad} gave the wrong reason: {reason}"
                    )
                }
                other => panic!("{bad} gave the wrong error: {other:?}"),
            }
        }
    }

    #[test]
    fn an_unresolvable_mapping_fails_rather_than_falling_back() {
        // Falling back to the drive-letter spelling here is what would silently
        // reintroduce the aliasing, so a lookup failure must surface.
        let err = canonicalize_in("Z:\\Tenders\\bid.docx", &WinGrammar, &BrokenMounts, &[])
            .unwrap_err();
        match err {
            ChaprError::InvalidPath { reason, .. } => {
                assert!(reason.contains("cannot resolve"), "wrong reason: {reason}")
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn posix_has_nothing_to_resolve() {
        let out = canonicalize_in("/srv/share/a.md", &PosixGrammar, &NoMountTable, &[]).unwrap();
        assert_eq!(out.as_str(), "/srv/share/a.md");
    }

    // ---- E-025: coordinated root confinement (I-010) ---------------------

    #[test]
    fn no_configured_root_means_no_confinement() {
        assert!(is_within_roots("\\\\any\\where\\at\\all", &[], '\\'));
    }

    #[test]
    fn confinement_accepts_the_root_and_its_children() {
        let roots = vec![CanonicalPath::new_unchecked("\\\\filesrv\\aicollab")];
        assert!(is_within_roots("\\\\filesrv\\aicollab", &roots, '\\'));
        assert!(is_within_roots("\\\\filesrv\\aicollab\\tenders\\a.md", &roots, '\\'));
    }

    #[test]
    fn confinement_is_not_a_bare_prefix_match() {
        // `aicollab2` starts with `aicollab` but is a different share, and a
        // prefix-only check would have let it through.
        let roots = vec![CanonicalPath::new_unchecked("\\\\filesrv\\aicollab")];
        assert!(!is_within_roots("\\\\filesrv\\aicollab2\\secret.md", &roots, '\\'));
        assert!(!is_within_roots("\\\\otherhost\\c$\\windows", &roots, '\\'));
    }

    #[test]
    fn confinement_accepts_any_of_several_roots() {
        let roots = vec![
            CanonicalPath::new_unchecked("\\\\filesrv\\tenders"),
            CanonicalPath::new_unchecked("\\\\filesrv\\proposals"),
        ];
        assert!(is_within_roots("\\\\filesrv\\proposals\\q3.docx", &roots, '\\'));
        assert!(!is_within_roots("\\\\filesrv\\payroll\\salaries.xlsx", &roots, '\\'));
    }

    #[test]
    fn canonicalize_refuses_a_path_outside_the_root() {
        // I-010 end to end: a UNC path to an arbitrary host used to be accepted,
        // bounded only by the user's own ACLs.
        let roots = vec![CanonicalPath::new_unchecked("\\\\filesrv\\aicollab")];
        let err = canonicalize_in(
            "\\\\otherhost\\finance\\salaries.xlsx",
            &WinGrammar,
            &NoMountTable,
            &roots,
        )
        .unwrap_err();
        match err {
            ChaprError::InvalidPath { reason, .. } => {
                assert!(reason.contains("outside"), "wrong reason: {reason}")
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn a_mapped_drive_is_confined_by_the_share_it_resolves_to() {
        // The two features have to compose: confinement is checked *after*
        // resolution, so a drive mapped outside the root is caught, and a drive
        // mapped inside it is allowed even though the raw path resembles neither.
        let roots = vec![CanonicalPath::new_unchecked("\\\\filesrv\\aicollab")];
        let inside = FakeMounts { drive: "Z:", share: "\\\\FILESRV\\AICollab" };
        let outside = FakeMounts { drive: "Z:", share: "\\\\FILESRV\\Payroll" };

        assert!(canonicalize_in("Z:\\a.md", &WinGrammar, &inside, &roots).is_ok());
        assert!(canonicalize_in("Z:\\a.md", &WinGrammar, &outside, &roots).is_err());
    }
}
