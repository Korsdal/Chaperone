// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! # chapr-endpoint — the local MCP server (foundation slice)
//!
//! One instance per sales laptop, a stdio child of Claude Desktop. It does SMB
//! I/O as the logged-in user and owns the exclusive-open write path, the read
//! state machine, and the lease-renewal thread. Built last (implementation
//! notes §4), once the protocol and coord are proven — which they now are.
//!
//! ## This slice (E-005 slice 1)
//!
//! The crate is a **library** for now, holding the two pieces that need neither
//! Win32 nor MCP and are the foundation everything else sits on:
//!
//! - [`canon`] — path canonicalisation (concept §5.1), the sole minter of
//!   [`chapr_proto::CanonicalPath`].
//! - [`coord_client`] — a typed HTTP client for coord's control channel, with
//!   the `ChaprError` round-trip and the `CoordUnreachable` mapping the
//!   availability rules (concept §10) depend on.
//!
//! ## Later slices
//!
//! - the `rmcp` stdio server (this crate gains a `[[bin]]`);
//! - the read state machine (concept §8) + the untrusted-data envelope (§13.3);
//! - the **boring** write path (concept §7) via `windows-rs` `CreateFileW` with
//!   `FILE_SHARE_NONE` — the crown jewel, kept deliberately dull;
//! - DFS + drive-letter→UNC resolution folded into [`canon`];
//! - the lease-renewal timer thread (the async-ownership tax, notes §5).

pub mod backend;
pub mod canon;
pub mod coord_client;
pub mod diag;
pub mod hostconfig;
pub mod identity;
pub mod lease_manager;
pub mod mount;
pub mod moverecover;
pub mod nearname;
pub mod ops;
pub mod pathgrammar;
pub mod pathlock;
pub mod posixfs;
pub mod read;
pub mod selftest;
pub mod server;
pub mod sniff;
pub mod traceprobe;
#[cfg(windows)]
pub mod winfs;
pub mod write;

pub use backend::{
    default_backend_kind, make_backend, Backend, Capabilities, PosixBackend,
};
#[cfg(windows)]
pub use backend::SmbBackend;
pub use canon::canonicalize;
pub use pathgrammar::{grammar_for, PathGrammar};
pub use coord_client::CoordClient;
pub use lease_manager::LeaseManager;
pub use read::{list, read, stat, FileSource, ReadConfig};
pub use server::ChaprServer;
pub use write::write;
