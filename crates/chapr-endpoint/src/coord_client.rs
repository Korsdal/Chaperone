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
    ConflictEntry, ConflictsQuery, ConflictsResponse, DiagnosticGroup, DiagnosticReport,
    HistoryQuery, HistoryResponse,
    LeaseAcquireResponse, LeaseId, LeaseRenewResponse, OpenJournalRequest, PutBlobResponse,
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

impl CoordClient {
    /// Bind to `base_url`. A trailing slash is trimmed so path joins are clean.
    pub fn new(base_url: impl Into<String>) -> Self {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            // Only fails if the TLS backend cannot initialise, which would make
            // every request fail anyway; fall back rather than poison `new`.
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "falling back to a default HTTP client with no timeouts");
                Client::new()
            });
        CoordClient {
            base: base_url.into().trim_end_matches('/').to_string(),
            http,
            principal: None,
        }
    }

    /// Present `principal` as the caller identity on every request.
    pub fn with_principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = Some(principal.into());
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Attach the caller-identity header if one is configured.
    fn authed(&self, rb: RequestBuilder) -> RequestBuilder {
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

    /// Migrate coord state after an SMB rename (concept §6.3).
    pub async fn move_paths(&self, req: &MovePathsRequest) -> Result<(), ChaprError> {
        self.recv_empty(self.http.post(self.url("/move")).json(req)).await
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

/// A transport failure means coord could not be reached at all.
fn unreachable(_e: reqwest::Error) -> ChaprError {
    ChaprError::CoordUnreachable
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

    #[tokio::test]
    async fn transport_failure_maps_to_coord_unreachable() {
        // Port 1 is not listening → connection refused.
        let client = CoordClient::new("http://127.0.0.1:1");
        let err = client.history(&HistoryQuery { path: cpath() }).await.unwrap_err();
        assert!(matches!(err, ChaprError::CoordUnreachable));
    }
}
