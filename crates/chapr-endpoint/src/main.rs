//! chapr-endpoint binary — the stdio MCP server (one per sales laptop, a child
//! of Claude Desktop).
//!
//! Configuration (environment):
//! - `CHAPR_COORD_URL` — coord base URL. Default `http://127.0.0.1:8787`.
//! - `CHAPR_BACKEND`   — `smb` | `posix`. Default: SMB on Windows, POSIX else
//!   (E-019). The endpoint is local-authoritative (decision D-A); coord's
//!   announcement is only cross-checked, never overrides this.
//! - `CHAPR_PRINCIPAL` — optional override of the identity; normally the acting
//!   principal is derived automatically from the OS logon (E-023, D-024).
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

    tracing::info!(%coord_url, backend = %backend_kind, principal = principal.as_str(), "chapr-endpoint starting");
    // Present our identity to coord (dev auth boundary; real deployments use
    // Negotiate/Kerberos on the transport instead of this header — I-001/I-002).
    let coord = CoordClient::new(&coord_url).with_principal(principal.as_str());
    let server = ChaprServer::new(coord, backend, principal, session_id);

    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
