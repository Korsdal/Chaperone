// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Fault-injection regression probe for the commit-tail lease leak (now fixed).
//!
//! `write()` releases the lease at the very bottom. The post-close tail above it
//! used `?` directly in the function body, and `?` returns from the *function* —
//! so a coord failure after the bytes were already on disk skipped the release
//! entirely, and the background renewer kept the lease alive to the 20-minute
//! hard ceiling while every other session got `LEASE_HELD`. Reachable in a real
//! deployment via a coord restart, a network blip, or a `SQLITE_BUSY` that coord
//! maps to `Internal` -> HTTP 500.
//!
//! The tail now lives in an inner `async` block, so `?` exits the block and the
//! release always runs; and the failure surfaces as
//! `ChaprError::CommittedButUnrecorded`, which tells the caller its bytes ARE
//! live and it must re-read rather than retry the write.
//!
//! Expected result after the fix: the "bytes on disk" check passes, the lease
//! probe acquires cleanly, and no lease row survives in coord's database.
//!
//! Needs two URLs: a fault proxy that 500s `POST /version-log`, and the real
//! coord for setup and for observing the leaked lease.
//!
//! ```text
//! CHAPR_COORD_URL=http://127.0.0.1:8899 \
//! CHAPR_FAULT_URL=http://127.0.0.1:8898 \
//! SMOKE_DIR=/tmp/smoke CHAPR_BACKEND=posix \
//!   cargo run -p chapr-endpoint --example smoke_lease_leak
//! ```

use chapr_endpoint::backend::{default_backend_kind, make_backend};
use chapr_endpoint::{canonicalize, grammar_for, ops, read, write, CoordClient, LeaseManager, ReadConfig};
use chapr_proto::{
    AcquireLeaseRequest, BackendKind, ChaprError, LeasePurpose, Principal, SessionId, WriteMode,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let real = std::env::var("CHAPR_COORD_URL").unwrap_or_else(|_| "http://127.0.0.1:8899".into());
    let fault = std::env::var("CHAPR_FAULT_URL").expect("set CHAPR_FAULT_URL to the proxy");
    let dir = std::env::var("SMOKE_DIR").expect("set SMOKE_DIR to a writable local dir");
    let kind = match std::env::var("CHAPR_BACKEND") {
        Ok(v) => v.parse::<BackendKind>().unwrap_or_else(|_| default_backend_kind()),
        Err(_) => default_backend_kind(),
    };

    let coord_real = CoordClient::new(&real);
    let coord_fault = CoordClient::new(&fault);
    // The manager under test talks through the proxy, exactly as a real endpoint
    // would if coord started failing mid-operation.
    let leases_fault = Arc::new(LeaseManager::new(coord_fault.clone()));
    leases_fault.clone().spawn_renewer();
    let leases_real = Arc::new(LeaseManager::new(coord_real.clone()));

    let backend = make_backend(kind).expect("backend available for this OS");
    let g = grammar_for(kind);
    let cfg = ReadConfig::default();
    let who = Principal::new_unchecked("CONTOSO\\leaker");
    let sess = SessionId::new_unchecked("sess-leak");

    let uri = format!("{dir}/leak_probe.txt");
    let canon = canonicalize(&uri, g)?;
    let _ = std::fs::remove_file(canon.as_str());

    let mut pass = 0u32;
    let mut fail = 0u32;
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

    println!("backend = {kind}");
    println!("\n== setup via the REAL coord ==");
    let created = ops::create(&coord_real, &leases_real, backend.clone(), &who, &sess, &uri, b"v1".to_vec()).await?;
    println!("  created at version {}", created.version);

    // Read through the real coord so the receipt exists for the write below.
    let r = read(&coord_real, backend.as_file_source(), backend.kind(), &cfg, &who, &sess, &uri).await?;
    let base = r.version.clone().expect("read produced a version");

    println!("\n== write through the FAULT proxy (POST /version-log -> 500) ==");
    let res = write(
        &coord_fault,
        &leases_fault,
        backend.clone(),
        &who,
        &sess,
        &uri,
        b"v2-committed-to-disk".to_vec(),
        base,
        WriteMode::Cas,
    )
    .await;

    match &res {
        Ok(_) => println!("  write reported SUCCESS (fault did not land)"),
        Err(e) => println!("  write reported failure: {e}"),
    }

    let on_disk = std::fs::read_to_string(canon.as_str()).unwrap_or_default();
    println!("  disk now contains: {on_disk:?}");

    // The crux: the bytes are committed, so the operation really happened, but
    // the caller was told it failed.
    check!(
        "bytes ARE on disk even though the write reported failure",
        on_disk == "v2-committed-to-disk" && res.is_err()
    );

    println!("\n== is the lease still held? (ask the REAL coord directly) ==");
    let probe = coord_real
        .lease_acquire(&AcquireLeaseRequest {
            principal: Principal::new_unchecked("CONTOSO\\someone-else"),
            session_id: SessionId::new_unchecked("sess-other"),
            purpose: LeasePurpose::Write,
            paths: vec![canon.clone()],
        })
        .await;

    match &probe {
        Err(ChaprError::LeaseHeld { holder, .. }) => {
            println!("  LEAKED: path still leased by {holder}");
            println!("  another user cannot write this file until the 20-min hard cap expires");
        }
        Ok(l) => {
            println!("  lease acquired cleanly ({}) — no leak on this path", l.lease_id);
            let _ = coord_real.lease_release(&l.lease_id).await;
        }
        Err(e) => println!("  unexpected: {e}"),
    }

    check!(
        "lease was released despite the commit-tail failure",
        probe.is_ok()
    );

    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
    Ok(())
}
