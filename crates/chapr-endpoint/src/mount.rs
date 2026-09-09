// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! The live per-session mount table — mapped drive letter → UNC (E-022).
//!
//! Invariant 5 keys every piece of coordination state by canonical path, so two
//! users naming the same file differently **must** land on the same key. A mapped
//! drive breaks that in the one way that is invisible: `Z:\Tenders\bid.docx` on
//! one laptop and `P:\Tenders\bid.docx` on another are the same file on the share
//! and two different keys in coord.
//!
//! Live bytes survive it — invariant 3 puts correctness on exclusive-open + CAS,
//! never on leases — so the loser of a race still gets a conflict sidecar rather
//! than silence. What does not survive is everything keyed *beside* the bytes: a
//! torn-file marker or a conflict entry written under one alias is invisible
//! under the other, which is exactly the silent-corruption path D-027 closed.
//! The change-watcher makes it worse rather than better, because it keys UNC and
//! would invalidate index entries no endpoint ever looks up.
//!
//! D-030 chose resolution over requiring every user to map the same letter: a
//! requirement whose violation cannot be detected is not a safeguard, and a
//! mismatched letter errors nowhere. `WNetGetUniversalNameW` is the OS answering
//! the question directly, which is both cheaper than the documentation the
//! requirement would need and correct when someone ignores it.

/// Resolves a local path to the UNC name of the share it lives on.
///
/// A trait rather than a bare function so canonicalisation can be exercised
/// against a fake table on any platform: the real implementation needs a live
/// Windows session with actual drive mappings, which no unit test can rely on.
pub trait MountTable: Send + Sync {
    /// `Ok(Some(unc))` — `local_path` is on a mapped network drive and `unc` is
    /// its universal name. `Ok(None)` — the path is genuinely local and has no
    /// UNC form. `Err` — the lookup itself failed and the caller must not guess.
    fn universal_name(&self, local_path: &str) -> Result<Option<String>, String>;
}

/// A table with no mappings: every path is local.
///
/// The POSIX default, and the default in tests. Not a stub — on a backend with
/// no drive-letter concept this is the truthful answer.
pub struct NoMountTable;

impl MountTable for NoMountTable {
    fn universal_name(&self, _local_path: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
}

/// The mount table this process should use.
pub fn default_mounts() -> &'static dyn MountTable {
    #[cfg(windows)]
    {
        &WinMountTable
    }
    #[cfg(not(windows))]
    {
        &NoMountTable
    }
}

/// The real Windows mount table, via `WNetGetUniversalNameW` (mpr.dll).
#[cfg(windows)]
pub struct WinMountTable;

#[cfg(windows)]
impl MountTable for WinMountTable {
    fn universal_name(&self, local_path: &str) -> Result<Option<String>, String> {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{
            ERROR_MORE_DATA, ERROR_NOT_CONNECTED, ERROR_NO_NET_OR_BAD_PATH, NO_ERROR,
        };
        use windows::Win32::NetworkManagement::WNet::{
            WNetGetUniversalNameW, UNIVERSAL_NAME_INFOW, UNIVERSAL_NAME_INFO_LEVEL,
        };

        let wide: Vec<u16> = local_path
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        // The API writes a `UNIVERSAL_NAME_INFOW` at the head of the buffer whose
        // one field points at a string stored in the *same* buffer, so the buffer
        // must hold both. Start generous, then honour one ERROR_MORE_DATA — the
        // second attempt uses the size the OS asked for, so two rounds is enough
        // by construction rather than by luck.
        let mut size: u32 = 1024;
        for attempt in 0..2 {
            // `u64` alignment: the buffer is cast to a struct containing a
            // pointer, and a `Vec<u8>` gives no such guarantee.
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let rc = unsafe {
                WNetGetUniversalNameW(
                    PCWSTR(wide.as_ptr()),
                    UNIVERSAL_NAME_INFO_LEVEL,
                    buf.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            match rc {
                NO_ERROR => {
                    let info = unsafe { &*(buf.as_ptr() as *const UNIVERSAL_NAME_INFOW) };
                    if info.lpUniversalName.is_null() {
                        return Err("WNetGetUniversalNameW returned no name".into());
                    }
                    let unc = unsafe { info.lpUniversalName.to_string() }
                        .map_err(|e| format!("universal name is not valid UTF-16: {e}"))?;
                    return Ok(Some(unc));
                }
                // Not a network mapping — a genuinely local path. Not an error:
                // the caller distinguishes "local" from "unresolvable".
                ERROR_NOT_CONNECTED | ERROR_NO_NET_OR_BAD_PATH => return Ok(None),
                ERROR_MORE_DATA if attempt == 0 => continue,
                other => {
                    return Err(format!(
                        "WNetGetUniversalNameW failed with Win32 error {}",
                        other.0
                    ))
                }
            }
        }
        Err("WNetGetUniversalNameW kept asking for a larger buffer".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table with one mapping, for canonicalisation tests.
    pub struct FakeMounts {
        pub drive: &'static str,
        pub share: &'static str,
    }

    impl MountTable for FakeMounts {
        fn universal_name(&self, local_path: &str) -> Result<Option<String>, String> {
            match local_path.strip_prefix(self.drive) {
                Some(rest) => Ok(Some(format!("{}{}", self.share, rest))),
                None => Ok(None),
            }
        }
    }

    #[test]
    fn no_mount_table_reports_everything_local() {
        assert_eq!(NoMountTable.universal_name("Z:\\a\\b.md").unwrap(), None);
    }

    /// Exercises the **real** FFI: buffer sizing, the `u64` alignment the struct
    /// cast needs, and the `ERROR_NOT_CONNECTED` → `Ok(None)` mapping. `C:\` is
    /// always a local volume, so the expected answer is deterministic.
    ///
    /// The mapped-drive branch cannot be reached from a unit test — it needs a
    /// live network mapping, which a build agent has no reason to have — so that
    /// path is covered by the fake table in `canon`'s tests plus manual
    /// verification against a real share before rollout.
    #[cfg(windows)]
    #[test]
    fn the_real_table_reports_a_local_path_as_local() {
        assert_eq!(WinMountTable.universal_name("C:\\Windows").unwrap(), None);
    }

    #[test]
    fn a_fake_table_maps_only_its_own_drive() {
        let m = FakeMounts {
            drive: "Z:",
            share: "\\\\srv\\share",
        };
        assert_eq!(
            m.universal_name("Z:\\Tenders\\bid.docx").unwrap(),
            Some("\\\\srv\\share\\Tenders\\bid.docx".to_string())
        );
        // A different letter is not this mapping, and must not be guessed at.
        assert_eq!(m.universal_name("P:\\Tenders\\bid.docx").unwrap(), None);
    }

    /// **E-022's unverified branch**, against a real mapped drive.
    ///
    /// Ignored by default and opt-in on purpose: `WinMountTable` calls
    /// `WNetGetUniversalNameW`, which needs a live Windows session with an actual
    /// mapping. There is no way to fake that and still be testing the thing, so
    /// every other test in this module drives `FakeMounts` instead — which proves
    /// the *canonicalisation* around the lookup and never the lookup itself.
    ///
    /// That gap is why E-022 has carried a live `HIGH` row while marked DONE.
    /// Run it against the rig:
    ///
    /// ```text
    /// net use Z: \\CHAPR-FS\chaprtest
    /// set CHAPR_TEST_MAPPED_DRIVE=Z:\
    /// cargo test -p chapr-endpoint --lib real_mapped_drive -- --ignored --nocapture
    /// ```
    ///
    /// Asserts the shape rather than a literal, so it works against any rig: the
    /// answer must be `Some`, must be a UNC path, and must not still be a drive
    /// letter — the three ways this call can be wrong without erroring.
    #[test]
    #[ignore = "needs a real mapped network drive; set CHAPR_TEST_MAPPED_DRIVE"]
    #[cfg(windows)]
    fn real_mapped_drive_resolves_to_unc() {
        let Ok(drive) = std::env::var("CHAPR_TEST_MAPPED_DRIVE") else {
            panic!("set CHAPR_TEST_MAPPED_DRIVE to a mapped drive path, e.g. Z:\\");
        };

        let got = default_mounts()
            .universal_name(&drive)
            .unwrap_or_else(|e| panic!("universal_name({drive}) failed: {e}"));

        let unc = got.unwrap_or_else(|| {
            panic!(
                "universal_name({drive}) returned Ok(None) — 'this path is genuinely \
                 local'. It is a mapped network drive, so None is wrong: two users \
                 with different letters would key the same file differently and \
                 invariant 5 would not hold."
            )
        });

        assert!(unc.starts_with("\\\\"), "expected a UNC name, got {unc:?}");
        assert!(
            !unc.starts_with(&drive[..2]),
            "the drive letter survived into the canonical form: {unc:?}"
        );
        println!("  E-022: {drive} -> {unc}");
    }
}
