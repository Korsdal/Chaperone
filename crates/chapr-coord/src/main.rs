// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! chapr-coord — the coordination service.
//!
//! One instance on-prem beside the fileserver; owns all coordination metadata
//! (concept §4.2). Subcommands (E-016):
//! - `serve` (default) — run the service, config from a TOML file + env.
//! - `setup` — interactive/unattended install wizard.
//! - `run-service` (Windows) — run under the Service Control Manager.
//!
//! Config precedence: defaults → TOML (`--config`) → `CHAPR_COORD_*` env vars.
//! See `config.rs` for the full surface. TLS: set `[tls]` in the config (or
//! `CHAPR_COORD_TLS_CERT`/`_KEY`) to serve HTTPS via rustls.

mod admin_token;
mod audit;
mod auth;
mod config;
mod conflict;
mod db;
mod diagnostics;
mod endpoint_token;
mod gc;
mod history;
mod host;
mod http;
mod index;
mod journal;
mod lease;
mod mv;
mod reads;
mod reaper;
mod setup;
mod shares;
mod state;
// The change-watcher *effect core* (`watch`) is platform-agnostic (E-017): it
// backs both the Windows ReadDirectoryChangesW source and the push endpoint
// `POST /watch/event`. Only the OS-specific event source (`watch_win`) and the
// SCM service integration are Windows-gated.
#[cfg(windows)]
mod service_win;
mod watch;
#[cfg(windows)]
mod watch_win;

use crate::config::Config;
use clap::{Parser, Subcommand};
use state::AppState;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "chapr-coord", about = "Chaperone coordination service")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
// A one-shot CLI dispatch enum built once at startup; the size gap between the
// small `Serve`/`RunService` variants and the flag-rich `Setup` doesn't matter,
// and clap's derive can't box a variant's `Args`.
#[allow(clippy::large_enum_variant)]
enum Cmd {
    /// Run the coordination service (default).
    Serve {
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
    },
    /// Install/configure coord: an interactive or unattended setup wizard.
    Setup(setup::SetupArgs),
    /// Run under the Windows Service Control Manager (used by the installed service).
    #[cfg(windows)]
    RunService {
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
    },
}

/// The conventional config filename — the default `setup --config-out` writes and
/// the one a bare invocation looks for beside the executable's working directory.
const DEFAULT_CONFIG: &str = "coord.toml";

/// Returns `ExitCode` rather than `Result` so errors are reported with `Display`.
///
/// Rust's default `main` error handler formats with `Debug`, which turned a
/// perfectly good message about a Windows share path into
/// `"\\\\FILESRV01\\\\mappe$"` — every backslash doubled and the whole thing
/// re-quoted. The audience for these messages is an administrator mid-install, so
/// the escaping mattered more than the one line it costs to avoid.
fn main() -> std::process::ExitCode {
    init_tracing();
    let cli = Cli::parse();
    let interactive = cli.cmd.is_none();
    let cmd = cli.cmd.unwrap_or_else(default_command);

    let result = dispatch(cmd);
    if let Err(e) = &result {
        eprintln!("\n!! {e}");
    }

    // A bare invocation is a double-click until proven otherwise, and the thing a
    // double-click does worst is fail: the window closes with the reason in it.
    // Only reached for the no-subcommand path, so scripts and the SCM are
    // untouched.
    if interactive {
        host::pause_if_own_console();
    }

    if result.is_err() {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}

fn dispatch(cmd: Cmd) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        Cmd::Serve { config } => {
            let cfg = Config::load(config.as_deref())?;
            runtime()?.block_on(run_server(cfg))
        }
        Cmd::Setup(args) => runtime()?.block_on(setup::run(args)),
        #[cfg(windows)]
        Cmd::RunService { config } => service_win::run(config),
    }
}

/// What a bare `chapr-coord` (no subcommand) should do.
///
/// It used to be `serve`, which is the wrong guess for the way this binary is
/// actually delivered: copied to a server and double-clicked. There is no config
/// yet at that point, so `serve` bound loopback with built-in defaults and looked
/// like a working coordinator that no laptop could reach — and left an
/// administrator needing a script just to reach the wizard.
///
/// `serve` still wins whenever there is a config to serve or nobody to prompt, so
/// no scripted or service invocation changes behaviour.
fn default_command() -> Cmd {
    use std::io::IsTerminal;
    let config = std::path::Path::new(DEFAULT_CONFIG);
    if config.exists() {
        return Cmd::Serve {
            config: Some(config.to_path_buf()),
        };
    }
    if std::io::stdin().is_terminal() {
        return Cmd::Setup(setup::SetupArgs::default_for_wizard());
    }
    Cmd::Serve { config: None }
}

/// Bring up all subsystems from a resolved config and serve until shutdown.
/// Shared by `serve` and (on Windows) the SCM service main.
pub(crate) async fn run_server(cfg: Config) -> Result<(), Box<dyn std::error::Error>> {
    run_server_ready(cfg, None).await
}

/// As [`run_server`], but sends on `ready` once the listener is **bound**.
///
/// I-006: the Windows service main reported RUNNING to the SCM before any of
/// this ran, so a failure to open the database, load a TLS certificate or bind
/// the port left the SCM showing a healthy service with nothing listening — and
/// the rustls provider panic (D-026 session) hit exactly that window. Ordering
/// is the whole fix: every bring-up step that can fail happens before the signal,
/// and a supervisor that never receives it knows startup failed.
///
/// A `std::sync::mpsc::Sender` rather than a `tokio::sync::oneshot`: the send is
/// non-blocking and needs no runtime, so the caller can be a plain OS thread
/// (which the SCM service main is), and coord's tokio does not carry the `sync`
/// feature.
pub(crate) async fn run_server_ready(
    cfg: Config,
    ready: Option<std::sync::mpsc::Sender<()>>,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(db_url = %cfg.db_url, "opening coordination store");
    let pool = db::connect(&cfg.db_url, 5).await?;
    db::migrate(&pool).await?;

    // The content-addressed blob store lives on coord's own volume (concept §12).
    tokio::fs::create_dir_all(&cfg.blob_root).await?;
    tracing::info!(blob_root = %cfg.blob_root, "blob store ready");

    // The admin token lives in the coordinator's own data directory, which the
    // installer already restricts to administrators and the service account
    // (D-029) — so its confidentiality is a protection that already exists. If
    // there is no data directory (an in-memory database) or it cannot be written,
    // the administrative surface fails closed rather than open.
    let admin_token = match cfg.data_dir() {
        Some(dir) => match admin_token::load_or_create(&dir) {
            Ok(t) => {
                tracing::info!(path = %admin_token::path_in(&dir).display(), "admin token ready");
                Some(t)
            }
            Err(e) => {
                tracing::error!(error = %e, dir = %dir.display(), "could not establish an admin token; the admin page will refuse to serve data");
                None
            }
        },
        None => {
            tracing::warn!("no data directory (in-memory database?); the admin page will refuse to serve data");
            None
        }
    };

    // The endpoint token — the shared secret that admits a caller to the control
    // channel at all. Same directory and same reasoning as the admin token, but a
    // different failure direction: the admin surface degrades to "unavailable",
    // whereas an *enforcing* auth mode that cannot enforce must not come up at all.
    // A coordinator that believes it is authenticating while accepting any header
    // is worse than one that is honestly open.
    let enforcing = cfg.auth == "shared-secret" || cfg.auth_fallback.as_deref() == Some("shared-secret");
    let endpoint_token = match cfg.data_dir() {
        Some(dir) => match endpoint_token::load_or_create(&dir) {
            Ok(t) => {
                tracing::info!(path = %endpoint_token::path_in(&dir).display(), "endpoint token ready");
                Some(t)
            }
            Err(e) if enforcing => {
                return Err(format!(
                    "auth mode {:?} needs the endpoint token in {}, and it could not be \
                     established: {e}\nFix the data directory's permissions, or set auth to \
                     \"trusted-header\" if this deployment is deliberately open.",
                    cfg.auth,
                    dir.display()
                )
                .into());
            }
            Err(e) => {
                tracing::warn!(error = %e, dir = %dir.display(), "could not establish an endpoint token; shared-secret auth is unavailable");
                None
            }
        },
        None if enforcing => {
            return Err("auth mode \"shared-secret\" needs a data directory to hold the \
                        endpoint token, and this coordinator has none (an in-memory \
                        database?). Point db_url at a file, or use \"trusted-header\"."
                .into());
        }
        None => None,
    };

    let mut state = AppState::new(pool)
        .with_blob_root(cfg.blob_root.as_str())
        .with_auth(auth::from_config(&cfg, endpoint_token.as_deref()))
        .with_backends(cfg.backend, cfg.backend_routes.clone())
        .with_config(cfg.clone());
    if let Some(t) = admin_token {
        state = state.with_admin_token(t);
    }
    // Held on state so the settings-reload path can rebuild the authenticator
    // without re-reading the file (http.rs `put_settings`).
    if let Some(t) = endpoint_token {
        state = state.with_endpoint_token(t);
    }
    if let Some(fallback) = &cfg.auth_fallback {
        tracing::warn!(
            primary = %cfg.auth, %fallback,
            "auth fallback active — a cutover is in progress; remove it once the primary is admitting everything"
        );
    }
    tracing::info!(auth = %cfg.auth, backend = %cfg.backend, "connection auth mode");

    // Proactive recovery scan (concept §15).
    let dangling = journal::scan_dangling(&state.pool, chrono::Utc::now().timestamp_millis()).await?;
    if dangling.is_empty() {
        tracing::info!("startup recovery scan: no dangling journal entries");
    } else {
        tracing::warn!(count = dangling.len(), "startup recovery scan: dangling journal entries pending recovery");
        for e in &dangling {
            tracing::warn!(path = %e.path, principal = %e.principal, "dangling in-flight write");
        }
    }

    reaper::spawn(state.clone(), Duration::from_secs(cfg.reap_secs));
    tracing::info!(reap_secs = cfg.reap_secs, "lease reaper started");

    gc::spawn(state.clone(), Duration::from_secs(cfg.gc_secs), gc::GcConfig::default());
    tracing::info!(gc_secs = cfg.gc_secs, "blob GC started");

    // Change-watcher (concept §14), Windows-only.
    #[cfg(windows)]
    if let Some(watch_dir) = cfg.watch_dir.clone() {
        let share_unc = cfg.share_unc.clone().unwrap_or_else(|| watch_dir.clone());
        watch_win::spawn(state.clone(), watch_dir.clone(), share_unc);
        tracing::info!(%watch_dir, "change-watcher started");
    }

    let app = http::router(state);
    let addr: SocketAddr = cfg
        .addr
        .parse()
        .map_err(|e| format!("invalid listen address {:?}: {e}", cfg.addr))?;

    match &cfg.tls {
        Some(tls) => {
            // Fail with a sentence an operator can act on, rather than a bare
            // NotFound from inside from_pem_file naming neither path nor remedy.
            tls.preflight()?;
            // A workspace build compiles rustls with BOTH `aws-lc-rs` (via
            // axum-server) and `ring` (via the endpoint's reqwest), and rustls
            // then refuses to infer a process-level provider — `ServerConfig::
            // builder()` inside from_pem_file panics. Pick one explicitly.
            // Err means a provider is already installed, which is fine.
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            let rustls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
                &tls.cert_path,
                &tls.key_path,
            )
            .await?;
            // Bind explicitly instead of `bind_rustls`, which binds lazily inside
            // `serve`: the readiness signal must not fire until the port is
            // genuinely held, or I-006 reappears for the TLS path only.
            let listener = std::net::TcpListener::bind(addr)?;
            tracing::info!(%addr, "chapr-coord listening (TLS)");
            signal_ready(ready);
            axum_server::from_tcp_rustls(listener, rustls)
                .serve(app.into_make_service())
                .await?;
        }
        None => {
            // TLS is the intended default: the wizard generates a certificate and
            // takes it unless told otherwise, and the shipped template enables it.
            // Reaching this arm means someone opted out — so say what that costs,
            // at every start rather than once at install time. A bearer token or a
            // principal header over plain HTTP is theatre, and the pre-image bytes
            // of every write cross the same wire.
            tracing::warn!(
                %addr,
                "serving the control channel over PLAINTEXT HTTP — principal headers, \
                 control-plane credentials and file pre-images all cross the network \
                 unencrypted. Run `chapr-coord setup` to generate a certificate, or set \
                 [tls] cert_path/key_path in the config."
            );
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            tracing::info!(%addr, "chapr-coord listening");
            signal_ready(ready);
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await?;
        }
    }

    tracing::info!("chapr-coord shut down cleanly");
    Ok(())
}

/// Tell a waiting supervisor the listener is up (I-006). A dropped receiver just
/// means nobody is waiting any more, which is not a reason to stop serving.
fn signal_ready(ready: Option<std::sync::mpsc::Sender<()>>) {
    if let Some(tx) = ready {
        let _ = tx.send(());
    }
}

fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread().enable_all().build()
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
}

/// Resolve on Ctrl-C so `axum::serve` can drain in-flight requests before exit.
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
