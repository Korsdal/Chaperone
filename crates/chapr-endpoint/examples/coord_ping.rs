// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! A tiny end-to-end exercise of [`chapr_endpoint::CoordClient`] against a
//! running coord. Doubles as a dev smoke test that the shared proto types
//! round-trip across the real HTTP boundary.
//!
//! Run against a live coord:
//! ```text
//! CHAPR_COORD_URL=http://127.0.0.1:8787 cargo run -p chapr-endpoint --example coord_ping
//! ```

use chapr_endpoint::{canonicalize, grammar_for, CoordClient};
use chapr_proto::{
    AcquireLeaseRequest, AppendVersionLogRequest, BackendKind, HistoryQuery, LeasePurpose,
    Principal, ResolveRequest, SessionId, VersionEvent,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::var("CHAPR_COORD_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
    let client = CoordClient::new(&base);
    let who = Principal::new_unchecked("CONTOSO\\demo");

    // Canonicalise a raw path the way a tool call would arrive.
    let path = canonicalize("//SRV/Share/ping.md", grammar_for(BackendKind::Smb))?;
    println!("canonicalize  -> {path}");

    let acq = client
        .lease_acquire(&AcquireLeaseRequest {
            principal: who.clone(),
            session_id: SessionId::new_unchecked("sess-ping"),
            purpose: LeasePurpose::Write,
            paths: vec![path.clone()],
        })
        .await?;
    println!("lease_acquire -> {} (ttl {}s)", acq.lease_id, acq.ttl_s);

    let content = b"ping body".to_vec();
    let blob = client.put_blob(content.clone()).await?;
    println!("put_blob      -> {} dedup={}", blob.version, blob.deduplicated);

    let got = client.get_blob(&blob.version).await?;
    println!("get_blob      -> {} bytes, match={}", got.len(), got == content);

    let entry = client
        .append_version_log(&AppendVersionLogRequest {
            path: path.clone(),
            blob_hash: blob.version.clone(),
            writer_principal: who.clone(),
            size: content.len() as u64,
            event: VersionEvent::Create,
            pre_image: None,
        })
        .await?;
    println!("version_log   -> event={:?} prev={:?}", entry.event, entry.prev_hash);

    let resolved = client
        .resolve(&ResolveRequest {
            path: path.clone(),
            mtime: entry.timestamp,
            size: content.len() as u64,
        })
        .await?;
    println!(
        "resolve       -> journal={:?} lease_held_by={:?}",
        resolved.journal_state,
        resolved.lease_state.map(|l| l.principal)
    );

    let hist = client.history(&HistoryQuery { path: path.clone() }).await?;
    println!("history       -> {} entries", hist.entries.len());

    client.lease_release(&acq.lease_id).await?;
    println!("lease_release -> ok");

    Ok(())
}
