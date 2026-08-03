//! Authentication boundary (concept §13.1) — pluggable, so the deployment can
//! swap the dev stand-in for real Negotiate/Kerberos.
//!
//! Closes the I-001 shim: the acting principal is derived from the
//! **authenticated connection**, not trusted from the request body (a
//! body-supplied principal is spoofable and would corrupt the audit trail — a
//! primary deliverable). Handlers take the authenticated [`Caller`] and prefer
//! it over any body principal.
//!
//! Three implementations:
//! - [`DisabledAuth`] — no connection auth; handlers fall back to the body
//!   principal. The default, so existing dev/tests are unchanged.
//! - [`TrustedHeaderAuth`] — trusts an `X-Chapr-Principal` header set by the
//!   endpoint. A dev/loopback stand-in for SSO; NOT for untrusted networks.
//! - [`NegotiateAuth`] — placeholder for SSPI/SPNEGO Kerberos (concept §13.1),
//!   only exercisable against a real AD domain; wired so the boundary is ready.

use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::{request::Parts, HeaderMap, StatusCode};
use chapr_proto::Principal;

/// Resolves the authenticated principal for a request.
pub trait Authenticator: Send + Sync {
    /// `Ok(Some(p))` — authenticated as `p`; `Ok(None)` — this authenticator
    /// does not authenticate (fall back to the body principal); `Err` — auth
    /// was attempted and failed (→ 401).
    fn authenticate(&self, headers: &HeaderMap) -> Result<Option<Principal>, AuthError>;
}

/// Authentication failed — surfaced as HTTP 401.
#[derive(Debug)]
pub struct AuthError;

/// No connection authentication (default). Handlers use the body principal.
pub struct DisabledAuth;
impl Authenticator for DisabledAuth {
    fn authenticate(&self, _headers: &HeaderMap) -> Result<Option<Principal>, AuthError> {
        Ok(None)
    }
}

/// Dev/loopback: trust `X-Chapr-Principal` (set by the endpoint). Stand-in for
/// SSO on a trusted intranet; the real deployment uses [`NegotiateAuth`].
pub struct TrustedHeaderAuth;
impl Authenticator for TrustedHeaderAuth {
    fn authenticate(&self, headers: &HeaderMap) -> Result<Option<Principal>, AuthError> {
        match headers.get("x-chapr-principal").and_then(|v| v.to_str().ok()) {
            Some(p) if !p.is_empty() => Ok(Some(Principal::new_unchecked(p))),
            _ => Err(AuthError),
        }
    }
}

/// Placeholder for Negotiate/Kerberos (SSPI/SPNEGO, concept §13.1). Only
/// exercisable against a real AD domain. TODO(auth): perform the SPNEGO
/// handshake and extract the AD principal from the security context.
pub struct NegotiateAuth;
impl Authenticator for NegotiateAuth {
    fn authenticate(&self, _headers: &HeaderMap) -> Result<Option<Principal>, AuthError> {
        Err(AuthError) // not configured in this build
    }
}

/// Select an authenticator by name (env `CHAPR_COORD_AUTH`).
pub fn from_name(name: &str) -> std::sync::Arc<dyn Authenticator> {
    match name {
        "trusted-header" => std::sync::Arc::new(TrustedHeaderAuth),
        "negotiate" => std::sync::Arc::new(NegotiateAuth),
        _ => std::sync::Arc::new(DisabledAuth),
    }
}

/// The authenticated caller extracted from a request. `Some` when the
/// configured authenticator produced a principal; `None` under `DisabledAuth`
/// (the handler then falls back to the body principal).
pub struct Caller(pub Option<Principal>);

impl FromRequestParts<AppState> for Caller {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match state.auth.authenticate(&parts.headers) {
            Ok(principal) => Ok(Caller(principal)),
            Err(_) => Err(StatusCode::UNAUTHORIZED),
        }
    }
}
