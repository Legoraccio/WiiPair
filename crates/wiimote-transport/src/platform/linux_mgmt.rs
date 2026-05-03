//! Raw BlueZ kernel mgmt socket helper for legacy-pairing PIN replies.
//!
//! The Wiimote pairs with a 6-byte raw PIN equal to its own BD address
//! sent on the wire (LSB-first). The standard BlueZ DBus Agent1 API
//! marshals the PIN as a UTF-8 string, which can't carry arbitrary
//! bytes — `dbus-daemon` validates UTF-8 and even raw `as char` casts
//! get re-encoded as multi-byte UTF-8 sequences that arrive at the
//! controller mangled.
//!
//! The kernel mgmt socket (one-level below BlueZ) accepts a raw 16-byte
//! buffer in `MGMT_OP_PIN_CODE_REPLY`. We open it directly, listen for
//! `MGMT_EV_PIN_CODE_REQUEST`, and answer with the wire bytes. BlueZ's
//! own agent flow still runs in parallel — it loses the race because
//! kernel→us is microseconds while kernel→BlueZ→DBus→us(callback)→
//! DBus→BlueZ→kernel is milliseconds, so by the time BlueZ tries to
//! reply the kernel has already accepted ours and the late reply just
//! gets a harmless ENOENT.
//!
//! Opening the socket needs `CAP_NET_ADMIN`. If the call fails with
//! EPERM, we log a Warn (with a setcap hint) and exit the helper —
//! pairing then falls back to BlueZ's broken UTF-8 path, which works
//! only for Wiimotes whose reversed BD address happens to be all
//! ASCII (rare). The README documents the setcap step.
//!
//! Only addresses present in the shared `ActiveSet` get a reply, so a
//! parallel pairing of an unrelated device (Bluetooth keyboard, etc.)
//! still goes through BlueZ's normal path.

use crate::platform::ScannerEvent;
use crossbeam_channel::Sender;
use std::collections::HashSet;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::thread;
use tracing::{info, warn};

const AF_BLUETOOTH: libc::c_int = 31;
const BTPROTO_HCI: libc::c_int = 1;
const HCI_CHANNEL_CONTROL: u16 = 3;
const HCI_DEV_NONE: u16 = 0xffff;

const MGMT_EV_PIN_CODE_REQUEST: u16 = 0x000E;
const MGMT_OP_PIN_CODE_REPLY: u16 = 0x0016;

const BDADDR_BREDR: u8 = 0;
const PIN_LEN: usize = 6;

/// MAC address in little-endian (wire) order — the same representation
/// the kernel uses in mgmt event payloads. The Wiimote's expected PIN
/// is exactly these 6 bytes.
pub type WireAddr = [u8; 6];

/// Set of Wiimote addresses (LSB-first) that the helper should answer
/// for. Populated by the scan loop right before `device.pair()` and
/// drained immediately after, so unrelated PIN requests fall through
/// to BlueZ's default agent.
pub type ActiveSet = Arc<Mutex<HashSet<WireAddr>>>;

pub fn new_active_set() -> ActiveSet {
    Arc::new(Mutex::new(HashSet::new()))
}

/// Reverse a `bluer::Address` (MSB-first) into wire/LSB-first form.
pub fn wire_from_msb(msb: [u8; 6]) -> WireAddr {
    let mut w = [0u8; 6];
    for (i, b) in msb.iter().rev().enumerate() {
        w[i] = *b;
    }
    w
}

#[repr(C, packed)]
struct SockaddrHci {
    sa_family: u16,
    hci_dev: u16,
    hci_channel: u16,
}

fn open_mgmt_socket() -> std::io::Result<OwnedFd> {
    let raw = unsafe {
        libc::socket(
            AF_BLUETOOTH,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            BTPROTO_HCI,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let addr = SockaddrHci {
        sa_family: AF_BLUETOOTH as u16,
        hci_dev: HCI_DEV_NONE,
        hci_channel: HCI_CHANNEL_CONTROL,
    };
    let r = unsafe {
        libc::bind(
            fd.as_raw_fd(),
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<SockaddrHci>() as libc::socklen_t,
        )
    };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

/// Spawn the PIN helper thread. Returns immediately; failures to open
/// the socket are surfaced as a `ScannerEvent::Error` (so the user
/// sees it in the UI log) and the thread exits.
pub fn start(active: ActiveSet, events: Sender<ScannerEvent>) {
    let _ = thread::Builder::new()
        .name("bt-pin-helper".into())
        .spawn(move || run(active, events));
}

fn run(active: ActiveSet, events: Sender<ScannerEvent>) {
    let fd = match open_mgmt_socket() {
        Ok(f) => f,
        Err(e) => {
            let kind = e.kind();
            let msg = if kind == std::io::ErrorKind::PermissionDenied {
                "PIN helper unavailable (no CAP_NET_ADMIN). Wiimote pairing \
                 will likely fail with Authentication Rejected. Run once: \
                 `sudo setcap cap_net_admin+ep ./target/debug/wiipair` (or \
                 the release binary path)."
                    .to_string()
            } else {
                format!("PIN helper: failed to open mgmt socket: {e}")
            };
            warn!("{msg}");
            let _ = events.send(ScannerEvent::Error(msg));
            return;
        }
    };
    info!("PIN helper listening on BlueZ mgmt socket");

    let mut buf = [0u8; 1024];
    loop {
        let n = unsafe {
            libc::read(
                fd.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            warn!("PIN helper read failed: {e} — exiting");
            return;
        }
        let n = n as usize;
        if n < 6 {
            continue;
        }
        let opcode = u16::from_le_bytes([buf[0], buf[1]]);
        let index = u16::from_le_bytes([buf[2], buf[3]]);
        let payload_len = u16::from_le_bytes([buf[4], buf[5]]) as usize;
        if 6 + payload_len > n || opcode != MGMT_EV_PIN_CODE_REQUEST {
            continue;
        }
        // mgmt_ev_pin_code_request: bdaddr[6] + type[1] + secure[1]
        if payload_len < 8 {
            continue;
        }
        let mut wire_addr = [0u8; 6];
        wire_addr.copy_from_slice(&buf[6..12]);

        let interested = active
            .lock()
            .map(|s| s.contains(&wire_addr))
            .unwrap_or(false);
        if !interested {
            continue;
        }

        if let Err(e) = send_pin_reply(&fd, index, wire_addr) {
            warn!("PIN helper write failed: {e}");
        } else {
            info!(
                "PIN helper replied to {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} \
                 (idx {index})",
                wire_addr[5], wire_addr[4], wire_addr[3], wire_addr[2], wire_addr[1], wire_addr[0],
            );
        }
    }
}

fn send_pin_reply(fd: &OwnedFd, index: u16, wire_addr: WireAddr) -> std::io::Result<()> {
    // mgmt_hdr (6) + mgmt_addr_info (7) + pin_len (1) + pin_code (16)
    let mut reply = [0u8; 30];
    reply[0..2].copy_from_slice(&MGMT_OP_PIN_CODE_REPLY.to_le_bytes());
    reply[2..4].copy_from_slice(&index.to_le_bytes());
    let cp_len: u16 = (7 + 1 + 16) as u16;
    reply[4..6].copy_from_slice(&cp_len.to_le_bytes());
    // mgmt_addr_info
    reply[6..12].copy_from_slice(&wire_addr);
    reply[12] = BDADDR_BREDR;
    // pin_len
    reply[13] = PIN_LEN as u8;
    // pin_code: first 6 bytes are the wire-form BD address; rest stay 0.
    reply[14..20].copy_from_slice(&wire_addr);

    let written = unsafe {
        libc::write(
            fd.as_raw_fd(),
            reply.as_ptr() as *const libc::c_void,
            reply.len(),
        )
    };
    if written != reply.len() as isize {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
