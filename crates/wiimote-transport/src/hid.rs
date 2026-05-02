use crate::{DeviceId, DeviceInfo, Transport, TransportError, TransportEvent};
use crossbeam_channel::{Sender, unbounded};
use hidapi::{HidApi, HidDevice};
use std::collections::HashMap;
use std::ffi::CString;
use std::thread;
use std::time::Duration;
use tracing::debug;
use wiimote_core::{is_wiimote, parse_input};

/// Per-device handle: outbound write channel + the join handle of the
/// I/O thread, kept here purely so we can drop it on close.
struct DeviceHandle {
    writer: Sender<Vec<u8>>,
    _thread: thread::JoinHandle<()>,
}

pub struct HidTransport {
    api: HidApi,
    handles: HashMap<String, DeviceHandle>,
    events: Sender<TransportEvent>,
}

impl HidTransport {
    pub fn new(events: Sender<TransportEvent>) -> Result<Self, TransportError> {
        Ok(Self {
            api: HidApi::new()?,
            handles: HashMap::new(),
            events,
        })
    }

    /// Re-scan and return all currently-attached Wiimote HID devices.
    /// Each entry carries both the OS HID `path` (for `open`) and the
    /// BT `mac` (stable across reconnects, used by the daemon as the
    /// canonical key).
    pub fn enumerate(&mut self) -> Result<Vec<DeviceInfo>, TransportError> {
        self.api.refresh_devices()?;
        let mut out = Vec::new();
        for d in self.api.device_list() {
            if !is_wiimote(d.vendor_id(), d.product_id()) {
                continue;
            }
            // hidapi exposes the OS device path as a CStr. Lossy UTF-8
            // conversion would mangle bytes and break the round-trip
            // through `CString::new` in `open()` (B15) — skip rather
            // than corrupt. In practice all real HID paths on Win/
            // Linux/macOS are ASCII.
            let path_bytes = d.path().to_bytes();
            let path = match std::str::from_utf8(path_bytes) {
                Ok(s) => s.to_owned(),
                Err(_) => {
                    debug!("skipping wiimote with non-UTF-8 HID path");
                    continue;
                }
            };
            let mac = d.serial_number().and_then(format_mac);
            out.push(DeviceInfo {
                id: DeviceId(path),
                name: d.product_string().unwrap_or("Wii Remote").to_string(),
                vendor_id: d.vendor_id(),
                product_id: d.product_id(),
                mac,
            });
        }
        Ok(out)
    }

    /// Open the device and start its I/O thread. Idempotent: re-opening an
    /// already-open device returns Ok.
    pub fn open(&mut self, info: &DeviceInfo) -> Result<(), TransportError> {
        if self.handles.contains_key(&info.id.0) {
            return Ok(());
        }
        let cpath = CString::new(info.id.0.clone())
            .map_err(|e| TransportError::Io(format!("path contains NUL: {e}")))?;
        let device = self.api.open_path(&cpath)?;
        // Wiimotes need non-blocking reads when we also want to drain writes
        // on the same thread; we use timeout-based reads instead.
        device
            .set_blocking_mode(true)
            .map_err(TransportError::from)?;

        let (write_tx, write_rx) = unbounded::<Vec<u8>>();
        let id = info.id.clone();
        let events = self.events.clone();

        let join = thread::Builder::new()
            .name(format!("wiimote-io-{}", short_id(&info.id.0)))
            .spawn(move || io_loop(id, device, write_rx, events))
            .map_err(|e| TransportError::Io(format!("spawn: {e}")))?;

        self.handles.insert(
            info.id.0.clone(),
            DeviceHandle {
                writer: write_tx,
                _thread: join,
            },
        );
        debug!(id = %info.id.0, "opened wiimote");
        Ok(())
    }
}

impl Transport for HidTransport {
    fn send(&mut self, id: &DeviceId, payload: &[u8]) -> Result<(), TransportError> {
        let h = self
            .handles
            .get(&id.0)
            .ok_or_else(|| TransportError::NotOpen(id.0.clone()))?;
        h.writer
            .send(payload.to_vec())
            .map_err(|_| TransportError::Io("device thread closed".into()))
    }

    fn close(&mut self, id: &DeviceId) -> Result<(), TransportError> {
        // Dropping the writer Sender causes the io_loop to exit on its
        // next iteration; the JoinHandle is dropped along with the entry.
        self.handles.remove(&id.0);
        Ok(())
    }
}

fn io_loop(
    id: DeviceId,
    device: HidDevice,
    writes: crossbeam_channel::Receiver<Vec<u8>>,
    events: Sender<TransportEvent>,
) {
    /// Wiimote in mode 0x31 with continuous=true should pump ~100 reports
    /// per second. If we go this many consecutive 50 ms read-timeouts
    /// (≈ 2 s) without any data, treat the device as offline — Windows
    /// often won't error the HID handle when the BT link drops.
    const READ_TIMEOUT_MS: i32 = 50;
    const IDLE_DEADLINE: u32 = 40;

    let mut buf = [0u8; 64];
    let mut idle_timeouts: u32 = 0;
    loop {
        // Drain any pending writes first — keeps latency low if the UI
        // changes LEDs / reporting mode.
        loop {
            match writes.try_recv() {
                Ok(mut payload) => {
                    // Windows' HID stack rejects writes shorter than
                    // the device's max output-report size; for the
                    // Wiimote that's 22 bytes (1 ID + 21 data). Without
                    // this, SetLeds/SetReportingMode writes fail
                    // silently and the Wiimote stays in pairing-blink
                    // mode forever.
                    #[cfg(target_os = "windows")]
                    if payload.len() < 22 {
                        payload.resize(22, 0);
                    }
                    if let Err(e) = device.write(&payload) {
                        // A failing write almost always means the BT
                        // link is gone. We don't surface it as a
                        // separate UI event — the read loop will see
                        // the same and fall through to DeviceLost.
                        debug!(?id, "hid write error (likely disconnect): {e}");
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    let _ = events.send(TransportEvent::DeviceLost(id.clone()));
                    return;
                }
            }
        }

        match device.read_timeout(&mut buf, READ_TIMEOUT_MS) {
            Ok(0) => {
                idle_timeouts = idle_timeouts.saturating_add(1);
                if idle_timeouts >= IDLE_DEADLINE {
                    debug!(
                        ?id,
                        "no reports for {} ms — declaring device offline",
                        READ_TIMEOUT_MS as u32 * IDLE_DEADLINE
                    );
                    break;
                }
                continue;
            }
            Ok(n) => {
                idle_timeouts = 0;
                match parse_input(&buf[..n]) {
                    Ok(report) => {
                        let _ = events.send(TransportEvent::Report {
                            id: id.clone(),
                            report,
                        });
                    }
                    Err(e) => {
                        // Non-fatal: many reports we don't yet decode.
                        debug!(?id, "unparsed report 0x{:02x}: {e}", buf[0]);
                    }
                }
            }
            Err(e) => {
                // Read errors are virtually always "device disconnected"
                // (HID handle invalidated by BT-link drop). Don't show
                // the verbose OS-locale error in the UI; the upcoming
                // DeviceLost event already speaks for itself.
                debug!(?id, "hid read error (likely disconnect): {e}");
                break;
            }
        }

        // Tiny yield so we don't pin a CPU core if the device is silent
        // and there are no writes pending.
        thread::sleep(Duration::from_millis(1));
    }
    let _ = events.send(TransportEvent::DeviceLost(id));
}

fn short_id(path: &str) -> String {
    path.chars().rev().take(12).collect::<String>().chars().rev().collect()
}

/// Convert a hidapi serial number (typically a 12-char lowercase hex
/// string for BT HID on Windows, e.g. `"e0e751b9d558"`) into the
/// canonical `AA:BB:CC:DD:EE:FF` form. Returns `None` for anything
/// that doesn't look like a 6-byte MAC.
fn format_mac(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() != 12 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let u = s.to_ascii_uppercase();
    Some(format!(
        "{}:{}:{}:{}:{}:{}",
        &u[0..2],
        &u[2..4],
        &u[4..6],
        &u[6..8],
        &u[8..10],
        &u[10..12]
    ))
}
