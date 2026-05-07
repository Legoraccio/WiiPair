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
use std::mem::size_of;
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
const PIN_LEN: u8 = 6;
/// Length of `pin_code` in `mgmt_cp_pin_code_reply`: a fixed 16-byte
/// buffer regardless of `pin_len`, per `linux/include/net/bluetooth/mgmt.h`.
const MGMT_PIN_BUFFER_LEN: usize = 16;

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

/// `struct mgmt_addr_info` from `linux/include/net/bluetooth/mgmt.h`.
/// Wire-encoded BD address followed by an address-type byte. Used only
/// via `size_of` to assert the expected wire size; we serialise fields
/// individually in [`send_pin_reply`].
#[repr(C, packed)]
#[allow(dead_code)]
struct MgmtAddrInfo {
    bdaddr: [u8; 6],
    addr_type: u8,
}

/// `struct mgmt_cp_pin_code_reply` from
/// `linux/include/net/bluetooth/mgmt.h`. Sent after the 6-byte
/// `mgmt_hdr` to answer `MGMT_EV_PIN_CODE_REQUEST`. Used only via
/// `size_of` for layout assertions; serialisation is manual.
#[repr(C, packed)]
#[allow(dead_code)]
struct MgmtCpPinCodeReply {
    addr: MgmtAddrInfo,
    pin_len: u8,
    pin_code: [u8; MGMT_PIN_BUFFER_LEN],
}

fn open_mgmt_socket() -> std::io::Result<OwnedFd> {
    // SAFETY: `libc::socket` takes integer constants and writes no
    // memory; failure is signalled by a negative return.
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
    // SAFETY: `raw` is a fresh, owned, non-negative fd we got from
    // `socket()` and have not yet handed to anything else.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let addr = SockaddrHci {
        sa_family: AF_BLUETOOTH as u16,
        hci_dev: HCI_DEV_NONE,
        hci_channel: HCI_CHANNEL_CONTROL,
    };
    // SAFETY: `addr` is fully initialised on the stack and lives across
    // the call; the cast pointer is valid for `size_of::<SockaddrHci>()`
    // bytes; `fd.as_raw_fd()` is a valid socket fd we just opened.
    let r = unsafe {
        libc::bind(
            fd.as_raw_fd(),
            (&addr as *const SockaddrHci).cast::<libc::sockaddr>(),
            size_of::<SockaddrHci>() as libc::socklen_t,
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
        // SAFETY: `fd` is a live mgmt socket; `buf` is a fully-owned
        // stack array and we pass `buf.len()` as the bound, so the
        // kernel can write at most that many bytes into it.
        let n = unsafe {
            libc::read(
                fd.as_raw_fd(),
                buf.as_mut_ptr().cast::<libc::c_void>(),
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
    // Layout: 6-byte mgmt_hdr (opcode + index + payload_len) followed
    // by `MgmtCpPinCodeReply`. Manual serialisation keeps the unsafe
    // surface tiny, while `size_of` of the structs above guarantees
    // the byte counts can't drift if the kernel ever adds fields.
    const HDR_LEN: usize = 6;
    const ADDR_LEN: usize = size_of::<MgmtAddrInfo>();
    const CP_LEN: usize = size_of::<MgmtCpPinCodeReply>();
    const TOTAL_LEN: usize = HDR_LEN + CP_LEN;
    // Compile-time assertion: the packed structs have the exact wire
    // sizes the kernel ABI expects.
    const _: () = assert!(ADDR_LEN == 7);
    const _: () = assert!(CP_LEN == 7 + 1 + MGMT_PIN_BUFFER_LEN);

    let mut reply = [0u8; TOTAL_LEN];
    // mgmt_hdr
    reply[0..2].copy_from_slice(&MGMT_OP_PIN_CODE_REPLY.to_le_bytes());
    reply[2..4].copy_from_slice(&index.to_le_bytes());
    reply[4..6].copy_from_slice(&(CP_LEN as u16).to_le_bytes());
    // mgmt_addr_info
    reply[HDR_LEN..HDR_LEN + 6].copy_from_slice(&wire_addr);
    reply[HDR_LEN + 6] = BDADDR_BREDR;
    // pin_len
    reply[HDR_LEN + ADDR_LEN] = PIN_LEN;
    // pin_code: first 6 bytes are the wire-form BD address; rest stay 0.
    let pin_off = HDR_LEN + ADDR_LEN + 1;
    reply[pin_off..pin_off + 6].copy_from_slice(&wire_addr);

    // SAFETY: `reply` is a fully-initialised stack array of TOTAL_LEN
    // bytes; `fd` is a live mgmt socket. The length we pass matches
    // the array length.
    let written = unsafe {
        libc::write(
            fd.as_raw_fd(),
            reply.as_ptr().cast::<libc::c_void>(),
            reply.len(),
        )
    };
    if written != reply.len() as isize {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
