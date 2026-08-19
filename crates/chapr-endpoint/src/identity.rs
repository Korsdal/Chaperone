// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Ambient OS identity for the MVP control channel (E-023, decision D-024).
//!
//! The endpoint runs *as the logged-in user*, so their identity is already
//! established by the OS session — the same identity that grants them access to
//! the share. The MVP reuses it directly: no token, no prompt, no app
//! registration, no setup. coord runs in `trusted-header` mode and stamps this
//! principal into the audit trail.
//!
//! This is **accountability-grade, not evidence-grade** (concept §13.1 MVP note):
//! a tampered endpoint could assert a different principal. Accepted, documented
//! MVP limitation — Chaperone is a collaboration engine that *values*
//! traceability, not a security tool requiring non-repudiation. The enforced
//! modes (`negotiate` / `oidc`) are the hardening path via the same coord
//! `Authenticator` seam, selected by config with no change here (E-015).

use chapr_proto::Principal;

/// The acting principal for this endpoint session, derived from the OS logon.
/// `CHAPR_PRINCIPAL` overrides (useful for tests or unusual setups).
pub fn logged_in_principal() -> Principal {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    Principal::new_unchecked(principal_from_parts(
        get("CHAPR_PRINCIPAL"),
        get("USERDOMAIN"),
        get("USERNAME"),
        get("USER").or_else(|| get("LOGNAME")),
    ))
}

/// Pure derivation (testable without touching process env):
/// explicit override → Windows `DOMAIN\user` → Unix user → fallback.
pub(crate) fn principal_from_parts(
    override_principal: Option<String>,
    win_domain: Option<String>,
    win_user: Option<String>,
    unix_user: Option<String>,
) -> String {
    if let Some(p) = override_principal {
        return p;
    }
    match (win_domain, win_user, unix_user) {
        (Some(d), Some(u), _) => format!("{d}\\{u}"), // Windows: DOMAIN\user
        (None, Some(u), _) => u,                      // Windows without USERDOMAIN (rare)
        (_, None, Some(u)) => u,                      // Unix: bare username
        _ => "UNKNOWN\\user".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::principal_from_parts as p;

    #[test]
    fn override_wins() {
        assert_eq!(
            p(Some("CONTOSO\\override".into()), Some("D".into()), Some("u".into()), Some("x".into())),
            "CONTOSO\\override"
        );
    }

    #[test]
    fn windows_domain_and_user() {
        assert_eq!(p(None, Some("CONTOSO".into()), Some("jsmith".into()), None), "CONTOSO\\jsmith");
    }

    #[test]
    fn unix_user() {
        assert_eq!(p(None, None, None, Some("alice".into())), "alice");
    }

    #[test]
    fn fallback_when_nothing_known() {
        assert_eq!(p(None, None, None, None), "UNKNOWN\\user");
    }
}
