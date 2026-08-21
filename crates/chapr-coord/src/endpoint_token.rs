// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The endpoint token — one shared secret per deployment, proving a caller on the
//! control channel is one of this deployment's endpoints.
//!
//! ## What this fixes, and what it does not
//!
//! `trusted-header` tests two things about `X-Chapr-Principal`: that it is valid
//! UTF-8, and that it is non-empty. So **anyone who can reach the port and type a
//! header is authenticated as anyone** — enough to fetch any snapshotted version of
//! any file coord knows about, to forge a row into the audit trail that is
//! documented as a primary deliverable, and to clear a journal entry that crash
//! recovery depends on.
//!
//! This closes that. It does **not** make the principal unforgeable: an endpoint
//! holding the secret can still name any user it likes. The two questions are
//! separate on purpose —
//!
//! - **the token** answers *is this one of our endpoints* (admission), and
//! - **the principal header** answers *acting as whom* (attribution).
//!
//! That is the honest shape of Chaperone's own framing: the audit trail proves a
//! user is responsible for their agents, not court-grade non-repudiation. Binding
//! identity to a verified subject is E-015's job — `negotiate` for a Kerberos
//! realm, an OIDC validator otherwise — and it stays pluggable because the
//! deployment posture varies (D-037).
//!
//! ## Why a shared secret rather than per-endpoint credentials
//!
//! Deliberately a **bridge, not an architecture** (D-037). Per-endpoint tokens
//! would need issuance, rotation, revocation and a registry — which means a table,
//! and `db.rs` carries no migration machinery at all. Adding one against a live
//! customer install, for a credential that E-015 is going to replace, is a bad
//! trade. One secret per deployment closes the hole reachable from the network
//! today and leaves nothing to unpick later.
//!
//! The consequence to be honest about: revoking one laptop means rotating for all
//! of them. At ~20 users that is a distribution job, not an engineering one.
//!
//! ## Where it lives
//!
//! Beside the admin token, in the coordinator's data directory, whose ACL the
//! installer already restricts to administrators plus the service account (D-029).
//! Same reasoning as [`crate::admin_token`]: the confidentiality is a protection
//! that already exists, so there is no key store, no hashing and nothing new to get
//! wrong. It reuses that module's file handling and its constant-time [`verify`]
//! rather than repeating either.

use std::io;
use std::path::{Path, PathBuf};

/// File name inside the coordinator's data directory.
const FILE_NAME: &str = "endpoint-token";

/// The token file's path within `dir`.
pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Read the token, creating one if the file does not exist yet.
///
/// Idempotent, for the same reason the admin token is: a restart must not
/// invalidate a secret that has already been distributed to twenty laptops.
pub fn load_or_create(dir: &Path) -> io::Result<String> {
    crate::admin_token::load_or_create_at(&path_in(dir))
}

/// Constant-time comparison. Shared with the admin token — see
/// [`crate::admin_token::verify`].
pub fn verify(stored: &str, presented: &str) -> bool {
    crate::admin_token::verify(stored, presented)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_created_once_and_then_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let first = load_or_create(tmp.path()).unwrap();
        let second = load_or_create(tmp.path()).unwrap();
        assert_eq!(first, second, "twenty laptops hold this; a restart must not change it");
        assert_eq!(first.len(), 64, "256 bits, hex");
    }

    /// The two credentials must be independent files with independent values.
    /// Sharing one would mean handing every laptop the break-glass credential that
    /// can rewrite coord's configuration and turn auth off.
    #[test]
    fn the_endpoint_token_is_not_the_admin_token() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = load_or_create(tmp.path()).unwrap();
        let admin = crate::admin_token::load_or_create(tmp.path()).unwrap();
        assert_ne!(endpoint, admin);
        assert_ne!(path_in(tmp.path()), crate::admin_token::path_in(tmp.path()));
        assert!(!verify(&endpoint, &admin), "one must not authenticate as the other");
    }

    #[test]
    fn an_empty_file_is_replaced_rather_than_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(path_in(tmp.path()), "  \n").unwrap();
        assert_eq!(load_or_create(tmp.path()).unwrap().len(), 64);
    }
}
