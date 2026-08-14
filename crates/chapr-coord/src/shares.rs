//! Local SMB share discovery for the setup wizard.
//!
//! The wizard has to learn one value it cannot guess: which share this
//! coordinator is fronting. Asking for it as free text put a customer in the
//! position of typing `\\servername\folder$` from memory, and a share path typed
//! from memory is the beginning of invariant 5 going wrong — two people naming
//! the same file differently map to two different leases.
//!
//! So the wizard asks the machine instead. `NetShareEnum` is the OS answering
//! "what do you actually serve", the same shape of answer as
//! `WNetGetUniversalNameW` on the endpoint side (E-022/D-030): cheaper than the
//! documentation the alternative would need, and correct when someone guesses
//! wrong.
//!
//! The filter is the part with judgement in it, so it lives out here as pure
//! string/integer logic that unit-tests on any platform; only the enumeration
//! itself is Windows-gated (the cross-platform-buildable choice, D-C).

/// A share worth offering as the coordinated location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalShare {
    pub name: String,
    pub remark: String,
}

impl LocalShare {
    /// The UNC form, which is what coordination state is keyed by (invariant 5).
    pub fn unc(&self, host: &str) -> String {
        format!(r"\\{host}\{}", self.name)
    }
}

/// `shi1_type` bit layout, from the Win32 `STYPE_*` constants. Named here so the
/// filter reads as intent rather than as arithmetic.
const STYPE_MASK: u32 = 0xFF;
const STYPE_DISKTREE: u32 = 0;
const STYPE_SPECIAL: u32 = 0x8000_0000;

/// Disk shares that exist for the domain, not for users' documents.
const NEVER_OFFERED: &[&str] = &["NETLOGON", "SYSVOL", "print$"];

/// Whether a share should be offered as a candidate coordinated location.
///
/// The subtle case, and the reason this is a named function with tests rather
/// than an inline condition: **a trailing `$` is not what makes a share
/// administrative.** `C$`, `D$`, `ADMIN$` and `IPC$` carry the `STYPE_SPECIAL`
/// bit; a hidden share an administrator created — like the customer's `mappe$` —
/// does not. Filtering on the `$` would have silently dropped exactly the share
/// the wizard exists to find.
pub fn is_offerable(name: &str, share_type: u32) -> bool {
    if share_type & STYPE_MASK != STYPE_DISKTREE {
        return false; // printers, devices, IPC
    }
    if share_type & STYPE_SPECIAL != 0 {
        return false; // C$, ADMIN$, and friends
    }
    !NEVER_OFFERED.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// Shares this machine serves, best-effort.
///
/// Never fails and never blocks the wizard: an error, a non-Windows host, or a
/// server with nothing shared all yield an empty list, and the prompt falls back
/// to free text. Discovery is a convenience, and a convenience that can stop an
/// install is worse than no convenience.
#[cfg(windows)]
pub fn local_shares() -> Vec<LocalShare> {
    use windows::Win32::NetworkManagement::NetManagement::{
        NetApiBufferFree, MAX_PREFERRED_LENGTH,
    };
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{NetShareEnum, SHARE_INFO_1};

    let mut out = Vec::new();
    let mut buf: *mut u8 = std::ptr::null_mut();
    let mut read: u32 = 0;
    let mut total: u32 = 0;

    // SAFETY: `servername` null means the local machine. Level 1 fills
    // `SHARE_INFO_1`, which is what `entries` is cast to below, and netapi32
    // allocates `buf` — freed unconditionally at the end of this block.
    unsafe {
        let rc = NetShareEnum(
            PCWSTR::null(),
            1,
            &mut buf,
            MAX_PREFERRED_LENGTH,
            &mut read,
            &mut total,
            None,
        );
        // 0 = NERR_Success, 234 = ERROR_MORE_DATA (a partial answer is still an
        // answer; the wizard offers what it got).
        if (rc == 0 || rc == 234) && !buf.is_null() {
            let entries = std::slice::from_raw_parts(buf as *const SHARE_INFO_1, read as usize);
            for e in entries {
                let name = e.shi1_netname.to_string().unwrap_or_default();
                if name.is_empty() || !is_offerable(&name, e.shi1_type.0) {
                    continue;
                }
                out.push(LocalShare {
                    name,
                    remark: e.shi1_remark.to_string().unwrap_or_default(),
                });
            }
        }
        if !buf.is_null() {
            NetApiBufferFree(Some(buf as *const core::ffi::c_void));
        }
    }
    out.sort_by_key(|s| s.name.to_lowercase());
    out
}

#[cfg(not(windows))]
pub fn local_shares() -> Vec<LocalShare> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case that made this a function: the customer's real share is hidden
    /// (`mappe$`) but not administrative, and a `$`-based filter would have
    /// dropped it while keeping nothing useful.
    #[test]
    fn a_hidden_user_share_is_offered_but_an_admin_share_is_not() {
        assert!(is_offerable("mappe$", STYPE_DISKTREE));
        assert!(is_offerable("AICollab", STYPE_DISKTREE));

        for admin in ["C$", "D$", "ADMIN$"] {
            assert!(
                !is_offerable(admin, STYPE_DISKTREE | STYPE_SPECIAL),
                "{admin} should be filtered out"
            );
        }
    }

    #[test]
    fn only_disk_trees_are_offered() {
        assert!(!is_offerable("Reception", 1)); // STYPE_PRINTQ
        assert!(!is_offerable("SomeDevice", 2)); // STYPE_DEVICE
        assert!(!is_offerable("IPC$", 3 | STYPE_SPECIAL)); // STYPE_IPC
    }

    #[test]
    fn domain_shares_are_not_document_locations() {
        assert!(!is_offerable("NETLOGON", STYPE_DISKTREE));
        assert!(!is_offerable("sysvol", STYPE_DISKTREE), "match is case-insensitive");
    }

    /// A temporary share is still a disk tree, and still a legitimate place to
    /// put documents — the bit says how it was created, not what it is for.
    #[test]
    fn a_temporary_share_is_still_offered() {
        assert!(is_offerable("Scratch", STYPE_DISKTREE | 0x4000_0000));
    }

    #[test]
    fn unc_is_built_the_way_coordination_state_is_keyed() {
        let s = LocalShare {
            name: "mappe$".into(),
            remark: String::new(),
        };
        assert_eq!(s.unc("FILESRV01"), r"\\FILESRV01\mappe$");
    }

    #[cfg(not(windows))]
    #[test]
    fn discovery_is_empty_rather_than_failing_off_windows() {
        assert!(local_shares().is_empty());
    }
}
