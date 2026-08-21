// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Windows `ReadDirectoryChangesW` source for the change-watcher (concept §14).
//!
//! Runs a blocking watch loop on a dedicated OS thread (the API blocks until
//! changes) and forwards normalised [`WatchEvent`]s into a channel that the
//! async [`crate::watch::run`] loop consumes. A zero-byte return or
//! `ERROR_NOTIFY_ENUM_DIR` is the buffer-overflow signal → `Overflow`, which
//! drives the required rescan path (§14 caveat).
//!
//! This is the one Windows-specific piece of coord, isolated behind the
//! platform-agnostic core. It is exercised in a real deployment (and by a local
//! smoke test); the coordination *effects* it triggers are unit-tested in
//! `crate::watch`.

use crate::state::AppState;
use crate::watch::{run, ChannelSource, WatchEvent};
use chapr_proto::CanonicalPath;
use std::ffi::c_void;
use std::io;
use tokio::sync::mpsc::Sender;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_OLD_NAME,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};

const ERROR_NOTIFY_ENUM_DIR: u32 = 1022;

/// Start the watcher: an OS thread runs the blocking watch loop and feeds the
/// async effect-runner via a channel. `watch_dir` is the local path to watch;
/// `share_unc` is the canonical UNC prefix used to map observed relative paths
/// back onto coord's canonical keys.
pub fn spawn(st: AppState, watch_dir: String, share_unc: String) {
    let (tx, rx) = tokio::sync::mpsc::channel::<WatchEvent>(256);
    std::thread::Builder::new()
        .name("chapr-watch".into())
        .spawn(move || {
            if let Err(e) = watch_loop(&watch_dir, &share_unc, &tx) {
                tracing::error!(error = %e, "watch thread exited");
            }
        })
        .expect("spawn watch thread");
    tokio::spawn(run(st, ChannelSource::new(rx)));
}

fn open_directory(path: &str) -> io::Result<HANDLE> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS, // required to open a directory handle
            HANDLE::default(),
        )
    }
    .map_err(|e| io::Error::from_raw_os_error((e.code().0 as u32 & 0xFFFF) as i32))
}

fn watch_loop(watch_dir: &str, share_unc: &str, tx: &Sender<WatchEvent>) -> io::Result<()> {
    let handle = open_directory(watch_dir)?;
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE;
    let mut buf = vec![0u8; 64 * 1024];

    let result = loop {
        let mut returned: u32 = 0;
        let call = unsafe {
            ReadDirectoryChangesW(
                handle,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                true, // watch the whole subtree
                filter,
                Some(&mut returned),
                None, // synchronous (blocks)
                None,
            )
        };
        match call {
            Ok(()) if returned == 0 => {
                // Buffer overflowed; events were lost → rescan.
                if tx.blocking_send(WatchEvent::Overflow).is_err() {
                    break Ok(());
                }
            }
            Ok(()) => {
                for (action, name) in parse(&buf[..returned as usize]) {
                    let path = to_canonical(share_unc, &name);
                    let event = if action == FILE_ACTION_REMOVED.0
                        || action == FILE_ACTION_RENAMED_OLD_NAME.0
                    {
                        WatchEvent::Removed(path)
                    } else {
                        WatchEvent::Changed(path)
                    };
                    if tx.blocking_send(event).is_err() {
                        break;
                    }
                }
            }
            Err(e) => {
                let code = e.code().0 as u32 & 0xFFFF;
                if code == ERROR_NOTIFY_ENUM_DIR {
                    if tx.blocking_send(WatchEvent::Overflow).is_err() {
                        break Ok(());
                    }
                } else {
                    break Err(io::Error::from_raw_os_error(code as i32));
                }
            }
        }
        // The receiver was dropped (coord shutting down).
        if tx.is_closed() {
            break Ok(());
        }
    };

    unsafe {
        let _ = CloseHandle(handle);
    }
    result
}

/// Parse a `FILE_NOTIFY_INFORMATION` chain into `(action, relative_name)` pairs.
fn parse(buf: &[u8]) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + 12 <= buf.len() {
        let next = u32::from_le_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]]) as usize;
        let action = u32::from_le_bytes([buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7]]);
        let name_len = u32::from_le_bytes([buf[offset + 8], buf[offset + 9], buf[offset + 10], buf[offset + 11]]) as usize;
        let name_start = offset + 12;
        if name_start + name_len > buf.len() {
            break;
        }
        // `as_chunks::<2>()` rather than `chunks_exact(2)`: the chunk size is a
        // constant, so this hands back `&[[u8; 2]]` and `from_le_bytes` takes the
        // array directly instead of re-indexing it. Behaviour is identical — both
        // drop a trailing odd byte, and `name_len` is always even because the field
        // is UTF-16. (clippy 1.98's `chunks_exact_to_as_chunks`; `as_chunks` is
        // stable as of 1.88, which is this workspace's declared MSRV.)
        let (pairs, _odd_tail) = buf[name_start..name_start + name_len].as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().copied().map(u16::from_le_bytes).collect();
        out.push((action, String::from_utf16_lossy(&units)));
        if next == 0 {
            break;
        }
        offset += next;
    }
    out
}

/// Map an observed relative path onto coord's canonical key (lowercased UNC).
/// A string-level approximation of §5.1 (NFC/DFS not applied here) — sufficient
/// for the flat ASCII paths of the target deployment; documented as such.
fn to_canonical(share_unc: &str, relative: &str) -> CanonicalPath {
    let joined = format!(
        "{}\\{}",
        share_unc.trim_end_matches('\\'),
        relative.replace('/', "\\")
    );
    CanonicalPath::new_unchecked(joined.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_canonical_lowercases_and_joins() {
        let c = to_canonical("\\\\SRV\\Share", "Sub/File.TXT");
        assert_eq!(c.as_str(), "\\\\srv\\share\\sub\\file.txt");
    }
}
