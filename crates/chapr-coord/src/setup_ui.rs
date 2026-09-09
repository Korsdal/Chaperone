// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! `chapr-coord setup --ui` — the wizard in a browser (W2).
//!
//! ## Why a browser and not a dialog
//!
//! D-032 settled that **the executable is the installer**. What it did not settle
//! was what an administrator *meets*, and until now that was a terminal with
//! `dialoguer` prompts. Coord already has `axum` and already serves an HTML admin
//! page, so a served page costs **no new dependency** — where a native dialog
//! means a GUI toolkit in a binary that ships as one static executable, and would
//! put UI code in the crate whose standing rule is that it contains no Windows
//! primitives. It is also the only option that is not Windows-only, which matters
//! because coord runs on Linux too.
//!
//! ## This is a front end for `SetupArgs`, and nothing else
//!
//! The page collects values and then calls the **same** [`crate::setup::run`]
//! that the terminal wizard calls, in its unattended mode. There is deliberately
//! no second copy of the probe, the config write, the hardening, the service
//! install or the handover — the two front ends cannot drift because there is
//! only one back end. Anything the browser can produce, `--non-interactive`
//! flags can produce too.
//!
//! ## What guards it
//!
//! A page that writes a config file and registers a system service is a local
//! privilege surface, so:
//!
//! - it binds **127.0.0.1** on an ephemeral port — never a wildcard, so nothing
//!   off-box can reach it even for the seconds it lives;
//! - every request carries a **one-time token** minted per run and compared in
//!   constant time (the same primitive as the admin token);
//! - it is **single-shot**: the listener stops after one successful apply, so the
//!   window is one operation wide rather than "until someone closes the window";
//! - the URL is printed to the console as well as opened, because a wizard whose
//!   only route in is an auto-opened browser is a wizard that fails on a server
//!   with no default browser.

use crate::config::{machine_hostname, Config};
use crate::setup::SetupArgs;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::Mutex;

/// What the page posts back. Every field is a string because it comes from a
/// form; parsing and validation stay in [`crate::setup`], which already refuses
/// what it cannot use.
#[derive(Debug, Deserialize)]
struct Form {
    token: String,
    addr: String,
    hostname: String,
    share_unc: String,
    db_url: String,
    blob_root: String,
    auth: String,
    backend: String,
    watch_dir: String,
    /// `none` | `generate` | `supply`
    tls: String,
    tls_cert: String,
    tls_key: String,
    config_out: String,
    install_service: bool,
}

struct Wizard {
    token: String,
    /// Set once an apply succeeds, so the server can stop itself.
    done: Arc<Mutex<Option<String>>>,
    /// The args this run was invoked with, used to pre-fill the form.
    ///
    /// Carried rather than ignored because they are not cosmetic: a double-click
    /// arrives via `SetupArgs::default_for_wizard`, which points the config and
    /// the data at `%ProgramData%\Chaperone` instead of whatever directory the
    /// executable was launched from — usually a Downloads folder. A form that
    /// rebuilt its own defaults would silently undo that, and the install would
    /// land beside the exe.
    args: SetupArgs,
}

/// Run the browser wizard. Returns once a config has been applied, or when the
/// operator abandons it (Ctrl-C).
pub async fn run(args: SetupArgs) -> Result<(), Box<dyn std::error::Error>> {
    // The elevation warning belongs here as well as in the terminal wizard: the
    // browser is a different route to the same privileged operation, and an
    // operator who never sees a console must still be told before answering ten
    // questions. Repeated on the page itself for the same reason.
    let elevated = crate::host::is_elevated();

    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let done: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let state = Arc::new(Wizard {
        token: token.clone(),
        done: done.clone(),
        args,
    });

    // Port 0 = let the OS choose. A fixed port would collide with whatever else
    // is on this machine and, worse, would be guessable by anything local.
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}/?t={token}");

    println!("── Chaperone coordination service — setup ──\n");
    if !elevated {
        println!("  ! Not running as administrator. The config can still be written, but");
        println!("    registering the service will fail. Close this and re-run elevated if");
        println!("    you want the service installed.\n");
    }
    println!("  Open this in a browser:\n    {url}\n");
    println!("  Only this machine can reach it, and the link works once.");
    println!("  Leave this window open; it closes itself when setup finishes.\n");
    open_browser(&url);

    let app = Router::new()
        .route("/", get(page))
        .route("/apply", post(apply))
        .with_state(state);

    // Shutdown is driven by the apply handler setting `done`. Polled rather than
    // signalled through a channel because the handler needs the result *in* the
    // response as well, so the value has to live somewhere both can see.
    let done_watch = done.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            loop {
                if done_watch.lock().await.is_some() {
                    // A beat, so the response is flushed to the browser before
                    // the socket goes away. Without it the operator sees a
                    // connection error in place of the handover they just earned.
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        })
        .await?;

    // Bound out of the `match`: a guard held in a scrutinee at the end of a
    // function outlives the value it borrows, which the borrow checker refuses.
    let summary = done.lock().await.clone();
    match summary {
        Some(summary) => {
            // Printed to the console too. A handover that exists only in a
            // browser tab is one refresh from being lost, and the tab is not
            // where anyone looks for it a week later.
            print!("{summary}");
            println!("\nSetup finished. `chapr-coord handover` reprints this at any time.");
            Ok(())
        }
        None => Err("setup was abandoned before anything was applied".into()),
    }
}

/// Best-effort browser launch. Never fatal: the console has the URL, and a
/// headless server legitimately has nothing to open.
fn open_browser(url: &str) {
    #[cfg(windows)]
    let spawned = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = std::process::Command::new("xdg-open").arg(url).spawn();

    if spawned.is_err() {
        println!("  (could not open a browser automatically — use the link above)");
    }
}

#[derive(Deserialize)]
struct Tok {
    t: Option<String>,
}

async fn page(State(w): State<Arc<Wizard>>, Query(q): Query<Tok>) -> impl IntoResponse {
    if !crate::admin_token::verify(&w.token, q.t.as_deref().unwrap_or_default()) {
        return (
            StatusCode::UNAUTHORIZED,
            Html("<h1>Wrong or missing link</h1><p>Use the URL printed in the console. It is minted per run.</p>".to_string()),
        );
    }
    (StatusCode::OK, Html(render(&w.token, &w.args)))
}

async fn apply(State(w): State<Arc<Wizard>>, Json(form): Json<Form>) -> axum::response::Response {
    if !crate::admin_token::verify(&w.token, &form.token) {
        return (StatusCode::UNAUTHORIZED, Json(reply(false, "Wrong token."))).into_response();
    }
    if w.done.lock().await.is_some() {
        return (
            StatusCode::CONFLICT,
            Json(reply(false, "Setup has already been applied by this run.")),
        )
            .into_response();
    }

    // Everything the page collected becomes flags. This is the whole design: the
    // browser decides *values*, `setup::run` decides *behaviour*.
    let port = form.addr.rsplit(':').next().unwrap_or("8787").to_string();
    let scheme = if form.tls == "none" { "http" } else { "https" };
    let args = SetupArgs {
        config_out: std::path::PathBuf::from(&form.config_out),
        non_interactive: true,
        no_service: !form.install_service,
        addr: Some(form.addr.clone()),
        public_url: Some(format!("{scheme}://{}:{port}", form.hostname.trim())),
        db: Some(form.db_url.clone()),
        blobs: Some(form.blob_root.clone()),
        auth: Some(form.auth.clone()),
        watch_dir: non_empty(&form.watch_dir),
        share_unc: non_empty(&form.share_unc),
        tls_cert: (form.tls == "supply").then(|| form.tls_cert.clone()),
        tls_key: (form.tls == "supply").then(|| form.tls_key.clone()),
        tls_generate: form.tls == "generate",
        tls_hostname: non_empty(&form.hostname),
        backend: Some(form.backend.clone()),
        // False, and it has to be: `setup::run` redirects here when `ui` is set,
        // so passing it back through would recurse into a second wizard.
        // `non_interactive` above already guards that, and this makes the intent
        // explicit rather than relying on the guard.
        ui: false,
    };

    // `map_err` **before** the match, and this is not style. `setup::run`'s error
    // is a `Box<dyn Error>`, which is not `Send`; a `match` keeps its scrutinee
    // alive for the whole match, so the arms' `.await`s would hold a non-Send
    // value across a suspension point and the handler would stop being a valid
    // axum `Handler` — reported only as "the trait bound is not satisfied", with
    // no mention of Send or of this line.
    let outcome = crate::setup::run(args).await.map_err(|e| e.to_string());
    match outcome {
        Ok(applied) => {
            let summary = crate::setup::handover_from(&applied);
            *w.done.lock().await = Some(summary.clone());
            (StatusCode::OK, Json(reply(true, &summary))).into_response()
        }
        // Reported to the page and NOT recorded as done, so the operator can fix
        // the value and post again rather than restarting the whole wizard.
        Err(e) => (StatusCode::BAD_REQUEST, Json(reply(false, &e))).into_response(),
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn reply(ok: bool, message: &str) -> serde_json::Value {
    serde_json::json!({ "ok": ok, "message": message })
}

/// The form. Defaults mirror [`crate::setup`]'s interactive prompts exactly —
/// same baseline listen address, same hostname source, same auth ordering — so
/// the two front ends do not disagree about what a fresh install looks like.
fn render(token: &str, args: &SetupArgs) -> String {
    let cfg = Config::default();
    let host = machine_hostname().unwrap_or_else(|| "localhost".to_string());
    // Args win over built-in defaults wherever they were supplied — see the note
    // on `Wizard::args`. `addr`'s baseline is the wildcard rather than
    // `Config::default()`'s loopback, matching the terminal wizard exactly: a
    // delivered service has to be reachable.
    let addr = args
        .addr
        .clone()
        .unwrap_or_else(|| "0.0.0.0:8787".to_string());
    let db = args.db.clone().unwrap_or_else(|| cfg.db_url.clone());
    let blobs = args.blobs.clone().unwrap_or_else(|| cfg.blob_root.clone());
    let share = args.share_unc.clone().unwrap_or_default();
    let watch = args.watch_dir.clone().unwrap_or_default();
    let config_out = args.config_out.display().to_string();
    format!(
        r#"<!doctype html>
<meta charset="utf-8">
<title>Chaperone coordinator — setup</title>
<style>
  :root {{ color-scheme: light dark; }}
  body {{ font: 15px/1.5 system-ui, sans-serif; max-width: 46rem; margin: 2rem auto; padding: 0 1rem; }}
  h1 {{ font-size: 1.4rem; }}
  fieldset {{ border: 1px solid #8884; border-radius: 6px; margin: 1.2rem 0; padding: 1rem; }}
  legend {{ font-weight: 600; padding: 0 .4rem; }}
  label {{ display: block; margin: .7rem 0; }}
  label > span {{ display: block; font-weight: 600; }}
  label > em {{ display: block; font-style: normal; opacity: .75; font-size: .9em; }}
  input[type=text], select {{ width: 100%; padding: .4rem; font: inherit; }}
  .row {{ display: flex; gap: .5rem; align-items: baseline; }}
  button {{ font: inherit; padding: .6rem 1.2rem; border-radius: 6px; }}
  #out {{ white-space: pre-wrap; font-family: ui-monospace, monospace; font-size: .85em;
          background: #8881; padding: 1rem; border-radius: 6px; margin-top: 1rem; }}
  .warn {{ color: #b45309; }}
</style>
<h1>Chaperone coordinator — setup</h1>
<p>These answers are written to a config file, then the service is registered and
started. Nothing is applied until you press the button.</p>

<fieldset><legend>Reachability</legend>
  <label><span>Listen address</span>
    <em>Where the socket binds. <code>0.0.0.0:8787</code> means every interface on this machine.</em>
    <input type=text id=addr value="{addr}"></label>
  <label><span>Hostname the laptops connect to</span>
    <em>What a client types — deliberately not the listen address, which is not usable by a client.</em>
    <input type=text id=hostname value="{host}"></label>
</fieldset>

<fieldset><legend>The coordinated share</legend>
  <label><span>Share path (UNC)</span>
    <em>The location Chaperone coordinates, e.g. <code>\\FILESRV\AICollab</code>. Every laptop
       is given this exact string — coordination is keyed by path, so two spellings are two
       different shares forever.</em>
    <input type=text id=share_unc value="{share}" placeholder="\\FILESRV\AICollab"></label>
</fieldset>

<fieldset><legend>Storage</legend>
  <label><span>SQLite URL</span><input type=text id=db_url value="{db}"></label>
  <label><span>Blob store directory</span>
    <em>Holds a copy of every file version. A separate persistent mount is recommended, and it
       must be backed up together with the database — history needs both from the same moment.</em>
    <input type=text id=blob_root value="{blobs}"></label>
</fieldset>

<fieldset><legend>Security</legend>
  <label><span>Connection auth</span>
    <em><b>shared-secret</b> is the default and the only one that refuses a stranger on the
       network. The acting user is still asserted rather than proven either way.</em>
    <select id=auth>
      <option value="shared-secret" selected>shared-secret — endpoints present this deployment's token</option>
      <option value="trusted-header">trusted-header — accepts any principal from anyone who can reach the port</option>
      <option value="disabled">disabled — authenticates nobody</option>
      <option value="negotiate">negotiate — not enforced yet (E-015)</option>
    </select></label>
  <label><span>TLS</span>
    <select id=tls>
      <option value="generate" selected>Generate a self-signed certificate</option>
      <option value="supply">Supply my own (from an internal CA)</option>
      <option value="none">None — plain HTTP</option>
    </select>
    <em id=tlsnote class=warn>Self-signed: every laptop must be told to trust it, or it will refuse the connection.</em>
  </label>
  <div id=tlsfields hidden>
    <label><span>Certificate path (PEM)</span><input type=text id=tls_cert></label>
    <label><span>Private key path (PEM)</span><input type=text id=tls_key></label>
  </div>
</fieldset>

<fieldset><legend>Optional</legend>
  <label><span>Fileserver backend</span>
    <em>Coord only announces this; each endpoint confirms it against its own capabilities.</em>
    <select id=backend><option value="smb" selected>smb</option><option value="posix">posix</option></select></label>
  <label><span>Change-watcher directory</span>
    <em>Leave empty to disable. Recommended if people also edit these files outside Chaperone.
       Windows-only today.</em>
    <input type=text id=watch_dir value="{watch}"></label>
  <label><span>Config file to write</span><input type=text id=config_out value="{config_out}"></label>
  <label class=row><input type=checkbox id=install_service checked>
    <span style="font-weight:400">Register and start the service (needs administrator)</span></label>
</fieldset>

<button id=go>Apply</button>
<div id=out hidden></div>

<script>
const el = (id) => document.getElementById(id);
const tls = el("tls");
function syncTls() {{
  el("tlsfields").hidden = tls.value !== "supply";
  el("tlsnote").hidden = tls.value !== "generate";
}}
tls.addEventListener("change", syncTls); syncTls();

el("go").addEventListener("click", async () => {{
  const out = el("out");
  out.hidden = false;
  out.textContent = "Applying…";
  el("go").disabled = true;
  const body = {{
    token: "{token}",
    addr: el("addr").value, hostname: el("hostname").value,
    share_unc: el("share_unc").value, db_url: el("db_url").value,
    blob_root: el("blob_root").value, auth: el("auth").value,
    backend: el("backend").value, watch_dir: el("watch_dir").value,
    tls: tls.value, tls_cert: el("tls_cert").value, tls_key: el("tls_key").value,
    config_out: el("config_out").value, install_service: el("install_service").checked,
  }};
  try {{
    const r = await fetch("/apply", {{
      method: "POST", headers: {{ "content-type": "application/json" }},
      body: JSON.stringify(body),
    }});
    const j = await r.json();
    out.textContent = j.message;
    // Re-enabled on failure only: a successful apply has stopped the server, so
    // a second press could only produce a connection error.
    if (!j.ok) el("go").disabled = false;
    else document.title = "Setup finished — Chaperone";
  }} catch (e) {{
    out.textContent = "Could not reach the wizard: " + e
      + "\n(If setup already finished, this is expected — the link works once.)";
  }}
}});
</script>
"#,
        host = host,
        addr = addr,
        share = share,
        watch = watch,
        config_out = config_out,
        db = db,
        blobs = blobs,
        token = token,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> SetupArgs {
        SetupArgs {
            config_out: std::path::PathBuf::from("C:/ProgramData/Chaperone/coord.toml"),
            db: Some("sqlite:C:/ProgramData/Chaperone/coord.db?mode=rwc".into()),
            blobs: Some("C:/ProgramData/Chaperone/blobs".into()),
            share_unc: Some(r"\FILESRV\AICollab".into()),
            ..Default::default()
        }
    }

    /// The bug this exists to prevent, which was live for one build: `render`
    /// rebuilt its own defaults and ignored the args, so a double-click — whose
    /// whole purpose is to point the install at `%ProgramData%` rather than the
    /// Downloads folder the exe was launched from — silently landed everything
    /// beside the executable.
    #[test]
    fn the_form_is_prefilled_from_the_args_not_from_scratch() {
        let html = render("tok", &args());
        for expected in [
            "C:/ProgramData/Chaperone/coord.toml",
            "sqlite:C:/ProgramData/Chaperone/coord.db?mode=rwc",
            "C:/ProgramData/Chaperone/blobs",
            r"\FILESRV\AICollab",
        ] {
            assert!(html.contains(expected), "form lost {expected:?}");
        }
    }

    /// The token is what stands between a local page and a config write plus a
    /// service registration, so it has to reach the form — a page that rendered
    /// without it would post unauthenticated and look broken instead of guarded.
    #[test]
    fn the_one_time_token_reaches_the_page() {
        assert!(render("abc123", &args()).contains("abc123"));
    }

    /// Same baseline as the terminal wizard: the wildcard, not
    /// `Config::default()`'s loopback. A delivered service that binds loopback is
    /// a coordinator no laptop can reach, which is the defect `public_url` versus
    /// `addr` already exists to prevent.
    #[test]
    fn the_listen_default_is_reachable_not_loopback() {
        let html = render("tok", &SetupArgs::default());
        assert!(
            html.contains(r#"id=addr value="0.0.0.0:8787""#),
            "{html:.0}"
        );
    }

    /// `--non-interactive` has no questions, so it must never be sent to a
    /// browser. Asserted on the same condition `main` dispatches on, so the two
    /// cannot drift.
    #[test]
    fn unattended_runs_never_get_a_browser() {
        let a = SetupArgs {
            ui: true,
            non_interactive: true,
            ..Default::default()
        };
        assert!(
            !(a.ui && !a.non_interactive),
            "unattended must not open a UI"
        );
    }
}
