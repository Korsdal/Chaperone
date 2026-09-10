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
    /// Where this coordinator keeps everything of its own: database, blob store,
    /// both tokens, TLS material.
    ///
    /// The one location an installer has to know. `--db` and `--blobs` still
    /// override individually, for the split-volume case the pilot runs (share on
    /// one volume, coordinator state on another); given neither, they default
    /// under this. Omitted entirely → the platform's conventional directory.
    #[arg(long, value_name = "DIR")]
    pub data_dir: Option<std::path::PathBuf>,
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
    /// Collect the answers in a browser instead of at the prompt.
    ///
    /// Opens a one-time, loopback-only page and then runs exactly this same
    /// wizard with the values it gathered — see [`crate::setup_ui`]. Ignored
    /// under `--non-interactive`, which has no questions to ask.
    #[arg(long)]
    pub ui: bool,
}

/// Write a config, data directory and handover for a coordinator that has none.
///
/// The service calls this before it starts, and it is what lets an installer be
/// purely declarative: place the binary, register the service with these values
/// on its command line, start it. No custom action, and nothing that can be
/// silently skipped and leave a registered service pointing at a config nobody
/// wrote - which is exactly what a deferred custom action did.
///
/// **A config that already exists is never touched**, so this runs once in a
/// deployment's life and an upgrade or a restart cannot overwrite an
/// administrator's edits.
///
/// It deliberately does NOT probe. `setup` probes because a human is watching and
/// can act on the answer; a service starting at boot has nobody to tell, and
/// refusing to start over an unreachable share would turn a warning into an
/// outage.
///
/// Windows-only in practice, and the `allow` says so rather than hiding it: the
/// caller is `run-service`, which the SCM invokes. A systemd unit cannot reach
/// this state — the Linux wizard writes the config *before* it installs the unit,
/// so a Linux coordinator always starts against a config that exists. The MSI is
/// the only installer that registers a service before one has been written.
///
/// The tests below exercise it on every platform, because the logic is pure.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn provision_if_missing(args: &SetupArgs) -> Result<bool, String> {
    if args.config_out.exists() {
        return Ok(false);
    }
    let cfg = config_from_args(args);
    cfg.validate().map_err(|e| format!("the values this service was installed with do not make a usable config: {e}"))?;

    // Before the config, because SQLite cannot create a database in a directory
    // that is not there, and because hardening an existing directory is the only
    // order that leaves no window where the tokens are world-readable.
    if let Some(dir) = cfg.data_dir() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    // The blob store too, and before hardening rather than at bring-up: it is
    // restricted the moment it exists, so there is no window in which file
    // pre-images land in a world-readable directory. Bring-up still creates it if
    // it is missing, which covers a config that points it somewhere else.
    std::fs::create_dir_all(&cfg.blob_root)
        .map_err(|e| format!("creating {}: {e}", cfg.blob_root))?;
    if let Some(parent) = args.config_out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    std::fs::write(&args.config_out, cfg.to_toml())
        .map_err(|e| format!("writing {}: {e}", args.config_out.display()))?;

    // Tokens first, then the door. `run_server_ready` creates both with
    // `load_or_create` a moment from now; doing it here as well would duplicate
    // that, so this only makes the directory they will land in restricted.
    let warnings = harden_data_dirs(&cfg);
    for w in &warnings {
        tracing::warn!("{w}");
    }
    Ok(true)
}

/// Write `handover.txt` for a coordinator that provisioned itself.
///
/// Separate from [`provision_if_missing`] because the tokens do not exist yet at
/// that point - the server mints them during bring-up - so the values every
/// laptop needs can only be stated once it is up.
pub(crate) fn write_handover_file(
    cfg: &Config,
    admin_token: Option<&str>,
    endpoint_token: Option<&str>,
) -> Option<std::path::PathBuf> {
    let dir = cfg.data_dir()?;
    let admin_path = crate::admin_token::path_in(&dir);
    let no_token = "no admin token was established; the admin page will refuse to serve data"
        .to_string();
    let admin = match admin_token {
        Some(t) => Ok((t, admin_path.as_path())),
        None => Err(&no_token),
    };
    let path = dir.join(HANDOVER_FILE);
    match std::fs::write(&path, endpoint_snippet(cfg, admin, endpoint_token)) {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not write the handover file");
            None
        }
    }
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
        SetupArgs {
            config_out: dir.join("coord.toml"),
            data_dir: Some(dir.clone()),
            // A double-click gets the browser wizard. That is the whole point of
            // W2: the person doing this is an IT administrator who was handed an
            // executable, and a terminal full of prompts is what they should not
            // have to meet. `chapr-coord setup` still gives the prompts, so the
            // rule is discoverable without a flag to turn anything off:
            // double-click → browser, named subcommand → terminal, `--ui` →
            // browser on purpose.
            ui: true,
            ..Default::default()
        }
    }
}

/// Where setup leaves the values every laptop needs, inside the data directory.
///
/// Named here rather than inline because the installer's finish dialog opens this
/// exact path, so the two must agree.
pub(crate) const HANDOVER_FILE: &str = "handover.txt";

/// The SQLite URL for a database inside `dir`.
///
/// One function, because the shape is easy to get subtly wrong and the failure is
/// silent: SQLite wants forward slashes in its URL even on Windows, and the
/// `sqlite:` prefix is what tells the rest of coord this URL names a file at all
/// (I-017).
pub(crate) fn default_db_url_in(dir: &Path) -> String {
    let url_dir = dir.display().to_string().replace('\\', "/");
    format!("sqlite:{url_dir}/coord.db?mode=rwc")
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
    // Wizard baseline: `shared-secret`. It is the weakest mode that actually
    // refuses a stranger, and a **new** install has no laptops to break, so
    // defaulting to it costs one value in the handover and closes the hole that
    // `trusted-header` leaves wide open (anyone who can reach the port acts as
    // anyone). The endpoint still auto-derives the logged-in user (D-024), so the
    // zero-config identity story is unchanged — a packager can bake the token into
    // the bundle and users type nothing.
    //
    // Deliberately does not change existing installs: their config already names a
    // mode, and switching one is a **cutover** via `auth_fallback` (D-031), never a
    // flag day. `--auth` still overrides (`disabled` for a loopback demo,
    // `trusted-header` for a deployment that wants the old posture, `negotiate` for
    // E-015).
    let mut cfg = Config {
        auth: "shared-secret".to_string(),
        ..Config::default()
    };
    if let Some(v) = set(&args.addr) {
        cfg.addr = v;
    }
    cfg.public_url = set(&args.public_url);
    // The data directory is written into the config even when `--db` and
    // `--blobs` point elsewhere, because it is what the tokens and the TLS
    // material follow. Deriving it from the database's parent is what I-017 was.
    if let Some(dir) = &args.data_dir {
        cfg.data_dir = Some(dir.display().to_string());
        cfg.db_url = default_db_url_in(dir);
        cfg.blob_root = dir.join("blobs").display().to_string();
    }
    if let Some(v) = set(&args.db) {
        cfg.db_url = v;
    }
    if let Some(v) = set(&args.blobs) {
        cfg.blob_root = v;
    }
    if let Some(v) = set(&args.auth) {
        cfg.auth = v;
    }
    cfg.watch_dir = set(&args.watch_dir);
    cfg.share_unc = set(&args.share_unc);
    if let (Some(cert_path), Some(key_path)) = (set(&args.tls_cert), set(&args.tls_key)) {
        cfg.tls = Some(TlsConfig { cert_path, key_path });
    }
    if let Some(k) = set(&args.backend).and_then(|v| v.parse().ok()) {
        cfg.backend = k;
    }
    cfg
}

/// A flag that was given a real value, as opposed to given an empty one.
///
/// The installer is why this exists. An MSI's `ServiceInstall` arguments are one
/// formatted string with no way to leave a flag out, so an unanswered field
/// arrives as `--share-unc ""`. Treating that as "the share is the empty string"
/// produced a config that validated and coordinated nothing. Empty means unset,
/// everywhere, so the same flags work from a script, a service registration and a
/// wizard.
fn set(v: &Option<String>) -> Option<String> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

pub async fn run(args: SetupArgs) -> Result<Applied, Box<dyn std::error::Error>> {
    // No `--ui` branch here, deliberately. The browser front end calls *this*
    // function, so a redirect the other way would make the two mutually
    // recursive — and a recursive `async fn` whose other arm owns an HTTP server
    // has a future that can never be `Send`, which stops the wizard's own handler
    // from being a valid axum handler. The front end is chosen in `main`, which
    // is where a choice between front ends belongs.
    println!("-- Chaperone coordination service - setup --\n");

    // Said up front, not discovered at the service-registration step. Reaching
    // that step means the administrator has already answered every question and
    // committed to a config, which is the worst moment to learn the shell was
    // wrong.
    if !args.no_service && !crate::host::is_elevated() {
        println!("  ! Not running as administrator, so registering the Windows service will fail.");
        println!("    Close this, right-click the executable and choose \"Run as administrator\".");
        println!("    (Or continue anyway - the config is still written, and the wizard will");
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
        // Under the data directory, which is the fourth location I-017's fix
        // collapses: the certificate and key are coordinator state like the
        // tokens, and putting them beside `config_out` meant a config written to
        // one place and its key material written to another whenever those
        // differed. Falls back to the config's own directory for a config that
        // names no data directory.
        let dir = cfg
            .data_dir()
            .unwrap_or_else(|| {
                args.config_out
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."))
                    .to_path_buf()
            })
            .join("tls");
        let host = args
            .tls_hostname
            .clone()
            .or_else(machine_hostname)
            .unwrap_or_else(|| "localhost".to_string());
        println!("\nGenerating a self-signed certificate for {host}...");
        cfg.tls = Some(generate_self_signed(&dir, &host)?);
        println!("  cert -> {}", dir.join("coord.crt").display());
        println!("  key  -> {}", dir.join("coord.key").display());
        println!(
            "  valid until {} - note the date; TLS stops working that day.",
            (chrono::Utc::now() + chrono::Duration::days(CERT_VALID_DAYS))
                .format("%Y-%m-%d")
        );
        println!(
            "  ! Self-signed: install this certificate as trusted on the laptops, or they\n  \
               will refuse the connection. An internal CA is the better answer if you have one."
        );
    }

    // After every path that can turn TLS on — interactive answer or
    // `--tls-generate` — and before `probe`, which is where `validate` refuses a
    // scheme that contradicts the TLS setting.
    if let Some(url) = reconcile_public_url_scheme(&mut cfg) {
        println!("  -> coordinator URL is now {url}");
    }

    println!("\nChecking the environment...");
    probe(&cfg).map_err(|e| format!("environment check failed: {e}"))?;
    println!("  ok");

    if let Some(parent) = args.config_out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.config_out, cfg.to_toml())?;
    println!("Wrote config -> {}", args.config_out.display());

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

    // The endpoint token, created here for exactly the reason above: before the
    // door is locked. It is also the value every laptop needs, so a wizard that
    // failed to create it has produced an install nobody can connect to — which is
    // why the handover states it rather than leaving it to be discovered.
    let endpoint_token = match cfg.data_dir() {
        Some(dir) => crate::endpoint_token::load_or_create(&dir).ok(),
        None => None,
    };

    // Built here and printed at the end, so it can also be written to disk before
    // the door is locked - the same rule the two tokens follow.
    //
    // The file is what makes an install that nobody watched still usable. Setup's
    // stdout is the only place these values appear, and an installer runs it with
    // no console at all: the MSI's custom action is a SYSTEM process, so the URL
    // and both tokens were computed and discarded. It costs no new exposure -
    // the same directory already holds `admin-token` and `endpoint-token` as
    // plaintext, restricted to administrators and the service account.
    let snippet = endpoint_snippet(
        &cfg,
        token.as_ref().map(|(t, p)| (t.as_str(), p.as_path())),
        endpoint_token.as_deref(),
    );
    let handover_file = cfg.data_dir().map(|d| d.join(HANDOVER_FILE));
    if let Some(path) = &handover_file {
        match std::fs::write(path, &snippet) {
            Ok(()) => println!("\nConnection details -> {}", path.display()),
            // Not fatal: the same text is on stdout, and `chapr-coord handover`
            // reprints it. Losing the file is an inconvenience, not a broken
            // install, and refusing here would fail an otherwise good setup.
            Err(e) => eprintln!("\n  ! could not write {}: {e}", path.display()),
        }
    }

    // Before the service starts, so SQLite's WAL and the first blobs are created
    // inside an already-restricted directory rather than being tightened after
    // the fact (D-029).
    println!("\nRestricting the data directories...");
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

    print!("{snippet}");

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
    // Returned rather than dropped: the browser front end renders the same
    // handover from these values, and re-reading them from a directory setup has
    // just hardened is a failure that does not need to exist.
    Ok(Applied {
        cfg,
        admin_token: token,
        endpoint_token,
    })
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
    println!("  -> coordinator URL: {}", cfg.public_url.as_deref().unwrap_or(""));
    println!("    This is what goes on every laptop. It is deliberately not the listen");
    println!("    address above - that binds a socket and is not something a client can use.");

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
    let auth_opts = ["shared-secret", "trusted-header", "disabled", "negotiate"];
    let sel = Select::new()
        .with_prompt("Connection auth")
        .items(&auth_opts)
        // Index 0 — shared-secret. The default is the one that refuses a stranger.
        .default(0)
        .interact()?;
    cfg.auth = auth_opts[sel].to_string();
    match cfg.auth.as_str() {
        "shared-secret" => println!(" Endpoints must present this deployment's token; the handover prints it. The acting user is still asserted rather than proven (that is E-015)."),
        "disabled" => println!("  ! warning: no connection auth - development / trusted-LAN only."),
        "trusted-header" => println!("  ! warning: accepts ANY principal header from anyone who can reach the port. Accountability only, and not spoof-proof (concept 13.1)."),
        "negotiate" => println!("  ! note: enforced Negotiate/OIDC is deferred (E-015, D-024); use shared-secret for now."),
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
        println!("  ! warning: serving plaintext HTTP - not for production.");
    }

    Ok(generate_tls)
}

/// Bring `public_url`'s scheme into line with whether TLS is configured.
///
/// Handing out an `http://` URL for a service that only speaks TLS is the same
/// class of mistake as handing out the bind address: a value that looks right and
/// connects to nothing. `Config::validate` now refuses the mismatch outright, so
/// this has to run on **every** path that can enable TLS.
///
/// It used to live inside [`interactive_fill`], which is skipped entirely under
/// `--non-interactive` — so `setup --non-interactive --tls-generate` produced TLS
/// plus an `http://` URL. That combination was merely wrong before and is now
/// fatal, which would have broken the scripted installer.
///
/// Deliberately one-directional. `https` with no TLS is *not* rewritten down to
/// `http`: an operator who typed https and configured no certificate more likely
/// wanted TLS than wanted plaintext, and silently downgrading the advertised
/// scheme would hide that. `validate` refuses it and says so.
fn reconcile_public_url_scheme(cfg: &mut Config) -> Option<String> {
    cfg.tls.as_ref()?;
    let url = cfg.public_url.take()?;
    let fixed = url.replacen("http://", "https://", 1);
    let changed = fixed != url;
    cfg.public_url = Some(fixed);
    changed.then(|| cfg.public_url.clone().unwrap_or_default())
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
        labels.push("Type a different path...".to_string());

        let sel = Select::new()
            .with_prompt("Which share should Chaperone coordinate?")
            .items(&labels)
            .default(0)
            .interact()?;
        if sel < found.len() {
            return Ok(found[sel].unc(host));
        }
    } else {
        println!("  (no local shares found - this host may not be the fileserver)");
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
             a drive letter here would not match what the endpoints report - use \\\\server\\share."
        ));
    }
    if raw.trim_start_matches('\\').split('\\').filter(|p| !p.is_empty()).count() < 2 {
        return Err(format!("'{raw}' names a server but no share"));
    }
    if !Path::new(raw).exists() {
        println!(
            "  note: {raw} is not reachable from this machine. That can be correct - \
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
///
/// Both guards compare *canonicalised* components, which is why a leading
/// `private` is dropped first — see the note in the body.
fn is_unsafe_to_harden(p: &Path) -> bool {
    use std::path::Component;
    let mut named: Vec<String> = p
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    // macOS puts the real `/var`, `/etc` and `/tmp` under `/private`, reaching them
    // through symlinks — and the caller checks the **canonicalised** path, so
    // `/var/lib` arrives here as `/private/var/lib`. With the extra component this
    // is three levels deep and equal to nothing in the list below, so it read as a
    // private directory and `restrict_dir` went on to chmod a shared system
    // directory. Only the CI runner's permissions stopped it; setup runs elevated.
    //
    // Stripped unconditionally rather than behind `cfg(target_os = "macos")`. It is
    // one code path exercised on every platform instead of a branch only one
    // platform ever runs — this file has already been bitten twice by
    // platform-conditional logic nothing else executes — and the cost elsewhere is
    // that a genuine `/private/...` directory is *refused*, which is the safe
    // direction for a guard whose job is to decline.
    if named.first().is_some_and(|c| c == "private") {
        named.remove(0);
    }
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
    // First, because it is where both tokens go. With `db_url` and `blob_root`
    // pointed at another volume — the split-volume deployment — this directory is
    // named by nothing else in the list, and the tokens would sit unrestricted.
    if let Some(dir) = cfg.data_dir() {
        targets.push(dir);
    }
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
                "did NOT restrict {} - it is a shared or top-level directory. \
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
        println!("Self-test: TLS enabled - verify manually at https://{}/healthz", cfg.addr);
        return;
    }
    std::thread::sleep(Duration::from_millis(500));
    match probe_healthz(&cfg.addr) {
        Ok(true) => println!("Self-test: /healthz OK"),
        Ok(false) => println!("Self-test: coord reachable but /healthz not OK yet - check the logs"),
        Err(e) => println!("Self-test: could not reach coord ({e}) - check the service status"),
    }
}

/// Also used by `chapr-coord status`, which asks the same question of a running
/// install that setup asks of a fresh one.
pub(crate) fn probe_healthz(addr: &str) -> Result<bool, String> {
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
/// The `mcpServers` block an MCP host can take directly, as pretty JSON.
///
/// ## Why this exists at all
///
/// The endpoint bundle ships **no defaults** — a default only ever reaches a
/// first-time installer, so a wrong one is inherited invisibly by everyone who
/// upgrades. That makes the values something an administrator has to distribute,
/// and distribution by retyping is where a root typo'd as `charptest` came from.
/// This is the same three values, in a form nothing has to retype.
///
/// ## The one field coord cannot know
///
/// `command` is the path to `chapr-endpoint` **on the user's machine**, and coord
/// has never seen it. An MCPB bundle fills it itself (`${__dirname}/server/...`);
/// a bare-binary install needs a real path. So it is emitted as a marked
/// placeholder rather than a guess — a plausible-looking wrong path is worse than
/// an obvious hole, which is the whole lesson of the defaults.
///
/// Contains the deployment token when auth is enforced. That is the point of the
/// block and also its hazard: it is a credential, and it is now in a file or a
/// clipboard. The caller says so out loud.
pub(crate) fn mcp_servers_block(cfg: &Config, endpoint_token: Option<&str>) -> String {
    let mut env = serde_json::Map::new();
    env.insert(
        "CHAPR_COORD_URL".into(),
        serde_json::Value::String(cfg.advertised_url()),
    );
    // When auth enforces and no token is present, the field is emitted as a
    // visible hole rather than omitted. Omitting it produces a block that
    // installs cleanly and then 401s on every call — a silent wrong answer,
    // which is the failure mode the no-defaults rule exists to prevent.
    match (endpoint_token, cfg.auth.as_str()) {
        (Some(t), _) => {
            env.insert(
                "CHAPR_COORD_TOKEN".into(),
                serde_json::Value::String(t.to_string()),
            );
        }
        (None, "shared-secret") => {
            env.insert(
                "CHAPR_COORD_TOKEN".into(),
                serde_json::Value::String(
                    "<MISSING - this coordinator enforces auth and has no token file>".into(),
                ),
            );
        }
        // Genuinely not needed: an auth mode that authenticates nobody has no
        // token to present, and inventing a field would imply otherwise.
        (None, _) => {}
    }
    // Emitted even when unset, so the field is visibly *there to fill* rather
    // than absent and forgotten — an unconfined endpoint is a deliberate choice,
    // not a missing line.
    env.insert(
        "CHAPR_ROOT".into(),
        serde_json::Value::String(
            cfg.share_unc
                .clone()
                .unwrap_or_else(|| "<the share path, as UNC - setup recorded none>".to_string()),
        ),
    );

    let block = serde_json::json!({
        "mcpServers": {
            "chaperone": {
                "command": "<full path to chapr-endpoint on the user's machine>",
                "args": [],
                "env": serde_json::Value::Object(env),
            }
        }
    });
    serde_json::to_string_pretty(&block).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

/// What an applied setup produced, returned so a non-console caller can render
/// the handover without going back to disk.
///
/// Going back to disk is not a theoretical problem: setup **hardens the data
/// directory** before it finishes, and the config usually lives inside it, so an
/// unelevated run cannot re-read the file it just wrote. The wizard's first
/// version did exactly that and reported "the written config could not be
/// re-read" on a run that had otherwise fully succeeded. Every value needed is
/// already in hand at that point; passing it back removes the failure mode
/// rather than handling it.
pub struct Applied {
    pub cfg: Config,
    /// The admin token and where it is kept, or why neither exists.
    pub admin_token: Result<(String, std::path::PathBuf), String>,
    pub endpoint_token: Option<String>,
}

/// The handover as a string, from what setup already has in memory.
pub(crate) fn handover_from(applied: &Applied) -> String {
    let mut s = endpoint_snippet(
        &applied.cfg,
        applied
            .admin_token
            .as_ref()
            .map(|(t, p)| (t.as_str(), p.as_path())),
        applied.endpoint_token.as_deref(),
    );
    s.push_str("\n-- Or paste this into the host's MCP config --\n");
    s.push_str("Fill in `command`; coord cannot know where the endpoint lives on a user's machine.\n\n");
    s.push_str(&mcp_servers_block(
        &applied.cfg,
        applied.endpoint_token.as_deref(),
    ));
    s.push('\n');
    s
}

/// `chapr-coord handover` — reprint what an administrator has to distribute.
///
/// Setup already prints this once. It exists as its own command because that one
/// printing scrolls away, an install outlives the terminal it was run in, and the
/// values are needed again every time a laptop is added — at which point the
/// alternative is reading the token out of the data directory by hand and
/// reconstructing the rest from memory.
///
/// Reads, never writes: no token is created here. A missing endpoint token is
/// reported rather than minted, because minting one at handover time would be
/// indistinguishable from rotating the deployment's credential.
pub fn handover(
    config: Option<&Path>,
    out: Option<&Path>,
    json_only: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::load(config)?;
    let dir = cfg.data_dir();

    // `path_in` + read, not `load_or_create`: see the note above.
    let endpoint_token = dir.as_deref().and_then(|d| {
        std::fs::read_to_string(crate::endpoint_token::path_in(d))
            .ok()
            .map(|s| s.trim().to_string())
    });
    let admin = match dir.as_deref() {
        Some(d) => std::fs::read_to_string(crate::admin_token::path_in(d))
            .map(|s| (s.trim().to_string(), crate::admin_token::path_in(d)))
            .map_err(|e| format!("could not read the admin token: {e}")),
        None => Err("no data directory (an in-memory database?), so no admin token".to_string()),
    };

    let block = mcp_servers_block(&cfg, endpoint_token.as_deref());
    let prose = endpoint_snippet(
        &cfg,
        admin.as_ref().map(|(t, p)| (t.as_str(), p.as_path())),
        endpoint_token.as_deref(),
    );

    if json_only {
        // D-035's discipline: the machine-readable thing to stdout alone, so
        // `handover --json > .mcp.json` is a usable file, and everything a human
        // needs to read on stderr beside it.
        eprint!("{prose}");
        println!("{block}");
    } else {
        print!("{prose}");
        println!(
            "\n-- Or paste this into the host's MCP config --\n\
             Fill in `command`; coord cannot know where the endpoint lives on a user's\n\
             machine. An .mcpb bundle sets it itself.\n\n{block}"
        );
    }

    if let Some(path) = out {
        std::fs::write(path, format!("{block}\n"))?;
        // Restricting it is best-effort and its failure is reported, not
        // swallowed: the file holds the deployment token, and an operator who
        // thinks it was locked down when it was not is worse off than one who
        // knows it is readable.
        let restricted = restrict_handover_file(path);
        eprintln!("\nWrote {} - it contains the deployment token.", path.display());
        match restricted {
            Ok(()) => eprintln!(
                "  Readable only by you and by administrators. Delete it once distributed."
            ),
            Err(e) => eprintln!(
                "  ! Could not restrict its permissions ({e}).\n  \
                   Anyone who can read that path can read the token. Move it somewhere\n  \
                   only administrators can reach, or delete it once distributed."
            ),
        }
    }
    Ok(())
}

/// Lock a written handover file down to its creator and administrators.
///
/// **Not** [`restrict_dir`], and the difference matters: that one grants
/// Administrators and SYSTEM, which is right for a data directory a service
/// account owns — and wrong here, because it locked the file against the very
/// operator who asked for it. A handover file exists to be *distributed*; one its
/// author cannot read is not a safer file, it is a broken feature.
///
/// The creator is named from the environment. That is the same ambient-identity
/// assumption E-023/D-024 already make for the acting principal, so it introduces
/// no new trust; if the variables are absent the grant is skipped and the caller
/// is told the permissions could not be set.
#[cfg(windows)]
fn restrict_handover_file(path: &Path) -> Result<(), String> {
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(d), Ok(u)) if !d.is_empty() && !u.is_empty() => format!("{d}\\{u}"),
        (_, Ok(u)) if !u.is_empty() => u,
        _ => return Err("could not determine the current user from the environment".into()),
    };
    let out = std::process::Command::new("icacls")
        .arg(path)
        .args([
            "/inheritance:r",
            "/grant:r",
            &format!("{user}:F"),
            "/grant:r",
            // Administrators by SID, not by name: the group is localised, and
            // this codebase has already been bitten by Danish Windows.
            "*S-1-5-32-544:F",
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
fn restrict_handover_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    // 0600: the creator only. A group grant would be guesswork here — unlike the
    // data directory, whose group is the service account's by construction.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("setting mode 0600: {e}"))
}

#[cfg(not(any(windows, unix)))]
fn restrict_handover_file(_path: &Path) -> Result<(), String> {
    Err("no file-restriction mechanism is implemented for this platform".into())
}

fn endpoint_snippet(
    cfg: &Config,
    token: Result<(&str, &Path), &String>,
    endpoint_token: Option<&str>,
) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    // Never `cfg.addr` — that is a bind address, and formatting it as a URL is
    // exactly the defect this handover once shipped.
    let base = cfg.advertised_url();

    let _ = write!(
        s,
        "\n-- Where to watch this --\n  \
         Admin page:   {base}/admin\n    \
         Overview, failures with what fixes them, conflicts, leases, audit trail,\n    \
         and the settings - including how to change the connection auth mode safely.\n  \
         Health check: {base}/healthz\n"
    );

    match token {
        Ok((token, path)) => {
            let _ = write!(
                s,
                "\n-- Sign in to the admin page with this --\n  \
                 {token}\n  \
                 Kept in {} - a directory restricted to administrators, so the file\n  \
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

    let _ = write!(s, "\n-- Give this to the users --\n  Coordinator URL:      {base}\n");
    // The token belongs in *this* section, not with the admin token: it is not a
    // secret for the administrator to keep, it is a value every laptop needs. An
    // install whose handover omitted it would look complete and admit nobody.
    match (cfg.auth.as_str(), endpoint_token) {
        ("shared-secret", Some(t)) => {
            let _ = write!(
                s,
                "  Coordinator token:    {t}\n    \
                 The same value for everyone here - it proves a laptop is one of this\n    \
                 deployment's endpoints, and is not a personal password. Without it this\n    \
                 coordinator answers 401. A packager can bake it into the .mcpb so users\n    \
                 type nothing; otherwise they paste it once, beside the URL.\n    \
                 It does NOT make the acting user verified - that is E-015. What it stops\n    \
                 is a stranger on the network acting as anyone at all.\n"
            );
        }
        ("shared-secret", None) => {
            let _ = write!(
                s,
                // "is not present", not "could not be created": this snippet is
                // printed by `handover` too, which only ever reads. A message
                // that names an action the caller did not take sends the reader
                // looking for a failure that did not happen.
                "  ! auth is \"shared-secret\" but no endpoint token is present.\n    \
                 This coordinator will refuse every endpoint. Check the data directory's\n    \
                 permissions and restart setup, or set auth = \"trusted-header\".\n"
            );
        }
        _ => {
            let _ = write!(
                s,
                "  Coordinator token:    (none - auth is \"{}\", which authenticates nobody)\n    \
                 Anyone who can reach the port can act as any user. Acceptable only on a\n    \
                 network where that is already true; switch to \"shared-secret\" otherwise.\n",
                cfg.auth
            );
        }
    }
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
                "  Coordinated location: (not configured here - the share path, as UNC)\n    \
                 Setup did not record one, so this is the one value you have to\n    \
                 supply from memory. Re-run setup to store it.\n"
            );
        }
    }
    let _ = write!(
        s,
        "    Both are fields in the endpoint bundle's own install dialog. A mapped\n    \
         drive letter is fine - it is resolved to its UNC form, so users with\n    \
         different letters still agree on which file is which.\n  \
         Listening on {} - that is where the socket binds, not what the\n    \
         laptops type. The URL above is the one to hand out.\n  \
         Identity is auto-derived from each user's OS logon (auth mode: {}). Nothing to type.\n  \
         Backend is auto-selected per endpoint OS and confirmed against coord's announcement ({}).\n",
        cfg.addr, cfg.auth, cfg.backend
    );

    let _ = write!(
        s,
        "\n-- When something breaks --\n  \
         Whole fleet:  the admin page's Errors tab.\n  \
         One laptop:   %LOCALAPPDATA%\\Chaperone\\diagnostics.jsonl on that machine.\n    \
         That second one matters: a laptop that cannot reach the coordinator\n    \
         cannot report it to the coordinator.\n  \
         Only unexpected faults appear there. A write that lost a compare-and-swap,\n  \
         or a document someone has open in Word, is a designed outcome - look under\n  \
         Conflicts and Leases for those.\n"
    );

    let _ = write!(
        s,
        "\n-- Back up together --\n  \
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

    /// The regression that motivated pulling this out of `interactive_fill`.
    /// `--non-interactive` skips that function entirely, so a scripted install with
    /// `--tls-generate` built TLS plus an `http://` URL. Wrong before; fatal once
    /// `validate` started refusing the mismatch. Asserted in both directions so the
    /// test fails if either half regresses.
    #[test]
    fn non_interactive_tls_yields_a_config_that_validates() {
        let mut cfg = Config {
            addr: "0.0.0.0:8787".into(),
            public_url: Some("http://FILESRV01:8787".into()),
            tls: Some(TlsConfig {
                cert_path: "c.pem".into(),
                key_path: "k.pem".into(),
            }),
            ..Default::default()
        };
        assert!(
            cfg.validate().is_err(),
            "the unreconciled state must be the failure this guards against"
        );

        let changed = reconcile_public_url_scheme(&mut cfg);
        assert_eq!(changed.as_deref(), Some("https://FILESRV01:8787"));
        assert_eq!(cfg.public_url.as_deref(), Some("https://FILESRV01:8787"));
        cfg.validate().expect("reconciled config must pass probe's validate");
    }

    #[test]
    fn reconciling_is_idempotent_and_scoped() {
        // Already https: nothing to report, nothing to change.
        let mut https = Config {
            public_url: Some("https://SRV:8787".into()),
            tls: Some(TlsConfig { cert_path: "c".into(), key_path: "k".into() }),
            ..Default::default()
        };
        assert_eq!(reconcile_public_url_scheme(&mut https), None);
        assert_eq!(https.public_url.as_deref(), Some("https://SRV:8787"));

        // No TLS: leave the URL exactly as typed, including an https one, so
        // `validate` can object rather than this silently downgrading it.
        let mut plaintext = Config {
            public_url: Some("https://SRV:8787".into()),
            tls: None,
            ..Default::default()
        };
        assert_eq!(reconcile_public_url_scheme(&mut plaintext), None);
        assert_eq!(
            plaintext.public_url.as_deref(),
            Some("https://SRV:8787"),
            "a one-directional rewrite must not hide the operator's contradiction"
        );

        // A derived URL stays derived — the scheme comes from the TLS setting.
        let mut derived = Config {
            public_url: None,
            tls: Some(TlsConfig { cert_path: "c".into(), key_path: "k".into() }),
            ..Default::default()
        };
        assert_eq!(reconcile_public_url_scheme(&mut derived), None);
        assert!(derived.public_url.is_none());
    }

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
        let text = endpoint_snippet(&cfg, Ok(("tok-abc", path)), Some("endpoint-tok-xyz"));

        let handout = text
            .split("-- Give this to the users --")
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
        let text = endpoint_snippet(&cfg, Err(&"no data directory".to_string()), None);
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
    fn wizard_defaults_auth_to_the_mode_that_refuses_a_stranger() {
        // No --auth flag → the wizard baseline is `shared-secret`. Was
        // `trusted-header` (D-024's MVP), which accepts any principal header from
        // anyone who can reach the port; a new install has no laptops to break, so
        // the default moved to the weakest mode that actually authenticates.
        let cfg = config_from_args(&SetupArgs::default());
        assert_eq!(cfg.auth, "shared-secret");

        // The bare `Config` default is still `disabled`, so `serve` with no config
        // and the existing test fixtures are unaffected. Only new *installs* move.
        assert_eq!(Config::default().auth, "disabled");

        // And an operator can still ask for the old posture explicitly.
        let opted_out = config_from_args(&SetupArgs {
            auth: Some("trusted-header".into()),
            ..Default::default()
        });
        assert_eq!(opted_out.auth, "trusted-header");
    }

    /// The token is a thing to hand to users, so it has to be *in* the handover.
    /// An install whose handover omitted it would look complete and admit nobody.
    #[test]
    fn the_handover_states_the_endpoint_token_when_auth_needs_one() {
        let cfg = Config {
            auth: "shared-secret".into(),
            public_url: Some("https://coord-01:8787".into()),
            tls: Some(TlsConfig { cert_path: "c".into(), key_path: "k".into() }),
            share_unc: Some(r"\\FILESRV\AICollab".into()),
            ..Default::default()
        };
        let path = Path::new("C:/ProgramData/Chaperone/admin-token");
        let text = endpoint_snippet(&cfg, Ok(("admin-tok", path)), Some("endpoint-tok-xyz"));
        assert!(text.contains("endpoint-tok-xyz"), "the value itself must be printed");
        assert!(text.contains("Coordinator token"));
        assert!(
            text.contains("not a personal password"),
            "it is one value for everyone; saying so prevents a support call"
        );
        assert!(!text.contains("admin-tok\n  The same value"), "the two tokens must not be conflated");

        // And when the mode does not need one, say that plainly rather than
        // printing nothing — silence reads as "nothing to configure here".
        let open = Config { auth: "trusted-header".into(), ..cfg.clone() };
        let text = endpoint_snippet(&open, Ok(("admin-tok", path)), Some("unused"));
        assert!(text.contains("authenticates nobody"), "{text}");
        assert!(!text.contains("unused"), "an unused token must not be advertised");

        // The failure case is the dangerous one: enforcing, with no token.
        let broken = Config { auth: "shared-secret".into(), ..cfg };
        let text = endpoint_snippet(&broken, Ok(("admin-tok", path)), None);
        assert!(text.contains("refuse every endpoint"), "{text}");
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
            addr: "127.0.0.1:0".into(), // ephemeral -> always bindable
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
    fn an_empty_flag_means_unset_rather_than_an_empty_value() {
        // The installer's constraint, made a property of the flags themselves. An
        // MSI's ServiceInstall arguments are one formatted string with no way to
        // omit a flag, so every unanswered field arrives as `--flag ""`. Taken
        // literally that is a coordinator fronting the share named "", which
        // validates and coordinates nothing.
        let args = SetupArgs {
            addr: Some("0.0.0.0:8787".into()),
            share_unc: Some(r"\\FILESRV01\Sales".into()),
            public_url: Some("   ".into()),
            watch_dir: Some(String::new()),
            ..Default::default()
        };
        let cfg = config_from_args(&args);
        assert_eq!(cfg.addr, "0.0.0.0:8787");
        assert_eq!(cfg.share_unc.as_deref(), Some(r"\\FILESRV01\Sales"));
        assert_eq!(cfg.public_url, None, "an empty URL flag must read as unset");
        assert_eq!(cfg.watch_dir, None);
    }

    #[test]
    fn a_service_provisions_a_config_it_does_not_have() {
        // The installer's whole contract: it registers a service with these
        // values and starts it, and nothing else writes the config. If this does
        // not happen the service comes up against a file that does not exist,
        // which is exactly the failure that removed the custom action.
        //
        // The written file is deliberately NOT read back here. Provisioning ends
        // by restricting the directory to administrators and the service account
        // (D-029), which an ordinary test process cannot then read - the same
        // reason an unelevated `serve` cannot read its own config. The content is
        // `config_from_args`, covered by the test above.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Chaperone");
        let args = SetupArgs {
            config_out: dir.join("coord.toml"),
            data_dir: Some(dir.clone()),
            addr: Some("0.0.0.0:8787".into()),
            ..Default::default()
        };
        assert!(provision_if_missing(&args).unwrap(), "a missing config is written");
        assert!(dir.exists(), "the data directory is created, or SQLite has nowhere to go");
    }

    #[test]
    fn provisioning_never_touches_a_config_that_exists() {
        // Every start after the first. An administrator's edits outlive the
        // service that wrote the file, and an upgrade must not reset them.
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("coord.toml");
        std::fs::write(&existing, "# edited by hand\ndb_url = \"sqlite::memory:\"\n").unwrap();
        let args = SetupArgs {
            config_out: existing.clone(),
            addr: Some("0.0.0.0:9999".into()),
            ..Default::default()
        };
        assert!(!provision_if_missing(&args).unwrap());
        let after = std::fs::read_to_string(&existing).unwrap();
        assert!(after.contains("edited by hand"), "{after}");
        assert!(!after.contains("9999"), "{after}");
    }

    #[test]
    fn provisioning_refuses_values_that_would_not_make_a_usable_config() {
        // Fail at the point the values arrive, not four screens later as a
        // coordinator with no tokens (I-017).
        let tmp = tempfile::tempdir().unwrap();
        let args = SetupArgs {
            config_out: tmp.path().join("coord.toml"),
            db: Some(r"C:\ProgramData\Chaperone\coord.db".into()),
            ..Default::default()
        };
        let err = provision_if_missing(&args).unwrap_err();
        assert!(err.contains("sqlite:"), "{err}");
        assert!(!args.config_out.exists(), "nothing is written when the values are refused");
    }

    /// What makes the MSI a single property instead of four (I-017, D-049).
    ///
    /// Split by platform for the reason `hardening_refuses_shared_and_top_level_directories`
    /// gives a few tests down, and it is not tidiness: `PathBuf::join` uses the
    /// **host's** separator, so a Windows-shaped literal asserted on Linux
    /// produces `C:\ProgramData\Chaperone/blobs` — which is not a bug in the
    /// coordinator, only a test written on one platform for three.
    #[test]
    fn one_data_dir_places_the_database_and_the_blob_store() {
        let dir = if cfg!(windows) { r"C:\ProgramData\Chaperone" } else { "/var/lib/chaperone" };
        let args = SetupArgs {
            data_dir: Some(std::path::PathBuf::from(dir)),
            ..Default::default()
        };
        let cfg = config_from_args(&args);

        // The database URL is the platform-independent half, and the half that
        // matters: forward slashes on every host, and the `sqlite:` prefix that
        // tells the rest of coord this names a file at all (I-017).
        assert!(cfg.db_url.starts_with("sqlite:"), "{}", cfg.db_url);
        assert!(!cfg.db_url.contains('\\'), "SQLite wants forward slashes: {}", cfg.db_url);
        assert!(cfg.db_url.ends_with("/coord.db?mode=rwc"), "{}", cfg.db_url);

        // The blob root is a filesystem path, so it is spelled the host's way.
        assert_eq!(cfg.blob_root, std::path::Path::new(dir).join("blobs").display().to_string());
        assert_eq!(cfg.data_dir(), Some(std::path::PathBuf::from(dir)));
        cfg.validate().expect("the derived database URL must satisfy the strict reading");
    }

    /// The Windows spelling in full, because the deployment that shipped it is a
    /// Windows one and an assertion about `C:/ProgramData/...` is worth making
    /// literally rather than by construction.
    #[cfg(windows)]
    #[test]
    fn a_windows_data_dir_produces_the_url_the_installer_relies_on() {
        let args = SetupArgs {
            data_dir: Some(std::path::PathBuf::from(r"C:\ProgramData\Chaperone")),
            ..Default::default()
        };
        let cfg = config_from_args(&args);
        assert_eq!(cfg.db_url, "sqlite:C:/ProgramData/Chaperone/coord.db?mode=rwc");
        assert_eq!(cfg.blob_root, r"C:\ProgramData\Chaperone\blobs");
    }

    #[test]
    fn explicit_locations_still_override_the_data_directory() {
        // The split-volume deployment: state has a home, the database lives
        // elsewhere, and the tokens follow the home rather than the database.
        let args = SetupArgs {
            data_dir: Some(std::path::PathBuf::from(r"C:\ProgramData\Chaperone")),
            db: Some("sqlite:D:/coordstate/coord.db?mode=rwc".into()),
            blobs: Some(r"D:\coordstate\blobs".into()),
            ..Default::default()
        };
        let cfg = config_from_args(&args);
        assert_eq!(cfg.db_url, "sqlite:D:/coordstate/coord.db?mode=rwc");
        assert_eq!(cfg.blob_root, r"D:\coordstate\blobs");
        assert_eq!(
            cfg.data_dir(),
            Some(std::path::PathBuf::from(r"C:\ProgramData\Chaperone"))
        );
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
        // The `/private/...` forms are macOS's canonicalisation of exactly these
        // paths, and they are asserted on every platform on purpose. They shipped
        // as a hole because they were only reachable on the one OS nobody built
        // for; a POSIX-shaped literal costs nothing to check everywhere.
        let mut refuse: Vec<&str> = vec![
            "/",
            "/var",
            "/var/lib",
            "/etc",
            "/home",
            "/private/var",
            "/private/var/lib",
            "/private/etc",
            "/private/tmp",
        ];
        let mut allow: Vec<&str> = vec![
            "/var/lib/chapr",
            "/srv/chapr/blobs",
            // Still allowed once it is genuinely the coordinator's own directory.
            "/private/var/lib/chapr",
        ];
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

    /// The regression that shipped: on macOS `/var` is a symlink into `/private`,
    /// and `harden_data_dirs` checks the **canonicalised** path — so `/var/lib`
    /// arrived at the guard as `/private/var/lib`, matched nothing, and was handed
    /// to `restrict_dir`, which tried to chmod a shared system directory. The CI
    /// runner lacked permission; setup runs elevated, so it would have succeeded.
    ///
    /// Driven through `harden_data_dirs` with the already-canonical form rather
    /// than relying on a real symlink, so it reproduces the actual failure on every
    /// platform instead of only the one that has `/private`.
    #[test]
    fn harden_data_dirs_refuses_the_macos_canonical_form_of_a_shared_directory() {
        let cfg = Config {
            blob_root: "/private/var/lib".into(),
            db_url: "sqlite::memory:".into(),
            ..Config::default()
        };
        let warnings = harden_data_dirs(&cfg);
        assert_eq!(warnings.len(), 1, "one refusal expected, got {warnings:?}");
        assert!(
            warnings[0].contains("did NOT restrict"),
            "must refuse, not attempt: {}",
            warnings[0]
        );
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
