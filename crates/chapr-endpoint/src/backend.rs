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
//!   and rename primitives (SMB = `CreateFileW share=NONE` / `MoveFileExW`;
//!   POSIX = `open`+advisory-`flock` / `rename`).
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
use crate::winfs::{create_new_file, move_file, ExclusiveFile};
use chapr_proto::{
    BackendDescriptor, BackendKind, CanonicalPath, ChaprError, ClearJournalRequest, HistoryQuery,
    LeaseId, MovePathsRequest, OpenJournalRequest, Principal, ReadReceipt, RegisterConflictRequest,
    SessionId, VersionToken, WriteMode,
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
pub struct CommitReceipt {
    pub from_version: Option<VersionToken>, // None for create
    pub to_version: Option<VersionToken>,   // None for delete
    pub size: u64,
}

/// A move reports nothing to the tool layer: its version-log entry + audit are
/// emitted coord-side by `move_paths` (D-013).
pub struct MoveReceipt;

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
}

/// Args for a move/rename (`move_cas`). No `lease_id`: move does not journal.
pub struct MoveCasArgs {
    pub src: CanonicalPath,
    pub dst: CanonicalPath,
    pub src_base_version: VersionToken,
    pub dst_base_version: Option<VersionToken>,
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
    /// Rename `src` to `dst` (a true rename preserving identity/ACL), replacing
    /// an existing `dst` when `overwrite`.
    fn rename(&self, src: &str, dst: &str, overwrite: bool) -> io::Result<()>;
}

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
    fn create(&self, ctx: &WriteCtx, path: &CanonicalPath, content: &[u8]) -> Result<CommitReceipt, ChaprError>;

    /// Soft delete: snapshot the pre-image, then remove. CAS on `base_version`.
    fn delete_cas(&self, ctx: &WriteCtx, args: &DeleteCasArgs) -> Result<CommitReceipt, ChaprError>;

    /// Restore old bytes into a fresh uniquely-named sibling (no lease). Returns
    /// the path written.
    fn restore_copy(&self, ctx: &WriteCtx, path: &CanonicalPath, bytes: &[u8], version: &VersionToken) -> Result<CanonicalPath, ChaprError>;

    /// Restore old bytes in place (the full contended path).
    fn restore_in_place(&self, ctx: &WriteCtx, args: &RestoreInPlaceArgs) -> Result<CommitReceipt, ChaprError>;

    /// Atomic dual-lease rename with CAS on both sides. Coord state is migrated
    /// server-side (`move_paths`), so nothing is returned for the tool layer.
    fn move_cas(&self, ctx: &WriteCtx, args: &MoveCasArgs) -> Result<MoveReceipt, ChaprError>;
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
    let file = prims.open_existing(path.as_str()).map_err(|e| map_os_err(path, e))?;

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
        prims.create_new(sidecar.as_str(), content).map_err(|e| map_os_err(&sidecar, e))?;
        let _entry = rt.block_on(coord.register_conflict(&RegisterConflictRequest {
            base_path: path.clone(),
            sidecar_path: sidecar.clone(),
            losing_principal: principal.clone(),
            session_id: session_id.clone(),
        }))?;
        let (last_writer, when) = last_writer_of(rt, coord, path, principal);
        drop(file); // release the exclusive handle
        return Err(ChaprError::Conflict {
            current_version: v_now,
            last_writer,
            when,
            sidecar_path: sidecar,
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

    // Step 8: journal intent (fail-closed if coord is unreachable — no write).
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
    if !atomic_writes {
        rt.block_on(coord.journal_clear(&ClearJournalRequest { path: path.clone() }))?;
    }

    Ok(CommitReceipt {
        from_version: Some(v_now),
        to_version: Some(v_new),
        size: content.len() as u64,
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
    prims.create_new(path.as_str(), content).map_err(|e| map_os_err(path, e))?;
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

    let file = prims.open_existing(path.as_str()).map_err(|e| map_os_err(path, e))?;
    let current = file.read_all().map_err(|e| map_os_err(path, e))?;
    let v_now = VersionToken::hash(&current);
    if v_now != args.base_version {
        let (last_writer, when) = last_writer_of(rt, coord, path, principal);
        drop(file);
        return Err(ChaprError::Conflict {
            current_version: v_now,
            last_writer,
            when,
            sidecar_path: path.clone(),
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

    if !atomic_writes {
        rt.block_on(coord.journal_clear(&ClearJournalRequest { path: path.clone() }))?;
    }
    Ok(CommitReceipt {
        from_version: Some(v_now),
        to_version: None,
        size: current.len() as u64,
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
    prims.create_new(restored.as_str(), bytes).map_err(|e| map_os_err(&restored, e))?;
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
    let rt = ctx.rt;
    let coord = ctx.coord;
    let principal = ctx.principal;
    let path = &args.path;

    let file = prims.open_existing(path.as_str()).map_err(|e| map_os_err(path, e))?;
    let current = file.read_all().map_err(|e| map_os_err(path, e))?;
    let v_prev = VersionToken::hash(&current);

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

    file.overwrite(&args.bytes).map_err(|e| map_os_err(path, e))?;
    drop(file);

    if !atomic_writes {
        rt.block_on(coord.journal_clear(&ClearJournalRequest { path: path.clone() }))?;
    }
    Ok(CommitReceipt {
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
    let sfile = prims.open_existing(src.as_str()).map_err(|e| map_os_err(src, e))?;
    let sbytes = sfile.read_all().map_err(|e| map_os_err(src, e))?;
    let src_now = VersionToken::hash(&sbytes);
    let size = sbytes.len() as u64;
    if src_now != args.src_base_version {
        let (last_writer, when) = last_writer_of(rt, coord, src, principal);
        drop(sfile);
        return Err(ChaprError::Conflict {
            current_version: src_now,
            last_writer,
            when,
            sidecar_path: src.clone(),
        });
    }
    drop(sfile); // close before renaming

    // Overwrite? Then CAS the destination too (concept §6.3).
    let overwrite = std::path::Path::new(dst.as_str()).exists();
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
        let dfile = prims.open_existing(dst.as_str()).map_err(|e| map_os_err(dst, e))?;
        let dbytes = dfile.read_all().map_err(|e| map_os_err(dst, e))?;
        let dst_now = VersionToken::hash(&dbytes);
        drop(dfile);
        if dst_now != *dbv {
            let (last_writer, when) = last_writer_of(rt, coord, dst, principal);
            return Err(ChaprError::Conflict {
                current_version: dst_now,
                last_writer,
                when,
                sidecar_path: dst.clone(),
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
        rt.block_on(coord.put_blob(dbytes))?;
    }

    // Ground truth first: the rename.
    prims.rename(src.as_str(), dst.as_str(), overwrite).map_err(|e| map_os_err(dst, e))?;

    // Then migrate coord state atomically to match.
    rt.block_on(coord.move_paths(&MovePathsRequest {
        src: src.clone(),
        dst: dst.clone(),
        version: src_now,
        size,
        overwrite,
        principal: principal.clone(),
        session_id: session_id.clone(),
    }))?;

    Ok(MoveReceipt)
}

/// Refuse the write if the backend's human/Office lock sibling is present
/// (concept §7 step 3, §10 — humans always win). A no-op for backends whose
/// grammar has no such convention (POSIX → advisory lock only, D-F).
fn check_human_lock(
    g: &dyn crate::pathgrammar::PathGrammar,
    path: &CanonicalPath,
) -> Result<(), ChaprError> {
    if let Some(lock) = g.human_lock_path(path) {
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
    fn rename(&self, src: &str, dst: &str, overwrite: bool) -> io::Result<()> {
        move_file(src, dst, overwrite)
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

    fn delete_cas(&self, ctx: &WriteCtx, args: &DeleteCasArgs) -> Result<CommitReceipt, ChaprError> {
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
    fn rename(&self, src: &str, dst: &str, overwrite: bool) -> io::Result<()> {
        crate::posixfs::rename(src, dst, overwrite)
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

    fn delete_cas(&self, ctx: &WriteCtx, args: &DeleteCasArgs) -> Result<CommitReceipt, ChaprError> {
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
        BackendKind::Smb => {
            Err("the SMB backend is only available on Windows builds".to_string())
        }
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
}
