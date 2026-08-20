// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

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

use crate::config::{machine_hostname, Config, TlsConfig};
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
    /// The base URL laptops connect to (e.g. `http://FILESRV01:8787`).
    ///
    /// Separate from `--addr` on purpose: that binds a socket, this is what a
    /// client types. Omitted → derived from this machine's hostname and `--addr`'s
    /// port, which is right far more often than the bind address ever was.
    #[arg(long)]
    pub public_url: Option<String>,
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

impl SetupArgs {
    /// Defaults for a wizard run nobody passed flags to — i.e. a double-click.
    ///
    /// The config and data go where the service will look for them rather than
    /// into whatever directory the executable was launched from, which for a
    /// hand-delivered binary is usually a Downloads folder. This is the last thing
    /// the PowerShell wrapper contributed that the exe did not do itself.
    pub fn default_for_wizard() -> Self {
        let dir = default_data_dir();
        // SQLite wants forward slashes in its URL even on Windows.
        let url_dir = dir.display().to_string().replace('\\', "/");
        SetupArgs {
            config_out: dir.join("coord.toml"),
            db: Some(format!("sqlite:{url_dir}/coord.db?mode=rwc")),
            blobs: Some(dir.join("blobs").display().to_string()),
            ..Default::default()
        }
    }
}

/// Where a coordinator's persistent state belongs on this platform.
fn default_data_dir() -> std::path::PathBuf {
    if cfg!(windows) {
        let root = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".to_string());
        std::path::PathBuf::from(root).join("Chaperone")
    } else {
        std::path::PathBuf::from("/var/lib/chaperone")
    }
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
    cfg.public_url = args.public_url.clone();
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

    // Said up front, not discovered at the service-registration step. Reaching
    // that step means the administrator has already answered every question and
    // committed to a config, which is the worst moment to learn the shell was
    // wrong.
    if !args.no_service && !crate::host::is_elevated() {
        println!("  ! Not running as administrator, so registering the Windows service will fail.");
        println!("    Close this, right-click the executable and choose \"Run as administrator\".");
        println!("    (Or continue anyway — the config is still written, and the wizard will");
        println!("     print how to start the service by hand.)\n");
    }

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

    if let Some(parent) = args.config_out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.config_out, cfg.to_toml())?;
    println!("Wrote config → {}", args.config_out.display());

    // The admin token is created **before** hardening, for the same reason the
    // self-signed certificate is (see `generate_self_signed`): hardening restricts
    // the data directory to administrators and the service account, and a wizard
    // run without elevation then cannot write into the directory it just locked.
    // Creating it afterwards produced a coordinator whose admin page could never be
    // signed into — fail-closed, so not dangerous, but a dead install. This is the
    // second time this ordering has bitten; the rule is now "everything the wizard
    // has to create, it creates before it locks the door".
    let token = match cfg.data_dir() {
        Some(dir) => crate::admin_token::load_or_create(&dir)
            .map(|t| (t, crate::admin_token::path_in(&dir)))
            .map_err(|e| format!("could not create the admin token: {e}")),
        None => Err("no data directory (an in-memory database?), so no admin token".to_string()),
    };

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

    print!(
        "{}",
        endpoint_snippet(
            &cfg,
            token.as_ref().map(|(t, p)| (t.as_str(), p.as_path()))
        )
    );

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

    // The four questions that decide whether this install works come first, and
    // the two that used to be wrong — the advertised URL and the share — are
    // asked outright instead of being inferred or hidden behind another prompt.

    // 1. Where the socket binds. A delivered service has to be reachable, so the
    //    wizard's baseline is the wildcard rather than `Config::default()`'s
    //    loopback (which stays the `serve`/test baseline, as with `auth`).
    if cfg.addr == Config::default().addr {
        cfg.addr = "0.0.0.0:8787".to_string();
    }
    cfg.addr = Input::new()
        .with_prompt("Listen address (where the socket binds)")
        .default(cfg.addr.clone())
        .interact_text()?;

    // 2. What the laptops type. Echoed back immediately: a wrong hostname is
    //    obvious when you see the resulting URL and invisible in a config file.
    let host: String = Input::new()
        .with_prompt("Hostname the laptops will connect to")
        .default(machine_hostname().unwrap_or_else(|| "localhost".to_string()))
        .interact_text()?;
    let port = cfg.addr.rsplit(':').next().unwrap_or("8787").to_string();
    cfg.public_url = Some(format!("http://{}:{port}", host.trim()));
    println!("  → coordinator URL: {}", cfg.public_url.as_deref().unwrap_or(""));
    println!("    This is what goes on every laptop. It is deliberately not the listen");
    println!("    address above — that binds a socket and is not something a client can use.");

    // 3. The coordinated share. Asked unconditionally, because it is the value
    //    the handover has to be able to state, and it is keyed by invariant 5 —
    //    an administrator recalling it from memory is how two spellings of one
    //    share enter a rollout.
    cfg.share_unc = Some(prompt_for_share(&host)?);

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

    // 4. The watcher. `share_unc` is already known, so this no longer asks for the
    //    same path twice under two names. The two fields are genuinely different —
    //    `share_unc` is the canonical prefix coordination state is keyed by,
    //    `watch_dir` is a local path an OS thread reads — and they are usually
    //    equal, which is why one should default to the other rather than be
    //    conflated with it.
    if Confirm::new()
        .with_prompt("Enable the proactive change-watcher? (recommended if users edit files outside Chaperone)")
        .default(false)
        .interact()?
    {
        cfg.watch_dir = Some(
            Input::new()
                .with_prompt("Watch directory (a local path, or the share)")
                .default(cfg.share_unc.clone().unwrap_or_default())
                .interact_text()?,
        );
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

    // The URL was echoed as `http://` before the TLS question was asked, so it has
    // to be reconciled now. Handing out an `http://` URL for a service that only
    // speaks TLS is the same class of mistake as handing out the bind address:
    // a value that looks right and connects to nothing.
    if cfg.tls.is_some() || generate_tls {
        if let Some(url) = cfg.public_url.take() {
            cfg.public_url = Some(url.replacen("http://", "https://", 1));
            println!("  → coordinator URL is now {}", cfg.public_url.as_deref().unwrap_or(""));
        }
    }
    Ok(generate_tls)
}

/// Ask which share this coordinator fronts, offering what the machine serves.
///
/// Free text is kept as an option rather than replaced: a coordinator does not
/// have to live on the fileserver, and when it does not, `NetShareEnum` on the
/// local machine has nothing useful to say.
fn prompt_for_share(host: &str) -> Result<String, Box<dyn std::error::Error>> {
    use dialoguer::{Input, Select};

    let found = crate::shares::local_shares();
    if !found.is_empty() {
        let mut labels: Vec<String> = found
            .iter()
            .map(|s| {
                let unc = s.unc(host);
                if s.remark.is_empty() {
                    unc
                } else {
                    format!("{unc}  ({})", s.remark)
                }
            })
            .collect();
        labels.push("Type a different path…".to_string());

        let sel = Select::new()
            .with_prompt("Which share should Chaperone coordinate?")
            .items(&labels)
            .default(0)
            .interact()?;
        if sel < found.len() {
            return Ok(found[sel].unc(host));
        }
    } else {
        println!("  (no local shares found — this host may not be the fileserver)");
    }

    loop {
        let raw: String = Input::new()
            .with_prompt("Share to coordinate, as a UNC path (\\\\server\\share)")
            .interact_text()?;
        match validate_share_path(raw.trim()) {
            Ok(path) => return Ok(path),
            Err(why) => println!("  ! {why}"),
        }
    }
}

/// Check a hand-typed share path before it becomes the key everything hangs off.
///
/// Existence is checked but a missing path is **not** fatal here — the coordinator
/// legitimately may not be able to see the share itself (it does no file I/O; the
/// endpoints do, as the logged-in user). Shape is fatal, because a non-UNC value
/// cannot be the canonical prefix invariant 5 needs.
fn validate_share_path(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("a share path is needed".into());
    }
    // Single-quoted, not `{:?}` — debug-formatting a Windows path doubles every
    // backslash, so a message about a missing backslash arrives with four of them.
    if !raw.starts_with(r"\\") {
        return Err(format!(
            "'{raw}' is not a UNC path. Coordination state is keyed by the UNC form, so \
             a drive letter here would not match what the endpoints report — use \\\\server\\share."
        ));
    }
    if raw.trim_start_matches('\\').split('\\').filter(|p| !p.is_empty()).count() < 2 {
        return Err(format!("'{raw}' names a server but no share"));
    }
    if !Path::new(raw).exists() {
        println!(
            "  note: {raw} is not reachable from this machine. That can be correct — \
             coord does no file I/O; the endpoints do, as each logged-in user."
        );
    }
    Ok(raw.to_string())
}

/// Validate the host can actually run this config, before we commit to it.
fn probe(cfg: &Config) -> Result<(), String> {
    // The structural rules first, so the wizard refuses the same configurations
    // the settings API refuses. They used to be enforced only on the API path,
    // which meant the installer could write a config the admin page would not.
    cfg.validate().map_err(|e| e.to_string())?;
    let addr: SocketAddr = cfg.addr.parse().map_err(|e| format!("invalid listen address {:?}: {e}", cfg.addr))?;
    // Bind-and-drop to confirm the port is free.
    std::net::TcpListener::bind(addr).map_err(|e| format!("cannot bind {}: {e}", cfg.addr))?;
    ensure_writable_dir(&cfg.blob_root)?;
    // The database directory too, and for the same reason: it is created here so
    // SQLite's first write lands somewhere already checked, rather than failing
    // after the service has been registered and reported healthy.
    if let Some(dir) = cfg.data_dir() {
        ensure_writable_dir(&dir.display().to_string())?;
    }
    if let Some(wd) = &cfg.watch_dir {
        if !Path::new(wd).exists() {
            return Err(format!("watch directory does not exist: {wd}"));
        }
    }
    // Shape-check the share here too, not only in the interactive prompt. The
    // unattended path takes `--share-unc` straight from a flag, and a mangled value
    // (a lost backslash from whichever shell invoked us) would otherwise be written
    // to the config and become the canonical prefix everything is keyed by.
    if let Some(share) = &cfg.share_unc {
        validate_share_path(share)?;
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
    if let Some(db) = crate::config::db_file_path(&cfg.db_url) {
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
/// The handover text, built rather than printed so it can be asserted on.
///
/// The token is passed in rather than loaded here: it has to be created before the
/// data directory is locked down, which is a decision about *ordering* in
/// [`run`] and not something this formatter should be able to get wrong.
///
/// It is a returned `String` for one reason: the defect that made this whole
/// change necessary was a *wrong line in this text*, shipped and unnoticed
/// because nothing could read it back. `handover_never_advertises_a_bind_address`
/// is the test that could not exist while this function only called `println!`.
fn endpoint_snippet(cfg: &Config, token: Result<(&str, &Path), &String>) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    // Never `cfg.addr` — that is a bind address, and formatting it as a URL is
    // exactly the defect this handover once shipped.
    let base = cfg.advertised_url();

    let _ = write!(
        s,
        "\n── Where to watch this ──\n  \
         Admin page:   {base}/admin\n    \
         Overview, failures with what fixes them, conflicts, leases, audit trail,\n    \
         and the settings — including how to change the connection auth mode safely.\n  \
         Health check: {base}/healthz\n"
    );

    match token {
        Ok((token, path)) => {
            let _ = write!(
                s,
                "\n── Sign in to the admin page with this ──\n  \
                 {token}\n  \
                 Kept in {} — a directory restricted to administrators, so the file\n  \
                 itself is the safe place for it. Read it again any time.\n  \
                 It does not depend on the auth mode, which is deliberate: it is the\n  \
                 way back in if a change to the auth setting turns out to be wrong.\n",
                path.display()
            );
        }
        Err(e) => {
            let _ = write!(
                s,
                "\n  ! {e}\n    \
                 The admin page will refuse to show anything until this is fixed.\n"
            );
        }
    }

    let _ = write!(s, "\n── Give this to the users ──\n  Coordinator URL:      {base}\n");
    match cfg.share_unc.as_deref() {
        // The actual configured value, not an example of one. A handover that
        // prints `\\FILESRV\AICollab` leaves the administrator to work out what
        // their own share is called, which is the moment a guess enters the
        // rollout and two users end up naming the same file differently.
        Some(share) => {
            let _ = writeln!(s, "  Coordinated location: {share}");
        }
        None => {
            let _ = write!(
                s,
                "  Coordinated location: (not configured here — the share path, as UNC)\n    \
                 Setup did not record one, so this is the one value you have to\n    \
                 supply from memory. Re-run setup to store it.\n"
            );
        }
    }
    let _ = write!(
        s,
        "    Both are fields in the endpoint bundle's own install dialog. A mapped\n    \
         drive letter is fine — it is resolved to its UNC form, so users with\n    \
         different letters still agree on which file is which.\n  \
         Listening on {} — that is where the socket binds, not what the\n    \
         laptops type. The URL above is the one to hand out.\n  \
         Identity is auto-derived from each user's OS logon (auth mode: {}). Nothing to type.\n  \
         Backend is auto-selected per endpoint OS and confirmed against coord's announcement ({}).\n",
        cfg.addr, cfg.auth, cfg.backend
    );

    let _ = write!(
        s,
        "\n── When something breaks ──\n  \
         Whole fleet:  the admin page's Errors tab.\n  \
         One laptop:   %LOCALAPPDATA%\\Chaperone\\diagnostics.jsonl on that machine.\n    \
         That second one matters: a laptop that cannot reach the coordinator\n    \
         cannot report it to the coordinator.\n  \
         Only unexpected faults appear there. A write that lost a compare-and-swap,\n  \
         or a document someone has open in Word, is a designed outcome — look under\n  \
         Conflicts and Leases for those.\n"
    );

    let _ = write!(
        s,
        "\n── Back up together ──\n  \
         Database:   {}\n  \
         Blob store: {}\n    \
         History is only restorable if both are restored from the same moment.\n",
        cfg.db_url, cfg.blob_root
    );
    if cfg.tls.is_none() {
        let _ = write!(
            s,
            "\n  ! Serving plain HTTP. Restrict the port to the machines that need it.\n"
        );
    }
    s
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

    /// The regression the customer hit: the handover told an administrator to
    /// configure `http://127.0.0.1:8787` on every laptop, because the text was
    /// built from the bind address. Asserting on the *absence* of the bind address
    /// rather than the presence of the right URL is deliberate — it fails for any
    /// future line that reaches for `cfg.addr` to build a URL, not just this one.
    #[test]
    fn handover_never_advertises_a_bind_address() {
        let cfg = Config {
            addr: "0.0.0.0:8787".into(),
            public_url: Some("http://FILESRV01:8787".into()),
            share_unc: Some(r"\\FILESRV01\mappe$".into()),
            ..Config::default()
        };
        let path = std::path::Path::new("C:/ProgramData/Chaperone/admin-token");
        let text = endpoint_snippet(&cfg, Ok(("tok-abc", path)));

        let handout = text
            .split("── Give this to the users ──")
            .nth(1)
            .expect("the handout section exists");
        let url_line = handout
            .lines()
            .find(|l| l.contains("Coordinator URL:"))
            .expect("the URL line exists");
        assert!(
            url_line.contains("http://FILESRV01:8787"),
            "wrong URL handed out: {url_line}"
        );
        for unreachable in ["0.0.0.0:8787", "127.0.0.1"] {
            assert!(
                !url_line.contains(unreachable),
                "handed out {unreachable} as the URL: {url_line}"
            );
        }
        // The bind address still appears, but only labelled as what it is.
        assert!(text.contains("Listening on 0.0.0.0:8787"));
        // And the real share, not the generic example it used to print.
        assert!(text.contains(r"\\FILESRV01\mappe$"), "{text}");
        assert!(!text.contains("FILESRV\\AICollab"), "still printing the example share");
    }

    #[test]
    fn handover_says_so_when_no_share_was_recorded() {
        let cfg = Config::default();
        let text = endpoint_snippet(&cfg, Err(&"no data directory".to_string()));
        assert!(text.contains("not configured here"), "{text}");
        assert!(text.contains("refuse to show anything"), "{text}");
    }

    /// The exact value a shell mangled during testing: `\\srv\share` arriving as
    /// `\srv\share`. It has to fail, because it would otherwise become the
    /// canonical prefix (invariant 5) and silently disagree with what every
    /// endpoint reports for the same files.
    #[test]
    fn a_mangled_share_path_is_refused_rather_than_stored() {
        let err = validate_share_path(r"\FILESRV01\mappe$").expect_err("one backslash is not UNC");
        assert!(err.contains("not a UNC path"), "{err}");

        let cfg = Config {
            addr: "0.0.0.0:8787".into(),
            share_unc: Some(r"\FILESRV01\mappe$".into()),
            blob_root: std::env::temp_dir()
                .join("chapr-probe-share")
                .display()
                .to_string(),
            ..Config::default()
        };
        assert!(probe(&cfg).is_err(), "the unattended path must check this too");
    }

    #[test]
    fn validate_share_path_accepts_a_hidden_share_and_rejects_a_drive_letter() {
        // A trailing `$` is a hidden share, not an invalid one — the customer's
        // real share is spelled this way.
        assert_eq!(
            validate_share_path(r"\\FILESRV01\mappe$").unwrap(),
            r"\\FILESRV01\mappe$"
        );
        assert!(validate_share_path(r"D:\Shared").is_err(), "a drive letter is not canonical");
        assert!(validate_share_path(r"\\FILESRV01").is_err(), "a server with no share");
        assert!(validate_share_path("").is_err());
    }

    #[test]
    fn public_url_flag_applies() {
        let args = SetupArgs {
            public_url: Some("http://FILESRV01:8787".into()),
            ..Default::default()
        };
        assert_eq!(
            config_from_args(&args).public_url.as_deref(),
            Some("http://FILESRV01:8787")
        );
        // Absent → derived, never the bind address verbatim.
        assert!(config_from_args(&SetupArgs::default()).public_url.is_none());
    }

    /// `probe` now runs the structural rules too, so the wizard cannot write a
    /// config the admin page's settings API would reject.
    #[test]
    fn probe_refuses_what_validate_refuses() {
        let cfg = Config {
            addr: "0.0.0.0:8787".into(),
            public_url: Some("http://localhost:8787".into()),
            ..Config::default()
        };
        let err = probe(&cfg).expect_err("a loopback URL on a reachable listener must not pass");
        assert!(err.contains("laptops resolve"), "unhelpful message: {err}");
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
            crate::config::db_file_path("sqlite:C:/ProgramData/Chaperone/coord.db?mode=rwc"),
            Some(std::path::PathBuf::from("C:/ProgramData/Chaperone/coord.db"))
        );
        assert_eq!(
            crate::config::db_file_path("sqlite:///var/lib/chapr/coord.db"),
            Some(std::path::PathBuf::from("/var/lib/chapr/coord.db"))
        );
        // `:memory:` names no directory to restrict — must not be mistaken for one.
        assert_eq!(crate::config::db_file_path("sqlite::memory:"), None);
        assert_eq!(crate::config::db_file_path("postgres://host/db"), None);
    }

    #[test]
    fn hardening_refuses_shared_and_top_level_directories() {
        // POSIX-shaped cases run on BOTH platforms: `/` is a separator on Windows
        // too, so `std::path` splits these identically either way.
        //
        // Windows-shaped cases are cfg-gated, and the reason is not tidiness. On
        // Unix a backslash is an ordinary filename character, so
        // `Path::new(r"C:\ProgramData\Chaperone")` is ONE component there, not
        // three — it collapses into the depth guard and reads as unsafe. That is
        // correct behaviour for a path Linux will never be handed, but asserting
        // it cross-platform asserts a portability `std::path` does not offer.
        // (This test claimed exactly that and failed on ubuntu-latest.)
        //
        // Fixing the function instead — normalising `\` to `/` before parsing —
        // would be worse: it would misread a legitimately-named Unix directory,
        // to buy portability the hardening path never needs, since it only ever
        // sees local paths on the host it runs on.
        let mut refuse: Vec<&str> = vec!["/", "/var", "/var/lib", "/etc", "/home"];
        let mut allow: Vec<&str> = vec!["/var/lib/chapr", "/srv/chapr/blobs"];
        if cfg!(windows) {
            refuse.extend([r"C:\", r"C:\ProgramData", r"C:\Windows", r"C:\Program Files"]);
            allow.extend([r"C:\ProgramData\Chaperone", r"C:\ProgramData\Chaperone\blobs"]);
        }

        // The whole point of the guard: locking one of these down would take out
        // the machine rather than protect the coordinator.
        for bad in refuse {
            assert!(is_unsafe_to_harden(Path::new(bad)), "{bad} must be refused");
        }
        // A directory that is genuinely the coordinator's own is allowed.
        for good in allow {
            assert!(!is_unsafe_to_harden(Path::new(good)), "{good} must be allowed");
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
