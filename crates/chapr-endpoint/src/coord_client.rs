// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Typed HTTP client for the coordination service's control channel.
//!
//! One method per coord endpoint, speaking the shared [`chapr_proto`] types, so
//! the endpoint never hand-builds a request body or hand-parses a response. The
//! error contract closes the loop coord opened: coord serialises a
//! [`ChaprError`] into the HTTP error body, and this client deserialises it
//! straight back, so callers `match` on the same typed error they would get
//! in-process.
//!
//! Two mappings matter:
//! - a **transport failure** (can't connect / dropped) → [`ChaprError::CoordUnreachable`],
//!   the signal the read/write paths key their degrade-open / fail-closed
//!   decisions on (concept §10);
//! - an **HTTP error status** → the `ChaprError` parsed from the body, or
//!   [`ChaprError::Internal`] if the body isn't one.
//!
//! Auth: none yet. The intranet link is plain HTTP; Negotiate/Kerberos (concept
//! §13.1) is deferred (logbook I-002).

use chapr_proto::{
    AcquireLeaseRequest, AppendVersionLogRequest, AuditEvent, ChaprError, ClearJournalRequest,
    ClearMoveJournalRequest, ConflictEntry, ConflictsQuery, ConflictsResponse,
    DanglingMovesResponse, DiagnosticGroup, DiagnosticReport,
    HistoryQuery, HistoryResponse,
    LeaseAcquireResponse, LeaseId, LeaseRenewResponse, OpenJournalRequest,
    OpenMoveJournalRequest, PutBlobResponse,
    MovePathsRequest, ReadReceipt, RecordAuditRequest, RecoverJournalRequest, RecoveredFrom,
    RefreshIndexRequest, RegisterConflictRequest, ResolveConflictControl, ResolveRequest,
    ResolveResponse, VersionLogEntry, VersionToken,
};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;

/// A client bound to one coord base URL (e.g. `http://coord.internal:8787`).
#[derive(Clone)]
pub struct CoordClient {
    base: String,
    http: Client,
    /// The caller identity to present to coord (dev auth boundary, §13.1). When
    /// set, sent as `X-Chapr-Principal` on every request; coord's authenticator
    /// derives the acting principal from it (real deployments use Kerberos).
    principal: Option<String>,
    /// The deployment's shared secret, presented as `Authorization: Bearer …` on
    /// every request when set.
    ///
    /// Distinct from `principal` and not a substitute for it: the token says *this
    /// is one of this deployment's endpoints*, the header says *acting for this
    /// user*. Coord's `shared-secret` mode requires both. Absent here, an
    /// enforcing coord returns 401 — which is the point, and is why the wizard
    /// prints this value in the handover.
    token: Option<String>,
}

/// How long to wait for a TCP connect to coord before giving up.
///
/// Short on purpose: an unreachable coord should fail fast so a write fails
/// closed and a read degrades open promptly.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Overall per-request ceiling.
///
/// Deliberately generous rather than snappy: `put_blob` sends a whole pre-image,
/// which is now allowed up to coord's 256 MiB blob limit, and killing a
/// legitimate large upload would be worse than the wedge this prevents. What it
/// does buy is a bound — `reqwest::Client::new()` has NO timeout at all, and
/// these calls run under `block_on` while the exclusive `FILE_SHARE_NONE` handle
/// is held, so a coord that accepts the connection and then stalls used to wedge
/// the file indefinitely for every other user, Excel and Explorer included.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// A PEM file holding an extra root certificate to trust, named by path.
///
/// The machine's own trust store is consulted first (`rustls-tls-native-roots`),
/// which is the right answer wherever a certificate can be pushed to the laptops
/// — a domain with an internal CA, or a fleet tool. This exists for the case that
/// has neither: a workgroup, or a NAS with local accounts, where the coordinator's
/// certificate is a file somebody was handed. It also makes the trust path
/// testable, which the trust store is not.
const CA_CERT_ENV: &str = "CHAPR_COORD_CA_CERT";

impl CoordClient {
    /// Bind to `base_url`. A trailing slash is trimmed so path joins are clean.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::build(base_url.into(), ca_pem_from_env())
    }

    /// Bind to `base_url`, additionally trusting the PEM certificate(s) in `pem`.
    ///
    /// The explicit form of [`CA_CERT_ENV`]. Tests use it because a test cannot
    /// install a root certificate on the machine running it.
    pub fn with_ca_pem(base_url: impl Into<String>, pem: Vec<u8>) -> Self {
        Self::build(base_url.into(), Some(pem))
    }

    fn build(base_url: String, ca_pem: Option<Vec<u8>>) -> Self {
        let mut builder = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT);
        if let Some(pem) = ca_pem {
            match reqwest::Certificate::from_pem_bundle(&pem) {
                Ok(certs) => {
                    for c in certs {
                        builder = builder.add_root_certificate(c);
                    }
                }
                // Warn rather than refuse: without the extra root the connection
                // fails anyway, and it now fails with a message that names trust
                // as the cause. A hard error here would only move the complaint.
                Err(e) => tracing::warn!(
                    error = %e,
                    "{CA_CERT_ENV} did not parse as PEM; continuing with the machine's trust store only"
                ),
            }
        }
        let http = builder
            .build()
            // Only fails if the TLS backend cannot initialise, which would make
            // every request fail anyway; fall back rather than poison `new`.
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "falling back to a default HTTP client with no timeouts");
                Client::new()
            });
        CoordClient {
            base: base_url.trim_end_matches('/').to_string(),
            http,
            principal: None,
            token: None,
        }
    }

    /// Present `principal` as the caller identity on every request.
    pub fn with_principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = Some(principal.into());
        self
    }

    /// Present the deployment's shared secret on every request.
    ///
    /// An empty value is treated as absent, so an unset or blank
    /// `CHAPR_COORD_TOKEN` does not turn into an `Authorization: Bearer ` header
    /// that coord has to reason about.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        let token = token.into();
        self.token = (!token.trim().is_empty()).then(|| token.trim().to_string());
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Attach the credentials this client has: the shared secret and the caller
    /// identity.
    ///
    /// The single chokepoint — `recv_json` and `recv_empty` both route through
    /// here, and the two non-JSON paths call it directly — so a credential added
    /// here reaches every request by construction rather than by remembering.
    fn authed(&self, rb: RequestBuilder) -> RequestBuilder {
        let rb = match &self.token {
            Some(t) => rb.header(reqwest::header::AUTHORIZATION, format!("Bearer {t}")),
            None => rb,
        };
        match &self.principal {
            Some(p) => rb.header("x-chapr-principal", p),
            None => rb,
        }
    }

    // ---- leases (concept §6.4) -------------------------------------------

    pub async fn lease_acquire(
        &self,
        req: &AcquireLeaseRequest,
    ) -> Result<LeaseAcquireResponse, ChaprError> {
        self.recv_json(self.http.post(self.url("/leases")).json(req)).await
    }

    pub async fn lease_renew(&self, lease_id: &LeaseId) -> Result<LeaseRenewResponse, ChaprError> {
        self.recv_json(self.http.post(self.url(&format!("/leases/{lease_id}/renew"))))
            .await
    }

    pub async fn lease_release(&self, lease_id: &LeaseId) -> Result<(), ChaprError> {
        self.recv_empty(self.http.delete(self.url(&format!("/leases/{lease_id}"))))
            .await
    }

    // ---- read path metadata (concept §8.1) -------------------------------

    pub async fn resolve(&self, req: &ResolveRequest) -> Result<ResolveResponse, ChaprError> {
        self.recv_json(self.http.post(self.url("/resolve")).json(req)).await
    }

    pub async fn refresh_index(&self, req: &RefreshIndexRequest) -> Result<(), ChaprError> {
        self.recv_empty(self.http.put(self.url("/index")).json(req)).await
    }

    // ---- intent journal (concept §7) -------------------------------------

    pub async fn journal_open(&self, req: &OpenJournalRequest) -> Result<(), ChaprError> {
        self.recv_empty(self.http.post(self.url("/journal")).json(req)).await
    }

    pub async fn journal_clear(&self, req: &ClearJournalRequest) -> Result<(), ChaprError> {
        self.recv_empty(self.http.post(self.url("/journal/clear")).json(req))
            .await
    }

    /// Recover a dangling write (concept §8.1): coord clears the entry, audits
    /// `crash_recover`, and returns the pre-image provenance the read path needs
    /// to serve the recovered bytes.
    pub async fn recover_journal(
        &self,
        req: &RecoverJournalRequest,
    ) -> Result<RecoveredFrom, ChaprError> {
        self.recv_json(self.http.post(self.url("/journal/recover")).json(req))
            .await
    }

    // ---- history / blob store (concept §12) ------------------------------

    pub async fn put_blob(&self, bytes: Vec<u8>) -> Result<PutBlobResponse, ChaprError> {
        self.recv_json(
            self.http
                .put(self.url("/blobs"))
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(bytes),
        )
        .await
    }

    pub async fn get_blob(&self, version: &VersionToken) -> Result<Vec<u8>, ChaprError> {
        let resp = self
            .authed(self.http.get(self.url(&format!("/blobs/{version}"))))
            .send()
            .await
            .map_err(unreachable)?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(unreachable)?;
        if status.is_success() {
            Ok(bytes.to_vec())
        } else {
            Err(parse_error(status, &bytes))
        }
    }

    pub async fn append_version_log(
        &self,
        req: &AppendVersionLogRequest,
    ) -> Result<VersionLogEntry, ChaprError> {
        self.recv_json(self.http.post(self.url("/version-log")).json(req))
            .await
    }

    /// Record an endpoint-driven audit event (e.g. `write_commit`).
    pub async fn record_audit(&self, req: &RecordAuditRequest) -> Result<AuditEvent, ChaprError> {
        self.recv_json(self.http.post(self.url("/audit")).json(req)).await
    }

    /// Liveness check against `GET /healthz`, for the start-up preflight.
    ///
    /// Worth doing eagerly: without it the first sign that a laptop cannot reach
    /// its coordinator is a refused write in the middle of somebody's work, and
    /// the cause — a wrong URL, a blocked port, an untrusted certificate — is
    /// exactly the class of problem that is obvious at start-up and mystifying
    /// later.
    pub async fn healthz(&self) -> Result<(), ChaprError> {
        let resp = self
            .authed(self.http.get(self.url("/healthz")))
            .send()
            .await
            .map_err(probe_failure)?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ChaprError::Internal {
                message: format!("coord answered /healthz with HTTP {}", resp.status()),
            })
        }
    }

    /// Report one unexpected failure to coord's diagnostics store (E-026).
    ///
    /// Separate from `record_audit` on purpose: audit records what a principal did
    /// and is a primary deliverable, this records why something broke. Different
    /// reader, different retention.
    pub async fn report_diagnostic(
        &self,
        req: &DiagnosticReport,
    ) -> Result<DiagnosticGroup, ChaprError> {
        self.recv_json(self.http.post(self.url("/diagnostics")).json(req))
            .await
    }

    /// Migrate coord state after an SMB rename (concept §6.3). Also discharges
    /// the move intent opened by [`Self::open_move`], in the same transaction.
    pub async fn move_paths(&self, req: &MovePathsRequest) -> Result<(), ChaprError> {
        self.recv_empty(self.http.post(self.url("/move")).json(req)).await
    }

    /// Record the intent to rename, *before* renaming (B3). Fail-closed: a move
    /// that cannot record its intent must not proceed, for the same reason the
    /// write path refuses when it cannot journal — nothing has been changed yet,
    /// so refusing costs a retry and proceeding costs recoverability.
    /// A **404 is translated**, because there is one realistic way to get one:
    /// a coordinator older than this endpoint, which has no `/move/open` route.
    /// Failing closed is right — silently skipping the intent would restore the
    /// unrecoverable window while reporting success — but `HTTP 404 Not Found`
    /// on a move would send an operator hunting for a missing file. It is the
    /// coordinator that is missing, and the fix is to update it first.
    pub async fn open_move(&self, req: &OpenMoveJournalRequest) -> Result<(), ChaprError> {
        match self
            .recv_empty(self.http.post(self.url("/move/open")).json(req))
            .await
        {
            Err(ChaprError::Internal { message }) if message.contains("404") => {
                Err(ChaprError::Internal {
                    message: format!(
                        "this coordinator has no /move/open route, so it is older than this \
                         endpoint. A move records its intent before renaming, so that an \
                         interrupted move can be finished; without the route it cannot, and the \
                         move is refused rather than made unrecoverable. Update the coordinator \
                         first, then the endpoints. ({message})"
                    ),
                })
            }
            other => other,
        }
    }

    /// Drop a move intent because the rename did not happen.
    pub async fn clear_move(&self, req: &ClearMoveJournalRequest) -> Result<(), ChaprError> {
        self.recv_empty(self.http.post(self.url("/move/clear")).json(req))
            .await
    }

    /// Renames whose coord migration is still owed (lease dead). The endpoint
    /// resolves these; coord can only report them, having no file access.
    pub async fn dangling_moves(&self) -> Result<DanglingMovesResponse, ChaprError> {
        self.recv_json(self.http.get(self.url("/move/dangling")))
            .await
    }

    // ---- read-before-write (concept §6.2) --------------------------------

    /// Record that this session has read a version (posted by reads/commits).
    pub async fn record_read(&self, req: &ReadReceipt) -> Result<(), ChaprError> {
        self.recv_empty(self.http.post(self.url("/reads")).json(req)).await
    }

    /// Assert this session read the given version, else `BaseVersionNotRecorded`.
    pub async fn assert_read(&self, req: &ReadReceipt) -> Result<(), ChaprError> {
        self.recv_empty(self.http.post(self.url("/reads/assert")).json(req)).await
    }

    pub async fn history(&self, req: &HistoryQuery) -> Result<HistoryResponse, ChaprError> {
        self.recv_json(self.http.post(self.url("/history")).json(req)).await
    }

    // ---- conflict registry (concept §11) ---------------------------------

    pub async fn register_conflict(
        &self,
        req: &RegisterConflictRequest,
    ) -> Result<ConflictEntry, ChaprError> {
        self.recv_json(self.http.post(self.url("/conflicts/register")).json(req))
            .await
    }

    pub async fn list_conflicts(
        &self,
        req: &ConflictsQuery,
    ) -> Result<ConflictsResponse, ChaprError> {
        self.recv_json(self.http.post(self.url("/conflicts/query")).json(req))
            .await
    }

    pub async fn resolve_conflict(
        &self,
        req: &ResolveConflictControl,
    ) -> Result<ConflictEntry, ChaprError> {
        self.recv_json(self.http.post(self.url("/conflicts/resolve")).json(req))
            .await
    }

    // ---- shared plumbing --------------------------------------------------

    /// Send and decode a JSON body on success; map errors as documented.
    async fn recv_json<T: DeserializeOwned>(&self, rb: RequestBuilder) -> Result<T, ChaprError> {
        let resp = self.authed(rb).send().await.map_err(unreachable)?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(unreachable)?;
        if status.is_success() {
            serde_json::from_slice(&bytes).map_err(|e| ChaprError::Internal {
                message: format!("coord response decode failed: {e}"),
            })
        } else {
            Err(parse_error(status, &bytes))
        }
    }

    /// Send expecting no body (204/200-empty); success → `Ok(())`.
    async fn recv_empty(&self, rb: RequestBuilder) -> Result<(), ChaprError> {
        let resp = self.authed(rb).send().await.map_err(unreachable)?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let bytes = resp.bytes().await.map_err(unreachable)?;
            Err(parse_error(status, &bytes))
        }
    }
}

/// Read the extra root certificate named by [`CA_CERT_ENV`], if any.
fn ca_pem_from_env() -> Option<Vec<u8>> {
    let path = std::env::var(CA_CERT_ENV).ok().filter(|p| !p.trim().is_empty())?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            tracing::info!(path = %path, "trusting an extra root certificate");
            Some(bytes)
        }
        Err(e) => {
            tracing::warn!(path = %path, error = %e, "{CA_CERT_ENV} could not be read");
            None
        }
    }
}

/// The whole source chain of a transport error, flattened onto one line.
///
/// reqwest's own `Display` is "error sending request for url (...)" and says
/// nothing about why; the sentence that identifies the problem is always further
/// down the chain, in hyper or rustls. Throwing that away is what made a wrong
/// port and an untrusted certificate the same event (I-018).
fn transport_cause(e: &reqwest::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        parts.push(s.to_string());
        src = s.source();
    }
    parts.join(": ")
}

/// Does this transport failure say the certificate itself was not accepted?
///
/// Matched on the chain's text rather than on a type, because rustls' error is
/// erased behind `reqwest::Error` by the time it gets here. Over-matching is the
/// safe direction: the worst case is advice about certificates attached to a
/// failure that mentions one anyway.
fn is_trust_failure(e: &reqwest::Error) -> bool {
    let cause = transport_cause(e).to_ascii_lowercase();
    cause.contains("certificate")
        || cause.contains("unknownissuer")
        || cause.contains("notvalidforname")
}

/// A transport failure means coord could not be reached at all.
///
/// The variant stays unit-typed — it is the wire contract, and every write-path
/// caller only needs "fail closed". The *cause* is logged instead of carried,
/// because the person who has to fix a refused certificate is not the code.
fn unreachable(e: reqwest::Error) -> ChaprError {
    tracing::warn!(cause = %transport_cause(&e), "coord transport failure");
    ChaprError::CoordUnreachable
}

/// Map a transport failure from the start-up probe, where a human is reading.
///
/// A refused certificate is **not** "the coordinator is unreachable": it is
/// running, it answered, and this machine declined to trust it. Reporting that as
/// unreachable sends an administrator to check a service that is fine — the
/// explanation defect F2 named, and the reason this one call site does not use
/// [`unreachable`].
fn probe_failure(e: reqwest::Error) -> ChaprError {
    if !is_trust_failure(&e) {
        return unreachable(e);
    }
    ChaprError::Internal {
        message: format!(
            "the coordinator answered, but this machine does not trust its TLS certificate. \
             Install that certificate — or the CA that issued it — in this machine's trusted \
             root store, or set {CA_CERT_ENV} to the path of the certificate file. \
             The service itself is running; nothing is wrong with the port or the URL. \
             ({})",
            transport_cause(&e)
        ),
    }
}

/// Turn an HTTP error response into the `ChaprError` coord encoded in the body,
/// falling back to `Internal` if the body isn't a recognisable one.
fn parse_error(status: StatusCode, body: &[u8]) -> ChaprError {
    serde_json::from_slice::<ChaprError>(body).unwrap_or(ChaprError::Internal {
        message: format!("coord returned HTTP {status}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chapr_proto::{CanonicalPath, JournalState, LeasePurpose, Principal};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cpath() -> CanonicalPath {
        CanonicalPath::new_unchecked("\\\\srv\\share\\a.md")
    }

    fn resolve_req() -> ResolveRequest {
        ResolveRequest {
            path: cpath(),
            mtime: chrono::DateTime::parse_from_rfc3339("2026-07-21T09:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            size: 5,
        }
    }

    #[tokio::test]
    async fn resolve_sends_to_resolve_and_parses_ok() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/resolve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "cached_version": VersionToken::hash(b"x").as_str(),
                "journal_state": "clean"
            })))
            .mount(&server)
            .await;

        let client = CoordClient::new(server.uri());
        let resp = client.resolve(&resolve_req()).await.unwrap();
        assert_eq!(resp.cached_version, Some(VersionToken::hash(b"x")));
        assert_eq!(resp.journal_state, JournalState::Clean);
        assert!(resp.lease_state.is_none());
    }

    #[tokio::test]
    async fn http_error_body_deserialises_back_to_chapr_error() {
        // The closed loop: coord's LEASE_HELD → client returns ChaprError::LeaseHeld.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/leases"))
            .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
                "code": "LEASE_HELD",
                "holder": "CONTOSO\\jsmith",
                "paths": ["\\\\srv\\share\\a.md"]
            })))
            .mount(&server)
            .await;

        let client = CoordClient::new(server.uri());
        let err = client
            .lease_acquire(&AcquireLeaseRequest {
                principal: Principal::new_unchecked("CONTOSO\\bthomas"),
                session_id: chapr_proto::SessionId::new_unchecked("sess-1"),
                purpose: LeasePurpose::Write,
                paths: vec![cpath()],
            })
            .await
            .unwrap_err();
        match err {
            ChaprError::LeaseHeld { holder, .. } => {
                assert_eq!(holder, Principal::new_unchecked("CONTOSO\\jsmith"));
            }
            other => panic!("expected LeaseHeld, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_content_success_is_ok() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/leases/lease-abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let client = CoordClient::new(server.uri());
        client
            .lease_release(&LeaseId::new_unchecked("lease-abc"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn get_blob_returns_raw_bytes() {
        let version = VersionToken::hash(b"pre-image");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/blobs/{version}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"pre-image".to_vec()))
            .mount(&server)
            .await;
        let client = CoordClient::new(server.uri());
        let bytes = client.get_blob(&version).await.unwrap();
        assert_eq!(bytes, b"pre-image");
    }

    #[tokio::test]
    async fn recover_journal_parses_recovered_from() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/journal/recover"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": VersionToken::hash(b"pre").as_str(),
                "interrupted_writer": "CONTOSO\\crashed",
                "at": "2026-07-21T09:00:00Z"
            })))
            .mount(&server)
            .await;
        let client = CoordClient::new(server.uri());
        let rf = client
            .recover_journal(&chapr_proto::RecoverJournalRequest {
                path: cpath(),
                principal: Principal::new_unchecked("CONTOSO\\reader"),
                session_id: chapr_proto::SessionId::new_unchecked("sess-1"),
            })
            .await
            .unwrap();
        assert_eq!(rf.version, VersionToken::hash(b"pre"));
        assert_eq!(rf.interrupted_writer, Principal::new_unchecked("CONTOSO\\crashed"));
    }

    /// The realistic upgrade mistake: endpoints updated before the coordinator.
    /// The move is refused either way — that is the fail-closed choice — but the
    /// message has to name the actual cause, or an operator reads "404 Not
    /// Found" on a move and goes looking for a missing file.
    #[tokio::test]
    async fn a_coordinator_without_the_route_says_so_in_the_error() {
        // An empty mock server answers 404 to everything, which is exactly what
        // a coordinator predating this route does.
        let server = MockServer::start().await;
        let client = CoordClient::new(server.uri());
        let err = client
            .open_move(&chapr_proto::OpenMoveJournalRequest {
                src: cpath(),
                dst: CanonicalPath::new_unchecked("\\\\srv\\share\\b.md"),
                lease_id: LeaseId::new_unchecked("lease-1"),
                principal: Principal::new_unchecked("CONTOSO\\demo"),
                session_id: chapr_proto::SessionId::new_unchecked("sess-1"),
                version: VersionToken::hash(b"x"),
                size: 1,
                overwrite: false,
                dst_pre_image: None,
            })
            .await
            .unwrap_err();
        let ChaprError::Internal { message } = err else {
            panic!("expected Internal, got {err:?}");
        };
        assert!(
            message.contains("older than this endpoint") && message.contains("Update the coordinator"),
            "the error must name the cause and the fix: {message}"
        );
    }

    #[tokio::test]
    async fn transport_failure_maps_to_coord_unreachable() {
        // Port 1 is not listening → connection refused.
        let client = CoordClient::new("http://127.0.0.1:1");
        let err = client.history(&HistoryQuery { path: cpath() }).await.unwrap_err();
        assert!(matches!(err, ChaprError::CoordUnreachable));
    }
}
