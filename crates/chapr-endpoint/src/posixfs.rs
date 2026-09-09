// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! POSIX filesystem primitives for the shared §7 core (E-019).
//!
//! The POSIX counterpart to `winfs`: an exclusive-open via **advisory** `flock`
//! (`fs4`), plus create-new, and a rename that runs through the held lock.
//! Unlike SMB's `share = NONE` (a mandatory
//! lock excluding Excel/Explorer), `flock` only excludes other *advisory* lockers
//! — so a non-Chaperone editor can still write concurrently. That is the accepted
//! trade-off (decisions D-F / D-019); the CAS re-hash-under-lock still protects
//! against two Chaperone endpoints racing each other.
//!
//! `fs4` is portable (advisory `flock` on Unix, `LockFileEx` on Windows), so this
//! module compiles and unit-tests on the Windows dev box; the genuinely-advisory
//! semantics are exercised on Linux (decision D-C).
//!
//! We use **`try_lock_exclusive`** (non-blocking): contention returns
//! `WouldBlock`, which [`crate::backend::map_os_err`] maps to
//! `ChaprError::SharingViolation` — matching SMB's immediate-conflict behaviour so
//! the read state machine's Live/retry path is consistent across backends.

use crate::backend::LockedFile;
use fs4::FileExt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};

/// A file held open with an advisory exclusive `flock` for the duration of a §7
/// write. The lock is released on drop (explicitly, and by the OS on close).
#[derive(Debug)]
pub struct PosixFile {
    file: File,
    /// The path this was opened by, kept so [`LockedFile::rename_to`] can rename
    /// without closing. POSIX has no rename-by-fd for the source name (`renameat`
    /// wants a directory fd plus a name), so the path is the only handle-free
    /// thing to name it with.
    path: String,
}

impl PosixFile {
    /// Open an existing file read+write and take an advisory exclusive lock.
    /// Non-blocking: contention returns `TryLockError::WouldBlock`, which fs4
    /// maps to `io::ErrorKind::WouldBlock` (→ `SharingViolation` via
    /// [`crate::backend::map_os_err`]). Fully-qualified `FileExt::try_lock` so it
    /// resolves to fs4's trait method, not std's inherent one.
    pub fn open_existing(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        FileExt::try_lock(&file).map_err(io::Error::from)?;
        Ok(PosixFile {
            file,
            path: path.to_string(),
        })
    }
}

impl LockedFile for PosixFile {
    fn read_all(&self) -> io::Result<Vec<u8>> {
        let mut f = &self.file;
        f.seek(SeekFrom::Start(0))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn overwrite(&self, bytes: &[u8]) -> io::Result<()> {
        let mut f = &self.file;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(bytes)?;
        f.set_len(bytes.len() as u64)?; // truncate to the new length
        f.sync_all()?; // durable before we drop the lock (crash safety, §7)
        Ok(())
    }

    /// Rename without releasing the lock. `flock` is held on the open file
    /// description, not on the name, so it survives the rename and stays held
    /// until this value drops — the fd remains valid and refers to the same
    /// inode at its new name.
    ///
    /// Honest about what this is and is not: on Win32 the equivalent renames the
    /// object the handle *already refers to*, so no name is resolved twice. Here
    /// the source is still named by path, so a sufficiently adversarial third
    /// party could in principle swap it in between. That is the same advisory
    /// trade-off this whole backend is built on (`mandatory_lock: false`, D-F /
    /// D-019) and not a new gap — a non-Chaperone writer can already race every
    /// other operation in this module. Against two Chaperone endpoints, which is
    /// what the lock is for, the window is closed.
    fn rename_to(&self, dst: &str, replace: bool) -> io::Result<()> {
        if !replace && std::path::Path::new(dst).exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "destination exists",
            ));
        }
        std::fs::rename(&self.path, dst)
    }
}

impl Drop for PosixFile {
    fn drop(&mut self) {
        // Also released by the OS on close; explicit for clarity.
        let _ = FileExt::unlock(&self.file);
    }
}

/// Create a brand-new file (fails if it exists) and write `bytes` — sidecar /
/// restore-copy / create. Uniquely named, so no lock is needed.
pub fn create_new(path: &str, bytes: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("chapr-posixfs-{name}-{}", std::process::id()))
    }

    #[test]
    fn open_read_overwrite_roundtrip() {
        let p = tmp("rt");
        let ps = p.to_string_lossy().to_string();
        {
            let mut f = File::create(&p).unwrap();
            f.write_all(b"original").unwrap();
        }
        let lf = PosixFile::open_existing(&ps).unwrap();
        assert_eq!(lf.read_all().unwrap(), b"original");
        lf.overwrite(b"new longer contents").unwrap();
        drop(lf);
        assert_eq!(std::fs::read(&ps).unwrap(), b"new longer contents");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn second_exclusive_open_is_would_block() {
        let p = tmp("lock");
        let ps = p.to_string_lossy().to_string();
        std::fs::write(&p, b"x").unwrap();
        let _held = PosixFile::open_existing(&ps).unwrap();
        // A second exclusive open while the first is held → WouldBlock.
        let second = PosixFile::open_existing(&ps);
        assert!(
            matches!(&second, Err(e) if e.kind() == io::ErrorKind::WouldBlock),
            "expected WouldBlock, got {second:?}"
        );
        drop(_held);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn create_new_refuses_existing() {
        let p = tmp("create");
        let ps = p.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&p);
        create_new(&ps, b"first").unwrap();
        assert!(matches!(
            create_new(&ps, b"second"),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists
        ));
        let _ = std::fs::remove_file(&p);
    }

    /// The rename happens **through the held lock** (I-007), so this drives it
    /// via [`LockedFile::rename_to`] on an open file rather than by path. The
    /// non-overwrite guard has to survive that move; it is the only thing
    /// stopping a move from silently destroying an unrelated destination.
    #[test]
    fn rename_guards_non_overwrite() {
        let src = tmp("mvsrc");
        let dst = tmp("mvdst");
        std::fs::write(&src, b"s").unwrap();
        std::fs::write(&dst, b"d").unwrap();
        let (ss, ds) = (
            src.to_string_lossy().to_string(),
            dst.to_string_lossy().to_string(),
        );

        let f = PosixFile::open_existing(&ss).unwrap();
        assert!(matches!(
            f.rename_to(&ds, false),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists
        ));
        f.rename_to(&ds, true).unwrap();
        // Drop BEFORE reading the destination, and the reason is worth keeping:
        // `fs4` is `LockFileEx` on Windows, which is **mandatory**, so a plain
        // `std::fs::read` of the still-locked file fails with
        // `ERROR_LOCK_VIOLATION` (33). Reading here first is how this test failed
        // when it was written — and the failure was evidence *for* the property
        // I-007 wanted: the lock followed the file through the rename.
        drop(f);
        assert_eq!(std::fs::read(&ds).unwrap(), b"s");
        let _ = std::fs::remove_file(&dst);
    }

    /// The property I-007 is about: the lock is still held *after* the rename,
    /// so nothing can slip in between the CAS and the mutation. If `rename_to`
    /// released it, a second `open_existing` on the new name would succeed.
    #[test]
    fn lock_survives_the_rename() {
        let src = tmp("holdsrc");
        let dst = tmp("holddst");
        std::fs::write(&src, b"s").unwrap();
        let _ = std::fs::remove_file(&dst);
        let (ss, ds) = (
            src.to_string_lossy().to_string(),
            dst.to_string_lossy().to_string(),
        );

        let held = PosixFile::open_existing(&ss).unwrap();
        held.rename_to(&ds, false).unwrap();
        assert!(
            PosixFile::open_existing(&ds).is_err(),
            "the advisory lock must still be held at the new name after the rename"
        );
        drop(held);
        assert!(
            PosixFile::open_existing(&ds).is_ok(),
            "and released once the holder drops"
        );
        let _ = std::fs::remove_file(&dst);
    }
}
