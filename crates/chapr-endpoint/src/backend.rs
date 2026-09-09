// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The `Backend` seam: the abstraction that lets one per-OS MCP bundle drive
//! several fileserver backends, chosen at runtime.
//!
//! §14 is explicit that the adapter **declares capabilities honestly** rather
//! than faking a uniform abstraction: the only primitive synthesizable
//! everywhere is *version token + compare-and-swap write*; everything else
//! degrades per backend.
//!
//! ## The shared §7 core (E-019, decision D-D)
//!
//! The entire under-one-handle §7 atomic core (invariant 4) lives **once**, in
//! the private generic `*_core` functions below, parameterised over two seams:
//!
//! - [`FsPrimitives`] — the backend's exclusive-open ([`LockedFile`]), create-new
//!   primitives, plus a rename that runs through the held handle (SMB =
//!   `CreateFileW share=NONE` + `SetFileInformationByHandle`; POSIX =
//!   `open`+advisory-`flock` + `rename`).
//! - [`crate::pathgrammar::PathGrammar`] — separator, casefold, and the
//!   human-lock / sidecar / restored name conventions.
//!
//! So the correctness-critical ordering (version-check → CAS → journal → snapshot
//! → in-place write → clear) is written once and every backend shares it; a new
//! backend supplies only primitives + a grammar. The core stays **synchronous**
//! (it runs inside `spawn_blocking`) and drives coord durability via
//! [`WriteCtx`]'s `block_on`, so the held handle never crosses an `.await`.
//!
//! The intent journal (steps 7/11) is capability-gated on
//! [`Capabilities::atomic_writes`]: an atomic backend cannot tear a write, so the
//! journal is meaningless there. SMB (`atomic_writes == false`) always takes the
//! gate, so behaviour is byte-identical to the pre-refactor path. POSIX in-place
//! writes are likewise non-atomic, so the journal runs there too.
//!
//! The tool-layer scaffolding around the core — lease acquire/release,
//! read-before-write assert, and the post-close `append_version_log` +
//! `record_audit` + `record_read` tail — stays uniform across backends.

use crate::coord_client::CoordClient;
use crate::pathgrammar::grammar_for;
use crate::read::{system_time_to_utc, FileSource, FileStat, RawDirEntry};
#[cfg(windows)]
use crate::winfs::{create_new_file, ExclusiveFile};
use chapr_proto::{
    BackendDescriptor, BackendKind, CanonicalPath, ChaprError, ClearJournalRequest,
    ClearMoveJournalRequest, HistoryQuery, LeaseId, MovePathsRequest, OpenJournalRequest,
    OpenMoveJournalRequest, PreImage, Principal, ReadReceipt, RegisterConflictRequest,
    RestoreBase, SessionId, VersionToken, WriteMode,
};
use chrono::{DateTime, Utc};
use std::io;
use tokio::runtime::Handle;

/// Largest pre-image the write path will snapshot to coord.
///
/// Must not exceed coord's `PUT /blobs` body limit (`chapr_coord::http::
/// MAX_BLOB_BYTES`), which is the real ceiling; this mirrors it so the refusal
/// happens locally with a message that explains *why* a big file cannot be
/// written, instead of a bare 413 from the middle of the write.
pub const MAX_PRE_IMAGE_BYTES: usize = 256 * 1024 * 1024;

/// What a backend can and cannot do (§14). Endpoint-internal — never crosses the
/// wire (coord is already backend-agnostic). Drives exactly one behavioural
/// branch: the journal gate on [`Self::atomic_writes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The backend has an atomic conditional-write primitive (S3/Azure). SMB and
    /// POSIX synthesize CAS via exclusive-open + re-hash instead.
    pub native_cas: bool,
    /// The backend has its own lease with a TTL (Azure Blob); otherwise leases
    /// are advisory-via-coord.
    pub native_lease: bool,
    /// The lock excludes **non-Chaperone** writers too (SMB `share=NONE` — the
    /// gold standard). Advisory-only backends (POSIX `flock`) report `false`.
    pub mandatory_lock: bool,
    /// A write cannot tear (atomic backends). **The one gate that changes
    /// behaviour**: when `true`, the intent journal is skipped.
    pub atomic_writes: bool,
    /// How the backend surfaces changes (a watcher concern; descriptive here).
    pub change_notify: ChangeNotify,
    /// The version-token shape. Descriptive only — `VersionToken` stays BLAKE3
    /// (invariant 2 unrelaxed until a cloud backend lands, E-021).
    pub token_kind: TokenKind,
}

/// The change-notification mechanism a backend offers (descriptive; the watcher
/// itself lives on coord).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeNotify {
    ReadDirectoryChangesW,
    Inotify,
    Webhook,
    None,
}

/// The version-token shape a backend uses (descriptive this cycle).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Blake3,
    ETag,
}

/// Coord-facing context a backend drives from inside its blocking sequence, so
/// the durability calls stay ordered under the held handle (invariant 4).
pub struct WriteCtx<'a> {
    pub rt: &'a Handle,
    pub coord: &'a CoordClient,
    pub principal: &'a Principal,
    pub session_id: &'a SessionId,
}

/// What a successful atomic-core write reports back so the **tool layer** can do
/// the uniform post-close scaffolding (version log + audit).
#[derive(Debug)]
pub struct CommitReceipt {
    pub from_version: Option<VersionToken>, // None for create
    pub to_version: Option<VersionToken>,   // None for delete
    pub size: u64,
    /// Size of the pre-image this operation snapshotted into the blob store, when
    /// it snapshotted one. With `from_version` it names the blob the endpoint
    /// uploaded — which the version log must reference or GC reclaims it. Coord
    /// cannot derive the size itself (it does no file I/O, invariant 1).
    pub from_size: Option<u64>,
}

/// A move reports back the version that landed at the destination. Its
/// version-log entry and audit are still emitted coord-side by `move_paths`
/// (D-013); this exists so the caller does not have to read the destination just
/// to obtain a version it already CAS-verified at the source.
#[derive(Debug)]
pub struct MoveReceipt {
    /// The source's CAS-verified version, unchanged by the rename.
    pub version: VersionToken,
}

/// Args for the contended §7 write (`write_cas`).
pub struct WriteCasArgs {
    pub path: CanonicalPath,
    pub lease_id: LeaseId,
    pub content: Vec<u8>,
    pub base_version: VersionToken,
    pub mode: WriteMode,
}

/// Args for a soft delete (`delete_cas`).
pub struct DeleteCasArgs {
    pub path: CanonicalPath,
    pub lease_id: LeaseId,
    pub base_version: VersionToken,
}

/// Args for an in-place restore (`restore_in_place`).
pub struct RestoreInPlaceArgs {
    pub path: CanonicalPath,
    pub lease_id: LeaseId,
    pub bytes: Vec<u8>,
    pub version: VersionToken,
    /// What the caller observed at the target. Restore performed no CAS at all
    /// — "a deliberate overwrite" — which made it the one verb able to destroy
    /// content the agent had never read. Now it must say what it saw.
    pub base: RestoreBase,
}

/// Args for a move/rename (`move_cas`).
pub struct MoveCasArgs {
    pub src: CanonicalPath,
    pub dst: CanonicalPath,
    pub src_base_version: VersionToken,
    pub dst_base_version: Option<VersionToken>,
    /// The all-or-none `{src, dst}` lease this move holds.
    ///
    /// This used to be absent, with the note "move does not journal" — which was
    /// true and was the bug B3 fixed. The move journal records the lease for the
    /// same reason the write journal does: its liveness is what separates a move
    /// still running from one that died owing coord a migration.
    pub lease_id: LeaseId,
}

/// A file held open exclusively, from version-check to close (invariant 4).
/// Blocking (runs inside `spawn_blocking`); the concrete type RAII-closes on drop
/// (SMB via `CloseHandle`, POSIX via `flock(LOCK_UN)` + close).
pub trait LockedFile {
    /// Read the whole file under the lock (to re-hash for CAS).
    fn read_all(&self) -> io::Result<Vec<u8>>;
    /// Overwrite from the start, truncate to the new length, and flush. In place
    /// — never temp-rename (a rename carries the source ACL and strips the
    /// target's; concept §7 step 9).
    fn overwrite(&self, bytes: &[u8]) -> io::Result<()>;
    /// Rename this file to `dst` **without releasing the lock** (I-007,
    /// invariant 4). `replace` allows overwriting an existing `dst`.
    ///
    /// The move path's version-check and its mutation must share one handle for
    /// the same reason the write path's do: closing in between opens a window in
    /// which another writer's bytes land in the file and are then renamed away
    /// with no snapshot and no conflict. Both backends can do this — Win32 via
    /// `SetFileInformationByHandle(FileRenameInfo)`, POSIX because `rename(2)`
    /// operates on the directory entry and never needed the fd closed — so it
    /// belongs on the seam rather than in either backend's adapter.
    fn rename_to(&self, dst: &str, replace: bool) -> io::Result<()>;
}

/// The per-backend filesystem primitives the shared §7 core drives. Kept separate
/// from [`Backend`] (which must stay object-safe for `Arc<dyn Backend>`) because
/// this trait has an associated type. Each concrete backend implements both.
pub trait FsPrimitives: Send + Sync {
    /// The backend's held-exclusive-file type.
    type File: LockedFile;
    /// Which backend this is — the core resolves the grammar via [`grammar_for`].
    fn kind(&self) -> BackendKind;
    /// Open an existing file exclusively (contention → an `io::Error` mapping to
    /// [`ChaprError::SharingViolation`] via [`map_os_err`]).
    fn open_existing(&self, path: &str) -> io::Result<Self::File>;
    /// Create a brand-new file (fails if it exists) — sidecar / restore-copy /
    /// create. Uniquely named, so no lock is needed.
    fn create_new(&self, path: &str, bytes: &[u8]) -> io::Result<()>;
    /// Does this path exist at all, as a file or a directory?
    ///
    /// Deliberately not a stat: the two callers ask a yes/no question — is the
    /// parent there ([`create_err`]), and does the target still exist (the
    /// restore CAS) — and a metadata struct they would discard invites branching
    /// on fields nobody checked.
    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }
    /// Create one directory. Non-recursive: a missing parent is an error, which
    /// the core turns into [`ChaprError::ParentMissing`].
    fn create_dir(&self, path: &str) -> io::Result<()> {
        std::fs::create_dir(path)
    }
}

// There is deliberately **no** `rename(src, dst)` on this seam. A rename that
// takes two paths can only be reached by closing the exclusive handle first,
// which is precisely the invariant-4 violation I-007 recorded: between the close
// and the rename, another writer's bytes can land in the source and be moved to
// the destination unnoticed, with no snapshot and no conflict. Renaming is a
// method on the *held file* ([`LockedFile::rename_to`]) so that the type system
// makes the safe order the only expressible one. If a future backend genuinely
// cannot rename through a handle, give it an explicit capability flag and a
// documented degradation — do not put the two-path version back here.

/// A fileserver backend. `Backend: FileSource` (supertrait) so a backend also
/// serves the read path; the narrow `FileSource` seam stays separate to keep the
/// read state machine unit-testable against a tiny mock.
///
/// The mutation methods delegate to the shared generic `*_core` functions, so
/// each impl is a thin adapter over its [`FsPrimitives`].
pub trait Backend: FileSource + Send + Sync {
    fn capabilities(&self) -> Capabilities;

    /// Which backend kind this is — the local authority the endpoint cross-checks
    /// coord's advisory announcement against (invariant 3, decision D-A).
    fn kind(&self) -> BackendKind;

    /// Upcast to the read sub-seam. A manual upcast because trait-object
    /// upcasting is not stable on this crate's Rust version; each impl returns
    /// `self`.
    fn as_file_source(&self) -> &dyn FileSource;

    /// The contended §7 core. One held exclusive handle from version-check to
    /// close. On CAS mismatch returns `Err(ChaprError::Conflict{..})` having
    /// already parked the losing bytes in a sidecar.
    fn write_cas(&self, ctx: &WriteCtx, args: &WriteCasArgs) -> Result<CommitReceipt, ChaprError>;

    /// Create a new file (atomic exists-check). No pre-image.
    fn create(
        &self,
        ctx: &WriteCtx,
        path: &CanonicalPath,
        content: &[u8],
    ) -> Result<CommitReceipt, ChaprError>;

    /// Soft delete: snapshot the pre-image, then remove. CAS on `base_version`.
    fn delete_cas(&self, ctx: &WriteCtx, args: &DeleteCasArgs)
        -> Result<CommitReceipt, ChaprError>;

    /// Restore old bytes into a fresh uniquely-named sibling (no lease). Returns
    /// the path written.
    fn restore_copy(
        &self,
        ctx: &WriteCtx,
        path: &CanonicalPath,
        bytes: &[u8],
        version: &VersionToken,
    ) -> Result<CanonicalPath, ChaprError>;

    /// Restore old bytes in place (the full contended path).
    fn restore_in_place(
        &self,
        ctx: &WriteCtx,
        args: &RestoreInPlaceArgs,
    ) -> Result<CommitReceipt, ChaprError>;

    /// Atomic dual-lease rename with CAS on both sides. Coord state is migrated
    /// server-side (`move_paths`), so nothing is returned for the tool layer.
    fn move_cas(&self, ctx: &WriteCtx, args: &MoveCasArgs) -> Result<MoveReceipt, ChaprError>;
    /// Create one directory (`chapr.mkdir`).
    ///
    /// Not part of the §7 write core and deliberately outside it: a directory has
    /// no content, so there is no version to compare, no pre-image to snapshot
    /// and nothing a journal could recover. Routing it through the write path
    /// would mean inventing all three.
    ///
    /// The near-name guard lives one layer up in [`crate::ops::mkdir`], where the
    /// listing it needs is already available.
    fn mkdir(&self, path: &CanonicalPath) -> Result<(), ChaprError>;
}

// ---------------------------------------------------------------------------
// The shared §7 core — one copy of the correctness-critical ordering, generic
// over the backend's FsPrimitives + PathGrammar (E-019 D-D).
// ---------------------------------------------------------------------------

/// Whether `content` is the base64 transcript of `current` — i.e. a caller read a
/// binary file, received the base64 body, and wrote it straight back as TEXT
/// without declaring `encoding: "base64"`.
///
/// **Whitespace-tolerant on purpose.** The original check compared
/// `content.len()` to the exact padded base64 length, which a model re-wrapping
/// the body across lines defeats — and models routinely wrap long payloads. The
/// wrapped transcript then slipped past this guard and past CAS (whose
/// `base_version` is a genuine hash of the genuine bytes) and replaced the file
/// with its own base64 listing. `decode_content` already strips whitespace on the
/// declared-base64 path; this is the same tolerance on the *undeclared* path.
///
/// Ordering is for cost, not correctness: stripping whitespace can only shrink
/// `content`, so a short body cannot be a transcript and is rejected in O(1)
/// before anything scans or allocates. A normal text write pays one length
/// comparison.
fn is_base64_transcript_of(content: &[u8], current: &[u8]) -> bool {
    let expected = current.len().div_ceil(3) * 4;
    if content.len() < expected || std::str::from_utf8(current).is_ok() {
        return false;
    }
    let compact: Vec<u8> = content
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    compact.len() == expected
        && base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &compact)
            .is_ok_and(|decoded| decoded == current)
}

/// The contended §7 write core. One held exclusive handle from version-check
/// (`read_all` → hash) to close (`overwrite` → drop), invariant 4: no reopen
/// between them, and the snapshot + journal are durable before the overwrite.
fn write_cas_core<P: FsPrimitives>(
    prims: &P,
    atomic_writes: bool,
    ctx: &WriteCtx,
    args: &WriteCasArgs,
) -> Result<CommitReceipt, ChaprError> {
    let g = grammar_for(prims.kind());
    let rt = ctx.rt;
    let coord = ctx.coord;
    let principal = ctx.principal;
    let session_id = ctx.session_id;
    let path = &args.path;
    let content = args.content.as_slice();
    let base_version = &args.base_version;

    // Step 3: human/Office lock pre-flight (backends without the convention → None).
    check_human_lock(g, path)?;

    // Step 4: exclusive open. A sharing violation means someone raced us.
    let file = prims
        .open_existing(path.as_str())
        .map_err(|e| map_os_err(path, e))?;

    // Step 5: read current bytes under the lock and hash → V_now.
    let current = file.read_all().map_err(|e| map_os_err(path, e))?;
    let v_now = VersionToken::hash(&current);
    let v_new = VersionToken::hash(content);
    let force = matches!(args.mode, WriteMode::Force { .. });

    // The pre-image has to fit through coord's blob channel. Check before doing
    // anything durable, so an oversized file is a clear refusal here rather than
    // an opaque HTTP 413 several steps later.
    if current.len() > MAX_PRE_IMAGE_BYTES {
        drop(file);
        return Err(ChaprError::Io {
            path: path.clone(),
            message: format!(
                "file is {} bytes; Chaperone snapshots the previous contents to history on \
                 every write and the coordinator accepts at most {MAX_PRE_IMAGE_BYTES} bytes",
                current.len()
            ),
        });
    }

    // Guard against a client that read a binary file — which the MCP layer hands
    // back base64-encoded — and wrote that base64 TEXT straight back without
    // declaring `encoding: "base64"`. The envelope header, the tool descriptions
    // and the server instructions all state the rule, but a caller can ignore it,
    // and the result would be silent destruction of a binary file that CAS
    // happily accepts (the base_version is a genuine hash of the genuine bytes).
    //
    // The signature is unambiguous: the incoming bytes are valid base64 whose
    // decoding is byte-for-byte the file's current contents. No caller means that.
    if is_base64_transcript_of(content, &current) {
        drop(file);
        return Err(ChaprError::Io {
            path: path.clone(),
            message: "this write is the base64 TEXT of the file's current binary contents. \
                      The read returned encoding=base64; write it back with encoding \"base64\" \
                      so the bytes are decoded, or the file would be replaced by its own \
                      base64 transcript"
                .to_string(),
        });
    }

    // Step 6: CAS.
    if !force && v_now != *base_version {
        // Conflict: park the losing bytes in a fresh sidecar and register it.
        // Neither party's bytes are lost; Office binaries are never merged.
        let sidecar = g.sidecar_path(path, principal);
        prims
            .create_new(sidecar.as_str(), content)
            .map_err(|e| map_os_err(&sidecar, e))?;
        let _entry = rt.block_on(coord.register_conflict(&RegisterConflictRequest {
            base_path: path.clone(),
            sidecar_path: sidecar.clone(),
            losing_principal: principal.clone(),
            session_id: session_id.clone(),
        }))?;
        let (last_writer, when) = last_writer_of(rt, coord, path, principal);
        drop(file); // release the exclusive handle
        return Err(ChaprError::Conflict {
            base_path: path.clone(),
            current_version: v_now,
            last_writer,
            when,
            // The one verb with losing bytes to park. `write` is why the field
            // exists; the other three raise sites leave it `None`.
            sidecar_path: Some(sidecar),
        });
    }

    // Step 7: snapshot the pre-image into the history store, keyed by V_now.
    //
    // BEFORE the journal, not after. A journal entry names `pre_image_version` as
    // the bytes recovery will serve, so if the entry exists and the blob does not,
    // a crash leaves a dangling entry pointing at nothing and the next reader gets
    // `RecoveryFailed` instead of its file. Storing first means the worst case is
    // a blob with no entry — harmless garbage the GC reclaims. `delete` and
    // `restore` already ordered it this way; `write` was the outlier.
    rt.block_on(coord.put_blob(current.clone()))?;

    // Step 8: journal intent. Also fail-closed — but this is not where
    // fail-closed *starts*: `assert_read` (in `write`) and `put_blob` (step 7)
    // both talk to coord first, so an unreachable coordinator has already
    // refused the write by the time control reaches here. Nothing is written on
    // any of the three failures.
    // Gated: only for non-atomic backends.
    if !atomic_writes {
        rt.block_on(coord.journal_open(&OpenJournalRequest {
            path: path.clone(),
            lease_id: args.lease_id.clone(),
            principal: principal.clone(),
            pre_image_version: v_now.clone(),
            intended_version: Some(v_new.clone()),
        }))?;
    }

    // Step 9–10: write in place, truncate, flush, close.
    file.overwrite(content).map_err(|e| map_os_err(path, e))?;
    drop(file);

    // Step 11: clear the journal (gated). Version log + audit are the tool
    // layer's uniform post-close tail (built from this receipt).
    //
    // NOT fatal. The bytes are durable and the handle is closed, so failing here
    // would tell the caller its write failed when it committed — the lie
    // `commit_tail` exists to prevent, except this call sits inside the core and
    // so was never covered by it. An agent acting on that lie re-writes, CAS
    // mismatches against its own bytes, and a spurious conflict sidecar appears.
    // The entry left behind is self-healing: the next read hashes the file,
    // matches it against the journal's `intended_version`, and clears it
    // (`read::serve_dangling`).
    if !atomic_writes {
        if let Err(e) =
            rt.block_on(coord.journal_clear(&ClearJournalRequest { path: path.clone() }))
        {
            tracing::warn!(
                path = %path, error = %e,
                "write committed but its journal entry could not be cleared; the next read resolves it"
            );
        }
    }

    Ok(CommitReceipt {
        from_version: Some(v_now),
        to_version: Some(v_new),
        size: content.len() as u64,
        from_size: Some(current.len() as u64),
    })
}

/// Create a new file (atomic `CREATE_NEW` — an existing file → AlreadyExists).
/// No journal (there is no pre-image to recover to); the version-log + audit tail
/// is the tool layer's.
///
/// The created content IS snapshotted, even though nothing can tear here. The
/// tool layer appends a version-log row naming this version, and a version-log
/// row whose blob is absent is a broken promise: `chapr.history` lists the
/// version and `chapr.restore` of it fails with `VersionNotFound`. Before this,
/// a file created and never re-written had no recoverable history at all.
fn create_core<P: FsPrimitives>(
    prims: &P,
    ctx: &WriteCtx,
    path: &CanonicalPath,
    content: &[u8],
) -> Result<CommitReceipt, ChaprError> {
    if content.len() > MAX_PRE_IMAGE_BYTES {
        return Err(ChaprError::Io {
            path: path.clone(),
            message: format!(
                "content is {} bytes; the coordinator accepts at most {MAX_PRE_IMAGE_BYTES}",
                content.len()
            ),
        });
    }
    // Office pre-flight, like every other mutating verb. `create` was the one
    // exception, on the reasoning that `CREATE_NEW` cannot overwrite anything —
    // true, and not the whole risk: Word and Excel hold `~$F` for a document that
    // has never been saved, so creating `F.docx` underneath one collides the
    // moment the person saves. "Humans always win" with a per-verb exception is
    // the kind of rule that gets reintroduced by the next author (B6).
    check_human_lock(grammar_for(prims.kind()), path)?;

    prims
        .create_new(path.as_str(), content)
        .map_err(|e| create_err(prims, path, e))?;
    // After the file exists: a failed snapshot must not leave a phantom create.
    // Best-effort — the bytes are on the share either way, and the tool layer's
    // version-log append is what makes the version visible.
    if let Err(e) = ctx.rt.block_on(ctx.coord.put_blob(content.to_vec())) {
        tracing::warn!(
            path = %path, error = %e,
            "created file but could not snapshot it to history; restore of this version will fail"
        );
    }
    Ok(CommitReceipt {
        from_version: None,
        to_version: Some(VersionToken::hash(content)),
        size: content.len() as u64,
        from_size: None, // a create replaces nothing
    })
}

/// Soft delete: snapshot the pre-image, journal, then remove. CAS on
/// `base_version` — a mismatch parks nothing (delete has no losing content), so
/// the `Conflict` sidecar field points at the file itself ("it changed; re-read").
fn delete_cas_core<P: FsPrimitives>(
    prims: &P,
    atomic_writes: bool,
    ctx: &WriteCtx,
    args: &DeleteCasArgs,
) -> Result<CommitReceipt, ChaprError> {
    let g = grammar_for(prims.kind());
    let rt = ctx.rt;
    let coord = ctx.coord;
    let principal = ctx.principal;
    let path = &args.path;

    check_human_lock(g, path)?;

    let file = prims
        .open_existing(path.as_str())
        .map_err(|e| map_os_err(path, e))?;
    let current = file.read_all().map_err(|e| map_os_err(path, e))?;
    let v_now = VersionToken::hash(&current);
    if v_now != args.base_version {
        let (last_writer, when) = last_writer_of(rt, coord, path, principal);
        drop(file);
        return Err(ChaprError::Conflict {
            base_path: path.clone(),
            current_version: v_now,
            last_writer,
            when,
            // A delete has no losing content: nothing was submitted to park.
            // This used to name the file itself as the "sidecar" so the message
            // would read correctly, which told every caller a sidecar existed
            // that never did (B6).
            sidecar_path: None,
        });
    }

    // Snapshot the pre-image so the delete is recoverable (concept §6.2).
    rt.block_on(coord.put_blob(current.clone()))?;
    if !atomic_writes {
        rt.block_on(coord.journal_open(&OpenJournalRequest {
            path: path.clone(),
            lease_id: args.lease_id.clone(),
            principal: principal.clone(),
            pre_image_version: v_now.clone(),
            intended_version: None, // deleting → no intended new content
        }))?;
    }
    drop(file); // close before removing

    std::fs::remove_file(path.as_str()).map_err(|e| map_os_err(path, e))?;

    // Not fatal — the file is already gone, so reporting a failed delete would be
    // false. A read of the path now fails at `stat` before the journal is ever
    // consulted, and a later `create` supersedes the entry (INSERT OR REPLACE).
    if !atomic_writes {
        if let Err(e) =
            rt.block_on(coord.journal_clear(&ClearJournalRequest { path: path.clone() }))
        {
            tracing::warn!(
                path = %path, error = %e,
                "delete committed but its journal entry could not be cleared"
            );
        }
    }
    Ok(CommitReceipt {
        from_version: Some(v_now),
        to_version: None,
        size: current.len() as u64,
        // A delete's own version-log entry is keyed by the pre-image hash, so
        // the snapshot is already referenced — no baseline entry is needed.
        from_size: None,
    })
}

/// Restore old bytes into a fresh `.restored-{ts}` sibling (no lease). Returns
/// the path written; the version-log + audit tail is the tool layer's.
fn restore_copy_core<P: FsPrimitives>(
    prims: &P,
    _ctx: &WriteCtx,
    path: &CanonicalPath,
    bytes: &[u8],
    _version: &VersionToken,
) -> Result<CanonicalPath, ChaprError> {
    let g = grammar_for(prims.kind());
    let restored = g.restored_path(path);
    prims
        .create_new(restored.as_str(), bytes)
        .map_err(|e| map_os_err(&restored, e))?;
    Ok(restored)
}

/// Restore old bytes in place: the full contended path — exclusive open, snapshot
/// current, journal, overwrite. No CAS (a restore is a deliberate overwrite).
fn restore_in_place_core<P: FsPrimitives>(
    prims: &P,
    atomic_writes: bool,
    ctx: &WriteCtx,
    args: &RestoreInPlaceArgs,
) -> Result<CommitReceipt, ChaprError> {
    let g = grammar_for(prims.kind());
    let rt = ctx.rt;
    let coord = ctx.coord;
    let principal = ctx.principal;
    let path = &args.path;

    // Office pre-flight — humans always win (concept §7 step 3, §10). A restore
    // in place is the one mutating verb that was missing this, and it does no CAS
    // by design ("a restore is a deliberate overwrite"), so this check was the
    // only thing standing between an agent restore and a document a human has
    // open. The exclusive open is not a substitute: the `~$F` sibling also
    // outlives an Office crash, which is a case the rule exists to respect.
    check_human_lock(g, path)?;

    // A soft-deleted target is a *state*, not an error.
    //
    // `open_existing` on a removed path answered `NotFound` naming the file —
    // for a path whose history resolved and whose copy-restore worked, which is
    // nonsense from the caller's side. And `chapr_delete` promises "recoverable
    // via chapr_restore", a promise only technically kept while recovery landed
    // at `F.restored-{ts}.ext` and needed a follow-up move (itself needing a
    // read) to get the name back.
    //
    // So: absent target + a caller that said `absent` recreates the file at its
    // original name. There is nothing to overwrite, nothing to snapshot and
    // nothing to journal — the failure mode a journal exists for cannot happen
    // when the pre-image is "no file".
    if !prims.exists(path.as_str()) {
        return match &args.base {
            RestoreBase::Absent(_) => {
                prims
                    .create_new(path.as_str(), &args.bytes)
                    .map_err(|e| create_err(prims, path, e))?;
                Ok(CommitReceipt {
                    from_size: None,
                    from_version: None,
                    to_version: Some(args.version.clone()),
                    size: args.bytes.len() as u64,
                })
            }
            // The caller believed a specific version was there. It is not, so it
            // has been deleted or moved since — re-read rather than resurrect
            // under an assumption that no longer holds.
            RestoreBase::Version(seen) => {
                let (last_writer, when) = last_writer_of(rt, coord, path, principal);
                Err(ChaprError::Conflict {
                    base_path: path.clone(),
                    current_version: seen.clone(),
                    last_writer,
                    when,
                    sidecar_path: None,
                })
            }
        };
    }

    let file = prims
        .open_existing(path.as_str())
        .map_err(|e| map_os_err(path, e))?;
    let current = file.read_all().map_err(|e| map_os_err(path, e))?;
    let v_prev = VersionToken::hash(&current);

    // The CAS restore never had (Q13, resolved as concept §6.5 always described
    // it). Under the exclusive handle, so the comparison and the overwrite share
    // one handle exactly as `write` does (invariant 4).
    let expected = match &args.base {
        RestoreBase::Version(v) => v,
        // A live file where the caller expected none: something was created
        // there after they looked, and restoring would destroy it unseen.
        RestoreBase::Absent(_) => {
            let (last_writer, when) = last_writer_of(rt, coord, path, principal);
            drop(file);
            return Err(ChaprError::Conflict {
                base_path: path.clone(),
                current_version: v_prev,
                last_writer,
                when,
                sidecar_path: None,
            });
        }
    };
    if v_prev != *expected {
        let (last_writer, when) = last_writer_of(rt, coord, path, principal);
        drop(file);
        return Err(ChaprError::Conflict {
            base_path: path.clone(),
            current_version: v_prev,
            last_writer,
            when,
            sidecar_path: None,
        });
    }

    rt.block_on(coord.put_blob(current.clone()))?; // snapshot what we're overwriting
    if !atomic_writes {
        rt.block_on(coord.journal_open(&OpenJournalRequest {
            path: path.clone(),
            lease_id: args.lease_id.clone(),
            principal: principal.clone(),
            pre_image_version: v_prev.clone(),
            intended_version: Some(args.version.clone()),
        }))?;
    }

    file.overwrite(&args.bytes)
        .map_err(|e| map_os_err(path, e))?;
    drop(file);

    // Not fatal, for the same reason as `write_cas_core`: the restored bytes are
    // durable, and the next read reconciles the entry against `intended_version`.
    if !atomic_writes {
        if let Err(e) =
            rt.block_on(coord.journal_clear(&ClearJournalRequest { path: path.clone() }))
        {
            tracing::warn!(
                path = %path, error = %e,
                "restore committed but its journal entry could not be cleared; the next read resolves it"
            );
        }
    }
    Ok(CommitReceipt {
        from_size: Some(current.len() as u64),
        from_version: Some(v_prev),
        to_version: Some(args.version.clone()),
        size: args.bytes.len() as u64,
    })
}

/// Atomic dual-lease rename: CAS the source, CAS the destination if overwriting,
/// rename ground-truth-first, then migrate coord state (`move_paths`, which emits
/// the version-log + audit server-side — so there is no tool-layer tail).
fn move_cas_core<P: FsPrimitives>(
    prims: &P,
    ctx: &WriteCtx,
    args: &MoveCasArgs,
) -> Result<MoveReceipt, ChaprError> {
    let g = grammar_for(prims.kind());
    let rt = ctx.rt;
    let coord = ctx.coord;
    let principal = ctx.principal;
    let session_id = ctx.session_id;
    let src = &args.src;
    let dst = &args.dst;

    // Office pre-flight on the source.
    check_human_lock(g, src)?;

    // CAS the source: exclusive open, hash, compare to the version read.
    let sfile = prims
        .open_existing(src.as_str())
        .map_err(|e| map_os_err(src, e))?;
    let sbytes = sfile.read_all().map_err(|e| map_os_err(src, e))?;
    let src_now = VersionToken::hash(&sbytes);
    let size = sbytes.len() as u64;
    if src_now != args.src_base_version {
        let (last_writer, when) = last_writer_of(rt, coord, src, principal);
        drop(sfile);
        return Err(ChaprError::Conflict {
            base_path: src.clone(),
            current_version: src_now,
            last_writer,
            when,
            // A move parks nothing: the caller submitted no content (B6).
            sidecar_path: None,
        });
    }
    // `sfile` is deliberately NOT dropped here. It stays held until the rename
    // below goes through it, so version-check and mutation share one handle
    // (invariant 4, I-007). Closing here — as this used to — left the source
    // unlocked across the destination's CAS and blob upload, a window in which
    // another writer could replace the very bytes we just hashed and have them
    // renamed into `dst` unnoticed.

    // Overwrite? Then CAS the destination too (concept §6.3).
    let overwrite = std::path::Path::new(dst.as_str()).exists();
    let mut dst_pre_image = None;
    if overwrite {
        check_human_lock(g, dst)?;
        let dbv = args
            .dst_base_version
            .as_ref()
            .ok_or_else(|| ChaprError::BaseVersionRequired { path: dst.clone() })?;
        // Read-before-write on the destination too (§6.2).
        rt.block_on(coord.assert_read(&ReadReceipt {
            session_id: session_id.clone(),
            path: dst.clone(),
            version: dbv.clone(),
        }))?;
        let dfile = prims
            .open_existing(dst.as_str())
            .map_err(|e| map_os_err(dst, e))?;
        let dbytes = dfile.read_all().map_err(|e| map_os_err(dst, e))?;
        let dst_now = VersionToken::hash(&dbytes);
        if dst_now != *dbv {
            let (last_writer, when) = last_writer_of(rt, coord, dst, principal);
            drop(dfile);
            return Err(ChaprError::Conflict {
                base_path: dst.clone(),
                current_version: dst_now,
                last_writer,
                when,
                sidecar_path: None,
            });
        }
        // Snapshot what the rename is about to destroy. `dbytes` is exactly those
        // bytes and was previously read only to hash, then dropped — so an
        // overwrite-move was the one mutating verb with no recovery path at all,
        // and coord's `move_paths` deliberately keeps dst's version log, which
        // then advertised versions whose blobs were never stored.
        if dbytes.len() > MAX_PRE_IMAGE_BYTES {
            return Err(ChaprError::Io {
                path: dst.clone(),
                message: format!(
                    "destination is {} bytes; an overwrite-move snapshots it to history first \
                     and the coordinator accepts at most {MAX_PRE_IMAGE_BYTES}",
                    dbytes.len()
                ),
            });
        }
        // Upload while the exclusive handle is still held. The destination's
        // handle must be closed before the rename can replace it — a rename
        // cannot unlink a name this process holds with `FILE_SHARE_NONE` — but
        // that close now happens immediately before the rename, with nothing
        // between them, and the *source* stays locked throughout.
        //
        // This used to close first and upload afterwards, stretching the unlocked
        // window across a network round-trip of up to `MAX_PRE_IMAGE_BYTES`.
        // Other Chaperone sessions are excluded by the all-or-none `{src,dst}`
        // lease either way; what changed is the window for everyone else.
        let dst_size = dbytes.len() as u64;
        rt.block_on(coord.put_blob(dbytes))?;
        dst_pre_image = Some(PreImage {
            version: dst_now,
            size: dst_size,
        });
        drop(dfile);
    }

    // Record the intent BEFORE the rename (B3), so the window D-013 accepted has
    // something to recover from. Everything the migration needs is in the entry,
    // because the session that completes it is often not this one.
    //
    // Fail-closed, exactly like the write path's `journal_open`: nothing has been
    // changed at this point, so refusing costs a retry, while proceeding would
    // buy a rename that no record connects to its destination — which is the
    // state B3 exists to abolish.
    rt.block_on(coord.open_move(&OpenMoveJournalRequest {
        src: src.clone(),
        dst: dst.clone(),
        lease_id: args.lease_id.clone(),
        principal: principal.clone(),
        session_id: session_id.clone(),
        version: src_now.clone(),
        size,
        overwrite,
        dst_pre_image: dst_pre_image.clone(),
    }))?;

    // Ground truth first: the rename — through the source handle held since its
    // CAS, so nothing could have changed the bytes we verified (invariant 4).
    if let Err(e) = sfile.rename_to(dst.as_str(), overwrite) {
        // The rename did not happen, so the intent describes nothing. Drop it,
        // or the next sweep would find an entry whose `src` still exists and
        // reach the same conclusion the slow way — and until it did, coord would
        // report a pending move that never began.
        //
        // Best-effort: the move already failed and this is bookkeeping. A left
        // entry is self-correcting (recovery sees `src` present and clears it),
        // so failing the call twice would only replace a precise error with a
        // vaguer one.
        if let Err(clear_err) =
            rt.block_on(coord.clear_move(&ClearMoveJournalRequest { src: src.clone() }))
        {
            tracing::warn!(
                src = %src, dst = %dst, error = %clear_err,
                "the rename failed and its move intent could not be cleared; the next sweep resolves it"
            );
        }
        return Err(map_os_err(dst, e));
    }
    drop(sfile); // released here rather than by scope end, so the order is explicit

    // Then migrate coord state atomically to match.
    //
    // The rename above ALREADY HAPPENED, so a failure here is not a failed move:
    // the file is at `dst` and only coord's bookkeeping is behind. D-013 accepted
    // exactly this window ("a failure there leaves coord stale-but-recoverable");
    // what it did not accept is telling the caller the opposite of what happened.
    // Propagating the raw error did precisely that — a `CoordUnreachable` rendered
    // as "NOTHING WAS CHANGED. …Chaperone deliberately refuses writes" *after* a
    // successful rename. So this takes the committed-but-unrecorded shape that
    // `create`, `delete`, `restore` and `write` already use for their tails.
    rt.block_on(coord.move_paths(&MovePathsRequest {
        src: src.clone(),
        dst: dst.clone(),
        version: src_now.clone(),
        size,
        overwrite,
        principal: principal.clone(),
        session_id: session_id.clone(),
        // The bytes the rename destroyed. `move_paths` logs the *source* version
        // as dst's new head, so without this the snapshot above is named by
        // nothing and GC reclaims the only copy of what the overwrite replaced.
        dst_pre_image,
    }))
    .map_err(|e| ChaprError::CommittedButUnrecorded {
        // `dst` is where the file actually is now, so it is the path the caller
        // must re-read. Naming `src` would send it back to a path that is gone.
        path: dst.clone(),
        version: src_now.clone(),
        message: format!("the file was renamed from {src} to {dst}, but {e}"),
    })?;

    Ok(MoveReceipt { version: src_now })
}

/// Create one directory, shared by both backends.
///
/// `std::fs::create_dir` on both, and that is not a shortcut: on Windows it calls
/// `CreateDirectoryW` and accepts a UNC path, so the windows-rs treatment the
/// write path needs — an exclusive handle with `FILE_SHARE_NONE` — has nothing to
/// contribute here. A directory cannot be opened for exclusive content access
/// because it has no content.
///
/// The two failures worth distinguishing are the two a caller can act on:
/// a missing parent (fix by creating it, or by choosing a directory that exists)
/// and an existing entry (use it).
fn mkdir_core<P: FsPrimitives>(prims: &P, path: &CanonicalPath) -> Result<(), ChaprError> {
    if prims.exists(path.as_str()) {
        return Err(ChaprError::AlreadyExists { path: path.clone() });
    }
    prims
        .create_dir(path.as_str())
        .map_err(|e| create_err(prims, path, e))
}

/// Refuse the write if the backend's human/Office lock sibling is present
/// (concept §7 step 3, §10 — humans always win). A no-op for backends whose
/// grammar has no such convention (POSIX → advisory lock only, D-F).
fn check_human_lock(
    g: &dyn crate::pathgrammar::PathGrammar,
    path: &CanonicalPath,
) -> Result<(), ChaprError> {
    for lock in g.human_lock_paths(path) {
        if std::path::Path::new(lock.as_str()).exists() {
            return Err(ChaprError::OfficeLockPresent {
                path: path.clone(),
                lock_file: lock,
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SMB backend (Windows `CreateFileW share=NONE`, `MoveFileExW`).
// ---------------------------------------------------------------------------

/// The SMB backend. Wraps the `winfs` primitives; the v1 backend. Windows-only
/// (the `windows` crate / `CreateFileW`), so gated — the POSIX backend keeps the
/// crate buildable on Linux (E-019 D-C).
#[cfg(windows)]
pub struct SmbBackend;

/// The read sub-seam over `std::fs`. Reads the canonical (lowercased) path,
/// which resolves on case-insensitive Windows/SMB.
#[cfg(windows)]
impl FileSource for SmbBackend {
    fn stat(&self, path: &CanonicalPath) -> io::Result<FileStat> {
        let md = std::fs::metadata(path.as_str())?;
        Ok(FileStat {
            mtime: system_time_to_utc(md.modified()?),
            size: md.len(),
        })
    }
    fn read(&self, path: &CanonicalPath) -> io::Result<Vec<u8>> {
        std::fs::read(path.as_str())
    }
    fn list(&self, dir: &CanonicalPath) -> io::Result<Vec<RawDirEntry>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir.as_str())? {
            let entry = entry?;
            let md = entry.metadata()?;
            out.push(RawDirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                entry_type: crate::read::entry_type_of(&md),
                size: md.len(),
                mtime: system_time_to_utc(md.modified()?),
            });
        }
        Ok(out)
    }
}

#[cfg(windows)]
impl FsPrimitives for SmbBackend {
    type File = ExclusiveFile;
    fn kind(&self) -> BackendKind {
        BackendKind::Smb
    }
    fn open_existing(&self, path: &str) -> io::Result<ExclusiveFile> {
        ExclusiveFile::open_existing(path)
    }
    fn create_new(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        create_new_file(path, bytes)
    }
}

#[cfg(windows)]
impl Backend for SmbBackend {
    fn as_file_source(&self) -> &dyn FileSource {
        self
    }

    fn kind(&self) -> BackendKind {
        BackendKind::Smb
    }

    /// SMB's honest capability profile: synth CAS via a **mandatory** exclusive
    /// lock, no native lease, non-atomic writes (so the journal always runs).
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            native_cas: false,
            native_lease: false,
            mandatory_lock: true,
            atomic_writes: false,
            change_notify: ChangeNotify::ReadDirectoryChangesW,
            token_kind: TokenKind::Blake3,
        }
    }

    fn write_cas(&self, ctx: &WriteCtx, args: &WriteCasArgs) -> Result<CommitReceipt, ChaprError> {
        write_cas_core(self, self.capabilities().atomic_writes, ctx, args)
    }

    fn create(
        &self,
        ctx: &WriteCtx,
        path: &CanonicalPath,
        content: &[u8],
    ) -> Result<CommitReceipt, ChaprError> {
        create_core(self, ctx, path, content)
    }

    fn delete_cas(
        &self,
        ctx: &WriteCtx,
        args: &DeleteCasArgs,
    ) -> Result<CommitReceipt, ChaprError> {
        delete_cas_core(self, self.capabilities().atomic_writes, ctx, args)
    }

    fn restore_copy(
        &self,
        ctx: &WriteCtx,
        path: &CanonicalPath,
        bytes: &[u8],
        version: &VersionToken,
    ) -> Result<CanonicalPath, ChaprError> {
        restore_copy_core(self, ctx, path, bytes, version)
    }

    fn restore_in_place(
        &self,
        ctx: &WriteCtx,
        args: &RestoreInPlaceArgs,
    ) -> Result<CommitReceipt, ChaprError> {
        restore_in_place_core(self, self.capabilities().atomic_writes, ctx, args)
    }

    fn move_cas(&self, ctx: &WriteCtx, args: &MoveCasArgs) -> Result<MoveReceipt, ChaprError> {
        move_cas_core(self, ctx, args)
    }
    fn mkdir(&self, path: &CanonicalPath) -> Result<(), ChaprError> {
        mkdir_core(self, path)
    }
}

// ---------------------------------------------------------------------------
// POSIX backend (advisory `flock`, case-sensitive `/`-paths). E-019.
// ---------------------------------------------------------------------------

/// The POSIX backend (local/NFS). Same shared §7 core as SMB, but its
/// [`FsPrimitives`] use `std::fs` + an **advisory** `flock` ([`crate::posixfs`]),
/// and its [`crate::pathgrammar::PosixGrammar`] is case-sensitive with
/// `/`-separators and no Office-lock convention.
pub struct PosixBackend;

/// Read sub-seam over `std::fs`. Case-**sensitive** (POSIX), so the canonical
/// path must match on disk exactly.
impl FileSource for PosixBackend {
    fn stat(&self, path: &CanonicalPath) -> io::Result<FileStat> {
        let md = std::fs::metadata(path.as_str())?;
        Ok(FileStat {
            mtime: system_time_to_utc(md.modified()?),
            size: md.len(),
        })
    }
    fn read(&self, path: &CanonicalPath) -> io::Result<Vec<u8>> {
        std::fs::read(path.as_str())
    }
    fn list(&self, dir: &CanonicalPath) -> io::Result<Vec<RawDirEntry>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir.as_str())? {
            let entry = entry?;
            let md = entry.metadata()?;
            out.push(RawDirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                entry_type: crate::read::entry_type_of(&md),
                size: md.len(),
                mtime: system_time_to_utc(md.modified()?),
            });
        }
        Ok(out)
    }
}

impl FsPrimitives for PosixBackend {
    type File = crate::posixfs::PosixFile;
    fn kind(&self) -> BackendKind {
        BackendKind::Posix
    }
    fn open_existing(&self, path: &str) -> io::Result<Self::File> {
        crate::posixfs::PosixFile::open_existing(path)
    }
    fn create_new(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        crate::posixfs::create_new(path, bytes)
    }
}

impl Backend for PosixBackend {
    fn as_file_source(&self) -> &dyn FileSource {
        self
    }

    fn kind(&self) -> BackendKind {
        BackendKind::Posix
    }

    /// POSIX's honest profile: synth CAS via an **advisory** lock (not mandatory —
    /// external editors can race, the D-F/D-019 trade-off), no native lease,
    /// non-atomic in-place writes (so the journal runs), inotify change-notify.
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            native_cas: false,
            native_lease: false,
            mandatory_lock: false,
            atomic_writes: false,
            change_notify: ChangeNotify::Inotify,
            token_kind: TokenKind::Blake3,
        }
    }

    fn write_cas(&self, ctx: &WriteCtx, args: &WriteCasArgs) -> Result<CommitReceipt, ChaprError> {
        write_cas_core(self, self.capabilities().atomic_writes, ctx, args)
    }

    fn create(
        &self,
        ctx: &WriteCtx,
        path: &CanonicalPath,
        content: &[u8],
    ) -> Result<CommitReceipt, ChaprError> {
        create_core(self, ctx, path, content)
    }

    fn delete_cas(
        &self,
        ctx: &WriteCtx,
        args: &DeleteCasArgs,
    ) -> Result<CommitReceipt, ChaprError> {
        delete_cas_core(self, self.capabilities().atomic_writes, ctx, args)
    }

    fn restore_copy(
        &self,
        ctx: &WriteCtx,
        path: &CanonicalPath,
        bytes: &[u8],
        version: &VersionToken,
    ) -> Result<CanonicalPath, ChaprError> {
        restore_copy_core(self, ctx, path, bytes, version)
    }

    fn restore_in_place(
        &self,
        ctx: &WriteCtx,
        args: &RestoreInPlaceArgs,
    ) -> Result<CommitReceipt, ChaprError> {
        restore_in_place_core(self, self.capabilities().atomic_writes, ctx, args)
    }

    fn move_cas(&self, ctx: &WriteCtx, args: &MoveCasArgs) -> Result<MoveReceipt, ChaprError> {
        move_cas_core(self, ctx, args)
    }
    fn mkdir(&self, path: &CanonicalPath) -> Result<(), ChaprError> {
        mkdir_core(self, path)
    }
}

/// Best-effort winning-writer attribution for a CONFLICT (from the version log).
fn last_writer_of(
    rt: &Handle,
    coord: &CoordClient,
    path: &CanonicalPath,
    fallback: &Principal,
) -> (Principal, DateTime<Utc>) {
    rt.block_on(coord.history(&HistoryQuery { path: path.clone() }))
        .ok()
        .and_then(|h| h.entries.into_iter().next())
        .map(|e| (e.writer, e.timestamp))
        .unwrap_or_else(|| (fallback.clone(), Utc::now()))
}

/// Map an OS I/O error to a `ChaprError`, keyed on the path. `WouldBlock` (a
/// POSIX advisory `flock` contention via `try_lock`) and Windows
/// `ERROR_SHARING_VIOLATION` (32) both map to `SharingViolation`, so the read
/// state machine's Live/retry path is consistent across backends.
/// [`map_os_err`] for an operation that **creates** a path, which turns one OS
/// answer into a different, truer one.
///
/// A create fails with `NotFound` when the *parent directory* is missing, and
/// relabelling that with the child's path produced the least helpful message in
/// the product: `not found: <the file you asked me to create>`. It contradicts
/// the operation's own premise, and it points the reader at the one path they got
/// right — observed sending a session into a filename-permutation loop.
///
/// So on `NotFound` only, ask the backend whether the parent exists and raise
/// [`ChaprError::ParentMissing`] when it does not. One extra stat, on a failure
/// path, to replace a lie with an address.
fn create_err<P: FsPrimitives>(prims: &P, path: &CanonicalPath, e: io::Error) -> ChaprError {
    if e.kind() == io::ErrorKind::NotFound {
        if let Some(parent) = parent_of(prims.kind(), path) {
            if !prims.exists(parent.as_str()) {
                return ChaprError::ParentMissing {
                    path: path.clone(),
                    parent,
                };
            }
        }
    }
    map_os_err(path, e)
}

/// The parent directory of a canonical path, or `None` when it has no parent
/// inside the backend's namespace (a share root, or `/`).
///
/// Uses the backend's own separator rather than `std::path`: a UNC path handled
/// by `std::path` on a POSIX host would not split, and the POSIX backend runs on
/// the host where that matters.
pub(crate) fn parent_of(kind: BackendKind, path: &CanonicalPath) -> Option<CanonicalPath> {
    let sep = grammar_for(kind).sep();
    let s = path.as_str();

    // A UNC share root is a root. `\\server\share` has a separator in it, so
    // naive truncation yields `\\server` — a path nothing can stat and not a
    // directory the caller could create. The prefix has to be excluded from the
    // walk, then the share name counted as the floor.
    let unc = format!("{sep}{sep}");
    if let Some(rest) = s.strip_prefix(&unc) {
        let parts: Vec<&str> = rest.split(sep).collect();
        // [server, share] is the root; a parent needs a component beyond it.
        if parts.len() <= 2 {
            return None;
        }
        return Some(CanonicalPath::new_unchecked(format!(
            "{unc}{}",
            parts[..parts.len() - 1].join(&sep.to_string())
        )));
    }

    let cut = s.rfind(sep)?;
    if cut == 0 {
        // `/a` under POSIX: the parent is the filesystem root, which exists and
        // is statable, unlike a UNC `\\server`.
        return Some(CanonicalPath::new_unchecked(sep.to_string()));
    }
    Some(CanonicalPath::new_unchecked(s[..cut].to_string()))
}

pub(crate) fn map_os_err(path: &CanonicalPath, e: io::Error) -> ChaprError {
    match e.kind() {
        io::ErrorKind::NotFound => ChaprError::NotFound { path: path.clone() },
        io::ErrorKind::PermissionDenied => ChaprError::PermissionDenied { path: path.clone() },
        io::ErrorKind::AlreadyExists => ChaprError::AlreadyExists { path: path.clone() },
        io::ErrorKind::WouldBlock => ChaprError::SharingViolation { path: path.clone() },
        _ => {
            if e.raw_os_error() == Some(32) {
                ChaprError::SharingViolation { path: path.clone() }
            } else {
                ChaprError::Io {
                    path: path.clone(),
                    message: e.to_string(),
                }
            }
        }
    }
}

/// The default backend for this build's OS: SMB on Windows, POSIX elsewhere.
/// Used when `CHAPR_BACKEND` is unset.
pub fn default_backend_kind() -> BackendKind {
    if cfg!(windows) {
        BackendKind::Smb
    } else {
        BackendKind::Posix
    }
}

/// Instantiate the backend the endpoint will drive (decision D-A: the endpoint is
/// local-authoritative). `Smb` is Windows-only (winfs); requesting it on another
/// OS fails fast rather than silently degrading.
pub fn make_backend(kind: BackendKind) -> Result<std::sync::Arc<dyn Backend>, String> {
    match kind {
        #[cfg(windows)]
        BackendKind::Smb => Ok(std::sync::Arc::new(SmbBackend)),
        #[cfg(not(windows))]
        BackendKind::Smb => Err("the SMB backend is only available on Windows builds".to_string()),
        BackendKind::Posix => Ok(std::sync::Arc::new(PosixBackend)),
    }
}

/// Interpret coord's advisory backend announcement into a concrete kind,
/// defaulting to the local default when coord did not classify. Per decision D-A
/// the endpoint is local-authoritative and coord advisory, so a missing
/// announcement never blocks — it falls back to the default rather than failing.
pub fn select_backend(announced: Option<&BackendDescriptor>) -> BackendKind {
    announced.map(|d| d.kind).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_backend_uses_announcement_else_default() {
        let d = BackendDescriptor {
            kind: BackendKind::Smb,
        };
        assert_eq!(select_backend(Some(&d)), BackendKind::Smb);
        assert_eq!(select_backend(None), BackendKind::default());
    }

    /// The `~$F` human-lock pre-flight on the restore path.
    ///
    /// Windows-only **as a group**, and not because the guard is platform-
    /// specific. `StubFs` forces `BackendKind::Smb` so the `~$F` grammar is
    /// exercised whatever the host, but these tests use a **real temp file**, and
    /// a native temp path is backslash-shaped only here. On Linux
    /// `human_lock_paths` finds no `\` to split on, emits the nonexistent
    /// candidate `~$/tmp/…/q3.xlsx`, and the pre-flight cannot fire — so the
    /// refusal test failed on ubuntu-latest while the code was correct. Feeding a
    /// backslash grammar a POSIX path is the test's mistake, not the guard's.
    ///
    /// Gating the module rather than the two tests keeps `StubFs` and
    /// `restore_against_stub` out of the Linux build with them; left behind they
    /// are dead code, and this workspace treats warnings as errors.
    ///
    /// Candidate generation is covered platform-independently in `pathgrammar`
    /// (that SMB emits the Excel and Word forms, and that POSIX emits none). What
    /// these two add is that the *restore path* calls the pre-flight at all, and
    /// one platform is enough to prove that.
    #[cfg(windows)]
    mod human_lock_preflight {
        use super::*;

        /// A [`FsPrimitives`] that never mutates anything. `open_existing` fails with
        /// a distinctive I/O error rather than panicking: reaching the open is the
        /// *correct* outcome when no human holds the file, and it has to be
        /// distinguishable from the pre-flight refusal.
        struct StubFs;

        struct NeverOpened;
        impl LockedFile for NeverOpened {
            fn read_all(&self) -> io::Result<Vec<u8>> {
                unreachable!("no handle is ever produced")
            }
            fn overwrite(&self, _bytes: &[u8]) -> io::Result<()> {
                unreachable!("no handle is ever produced")
            }
            fn rename_to(&self, _dst: &str, _replace: bool) -> io::Result<()> {
                unreachable!("no handle is ever produced")
            }
        }

        impl FsPrimitives for StubFs {
            type File = NeverOpened;
            fn kind(&self) -> BackendKind {
                BackendKind::Smb // the grammar with a `~$F` convention
            }
            fn open_existing(&self, _path: &str) -> io::Result<Self::File> {
                Err(io::Error::other("reached the exclusive open"))
            }
            fn create_new(&self, _path: &str, _bytes: &[u8]) -> io::Result<()> {
                unreachable!("a restore in place creates nothing")
            }
        }

        /// Drive `restore_in_place_core` against [`StubFs`] and return the error.
        /// A success is impossible here (the stub never yields a handle), so an `Ok`
        /// means the core committed something it should not have.
        fn restore_against_stub(path: &CanonicalPath) -> ChaprError {
            let coord = CoordClient::new("http://127.0.0.1:1"); // unroutable on purpose
            let principal = Principal::new_unchecked("CONTOSO\\agent");
            let session_id = SessionId::new_unchecked("sess-restore");
            let rt = Handle::current();
            let ctx = WriteCtx {
                rt: &rt,
                coord: &coord,
                principal: &principal,
                session_id: &session_id,
            };
            match restore_in_place_core(
                &StubFs,
                false,
                &ctx,
                &RestoreInPlaceArgs {
                    path: path.clone(),
                    lease_id: LeaseId::new_unchecked("lease-1"),
                    bytes: b"an old version the agent wants back".to_vec(),
                    version: VersionToken::hash(b"an old version the agent wants back"),
                    // This test is about the Office-lock preflight, which runs
                    // before the CAS, so the base only has to be well-formed.
                    base: RestoreBase::Version(VersionToken::hash(b"whatever is there")),
                },
            ) {
                Ok(_) => panic!("restore reported a commit against a stub that writes nothing"),
                Err(e) => e,
            }
        }

        /// `chapr.restore mode=in_place` overwrote a document a human had open in
        /// Word or Excel. It was the only mutating verb missing the `~$F` pre-flight,
        /// and it deliberately performs no CAS ("a restore is a deliberate
        /// overwrite"), so that check was the sole guard on the path — humans always
        /// win (concept §7 step 3, §10).
        #[tokio::test]
        async fn restore_in_place_refuses_while_a_human_has_the_file_open() {
            let dir = tempfile::tempdir().unwrap();
            let doc = dir.path().join("q3.xlsx");
            std::fs::write(&doc, b"the human's work").unwrap();
            // Excel's owner file — what "a human has this open" looks like on SMB.
            std::fs::write(dir.path().join("~$q3.xlsx"), b"lock").unwrap();

            let path = CanonicalPath::new_unchecked(doc.to_string_lossy().to_string());
            let err = restore_against_stub(&path);

            assert!(
                matches!(err, ChaprError::OfficeLockPresent { .. }),
                "expected OfficeLockPresent, got {err:?}"
            );
            // And the human's bytes are still theirs.
            assert_eq!(std::fs::read(&doc).unwrap(), b"the human's work");
        }

        /// `chapr.create` was the last verb without the `~$F` pre-flight, on the
        /// reasoning that `CREATE_NEW` cannot overwrite anything (B6a).
        ///
        /// True, and not the whole risk: Word and Excel hold `~$F` for a document
        /// that has never been saved, so creating `F.docx` underneath one collides
        /// the moment the person saves. "Humans always win" with a per-verb
        /// exception is a rule the next author reintroduces.
        ///
        /// The stub's `create_new` is `unreachable!`, so if the pre-flight ever
        /// stops firing this test does not merely fail — it panics inside the
        /// backend, naming the write that should not have been attempted.
        #[tokio::test]
        async fn create_refuses_while_a_human_has_the_file_open() {
            let dir = tempfile::tempdir().unwrap();
            // The file itself does NOT exist: this is the unsaved-document case,
            // where only the owner file is on disk.
            std::fs::write(dir.path().join("~$new.xlsx"), b"lock").unwrap();
            let target = dir.path().join("new.xlsx");
            let path = CanonicalPath::new_unchecked(target.to_string_lossy().to_string());

            let coord = CoordClient::new("http://127.0.0.1:1");
            let principal = Principal::new_unchecked("CONTOSO\\agent");
            let session_id = SessionId::new_unchecked("sess-create");
            let rt = Handle::current();
            let ctx = WriteCtx {
                rt: &rt,
                coord: &coord,
                principal: &principal,
                session_id: &session_id,
            };
            let err = create_core(&StubFs, &ctx, &path, b"the agent's file")
                .expect_err("a human has this document open");
            assert!(
                matches!(err, ChaprError::OfficeLockPresent { .. }),
                "expected OfficeLockPresent, got {err:?}"
            );
            assert!(!target.exists(), "nothing was created");
        }

        /// The mirror of the above: with no lock file the pre-flight lets the core
        /// through to the exclusive open, so the refusal really is the lock's doing
        /// rather than a test that passes for any input.
        ///
        /// It would pass on Linux, but only vacuously: there the pre-flight can
        /// never refuse anything, so the claim it exists to check is not being
        /// checked. Hence it lives in the gated module with its partner.
        #[tokio::test]
        async fn restore_in_place_proceeds_past_the_preflight_without_a_lock() {
            let dir = tempfile::tempdir().unwrap();
            let doc = dir.path().join("q3.xlsx");
            std::fs::write(&doc, b"the human's work").unwrap();
            // No `~$q3.xlsx` this time.

            let path = CanonicalPath::new_unchecked(doc.to_string_lossy().to_string());
            let err = restore_against_stub(&path);

            assert!(
                !matches!(err, ChaprError::OfficeLockPresent { .. }),
                "pre-flight refused with no lock file present: {err:?}"
            );
        }
    }

    /// `move_cas_core` — which had **no tests at all** until I-007's fix (B2).
    ///
    /// These drive the real generic core against the real [`PosixBackend`] and
    /// real files in a tempdir, with only coord mocked. That combination is the
    /// point: the core's whole job is ordering real filesystem operations around
    /// real coord calls, and a stub filesystem would skip exactly the part that
    /// can lose someone's data.
    mod move_cas {
        use super::*;
        use wiremock::matchers::{method as wmethod, path as wpath};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        /// A coord that accepts every call the move path makes.
        async fn coord() -> MockServer {
            let s = MockServer::start().await;
            for p in ["/reads/assert", "/move", "/move/open", "/move/clear"] {
                Mock::given(wmethod("POST"))
                    .and(wpath(p))
                    .respond_with(ResponseTemplate::new(204))
                    .mount(&s)
                    .await;
            }
            Mock::given(wmethod("PUT"))
                .and(wpath("/blobs"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "version": VersionToken::hash(b"ignored").as_str(),
                    "size": 0, "deduplicated": false
                })))
                .mount(&s)
                .await;
            Mock::given(wmethod("POST"))
                .and(wpath("/history"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({ "entries": [] })),
                )
                .mount(&s)
                .await;
            s
        }

        fn canon(p: &std::path::Path) -> CanonicalPath {
            CanonicalPath::new_unchecked(p.to_string_lossy().to_string())
        }

        /// Drive `move_cas_core` against the real POSIX backend.
        async fn run(
            coord_uri: String,
            src: &std::path::Path,
            dst: &std::path::Path,
            src_base: VersionToken,
            dst_base: Option<VersionToken>,
        ) -> Result<MoveReceipt, ChaprError> {
            let src = canon(src);
            let dst = canon(dst);
            tokio::task::spawn_blocking(move || {
                let client = CoordClient::new(coord_uri);
                let principal = Principal::new_unchecked("CONTOSO\\tester");
                let session_id = SessionId::new_unchecked("sess-move");
                let rt = Handle::current();
                let ctx = WriteCtx {
                    rt: &rt,
                    coord: &client,
                    principal: &principal,
                    session_id: &session_id,
                };
                let args = MoveCasArgs {
                    src,
                    dst,
                    src_base_version: src_base,
                    dst_base_version: dst_base,
                    lease_id: LeaseId::new_unchecked("lease-move"),
                };
                move_cas_core(&PosixBackend, &ctx, &args)
            })
            .await
            .expect("join")
        }

        /// The happy path, and the assertion that matters most: the bytes that
        /// arrive at `dst` are the bytes that were CAS-verified at `src`.
        #[tokio::test(flavor = "multi_thread")]
        async fn renames_and_preserves_the_verified_bytes() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src.txt");
            let dst = dir.path().join("dst.txt");
            std::fs::write(&src, b"the verified bytes").unwrap();
            let v = VersionToken::hash(b"the verified bytes");

            let c = coord().await;
            run(c.uri(), &src, &dst, v, None).await.expect("move");

            assert!(!src.exists(), "the source name must be gone after a rename");
            assert_eq!(std::fs::read(&dst).unwrap(), b"the verified bytes");
        }

        /// A stale `src_base_version` must lose, and must lose *before* touching
        /// anything. The regression this guards: a move that renames first and
        /// checks afterwards silently discards a concurrent writer's work.
        #[tokio::test(flavor = "multi_thread")]
        async fn stale_source_version_conflicts_and_moves_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src.txt");
            let dst = dir.path().join("dst.txt");
            std::fs::write(&src, b"what is actually on disk").unwrap();

            let c = coord().await;
            let err = run(
                c.uri(),
                &src,
                &dst,
                VersionToken::hash(b"what we read earlier"),
                None,
            )
            .await
            .expect_err("a stale base version must conflict");

            assert!(
                matches!(err, ChaprError::Conflict { .. }),
                "expected Conflict, got {err:?}"
            );
            assert!(src.exists(), "the source must survive a refused move");
            assert!(!dst.exists(), "and the destination must not be created");
            assert_eq!(std::fs::read(&src).unwrap(), b"what is actually on disk");
        }

        /// Overwriting an existing destination requires its base version too
        /// (concept §6.3). Without it the move would destroy `dst` with no CAS.
        #[tokio::test(flavor = "multi_thread")]
        async fn overwrite_without_dst_base_version_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src.txt");
            let dst = dir.path().join("dst.txt");
            std::fs::write(&src, b"source").unwrap();
            std::fs::write(&dst, b"destination worth keeping").unwrap();

            let c = coord().await;
            let err = run(c.uri(), &src, &dst, VersionToken::hash(b"source"), None)
                .await
                .expect_err("an overwrite-move needs dst_base_version");

            assert!(
                matches!(err, ChaprError::BaseVersionRequired { .. }),
                "expected BaseVersionRequired, got {err:?}"
            );
            assert_eq!(
                std::fs::read(&dst).unwrap(),
                b"destination worth keeping",
                "the destination must be untouched"
            );
        }

        /// A stale destination version loses too, and leaves both files intact.
        #[tokio::test(flavor = "multi_thread")]
        async fn stale_destination_version_conflicts() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src.txt");
            let dst = dir.path().join("dst.txt");
            std::fs::write(&src, b"source").unwrap();
            std::fs::write(&dst, b"dst as it really is").unwrap();

            let c = coord().await;
            let err = run(
                c.uri(),
                &src,
                &dst,
                VersionToken::hash(b"source"),
                Some(VersionToken::hash(b"dst as we read it")),
            )
            .await
            .expect_err("a stale dst base version must conflict");

            assert!(
                matches!(err, ChaprError::Conflict { .. }),
                "expected Conflict, got {err:?}"
            );
            assert_eq!(std::fs::read(&src).unwrap(), b"source");
            assert_eq!(std::fs::read(&dst).unwrap(), b"dst as it really is");
        }

        /// A successful overwrite-move replaces the destination with the source.
        #[tokio::test(flavor = "multi_thread")]
        async fn overwrite_move_replaces_the_destination() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src.txt");
            let dst = dir.path().join("dst.txt");
            std::fs::write(&src, b"the winner").unwrap();
            std::fs::write(&dst, b"the replaced").unwrap();

            let c = coord().await;
            run(
                c.uri(),
                &src,
                &dst,
                VersionToken::hash(b"the winner"),
                Some(VersionToken::hash(b"the replaced")),
            )
            .await
            .expect("overwrite move");

            assert!(!src.exists());
            assert_eq!(std::fs::read(&dst).unwrap(), b"the winner");
        }

        /// Coord failing *after* the rename must not tell the agent nothing
        /// happened — that was A1's bug, and this is its regression test at the
        /// core rather than at the tool layer. The file is at `dst`, and the
        /// error must name `dst`, because sending the caller back to `src` sends
        /// it to a path that no longer exists.
        #[tokio::test(flavor = "multi_thread")]
        async fn coord_failure_after_the_rename_reports_committed_but_unrecorded() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src.txt");
            let dst = dir.path().join("dst.txt");
            std::fs::write(&src, b"moved anyway").unwrap();

            // Everything succeeds except the bookkeeping that follows the rename.
            // `/move/open` is among the successes on purpose: this is the window
            // B3's intent record exists for, and the record must survive it.
            let s = MockServer::start().await;
            Mock::given(wmethod("POST"))
                .and(wpath("/move"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&s)
                .await;
            for p in ["/reads/assert", "/move/open"] {
                Mock::given(wmethod("POST"))
                    .and(wpath(p))
                    .respond_with(ResponseTemplate::new(204))
                    .mount(&s)
                    .await;
            }

            let err = run(
                s.uri(),
                &src,
                &dst,
                VersionToken::hash(b"moved anyway"),
                None,
            )
            .await
            .expect_err("coord refused the bookkeeping");

            match err {
                ChaprError::CommittedButUnrecorded { path, .. } => assert_eq!(
                    path.as_str(),
                    dst.to_string_lossy(),
                    "the caller must be sent to where the file now IS"
                ),
                other => panic!("expected CommittedButUnrecorded, got {other:?}"),
            }
            assert!(!src.exists(), "the rename really happened");
            assert_eq!(std::fs::read(&dst).unwrap(), b"moved anyway");
        }

        /// B3's fail-closed edge: a move that cannot record its intent must not
        /// touch the file. Before the intent existed this move would have
        /// succeeded on disk and left coord with no way to learn it had.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_move_that_cannot_record_its_intent_moves_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src.txt");
            let dst = dir.path().join("dst.txt");
            std::fs::write(&src, b"stays put").unwrap();

            let s = MockServer::start().await;
            Mock::given(wmethod("POST"))
                .and(wpath("/move/open"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&s)
                .await;
            Mock::given(wmethod("POST"))
                .and(wpath("/reads/assert"))
                .respond_with(ResponseTemplate::new(204))
                .mount(&s)
                .await;
            // No `/move` mock at all: reaching it would be the failure this test
            // is about, and a 404 there would be indistinguishable from success
            // at telling us whether the rename ran.

            let err = run(s.uri(), &src, &dst, VersionToken::hash(b"stays put"), None)
                .await
                .expect_err("the intent could not be recorded");

            assert!(
                !matches!(err, ChaprError::CommittedButUnrecorded { .. }),
                "nothing was committed, so this must not claim otherwise: {err:?}"
            );
            assert!(src.exists(), "the source was renamed away despite refusing");
            assert!(
                !dst.exists(),
                "the destination was created despite refusing"
            );
            assert_eq!(std::fs::read(&src).unwrap(), b"stays put");
        }

        /// The other side of it: a rename that fails leaves no intent behind,
        /// because an intent for a move that never began would have coord
        /// reporting a pending migration forever.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_failed_rename_clears_the_intent_it_opened() {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src.txt");
            // A directory that does not exist: the rename cannot succeed.
            let dst = dir.path().join("missing").join("dst.txt");
            std::fs::write(&src, b"cannot land").unwrap();

            let s = coord().await;
            let err = run(
                s.uri(),
                &src,
                &dst,
                VersionToken::hash(b"cannot land"),
                None,
            )
            .await
            .expect_err("the rename could not land");
            assert!(
                !matches!(err, ChaprError::CommittedButUnrecorded { .. }),
                "a rename that failed must not report a commit: {err:?}"
            );
            assert!(src.exists(), "the source is still there");

            // The intent was opened and then cleared: both calls happened, in
            // that order, which is what distinguishes "cleaned up" from "never
            // opened".
            let calls: Vec<String> = s
                .received_requests()
                .await
                .unwrap()
                .iter()
                .map(|r| r.url.path().to_string())
                .collect();
            assert!(
                calls.iter().any(|p| p == "/move/open"),
                "the intent was never opened: {calls:?}"
            );
            assert!(
                calls.iter().any(|p| p == "/move/clear"),
                "the intent was left behind: {calls:?}"
            );
            assert!(
                !calls.iter().any(|p| p == "/move"),
                "a failed rename must not migrate coord state: {calls:?}"
            );
        }
    }

    /// The restore CAS (Q13, resolved as option (a)) — driven against the real
    /// POSIX backend and real files, with only coord mocked.
    ///
    /// Restore was the one verb that performed **no** CAS: it reinstated old
    /// bytes over whatever was there. In a product whose purpose is preventing
    /// data loss, that let an agent destroy content it had never read. Each test
    /// below is one row of the table in [`chapr_proto::RestoreBase`], and the
    /// interesting half is the refusals.
    mod restore_cas {
        use super::*;
        use chapr_proto::AbsentMarker;
        use wiremock::matchers::{method as wmethod, path as wpath};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        /// A coord that accepts the snapshot, journal and history calls a restore
        /// makes. Nothing here decides an outcome — the CAS does.
        async fn coord() -> MockServer {
            let s = MockServer::start().await;
            for p in ["/journal", "/journal/clear", "/history"] {
                Mock::given(wmethod("POST"))
                    .and(wpath(p))
                    .respond_with(ResponseTemplate::new(204))
                    .mount(&s)
                    .await;
            }
            Mock::given(wmethod("PUT"))
                .and(wpath("/blobs"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "version": VersionToken::hash(b"ignored").as_str(),
                    "size": 0, "deduplicated": false
                })))
                .mount(&s)
                .await;
            s
        }

        const OLD: &[u8] = b"the version being restored";

        async fn run(
            coord_uri: String,
            path: &std::path::Path,
            base: RestoreBase,
        ) -> Result<CommitReceipt, ChaprError> {
            let path = CanonicalPath::new_unchecked(path.to_string_lossy().to_string());
            tokio::task::spawn_blocking(move || {
                let client = CoordClient::new(coord_uri);
                let principal = Principal::new_unchecked("CONTOSO\\tester");
                let session_id = SessionId::new_unchecked("sess-restore");
                let rt = Handle::current();
                let ctx = WriteCtx {
                    rt: &rt,
                    coord: &client,
                    principal: &principal,
                    session_id: &session_id,
                };
                restore_in_place_core(
                    &PosixBackend,
                    false,
                    &ctx,
                    &RestoreInPlaceArgs {
                        path,
                        lease_id: LeaseId::new_unchecked("lease-restore"),
                        bytes: OLD.to_vec(),
                        version: VersionToken::hash(OLD),
                        base,
                    },
                )
            })
            .await
            .expect("join")
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_live_file_at_the_version_the_caller_saw_is_overwritten() {
            let dir = tempfile::tempdir().unwrap();
            let f = dir.path().join("f.txt");
            std::fs::write(&f, b"current").unwrap();

            let s = coord().await;
            run(s.uri(), &f, RestoreBase::Version(VersionToken::hash(b"current")))
                .await
                .expect("the caller had read the current contents");
            assert_eq!(std::fs::read(&f).unwrap(), OLD);
        }

        /// The bazooka this closes: the file moved on after the caller read it,
        /// and restoring would destroy an edit nobody involved has seen.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_live_file_that_moved_on_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let f = dir.path().join("f.txt");
            std::fs::write(&f, b"someone else wrote this").unwrap();

            let s = coord().await;
            let err = run(s.uri(), &f, RestoreBase::Version(VersionToken::hash(b"what I read")))
                .await
                .expect_err("must not overwrite unseen content");
            match err {
                ChaprError::Conflict {
                    base_path,
                    sidecar_path,
                    ..
                } => {
                    assert_eq!(base_path.as_str(), f.to_string_lossy());
                    assert!(sidecar_path.is_none(), "a restore parks nothing");
                }
                other => panic!("expected Conflict, got {other:?}"),
            }
            assert_eq!(std::fs::read(&f).unwrap(), b"someone else wrote this");
        }

        /// The caller believed the path was empty. It is not — something was
        /// created there — so restoring would silently replace a new file.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_file_where_the_caller_expected_none_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let f = dir.path().join("f.txt");
            std::fs::write(&f, b"created since you looked").unwrap();

            let s = coord().await;
            let err = run(s.uri(), &f, RestoreBase::Absent(AbsentMarker::Absent))
                .await
                .expect_err("must not overwrite a file the caller did not know about");
            assert!(matches!(err, ChaprError::Conflict { .. }), "{err:?}");
            assert_eq!(std::fs::read(&f).unwrap(), b"created since you looked");
        }

        /// The undelete. `chapr_delete` promises "recoverable via chapr_restore";
        /// before this, in-place recovery answered `not found` and the copy
        /// landed under a `.restored-{ts}` name needing a follow-up move.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_soft_deleted_file_comes_back_at_its_original_name() {
            let dir = tempfile::tempdir().unwrap();
            let f = dir.path().join("deleted.txt");
            assert!(!f.exists());

            let s = coord().await;
            let receipt = run(s.uri(), &f, RestoreBase::Absent(AbsentMarker::Absent))
                .await
                .expect("recovering a deleted file is the point");
            assert_eq!(std::fs::read(&f).unwrap(), OLD, "back at its own name");
            assert_eq!(receipt.to_version, Some(VersionToken::hash(OLD)));
            assert!(receipt.from_version.is_none(), "nothing was replaced");
        }

        /// The caller thought a specific version was there and it has gone.
        /// Recreating under that assumption would be resurrecting a file whose
        /// history the caller no longer understands.
        #[tokio::test(flavor = "multi_thread")]
        async fn an_absent_file_the_caller_thought_was_present_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let f = dir.path().join("gone.txt");

            let s = coord().await;
            let err = run(s.uri(), &f, RestoreBase::Version(VersionToken::hash(b"what I read")))
                .await
                .expect_err("the file the caller read is gone");
            assert!(matches!(err, ChaprError::Conflict { .. }), "{err:?}");
            assert!(!f.exists(), "and nothing was created");
        }
    }

    /// A create whose parent is missing must name the parent (2.6).
    mod parent_missing {
        use super::*;

        /// Forward slashes so `PosixBackend`'s grammar and the real filesystem
        /// agree on both hosts — Windows accepts `/` in a path, and pairing the
        /// POSIX grammar with `\` would make `parent_of` find no separator and
        /// the test pass for the wrong reason.
        fn posix_path(p: &std::path::Path) -> CanonicalPath {
            CanonicalPath::new_unchecked(p.to_string_lossy().replace('\\', "/"))
        }

        #[test]
        fn a_create_into_a_missing_directory_names_the_directory() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("nosuchdir").join("f.txt");
            let path = posix_path(&target);

            let err = create_err(
                &PosixBackend,
                &path,
                io::Error::from(io::ErrorKind::NotFound),
            );
            match err {
                ChaprError::ParentMissing { parent, path: p } => {
                    assert!(parent.as_str().ends_with("nosuchdir"), "{parent}");
                    assert_eq!(p, path);
                    // The remedy has to be in the message, or the caller retries
                    // with a different file name — the loop this replaces.
                    let msg = ChaprError::ParentMissing {
                        parent: parent.clone(),
                        path: p,
                    }
                    .to_string();
                    assert!(msg.contains("chapr_mkdir"), "{msg}");
                }
                other => panic!("expected ParentMissing, got {other:?}"),
            }
        }

        /// When the parent *does* exist, a `NotFound` means what it says and must
        /// not be relabelled — otherwise the new branch hides a real answer.
        #[test]
        fn an_existing_parent_leaves_not_found_alone() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("f.txt");
            let path = posix_path(&target);
            let err = create_err(
                &PosixBackend,
                &path,
                io::Error::from(io::ErrorKind::NotFound),
            );
            assert!(matches!(err, ChaprError::NotFound { .. }), "{err:?}");
        }

        /// A share root has no parent to report, and asking for one must not
        /// produce `\\server` — a path nothing can stat.
        #[test]
        fn a_share_root_has_no_parent() {
            let root = CanonicalPath::new_unchecked("\\\\srv\\share".to_string());
            assert!(parent_of(BackendKind::Smb, &root).is_none());
            let file = CanonicalPath::new_unchecked("\\\\srv\\share\\f.md".to_string());
            assert_eq!(
                parent_of(BackendKind::Smb, &file).map(|p| p.as_str().to_string()),
                Some("\\\\srv\\share".to_string())
            );
        }
    }
}
