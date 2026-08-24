// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The version token — Chaperone's single change-detection primitive.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A content-derived version token: `BLAKE3(file_bytes)`, hex-encoded.
///
/// This one value does three jobs (invariant 2):
///
/// - **CAS change detector** — a write supplies the `base_version` it read; the
///   endpoint re-hashes under the lock and compares (concept §7 step 6).
/// - **History-store address** — the blob key *is* the token; "restore version
///   V" is a direct lookup (concept §12).
/// - **Audit chain link** — `from_version`/`to_version` on every write commit.
///
/// BLAKE3 is chosen for throughput (multi-GB/s over 200 MiB PDFs), **not** for
/// cryptographic strength. It is a change detector, not a commitment — SMB
/// offers no trustworthy cheap version token, so the file is hashed on read and
/// re-hashed under the lock on write (concept §7, cost accepted).
///
/// Stored and transmitted as the lowercase hex of the 32-byte digest (64 hex
/// chars). The wire form is a bare string (`#[serde(transparent)]`).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VersionToken(String);

impl VersionToken {
    /// Length of a BLAKE3 digest in bytes.
    pub const DIGEST_LEN: usize = 32;
    /// Length of the hex encoding of a BLAKE3 digest.
    pub const HEX_LEN: usize = Self::DIGEST_LEN * 2;

    /// Derive the version token from file bytes. This is the *only* correct way
    /// to mint a token from content — invariant 2 in one function.
    pub fn hash(bytes: &[u8]) -> Self {
        VersionToken(blake3::hash(bytes).to_hex().to_string())
    }

    /// Wrap an already-computed hex digest without re-hashing.
    ///
    /// Use this when reading a token back out of persistence or off the wire,
    /// where the value was produced by [`VersionToken::hash`] elsewhere. Returns
    /// `None` if `hex` is not exactly [`VersionToken::HEX_LEN`] lowercase hex
    /// characters, so a malformed token cannot masquerade as a real one.
    pub fn from_hex(hex: impl Into<String>) -> Option<Self> {
        let hex = hex.into();
        let valid = hex.len() == Self::HEX_LEN
            && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        valid.then_some(VersionToken(hex))
    }

    /// The token as its hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VersionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// Deliberately not `Debug`-derived to a bare string: show that this is a token,
// but keep the full hex so logs are greppable against the blob store.
impl fmt::Debug for VersionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VersionToken({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_and_hex() {
        let a = VersionToken::hash(b"the quick brown fox");
        let b = VersionToken::hash(b"the quick brown fox");
        assert_eq!(a, b);
        assert_eq!(a.as_str().len(), VersionToken::HEX_LEN);
        assert!(a.as_str().bytes().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn distinct_content_distinct_token() {
        assert_ne!(VersionToken::hash(b"a"), VersionToken::hash(b"b"));
    }

    #[test]
    fn from_hex_validates_length_and_charset() {
        let good = VersionToken::hash(b"x").as_str().to_owned();
        assert!(VersionToken::from_hex(good).is_some());
        assert!(VersionToken::from_hex("deadbeef").is_none()); // too short
        assert!(VersionToken::from_hex("g".repeat(64)).is_none()); // non-hex
        assert!(VersionToken::from_hex("A".repeat(64)).is_none()); // uppercase rejected
    }

    #[test]
    fn serde_round_trip_is_a_bare_string() {
        let v = VersionToken::hash(b"payload");
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.starts_with('"') && json.ends_with('"'));
        let back: VersionToken = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }
}
