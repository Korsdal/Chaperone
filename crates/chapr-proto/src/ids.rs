//! Typed identifier newtypes.
//!
//! Every coordination record is keyed by one of these. They are thin wrappers
//! around `String`, but distinct *types* — the compiler will not let a
//! [`LeaseId`] be passed where a [`ConflictId`] is expected, which is exactly
//! the class of mix-up that is otherwise invisible until it corrupts the wrong
//! row. All are `#[serde(transparent)]`, so on the wire each is a bare string.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Generate a transparent `String` newtype with the common accessors.
macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wrap a raw string. Named `_unchecked` because this type performs
            /// no validation of its own — see the type's docs for what the
            /// caller is responsible for guaranteeing.
            pub fn new_unchecked(s: impl Into<String>) -> Self {
                $name(s.into())
            }

            /// Borrow the underlying string.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume into the owned string.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
    };
}

string_newtype! {
    /// The canonical, server-relative path that keys **all** coordination state
    /// (invariant 5). Two users naming the same file differently must resolve to
    /// the same `CanonicalPath`, or contention control silently fails for the
    /// exact case it exists to handle.
    ///
    /// Canonicalisation (concept §5.1) is: DFS-resolved to the underlying
    /// server/share, Unicode NFC-normalised, casefolded (SMB is
    /// case-insensitive), drive-letter mappings stripped to UNC form, trailing
    /// separators removed, separators normalised.
    ///
    /// That procedure needs the platform (DFS resolution in particular) and so
    /// lives in `chapr-endpoint`, **not** here. `new_unchecked` therefore means
    /// exactly that: this crate trusts the caller ran the canonicaliser. Never
    /// build a `CanonicalPath` from a raw user- or model-supplied string on the
    /// coord side.
    CanonicalPath
}

string_newtype! {
    /// An Active Directory principal (e.g. `CONTOSO\\jsmith`). Every lease,
    /// write, restore, and history entry is stamped with one — "your session,
    /// your responsibility" (concept §13.1). The audit trail keyed by this is a
    /// primary deliverable, not a byproduct.
    Principal
}

string_newtype! {
    /// Opaque identifier for a granted lease (or lease *set* — one id covers an
    /// all-or-none acquisition over several paths; concept §9).
    LeaseId
}

string_newtype! {
    /// Opaque identifier for a registered conflict sidecar (concept §11).
    ConflictId
}

string_newtype! {
    /// Opaque identifier for a single audit event (concept §5.2 `AuditEvent`).
    EventId
}

string_newtype! {
    /// The agent session an operation belongs to. Load-bearing for the
    /// read-before-write invariant: coord rejects any `base_version` it has not
    /// recorded *this session* as having read (concept §6.2), so the session
    /// identity is part of the correctness story, not just telemetry.
    SessionId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_ids_are_distinct_types() {
        // This is really a compile-time assertion; the runtime check is trivial.
        let lease = LeaseId::new_unchecked("L-1");
        let conflict = ConflictId::new_unchecked("C-1");
        assert_ne!(lease.as_str(), conflict.as_str());
    }

    #[test]
    fn serde_round_trip_is_a_bare_string() {
        let p = Principal::new_unchecked("CONTOSO\\jsmith");
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"CONTOSO\\\\jsmith\"");
        let back: Principal = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
