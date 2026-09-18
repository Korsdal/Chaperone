// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Full-surface live smoke test: drives every mutation/read verb through the
//! **real** selected backend (SMB `winfs` on Windows, or POSIX `posixfs`
//! advisory-`flock` on Linux) against a running coord and real files on disk.
//! This is the behaviour proof the unit tests — which mock the filesystem —
//! cannot give.
//!
//! ```text
//! # Windows (SMB):
//! CHAPR_COORD_URL=http://127.0.0.1:8899 SMOKE_DIR=C:/tmp/smoke \
//!   cargo run -p chapr-endpoint --example smoke_parts
//! # Linux (POSIX):
//! CHAPR_BACKEND=posix CHAPR_COORD_URL=http://127.0.0.1:8899 SMOKE_DIR=/tmp/smoke \
//!   cargo run -p chapr-endpoint --example smoke_parts
//! ```

use chapr_endpoint::backend::{default_backend_kind, make_backend};
use chapr_endpoint::{canonicalize, grammar_for, ops, read, write, CoordClient, LeaseManager, ReadConfig};
use chapr_proto::{BackendKind, Principal, ReadContent, RestoreMode, SessionId, WriteMode};
use std::sync::Arc;

fn body(resp: &chapr_proto::ReadResponse) -> String {
    match &resp.content {
        ReadContent::Inline { bytes } => String::from_utf8_lossy(bytes).into_owned(),
        _ => "<ref>".into(),
    }
}

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
    let who = Principal::new_unchecked("CONTOSO\\smoke");
    let sess = SessionId::new_unchecked("sess-smoke");
    let cfg = ReadConfig::default();
    println!("backend = {kind}");

    let a = format!("{dir}/smoke_a.txt");
    let a2 = format!("{dir}/smoke_a_moved.txt");
    let disk = |uri: &str| std::fs::read_to_string(canonicalize(uri, g).unwrap().as_str());
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

    // Clean any leftovers from a prior run.
    let _ = std::fs::remove_file(canonicalize(&a, g).unwrap().as_str());
    let _ = std::fs::remove_file(canonicalize(&a2, g).unwrap().as_str());

    // create
    let created = ops::create(&coord, &leases, backend.clone(), &who, &sess, &a, b"v1".to_vec()).await?;
    check!("create returns version", !created.version.to_string().is_empty());
    check!("create wrote v1 to disk", disk(&a).ok().as_deref() == Some("v1"));
    let v1 = created.version.clone();

    // read back
    let r = read(&coord, backend.as_file_source(), backend.kind(), &cfg, &who, &sess, &a).await?;
    check!("read serves v1", body(&r) == "v1");
    check!("read version == create version", r.version.as_ref() == Some(&v1));

    // write v2 with the correct base_version
    write(&coord, &leases, backend.clone(), &who, &sess, &a, b"v2".to_vec(), v1.clone(), WriteMode::Cas).await?;
    check!("write v2 succeeds", disk(&a).ok().as_deref() == Some("v2"));

    // stale write → CONFLICT + sidecar parked (bytes never lost)
    let conflict = write(&coord, &leases, backend.clone(), &who, &sess, &a, b"v3-loser".to_vec(), v1.clone(), WriteMode::Cas).await;
    let sidecar_ok = match &conflict {
        Err(chapr_proto::ChaprError::Conflict { sidecar_path: Some(sidecar_path), .. }) => {
            std::fs::read_to_string(sidecar_path.as_str()).ok().as_deref() == Some("v3-loser")
        }
        _ => false,
    };
    check!("stale write is a CONFLICT", matches!(conflict, Err(chapr_proto::ChaprError::Conflict { .. })));
    check!("loser bytes parked in sidecar", sidecar_ok);
    check!("disk still v2 after conflict", disk(&a).ok().as_deref() == Some("v2"));

    // history has entries
    let hist = coord
        .history(&chapr_proto::HistoryQuery { path: canonicalize(&a, g)? })
        .await?;
    check!("history has >=2 entries", hist.entries.len() >= 2);

    // restore v1 as a copy (no clobber)
    let rest = ops::restore(&coord, &leases, backend.clone(), &who, &sess, &a, v1.clone(), RestoreMode::Copy, None).await?;
    let restored_ok = rest.restored_path.as_ref().map(|p| std::fs::read_to_string(p.as_str()).ok().as_deref() == Some("v1")).unwrap_or(false);
    check!("restore copy contains v1", restored_ok);
    check!("restore left live file at v2", disk(&a).ok().as_deref() == Some("v2"));

    // D-050 chains, against the REAL read-before-write check: every version a
    // verb returns must be accepted as base_version with no read in between.
    // These three were refused before 0.1.5 (restore recorded nothing; move
    // recorded nothing and dropped its version at the tool boundary).
    let copy_uri = rest.restored_path.as_ref().map(|p| p.as_str().to_string()).unwrap_or_default();
    let chain_copy = write(&coord, &leases, backend.clone(), &who, &sess, &copy_uri, b"copy-edited".to_vec(), rest.version.clone(), WriteMode::Cas).await;
    check!("chain: write to restore-copy with returned version, no read", chain_copy.is_ok());
    let _ = std::fs::remove_file(&copy_uri);

    // restore v1 in place: base is the live version (v2, from the last write).
    let v2 = write(&coord, &leases, backend.clone(), &who, &sess, &a, b"v2".to_vec(), v1.clone(), WriteMode::Force { reason: "smoke: reset to v2".into() }).await?.version;
    let rip = ops::restore(&coord, &leases, backend.clone(), &who, &sess, &a, v1.clone(), RestoreMode::InPlace, Some(chapr_proto::RestoreBase::Version(v2))).await?;
    check!("restore in place lands v1", disk(&a).ok().as_deref() == Some("v1"));
    let chain_rip = write(&coord, &leases, backend.clone(), &who, &sess, &a, b"v3".to_vec(), rip.version.clone(), WriteMode::Cas).await;
    check!("chain: write after in-place restore with returned version, no read", chain_rip.is_ok());
    let v3 = chain_rip.map(|r| r.version).unwrap_or(v1.clone());

    // history shows the forced write as write_forced (I-020: was rendered `write`)
    let hist = coord
        .history(&chapr_proto::HistoryQuery { path: canonicalize(&a, g)? })
        .await?;
    check!(
        "history renders the forced write as write_forced",
        hist.entries.iter().any(|e| e.event == chapr_proto::VersionEvent::WriteForced)
    );

    // move a -> a2 on the version the write returned, no read
    let moved = ops::mv(&coord, &leases, backend.clone(), &who, &sess, &a, &a2, v3, None).await?;
    check!("move: source gone", disk(&a).is_err());
    check!("move: dest has v3", disk(&a2).ok().as_deref() == Some("v3"));

    // chain: write to the destination with the version move returned, no read
    let chain_mv = write(&coord, &leases, backend.clone(), &who, &sess, &a2, b"v4".to_vec(), moved.version.clone(), WriteMode::Cas).await;
    check!("chain: write after move with returned version, no read", chain_mv.is_ok());
    let vdel = chain_mv.map(|r| r.version).unwrap_or(moved.version);

    // delete a2 on the version the write returned
    ops::delete(&coord, &leases, backend.clone(), &who, &sess, &a2, vdel).await?;
    check!("delete removes the file", disk(&a2).is_err());

    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
    Ok(())
}
