//! Per-backend path grammar (E-019, decision D-E).
//!
//! Invariant 5 keys all coordination state by canonical path, but the *shape* of
//! that path is backend-specific: SMB is case-insensitive with `\`-separators and
//! a `\\server\share` UNC prefix; POSIX is case-sensitive with `/`-separators and
//! a single leading-`/` root. The grammar also owns the derived-name conventions
//! the §7 write core needs — the Office/"human wins" lock sibling, the conflict
//! sidecar, and the restore copy.
//!
//! Everything here is pure string logic with no OS dependency, so it compiles and
//! unit-tests on any platform (the cross-platform-buildable choice, D-C).

use chapr_proto::{BackendKind, CanonicalPath, Principal};
use chrono::Utc;
use unicode_normalization::UnicodeNormalization;

/// The path grammar for one backend kind. `normalize` is the §5.1
/// canonicalisation body (backend-specific); the derived-name builders are
/// provided in terms of [`Self::sep`] so only genuinely-divergent behaviour is
/// overridden per impl.
pub trait PathGrammar: Send + Sync {
    /// The path separator (`\` for SMB, `/` for POSIX).
    fn sep(&self) -> char;

    /// Canonicalise a raw path (§5.1), or return the failure reason. Called by
    /// [`crate::canon::canonicalize`], which adds the empty-input guard and wraps
    /// the result in a [`CanonicalPath`].
    fn normalize(&self, raw: &str) -> Result<String, String>;

    /// The "humans always win" lock sibling to pre-flight before a write (SMB's
    /// Office `~$F`). `None` for backends with no such convention (POSIX), where
    /// the exclusive lock is merely advisory (accepted trade-off, D-F/D-019).
    fn human_lock_path(&self, _path: &CanonicalPath) -> Option<CanonicalPath> {
        None
    }

    /// A uniquely-named conflict sidecar `F.conflict-{user}-{ts}.ext` (concept
    /// §11), preserving the original extension. Provided in terms of [`Self::sep`].
    fn sidecar_path(&self, path: &CanonicalPath, principal: &Principal) -> CanonicalPath {
        let user: String = principal
            .as_str()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let ts = Utc::now().format("%Y%m%d%H%M%S");
        CanonicalPath::new_unchecked(insert_tag(
            path.as_str(),
            &format!("conflict-{user}-{ts}"),
            self.sep(),
        ))
    }

    /// A `F.restored-{ts}.ext` sibling for a restore copy (concept §6.5),
    /// preserving the original extension.
    fn restored_path(&self, path: &CanonicalPath) -> CanonicalPath {
        let ts = Utc::now().format("%Y%m%d%H%M%S");
        CanonicalPath::new_unchecked(insert_tag(
            path.as_str(),
            &format!("restored-{ts}"),
            self.sep(),
        ))
    }

    /// Join a directory and an entry name with this grammar's separator (used by
    /// `chapr.list`).
    fn join(&self, dir: &str, name: &str) -> String {
        format!("{}{}{}", dir.trim_end_matches(self.sep()), self.sep(), name)
    }
}

/// SMB/Windows grammar: case-insensitive, `\`-separators, `\\`-UNC prefix.
pub struct WinGrammar;

impl PathGrammar for WinGrammar {
    fn sep(&self) -> char {
        '\\'
    }

    fn normalize(&self, raw: &str) -> Result<String, String> {
        // 1. unify separators, 2. NFC, 3. casefold (case-insensitive).
        let unified: String = raw.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
        let folded: String = unified.nfc().collect::<String>().to_lowercase();
        // 4. collapse repeated separators, preserving a leading UNC `\\`.
        let is_unc = folded.starts_with("\\\\");
        let body = if is_unc { &folded[2..] } else { folded.as_str() };
        let collapsed = collapse(body, '\\');
        // 5. strip trailing separators.
        let trimmed = collapsed.trim_end_matches('\\');
        if trimmed.is_empty() {
            return Err("path has no body after normalisation".into());
        }
        Ok(if is_unc {
            format!("\\\\{trimmed}")
        } else {
            trimmed.to_string()
        })
    }

    fn human_lock_path(&self, path: &CanonicalPath) -> Option<CanonicalPath> {
        let s = path.as_str();
        Some(match s.rfind('\\') {
            Some(i) => CanonicalPath::new_unchecked(format!("{}~${}", &s[..=i], &s[i + 1..])),
            None => CanonicalPath::new_unchecked(format!("~${s}")),
        })
    }
}

/// POSIX grammar: case-**sensitive**, `/`-separators, single leading-`/` root.
/// Backslash is a valid filename character on POSIX, so it is **not** translated.
pub struct PosixGrammar;

impl PathGrammar for PosixGrammar {
    fn sep(&self) -> char {
        '/'
    }

    fn normalize(&self, raw: &str) -> Result<String, String> {
        // NFC only — no case-folding (POSIX is case-sensitive), no `\`→`/`.
        let nfc: String = raw.nfc().collect();
        let is_abs = nfc.starts_with('/');
        let body = if is_abs { nfc.trim_start_matches('/') } else { nfc.as_str() };
        let collapsed = collapse(body, '/');
        let trimmed = collapsed.trim_end_matches('/');
        if trimmed.is_empty() {
            // A bare "/" (root) is valid; anything else empty is not.
            return if is_abs {
                Ok("/".to_string())
            } else {
                Err("path has no body after normalisation".into())
            };
        }
        Ok(if is_abs {
            format!("/{trimmed}")
        } else {
            trimmed.to_string()
        })
    }
    // human_lock_path defaults to None: POSIX has no universal Office-lock
    // convention, so the write pre-flight is skipped and the lock is advisory.
}

/// The grammar for a backend kind. Returns a `'static` reference (the grammars
/// are zero-sized and const-promoted), so callers thread it without allocation.
pub fn grammar_for(kind: BackendKind) -> &'static dyn PathGrammar {
    match kind {
        BackendKind::Smb => &WinGrammar,
        BackendKind::Posix => &PosixGrammar,
    }
}

/// Collapse runs of `sep` into a single separator.
fn collapse(body: &str, sep: char) -> String {
    let mut out = String::with_capacity(body.len());
    let mut prev_sep = false;
    for c in body.chars() {
        if c == sep {
            if !prev_sep {
                out.push(sep);
            }
            prev_sep = true;
        } else {
            out.push(c);
            prev_sep = false;
        }
    }
    out
}

/// Insert `tag` before the final extension of `path` (after the last separator),
/// or append it if there is no extension. Shared by sidecar/restored naming.
fn insert_tag(path: &str, tag: &str, sep: char) -> String {
    let dir_end = path.rfind(sep).map(|i| i + 1).unwrap_or(0);
    match path[dir_end..].rfind('.') {
        Some(rel_dot) => {
            let dot = dir_end + rel_dot;
            format!("{}.{tag}{}", &path[..dot], &path[dot..])
        }
        None => format!("{path}.{tag}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_office_lock_is_the_tilde_dollar_sibling() {
        let p = CanonicalPath::new_unchecked("\\\\srv\\share\\dir\\report.xlsx");
        assert_eq!(
            WinGrammar.human_lock_path(&p).unwrap().as_str(),
            "\\\\srv\\share\\dir\\~$report.xlsx"
        );
    }

    #[test]
    fn posix_has_no_human_lock() {
        let p = CanonicalPath::new_unchecked("/mnt/share/report.xlsx");
        assert!(PosixGrammar.human_lock_path(&p).is_none());
    }

    #[test]
    fn win_sidecar_preserves_extension_and_sanitises_user() {
        let p = CanonicalPath::new_unchecked("\\\\srv\\share\\q3.xlsx");
        let s = WinGrammar.sidecar_path(&p, &Principal::new_unchecked("CONTOSO\\jsmith"));
        let s = s.as_str();
        assert!(s.starts_with("\\\\srv\\share\\q3.conflict-CONTOSO_jsmith-"));
        assert!(s.ends_with(".xlsx"));
        assert!(!s.contains("CONTOSO\\jsmith"));
    }

    #[test]
    fn posix_sidecar_uses_forward_slash_grammar() {
        let p = CanonicalPath::new_unchecked("/mnt/share/q3.xlsx");
        let s = PosixGrammar.sidecar_path(&p, &Principal::new_unchecked("jsmith"));
        let s = s.as_str();
        assert!(s.starts_with("/mnt/share/q3.conflict-jsmith-"));
        assert!(s.ends_with(".xlsx"));
    }

    #[test]
    fn win_restored_preserves_extension() {
        let p = CanonicalPath::new_unchecked("\\\\srv\\share\\q3.xlsx");
        let r = WinGrammar.restored_path(&p);
        assert!(r.as_str().starts_with("\\\\srv\\share\\q3.restored-"));
        assert!(r.as_str().ends_with(".xlsx"));
    }

    #[test]
    fn win_normalize_casefolds_and_preserves_unc() {
        assert_eq!(
            WinGrammar.normalize("//SRV/Share//Dir/A.MD/").unwrap(),
            "\\\\srv\\share\\dir\\a.md"
        );
    }

    #[test]
    fn posix_normalize_is_case_sensitive_and_collapses() {
        assert_eq!(
            PosixGrammar.normalize("/mnt//share/Dir/A.md/").unwrap(),
            "/mnt/share/Dir/A.md"
        );
        // Case is preserved (unlike SMB) → different casings are different paths.
        assert_ne!(
            PosixGrammar.normalize("/mnt/Report.md").unwrap(),
            PosixGrammar.normalize("/mnt/report.md").unwrap()
        );
    }

    #[test]
    fn posix_does_not_translate_backslash() {
        // Backslash is a valid POSIX filename char — must survive normalisation.
        assert_eq!(
            PosixGrammar.normalize("/mnt/od\\d.md").unwrap(),
            "/mnt/od\\d.md"
        );
    }

    #[test]
    fn grammar_for_maps_kinds() {
        assert_eq!(grammar_for(BackendKind::Smb).sep(), '\\');
        assert_eq!(grammar_for(BackendKind::Posix).sep(), '/');
    }

    #[test]
    fn join_uses_the_grammar_separator() {
        assert_eq!(WinGrammar.join("\\\\srv\\share", "a.md"), "\\\\srv\\share\\a.md");
        assert_eq!(PosixGrammar.join("/mnt/share", "a.md"), "/mnt/share/a.md");
    }
}
