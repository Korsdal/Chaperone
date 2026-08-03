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
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, MoveFileExW, ReadFile, SetEndOfFile, SetFilePointerEx, WriteFile,
    CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_BEGIN, FILE_SHARE_NONE, MOVEFILE_COPY_ALLOWED,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, OPEN_EXISTING,
};

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

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
                GENERIC_READ | GENERIC_WRITE,
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
                return Err(io::Error::new(io::ErrorKind::WriteZero, "WriteFile wrote 0 bytes"));
            }
            off += written as usize;
        }
        unsafe { SetEndOfFile(self.handle) }.map_err(to_io)?;
        unsafe { FlushFileBuffers(self.handle) }.map_err(to_io)?;
        Ok(())
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

/// Rename `src` to `dst` (concept §6.3). A true rename — preserves the file's
/// identity and ACL (unlike read+create+delete). `replace` allows overwriting
/// an existing destination. `WRITE_THROUGH` makes the move durable before
/// returning.
pub fn move_file(src: &str, dst: &str, replace: bool) -> io::Result<()> {
    let s = wide(src);
    let d = wide(dst);
    let mut flags = MOVEFILE_COPY_ALLOWED | MOVEFILE_WRITE_THROUGH;
    if replace {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }
    unsafe { MoveFileExW(PCWSTR(s.as_ptr()), PCWSTR(d.as_ptr()), flags) }.map_err(to_io)
}
