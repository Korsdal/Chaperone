// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The Win32 exclusive-open primitive — the correctness core of the write path
//! (concept §7, invariant 3–4).
//!
//! `CreateFileW` with `dwShareMode = FILE_SHARE_NONE` is a **true mandatory
//! lock** on SMB: it excludes Excel, Explorer, and other agents alike (concept
//! §14). This is the whole reason the project is in Rust and Windows-specific —
//! the share mode is a typed first-class argument here, not an FFI hack.
//!
//! This module is deliberately the most boring code in the crate: thin, safe
//! wrappers over the raw calls, one exclusive handle held from open to close via
//! [`ExclusiveFile`]'s RAII `Drop`. All the orchestration lives in
//! [`crate::write`]; keep the cleverness out of here.

use std::io;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, BOOLEAN, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FileRenameInfo, FlushFileBuffers, ReadFile, SetEndOfFile,
    SetFileInformationByHandle, SetFilePointerEx, WriteFile, CREATE_NEW, FILE_ATTRIBUTE_NORMAL,
    FILE_BEGIN, FILE_RENAME_INFO, FILE_SHARE_NONE, OPEN_EXISTING,
};

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
/// `DELETE` (0x0001_0000) is what `SetFileInformationByHandle(FileRenameInfo)`
/// requires — a rename is, to Win32, an unlink of the source name. Requesting it
/// in the *access mask* does not weaken the `FILE_SHARE_NONE` share mode: the
/// mask says what this handle may do, the share mode says what others may do.
const DELETE: u32 = 0x0001_0000;

fn wide(path: &str) -> Vec<u16> {
    path.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Map a windows error to `io::Error` preserving the Win32 code, so the
/// orchestration layer can distinguish NotFound(2) / AccessDenied(5) /
/// SharingViolation(32) via `ErrorKind`/`raw_os_error`.
fn to_io(e: windows::core::Error) -> io::Error {
    // A win32 failure is wrapped as HRESULT 0x8007_00XX; the low word is the code.
    let win32 = (e.code().0 as u32 & 0xFFFF) as i32;
    io::Error::from_raw_os_error(win32)
}

/// A file held open with `FILE_SHARE_NONE` — exclusive against every other
/// opener. The handle is closed on drop, so it is released the instant this
/// value goes out of scope, on every path including early `?` returns.
pub struct ExclusiveFile {
    handle: HANDLE,
}

impl ExclusiveFile {
    /// Open an existing file exclusively for read+write (write-path step 4).
    /// A `SharingViolation` here means someone else holds the file.
    pub fn open_existing(path: &str) -> io::Result<Self> {
        let w = wide(path);
        let handle = unsafe {
            CreateFileW(
                PCWSTR(w.as_ptr()),
                GENERIC_READ | GENERIC_WRITE | DELETE,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                HANDLE::default(),
            )
        }
        .map_err(to_io)?;
        Ok(ExclusiveFile { handle })
    }

    /// Read the whole file (write-path step 5, to re-hash under the lock).
    pub fn read_all(&self) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut chunk = [0u8; 65536];
        loop {
            let mut read = 0u32;
            unsafe { ReadFile(self.handle, Some(&mut chunk), Some(&mut read), None) }
                .map_err(to_io)?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..read as usize]);
        }
        Ok(out)
    }

    /// Overwrite from the start, truncate to the new length, and flush
    /// (write-path step 9). In place — never temp-rename (a rename carries the
    /// source ACL and strips the target's; concept §7 step 9).
    pub fn overwrite(&self, bytes: &[u8]) -> io::Result<()> {
        unsafe { SetFilePointerEx(self.handle, 0, None, FILE_BEGIN) }.map_err(to_io)?;
        let mut off = 0usize;
        while off < bytes.len() {
            let mut written = 0u32;
            unsafe { WriteFile(self.handle, Some(&bytes[off..]), Some(&mut written), None) }
                .map_err(to_io)?;
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "WriteFile wrote 0 bytes",
                ));
            }
            off += written as usize;
        }
        unsafe { SetEndOfFile(self.handle) }.map_err(to_io)?;
        unsafe { FlushFileBuffers(self.handle) }.map_err(to_io)?;
        Ok(())
    }

    /// Rename this file to `dst` **through the handle that is still held**
    /// (I-007, invariant 4). `replace` allows overwriting an existing `dst`.
    ///
    /// This is the whole point of the method, so it is worth being explicit:
    /// `MoveFileExW` cannot do this. It opens the source by *name*, and a file
    /// this process holds with `FILE_SHARE_NONE` refuses that open —
    /// `ERROR_SHARING_VIOLATION` (32), which D-027 established empirically. The
    /// move path therefore used to close the handle and then rename, leaving a
    /// window between version-check and mutation in which another writer's bytes
    /// could land in the file and be renamed away unnoticed. That window is
    /// exactly the invariant-4 violation I-007 tracked.
    ///
    /// `SetFileInformationByHandle(FileRenameInfo)` renames the object the handle
    /// already refers to, so there is no second open to violate and no window at
    /// all. It needs `DELETE` in the access mask (see the constant above).
    ///
    /// `FILE_RENAME_INFO` is a variable-length struct — a fixed header followed
    /// by the destination name inline — so it is built in a byte buffer rather
    /// than as a value. `FileNameLength` counts **bytes and excludes** the NUL,
    /// and with `RootDirectory` null the name must be fully qualified, which
    /// every path reaching here already is (invariant 5 canonicalises to UNC).
    pub fn rename_to(&self, dst: &str, replace: bool) -> io::Result<()> {
        let name: Vec<u16> = dst.encode_utf16().collect();
        let name_bytes = name.len() * std::mem::size_of::<u16>();
        // The header already carries `FileName[1]`, so it covers the trailing NUL
        // we leave room for but do not count in `FileNameLength`.
        let header = std::mem::size_of::<FILE_RENAME_INFO>();
        let total = header + name_bytes;

        // Backed by `u64`, not `u8`, and that is load-bearing rather than
        // fussiness: `FILE_RENAME_INFO` contains a `HANDLE`, so it needs pointer
        // alignment, while a `Vec<u8>`'s allocation is only guaranteed to be
        // 1-aligned. Writing through a misaligned `*mut FILE_RENAME_INFO` would be
        // undefined behaviour that happens to work, which is exactly what this
        // module must not contain. `u64` matches the struct's alignment on both
        // 32- and 64-bit Windows.
        let words = total.div_ceil(std::mem::size_of::<u64>());
        let mut buf = vec![0u64; words];
        debug_assert!(buf.len() * std::mem::size_of::<u64>() >= total);

        // SAFETY: `buf` is at least `total` bytes, zero-initialised, and aligned
        // for `FILE_RENAME_INFO` by construction above. Every write below lands
        // inside it: the header at offset 0, and `name.len()` `u16`s at
        // `FileName`, which the `total` computation sized the tail for.
        unsafe {
            let info = buf.as_mut_ptr() as *mut FILE_RENAME_INFO;
            (*info).Anonymous.ReplaceIfExists = BOOLEAN(u8::from(replace));
            (*info).RootDirectory = HANDLE::default();
            (*info).FileNameLength = name_bytes as u32;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName) as *mut u16,
                name.len(),
            );
            SetFileInformationByHandle(
                self.handle,
                FileRenameInfo,
                buf.as_ptr() as *const core::ffi::c_void,
                total as u32,
            )
        }
        .map_err(to_io)
    }
}

impl Drop for ExclusiveFile {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// The shared §7 core drives the held handle through this seam (E-019). Forwards
/// to the inherent methods; the RAII `Drop` above releases the lock.
impl crate::backend::LockedFile for ExclusiveFile {
    fn read_all(&self) -> io::Result<Vec<u8>> {
        ExclusiveFile::read_all(self)
    }
    fn overwrite(&self, bytes: &[u8]) -> io::Result<()> {
        ExclusiveFile::overwrite(self, bytes)
    }
    fn rename_to(&self, dst: &str, replace: bool) -> io::Result<()> {
        ExclusiveFile::rename_to(self, dst, replace)
    }
}

/// Create a brand-new file (fails if it already exists) and write `bytes` — for
/// the conflict sidecar (concept §11). Uniquely named, so no lock is needed.
pub fn create_new_file(path: &str, bytes: &[u8]) -> io::Result<()> {
    let w = wide(path);
    let handle = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            GENERIC_WRITE,
            FILE_SHARE_NONE,
            None,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
    }
    .map_err(to_io)?;
    let file = ExclusiveFile { handle };
    file.overwrite(bytes)
}

/// Tests for the rename-through-a-held-handle primitive (I-007).
///
/// These run against a **local NTFS temp path**, not a share. That is a
/// deliberate limit and worth stating: what they prove is that the
/// `FILE_RENAME_INFO` buffer is laid out correctly and that `DELETE` in the
/// access mask is sufficient — the parts that are pure Win32 API contract and
/// fail identically everywhere. What they cannot prove is that a *remote SMB
/// server* honours the same call; that is the e2e rig's job, and the mandatory
/// lock it checks is the property that has to be measured rather than assumed.
#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("chapr-winfs-{name}-{}", std::process::id()))
    }

    /// The core claim: a file held with `FILE_SHARE_NONE` can rename **itself**,
    /// which `MoveFileExW` cannot do (it reopens by name and hits
    /// `ERROR_SHARING_VIOLATION`). If the `FILE_RENAME_INFO` buffer were laid out
    /// wrong this is where it shows up, as `ERROR_INVALID_PARAMETER` (87).
    #[test]
    fn renames_through_the_held_handle() {
        let src = tmp("rt-src");
        let dst = tmp("rt-dst");
        std::fs::write(&src, b"held bytes").unwrap();
        let _ = std::fs::remove_file(&dst);

        let f = ExclusiveFile::open_existing(&src.to_string_lossy()).unwrap();
        f.rename_to(&dst.to_string_lossy(), false)
            .expect("a held handle must be able to rename itself");
        drop(f);

        assert!(!src.exists(), "the source name is gone");
        assert_eq!(std::fs::read(&dst).unwrap(), b"held bytes");
        let _ = std::fs::remove_file(&dst);
    }

    /// The property that makes this an invariant-4 fix rather than a rename
    /// convenience: the exclusive lock is **still held** after the rename, so
    /// nothing can slip between the CAS and the mutation.
    #[test]
    fn the_exclusive_lock_survives_the_rename() {
        let src = tmp("hold-src");
        let dst = tmp("hold-dst");
        std::fs::write(&src, b"x").unwrap();
        let _ = std::fs::remove_file(&dst);

        let held = ExclusiveFile::open_existing(&src.to_string_lossy()).unwrap();
        held.rename_to(&dst.to_string_lossy(), false).unwrap();

        let ds = dst.to_string_lossy().to_string();
        assert!(
            ExclusiveFile::open_existing(&ds).is_err(),
            "a second exclusive open must still be refused at the new name"
        );
        drop(held);
        assert!(
            ExclusiveFile::open_existing(&ds).is_ok(),
            "and must succeed once the holder drops"
        );
        let _ = std::fs::remove_file(&dst);
    }

    /// `replace = false` must refuse an existing destination rather than
    /// destroying it. This is the guard that stops a move from silently
    /// discarding an unrelated file.
    #[test]
    fn refuses_an_existing_destination_unless_replacing() {
        let src = tmp("rep-src");
        let dst = tmp("rep-dst");
        std::fs::write(&src, b"source").unwrap();
        std::fs::write(&dst, b"do not lose me").unwrap();

        let f = ExclusiveFile::open_existing(&src.to_string_lossy()).unwrap();
        assert!(
            f.rename_to(&dst.to_string_lossy(), false).is_err(),
            "must refuse without replace"
        );
        drop(f);
        assert_eq!(
            std::fs::read(&dst).unwrap(),
            b"do not lose me",
            "the destination must be untouched by a refused rename"
        );
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&dst);
    }

    /// And `replace = true` does replace it.
    #[test]
    fn replaces_an_existing_destination_when_asked() {
        let src = tmp("ovr-src");
        let dst = tmp("ovr-dst");
        std::fs::write(&src, b"the winner").unwrap();
        std::fs::write(&dst, b"the replaced").unwrap();

        let f = ExclusiveFile::open_existing(&src.to_string_lossy()).unwrap();
        f.rename_to(&dst.to_string_lossy(), true).unwrap();
        drop(f);

        assert!(!src.exists());
        assert_eq!(std::fs::read(&dst).unwrap(), b"the winner");
        let _ = std::fs::remove_file(&dst);
    }

    /// A long name exercises the variable-length tail of the buffer — the part
    /// most likely to be sized wrong. A too-small buffer surfaces here.
    #[test]
    fn handles_a_long_destination_name() {
        let src = tmp("long-src");
        let dst = tmp(&format!("long-{}", "n".repeat(180)));
        std::fs::write(&src, b"y").unwrap();
        let _ = std::fs::remove_file(&dst);

        let f = ExclusiveFile::open_existing(&src.to_string_lossy()).unwrap();
        f.rename_to(&dst.to_string_lossy(), false)
            .expect("a long name must not overrun the rename buffer");
        drop(f);

        assert_eq!(std::fs::read(&dst).unwrap(), b"y");
        let _ = std::fs::remove_file(&dst);
    }
}
