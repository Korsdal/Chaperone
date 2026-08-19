// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! POSIX filesystem primitives for the shared §7 core (E-019).
//!
//! The POSIX counterpart to `winfs`: an exclusive-open via **advisory** `flock`
//! (`fs4`), plus create-new and rename. Unlike SMB's `share = NONE` (a mandatory
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
        Ok(PosixFile { file })
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

/// Rename `src` to `dst`. POSIX `rename(2)` atomically replaces an existing
/// destination, so we guard the non-overwrite case explicitly to match the SMB
/// backend's semantics (which refuse via `MoveFileExW` without `REPLACE_EXISTING`).
pub fn rename(src: &str, dst: &str, overwrite: bool) -> io::Result<()> {
    if !overwrite && std::path::Path::new(dst).exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination exists",
        ));
    }
    std::fs::rename(src, dst)
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

    #[test]
    fn rename_guards_non_overwrite() {
        let src = tmp("mvsrc");
        let dst = tmp("mvdst");
        std::fs::write(&src, b"s").unwrap();
        std::fs::write(&dst, b"d").unwrap();
        let (ss, ds) = (src.to_string_lossy().to_string(), dst.to_string_lossy().to_string());
        assert!(matches!(
            rename(&ss, &ds, false),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists
        ));
        rename(&ss, &ds, true).unwrap();
        assert_eq!(std::fs::read(&ds).unwrap(), b"s");
        let _ = std::fs::remove_file(&dst);
    }
}
