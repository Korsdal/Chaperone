// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! chapr-endpoint binary — the stdio MCP server (one per sales laptop, a child
//! of Claude Desktop).
//!
//! Configuration (environment):
//! - `CHAPR_COORD_URL` — coord base URL. Default `http://127.0.0.1:8787`.
//! - `CHAPR_COORD_TOKEN` — the deployment's shared secret on the control channel,
//!   printed by `chapr-coord setup`. Required by a coordinator running
//!   `auth = "shared-secret"`; unused by one running `trusted-header`.
//! - `CHAPR_BACKEND`   — `smb` | `posix`. Default: SMB on Windows, POSIX else
//!   (E-019). The endpoint is local-authoritative (decision D-A); coord's
//!   announcement is only cross-checked, never overrides this.
//! - `CHAPR_PRINCIPAL` — optional override of the identity; normally the acting
//!   principal is derived automatically from the OS logon (E-023, D-024).
//! - `CHAPR_ROOT`      — comma-separated coordinated root(s), e.g.
//!   `\\FILESRV\AICollab`. Serves two purposes from one value (E-025): paths
//!   outside it are **refused** after canonicalisation (closes I-010), and it is
//!   **announced** to the model, which is what makes D-028's write-routing rule
//!   actionable. Unset means unconfined — the pre-E-025 behaviour, warned about
//!   at start-up. Set but unusable is fatal: an operator who meant to confine
//!   must not silently get an unconfined endpoint.
//! - `CHAPR_MAX_INLINE_BYTES` — cap on the rendered body of one `chapr_read`.
//!   Default [`chapr_endpoint::server::DEFAULT_MAX_INLINE_BYTES`] (1 MiB). A
//!   *context* limit — how much of a file can usefully enter the model's input
//!   window — not a round-trip limit. What a model can write back is the separate
//!   [`chapr_endpoint::server::WRITEBACK_BUDGET_BYTES`], reported per read as
//!   `writable_inline` in the envelope header rather than refusing the read.
//! - `CHAPR_DIAG_LOG`  — where unexpected failures are appended locally, as JSON
//!   lines (E-026). Defaults to `%LOCALAPPDATA%\Chaperone\diagnostics.jsonl` on
//!   Windows, `$XDG_STATE_HOME/Chaperone/diagnostics.jsonl` otherwise. This sink
//!   exists because a failure *before* coord is reachable — wrong URL, TLS
//!   mismatch, blocked port — cannot phone home, and the endpoint's stderr goes
//!   nowhere as a stdio child of Claude Desktop. Set it to an empty value to
//!   disable the local file entirely.
//! - `RUST_LOG`        — tracing filter. Default `info`.

use chapr_endpoint::backend::{default_backend_kind, make_backend};
use chapr_endpoint::identity::logged_in_principal;
use chapr_endpoint::{ChaprServer, CoordClient};
use chapr_proto::{BackendKind, SessionId};
use rmcp::transport::stdio;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // MCP speaks JSON-RPC over stdout, so every log MUST go to stderr or it
    // corrupts the protocol stream.
    init_tracing();

    // Explicit argument checks, deliberately not a clap parser: an MCP host
    // launches this binary with **no arguments** and expects an MCP server on
    // stdio. Anything that could reinterpret the no-argument case would break
    // every installed extension, so the fall-through below is left untouched and
    // each subcommand is matched by name and nothing else.
    match std::env::args().nth(1).as_deref() {
        Some("self-test") => std::process::exit(chapr_endpoint::selftest::run().await as i32),
        // `print-config <host>` emits this binary's MCP client config; see
        // `hostconfig`. Read-only, and its config goes to stdout alone so it can
        // be redirected straight into a host's config file.
        Some("print-config") => {
            let host = std::env::args().nth(2);
            std::process::exit(chapr_endpoint::hostconfig::run(host.as_deref()) as i32);
        }
        _ => {}
    }

    let coord_url =
        std::env::var("CHAPR_COORD_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".to_string());
    // Ambient OS identity — the logged-in user, reused directly (E-023, D-024).
    // No token/prompt/setup; coord runs `trusted-header` mode and stamps it.
    let principal = logged_in_principal();
    // One session per process for now (the auth/session model is deferred).
    let session_id = SessionId::new_unchecked(format!("sess-{}", std::process::id()));

    // Select the backend this endpoint drives (local-authoritative, D-A).
    let backend_kind = match std::env::var("CHAPR_BACKEND") {
        Ok(v) => v.parse::<BackendKind>().unwrap_or_else(|e| {
            tracing::warn!(%e, "invalid CHAPR_BACKEND; using OS default");
            default_backend_kind()
        }),
        Err(_) => default_backend_kind(),
    };
    let backend = make_backend(backend_kind).map_err(|e| {
        tracing::error!(%e, "cannot start with the requested backend");
        std::io::Error::new(std::io::ErrorKind::Unsupported, e)
    })?;

    // Coordinated roots (E-025). Fixed before the server is built, because both
    // the confinement check and the agent-facing announcement read them.
    let roots = coordinated_roots(backend_kind)?;
    let confined = !roots.is_empty();
    chapr_endpoint::canon::set_coordinated_roots(roots)
        .map_err(|_| std::io::Error::other("coordinated roots were already set"))?;

    tracing::info!(%coord_url, backend = %backend_kind, principal = principal.as_str(), confined, "chapr-endpoint starting");
    // Two credentials, two questions. The token proves this is one of the
    // deployment's endpoints (coord's `shared-secret` mode refuses without it);
    // the principal header says which user it is acting for, and stays asserted
    // rather than proven until E-015. Unset token is legitimate — a
    // `trusted-header` deployment does not use one — so it is not fatal here;
    // an enforcing coord answers 401 and the diagnostic says why.
    let coord_token = std::env::var("CHAPR_COORD_TOKEN").unwrap_or_default();
    if coord_token.trim().is_empty() {
        tracing::info!("no CHAPR_COORD_TOKEN set; a coordinator enforcing shared-secret auth will reject this endpoint");
    }
    let coord = CoordClient::new(&coord_url)
        .with_principal(principal.as_str())
        .with_token(coord_token);

    // Where unexpected failures land locally (E-026). Logged at start-up because a
    // support path nobody can find is not a support path.
    let diag_log = match std::env::var("CHAPR_DIAG_LOG") {
        Ok(v) if v.trim().is_empty() => None,
        Ok(v) => Some(std::path::PathBuf::from(v)),
        Err(_) => chapr_endpoint::diag::Diagnostics::default_log_path(),
    };
    match &diag_log {
        Some(p) => tracing::info!(diagnostics_log = %p.display(), "local diagnostics log"),
        None => tracing::warn!("local diagnostics log disabled; failures before coord is reachable will not be recorded anywhere"),
    }

    let diagnostics = std::sync::Arc::new(chapr_endpoint::diag::Diagnostics::new(diag_log));

    // Preflight. Not a gate — reads degrade open and work without coord, so
    // refusing to start would be the wrong direction (concept §10). This exists so
    // the cause of "writes are refused" is on screen at start-up rather than
    // discovered mid-task, and it lands in the diagnostics log too.
    let probe = std::time::Instant::now();
    match coord.healthz().await {
        Ok(()) => tracing::info!(
            %coord_url,
            ms = probe.elapsed().as_millis(),
            "coordinator reachable"
        ),
        Err(e) => {
            tracing::error!(
                %coord_url, error = %e,
                "coordinator NOT reachable — reads will work, writes will be refused. \
                 Check the coordinator service is running, that this URL is right, and that \
                 nothing between this machine and it blocks the port."
            );
            diagnostics.report(&coord, &principal, &e).await;
        }
    }

    let mut server = ChaprServer::with_diagnostics(
        coord,
        backend,
        principal,
        session_id,
        diagnostics,
    );
    if let Some(cap) = std::env::var("CHAPR_MAX_INLINE_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        tracing::info!(max_inline_bytes = cap, "inline read cap overridden");
        server = server.with_max_inline_bytes(cap);
    }

    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

/// Resolve `CHAPR_ROOT` into canonical roots (E-025).
///
/// Fail-closed on a set-but-unusable value: an operator who configured a root
/// meant to confine this endpoint, and quietly falling back to unconfined would
/// invert the one decision they made. Unset is a different case — that is a
/// deployment which has not opted in, so it warns and continues.
fn coordinated_roots(
    kind: BackendKind,
) -> Result<Vec<chapr_proto::CanonicalPath>, Box<dyn std::error::Error>> {
    let raw = match std::env::var("CHAPR_ROOT") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            tracing::warn!(
                "CHAPR_ROOT is not set — this endpoint will act on any absolute path the \
                 logged-in user can reach, bounded only by their own ACLs (I-010). Set it to \
                 the share that should be coordinated."
            );
            return Ok(Vec::new());
        }
    };

    let grammar = chapr_endpoint::grammar_for(kind);
    let mounts = chapr_endpoint::mount::default_mounts();
    let mut roots = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        // Canonicalised against an empty root set: there is nothing to confine the
        // root itself to, and a mapped drive as the root must resolve like any
        // other path so the announced boundary matches what gets enforced.
        match chapr_endpoint::canon::canonicalize_in(part, grammar, mounts, &[]) {
            Ok(p) => {
                tracing::info!(root = p.as_str(), "coordinated root");
                roots.push(p);
            }
            Err(e) => {
                tracing::error!(root = part, %e, "unusable coordinated root");
            }
        }
    }
    if roots.is_empty() {
        return Err(format!(
            "CHAPR_ROOT is set to {raw:?} but none of it resolved to a usable path; \
             refusing to start unconfined"
        )
        .into());
    }
    Ok(roots)
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
