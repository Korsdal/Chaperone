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
    /// Generate a self-signed certificate instead of supplying one.
    ///
    /// For a customer with no internal CA. It removes the "make a certificate by
    /// hand" step; it does **not** remove the need to trust the result on the
    /// laptops, which is the half nobody can automate away from here.
    #[arg(long)]
    pub tls_generate: bool,
    /// Host name the generated certificate is for. Defaults to this machine's
    /// name — which is what the laptops will be connecting to.
    #[arg(long)]
    pub tls_hostname: Option<String>,
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
    let mut generate_tls = args.tls_generate;
    if !args.non_interactive {
        generate_tls |= interactive_fill(&mut cfg)?;
    }

    // Before `probe`, which checks the certificate files exist — so a generated
    // pair is validated by the same check as a supplied one rather than trusted.
    if generate_tls && cfg.tls.is_none() {
        let dir = args
            .config_out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .join("tls");
        let host = args
            .tls_hostname
            .clone()
            .or_else(machine_hostname)
            .unwrap_or_else(|| "localhost".to_string());
        println!("\nGenerating a self-signed certificate for {host}…");
        cfg.tls = Some(generate_self_signed(&dir, &host)?);
        println!("  cert → {}", dir.join("coord.crt").display());
        println!("  key  → {}", dir.join("coord.key").display());
        println!(
            "  valid until {} — note the date; TLS stops working that day.",
            (chrono::Utc::now() + chrono::Duration::days(CERT_VALID_DAYS))
                .format("%Y-%m-%d")
        );
        println!(
            "  ! Self-signed: install this certificate as trusted on the laptops, or they\n  \
               will refuse the connection. An internal CA is the better answer if you have one."
        );
    }

    println!("\nChecking the environment…");
    probe(&cfg).map_err(|e| format!("environment check failed: {e}"))?;
    println!("  ok");

    std::fs::write(&args.config_out, cfg.to_toml())?;
    println!("Wrote config → {}", args.config_out.display());

    // Before the service starts, so SQLite's WAL and the first blobs are created
    // inside an already-restricted directory rather than being tightened after
    // the fact (D-029).
    println!("\nRestricting the data directories…");
    let hardening_warnings = harden_data_dirs(&cfg);

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

    // Last, so it is the final thing on screen rather than scrolled away by the
    // endpoint snippet. Coord's blob store holds file content, so an unrestricted
    // data directory is a real exposure the installer must not report silently.
    if !hardening_warnings.is_empty() {
        eprintln!("\n!! Data-directory permissions need attention:");
        for w in &hardening_warnings {
            eprintln!("   - {w}");
        }
        eprintln!(
            "   Coord's blob store holds file content (every write snapshots the pre-image),\n   \
             so anyone who can read these directories can read that history."
        );
    }
    Ok(())
}

fn interactive_fill(cfg: &mut Config) -> Result<bool, Box<dyn std::error::Error>> {
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

    let mut generate_tls = false;
    if Confirm::new().with_prompt("Enable TLS (HTTPS)?").default(true).interact()? {
        // Asking rather than assuming: a customer with an internal CA should use
        // it, and a self-signed certificate still has to be trusted on every
        // laptop — which is work the wizard cannot do for them.
        if Confirm::new()
            .with_prompt("Generate a self-signed certificate? (No = supply your own from your CA)")
            .default(true)
            .interact()?
        {
            generate_tls = true;
        } else {
            let cert_path: String = Input::new().with_prompt("TLS certificate path (PEM)").interact_text()?;
            let key_path: String = Input::new().with_prompt("TLS private key path (PEM)").interact_text()?;
            cfg.tls = Some(TlsConfig { cert_path, key_path });
        }
    } else {
        println!("  ! warning: serving plaintext HTTP — not for production.");
    }
    Ok(generate_tls)
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

// ---- self-signed TLS (E-016 residual) ------------------------------------

/// How long a generated certificate is valid.
///
/// Five years: long enough that a pilot and its successor do not trip over it,
/// short enough to be an honest date rather than rcgen's default of the year 4096.
/// The expiry is printed at generation so it is a known date rather than a
/// surprise TLS failure years later.
const CERT_VALID_DAYS: i64 = 5 * 365;

/// This machine's name — what the laptops will actually be connecting to.
fn machine_hostname() -> Option<String> {
    let key = if cfg!(windows) { "COMPUTERNAME" } else { "HOSTNAME" };
    std::env::var(key).ok().filter(|h| !h.is_empty()).or_else(|| {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|h| !h.is_empty())
    })
}

/// Write a self-signed cert/key pair into `dir` and return the paths.
///
/// Deliberately does **not** restrict the directory itself — [`harden_data_dirs`]
/// does that, together with the database and blob store, and it runs *after*
/// `probe` has validated the files. Locking the directory here instead locked the
/// wizard out of the pair it had just written, so `probe` then reported the
/// certificate as missing: a confusing failure with a correct-looking cause.
///
/// The private key is therefore unrestricted for the few seconds between being
/// written and being locked down, inside one wizard run on the administrator's own
/// machine. That is the right trade against validating a key nobody can read.
fn generate_self_signed(dir: &Path, hostname: &str) -> Result<TlsConfig, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;

    // localhost and the loopback address are included so the wizard's own
    // `/healthz` self-test and any on-box check work against the same cert.
    let sans = vec![
        hostname.to_string(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ];
    // Built by hand rather than via `generate_simple_self_signed`, which leaves
    // rcgen's defaults in place: a subject of "rcgen self signed cert" and a
    // validity of 1975–4096. Neither survives an administrator looking at the
    // certificate, and a millennium-long validity is the kind of thing a client
    // policy rejects for good reason.
    let mut params = rcgen::CertificateParams::new(sans)
        .map_err(|e| format!("building certificate parameters: {e}"))?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, hostname);
    use chrono::Datelike;
    let today = chrono::Utc::now().date_naive();
    // A day of slack backwards absorbs clock skew between this host and a laptop.
    let start = today - chrono::Duration::days(1);
    let end = today + chrono::Duration::days(CERT_VALID_DAYS);
    let ymd = |d: chrono::NaiveDate| {
        rcgen::date_time_ymd(d.year(), d.month() as u8, d.day() as u8)
    };
    params.not_before = ymd(start);
    params.not_after = ymd(end);

    let key_pair = rcgen::KeyPair::generate().map_err(|e| format!("generating a key: {e}"))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| format!("self-signing the certificate: {e}"))?;
    let key = rcgen::CertifiedKey { cert, key_pair };

    let cert_path = dir.join("coord.crt");
    let key_path = dir.join("coord.key");
    std::fs::write(&cert_path, key.cert.pem())
        .map_err(|e| format!("writing {}: {e}", cert_path.display()))?;
    std::fs::write(&key_path, key.key_pair.serialize_pem())
        .map_err(|e| format!("writing {}: {e}", key_path.display()))?;

    Ok(TlsConfig {
        cert_path: cert_path.display().to_string(),
        key_path: key_path.display().to_string(),
    })
}

// ---- data-directory hardening (D-029) ------------------------------------

/// The on-disk SQLite file a `db_url` names, if it names one.
///
/// `sqlite:C:/data/coord.db?mode=rwc` → the path; `sqlite::memory:` → `None`.
fn db_file_path(db_url: &str) -> Option<std::path::PathBuf> {
    let rest = db_url.strip_prefix("sqlite:")?;
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let rest = rest.split('?').next()?;
    // A leading ':' is a SQLite pseudo-target (`:memory:`), not a path.
    if rest.is_empty() || rest.starts_with(':') {
        return None;
    }
    Some(std::path::PathBuf::from(rest))
}

/// Directories we refuse to touch: hardening one of these locks down the machine
/// rather than the coordinator.
///
/// Two independent guards, because either alone is too weak. The depth rule
/// (at least two named components) rejects a bare volume or `/var`; the name list
/// rejects a shared system directory that happens to be deep enough. `C:\
/// ProgramData` is refused, `C:\ProgramData\Chaperone` is allowed.
fn is_unsafe_to_harden(p: &Path) -> bool {
    use std::path::Component;
    let named: Vec<String> = p
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    if named.len() < 2 {
        return true;
    }
    // Shared locations nobody should hand exclusively to one service.
    const SHARED: &[&[&str]] = &[
        &["windows"],
        &["users"],
        &["program files"],
        &["program files (x86)"],
        &["programdata"],
        &["etc"],
        &["usr"],
        &["var"],
        &["var", "lib"],
        &["var", "log"],
        &["home"],
        &["opt"],
        &["srv"],
        &["tmp"],
    ];
    SHARED.iter().any(|s| named == *s)
}

/// Restrict the coordinator's data directories to administrators + the service
/// account, and report what could not be done.
///
/// Since D-026 the blob store holds real file **content** — every write snapshots
/// the pre-image it replaces — and on this deployment shape coord runs *on the
/// fileserver*, so without this the share's own users can read and delete the
/// history of every file straight from Explorer. `GET /blobs/{version}` has its
/// own gate; this is the filesystem half, and the two are genuinely different
/// exposures (D-029).
///
/// Applied by the installer, not at every start: it is a one-time deployment
/// fact, and re-asserting ACLs on each boot would silently revert a deliberate
/// change by an administrator.
///
/// Windows goes through `icacls` with **well-known SIDs** rather than account
/// names — `S-1-5-32-544` (Administrators), `S-1-5-18` (LocalSystem) — because
/// the display names are localized and a Danish server has "Administratorer".
/// Shelling out keeps coord's core free of the Win32 security APIs, in keeping
/// with it having no Windows primitives outside the cfg-gated watcher and SCM
/// integration.
fn harden_data_dirs(cfg: &Config) -> Vec<String> {
    let mut targets: Vec<std::path::PathBuf> = vec![std::path::PathBuf::from(&cfg.blob_root)];
    // The TLS directory belongs here for the same reason: a private key readable
    // by every user on the fileserver makes the certificate pointless.
    if let Some(tls) = &cfg.tls {
        if let Some(parent) = Path::new(&tls.key_path).parent() {
            if !parent.as_os_str().is_empty() {
                targets.push(parent.to_path_buf());
            }
        }
    }
    if let Some(db) = db_file_path(&cfg.db_url) {
        if let Some(parent) = db.parent() {
            // The WAL and SHM siblings are created fresh by SQLite and inherit
            // from the directory, not from the .db file — so the directory is
            // the only thing worth restricting.
            if !parent.as_os_str().is_empty() {
                targets.push(parent.to_path_buf());
            }
        }
    }

    let mut warnings = Vec::new();
    let mut done: Vec<std::path::PathBuf> = Vec::new();
    for dir in targets {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if done.contains(&canonical) {
            continue;
        }
        done.push(canonical.clone());

        if is_unsafe_to_harden(&canonical) {
            warnings.push(format!(
                "did NOT restrict {} — it is a shared or top-level directory. \
                 Put coord's database and blob store in their own directory \
                 (e.g. {}) and re-run setup, or restrict it by hand.",
                canonical.display(),
                if cfg!(windows) { "C:\\ProgramData\\Chaperone" } else { "/var/lib/chapr" }
            ));
            continue;
        }
        if let Err(e) = restrict_dir(&canonical) {
            warnings.push(format!("could not restrict {}: {e}", canonical.display()));
        } else {
            println!("  restricted {} to administrators + the service account", canonical.display());
        }
    }
    warnings
}

#[cfg(windows)]
fn restrict_dir(dir: &Path) -> Result<(), String> {
    // /inheritance:r drops inherited ACEs (otherwise Users keeps whatever
    // ProgramData grants); /grant:r replaces rather than adds. (OI)(CI)F =
    // object+container inherit, full control, so new blobs and the WAL inherit.
    let out = std::process::Command::new("icacls")
        .arg(dir)
        .args([
            "/inheritance:r",
            "/grant:r",
            "*S-1-5-32-544:(OI)(CI)F",
            "/grant:r",
            "*S-1-5-18:(OI)(CI)F",
        ])
        .output()
        .map_err(|e| format!("running icacls: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "icacls exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(unix)]
fn restrict_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    // 0750: owner (root, which setup runs as to write the unit file) full,
    // group read+traverse, world nothing. The Linux counterpart of the Windows
    // ACL above — same intent, native mechanism.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o750))
        .map_err(|e| format!("setting mode 0750: {e}"))
}

#[cfg(not(any(windows, unix)))]
fn restrict_dir(_dir: &Path) -> Result<(), String> {
    Err("no directory-restriction mechanism is implemented for this platform".into())
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

/// The handover: everything the person who ran setup has to know or pass on.
///
/// Written as one block on purpose. The failure mode this replaces is not a
/// missing feature — it is an installer that finishes successfully and leaves the
/// administrator with no idea that an admin page exists, what to give the users,
/// or where to look when a laptop misbehaves. Each line below is something that
/// was previously only discoverable by reading source or docs.
fn print_endpoint_snippet(cfg: &Config) {
    let scheme = if cfg.tls.is_some() { "https" } else { "http" };
    let base = format!("{scheme}://{}", cfg.addr);

    println!("\n── Where to watch this ──");
    println!("  Admin page:   {base}/admin");
    println!("    Overview, failures with what fixes them, conflicts, leases, audit trail.");
    println!("    Read-only, and it changes nothing.");
    println!("  Health check: {base}/healthz");

    println!("\n── Give this to the users ──");
    println!("  Coordinator URL:      {base}");
    println!("  Coordinated location: the share path, as UNC (e.g. \\\\FILESRV\\AICollab)");
    println!("    Both are fields in the endpoint bundle's own install dialog. A mapped");
    println!("    drive letter is fine — it is resolved to its UNC form, so users with");
    println!("    different letters still agree on which file is which.");
    println!(
        "  Identity is auto-derived from each user's OS logon (auth mode: {}). Nothing to type.",
        cfg.auth
    );
    println!(
        "  Backend is auto-selected per endpoint OS and confirmed against coord's announcement ({}).",
        cfg.backend
    );

    println!("\n── When something breaks ──");
    println!("  Whole fleet:  the admin page's Errors tab.");
    println!("  One laptop:   %LOCALAPPDATA%\\Chaperone\\diagnostics.jsonl on that machine.");
    println!("    That second one matters: a laptop that cannot reach the coordinator");
    println!("    cannot report it to the coordinator.");
    println!("  Only unexpected faults appear there. A write that lost a compare-and-swap,");
    println!("  or a document someone has open in Word, is a designed outcome — look under");
    println!("  Conflicts and Leases for those.");

    println!("\n── Back up together ──");
    println!("  Database:   {}", cfg.db_url);
    println!("  Blob store: {}", cfg.blob_root);
    println!("    History is only restorable if both are restored from the same moment.");
    if cfg.tls.is_none() {
        println!("\n  ! Serving plain HTTP. Restrict the port to the machines that need it.");
    }
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

    #[test]
    fn generate_self_signed_writes_a_usable_pem_pair() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("chapr").join("tls");
        let tls = generate_self_signed(&dir, "coord-01.example.com").unwrap();

        let cert = std::fs::read_to_string(&tls.cert_path).unwrap();
        let key = std::fs::read_to_string(&tls.key_path).unwrap();
        assert!(cert.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(key.contains("PRIVATE KEY-----"));
        // `probe` is what the wizard uses to validate a supplied certificate, so a
        // generated one has to satisfy the same check rather than be trusted.
        let cfg = Config {
            addr: "127.0.0.1:0".into(),
            blob_root: tmp.path().join("blobs").display().to_string(),
            tls: Some(tls),
            ..Config::default()
        };
        probe(&cfg).expect("a generated pair must pass the same check as a supplied one");
    }

    #[test]
    fn db_file_path_finds_the_sqlite_file_and_ignores_pseudo_targets() {
        assert_eq!(
            db_file_path("sqlite:C:/ProgramData/Chaperone/coord.db?mode=rwc"),
            Some(std::path::PathBuf::from("C:/ProgramData/Chaperone/coord.db"))
        );
        assert_eq!(
            db_file_path("sqlite:///var/lib/chapr/coord.db"),
            Some(std::path::PathBuf::from("/var/lib/chapr/coord.db"))
        );
        // `:memory:` names no directory to restrict — must not be mistaken for one.
        assert_eq!(db_file_path("sqlite::memory:"), None);
        assert_eq!(db_file_path("postgres://host/db"), None);
    }

    #[test]
    fn hardening_refuses_shared_and_top_level_directories() {
        // The whole point of the guard: locking one of these down would take out
        // the machine rather than protect the coordinator.
        for bad in [
            "C:\\",
            "C:\\ProgramData",
            "C:\\Windows",
            "C:\\Program Files",
            "/",
            "/var",
            "/var/lib",
            "/etc",
            "/home",
        ] {
            assert!(
                is_unsafe_to_harden(Path::new(bad)),
                "{bad} must be refused"
            );
        }
        // A directory that is genuinely the coordinator's own is allowed.
        for good in [
            "C:\\ProgramData\\Chaperone",
            "C:\\ProgramData\\Chaperone\\blobs",
            "/var/lib/chapr",
            "/srv/chapr/blobs",
        ] {
            assert!(
                !is_unsafe_to_harden(Path::new(good)),
                "{good} must be allowed"
            );
        }
    }

    #[test]
    fn harden_data_dirs_reports_a_refusal_instead_of_acting() {
        // A config pointing the blob store at a shared directory must come back
        // as a warning, not a silent no-op — an unrestricted blob store is a real
        // exposure now that it holds file content (D-026).
        let cfg = Config {
            blob_root: if cfg!(windows) { "C:\\ProgramData".into() } else { "/var/lib".into() },
            db_url: "sqlite::memory:".into(),
            ..Config::default()
        };
        let warnings = harden_data_dirs(&cfg);
        assert_eq!(warnings.len(), 1, "one refusal expected, got {warnings:?}");
        assert!(warnings[0].contains("did NOT restrict"));
    }

    #[test]
    fn harden_data_dirs_covers_both_the_blob_store_and_the_database_directory() {
        // Both are exposures and both must be considered: the blob store holds
        // pre-image content, and the database directory holds the WAL, which
        // carries recent transaction data. Asserted through the refusal path so
        // no real ACL is touched — the platform mechanism itself is verified by
        // hand against a live directory, since running it here would strip the
        // test process's own access and leave an undeletable temp directory.
        let cfg = Config {
            blob_root: "/var/lib".into(),
            db_url: "sqlite:/etc/coord.db".into(),
            ..Config::default()
        };
        let warnings = harden_data_dirs(&cfg);
        assert_eq!(
            warnings.len(),
            2,
            "expected a refusal for the blob root and one for the db directory, got {warnings:?}"
        );
    }
}
