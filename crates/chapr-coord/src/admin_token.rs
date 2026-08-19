// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The admin token — the credential that gates the administrative surface.
//!
//! ## Why a token and not a role
//!
//! D-029 said admin should be a **role** resolved by the pluggable
//! [`crate::auth::Authenticator`]. That holds where the auth mode actually
//! authenticates. It does not hold under `trusted-header`, which accepts any
//! non-empty `X-Chapr-Principal` — a role built on top of that authorizes
//! nothing, it attributes. And the administrative surface can now *write coord's
//! configuration*, which means it can turn auth off, so it needs a credential
//! that is real rather than asserted.
//!
//! So: the token is the enforced layer now, and `admin_principals` is the role
//! that takes over when a mode that authenticates is configured (E-015). Two
//! layers, not two alternatives.
//!
//! ## Why a file in the data directory is enough
//!
//! The installer already restricts that directory to administrators plus the
//! service account, with `Users` denied (D-029). **The token's confidentiality is
//! therefore a protection that already exists** — no key store, no hashing, no
//! password policy, nothing new to get wrong. Whoever can read the file is
//! already someone who could read the database and the blob store beside it.
//!
//! ## Permanent, but rotatable
//!
//! The token is never disabled (jok's call): it is the break-glass credential, and
//! it keeps working regardless of what the auth mode is doing, which is what makes
//! an auth cutover safe to attempt. That leaves one thing to answer — a former
//! administrator may have kept a copy — and the answer is [`rotate`] rather than
//! removal.

use std::io;
use std::path::{Path, PathBuf};

/// File name inside the coordinator's data directory.
const FILE_NAME: &str = "admin-token";

/// The token file's path within `dir`.
pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Read the token, creating one if the file does not exist yet.
///
/// Idempotent: a coordinator that restarts keeps the token an administrator has
/// already been given. Only [`rotate`] replaces it.
pub fn load_or_create(dir: &Path) -> io::Result<String> {
    let path = path_in(dir);
    match std::fs::read_to_string(&path) {
        Ok(existing) => {
            let trimmed = existing.trim().to_string();
            if trimmed.is_empty() {
                // An empty file is a half-written one; replace it rather than
                // start up with a credential nobody can present.
                write_new(&path)
            } else {
                Ok(trimmed)
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)?;
            write_new(&path)
        }
        Err(e) => Err(e),
    }
}

/// Replace the token with a fresh one and return it.
pub fn rotate(dir: &Path) -> io::Result<String> {
    std::fs::create_dir_all(dir)?;
    write_new(&path_in(dir))
}

fn write_new(path: &Path) -> io::Result<String> {
    let token = generate();
    std::fs::write(path, &token)?;
    // Best-effort on Unix: the directory ACL is the real control (see the module
    // docs), but a 0600 file costs nothing and narrows the window if the directory
    // was never hardened.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

/// 256 bits of randomness, hex.
///
/// Two v4 UUIDs rather than a new dependency: `uuid`'s v4 draws from the OS
/// randomness source, and the hyphens are dropped so the result is one opaque
/// string an administrator can copy without wondering which part matters.
fn generate() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Compare a presented token against the stored one.
///
/// Length-independent and without an early exit, so the comparison does not leak
/// how much of a guess was right. The window is small — this is a local service on
/// a trusted network — but a constant-time compare costs three lines.
pub fn verify(stored: &str, presented: &str) -> bool {
    let (a, b) = (stored.as_bytes(), presented.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0 && !stored.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_created_once_and_then_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let first = load_or_create(tmp.path()).unwrap();
        let second = load_or_create(tmp.path()).unwrap();
        assert_eq!(first, second, "a restart must not invalidate the token");
        assert_eq!(first.len(), 64, "256 bits, hex");
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn the_directory_is_created_if_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("does").join("not").join("exist");
        let token = load_or_create(&nested).unwrap();
        assert!(path_in(&nested).exists());
        assert!(!token.is_empty());
    }

    #[test]
    fn an_empty_file_is_replaced_rather_than_trusted() {
        // A half-written file would otherwise leave coord up with a credential
        // nobody can present, and the admin surface unreachable.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(path_in(tmp.path()), "   \n").unwrap();
        let token = load_or_create(tmp.path()).unwrap();
        assert_eq!(token.len(), 64);
    }

    #[test]
    fn rotate_replaces_the_token() {
        let tmp = tempfile::tempdir().unwrap();
        let old = load_or_create(tmp.path()).unwrap();
        let new = rotate(tmp.path()).unwrap();
        assert_ne!(old, new);
        assert_eq!(
            load_or_create(tmp.path()).unwrap(),
            new,
            "the rotated token must be the one that persists"
        );
        assert!(!verify(&new, &old), "the old token must stop working");
    }

    #[test]
    fn two_tokens_are_never_the_same() {
        let a = generate();
        let b = generate();
        assert_ne!(a, b);
    }

    #[test]
    fn verify_accepts_only_the_exact_token() {
        let t = generate();
        assert!(verify(&t, &t));
        assert!(!verify(&t, &t[..63]), "a prefix must not pass");
        assert!(!verify(&t, &format!("{t}x")), "a longer string must not pass");
        assert!(!verify(&t, ""), "an empty presentation must not pass");
        // And an empty *stored* token must never authorise anything, or a failed
        // write would leave the admin surface open to a blank credential.
        assert!(!verify("", ""));
    }
}
