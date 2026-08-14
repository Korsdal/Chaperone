//! `chapr-endpoint self-test` — verification that runs on a customer's laptop.
//!
//! The live smoke suites in `examples/` already cover more than this does, and
//! none of them can be run where it matters: they need `cargo`, and a sales
//! laptop has a Claude Desktop extension and nothing else. So the checks that
//! prove a *deployment* rather than a *build* live here, in the binary that is
//! already installed on every machine.
//!
//! Two rules the output obeys, because a verification tool that overstates itself
//! is worse than none:
//!
//! - A check that could not run prints **SKIP**, never PASS. The mapped-drive
//!   branch (E-022) in particular has never been confirmed against a real server,
//!   and a green line for a check that did not execute would bury that.
//! - Contention is tested with **two sessions**, not two machines. Real lease
//!   contention needs two distinct `SessionId`s against one path; separate laptops
//!   only add distinct principals on top. One laptop is therefore enough to
//!   exercise the invariant the whole product exists for.

use crate::backend::{default_backend_kind, make_backend};
use crate::{canonicalize, grammar_for, ops, read, write, CoordClient, LeaseManager, ReadConfig};
use chapr_proto::{BackendKind, ChaprError, Principal, SessionId, WriteMode};
use std::sync::Arc;

/// One line of output, and the only three states it may have.
enum Outcome {
    Pass(String),
    Fail(String),
    Skip(String),
}

#[derive(Default)]
struct Report {
    lines: Vec<(&'static str, Outcome)>,
}

impl Report {
    fn pass(&mut self, what: &'static str, detail: impl Into<String>) {
        self.lines.push((what, Outcome::Pass(detail.into())));
    }
    fn fail(&mut self, what: &'static str, detail: impl Into<String>) {
        self.lines.push((what, Outcome::Fail(detail.into())));
    }
    fn skip(&mut self, what: &'static str, detail: impl Into<String>) {
        self.lines.push((what, Outcome::Skip(detail.into())));
    }

    /// Print and return the number of failures, which becomes the exit code.
    fn finish(&self) -> usize {
        let mut failed = 0;
        let mut skipped = 0;
        println!();
        for (what, outcome) in &self.lines {
            match outcome {
                Outcome::Pass(d) => println!("  PASS  {what}\n          {d}"),
                Outcome::Fail(d) => {
                    failed += 1;
                    println!("  FAIL  {what}\n          {d}");
                }
                Outcome::Skip(d) => {
                    skipped += 1;
                    println!("  SKIP  {what}\n          {d}");
                }
            }
        }
        let passed = self.lines.len() - failed - skipped;
        println!("\n  {passed} passed, {failed} failed, {skipped} skipped");
        if skipped > 0 {
            println!("  A skipped check was not verified. It is not a pass.");
        }
        failed
    }
}

/// Run the deployment self-test. Returns the process exit code.
pub async fn run() -> u8 {
    println!("── Chaperone endpoint — deployment self-test ──");

    let coord_url =
        std::env::var("CHAPR_COORD_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".to_string());
    let dir = match std::env::var("CHAPR_SELFTEST_DIR").or_else(|_| std::env::var("CHAPR_ROOT")) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            println!(
                "\n  Set CHAPR_SELFTEST_DIR to a folder on the coordinated share to test against,\n  \
                 e.g. CHAPR_SELFTEST_DIR=\\\\FILESRV01\\Share\\chapr-selftest\n\n  \
                 It must be inside the share this endpoint is configured for. Nothing was run."
            );
            return 2;
        }
    };
    println!("  coordinator: {coord_url}");
    println!("  test folder: {dir}");

    let mut r = Report::default();

    // 1. Is the coordinator reachable at all? Everything below needs it, so a
    //    failure here short-circuits rather than producing a cascade.
    // The principal has to be presented here exactly as `main` does it, or coord
    // in `trusted-header` mode answers 401 to everything and the self-test reports
    // a broken deployment when only the self-test was broken.
    let who = Principal::new_unchecked(crate::identity::logged_in_principal().as_str().to_string());
    let coord = CoordClient::new(&coord_url).with_principal(who.as_str());
    match coord.healthz().await {
        Ok(()) => r.pass("coordinator reachable", format!("{coord_url}/healthz answered ok")),
        Err(e) => {
            r.fail(
                "coordinator reachable",
                format!(
                    "{coord_url}/healthz failed: {e}. Check the URL, the port, and \
                     — if it is https — whether this laptop trusts the certificate."
                ),
            );
            return r.finish().min(255) as u8;
        }
    }

    // 2. The misconfiguration that shipped: a loopback coordinator URL on a laptop
    //    reaches the laptop, not the coordinator. It answers /healthz only if
    //    something else is listening locally, which is why check 1 cannot catch it.
    let host = coord_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&coord_url)
        .split(['/', ':'])
        .next()
        .unwrap_or("");
    if matches!(host, "127.0.0.1" | "localhost" | "::1" | "0.0.0.0") {
        r.fail(
            "coordinator URL is not loopback",
            format!(
                "configured as {host:?}, which on a laptop means this laptop. Every user \
                 needs the coordinator's hostname. This is the defect D-032 fixed on the \
                 coordinator side — a laptop configured before that fix still carries it."
            ),
        );
    } else {
        r.pass("coordinator URL is not loopback", format!("host is {host:?}"));
    }

    let kind = match std::env::var("CHAPR_BACKEND") {
        Ok(v) => v.parse::<BackendKind>().unwrap_or_else(|_| default_backend_kind()),
        Err(_) => default_backend_kind(),
    };
    let backend = match make_backend(kind) {
        Ok(b) => b,
        Err(e) => {
            r.fail("backend available", format!("{kind}: {e}"));
            return r.finish().min(255) as u8;
        }
    };
    r.pass("backend available", format!("driving {kind}"));
    let g = grammar_for(kind);

    // 3. Mapped drive → UNC. SKIP, loudly, when there is no drive letter to
    //    resolve: this branch has never been confirmed against a real fileserver,
    //    and it is the one that decides whether two users with different letters
    //    agree on which file is which (invariant 5).
    let looks_like_drive = dir.len() >= 2
        && dir.as_bytes()[1] == b':'
        && dir.as_bytes()[0].is_ascii_alphabetic();
    if !looks_like_drive {
        r.skip(
            "mapped drive resolves to UNC",
            format!(
                "the test folder ({dir}) is not on a drive letter, so nothing was resolved. \
                 To exercise this, point CHAPR_SELFTEST_DIR at a mapped drive instead."
            ),
        );
    } else {
        match crate::mount::default_mounts().universal_name(&dir) {
            Ok(Some(unc)) => r.pass(
                "mapped drive resolves to UNC",
                format!("{dir} → {unc}; colleagues with a different letter agree on this key"),
            ),
            // `Ok(None)` is the trait's documented answer for "genuinely local, no
            // UNC form" — a local disk, not a failure. Nothing was resolved, so
            // nothing was verified: SKIP. Reporting this as FAIL was this module's
            // own rule being broken by its first implementation.
            Ok(None) => r.skip(
                "mapped drive resolves to UNC",
                format!(
                    "{dir} is on a local disk, not a mapped network drive, so there was \
                     nothing to resolve. Point CHAPR_SELFTEST_DIR at a mapped drive on the \
                     share to exercise this."
                ),
            ),
            Err(e) => r.fail("mapped drive resolves to UNC", format!("lookup failed: {e}")),
        }
    }

    // Confinement (E-025) is **opt-in**: with no roots configured, `is_within_roots`
    // returns true for everything. So the roots have to be loaded before the check
    // below means anything — without this the check passed trivially and reported a
    // green line for work it had not done.
    let roots_configured = load_coordinated_roots(&mut r, kind);

    // From here on we touch the share.
    let what_confined = "test folder is inside the coordinated share";
    let canon_dir = match canonicalize(&dir, g) {
        Ok(c) => c,
        Err(e) => {
            r.fail(what_confined, format!("{dir}: {e}"));
            return r.finish().min(255) as u8;
        }
    };
    if roots_configured {
        r.pass(
            what_confined,
            format!("canonical form {}", canon_dir.as_str()),
        );
    } else {
        r.skip(
            what_confined,
            format!(
                "CHAPR_ROOT is not set, so confinement is off and every path passes — \
                 nothing was actually checked. The path canonicalises to {}. Set CHAPR_ROOT \
                 to the coordinated share to test this for real.",
                canon_dir.as_str()
            ),
        );
    }
    if let Err(e) = std::fs::create_dir_all(canon_dir.as_str()) {
        r.fail("test folder is writable", format!("{}: {e}", canon_dir.as_str()));
        return r.finish().min(255) as u8;
    }

    let leases = Arc::new(LeaseManager::new(coord.clone()));
    leases.clone().spawn_renewer();
    let cfg = ReadConfig::default();
    let stamp = std::process::id();

    // 4. Write and read the bytes back unchanged. The base case, and the one that
    //    proves the endpoint's own SMB path works as this user.
    let uri = format!("{dir}/selftest-roundtrip-{stamp}.txt");
    let sess = SessionId::new_unchecked(format!("selftest-{stamp}"));
    let payload = format!("chaperone self-test {stamp}").into_bytes();
    let created = ops::create(
        &coord,
        &leases,
        backend.clone(),
        &who,
        &sess,
        &uri,
        payload.clone(),
    )
    .await;
    match created {
        Ok(c) => {
            match read(
                &coord,
                backend.as_file_source(),
                backend.kind(),
                &cfg,
                &who,
                &sess,
                &uri,
            )
            .await
            {
                Ok(_) => {
                    let on_disk = canonicalize(&uri, g)
                        .ok()
                        .and_then(|p| std::fs::read(p.as_str()).ok());
                    if on_disk.as_deref() == Some(payload.as_slice()) {
                        r.pass(
                            "write then read back, byte-identical",
                            format!("{} bytes, version {}", payload.len(), c.version),
                        );
                    } else {
                        r.fail(
                            "write then read back, byte-identical",
                            "the bytes on the share differ from what was written",
                        );
                    }
                }
                Err(e) => r.fail("write then read back, byte-identical", format!("read failed: {e}")),
            }
            // 5. Contention: two sessions, one file, one common base version.
            //    Exactly one winner; the loser must be told and its bytes parked.
            contention_check(&mut r, &coord, &leases, &backend, &cfg, &uri, &c.version).await;
        }
        Err(e) => {
            r.fail(
                "write then read back, byte-identical",
                format!("create failed: {e}"),
            );
            r.skip(
                "two sessions contending on one file",
                "skipped because the base file could not be created",
            );
        }
    }

    // 6. The mandatory lock itself — the property no development environment can
    //    stand in for, checked directly rather than inferred from a write result.
    mandatory_lock_check(&mut r, &dir, stamp, g);

    // Clean up what we made; a self-test that litters a customer's share is a
    // support call of its own.
    for suffix in ["selftest-roundtrip", "selftest-lock"] {
        if let Ok(p) = canonicalize(&format!("{dir}/{suffix}-{stamp}.txt"), g) {
            let _ = std::fs::remove_file(p.as_str());
        }
    }

    r.finish().min(255) as u8
}

/// Install the coordinated roots from `CHAPR_ROOT`, as `main` does at start-up.
///
/// Returns whether any root is in force. The self-test runs in a process that never
/// went through `main`'s start-up path, so without this the confinement check has
/// nothing to check against and silently passes.
fn load_coordinated_roots(r: &mut Report, kind: BackendKind) -> bool {
    let raw = match std::env::var("CHAPR_ROOT") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return false,
    };
    let grammar = grammar_for(kind);
    let mounts = crate::mount::default_mounts();
    let mut roots = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        // Canonicalised against an empty root set — there is nothing to confine a
        // root itself to. Mirrors `main`'s own loading so the boundary the self-test
        // enforces is the boundary the running endpoint enforces.
        match crate::canon::canonicalize_in(part, grammar, mounts, &[]) {
            Ok(p) => roots.push(p),
            Err(e) => r.fail(
                "CHAPR_ROOT is usable",
                format!("{part:?} could not be canonicalised: {e}"),
            ),
        }
    }
    if roots.is_empty() {
        return false;
    }
    // Ignored deliberately: only an already-set root set can fail here, and in this
    // process nothing else sets one.
    let _ = crate::canon::set_coordinated_roots(roots);
    true
}

async fn contention_check(
    r: &mut Report,
    coord: &CoordClient,
    leases: &Arc<LeaseManager>,
    backend: &Arc<dyn crate::backend::Backend>,
    cfg: &ReadConfig,
    uri: &str,
    base: &chapr_proto::VersionToken,
) {
    let mut handles = Vec::new();
    for i in 0..2usize {
        let coord = coord.clone();
        let leases = leases.clone();
        let backend = backend.clone();
        let uri = uri.to_string();
        let base = base.clone();
        let cfg = cfg.clone();
        // Distinct sessions are what makes this real contention. Same principal is
        // fine and is in fact the harder case: nothing but the session id
        // distinguishes the two writers.
        let who = Principal::new_unchecked(format!("selftest-user{i}"));
        let sess = SessionId::new_unchecked(format!("selftest-contend-{i}"));
        handles.push(tokio::spawn(async move {
            read(
                &coord,
                backend.as_file_source(),
                backend.kind(),
                &cfg,
                &who,
                &sess,
                &uri,
            )
            .await?;
            write(
                &coord,
                &leases,
                backend.clone(),
                &who,
                &sess,
                &uri,
                format!("written-by-{i}").into_bytes(),
                base,
                WriteMode::Cas,
            )
            .await
            .map(|_| ())
        }));
    }

    let mut winners = 0;
    let mut told = 0;
    let mut unexpected = Vec::new();
    for h in handles {
        match h.await {
            Ok(Ok(())) => winners += 1,
            // All three of these are correct ways to lose: the lock refused it, CAS
            // refused it and parked the bytes, or its own read had already seen the
            // winner's newer bytes so the base it was given was never current.
            Ok(Err(ChaprError::Conflict { .. }))
            | Ok(Err(ChaprError::LeaseHeld { .. }))
            | Ok(Err(ChaprError::BaseVersionNotRecorded { .. })) => told += 1,
            Ok(Err(e)) => unexpected.push(e.to_string()),
            Err(e) => unexpected.push(format!("task panicked: {e}")),
        }
    }

    let what = "two sessions contending on one file";
    if !unexpected.is_empty() {
        r.fail(what, format!("unexpected failures: {}", unexpected.join("; ")));
    } else if winners == 1 && told == 1 {
        r.pass(
            what,
            "exactly one writer won and the other was told — no silent lost update",
        );
    } else {
        r.fail(
            what,
            format!(
                "{winners} winners and {told} refusals; exactly one of each is the \
                 only correct outcome. More than one winner means a lost update."
            ),
        );
    }
}

/// Open one file exclusively twice and require the second to be refused.
///
/// This is the single thing a development environment cannot prove, because it
/// depends on the *server* honouring `FILE_SHARE_NONE` over SMB rather than on
/// anything in this process. Everything in the write path is built on it
/// (invariant 3), so it is checked as a fact rather than inferred.
fn mandatory_lock_check(
    r: &mut Report,
    dir: &str,
    stamp: u32,
    g: &'static dyn crate::PathGrammar,
) {
    let what = "exclusive open is honoured by the server";
    #[cfg(not(windows))]
    {
        let _ = (dir, stamp, g);
        r.skip(
            what,
            "this check is Windows/SMB-specific; the POSIX backend uses advisory flock",
        );
    }
    #[cfg(windows)]
    {
        let path = match canonicalize(&format!("{dir}/selftest-lock-{stamp}.txt"), g) {
            Ok(p) => p,
            Err(e) => return r.fail(what, format!("could not canonicalise a test path: {e}")),
        };
        if let Err(e) = std::fs::write(path.as_str(), b"lock probe") {
            return r.fail(what, format!("could not create {}: {e}", path.as_str()));
        }
        let first = match crate::winfs::ExclusiveFile::open_existing(path.as_str()) {
            Ok(f) => f,
            Err(e) => {
                return r.fail(
                    what,
                    format!(
                        "the FIRST exclusive open failed: {e}. Something else already holds \
                         this file — most often antivirus or a backup agent scanning the \
                         share. Exclude the share from real-time scanning."
                    ),
                )
            }
        };
        match crate::winfs::ExclusiveFile::open_existing(path.as_str()) {
            Err(_) => r.pass(
                what,
                "a second exclusive open was refused while the first was held — \
                 the lock is real, so the write path's correctness core holds",
            ),
            Ok(_second) => r.fail(
                what,
                "a SECOND exclusive open SUCCEEDED. This server is not enforcing \
                 FILE_SHARE_NONE, so two sessions can write the same file at once and \
                 the write path cannot protect against a lost update. Do not roll out \
                 against this share until this is understood.",
            ),
        }
        drop(first);
        let _ = std::fs::remove_file(path.as_str());
    }
}
