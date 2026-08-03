//! Pilot-shaped live smoke: the two things `smoke_parts` structurally cannot
//! cover, both of which the 4-user pilot consists of.
//!
//! 1. **Realistic file size.** `smoke_parts` writes 2-byte payloads, so it never
//!    exercises `PUT /blobs` with a real pre-image. A write snapshots the file's
//!    *current* bytes to coord, so the size that matters is the size of the file
//!    already on disk. This walks a ladder and reports the first size that fails.
//! 2. **Concurrent sessions on one path.** `smoke_parts` uses a single
//!    `SessionId` and writes strictly in sequence. Four users on one file is the
//!    whole point of the product, and nothing tests it.
//!
//! Assertions are deliberately weak about *which* mechanism rejects a loser
//! (lease vs CAS) and strict about the invariant that matters: exactly one
//! winner, no torn file, and no silently discarded bytes.
//!
//! ```text
//! # Windows (SMB):
//! CHAPR_COORD_URL=http://127.0.0.1:8899 SMOKE_DIR=C:/tmp/smoke \
//!   cargo run -p chapr-endpoint --example smoke_pilot
//! # Linux (POSIX):
//! CHAPR_BACKEND=posix CHAPR_COORD_URL=http://127.0.0.1:8899 SMOKE_DIR=/tmp/smoke \
//!   cargo run -p chapr-endpoint --example smoke_pilot
//! ```

use chapr_endpoint::backend::{default_backend_kind, make_backend};
use chapr_endpoint::{canonicalize, grammar_for, ops, read, write, CoordClient, LeaseManager, ReadConfig};
use chapr_proto::{BackendKind, ChaprError, Principal, SessionId, WriteMode};
use std::sync::Arc;

/// The size ladder, in KiB. 2 MiB is axum's default request-body limit, so a
/// failure that starts between 1 MiB and 4 MiB points at `PUT /blobs`.
const LADDER_KIB: &[usize] = &[64, 512, 1024, 4096, 16384];

/// How many concurrent "users" contend for one file.
const USERS: usize = 4;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::var("CHAPR_COORD_URL").unwrap_or_else(|_| "http://127.0.0.1:8899".into());
    let dir = std::env::var("SMOKE_DIR").expect("set SMOKE_DIR to a writable local dir");
    let kind = match std::env::var("CHAPR_BACKEND") {
        Ok(v) => v.parse::<BackendKind>().unwrap_or_else(|_| default_backend_kind()),
        Err(_) => default_backend_kind(),
    };
    let coord = CoordClient::new(&base);
    let leases = Arc::new(LeaseManager::new(coord.clone()));
    leases.clone().spawn_renewer();
    let backend = make_backend(kind).expect("backend available for this OS");
    let g = grammar_for(kind);
    let cfg = ReadConfig::default();
    println!("backend = {kind}");

    let mut pass = 0u32;
    let mut fail = 0u32;

    // ---------------------------------------------------------------------
    // Part 1 — size ladder. create N bytes, then write N bytes over it, which
    // forces coord to swallow an N-byte pre-image through PUT /blobs.
    // ---------------------------------------------------------------------
    println!("\n== part 1: write size ladder (pre-image through PUT /blobs) ==");
    let who = Principal::new_unchecked("CONTOSO\\pilot");
    let sess = SessionId::new_unchecked("sess-pilot-size");

    for &kib in LADDER_KIB {
        let uri = format!("{dir}/pilot_size_{kib}k.bin");
        let canon = canonicalize(&uri, g)?;
        let _ = std::fs::remove_file(canon.as_str());

        let first = vec![b'a'; kib * 1024];
        let second = vec![b'b'; kib * 1024];

        let created = match ops::create(&coord, &leases, backend.clone(), &who, &sess, &uri, first).await {
            Ok(c) => c,
            Err(e) => {
                fail += 1;
                println!("  FAIL  {kib} KiB: create rejected: {e}");
                continue;
            }
        };

        match write(
            &coord,
            &leases,
            backend.clone(),
            &who,
            &sess,
            &uri,
            second,
            created.version.clone(),
            WriteMode::Cas,
        )
        .await
        {
            Ok(_) => {
                let on_disk = std::fs::metadata(canon.as_str()).map(|m| m.len()).unwrap_or(0);
                if on_disk == (kib * 1024) as u64 {
                    pass += 1;
                    println!("  PASS  {kib} KiB: write round-trip, {on_disk} bytes on disk");
                } else {
                    fail += 1;
                    println!("  FAIL  {kib} KiB: write reported OK but disk has {on_disk} bytes");
                }
            }
            Err(e) => {
                fail += 1;
                println!("  FAIL  {kib} KiB: write rejected: {e}");
                // Did the rejection leave the file intact, or torn?
                match std::fs::read(canon.as_str()) {
                    Ok(b) if b.iter().all(|&c| c == b'a') => {
                        println!("        (pre-image intact on disk — failed closed, good)");
                    }
                    Ok(b) => println!("        (DISK TORN: {} bytes, mixed content)", b.len()),
                    Err(e) => println!("        (file unreadable after failure: {e})"),
                }
            }
        }
        let _ = std::fs::remove_file(canon.as_str());
    }

    // ---------------------------------------------------------------------
    // Part 2 — USERS distinct sessions racing one path with the same valid
    // base_version. Exactly one must win; every loser must be told; no bytes
    // may vanish; the file must never contain a mix.
    // ---------------------------------------------------------------------
    println!("\n== part 2: {USERS} concurrent sessions, one file ==");
    let uri = format!("{dir}/pilot_race.txt");
    let canon = canonicalize(&uri, g)?;
    let _ = std::fs::remove_file(canon.as_str());

    let setup_sess = SessionId::new_unchecked("sess-pilot-setup");
    let created = ops::create(&coord, &leases, backend.clone(), &who, &setup_sess, &uri, b"base".to_vec()).await?;
    let base_version = created.version.clone();

    // Each user reads first (its own session needs its own read receipt), then
    // all of them write concurrently from the same base.
    let mut handles = Vec::new();
    for i in 0..USERS {
        let coord = coord.clone();
        let leases = leases.clone();
        let backend = backend.clone();
        let uri = uri.clone();
        let base_version = base_version.clone();
        let who = Principal::new_unchecked(format!("CONTOSO\\user{i}"));
        let sess = SessionId::new_unchecked(format!("sess-pilot-u{i}"));
        let cfg = cfg.clone();
        handles.push(tokio::spawn(async move {
            // Read to earn the receipt for base_version.
            let r = read(&coord, backend.as_file_source(), backend.kind(), &cfg, &who, &sess, &uri).await;
            if let Err(e) = r {
                return (i, Err(e));
            }
            let payload = format!("written-by-user{i}").into_bytes();
            let res = write(
                &coord,
                &leases,
                backend.clone(),
                &who,
                &sess,
                &uri,
                payload,
                base_version,
                WriteMode::Cas,
            )
            .await;
            (i, res.map(|_| ()))
        }));
    }

    let mut winners = Vec::new();
    let mut conflicts = Vec::new();
    let mut lease_held = Vec::new();
    let mut stale_read = Vec::new();
    let mut other_errs = Vec::new();
    for h in handles {
        let (i, res) = h.await?;
        match res {
            Ok(()) => winners.push(i),
            Err(ChaprError::Conflict { sidecar_path, .. }) => {
                let parked = std::fs::read_to_string(sidecar_path.as_str())
                    .map(|s| s == format!("written-by-user{i}"))
                    .unwrap_or(false);
                conflicts.push((i, parked, sidecar_path.as_str().to_string()));
            }
            Err(ChaprError::LeaseHeld { holder, .. }) => lease_held.push((i, holder.to_string())),
            // Legitimate third outcome, not a defect: this user's concurrent read
            // observed the winner's newer bytes, so it never read the common base
            // it was asked to write against. Refusing is correct.
            Err(ChaprError::BaseVersionNotRecorded { .. }) => stale_read.push(i),
            Err(e) => other_errs.push((i, e.to_string())),
        }
    }

    println!("  winners: {winners:?}");
    println!("  CAS conflicts (bytes parked in a sidecar): {}", conflicts.len());
    for (i, parked, path) in &conflicts {
        println!("    user{i} -> {path} (bytes recovered: {parked})");
    }
    println!("  refused on lease (no retry, no queue): {}", lease_held.len());
    for (i, holder) in &lease_held {
        println!("    user{i} blocked by {holder}");
    }
    println!("  refused: read landed after the winner's write: {:?}", stale_read);
    if !other_errs.is_empty() {
        println!("  other errors: {}", other_errs.len());
        for (i, e) in &other_errs {
            println!("    user{i}: {e}");
        }
    }

    let disk = std::fs::read_to_string(canon.as_str()).unwrap_or_default();

    macro_rules! check {
        ($label:expr, $cond:expr) => {
            if $cond {
                pass += 1;
                println!("  PASS  {}", $label);
            } else {
                fail += 1;
                println!("  FAIL  {}", $label);
            }
        };
    }

    check!("exactly one writer won", winners.len() == 1);
    check!(
        "every loser was told (none silently dropped)",
        conflicts.len() + lease_held.len() + stale_read.len() + other_errs.len()
            == USERS - winners.len()
    );
    check!("no unexpected error kinds", other_errs.is_empty());
    check!(
        "file is exactly one writer's bytes, not a mix",
        winners.len() == 1 && disk == format!("written-by-user{}", winners[0])
    );
    check!(
        "every CAS loser's bytes are recoverable from its sidecar",
        conflicts.iter().all(|(_, parked, _)| *parked)
    );

    // Only CAS losers get a sidecar. Anyone refused BEFORE reaching the CAS
    // compare — on the lease, or on read-before-write — has no sidecar at all,
    // so their bytes survive only in the caller's hands and there is no retry
    // anywhere in the endpoint. Correct, but lossy from the user's seat: report
    // it rather than assert on it.
    let pre_cas = lease_held.len() + stale_read.len();
    if pre_cas > 0 {
        println!(
            "  NOTE  {pre_cas} of {USERS} users were refused BEFORE the CAS compare and have\n        \
             NO sidecar. Their bytes survive only in the caller's hands; retrying is\n        \
             entirely the caller's job. Only the {} CAS loser(s) had bytes parked.",
            conflicts.len()
        );
    }

    // ---------------------------------------------------------------------
    // Part 3 — two conflicts by the SAME principal inside one second.
    // sidecar_path stamps %Y%m%d%H%M%S with no uniqueness suffix, so the second
    // conflict computes an identical filename. create_new then refuses, and the
    // `?` in write_cas_core returns AlreadyExists WITHOUT parking the bytes.
    // Part 2's real conflicts landed in the same second, so this is reachable.
    // ---------------------------------------------------------------------
    println!("\n== part 3: two same-principal conflicts in one second ==");
    let uri = format!("{dir}/pilot_collide.txt");
    let canon = canonicalize(&uri, g)?;
    let _ = std::fs::remove_file(canon.as_str());

    let cs = SessionId::new_unchecked("sess-collide");
    let base = ops::create(&coord, &leases, backend.clone(), &who, &cs, &uri, b"base".to_vec())
        .await?
        .version;

    // Advance the file so every write from `base` is stale.
    let r0 = read(&coord, backend.as_file_source(), backend.kind(), &cfg, &who, &cs, &uri).await?;
    write(
        &coord,
        &leases,
        backend.clone(),
        &who,
        &cs,
        &uri,
        b"advanced".to_vec(),
        r0.version.clone().unwrap(),
        WriteMode::Cas,
    )
    .await?;

    let mut parked = Vec::new();
    let mut collided = 0u32;
    for n in 0..2 {
        let payload = format!("loser-{n}").into_bytes();
        match write(
            &coord,
            &leases,
            backend.clone(),
            &who,
            &cs,
            &uri,
            payload,
            base.clone(),
            WriteMode::Cas,
        )
        .await
        {
            Err(ChaprError::Conflict { sidecar_path, .. }) => {
                let ok = std::fs::read_to_string(sidecar_path.as_str())
                    .map(|s| s == format!("loser-{n}"))
                    .unwrap_or(false);
                println!("  loser-{n}: parked at {} (recovered: {ok})", sidecar_path.as_str());
                parked.push(ok);
            }
            Err(ChaprError::AlreadyExists { .. }) => {
                collided += 1;
                println!("  loser-{n}: ALREADY_EXISTS — sidecar name collided, bytes NOT parked");
            }
            Err(e) => println!("  loser-{n}: other error: {e}"),
            Ok(_) => println!("  loser-{n}: unexpectedly SUCCEEDED (base should be stale)"),
        }
    }

    check!(
        "both same-second conflicts parked their bytes recoverably",
        collided == 0 && parked.len() == 2 && parked.iter().all(|p| *p)
    );
    if collided > 0 {
        println!(
            "  NOTE  {collided} conflict(s) lost their bytes to a sidecar name collision.\n        \
             base_version was valid and the user was told ALREADY_EXISTS, which names\n        \
             the wrong problem entirely."
        );
    }

    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
    Ok(())
}
