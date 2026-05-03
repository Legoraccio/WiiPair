//! Self-grant of `CAP_NET_ADMIN` via polkit/`pkexec`.
//!
//! WiiPair's mgmt-socket PIN helper needs `CAP_NET_ADMIN` to bind
//! `HCI_CHANNEL_CONTROL`. Asking the user to remember `sudo setcap`
//! after every `cargo build` is hostile UX — instead, on every
//! launch we check whether the binary already has the cap and, if
//! not, run `pkexec setcap cap_net_admin+ep $0` (which surfaces the
//! standard polkit password dialog) then re-exec ourselves so the
//! freshly-applied cap takes effect for this session.
//!
//! Failure modes (no `pkexec`, user cancels, fs without xattrs, …)
//! all degrade to "PIN helper unavailable" — pairing then falls back
//! to the broken UTF-8 agent path, which works only for Wiimotes
//! whose reversed BD address happens to be all-ASCII (rare). The
//! daemon already surfaces that limitation as a warn in the UI log.
//!
//! Anti-loop guard: if we've already re-exec'd once and still don't
//! see the cap (bug, fs quirk), we stop trying and just continue —
//! the existing PIN-helper warn path takes over.

use std::os::unix::process::CommandExt;
use std::process::Command;

const RETRY_ENV: &str = "WIIPAIR_CAP_RETRY";

/// CAP_NET_ADMIN is bit 12 in the capability bitmask exposed via
/// `/proc/self/status` (`include/uapi/linux/capability.h`).
const CAP_NET_ADMIN: u32 = 12;

pub fn ensure_cap_net_admin() {
    if has_cap_net_admin() {
        return;
    }
    if std::env::var_os(RETRY_ENV).is_some() {
        eprintln!(
            "wiipair: CAP_NET_ADMIN still missing after pkexec re-exec — \
             continuing without the PIN helper. Pairing may fail."
        );
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("wiipair: cannot resolve current_exe ({e}) — skipping cap grant.");
            return;
        }
    };

    eprintln!(
        "wiipair: requesting CAP_NET_ADMIN via pkexec (one-time per build)…"
    );
    let status = Command::new("pkexec")
        .arg("setcap")
        .arg("cap_net_admin+ep")
        .arg(&exe)
        .status();
    match status {
        Ok(s) if s.success() => {
            // The exec'd process replaces this one. On success it
            // never returns; on failure we fall through with the
            // returned io::Error and continue degraded.
            let err = Command::new(&exe)
                .args(std::env::args().skip(1))
                .env(RETRY_ENV, "1")
                .exec();
            eprintln!(
                "wiipair: pkexec setcap succeeded but re-exec failed ({err}) — \
                 continuing in the original (cap-less) process."
            );
        }
        Ok(s) => {
            eprintln!(
                "wiipair: pkexec setcap exited {s} (likely cancelled or polkit \
                 denied) — continuing without the PIN helper. Pairing may fail."
            );
        }
        Err(e) => {
            eprintln!(
                "wiipair: pkexec not available ({e}) — install policykit-1 / \
                 polkit, or run `sudo setcap cap_net_admin+ep {}` manually.",
                exe.display(),
            );
        }
    }
}

fn has_cap_net_admin() -> bool {
    let status = match std::fs::read_to_string("/proc/self/status") {
        Ok(s) => s,
        Err(_) => return false,
    };
    for line in status.lines() {
        // CapEff is the effective set: hex u64, MSB first.
        if let Some(hex) = line.strip_prefix("CapEff:") {
            if let Ok(bits) = u64::from_str_radix(hex.trim(), 16) {
                return (bits >> CAP_NET_ADMIN) & 1 != 0;
            }
        }
    }
    false
}
