// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! `status` and `uninstall` — the rest of the service's life (W2).
//!
//! ## Why these are their own commands
//!
//! `setup` installs an auto-start native service and always has. What it had no
//! answer for was every question afterwards: *is it running? on which config? how
//! do I remove it?* Without those, the honest recovery advice for a coordinator
//! in a bad state was "reinstall over it and hope", and an operator with no way
//! to check the service ends up running `serve` in a terminal instead — which
//! works until the terminal closes, and is exactly the fragility that prompted
//! this work.
//!
//! ## Uninstall keeps the data, deliberately
//!
//! Removing the service does **not** remove the database, the blob store or the
//! audit log. The audit trail is a primary deliverable, and history is only
//! restorable when the database and blobs come from the same moment — so a
//! command that deleted them would destroy, irreversibly, the two things the
//! product exists to preserve. There is no `--purge`: a flag that erases an audit
//! trail is a flag someone puts in a script. The command prints the paths and
//! leaves the choice to a human with a file manager.
//!
//! The tokens stay too, which is the cost of that choice and is stated rather
//! than hidden: they are live credentials until the directory is deleted.

use crate::config::Config;
use std::path::Path;

/// One line about the service, however this platform reports it.
pub struct ServiceState {
    pub installed: bool,
    /// `None` when not installed, or when the state could not be read.
    pub running: Option<bool>,
    /// Whatever the platform can say — a state name, or why it could not be read.
    pub detail: String,
}

/// `chapr-coord status`. Exit code is 0 when the service is installed **and**
/// running, 1 otherwise, so a monitoring script can use it without parsing.
///
/// Reports four things independently, because they fail independently and an
/// operator diagnosing a bad install needs to know *which* is wrong: the config
/// loads, the service exists, the service runs, and the port answers.
pub async fn status(config: Option<&Path>) -> i32 {
    println!("-- Chaperone coordination service - status --\n");

    let cfg = match Config::load(config) {
        Ok(c) => {
            println!("  config        loaded");
            Some(c)
        }
        Err(e) => {
            // Not fatal to the rest: the service can be installed and running on
            // a config this invocation cannot see, which is itself worth knowing.
            println!("  config        NOT loaded - {e}");
            println!("                (the service may still be running on a config");
            println!("                 this command was not pointed at)");
            None
        }
    };

    let svc = service_state();
    if svc.installed {
        match svc.running {
            Some(true) => println!("  service       installed and running   ({})", svc.detail),
            Some(false) => println!("  service       installed, NOT running ({})", svc.detail),
            None => println!("  service       installed, state unknown ({})", svc.detail),
        }
    } else {
        println!("  service       not installed          ({})", svc.detail);
    }

    let mut healthy = false;
    if let Some(cfg) = &cfg {
        let url = cfg.advertised_url();
        match crate::setup::probe_healthz(&cfg.addr) {
            Ok(true) => {
                healthy = true;
                println!("  responding    yes - {url}/healthz");
            }
            Ok(false) => println!("  responding    connected, but /healthz did not answer ok"),
            Err(e) => println!("  responding    no - {e}"),
        }
        println!("\n  Admin page    {url}/admin");
        if let Some(dir) = cfg.data_dir() {
            println!("  Data          {}", dir.display());
        }
        println!("  Database      {}", cfg.db_url);
        println!("  Blob store    {}", cfg.blob_root);
    }

    // A service that is installed but not running, or running but not answering,
    // is the state worth spelling out — it is the one an operator misreads as
    // "installed, so fine".
    if svc.installed && svc.running == Some(false) {
        println!(
            "\n  ! The service exists but is stopped. Start it from Services, or\n    \
               `sc start ChaprCoord` on Windows / `systemctl start chapr-coord` on Linux."
        );
    } else if svc.installed && !healthy && cfg.is_some() {
        println!(
            "\n  ! Registered and started, but nothing answered on the port. Check that\n    \
               the bind address is reachable and that the config the SERVICE runs on is\n    \
               the one you think it is - `sc qc ChaprCoord` shows its command line."
        );
    }

    i32::from(!(svc.installed && svc.running == Some(true) && healthy))
}

/// `chapr-coord uninstall`. Stops and removes the service; keeps every byte of
/// data and says where it is.
pub fn uninstall(config: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    println!("-- Chaperone coordination service - uninstall --\n");

    // Loaded first and only to report the data locations. A config that will not
    // load must not stop the service being removed: "the config is broken" is one
    // of the reasons someone reaches for uninstall.
    let cfg = Config::load(config).ok();

    // Service state first, so the elevation warning is only raised when there is
    // something it would actually block. Warning about privileges before knowing
    // whether any privileged work is needed is how an operator concludes a
    // successful no-op failed.
    let svc = service_state();
    if svc.installed && !crate::host::is_elevated() {
        println!("  ! Not running as administrator, so removing the service will fail.");
        println!(
            "    Close this, right-click the executable and choose \"Run as administrator\".\n"
        );
    }
    if !svc.installed {
        println!(
            "  Service is not installed ({}). Nothing to remove.",
            svc.detail
        );
    } else {
        match remove_service() {
            Ok(note) => println!("  Service removed. {note}"),
            Err(e) => {
                // Reported, not swallowed, and the data note still prints: an
                // operator who cannot remove the service still needs to know what
                // is being left behind.
                eprintln!("  ! Could not remove the service: {e}");
            }
        }
    }

    println!("\n-- What was kept, on purpose --");
    match &cfg {
        Some(cfg) => {
            println!("  Database      {}", cfg.db_url);
            println!("  Blob store    {}", cfg.blob_root);
            if let Some(dir) = cfg.data_dir() {
                println!("  Data dir      {}", dir.display());
                println!("                also holds the admin and endpoint tokens");
            }
            println!(
                "\n  The audit trail and every file version live here. Restoring history\n  \
                 needs the database and the blob store from the SAME moment, so delete\n  \
                 them together or not at all - and only once you are sure.\n  \
                 The tokens remain valid until the directory is gone."
            );
        }
        None => {
            println!(
                "  The config could not be read, so the paths cannot be named here.\n  \
                 Nothing was deleted. The data directory is wherever that config\n  \
                 pointed - by default %ProgramData%\\Chaperone on Windows."
            );
        }
    }
    Ok(())
}

// ---- platform: Windows -----------------------------------------------------

#[cfg(windows)]
fn service_state() -> ServiceState {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = match ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
    {
        Ok(m) => m,
        Err(e) => {
            return ServiceState {
                installed: false,
                running: None,
                detail: format!("could not reach the service manager: {e}"),
            }
        }
    };
    match manager.open_service(
        crate::service_win::SERVICE_NAME,
        ServiceAccess::QUERY_STATUS,
    ) {
        Ok(svc) => match svc.query_status() {
            Ok(st) => {
                use windows_service::service::ServiceState as S;
                ServiceState {
                    installed: true,
                    running: Some(st.current_state == S::Running),
                    detail: format!("{:?}", st.current_state),
                }
            }
            Err(e) => ServiceState {
                installed: true,
                running: None,
                detail: format!("state unreadable: {e}"),
            },
        },
        // Not found is the ordinary "not installed" answer; anything else is a
        // permissions or SCM problem and must not be reported as absence.
        //
        // Translated, because the raw message is `IO error in winapi call` — which
        // says nothing, and this whole command exists to stop an operator reading
        // an opaque failure as a fact about their install.
        Err(e) => ServiceState {
            installed: false,
            running: None,
            detail: describe_scm_error(&e),
        },
    }
}

/// Turn a `windows_service` error into something an administrator can act on.
///
/// Only two of these actually happen in practice, and they mean opposite things:
/// **1060** is "no such service", the ordinary answer for a machine where setup
/// has not run; **5** is access denied, meaning the service may well exist and
/// this process simply cannot see it. Reporting the second as absence would send
/// someone to reinstall over a working install.
#[cfg(windows)]
fn describe_scm_error(e: &windows_service::Error) -> String {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
    let code = match e {
        windows_service::Error::Winapi(io) => io.raw_os_error(),
        _ => None,
    };
    match code {
        Some(ERROR_SERVICE_DOES_NOT_EXIST) => {
            "no service registered under that name - setup has not run here".into()
        }
        Some(ERROR_ACCESS_DENIED) => {
            "access denied reading the service - run as administrator; it may well exist".into()
        }
        Some(c) => format!("service manager returned error {c}"),
        None => format!("{e}"),
    }
}

#[cfg(windows)]
fn remove_service() -> Result<String, String> {
    use std::time::{Duration, Instant};
    use windows_service::service::{ServiceAccess, ServiceState as S};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| format!("service manager: {e}"))?;
    let svc = manager
        .open_service(
            crate::service_win::SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        )
        .map_err(|e| format!("opening the service: {e}"))?;

    let mut note = String::new();
    if svc
        .query_status()
        .map(|s| s.current_state != S::Stopped)
        .unwrap_or(false)
    {
        svc.stop().map_err(|e| format!("stopping it: {e}"))?;
        // Waited for, not assumed: SCM refuses DELETE while a stop is in
        // progress, so the delete below would fail for a reason that reads like
        // a permissions problem.
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match svc.query_status() {
                Ok(s) if s.current_state == S::Stopped => break,
                Ok(_) => std::thread::sleep(Duration::from_millis(300)),
                Err(_) => break,
            }
        }
        note.push_str("Stopped it first. ");
    }
    svc.delete().map_err(|e| format!("deleting it: {e}"))?;
    // The SCM keeps a deleted service until every handle closes, which is why
    // this says "marked" rather than claiming it is gone.
    note.push_str("Marked for deletion; it disappears once no handle is open.");
    Ok(note)
}

// ---- platform: Linux -------------------------------------------------------

#[cfg(all(unix, not(target_os = "macos")))]
const UNIT_PATH: &str = "/etc/systemd/system/chapr-coord.service";

#[cfg(all(unix, not(target_os = "macos")))]
fn service_state() -> ServiceState {
    let installed = Path::new(UNIT_PATH).exists();
    if !installed {
        return ServiceState {
            installed: false,
            running: None,
            detail: format!("no unit at {UNIT_PATH}"),
        };
    }
    // `is-active` rather than `status`: a single word on stdout and an exit code,
    // which is what this needs. `status` pages and formats.
    let out = std::process::Command::new("systemctl")
        .args(["is-active", "chapr-coord"])
        .output();
    match out {
        Ok(o) => {
            let state = String::from_utf8_lossy(&o.stdout).trim().to_string();
            ServiceState {
                installed: true,
                running: Some(state == "active"),
                detail: if state.is_empty() {
                    "unknown".into()
                } else {
                    state
                },
            }
        }
        Err(e) => ServiceState {
            installed: true,
            running: None,
            detail: format!("systemctl unavailable: {e}"),
        },
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn remove_service() -> Result<String, String> {
    let run = |args: &[&str]| -> Result<(), String> {
        let out = std::process::Command::new("systemctl")
            .args(args)
            .output()
            .map_err(|e| format!("running systemctl {}: {e}", args.join(" ")))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "systemctl {} exited with {}: {}",
                args.join(" "),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    };
    // Stop before disable, and tolerate a stop that fails because it was already
    // stopped — the goal is "not running and not enabled", not a clean transcript.
    let _ = run(&["stop", "chapr-coord"]);
    run(&["disable", "chapr-coord"])?;
    std::fs::remove_file(UNIT_PATH).map_err(|e| format!("removing {UNIT_PATH}: {e}"))?;
    let _ = run(&["daemon-reload"]);
    Ok(format!("Unit {UNIT_PATH} removed and systemd reloaded."))
}

// ---- platform: everything else ---------------------------------------------

#[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
fn service_state() -> ServiceState {
    ServiceState {
        installed: false,
        running: None,
        detail: "no service integration on this platform".into(),
    }
}

#[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
fn remove_service() -> Result<String, String> {
    Err("no service integration on this platform - nothing was installed".into())
}
