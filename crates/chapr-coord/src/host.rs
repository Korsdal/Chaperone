// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Facts about the machine the installer is running on.
//!
//! Two questions the wizard has to answer, both previously answered by the
//! PowerShell wrapper that this replaces (D-032):
//!
//! - **Are we elevated?** Registering a Windows service needs it, and finding out
//!   at the registration step means the administrator watches the install fail
//!   after committing to a config.
//! - **Do we own this console window?** A double-clicked exe gets a console that
//!   closes when the process exits, so a message printed on the way out is a
//!   message nobody reads. That single behaviour is most of why running the exe
//!   directly felt untrustworthy enough to wrap in a script.
//!
//! Both degrade to the permissive answer off Windows and on error: the wizard
//! warns rather than refuses, and never blocks on a fact it could not establish.

/// Whether this process runs with local-administrator rights.
///
/// Checked against the **well-known SID** `S-1-5-32-544`, not the display name
/// "Administrators" — the same reason `harden_data_dirs` passes `*S-1-5-32-544`
/// to `icacls`. Display names are localized, and this customer's servers are not
/// guaranteed to be English.
#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{BOOL, HANDLE};
    use windows::Win32::Security::{
        AllocateAndInitializeSid, CheckTokenMembership, FreeSid, PSID, SID_IDENTIFIER_AUTHORITY,
    };

    const SECURITY_NT_AUTHORITY: SID_IDENTIFIER_AUTHORITY =
        SID_IDENTIFIER_AUTHORITY { Value: [0, 0, 0, 0, 0, 5] };
    const SECURITY_BUILTIN_DOMAIN_RID: u32 = 32;
    const DOMAIN_ALIAS_RID_ADMINS: u32 = 544;

    let mut sid = PSID::default();
    // SAFETY: two sub-authorities are supplied to match `nsubauthoritycount`; the
    // SID is freed on both exits below.
    unsafe {
        if AllocateAndInitializeSid(
            &SECURITY_NT_AUTHORITY,
            2,
            SECURITY_BUILTIN_DOMAIN_RID,
            DOMAIN_ALIAS_RID_ADMINS,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut sid,
        )
        .is_err()
        {
            return false;
        }
        let mut member = BOOL(0);
        // A null token handle means "the calling thread's effective token", which
        // is what makes this reflect elevation rather than group membership.
        let ok = CheckTokenMembership(HANDLE::default(), sid, &mut member).is_ok();
        FreeSid(sid);
        ok && member.as_bool()
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    // systemd install paths run as root and check differently; nothing here needs
    // to gate on it, so do not pretend to know.
    true
}

/// Whether this process is the only one attached to its console — i.e. the window
/// belongs to us and will close with us.
///
/// True for a double-click; false when launched from an existing shell, where the
/// output survives and pausing would just hang a script.
#[cfg(windows)]
pub fn owns_console() -> bool {
    use windows::Win32::System::Console::GetConsoleProcessList;
    let mut pids = [0u32; 2];
    // SAFETY: the slice is the buffer and its length; the return value is the
    // process count, and 0 means the call failed.
    let count = unsafe { GetConsoleProcessList(&mut pids) };
    count == 1
}

#[cfg(not(windows))]
pub fn owns_console() -> bool {
    false
}

/// Hold a console window open so its last message can be read.
///
/// A no-op unless we own the console, so scripted and service invocations are
/// unaffected. Deliberately not a timed pause: the reason to stop is that someone
/// has to read an error, and a five-second window is long enough to notice and
/// too short to act on.
pub fn pause_if_own_console() {
    if !owns_console() {
        return;
    }
    use std::io::{BufRead, Write};
    print!("\nPress Enter to close this window... ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
}
