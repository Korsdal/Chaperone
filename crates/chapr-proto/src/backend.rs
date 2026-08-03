//! Backend identity carried on the control channel (§14).
//!
//! Coord *announces* which kind of fileserver backend owns a resource; the
//! endpoint maps that to an in-binary adapter (roadmap). This is **advisory
//! policy**, not a correctness authority — the endpoint selects and verifies its
//! backend locally (invariant 3), and coord never does backend I/O (invariant
//! 6). Vendor-agnostic on coord's side: it just stores and echoes the kind.
//!
//! Keyed by canonical path today; the descriptor itself carries no path, so it
//! generalizes unchanged to an opaque resource locator (invariant 5) when a
//! non-path backend lands.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Which kind of fileserver backend owns a resource. `Smb` and `Posix` exist
/// today; `S3`/`Azure`/`Graph` are reserved for the roadmap. Adding a variant is
/// wire-compatible: an unknown kind is only ever produced by a newer coord for a
/// newer endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    #[default]
    Smb,
    /// A POSIX filesystem (local/NFS): advisory `flock`, case-sensitive `/`-paths
    /// (E-019). Capability profile differs from SMB (no mandatory lock).
    Posix,
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            BackendKind::Smb => "smb",
            BackendKind::Posix => "posix",
        };
        f.write_str(s)
    }
}

impl FromStr for BackendKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "smb" => Ok(BackendKind::Smb),
            "posix" => Ok(BackendKind::Posix),
            other => Err(format!("unknown backend kind {other:?}; expected: smb, posix")),
        }
    }
}

/// The descriptor coord hands down on [`crate::ResolveResponse::backend`]. A
/// struct (not a bare enum) so it can grow backend-scoped policy fields later
/// without another wire change. Minimal today: just the kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendDescriptor {
    pub kind: BackendKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_serialises_snake_case() {
        assert_eq!(serde_json::to_string(&BackendKind::Smb).unwrap(), "\"smb\"");
        assert_eq!(
            serde_json::from_str::<BackendKind>("\"smb\"").unwrap(),
            BackendKind::Smb
        );
        assert_eq!(BackendKind::Smb.to_string(), "smb");
        assert_eq!("SMB".parse::<BackendKind>().unwrap(), BackendKind::Smb);
        assert!("s3".parse::<BackendKind>().is_err());
    }

    #[test]
    fn posix_kind_round_trips() {
        assert_eq!(serde_json::to_string(&BackendKind::Posix).unwrap(), "\"posix\"");
        assert_eq!(
            serde_json::from_str::<BackendKind>("\"posix\"").unwrap(),
            BackendKind::Posix
        );
        assert_eq!(BackendKind::Posix.to_string(), "posix");
        assert_eq!("POSIX".parse::<BackendKind>().unwrap(), BackendKind::Posix);
    }

    #[test]
    fn descriptor_round_trips() {
        let d = BackendDescriptor {
            kind: BackendKind::Smb,
        };
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<BackendDescriptor>(&json).unwrap(), d);
    }
}
