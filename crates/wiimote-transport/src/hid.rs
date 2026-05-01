use crate::{DeviceId, DeviceInfo, Transport, TransportError, TransportEvent};
use crossbeam_channel::{Sender, unbounded};
use hidapi::{HidApi, HidDevice};
use std::collections::HashMap;
use std::ffi::CString;
use std::thread;
use std::time::Duration;
use tracing::{debug, warn};
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
    /// On Windows this requires the Wiimote to already be paired via the
    /// OS Bluetooth stack — auto-pairing is a separate step (TODO).
    pub fn enumerate(&mut self) -> Result<Vec<DeviceInfo>, TransportError> {
        self.api.refresh_devices()?;
        let mut out = Vec::new();
        for d in self.api.device_list() {
            if !is_wiimote(d.vendor_id(), d.product_id()) {
                continue;
            }
            let path = d.path().to_string_lossy().into_owned();
            out.push(DeviceInfo {
                id: DeviceId(path),
                name: d.product_string().unwrap_or("Wii Remote").to_string(),
                vendor_id: d.vendor_id(),
                product_id: d.product_id(),
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
    /// Surface gaps between reports — healthy is ~10 ms; anything over
    /// this is interesting (Bluetooth sniff windows, inquiry stalls,
    /// host scheduler hiccups).
    const GAP_WARN_MS: u128 = 80;
    /// Don't emit gap events to the UI more often than this — gaps
    /// often come in bursts, and a wall of identical log lines isn't
    /// useful.
    const GAP_EMIT_BACKOFF: Duration = Duration::from_millis(800);

    let mut buf = [0u8; 64];
    let mut idle_timeouts: u32 = 0;
    let mut last_report_at: Option<std::time::Instant> = None;
    let mut last_gap_emit: Option<std::time::Instant> = None;
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
                let now = std::time::Instant::now();
                if let Some(prev) = last_report_at {
                    let gap_ms = now.duration_since(prev).as_millis();
                    if gap_ms > GAP_WARN_MS {
                        let due_emit = last_gap_emit
                            .map(|t| now.duration_since(t) >= GAP_EMIT_BACKOFF)
                            .unwrap_or(true);
                        if due_emit {
                            last_gap_emit = Some(now);
                            warn!(?id, "report gap: {} ms", gap_ms);
                            // The UI-facing gap log is emitted by the
                            // daemon directly off of inter-arrival
                            // timestamps; this `warn!` is just the
                            // terminal/stderr breadcrumb.
                        }
                    }
                }
                last_report_at = Some(now);
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
