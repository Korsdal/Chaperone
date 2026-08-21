// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Authentication boundary (concept §13.1) — pluggable, so the deployment can
//! swap the dev stand-in for real Negotiate/Kerberos.
//!
//! Closes the I-001 shim: the acting principal is derived from the
//! **authenticated connection**, not trusted from the request body (a
//! body-supplied principal is spoofable and would corrupt the audit trail — a
//! primary deliverable). Handlers take the authenticated [`Caller`] and prefer
//! it over any body principal.
//!
//! Four implementations:
//! - [`DisabledAuth`] — no connection auth; handlers fall back to the body
//!   principal. The default, so existing dev/tests are unchanged.
//! - [`TrustedHeaderAuth`] — trusts an `X-Chapr-Principal` header set by the
//!   endpoint. A dev/loopback stand-in for SSO; NOT for untrusted networks,
//!   because it accepts *any* non-empty header from *anyone* who can reach the
//!   port.
//! - [`SharedSecretAuth`] — the first mode that actually refuses a stranger: one
//!   shared secret per deployment, plus the principal header. A bridge until
//!   E-015, deliberately not an architecture (D-037, [`crate::endpoint_token`]).
//! - [`NegotiateAuth`] — placeholder for SSPI/SPNEGO Kerberos (concept §13.1),
//!   only exercisable against a real AD domain; wired so the boundary is ready.
//!
//! ## Two extractors, two questions
//!
//! [`Caller`] asks *who is this* and yields a principal. [`Authenticated`] asks
//! only *may this request be here*, for the routes that carry no principal —
//! including two that mutate. Both run the configured authenticator, so both
//! start refusing the moment the mode is one that refuses.

use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::{request::Parts, HeaderMap, StatusCode};
use chapr_proto::Principal;

/// What an authenticator resolved, and which mode resolved it.
///
/// The mode name is first-class rather than a side channel because the auth
/// cutover depends on it: an administrator moving from `trusted-header` to
/// `negotiate` needs to see the real endpoints arriving on the new mode before
/// retiring the old one, and that is only observable if each request reports which
/// mode let it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// `Some(p)` — authenticated as `p`. `None` — this mode does not
    /// authenticate, so the handler falls back to the body principal.
    pub principal: Option<Principal>,
    /// The configured mode that produced this: `disabled`, `trusted-header`, …
    pub mode: &'static str,
}

/// Resolves the authenticated principal for a request.
pub trait Authenticator: Send + Sync {
    /// The configured name of this authenticator.
    fn mode(&self) -> &'static str;

    /// `Ok` — resolved (see [`Outcome`]); `Err` — auth was attempted and failed
    /// (→ 401, or the fallback in a layered arrangement).
    fn authenticate(&self, headers: &HeaderMap) -> Result<Outcome, AuthError>;
}

/// Authentication failed — surfaced as HTTP 401.
#[derive(Debug)]
pub struct AuthError;

/// No connection authentication (default). Handlers use the body principal.
pub struct DisabledAuth;
impl Authenticator for DisabledAuth {
    fn mode(&self) -> &'static str {
        "disabled"
    }
    fn authenticate(&self, _headers: &HeaderMap) -> Result<Outcome, AuthError> {
        Ok(Outcome {
            principal: None,
            mode: "disabled",
        })
    }
}

/// Dev/loopback: trust `X-Chapr-Principal` (set by the endpoint). Stand-in for
/// SSO on a trusted intranet; the real deployment uses [`NegotiateAuth`].
pub struct TrustedHeaderAuth;
impl Authenticator for TrustedHeaderAuth {
    fn mode(&self) -> &'static str {
        "trusted-header"
    }
    fn authenticate(&self, headers: &HeaderMap) -> Result<Outcome, AuthError> {
        match headers.get("x-chapr-principal").and_then(|v| v.to_str().ok()) {
            Some(p) if !p.is_empty() => Ok(Outcome {
                principal: Some(Principal::new_unchecked(p)),
                mode: "trusted-header",
            }),
            _ => Err(AuthError),
        }
    }
}

/// Enforced admission: the caller must present the deployment's shared secret
/// **and** name a principal.
///
/// The two halves answer different questions and neither substitutes for the
/// other — see [`crate::endpoint_token`]. The token proves the caller is one of
/// this deployment's endpoints; the header says which user it is acting for, and
/// remains asserted rather than proven until E-015 lands a mode that binds
/// identity to a verified subject.
///
/// Ordering is deliberate: the token is checked first, so an unauthenticated
/// caller learns nothing about whether a principal header would have been
/// accepted.
pub struct SharedSecretAuth {
    pub expected: String,
}

impl Authenticator for SharedSecretAuth {
    fn mode(&self) -> &'static str {
        "shared-secret"
    }
    fn authenticate(&self, headers: &HeaderMap) -> Result<Outcome, AuthError> {
        let presented = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("")
            .trim();
        if !crate::endpoint_token::verify(&self.expected, presented) {
            return Err(AuthError);
        }
        match headers.get("x-chapr-principal").and_then(|v| v.to_str().ok()) {
            Some(p) if !p.is_empty() => Ok(Outcome {
                principal: Some(Principal::new_unchecked(p)),
                mode: "shared-secret",
            }),
            _ => Err(AuthError),
        }
    }
}

/// Placeholder for Negotiate/Kerberos (SSPI/SPNEGO, concept §13.1). Only
/// exercisable against a real AD domain. TODO(auth): perform the SPNEGO
/// handshake and extract the AD principal from the security context.
pub struct NegotiateAuth;
impl Authenticator for NegotiateAuth {
    fn mode(&self) -> &'static str {
        "negotiate"
    }
    fn authenticate(&self, _headers: &HeaderMap) -> Result<Outcome, AuthError> {
        Err(AuthError) // not configured in this build
    }
}

/// A primary authenticator with a fallback, for changing auth modes safely.
///
/// Primary rejects → try the fallback. The reported mode says which one let the
/// request in, which is the whole point: an administrator watches the new mode
/// take over and retires the old one when nothing uses it.
///
/// `disabled` as the primary is refused by [`crate::config::Config::validate`] —
/// it never rejects, so the fallback would be unreachable and the arrangement
/// would look configured while doing nothing.
pub struct LayeredAuth {
    pub primary: std::sync::Arc<dyn Authenticator>,
    pub fallback: std::sync::Arc<dyn Authenticator>,
}

impl Authenticator for LayeredAuth {
    fn mode(&self) -> &'static str {
        self.primary.mode()
    }
    fn authenticate(&self, headers: &HeaderMap) -> Result<Outcome, AuthError> {
        match self.primary.authenticate(headers) {
            Ok(outcome) => Ok(outcome),
            Err(AuthError) => self.fallback.authenticate(headers),
        }
    }
}

/// Select an authenticator by name (env `CHAPR_COORD_AUTH`).
///
/// `secret` is the deployment's endpoint token, needed only by `shared-secret`.
/// When that mode is asked for and no secret could be established, this returns
/// [`NegotiateAuth`] — an authenticator that refuses everything. **Failing closed
/// is the whole point:** a coordinator told to enforce, that cannot, must not
/// silently fall back to accepting any header. `main` refuses to start in that
/// state; this is the belt to that braces, for the settings-reload path.
pub fn from_name(name: &str, secret: Option<&str>) -> std::sync::Arc<dyn Authenticator> {
    match name {
        "trusted-header" => std::sync::Arc::new(TrustedHeaderAuth),
        "negotiate" => std::sync::Arc::new(NegotiateAuth),
        "shared-secret" => match secret {
            Some(s) if !s.is_empty() => {
                std::sync::Arc::new(SharedSecretAuth { expected: s.to_string() })
            }
            _ => {
                tracing::error!(
                    "auth mode shared-secret was selected but no endpoint token could be \
                     established; refusing every request rather than accepting any header"
                );
                std::sync::Arc::new(NegotiateAuth)
            }
        },
        _ => std::sync::Arc::new(DisabledAuth),
    }
}

/// Build the authenticator a config asks for, layering a fallback if one is set.
pub fn from_config(
    cfg: &crate::config::Config,
    secret: Option<&str>,
) -> std::sync::Arc<dyn Authenticator> {
    match &cfg.auth_fallback {
        Some(fallback) => std::sync::Arc::new(LayeredAuth {
            primary: from_name(&cfg.auth, secret),
            fallback: from_name(fallback, secret),
        }),
        None => from_name(&cfg.auth, secret),
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
        match state.authenticator().authenticate(&parts.headers) {
            Ok(outcome) => {
                // Recorded per request so the cutover panel can show the new mode
                // taking over. Cheap: two atomics and a ring slot.
                state.auth_usage.record_success(outcome.mode);
                Ok(Caller(outcome.principal))
            }
            Err(_) => {
                // Counted too, and load-bearing: a client that cannot authenticate
                // under a new mode does not show up as fallback use, it shows up
                // here. Without this number, "no fallback use" is ambiguous between
                // "the cutover worked" and "everyone is failing".
                state.auth_usage.record_failure();
                Err(StatusCode::UNAUTHORIZED)
            }
        }
    }
}

/// Proof that the caller holds the coordinator's admin token.
///
/// Admission without attribution: run the configured authenticator and discard
/// the principal.
///
/// [`Caller`] already authenticates — that is why the twelve routes carrying it
/// gain real enforcement the moment the mode actually authenticates. The problem
/// was the eleven that carried **no extractor at all**, so nothing ran. Some of
/// those have no principal to attribute (`POST /resolve`, `POST /conflicts/query`)
/// and two of them *mutate*: `POST /journal/clear` destroys crash-recovery state,
/// `PUT /blobs` writes into the content store.
///
/// Bolting `Caller` onto those would work by side effect and read as if a
/// principal mattered. This says what it means. `/healthz` and `GET /admin` stay
/// open deliberately — the first is monitoring, the second is the page where the
/// admin token is typed.
pub struct Authenticated;

impl FromRequestParts<AppState> for Authenticated {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match state.authenticator().authenticate(&parts.headers) {
            Ok(outcome) => {
                state.auth_usage.record_success(outcome.mode);
                Ok(Authenticated)
            }
            Err(_) => {
                state.auth_usage.record_failure();
                Err(StatusCode::UNAUTHORIZED)
            }
        }
    }
}

/// A separate extractor from [`Caller`] because it answers a different question.
/// `Caller` asks *who is this* — attribution, and under `trusted-header` it is
/// asserted rather than proven. `AdminAuth` asks *may this request change the
/// coordinator*, which needs something real, because the administrative surface
/// can rewrite the config and therefore turn auth off.
///
/// Presented as `Authorization: Bearer <token>`.
pub struct AdminAuth;

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // No token on the server means the administrative surface is not open —
        // fail closed. This happens when the data directory could not be written,
        // and serving admin data to anyone in that state would be the wrong
        // direction entirely.
        let Some(expected) = state.admin_token() else {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "no admin token on this coordinator; check the data directory is writable",
            ));
        };
        let presented = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if crate::admin_token::verify(&expected, presented.trim()) {
            Ok(AdminAuth)
        } else {
            Err((
                StatusCode::UNAUTHORIZED,
                "admin token missing or wrong; it is in the coordinator's data directory",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(principal: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(p) = principal {
            h.insert("x-chapr-principal", p.parse().unwrap());
        }
        h
    }

    #[test]
    fn each_mode_reports_its_own_name() {
        let out = DisabledAuth.authenticate(&headers(None)).unwrap();
        assert_eq!(out.mode, "disabled");
        assert_eq!(out.principal, None, "disabled defers to the body principal");

        let out = TrustedHeaderAuth
            .authenticate(&headers(Some("CONTOSO\\a")))
            .unwrap();
        assert_eq!(out.mode, "trusted-header");
        assert_eq!(out.principal.unwrap().as_str(), "CONTOSO\\a");
    }

    #[test]
    fn layered_auth_reports_which_mode_admitted_the_request() {
        // The whole cutover story rests on this being observable. `negotiate`
        // rejects everything in this build, so it stands in for a primary that is
        // not working yet.
        let layered = LayeredAuth {
            primary: std::sync::Arc::new(NegotiateAuth),
            fallback: std::sync::Arc::new(TrustedHeaderAuth),
        };
        let out = layered
            .authenticate(&headers(Some("CONTOSO\\a")))
            .expect("the fallback should admit this");
        assert_eq!(
            out.mode, "trusted-header",
            "the reported mode must be the one that actually admitted it, not the primary"
        );
        // Primary is still what `mode()` names — that is the configured intent.
        assert_eq!(layered.mode(), "negotiate");
    }

    #[test]
    fn layered_auth_prefers_the_primary() {
        let layered = LayeredAuth {
            primary: std::sync::Arc::new(TrustedHeaderAuth),
            fallback: std::sync::Arc::new(DisabledAuth),
        };
        let out = layered.authenticate(&headers(Some("CONTOSO\\a"))).unwrap();
        assert_eq!(out.mode, "trusted-header");
    }

    #[test]
    fn layered_auth_fails_when_neither_admits() {
        let layered = LayeredAuth {
            primary: std::sync::Arc::new(NegotiateAuth),
            fallback: std::sync::Arc::new(TrustedHeaderAuth),
        };
        // No principal header, so the fallback rejects too.
        assert!(layered.authenticate(&headers(None)).is_err());
    }

    #[test]
    fn from_config_layers_only_when_a_fallback_is_set() {
        let mut cfg = crate::config::Config {
            auth: "trusted-header".into(),
            ..Default::default()
        };
        assert_eq!(from_config(&cfg, None).mode(), "trusted-header");

        cfg.auth = "negotiate".into();
        cfg.auth_fallback = Some("trusted-header".into());
        let layered = from_config(&cfg, None);
        assert_eq!(layered.mode(), "negotiate");
        assert_eq!(
            layered
                .authenticate(&headers(Some("CONTOSO\\a")))
                .unwrap()
                .mode,
            "trusted-header",
            "the fallback must be reachable"
        );
    }
}
