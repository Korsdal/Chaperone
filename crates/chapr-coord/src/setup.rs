//! `chapr-coord setup` — the install wizard (E-016).
//!
//! Runs interactively (dialoguer prompts) or unattended (flags / `--config`,
//! for fleet rollout). Flow: build config → probe the host (bind test, writable
//! dirs, watch dir, TLS files) → write the TOML → install+start a native service
//! (systemd unit on Linux, Windows Service via SCM) → best-effort `/healthz`
//! self-test → print the endpoint config snippet.
//!
//! Pure pieces (`config_from_args`, `systemd_unit`, `probe`) are unit-tested;
//! the interactive prompting is a thin layer on top.

use crate::config::{Config, TlsConfig};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// Wizard arguments (unattended mode / defaults for interactive).
#[derive(clap::Args, Debug, Default)]
pub struct SetupArgs {
    /// Where to write the generated config file.
    #[arg(long, value_name = "FILE", default_value = "coord.toml")]
    pub config_out: std::path::PathBuf,
    /// Skip all prompts; use flags + defaults (for fleet/unattended rollout).
    #[arg(long)]
    pub non_interactive: bool,
    /// Write config only; do not install or start a service.
    #[arg(long)]
    pub no_service: bool,
    #[arg(long)]
    pub addr: Option<String>,
    #[arg(long)]
    pub db: Option<String>,
    #[arg(long)]
    pub blobs: Option<String>,
    #[arg(long)]
    pub auth: Option<String>,
    #[arg(long)]
    pub watch_dir: Option<String>,
    #[arg(long)]
    pub share_unc: Option<String>,
    #[arg(long)]
    pub tls_cert: Option<String>,
    #[arg(long)]
    pub tls_key: Option<String>,
    /// Fileserver backend kind this coord fronts (§14). Only `smb` today.
    #[arg(long)]
    pub backend: Option<String>,
}

/// Build a config from defaults + any provided flags (the unattended baseline).
fn config_from_args(args: &SetupArgs) -> Config {
    // Wizard baseline: trusted-header is the MVP identity mode (D-024) — the
    // endpoint auto-derives the logged-in user and coord stamps it into the audit
    // trail. `--auth` overrides (e.g. `disabled` for a bare loopback demo,
    // `negotiate`/`oidc` for the enforced hardening paths, E-015).
    let mut cfg = Config {
        auth: "trusted-header".to_string(),
        ..Config::default()
    };
    if let Some(v) = &args.addr {
        cfg.addr = v.clone();
    }
    if let Some(v) = &args.db {
        cfg.db_url = v.clone();
    }
    if let Some(v) = &args.blobs {
        cfg.blob_root = v.clone();
    }
    if let Some(v) = &args.auth {
        cfg.auth = v.clone();
    }
    cfg.watch_dir = args.watch_dir.clone();
    cfg.share_unc = args.share_unc.clone();
    if let (Some(cert_path), Some(key_path)) = (&args.tls_cert, &args.tls_key) {
        cfg.tls = Some(TlsConfig {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
        });
    }
    if let Some(k) = args.backend.as_ref().and_then(|v| v.parse().ok()) {
        cfg.backend = k;
    }
    cfg
}

pub async fn run(args: SetupArgs) -> Result<(), Box<dyn std::error::Error>> {
    println!("── Chaperone coordination service — setup ──\n");
    let mut cfg = config_from_args(&args);
    if !args.non_interactive {
        interactive_fill(&mut cfg)?;
    }

    println!("\nChecking the environment…");
    probe(&cfg).map_err(|e| format!("environment check failed: {e}"))?;
    println!("  ok");

    std::fs::write(&args.config_out, cfg.to_toml())?;
    println!("Wrote config → {}", args.config_out.display());

    if args.no_service {
        print_manual_start(&args.config_out);
    } else {
        match install_service(&args.config_out) {
            Ok(()) => {
                println!("Service installed and started.");
                best_effort_self_test(&cfg);
            }
            Err(e) => {
                eprintln!("Service install skipped: {e}");
                print_manual_start(&args.config_out);
            }
        }
    }

    print_endpoint_snippet(&cfg);
    Ok(())
}

fn interactive_fill(cfg: &mut Config) -> Result<(), Box<dyn std::error::Error>> {
    use dialoguer::{Confirm, Input, Select};

    cfg.addr = Input::new()
        .with_prompt("Listen address (host:port reachable by the laptops)")
        .default(cfg.addr.clone())
        .interact_text()?;
    cfg.db_url = Input::new()
        .with_prompt("SQLite URL")
        .default(cfg.db_url.clone())
        .interact_text()?;
    cfg.blob_root = Input::new()
        .with_prompt("Blob-store directory (a separate persistent mount is recommended)")
        .default(cfg.blob_root.clone())
        .interact_text()?;

    // Default to trusted-header — the MVP identity mode (D-024): the endpoint
    // auto-derives the logged-in user, coord stamps it. Zero end-user setup.
    let auth_opts = ["disabled", "trusted-header", "negotiate"];
    let sel = Select::new()
        .with_prompt("Connection auth")
        .items(&auth_opts)
        .default(1)
        .interact()?;
    cfg.auth = auth_opts[sel].to_string();
    match cfg.auth.as_str() {
        "disabled" => println!("  ! warning: no connection auth — development / trusted-LAN only."),
        "trusted-header" => println!("  ✓ MVP mode: endpoint asserts the logged-in OS identity (accountability, not spoof-proof — concept §13.1)."),
        "negotiate" => println!("  ! note: enforced Negotiate/OIDC is deferred (E-015, D-024); use trusted-header for the MVP."),
        _ => {}
    }

    // Backend kind (§14). One option today; the prompt is the seam for the
    // roadmap's POSIX/S3/Azure adapters. Coord only announces the kind — the
    // per-backend I/O lives in the endpoint (MCPB), selected at runtime.
    let backend_opts = ["smb", "posix"];
    let bsel = Select::new()
        .with_prompt("Fileserver backend (coord announces this; endpoints confirm it against their own capabilities)")
        .items(&backend_opts)
        .default(0)
        .interact()?;
    cfg.backend = backend_opts[bsel].parse().unwrap_or_default();

    if Confirm::new()
        .with_prompt("Enable the proactive change-watcher? (recommended if users edit files outside Chaperone)")
        .default(false)
        .interact()?
    {
        let wd: String = Input::new()
            .with_prompt("Watch directory (the share path)")
            .interact_text()?;
        cfg.share_unc = Some(
            Input::new()
                .with_prompt("Canonical share UNC prefix")
                .default(wd.clone())
                .interact_text()?,
        );
        cfg.watch_dir = Some(wd);
        if !cfg!(windows) {
            println!("  ! note: the built-in watcher is Windows-only; on Linux use a push sidecar (roadmap Option 4).");
        }
    }

    if Confirm::new().with_prompt("Enable TLS (HTTPS)?").default(true).interact()? {
        let cert_path: String = Input::new().with_prompt("TLS certificate path (PEM)").interact_text()?;
        let key_path: String = Input::new().with_prompt("TLS private key path (PEM)").interact_text()?;
        cfg.tls = Some(TlsConfig { cert_path, key_path });
    } else {
        println!("  ! warning: serving plaintext HTTP — not for production.");
    }
    Ok(())
}

/// Validate the host can actually run this config, before we commit to it.
fn probe(cfg: &Config) -> Result<(), String> {
    let addr: SocketAddr = cfg.addr.parse().map_err(|e| format!("invalid listen address {:?}: {e}", cfg.addr))?;
    // Bind-and-drop to confirm the port is free.
    std::net::TcpListener::bind(addr).map_err(|e| format!("cannot bind {}: {e}", cfg.addr))?;
    ensure_writable_dir(&cfg.blob_root)?;
    if let Some(wd) = &cfg.watch_dir {
        if !Path::new(wd).exists() {
            return Err(format!("watch directory does not exist: {wd}"));
        }
    }
    if let Some(tls) = &cfg.tls {
        if !Path::new(&tls.cert_path).exists() {
            return Err(format!("TLS certificate not found: {}", tls.cert_path));
        }
        if !Path::new(&tls.key_path).exists() {
            return Err(format!("TLS key not found: {}", tls.key_path));
        }
    }
    Ok(())
}

fn ensure_writable_dir(dir: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {dir}: {e}"))?;
    let probe = Path::new(dir).join(".chapr-write-test");
    std::fs::write(&probe, b"ok").map_err(|e| format!("{dir} is not writable: {e}"))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Generate a systemd unit that runs `serve` against the written config.
/// (Used by the Linux install path + tests; unused in a non-test Windows build.)
#[allow(dead_code)]
fn systemd_unit(exe: &str, config_path: &str) -> String {
    format!(
        "[Unit]\n\
Description=Chaperone coordination service\n\
After=network-online.target\n\
Wants=network-online.target\n\n\
[Service]\n\
Type=simple\n\
ExecStart={exe} serve --config {config_path}\n\
Restart=on-failure\n\
RestartSec=2\n\n\
[Install]\n\
WantedBy=multi-user.target\n"
    )
}

#[cfg(target_os = "linux")]
fn install_service(config_path: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let config_abs = config_path
        .canonicalize()
        .unwrap_or_else(|_| config_path.to_path_buf());
    let unit = systemd_unit(&exe.display().to_string(), &config_abs.display().to_string());
    let unit_path = "/etc/systemd/system/chapr-coord.service";
    std::fs::write(unit_path, unit).map_err(|e| format!("writing {unit_path} (need root?): {e}"))?;
    run_cmd("systemctl", &["daemon-reload"])?;
    run_cmd("systemctl", &["enable", "--now", "chapr-coord"])?;
    Ok(())
}

#[cfg(windows)]
fn install_service(config_path: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    crate::service_win::install(&exe, config_path).map_err(|e| e.to_string())?;
    crate::service_win::start().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", windows)))]
fn install_service(_config_path: &Path) -> Result<(), String> {
    Err("automatic service install is only implemented for Linux (systemd) and Windows".into())
}

#[cfg(target_os = "linux")]
fn run_cmd(cmd: &str, args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new(cmd)
        .args(args)
        .status()
        .map_err(|e| format!("running {cmd}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd} {args:?} exited with {status}"))
    }
}

/// Best-effort `/healthz` probe over plain HTTP after the service starts.
fn best_effort_self_test(cfg: &Config) {
    if cfg.tls.is_some() {
        println!("Self-test: TLS enabled — verify manually at https://{}/healthz", cfg.addr);
        return;
    }
    std::thread::sleep(Duration::from_millis(500));
    match http_healthz(&cfg.addr) {
        Ok(true) => println!("Self-test: /healthz OK ✔"),
        Ok(false) => println!("Self-test: coord reachable but /healthz not OK yet — check the logs"),
        Err(e) => println!("Self-test: could not reach coord ({e}) — check the service status"),
    }
}

fn http_healthz(addr: &str) -> Result<bool, String> {
    let sa: SocketAddr = addr.parse().map_err(|e| format!("{e}"))?;
    let mut stream =
        TcpStream::connect_timeout(&sa, Duration::from_secs(2)).map_err(|e| format!("{e}"))?;
    stream
        .write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|e| format!("{e}"))?;
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);
    Ok(buf.contains("200") && buf.contains("ok"))
}

fn print_manual_start(config_path: &Path) {
    println!(
        "\nTo run coord: chapr-coord serve --config {}",
        config_path.display()
    );
}

fn print_endpoint_snippet(cfg: &Config) {
    let scheme = if cfg.tls.is_some() { "https" } else { "http" };
    println!("\n── Endpoint (MCPB) configuration ──");
    println!("Point each chapr-endpoint at coord:");
    println!("  CHAPR_COORD_URL={scheme}://{}", cfg.addr);
    println!(
        "  identity: auto-derived from the OS logon (auth mode: {}); set CHAPR_PRINCIPAL only to override.",
        cfg.auth
    );
    println!(
        "  backend: auto-selected per endpoint OS, confirmed against coord's announcement ({}).",
        cfg.backend
    );
    println!("Health check: {scheme}://{}/healthz", cfg.addr);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_args_applies_flags_and_tls() {
        let args = SetupArgs {
            addr: Some("0.0.0.0:9000".into()),
            auth: Some("trusted-header".into()),
            tls_cert: Some("c.pem".into()),
            tls_key: Some("k.pem".into()),
            ..Default::default()
        };
        let cfg = config_from_args(&args);
        assert_eq!(cfg.addr, "0.0.0.0:9000");
        assert_eq!(cfg.auth, "trusted-header");
        assert_eq!(cfg.tls.as_ref().unwrap().cert_path, "c.pem");
        assert_eq!(cfg.db_url, "sqlite:chapr-coord.db"); // default kept
    }

    #[test]
    fn tls_needs_both_flags() {
        let args = SetupArgs {
            tls_cert: Some("c.pem".into()),
            ..Default::default()
        };
        assert!(config_from_args(&args).tls.is_none());
    }

    #[test]
    fn wizard_defaults_auth_to_trusted_header() {
        // No --auth flag → the wizard baseline is trusted-header (MVP, D-024),
        // even though the bare Config default is `disabled`.
        let cfg = config_from_args(&SetupArgs::default());
        assert_eq!(cfg.auth, "trusted-header");
        assert_eq!(Config::default().auth, "disabled"); // serve/tests unaffected
    }

    #[test]
    fn backend_flag_applies_else_defaults_smb() {
        let args = SetupArgs {
            backend: Some("smb".into()),
            ..Default::default()
        };
        assert_eq!(config_from_args(&args).backend, chapr_proto::BackendKind::Smb);
        // Omitted → default; an unknown value is ignored (stays default).
        assert_eq!(config_from_args(&SetupArgs::default()).backend, chapr_proto::BackendKind::Smb);
        let bad = SetupArgs { backend: Some("s3".into()), ..Default::default() };
        assert_eq!(config_from_args(&bad).backend, chapr_proto::BackendKind::Smb);
    }

    #[test]
    fn systemd_unit_runs_serve_with_config() {
        let unit = systemd_unit("/usr/bin/chapr-coord", "/etc/chapr/coord.toml");
        assert!(unit.contains("ExecStart=/usr/bin/chapr-coord serve --config /etc/chapr/coord.toml"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn probe_passes_on_a_free_port_and_writable_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config {
            addr: "127.0.0.1:0".into(), // ephemeral → always bindable
            blob_root: tmp.path().join("blobs").display().to_string(),
            ..Config::default()
        };
        probe(&cfg).unwrap();
    }

    #[test]
    fn probe_rejects_missing_tls_files() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config {
            addr: "127.0.0.1:0".into(),
            blob_root: tmp.path().join("blobs").display().to_string(),
            tls: Some(TlsConfig {
                cert_path: "/nope/cert.pem".into(),
                key_path: "/nope/key.pem".into(),
            }),
            ..Config::default()
        };
        assert!(probe(&cfg).is_err());
    }
}
