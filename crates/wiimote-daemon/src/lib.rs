//! Orchestration layer: scan loop, device registry, glue between
//! transport (Wiimote-side) and output (OS-side). Owns one background
//! thread; UI talks to it via two channels.
//!
//! Connection lifecycle:
//! * Newly-seen devices are auto-connected (user can later disable).
//! * Connected devices that go quiet (Wiimote powered off) are
//!   detected by the transport's inactivity watchdog → `DeviceLost` →
//!   we mark them disconnected and queue a quick re-attempt.
//! * `Disconnect` from the UI sets a sticky `user_disabled` flag so the
//!   periodic auto-retry stays out of the way until `Connect` is hit
//!   again.

use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use wiimote_core::{
    Accelerometer, Buttons, InputReport, IrDots, OutputReport, PID_WIIMOTE, VID_NINTENDO,
};
use wiimote_output::{ControllerState, Output, default_output};
use wiimote_transport::hid::HidTransport;
use wiimote_transport::platform::{PlatformScanner, ScannerEvent};
use wiimote_transport::{DeviceId, DeviceInfo, Transport, TransportEvent};

/// How often we revisit disconnected (but known) devices to try opening
/// them again — picks up Wiimotes that have just been turned back on.
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// Right after a `DeviceLost` event we retry sooner — the Wiimote
/// might be cycled off-and-on quickly.
const QUICK_RETRY_AFTER_LOSS: Duration = Duration::from_millis(800);

#[derive(Debug, Clone)]
pub struct DeviceSnapshot {
    pub id: String,
    pub name: String,
    pub connected: bool,
    /// Set when the user explicitly clicked "Disconnetti" — auto-retry
    /// stays off until they click "Connetti" again. Cleared on Connect.
    pub user_disabled: bool,
    pub last_buttons: Buttons,
    pub last_accel: Accelerometer,
    pub last_ir: IrDots,
    pub battery: Option<u8>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    DeviceListChanged(Vec<DeviceSnapshot>),
    Log { level: LogLevel, message: String },
}

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug)]
pub enum UiCommand {
    Connect(String),
    Disconnect(String),
    Quit,
}

pub struct Daemon {
    pub events_rx: Receiver<UiEvent>,
    pub commands_tx: Sender<UiCommand>,
    _thread: thread::JoinHandle<()>,
}

impl Daemon {
    pub fn start() -> anyhow::Result<Self> {
        let (events_tx, events_rx) = unbounded();
        let (commands_tx, commands_rx) = unbounded();

        let handle = thread::Builder::new()
            .name("wiimote-daemon".into())
            .spawn(move || {
                if let Err(e) = run(events_tx.clone(), commands_rx) {
                    let _ = events_tx.send(UiEvent::Log {
                        level: LogLevel::Error,
                        message: format!("daemon stopped: {e}"),
                    });
                }
            })?;

        Ok(Self {
            events_rx,
            commands_tx,
            _thread: handle,
        })
    }
}

fn run(events_tx: Sender<UiEvent>, commands_rx: Receiver<UiCommand>) -> anyhow::Result<()> {
    let (transport_tx, transport_rx) = unbounded();
    let mut hid = HidTransport::new(transport_tx)?;

    let (scanner_tx, scanner_rx) = unbounded();
    let mut scanner = PlatformScanner::new(scanner_tx)?;
    if let Err(e) = scanner.start() {
        warn!("bluetooth scanner not available: {e}");
        let _ = events_tx.send(UiEvent::Log {
            level: LogLevel::Warn,
            message: format!("scanner disabled: {e}"),
        });
    }

    let mut devices: HashMap<String, DeviceSnapshot> = HashMap::new();
    let mut states: HashMap<String, ControllerState> = HashMap::new();
    let mut outputs: HashMap<String, Box<dyn Output>> = HashMap::new();
    // Earliest moment at which we'll try opening a given device again.
    let mut next_retry: HashMap<String, Instant> = HashMap::new();

    let scan_interval = Duration::from_secs(2);
    let mut last_scan = Instant::now()
        .checked_sub(scan_interval)
        .unwrap_or_else(Instant::now);
    let mut dirty = true;
    let mut force_rescan = false;

    info!("daemon started");

    loop {
        // 1) UI commands ---------------------------------------------------
        while let Ok(cmd) = commands_rx.try_recv() {
            match cmd {
                UiCommand::Quit => {
                    info!("daemon quitting");
                    return Ok(());
                }
                UiCommand::Connect(id) => {
                    if let Some(d) = devices.get_mut(&id) {
                        d.user_disabled = false;
                    }
                    next_retry.remove(&id);
                    if try_connect(&id, &mut devices, &mut hid, &mut outputs, &events_tx) {
                        dirty = true;
                    } else {
                        next_retry.insert(id.clone(), Instant::now() + RETRY_INTERVAL);
                    }
                }
                UiCommand::Disconnect(id) => {
                    let _ = hid.close(&DeviceId(id.clone()));
                    outputs.remove(&id);
                    if let Some(d) = devices.get_mut(&id) {
                        d.connected = false;
                        d.user_disabled = true;
                    }
                    next_retry.remove(&id);
                    dirty = true;
                }
            }
        }

        // 2a) Bluetooth scanner events ------------------------------------
        while let Ok(ev) = scanner_rx.try_recv() {
            match ev {
                ScannerEvent::Discovered {
                    addr,
                    name,
                    paired,
                    connected,
                } => {
                    let _ = events_tx.send(UiEvent::Log {
                        level: LogLevel::Info,
                        message: format!(
                            "[BT] discovered {name} {} (paired={paired}, connected={connected})",
                            format_addr(addr)
                        ),
                    });
                }
                ScannerEvent::Pairing { addr } => {
                    let _ = events_tx.send(UiEvent::Log {
                        level: LogLevel::Info,
                        message: format!("[BT] pairing {}…", format_addr(addr)),
                    });
                }
                ScannerEvent::Paired { addr } => {
                    let _ = events_tx.send(UiEvent::Log {
                        level: LogLevel::Info,
                        message: format!("[BT] paired {}", format_addr(addr)),
                    });
                }
                ScannerEvent::PairFailed { addr, reason } => {
                    let _ = events_tx.send(UiEvent::Log {
                        level: LogLevel::Warn,
                        message: format!(
                            "[BT] pair failed {}: {reason}",
                            format_addr(addr)
                        ),
                    });
                }
                ScannerEvent::HidEnabled { addr } => {
                    let _ = events_tx.send(UiEvent::Log {
                        level: LogLevel::Info,
                        message: format!(
                            "[BT] HID service enabled on {}",
                            format_addr(addr)
                        ),
                    });
                    force_rescan = true;
                }
                ScannerEvent::Error(e) => {
                    let _ = events_tx.send(UiEvent::Log {
                        level: LogLevel::Warn,
                        message: format!("[BT] {e}"),
                    });
                }
            }
        }

        // 2b) Periodic HID scan + auto-(re)connect ------------------------
        if force_rescan || last_scan.elapsed() >= scan_interval {
            force_rescan = false;
            last_scan = Instant::now();
            match hid.enumerate() {
                Ok(found) => {
                    for f in &found {
                        if !devices.contains_key(&f.id.0) {
                            devices.insert(
                                f.id.0.clone(),
                                DeviceSnapshot {
                                    id: f.id.0.clone(),
                                    name: f.name.clone(),
                                    connected: false,
                                    user_disabled: false,
                                    last_buttons: Buttons::default(),
                                    last_accel: Accelerometer::default(),
                                    last_ir: IrDots::default(),
                                    battery: None,
                                    last_error: None,
                                },
                            );
                            // Auto-connect on first sight: drop a stale
                            // retry guard, then attempt below.
                            next_retry.remove(&f.id.0);
                            dirty = true;
                        }
                    }
                }
                Err(e) => warn!("scan failed: {e}"),
            }

            // Try (re)connecting any device that is known, idle, allowed
            // to auto-connect, and past its cooldown.
            let now = Instant::now();
            let candidates: Vec<String> = devices
                .iter()
                .filter(|(id, snap)| {
                    !snap.connected
                        && !snap.user_disabled
                        && next_retry.get(id.as_str()).is_none_or(|t| *t <= now)
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in candidates {
                if try_connect(&id, &mut devices, &mut hid, &mut outputs, &events_tx) {
                    next_retry.remove(&id);
                    dirty = true;
                } else {
                    next_retry.insert(id, now + RETRY_INTERVAL);
                }
            }
        }

        // 3) Transport events ---------------------------------------------
        while let Ok(ev) = transport_rx.try_recv() {
            match ev {
                TransportEvent::Report { id, report } => {
                    let (buttons, accel, ir, battery) = decompose(&report);
                    if let Some(b) = buttons {
                        if let Some(d) = devices.get_mut(&id.0) {
                            d.last_buttons = b;
                        }
                        states.entry(id.0.clone()).or_default().buttons = b;
                        dirty = true;
                    }
                    if let Some(a) = accel {
                        if let Some(d) = devices.get_mut(&id.0) {
                            d.last_accel = a;
                        }
                        states.entry(id.0.clone()).or_default().accel = a;
                        dirty = true;
                    }
                    if let Some(i) = ir {
                        if let Some(d) = devices.get_mut(&id.0) {
                            d.last_ir = i;
                        }
                        states.entry(id.0.clone()).or_default().ir = i;
                    }
                    if let Some(bat) = battery {
                        if let Some(d) = devices.get_mut(&id.0) {
                            d.battery = Some(bat);
                            dirty = true;
                        }
                    }
                    if let Some(out) = outputs.get_mut(&id.0) {
                        if let Some(s) = states.get(&id.0) {
                            if let Err(e) = out.update(s) {
                                warn!("output update failed: {e}");
                            }
                        }
                    }
                }
                TransportEvent::DeviceLost(id) => {
                    if let Some(d) = devices.get_mut(&id.0) {
                        if d.connected {
                            let _ = events_tx.send(UiEvent::Log {
                                level: LogLevel::Info,
                                message: format!("device offline: {}", short_id(&id.0)),
                            });
                        }
                        d.connected = false;
                    }
                    outputs.remove(&id.0);
                    states.remove(&id.0);
                    // Quick retry — common case is the user briefly
                    // toggling the Wiimote off and back on.
                    next_retry.insert(id.0, Instant::now() + QUICK_RETRY_AFTER_LOSS);
                    dirty = true;
                }
                TransportEvent::DeviceFound(_) => {}
                TransportEvent::Error { id, error } => {
                    let msg = format!("transport error: {error}");
                    warn!(?id, "{msg}");
                    if let Some(idv) = id {
                        if let Some(d) = devices.get_mut(&idv.0) {
                            d.last_error = Some(msg.clone());
                            dirty = true;
                        }
                    }
                    let _ = events_tx.send(UiEvent::Log {
                        level: LogLevel::Warn,
                        message: msg,
                    });
                }
            }
        }

        if dirty {
            let list: Vec<_> = devices.values().cloned().collect();
            let _ = events_tx.send(UiEvent::DeviceListChanged(list));
            dirty = false;
        }

        thread::sleep(Duration::from_millis(10));
    }
}

/// Open the HID device, configure reporting, and arm the virtual
/// output. Returns `true` on full success. Failure paths surface a
/// log entry and stash the error in the device snapshot.
fn try_connect(
    id: &str,
    devices: &mut HashMap<String, DeviceSnapshot>,
    hid: &mut HidTransport,
    outputs: &mut HashMap<String, Box<dyn Output>>,
    events_tx: &Sender<UiEvent>,
) -> bool {
    let snap = match devices.get(id).cloned() {
        Some(s) => s,
        None => return false,
    };
    let info = DeviceInfo {
        id: DeviceId(snap.id.clone()),
        name: snap.name.clone(),
        vendor_id: VID_NINTENDO,
        product_id: PID_WIIMOTE,
    };
    match hid.open(&info) {
        Ok(()) => {
            let _ = hid.send(&info.id, &OutputReport::SetLeds { leds: 0b0001 }.encode());
            // 0x31 = buttons + 3-axis accel; continuous so the watchdog
            // in the transport sees a steady stream of reports.
            let _ = hid.send(
                &info.id,
                &OutputReport::SetReportingMode {
                    continuous: true,
                    mode: 0x31,
                }
                .encode(),
            );
            let _ = hid.send(&info.id, &OutputReport::RequestStatus.encode());

            match default_output() {
                Ok(out) => {
                    outputs.insert(id.to_string(), out);
                }
                Err(e) => {
                    let msg = format!("output disabled: {e}");
                    warn!("{msg}");
                    if let Some(d) = devices.get_mut(id) {
                        d.last_error = Some(msg.clone());
                    }
                    let _ = events_tx.send(UiEvent::Log {
                        level: LogLevel::Warn,
                        message: msg,
                    });
                }
            }

            if let Some(d) = devices.get_mut(id) {
                d.connected = true;
                d.last_error = None;
            }
            let _ = events_tx.send(UiEvent::Log {
                level: LogLevel::Info,
                message: format!("connected: {} ({})", snap.name, short_id(id)),
            });
            true
        }
        Err(e) => {
            let msg = format!("open failed: {e}");
            // Demote to debug-level log: with auto-retry on, this can
            // fire every few seconds when the Wiimote is just off.
            tracing::debug!("{msg}");
            if let Some(d) = devices.get_mut(id) {
                d.last_error = Some(msg);
            }
            false
        }
    }
}

fn format_addr(addr: u64) -> String {
    let b = addr.to_le_bytes();
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        b[5], b[4], b[3], b[2], b[1], b[0]
    )
}

fn short_id(id: &str) -> String {
    if id.len() <= 16 {
        id.to_string()
    } else {
        let tail = &id[id.len() - 16..];
        format!("…{tail}")
    }
}

/// Pull whatever fields a given report carries.
fn decompose(
    r: &InputReport,
) -> (
    Option<Buttons>,
    Option<Accelerometer>,
    Option<IrDots>,
    Option<u8>,
) {
    match r {
        InputReport::Status {
            buttons, battery, ..
        } => (Some(*buttons), None, None, Some(*battery)),
        InputReport::Buttons { buttons } => (Some(*buttons), None, None, None),
        InputReport::ButtonsAccel { buttons, accel } => {
            (Some(*buttons), Some(*accel), None, None)
        }
        InputReport::ButtonsAccelIr {
            buttons,
            accel,
            ir,
        } => (Some(*buttons), Some(*accel), Some(*ir), None),
    }
}
