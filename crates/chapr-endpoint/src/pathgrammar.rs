// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

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

use crate::mount::MountTable;
use chapr_proto::{BackendKind, CanonicalPath, Principal};
use chrono::Utc;
use unicode_normalization::UnicodeNormalization;

/// `X:\…` — a drive letter with a rooted remainder. `X:foo` (drive-relative) and
/// `foo\bar` are not absolute: neither names one file independently of who is
/// asking, which is the property invariant 5 needs.
fn is_absolute_local(p: &str) -> bool {
    let mut it = p.chars();
    matches!(
        (it.next(), it.next(), it.next()),
        (Some(c), Some(':'), Some('\\')) if c.is_ascii_alphabetic()
    )
}

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

    /// Bring `raw` into this backend's own rooted form *before* normalisation,
    /// consulting the live mount table where the backend has one (E-022).
    ///
    /// Split out from [`Self::normalize`] because it is the one part of
    /// canonicalisation that cannot be pure: resolving a mapped drive needs the
    /// OS to answer what the letter currently points at. Keeping it separate
    /// leaves `normalize` string-only and unit-testable on any platform.
    ///
    /// The default is identity — a backend whose paths have exactly one spelling
    /// (POSIX) has nothing to resolve.
    fn to_rooted(
        &self,
        raw: &str,
        _mounts: &dyn crate::mount::MountTable,
    ) -> Result<String, String> {
        Ok(raw.to_string())
    }

    /// Every "humans always win" lock sibling to pre-flight before a write (SMB's
    /// Office owner files). Empty for backends with no such convention (POSIX),
    /// where the exclusive lock is merely advisory (accepted trade-off,
    /// D-F/D-019).
    ///
    /// A list, not one name (refining D-F), because Office has **two** owner-file
    /// conventions and only one of them was implemented. Excel and PowerPoint
    /// prepend `~$` to the whole filename; **Word replaces the first two
    /// characters** — `Master_ISO.docx` → `~$ster_ISO.docx`. Checking only the
    /// prepended form meant the pre-flight never fired for any `.docx`, so
    /// "humans always win" silently did not hold for Word on write, delete, move
    /// or restore. Rather than branch on extension — which drifts, and guesses
    /// which app has the file open — every backend returns all the shapes its
    /// convention can produce and the caller refuses if any of them exists.
    fn human_lock_paths(&self, _path: &CanonicalPath) -> Vec<CanonicalPath> {
        Vec::new()
    }

    /// A uniquely-named conflict sidecar `F.conflict-{user}-{ts}-{id}.ext`
    /// (concept §11), preserving the original extension. Provided in terms of
    /// [`Self::sep`].
    ///
    /// The `{id}` is load-bearing, not decoration. The timestamp is
    /// second-granular, and two CAS losses by the same principal inside one
    /// second computed the *same* name — `create_new` then refused and
    /// `write_cas_core` returned `AlreadyExists` with the losing bytes written
    /// nowhere. That is reachable in practice: a live 4-session race produced two
    /// conflicts in the same second unprompted.
    fn sidecar_path(&self, path: &CanonicalPath, principal: &Principal) -> CanonicalPath {
        let user: String = principal
            .as_str()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let ts = Utc::now().format("%Y%m%d%H%M%S");
        let id = short_id();
        self.derived(insert_tag(
            path.as_str(),
            &format!("conflict-{user}-{ts}-{id}"),
            self.sep(),
        ))
    }

    /// A `F.restored-{ts}-{id}.ext` sibling for a restore copy (concept §6.5),
    /// preserving the original extension. Carries the same uniqueness suffix as
    /// the sidecar, for the same reason.
    fn restored_path(&self, path: &CanonicalPath) -> CanonicalPath {
        let ts = Utc::now().format("%Y%m%d%H%M%S");
        let id = short_id();
        self.derived(insert_tag(
            path.as_str(),
            &format!("restored-{ts}-{id}"),
            self.sep(),
        ))
    }

    /// Mint a derived path as a genuinely canonical one.
    ///
    /// Derived names are built from an already-canonical path plus ASCII, but
    /// they used to bypass [`Self::normalize`] entirely — so under `WinGrammar`
    /// (which casefolds) a sidecar carrying `CONTOSO_jsmith` was a
    /// `CanonicalPath` that violated invariant 5, and then keyed coord state via
    /// `register_conflict`. Route it through the grammar so that cannot happen.
    fn derived(&self, raw: String) -> CanonicalPath {
        match self.normalize(&raw) {
            Ok(n) => CanonicalPath::new_unchecked(n),
            // Unreachable for a derived name, and a failure here must not lose
            // the caller's bytes — the sidecar path is where they get parked.
            Err(_) => CanonicalPath::new_unchecked(raw),
        }
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

    fn to_rooted(&self, raw: &str, mounts: &dyn MountTable) -> Result<String, String> {
        let unified: String = raw.chars().map(|c| if c == '/' { '\\' } else { c }).collect();

        // Already UNC — nothing to resolve, and this is the overwhelmingly common
        // case: `chapr.list` re-canonicalises every entry it returns, and those are
        // joined onto an already-canonical parent. Short-circuiting here keeps the
        // syscall to at most one per user-supplied path.
        if unified.starts_with("\\\\") {
            return Ok(unified);
        }

        // A relative or drive-relative path cannot be keyed at all: `bid.docx`
        // means something different per working directory, so two agents naming
        // one file would key it differently — invariant 5 broken with no way to
        // notice. Refuse instead of guessing.
        if !is_absolute_local(&unified) {
            return Err(format!(
                "{raw:?} is not an absolute path — give a UNC path (\\\\server\\share\\…) \
                 or a full path on a mapped drive (Z:\\…)"
            ));
        }

        match mounts.universal_name(&unified) {
            // A mapped network drive: key it by the share so every laptop agrees
            // regardless of which letter it happens to have mapped (E-022, D-030).
            Ok(Some(unc)) => Ok(unc),
            // Genuinely local, left as it is. A local path cannot be shared
            // between laptops, so it carries no aliasing risk, and the dev and
            // live-smoke harnesses work on local temp trees. Confining a
            // deployment to the share is E-025's configured root, not this.
            Ok(None) => Ok(unified),
            Err(e) => Err(format!(
                "cannot resolve {raw:?} to a share: {e} — coordination state is keyed by \
                 the share path, so an unresolvable drive mapping is not safe to use"
            )),
        }
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

    fn human_lock_paths(&self, path: &CanonicalPath) -> Vec<CanonicalPath> {
        let s = path.as_str();
        let (dir, name) = match s.rfind('\\') {
            Some(i) => (&s[..=i], &s[i + 1..]),
            None => ("", s),
        };

        // Excel / PowerPoint: `~$` + the whole filename.
        let mut out = vec![CanonicalPath::new_unchecked(format!("{dir}~${name}"))];

        // Word: `~$` + the filename minus its first two characters. Counted in
        // `char`s, not bytes — these are user-named business documents and
        // slicing mid-codepoint would panic on the first accented filename.
        let tail: String = name.chars().skip(2).collect();
        if !tail.is_empty() {
            out.push(CanonicalPath::new_unchecked(format!("{dir}~${tail}")));
        }
        out
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

/// A short random suffix for derived names. 8 hex chars of a v4 uuid — enough to
/// make a same-second collision negligible without making the filename unreadable.
fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
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

    fn locks(p: &str) -> Vec<String> {
        WinGrammar
            .human_lock_paths(&CanonicalPath::new_unchecked(p))
            .iter()
            .map(|c| c.as_str().to_string())
            .collect()
    }

    /// Excel and PowerPoint prepend `~$` to the whole filename.
    #[test]
    fn win_office_lock_covers_the_prepended_excel_form() {
        assert!(locks("\\\\srv\\share\\dir\\report.xlsx")
            .contains(&"\\\\srv\\share\\dir\\~$report.xlsx".to_string()));
    }

    /// Word replaces the **first two characters** instead, so checking only the
    /// prepended form meant the pre-flight never fired for any `.docx` and
    /// "humans always win" silently did not hold for Word documents.
    #[test]
    fn win_office_lock_covers_the_word_minus_two_form() {
        assert!(locks("\\\\srv\\share\\dir\\Quarterly.docx")
            .contains(&"\\\\srv\\share\\dir\\~$arterly.docx".to_string()));
    }

    /// Business filenames carry accents and the slice is by `char`, not byte —
    /// a byte slice would panic mid-codepoint on exactly these names.
    #[test]
    fn win_office_lock_slices_by_char_not_byte() {
        let got = locks("\\\\srv\\share\\Årsrapport.docx");
        assert!(got.contains(&"\\\\srv\\share\\~$srapport.docx".to_string()), "{got:?}");
    }

    /// Nothing to drop two characters from — emit only the prepended form rather
    /// than a bare `~$`, which would match an unrelated file.
    #[test]
    fn win_office_lock_skips_the_word_form_for_very_short_names() {
        assert_eq!(locks("\\\\srv\\share\\ab"), vec!["\\\\srv\\share\\~$ab".to_string()]);
    }

    #[test]
    fn posix_has_no_human_lock() {
        let p = CanonicalPath::new_unchecked("/mnt/share/report.xlsx");
        assert!(PosixGrammar.human_lock_paths(&p).is_empty());
    }

    #[test]
    fn win_sidecar_preserves_extension_and_sanitises_user() {
        let p = CanonicalPath::new_unchecked("\\\\srv\\share\\q3.xlsx");
        let s = WinGrammar.sidecar_path(&p, &Principal::new_unchecked("CONTOSO\\jsmith"));
        let s = s.as_str();
        // Casefolded, because a sidecar IS a CanonicalPath under this grammar and
        // it keys coord state — the old behaviour preserved `CONTOSO_jsmith` and
        // so minted a path that violated invariant 5.
        assert!(s.starts_with("\\\\srv\\share\\q3.conflict-contoso_jsmith-"));
        assert!(s.ends_with(".xlsx"));
        assert!(!s.contains("CONTOSO\\jsmith"));
        assert_eq!(s, WinGrammar.normalize(s).unwrap(), "sidecar must be canonical");
    }

    #[test]
    fn sidecars_in_the_same_second_do_not_collide() {
        // The whole point of the uniqueness suffix: same path, same principal,
        // same wall-clock second used to produce the same filename, and the
        // second CAS loser's bytes were then written nowhere.
        let p = CanonicalPath::new_unchecked("\\\\srv\\share\\q3.xlsx");
        let who = Principal::new_unchecked("CONTOSO\\jsmith");
        let a = WinGrammar.sidecar_path(&p, &who);
        let b = WinGrammar.sidecar_path(&p, &who);
        assert_ne!(a.as_str(), b.as_str());
        assert!(a.as_str().ends_with(".xlsx") && b.as_str().ends_with(".xlsx"));
    }

    #[test]
    fn restored_copies_in_the_same_second_do_not_collide() {
        let p = CanonicalPath::new_unchecked("/mnt/share/q3.xlsx");
        let a = PosixGrammar.restored_path(&p);
        let b = PosixGrammar.restored_path(&p);
        assert_ne!(a.as_str(), b.as_str());
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
