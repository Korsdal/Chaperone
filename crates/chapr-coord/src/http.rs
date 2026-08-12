//! The axum HTTP surface for the lease endpoints (E-002 slice of concept §6.4).
//!
//! In production this channel is authenticated with Negotiate/Kerberos (concept
//! §13.1, intranet; no OAuth). That is **not** wired up in E-002 — see the
//! `principal` shim on [`AcquireLeaseRequest`].
//!
//! Routes:
//! - `GET    /healthz`             — liveness (coord availability == write
//!   availability, concept §4.2, so it must be monitorable).
//! - `POST   /leases`             — `lease_acquire` (all-or-none set).
//! - `DELETE /leases/{lease_id}`  — `lease_release`.

use crate::state::AppState;
use crate::{history, index, journal, lease};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chapr_proto::{
    AcquireLeaseRequest, AppendVersionLogRequest, AuditEvent, CanonicalPath, ChaprError,
    DiagnosticGroup, DiagnosticReport, DiagnosticsQuery, DiagnosticsResponse,
    ClearJournalRequest, ConflictEntry, ConflictsQuery, ConflictsResponse, HistoryQuery,
    HistoryResponse, LeaseAcquireResponse, LeaseId, LeaseReleaseResponse, LeaseRenewResponse,
    MovePathsRequest, OpenJournalRequest, PutBlobResponse, ReadReceipt, RecordAuditRequest,
    RecoverJournalRequest, RecoveredFrom, RefreshIndexRequest, RegisterConflictRequest,
    ResolveConflictControl, ResolveRequest, ResolveResponse, VersionLogEntry, VersionToken,
};
use serde::{Deserialize, Serialize};

// The control-channel request/response bodies (`AcquireLeaseRequest`,
// `RefreshIndexRequest`, `OpenJournalRequest`, `ClearJournalRequest`,
// `PutBlobResponse`, `AppendVersionLogRequest`, `HistoryQuery`) now live in
// `chapr-proto` — promoted from here in E-005 once the endpoint became the
// second consumer, so both binaries share one definition.

/// Body of `POST /audit/query` — the governance read of the audit trail.
/// Coord-internal (there is no `chapr.*` audit tool).
///
/// Every field is optional and they compose. With `path` set this is the
/// original per-file governance read ("who changed *this* and when"), which the
/// admin view's file drill-down and the existing callers both use; without it,
/// the trail across every path, which is what the admin view's Audit tab lists.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AuditQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<CanonicalPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<chapr_proto::AuditKind>,
    /// Only events at or after this instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Body of `POST /leases/query` — every currently-held lease.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LeasesQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Response of `GET /admin/overview` — the stat tiles, plus what this coordinator
/// is actually configured for.
///
/// The deployment half is not decoration. "What is this coord set up for" is the
/// first question in any support call, and it is also what a future Settings tab
/// has to render before it can offer to change anything (the parked follow-on).
#[derive(Clone, Debug, Serialize)]
pub struct AdminOverview {
    pub version: &'static str,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub uptime_s: i64,
    /// Database size from `page_count × page_size` rather than a file stat: it
    /// needs no path on `AppState` and works for an in-memory database too.
    pub db_bytes: i64,
    pub diagnostics_open_errors: i64,
    pub diagnostics_open_warnings: i64,
    pub conflicts_open: i64,
    pub leases_held: usize,
    pub leases_expiring: usize,
    pub leases_stale: usize,
    // ---- deployment ----
    pub config_path: Option<String>,
    pub auth_mode: String,
    pub backend: String,
    pub blob_root: String,
    pub backend_routes: Vec<AdminBackendRoute>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AdminBackendRoute {
    pub prefix: String,
    pub kind: String,
}

/// Body of `POST /watch/event` — an external watcher (a non-Windows coord's
/// inotify feeder, a POSIX-backend watcher, or a cloud webhook) pushes one
/// change event (§14, E-017). The OS-agnostic counterpart to the Windows
/// `ReadDirectoryChangesW` source; both feed the same [`crate::watch::apply`]
/// effect. Coord-local (no proto type) until a second Rust consumer appears
/// (D-022). `path` is expected **already canonical** — coord is backend-agnostic
/// and cannot canonicalise for a caller whose path grammar it does not know.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WatchEventRequest {
    Changed { path: CanonicalPath },
    Removed { path: CanonicalPath },
    Overflow,
}

impl From<WatchEventRequest> for crate::watch::WatchEvent {
    fn from(r: WatchEventRequest) -> Self {
        use crate::watch::WatchEvent;
        match r {
            WatchEventRequest::Changed { path } => WatchEvent::Changed(path),
            WatchEventRequest::Removed { path } => WatchEvent::Removed(path),
            WatchEventRequest::Overflow => WatchEvent::Overflow,
        }
    }
}

/// Largest pre-image coord will accept on `PUT /blobs`.
///
/// axum's `DefaultBodyLimit` is 2 MiB, which silently capped every write to a
/// file already larger than that — a write snapshots the file's *current* bytes,
/// so the limit applied to the existing file, not the new content. Nothing in
/// the config or docs ever mentioned a size ceiling because this byte channel
/// was not supposed to exist (see the invariant-6 note in README.md).
///
/// Note both ends buffer fully in memory (`Bytes` in, `Vec<u8>` out), so this
/// number is also coord's per-in-flight-write memory cost.
pub const MAX_BLOB_BYTES: usize = 256 * 1024 * 1024;

/// Build the coord router over the given state.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/leases", post(acquire))
        .route("/leases/{lease_id}", delete(release))
        .route("/leases/{lease_id}/renew", post(renew))
        .route("/resolve", post(resolve))
        .route("/index", put(refresh_index))
        .route("/journal", post(open_journal))
        .route("/journal/clear", post(clear_journal))
        .route("/journal/recover", post(recover_journal))
        .route(
            "/blobs",
            put(put_blob).layer(DefaultBodyLimit::max(MAX_BLOB_BYTES)),
        )
        .route("/blobs/{version}", get(get_blob))
        .route("/version-log", post(append_version_log))
        .route("/history", post(get_history))
        .route("/audit", post(record_audit))
        .route("/audit/query", post(query_audit))
        .route("/conflicts/register", post(register_conflict))
        .route("/conflicts/query", post(query_conflicts))
        .route("/conflicts/resolve", post(resolve_conflict))
        .route("/move", post(move_paths))
        .route("/reads", post(record_read))
        .route("/reads/assert", post(assert_read))
        .route("/watch/event", post(watch_event))
        .route("/diagnostics", post(report_diagnostic))
        .route("/diagnostics/query", post(query_diagnostics))
        .route("/leases/query", post(query_leases))
        .route("/admin", get(admin_page))
        .route("/admin/overview", get(admin_overview))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn acquire(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<AcquireLeaseRequest>,
) -> Result<Json<LeaseAcquireResponse>, ApiError> {
    // Read-before-write's sibling: the holder is the authenticated caller, not
    // the (spoofable) body field (concept §13.1, closes I-001). Falls back to
    // the body principal only when connection auth is disabled.
    let principal = caller.0.unwrap_or(req.principal);
    let resp = lease::acquire(&st, principal, req.session_id, req.purpose, req.paths).await?;
    Ok(Json(resp))
}

async fn release(
    State(st): State<AppState>,
    Path(lease_id): Path<String>,
) -> Result<Json<LeaseReleaseResponse>, ApiError> {
    lease::release(&st, &LeaseId::new_unchecked(lease_id)).await?;
    Ok(Json(LeaseReleaseResponse {}))
}

async fn renew(
    State(st): State<AppState>,
    Path(lease_id): Path<String>,
) -> Result<Json<LeaseRenewResponse>, ApiError> {
    let resp = lease::renew(&st, &LeaseId::new_unchecked(lease_id)).await?;
    Ok(Json(resp))
}

async fn resolve(
    State(st): State<AppState>,
    Json(req): Json<ResolveRequest>,
) -> Result<Json<ResolveResponse>, ApiError> {
    let resp = index::resolve(&st, req).await?;
    Ok(Json(resp))
}

async fn refresh_index(
    State(st): State<AppState>,
    Json(req): Json<RefreshIndexRequest>,
) -> Result<StatusCode, ApiError> {
    index::refresh(&st, &req.path, &req.version, req.mtime, req.size).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn open_journal(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<OpenJournalRequest>,
) -> Result<StatusCode, ApiError> {
    let principal = caller.0.unwrap_or(req.principal);
    journal::open(
        &st,
        &req.path,
        &req.lease_id,
        &principal,
        &req.pre_image_version,
        req.intended_version.as_ref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_journal(
    State(st): State<AppState>,
    Json(req): Json<ClearJournalRequest>,
) -> Result<StatusCode, ApiError> {
    journal::clear(&st, &req.path).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn recover_journal(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<RecoverJournalRequest>,
) -> Result<Json<RecoveredFrom>, ApiError> {
    // `principal`/`session_id` on the request are no longer consumed here: the
    // `crash_recover` audit event moved to the endpoint, which is the only side
    // that can hash the file and so the only side that knows whether a recovery
    // actually happened. Kept on the wire — re-binding them costs nothing and
    // removing them would break older endpoints for no gain.
    let _ = caller;
    let recovered = journal::recover(&st, &req.path).await?;
    Ok(Json(recovered))
}

/// `PUT /blobs` — body is the raw pre-image bytes; coord hashes and stores.
/// `Bytes` must be the final extractor.
async fn put_blob(
    State(st): State<AppState>,
    body: Bytes,
) -> Result<Json<PutBlobResponse>, ApiError> {
    let stored = history::put_blob(&st.blob_root, &body).await?;
    Ok(Json(PutBlobResponse {
        version: stored.version,
        size: stored.size,
        deduplicated: stored.deduplicated,
    }))
}

/// `GET /blobs/{version}` — the raw bytes, as `application/octet-stream`.
///
/// Gated by [`crate::auth::Caller`] (D-029). Since D-026 this route serves file
/// **content** — every write snapshots its pre-image here — and it applies no ACL
/// check of its own, so reachability of the control plane implies read access to
/// the history of every file coord knows about.
///
/// Under `trusted-header` the extractor authenticates nothing, and that is
/// understood: it buys **attribution**, and it puts the route on the enforcement
/// path *before* E-015 switches enforcement on. I-002's own note is that the
/// ungated routes stay reachable while the gated ones begin rejecting — a gap
/// that converts into a real bypass at exactly the moment auth starts working.
/// The two content-bearing routes are the ones that must not be in that set.
async fn get_blob(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Path(version): Path<String>,
) -> Result<Vec<u8>, ApiError> {
    let version = VersionToken::from_hex(version.clone()).ok_or_else(|| {
        ApiError(ChaprError::InvalidPath {
            raw: version,
            reason: "not a valid BLAKE3 version token".into(),
        })
    })?;
    let bytes = history::get_blob(&st.blob_root, &version).await?;
    tracing::info!(
        principal = %caller_label(&caller),
        %version,
        size = bytes.len(),
        "blob fetch"
    );
    Ok(bytes)
}

/// `POST /diagnostics` — an endpoint reports one unexpected failure (E-026).
///
/// Attributed to the **authenticated** caller over the body's principal, like
/// every other principal-bearing route (D-016): a diagnostic is read to tell one
/// misconfigured laptop from a fault hitting everybody, so the identity has to be
/// the connection's rather than one the body asserts.
async fn report_diagnostic(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<DiagnosticReport>,
) -> Result<Json<DiagnosticGroup>, ApiError> {
    let report = DiagnosticReport {
        principal: caller.0.unwrap_or(req.principal),
        ..req
    };
    let group = crate::diagnostics::record(&st, &report).await?;
    tracing::warn!(
        code = %group.code,
        path = group.path.as_ref().map(|p| p.as_str()).unwrap_or("-"),
        count = group.count,
        principal = %report.principal,
        "endpoint reported a diagnostic"
    );
    Ok(Json(group))
}

/// `POST /diagnostics/query` — read the grouped failures.
///
/// Ungated beyond `Caller` for now. The **admin role** that should own this is
/// E-024a, deliberately deferred with the dashboard (D-030): this slice exists so
/// that when something breaks at the customer we can find out *what*, and gating
/// it behind a role nobody can hold yet would defeat that.
async fn query_diagnostics(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<DiagnosticsQuery>,
) -> Result<Json<DiagnosticsResponse>, ApiError> {
    let resp = crate::diagnostics::query(&st.pool, &req).await?;
    tracing::info!(
        principal = %caller_label(&caller),
        groups = resp.groups.len(),
        "diagnostics query"
    );
    Ok(Json(resp))
}

/// `POST /leases/query` — every currently-held lease (E-024 read side).
///
/// A read, so expired rows are filtered by time rather than swept: the lazy sweep
/// in `lease_acquire` and the background reaper own that.
async fn query_leases(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<LeasesQuery>,
) -> Result<Json<Vec<lease::LeaseView>>, ApiError> {
    let limit = req.limit.unwrap_or(200) as i64;
    let leases = lease::list_held(&st.pool, limit).await?;
    tracing::debug!(
        principal = %caller_label(&caller),
        leases = leases.len(),
        "leases query"
    );
    Ok(Json(leases))
}

/// The admin page itself — one self-contained file, embedded in the binary.
///
/// Embedded rather than served from disk so the deployment stays "one binary":
/// nothing extra to copy, no path to get wrong, and it works on a closed network
/// because the page loads nothing from anywhere else.
async fn admin_page() -> impl IntoResponse {
    axum::response::Html(include_str!("admin/index.html"))
}

/// `GET /admin/overview` — the stat tiles plus this coordinator's configuration.
async fn admin_overview(State(st): State<AppState>) -> Result<Json<AdminOverview>, ApiError> {
    let (errors, warnings) = crate::diagnostics::count_open(&st.pool).await?;
    let conflicts_open = crate::conflict::count_open_all(&st.pool).await?;
    // Reuse the same listing the Leases tab renders, so a tile can never disagree
    // with the table below it.
    let leases = lease::list_held(&st.pool, 1000).await?;
    let count = |s: lease::LeaseHealth| leases.iter().filter(|l| l.state == s).count();

    let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(&st.pool)
        .await
        .unwrap_or(0);
    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
        .fetch_one(&st.pool)
        .await
        .unwrap_or(0);

    Ok(Json(AdminOverview {
        version: env!("CARGO_PKG_VERSION"),
        started_at: st.started_at,
        uptime_s: (chrono::Utc::now() - st.started_at).num_seconds(),
        db_bytes: page_count * page_size,
        diagnostics_open_errors: errors,
        diagnostics_open_warnings: warnings,
        conflicts_open,
        leases_held: count(lease::LeaseHealth::Held),
        leases_expiring: count(lease::LeaseHealth::Expiring),
        leases_stale: count(lease::LeaseHealth::Stale),
        config_path: st.config_path.as_ref().map(|p| p.display().to_string()),
        auth_mode: st.auth_mode.clone(),
        backend: st.backend_default.to_string(),
        blob_root: st.blob_root.display().to_string(),
        backend_routes: st
            .backend_routes
            .iter()
            .map(|r| AdminBackendRoute {
                prefix: r.prefix.clone(),
                kind: r.kind.to_string(),
            })
            .collect(),
    }))
}

/// How to name the caller in a log line. `AuditEvent` cannot carry these two
/// routes: it is keyed by `(canonical_path, session_id)`, and a blob fetch has
/// neither — the version hash is content-addressed and deduplicated, so it may
/// name bytes shared by several paths, and no session travels on the request.
/// Bending the audit record to fit would mean inventing a path, which is the
/// opposite of what an audit trail is for. Durable attribution for these reads
/// belongs in the diagnostics/access store (E-026), whose record shape fits;
/// until then the structured log is the trail.
fn caller_label(caller: &crate::auth::Caller) -> &str {
    caller.0.as_ref().map_or("(unauthenticated)", |p| p.as_str())
}

async fn append_version_log(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<AppendVersionLogRequest>,
) -> Result<Json<VersionLogEntry>, ApiError> {
    let writer_principal = caller.0.unwrap_or(req.writer_principal);
    let entry = history::append_version_log(
        &st,
        &req.path,
        &req.blob_hash,
        &writer_principal,
        req.size,
        req.event,
        req.pre_image.as_ref(),
    )
    .await?;
    Ok(Json(entry))
}

/// `POST /history` — a file's version chain, newest first.
///
/// Gated for the same reason as [`get_blob`]: the chain is what turns a blob
/// store into a readable history, since it hands out the very version hashes
/// `GET /blobs/{version}` takes. Gating the bytes and leaving their index open
/// would be half a gate.
async fn get_history(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<HistoryQuery>,
) -> Result<Json<HistoryResponse>, ApiError> {
    let resp = history::history(&st.pool, &req.path).await?;
    tracing::info!(
        principal = %caller_label(&caller),
        path = %req.path,
        entries = resp.entries.len(),
        "history query"
    );
    Ok(Json(resp))
}

async fn record_audit(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<RecordAuditRequest>,
) -> Result<Json<AuditEvent>, ApiError> {
    let principal = caller.0.unwrap_or(req.principal);
    let event = crate::audit::record(
        &st,
        &principal,
        &req.session_id,
        &req.path,
        req.kind,
        req.from_version.as_ref(),
        req.to_version.as_ref(),
        &req.detail,
    )
    .await?;
    Ok(Json(event))
}

/// `POST /audit/query` — the audit trail, per file or across every path.
///
/// `path` present keeps the original governance read exactly as it was, including
/// its unbounded result: a "who changed this file" answer is incomplete if it is
/// silently truncated. The fleet-wide form is a browsable list and is bounded.
async fn query_audit(
    State(st): State<AppState>,
    Json(req): Json<AuditQuery>,
) -> Result<Json<Vec<AuditEvent>>, ApiError> {
    let events = match &req.path {
        Some(path) => crate::audit::query(&st.pool, path).await?,
        None => {
            crate::audit::query_recent(
                &st.pool,
                req.principal.as_deref(),
                req.kind,
                req.since.map(|t| t.timestamp_millis()),
                req.limit.unwrap_or(200) as i64,
            )
            .await?
        }
    };
    Ok(Json(events))
}

async fn register_conflict(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<RegisterConflictRequest>,
) -> Result<Json<ConflictEntry>, ApiError> {
    let losing_principal = caller.0.unwrap_or(req.losing_principal);
    let entry = crate::conflict::register(
        &st,
        &req.base_path,
        &req.sidecar_path,
        &losing_principal,
        &req.session_id,
    )
    .await?;
    Ok(Json(entry))
}

async fn query_conflicts(
    State(st): State<AppState>,
    Json(req): Json<ConflictsQuery>,
) -> Result<Json<ConflictsResponse>, ApiError> {
    let conflicts = crate::conflict::list(&st.pool, &req.scope).await?;
    Ok(Json(ConflictsResponse { conflicts }))
}

async fn resolve_conflict(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<ResolveConflictControl>,
) -> Result<Json<ConflictEntry>, ApiError> {
    let principal = caller.0.unwrap_or(req.principal);
    let entry = crate::conflict::resolve(
        &st,
        &req.conflict_id,
        req.resolution,
        &principal,
        &req.session_id,
    )
    .await?;
    Ok(Json(entry))
}

async fn move_paths(
    State(st): State<AppState>,
    caller: crate::auth::Caller,
    Json(req): Json<MovePathsRequest>,
) -> Result<StatusCode, ApiError> {
    let principal = caller.0.unwrap_or(req.principal);
    crate::mv::move_paths(
        &st,
        &req.src,
        &req.dst,
        &req.version,
        req.size,
        req.overwrite,
        &principal,
        &req.session_id,
        req.dst_pre_image.as_ref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn record_read(
    State(st): State<AppState>,
    Json(req): Json<ReadReceipt>,
) -> Result<StatusCode, ApiError> {
    crate::reads::record(&st, &req.session_id, &req.path, &req.version).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn assert_read(
    State(st): State<AppState>,
    Json(req): Json<ReadReceipt>,
) -> Result<StatusCode, ApiError> {
    crate::reads::assert(&st.pool, &req.session_id, &req.path, &req.version).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /watch/event` — an external watcher pushes a change event (§14, E-017).
/// The OS-agnostic sibling of the Windows `ReadDirectoryChangesW` source: it
/// feeds the same [`crate::watch::apply`] effect that keeps coord's version
/// index honest against out-of-band edits (invalidate / auto-close a deleted
/// conflict sidecar / overflow rescan). Gated by [`crate::auth::Caller`] for
/// authorization only — the effects attribute to `SERVICE\chapr-watcher`
/// internally, so the caller's principal is intentionally unused here.
async fn watch_event(
    State(st): State<AppState>,
    _caller: crate::auth::Caller,
    Json(req): Json<WatchEventRequest>,
) -> Result<StatusCode, ApiError> {
    crate::watch::apply(&st, &req.into()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Newtype over [`ChaprError`] so it can carry an `IntoResponse` impl (the
/// orphan rule forbids implementing it on the foreign proto type directly). The
/// body is the serialised `ChaprError` itself, so clients get the same
/// machine-readable `code`-tagged error over HTTP as over any other channel.
pub struct ApiError(pub ChaprError);

impl From<ChaprError> for ApiError {
    fn from(e: ChaprError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        use ChaprError::*;
        let status = match &self.0 {
            // Contended state — someone else holds it, or it is otherwise busy.
            LeaseHeld { .. }
            | LeaseExpired { .. }
            | LeaseLost { .. }
            | MaxLeaseLifetimeExceeded { .. }
            | Conflict { .. }
            | SharingViolation { .. }
            | RetryBudgetExhausted { .. } => StatusCode::CONFLICT,

            // Locked by a human in Office — humans always win (concept §10).
            OfficeLockPresent { .. } => StatusCode::LOCKED,

            // Absent things.
            LeaseNotFound { .. }
            | NotFound { .. }
            | VersionNotFound { .. }
            | ConflictNotFound { .. } => StatusCode::NOT_FOUND,

            AlreadyExists { .. } => StatusCode::CONFLICT,

            // Bad request from the caller.
            BaseVersionNotRecorded { .. }
            | BaseVersionRequired { .. }
            | ForceRequiresReason { .. }
            | InvalidPath { .. } => StatusCode::BAD_REQUEST,

            PermissionDenied { .. } => StatusCode::FORBIDDEN,

            CoordUnreachable => StatusCode::SERVICE_UNAVAILABLE,

            // Bugs / operational faults. `CommittedButUnrecorded` is raised by
            // the endpoint, never by coord, but the shared enum stays total.
            RecoveryFailed { .. }
            | Io { .. }
            | Internal { .. }
            | CommittedButUnrecorded { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt; // for `oneshot`

    fn body_json(v: &serde_json::Value) -> Body {
        Body::from(serde_json::to_vec(v).unwrap())
    }

    #[tokio::test]
    async fn acquire_returns_200_and_a_lease_id() {
        let app = router(AppState::new(db::test_pool().await));
        let req = Request::builder()
            .method("POST")
            .uri("/leases")
            .header("content-type", "application/json")
            .body(body_json(&serde_json::json!({
                "principal": "CONTOSO\\jsmith",
                "session_id": "sess-t", "purpose": "write",
                "paths": ["\\\\srv\\share\\a.md"]
            })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: LeaseAcquireResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.lease_id.as_str().starts_with("lease-"));
        assert_eq!(parsed.ttl_s, lease::HEARTBEAT_TTL_S);
    }

    #[tokio::test]
    async fn second_acquire_returns_409_lease_held() {
        let state = AppState::new(db::test_pool().await);
        let app = router(state);

        let make = || {
            Request::builder()
                .method("POST")
                .uri("/leases")
                .header("content-type", "application/json")
                .body(body_json(&serde_json::json!({
                    "principal": "CONTOSO\\jsmith",
                    "session_id": "sess-t", "purpose": "write",
                    "paths": ["\\\\srv\\share\\a.md"]
                })))
                .unwrap()
        };

        let first = app.clone().oneshot(make()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = app.oneshot(make()).await.unwrap();
        assert_eq!(second.status(), StatusCode::CONFLICT);
        let bytes = to_bytes(second.into_body(), usize::MAX).await.unwrap();
        let err: ChaprError = serde_json::from_slice(&bytes).unwrap();
        assert!(matches!(err, ChaprError::LeaseHeld { .. }));
    }

    #[tokio::test]
    async fn release_unknown_lease_returns_404() {
        let app = router(AppState::new(db::test_pool().await));
        let req = Request::builder()
            .method("DELETE")
            .uri("/leases/lease-nope")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn healthz_is_ok() {
        let app = router(AppState::new(db::test_pool().await));
        let resp = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn renew_returns_200_with_renewed_until() {
        let state = AppState::new(db::test_pool().await);
        let app = router(state);

        let acq_req = Request::builder()
            .method("POST")
            .uri("/leases")
            .header("content-type", "application/json")
            .body(body_json(&serde_json::json!({
                "principal": "CONTOSO\\jsmith", "session_id": "sess-t", "purpose": "write",
                "paths": ["\\\\srv\\share\\a.md"]
            })))
            .unwrap();
        let acq = app.clone().oneshot(acq_req).await.unwrap();
        let bytes = to_bytes(acq.into_body(), usize::MAX).await.unwrap();
        let lease: LeaseAcquireResponse = serde_json::from_slice(&bytes).unwrap();

        let renew_req = Request::builder()
            .method("POST")
            .uri(format!("/leases/{}/renew", lease.lease_id))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(renew_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let renewed: LeaseRenewResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(renewed.renewed_until > chrono::Utc::now());
    }

    #[tokio::test]
    async fn refresh_then_resolve_over_http() {
        let state = AppState::new(db::test_pool().await);
        let app = router(state);

        // PUT /index
        let put_req = Request::builder()
            .method("PUT")
            .uri("/index")
            .header("content-type", "application/json")
            .body(body_json(&serde_json::json!({
                "path": "\\\\srv\\share\\doc.md",
                "version": VersionToken::hash(b"hello").as_str(),
                "mtime": "2026-07-21T09:00:00Z",
                "size": 5
            })))
            .unwrap();
        let put = app.clone().oneshot(put_req).await.unwrap();
        assert_eq!(put.status(), StatusCode::NO_CONTENT);

        // POST /resolve with the matching composite key → hit.
        let resolve_req = Request::builder()
            .method("POST")
            .uri("/resolve")
            .header("content-type", "application/json")
            .body(body_json(&serde_json::json!({
                "path": "\\\\srv\\share\\doc.md",
                "mtime": "2026-07-21T09:00:00Z",
                "size": 5
            })))
            .unwrap();
        let resp = app.oneshot(resolve_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let resolved: ResolveResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(resolved.cached_version, Some(VersionToken::hash(b"hello")));
        // Coord announces the backend on the wire (§14).
        assert_eq!(
            resolved.backend,
            Some(chapr_proto::BackendDescriptor {
                kind: chapr_proto::BackendKind::Smb
            })
        );
        assert!(String::from_utf8_lossy(&bytes).contains(r#""backend":{"kind":"smb"}"#));
    }

    // --- E-017: POST /watch/event wiring (effects themselves covered in watch.rs) ---

    fn fixed_mtime() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-07-21T09:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[tokio::test]
    async fn watch_event_changed_invalidates_index() {
        let st = AppState::new(db::test_pool().await);
        let path = CanonicalPath::new_unchecked("\\\\srv\\share\\doc.md");
        crate::index::refresh(&st, &path, &VersionToken::hash(b"x"), fixed_mtime(), 1)
            .await
            .unwrap();

        let app = router(st.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/watch/event")
            .header("content-type", "application/json")
            .body(body_json(&serde_json::json!({"type":"changed","path": path.as_str()})))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // The composite-key entry is gone → next resolve misses.
        let resolved = crate::index::resolve(
            &st,
            ResolveRequest { path: path.clone(), mtime: fixed_mtime(), size: 1 },
        )
        .await
        .unwrap();
        assert!(resolved.cached_version.is_none(), "changed → cache miss");
    }

    #[tokio::test]
    async fn watch_event_removed_sidecar_autocloses_conflict_inferred() {
        let st = AppState::new(db::test_pool().await);
        let base = CanonicalPath::new_unchecked("\\\\srv\\share\\q3.xlsx");
        let sidecar = CanonicalPath::new_unchecked("\\\\srv\\share\\q3.conflict-x.xlsx");
        crate::conflict::register(
            &st,
            &base,
            &sidecar,
            &chapr_proto::Principal::new_unchecked("CONTOSO\\jsmith"),
            &chapr_proto::SessionId::new_unchecked("s"),
        )
        .await
        .unwrap();
        assert_eq!(crate::conflict::count_open(&st.pool, &base).await.unwrap(), 1);

        let app = router(st.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/watch/event")
            .header("content-type", "application/json")
            .body(body_json(&serde_json::json!({"type":"removed","path": sidecar.as_str()})))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        assert_eq!(crate::conflict::count_open(&st.pool, &base).await.unwrap(), 0);
        let events = crate::audit::query(&st.pool, &base).await.unwrap();
        assert!(events
            .iter()
            .any(|e| e.kind == chapr_proto::AuditKind::ConflictResolve));
    }

    #[tokio::test]
    async fn watch_event_overflow_rescans_index() {
        let st = AppState::new(db::test_pool().await);
        let a = CanonicalPath::new_unchecked("\\\\srv\\share\\a.md");
        let b = CanonicalPath::new_unchecked("\\\\srv\\share\\b.md");
        crate::index::refresh(&st, &a, &VersionToken::hash(b"a"), fixed_mtime(), 1)
            .await
            .unwrap();
        crate::index::refresh(&st, &b, &VersionToken::hash(b"b"), fixed_mtime(), 1)
            .await
            .unwrap();

        let app = router(st.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/watch/event")
            .header("content-type", "application/json")
            .body(body_json(&serde_json::json!({"type":"overflow"})))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        for p in [&a, &b] {
            let r = crate::index::resolve(
                &st,
                ResolveRequest { path: p.clone(), mtime: fixed_mtime(), size: 1 },
            )
            .await
            .unwrap();
            assert!(r.cached_version.is_none(), "overflow → whole index cleared");
        }
    }

    #[tokio::test]
    async fn journal_open_then_resolve_live_then_clear_clean() {
        let app = router(AppState::new(db::test_pool().await));
        let path = "\\\\srv\\share\\wip.md";
        let resolve_body = body_json(&serde_json::json!({
            "path": path, "mtime": "2026-07-21T09:00:00Z", "size": 5
        }));

        // Acquire a live lease so the journal entry resolves Live.
        let acq = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/leases")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({
                        "principal": "CONTOSO\\jsmith", "session_id": "sess-t", "purpose": "write", "paths": [path]
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        let lease: LeaseAcquireResponse =
            serde_json::from_slice(&to_bytes(acq.into_body(), usize::MAX).await.unwrap()).unwrap();

        // Open a journal entry.
        let opened = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/journal")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({
                        "path": path,
                        "lease_id": lease.lease_id.as_str(),
                        "principal": "CONTOSO\\jsmith",
                        "pre_image_version": VersionToken::hash(b"pre").as_str()
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(opened.status(), StatusCode::NO_CONTENT);

        // resolve → Live.
        let live = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/resolve")
                    .header("content-type", "application/json")
                    .body(resolve_body)
                    .unwrap(),
            )
            .await
            .unwrap();
        let live: ResolveResponse =
            serde_json::from_slice(&to_bytes(live.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(live.journal_state, chapr_proto::JournalState::Live);

        // Clear → resolve Clean.
        let cleared = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/journal/clear")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({ "path": path })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cleared.status(), StatusCode::NO_CONTENT);

        let clean = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/resolve")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({
                        "path": path, "mtime": "2026-07-21T09:00:00Z", "size": 5
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        let clean: ResolveResponse =
            serde_json::from_slice(&to_bytes(clean.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(clean.journal_state, chapr_proto::JournalState::Clean);
    }

    #[tokio::test]
    async fn the_admin_page_is_served_and_self_contained() {
        let app = router(AppState::new(db::test_pool().await));
        let resp = app
            .oneshot(Request::builder().uri("/admin").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let html = String::from_utf8(
            to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec(),
        )
        .unwrap();
        assert!(html.contains("<!doctype html>"));
        // Load-bearing: the page must work on a network with no route out, so it
        // may not reference anything external. If someone adds a CDN font or a
        // framework tag, this fails rather than the customer's install.
        for offender in ["http://", "https://", "//unpkg", "//cdn"] {
            assert!(
                !html.contains(offender),
                "the admin page must not reference {offender}"
            );
        }
        // And it must never build DOM from data (paths and principals come off a
        // shared drive that other people write to — concept §13.2).
        assert!(
            !html.contains(".innerHTML ="),
            "the admin page must not assign innerHTML"
        );
    }

    #[tokio::test]
    async fn the_overview_reports_counts_and_what_coord_is_configured_for() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(db::test_pool().await)
            .with_blob_root(tmp.path().to_path_buf())
            .with_deployment(Some(std::path::PathBuf::from("C:/data/coord.toml")), "trusted-header");

        // One open diagnostic and one open conflict, so the tiles have something
        // to be right about.
        crate::diagnostics::record(
            &state,
            &chapr_proto::DiagnosticReport {
                code: "IO".into(),
                title: "t".into(),
                severity: chapr_proto::Severity::Error,
                path: Some(CanonicalPath::new_unchecked("\\\\srv\\share\\a.md")),
                principal: chapr_proto::Principal::new_unchecked("CONTOSO\\a"),
                host: None,
                detail: "d".into(),
                remedy: "r".into(),
                facts: Default::default(),
            },
        )
        .await
        .unwrap();
        crate::conflict::register(
            &state,
            &CanonicalPath::new_unchecked("\\\\srv\\share\\a.md"),
            &CanonicalPath::new_unchecked("\\\\srv\\share\\a.conflict.md"),
            &chapr_proto::Principal::new_unchecked("CONTOSO\\b"),
            &chapr_proto::SessionId::new_unchecked("sess-t"),
        )
        .await
        .unwrap();

        let resp = router(state)
            .oneshot(
                Request::builder()
                    .uri("/admin/overview")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();

        assert_eq!(v["diagnostics_open_errors"], 1);
        assert_eq!(v["diagnostics_open_warnings"], 0);
        assert_eq!(v["conflicts_open"], 1);
        assert!(v["db_bytes"].as_i64().unwrap() > 0, "page_count × page_size");
        assert!(v["uptime_s"].as_i64().is_some());
        // The deployment half: the first question in any support call.
        assert_eq!(v["auth_mode"], "trusted-header");
        assert_eq!(v["config_path"], "C:/data/coord.toml");
        assert!(v["version"].as_str().is_some());
    }

    #[tokio::test]
    async fn leases_query_returns_held_leases_over_http() {
        let state = AppState::new(db::test_pool().await);
        let app = router(state.clone());
        let acq = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/leases")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({
                        "principal": "CONTOSO\\jsmith", "session_id": "sess-a",
                        "purpose": "write", "paths": ["\\\\srv\\share\\a.md"]
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(acq.status(), StatusCode::OK);

        let listed = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/leases/query")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let v: serde_json::Value =
            serde_json::from_slice(&to_bytes(listed.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let rows = v.as_array().expect("an array of leases");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["principal"], "CONTOSO\\jsmith");
        assert_eq!(rows[0]["session_id"], "sess-a");
        assert_eq!(rows[0]["state"], "held");
        assert_eq!(rows[0]["paths"][0], "\\\\srv\\share\\a.md");
    }

    #[tokio::test]
    async fn audit_query_serves_both_the_per_path_and_the_fleet_wide_form() {
        let state = AppState::new(db::test_pool().await);
        for path in ["\\\\srv\\share\\a.md", "\\\\srv\\share\\b.md"] {
            crate::audit::record(
                &state,
                &chapr_proto::Principal::new_unchecked("CONTOSO\\jsmith"),
                &chapr_proto::SessionId::new_unchecked("sess-t"),
                &CanonicalPath::new_unchecked(path),
                chapr_proto::AuditKind::WriteCommit,
                None,
                None,
                "wrote",
            )
            .await
            .unwrap();
        }
        let app = router(state);

        // Body with a path: the original governance read, unchanged.
        let scoped = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/audit/query")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({"path": "\\\\srv\\share\\a.md"})))
                    .unwrap(),
            )
            .await
            .unwrap();
        let v: serde_json::Value =
            serde_json::from_slice(&to_bytes(scoped.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);

        // Body without a path: the admin view's fleet-wide list.
        let all = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/audit/query")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({"limit": 50})))
                    .unwrap(),
            )
            .await
            .unwrap();
        let v: serde_json::Value =
            serde_json::from_slice(&to_bytes(all.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_reported_diagnostic_comes_back_from_the_query() {
        let app = router(AppState::new(db::test_pool().await));
        let body = serde_json::json!({
            "code": "SHARING_VIOLATION",
            "title": "A file could not be opened exclusively",
            "severity": "error",
            "path": "\\\\srv\\share\\a.md",
            "principal": "CONTOSO\\jsmith",
            "host": "LAPTOP-04",
            "detail": "sharing violation opening \\\\srv\\share\\a.md exclusively",
            "remedy": "Exclude the share from antivirus real-time scanning.",
            "facts": { "os_error": "32" }
        });
        let posted = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/diagnostics")
                    .header("content-type", "application/json")
                    .body(body_json(&body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(posted.status(), StatusCode::OK);

        let queried = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/diagnostics/query")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(queried.status(), StatusCode::OK);
        let resp: chapr_proto::DiagnosticsResponse =
            serde_json::from_slice(&to_bytes(queried.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(resp.groups.len(), 1);
        let g = &resp.groups[0];
        assert_eq!(g.code, "SHARING_VIOLATION");
        assert_eq!(g.count, 1);
        // The two fields that make a diagnostic usable rather than merely present.
        assert!(g.remedy.contains("antivirus"));
        assert_eq!(g.facts.get("os_error").map(String::as_str), Some("32"));
        assert_eq!(g.occurrences[0].host.as_deref(), Some("LAPTOP-04"));
    }

    #[tokio::test]
    async fn a_diagnostic_is_attributed_to_the_authenticated_caller() {
        // Same rule as every other principal-bearing route (D-016). It matters here
        // because a diagnostic is read to tell one bad laptop from a fleet-wide
        // fault, so the identity must be the connection's rather than asserted.
        let state = AppState::new(db::test_pool().await)
            .with_auth(std::sync::Arc::new(crate::auth::TrustedHeaderAuth));
        let app = router(state);
        let body = serde_json::json!({
            "code": "IO", "title": "t", "severity": "error",
            "principal": "CONTOSO\\spoofed", "detail": "d", "remedy": "r"
        });
        let posted = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/diagnostics")
                    .header("content-type", "application/json")
                    .header("x-chapr-principal", "CONTOSO\\authed")
                    .body(body_json(&body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(posted.status(), StatusCode::OK);
        let g: chapr_proto::DiagnosticGroup =
            serde_json::from_slice(&to_bytes(posted.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let who: Vec<&str> = g.occurrences.iter().map(|o| o.principal.as_str()).collect();
        assert_eq!(who, vec!["CONTOSO\\authed"]);
        assert!(!who.contains(&"CONTOSO\\spoofed"));
    }

    #[tokio::test]
    async fn content_routes_require_an_identity_under_enforced_auth() {
        // D-029: `GET /blobs/{version}` serves file *content* (every write
        // snapshots its pre-image there) and `POST /history` hands out the very
        // version hashes it takes. Both were ungated, so control-plane
        // reachability implied read access to every file's history while
        // `/leases` already rejected — precisely the split I-002 warns turns into
        // a real bypass the day E-015 switches enforcement on.
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(db::test_pool().await)
            .with_blob_root(tmp.path().to_path_buf())
            .with_auth(std::sync::Arc::new(crate::auth::TrustedHeaderAuth));

        let version = VersionToken::hash(b"a pre-image nobody should be able to fetch");
        let blob = Request::builder()
            .uri(format!("/blobs/{version}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router(state.clone()).oneshot(blob).await.unwrap().status(),
            StatusCode::UNAUTHORIZED,
            "blob fetch must not be reachable without an identity"
        );

        let hist = Request::builder()
            .method("POST")
            .uri("/history")
            .header("content-type", "application/json")
            .body(body_json(&serde_json::json!({"path": "\\\\srv\\share\\a.md"})))
            .unwrap();
        assert_eq!(
            router(state).oneshot(hist).await.unwrap().status(),
            StatusCode::UNAUTHORIZED,
            "gating the bytes but not their index would be half a gate"
        );
    }

    #[tokio::test]
    async fn content_routes_serve_once_an_identity_is_present() {
        // The gate is attribution, not a new obstacle: the endpoint sends
        // `x-chapr-principal` on every request, so a real client is unaffected.
        let tmp = tempfile::tempdir().unwrap();
        let app = router(
            AppState::new(db::test_pool().await)
                .with_blob_root(tmp.path().to_path_buf())
                .with_auth(std::sync::Arc::new(crate::auth::TrustedHeaderAuth)),
        );
        let content = b"pre-image body".to_vec();
        let version = VersionToken::hash(&content);

        let put = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/blobs")
                    .header("content-type", "application/octet-stream")
                    .header("x-chapr-principal", "CONTOSO\\jsmith")
                    .body(Body::from(content.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);

        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/blobs/{version}"))
                    .header("x-chapr-principal", "CONTOSO\\jsmith")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        let got = to_bytes(get.into_body(), usize::MAX).await.unwrap();
        assert_eq!(got.as_ref(), content.as_slice(), "gate must not alter the bytes");

        let hist = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/history")
                    .header("content-type", "application/json")
                    .header("x-chapr-principal", "CONTOSO\\jsmith")
                    .body(body_json(&serde_json::json!({"path": "\\\\srv\\share\\a.md"})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(hist.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn blob_put_get_and_history_over_http() {
        let tmp = tempfile::tempdir().unwrap();
        let app =
            router(AppState::new(db::test_pool().await).with_blob_root(tmp.path().to_path_buf()));
        let path = "\\\\srv\\share\\report.md";
        let content = b"pre-image body".to_vec();
        let version = VersionToken::hash(&content);

        // PUT /blobs → returns the derived version.
        let put = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/blobs")
                    .header("content-type", "application/octet-stream")
                    .body(Body::from(content.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);
        let stored: PutBlobResponse =
            serde_json::from_slice(&to_bytes(put.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(stored.version, version);
        assert!(!stored.deduplicated);

        // GET /blobs/{version} → the exact bytes back.
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/blobs/{version}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        let got = to_bytes(get.into_body(), usize::MAX).await.unwrap();
        assert_eq!(got.as_ref(), content.as_slice());

        // POST /version-log → an entry appended.
        let append = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/version-log")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({
                        "path": path,
                        "blob_hash": version.as_str(),
                        "writer_principal": "CONTOSO\\jsmith",
                        "size": content.len(),
                        "event": "create"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(append.status(), StatusCode::OK);

        // POST /history → the version shows up.
        let hist = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/history")
                    .header("content-type", "application/json")
                    .body(body_json(&serde_json::json!({ "path": path })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(hist.status(), StatusCode::OK);
        let history: HistoryResponse =
            serde_json::from_slice(&to_bytes(hist.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].version, version);
        assert_eq!(history.entries[0].event, chapr_proto::VersionEvent::Create);
    }

    fn acquire_body(body_principal: &str) -> serde_json::Value {
        serde_json::json!({
            "principal": body_principal, "session_id": "s", "purpose": "write",
            "paths": ["\\\\srv\\share\\a.md"]
        })
    }

    #[tokio::test]
    async fn trusted_header_auth_overrides_the_body_principal() {
        let state = AppState::new(db::test_pool().await)
            .with_auth(std::sync::Arc::new(crate::auth::TrustedHeaderAuth));
        let app = router(state.clone());
        // Header identity differs from the (spoofed) body principal.
        let req = Request::builder()
            .method("POST")
            .uri("/leases")
            .header("content-type", "application/json")
            .header("x-chapr-principal", "CONTOSO\\authed")
            .body(body_json(&acquire_body("CONTOSO\\spoofed")))
            .unwrap();
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::OK);

        // The lease_grant is attributed to the AUTHENTICATED principal.
        let events = crate::audit::query(
            &state.pool,
            &chapr_proto::CanonicalPath::new_unchecked("\\\\srv\\share\\a.md"),
        )
        .await
        .unwrap();
        assert!(events.iter().any(|e| e.kind == chapr_proto::AuditKind::LeaseGrant
            && e.principal == chapr_proto::Principal::new_unchecked("CONTOSO\\authed")));
        assert!(!events
            .iter()
            .any(|e| e.principal == chapr_proto::Principal::new_unchecked("CONTOSO\\spoofed")));
    }

    #[tokio::test]
    async fn trusted_header_auth_rejects_a_missing_identity() {
        let state = AppState::new(db::test_pool().await)
            .with_auth(std::sync::Arc::new(crate::auth::TrustedHeaderAuth));
        let app = router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/leases")
            .header("content-type", "application/json")
            .body(body_json(&acquire_body("CONTOSO\\whoever")))
            .unwrap();
        // No X-Chapr-Principal → 401.
        assert_eq!(
            app.oneshot(req).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
}
