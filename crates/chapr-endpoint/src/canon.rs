//! Path canonicalisation (concept §5.1) — the construction site for
//! [`CanonicalPath`].
//!
//! Invariant 5: all coordination state is keyed by canonical path, so two users
//! naming the same file differently **must** land on the same key. This is the
//! one place allowed to mint a [`CanonicalPath`] from raw input — everywhere
//! else the value is already canonical and trusted.
//!
//! The backend-specific normalisation body lives in the [`PathGrammar`] (E-019):
//! SMB casefolds and `\`-joins with a `\\`-UNC prefix; POSIX is case-sensitive
//! and `/`-joins. This function is the thin, grammar-agnostic wrapper — the
//! empty-input guard plus the [`CanonicalPath`] mint.
//!
//! ## What is still deferred (needs Win32)
//!
//! - **DFS resolution** to the underlying server/share.
//! - **Drive-letter → UNC** mapping (needs the live per-session mount table).
//!
//! So input is expected already in the backend's native form (UNC for SMB, an
//! absolute POSIX path for POSIX). Full Unicode casefold is likewise still
//! approximated by lowercase in [`crate::pathgrammar::WinGrammar`].

use crate::pathgrammar::PathGrammar;
use chapr_proto::{CanonicalPath, ChaprError};

/// Canonicalise a raw path into a [`CanonicalPath`] using `grammar` for the
/// backend-specific rules. Returns [`ChaprError::InvalidPath`] if the input is
/// empty or normalises to nothing.
pub fn canonicalize(raw: &str, grammar: &dyn PathGrammar) -> Result<CanonicalPath, ChaprError> {
    let invalid = |reason: String| ChaprError::InvalidPath {
        raw: raw.to_string(),
        reason,
    };

    if raw.trim().is_empty() {
        return Err(invalid("empty path".into()));
    }

    let out = grammar.normalize(raw).map_err(invalid)?;
    if out.is_empty() {
        return Err(invalid("path has no body after normalisation".into()));
    }
    Ok(CanonicalPath::new_unchecked(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathgrammar::WinGrammar;

    fn c(s: &str) -> String {
        canonicalize(s, &WinGrammar).unwrap().into_inner()
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
        assert!(matches!(canonicalize("", &WinGrammar), Err(ChaprError::InvalidPath { .. })));
        assert!(matches!(canonicalize("   ", &WinGrammar), Err(ChaprError::InvalidPath { .. })));
    }

    #[test]
    fn canonicalisation_is_idempotent() {
        let once = c("//SRV/Share//dir/a.md/");
        let twice = canonicalize(&once, &WinGrammar).unwrap().into_inner();
        assert_eq!(once, twice);
    }
}
