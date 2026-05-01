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
use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use wiimote_core::{
    Accelerometer, Buttons, ExtensionData, ExtensionType, InputReport, IrDots, OutputReport,
    PID_WIIMOTE, VID_NINTENDO,
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
    /// Set when the user explicitly clicked "Disconnect" — auto-retry
    /// stays off until they click "Connect" again. Cleared on Connect.
    pub user_disabled: bool,
    pub last_buttons: Buttons,
    pub last_accel: Accelerometer,
    pub last_ir: IrDots,
    pub battery: Option<u8>,
    /// Type of extension plugged into the Wiimote (Nunchuk, guitar, …).
    /// `None` until the post-status init dance completes, or after the
    /// extension is unplugged.
    pub extension: Option<ExtensionType>,
    /// Live decoded state of the extension (currently held buttons,
    /// stick positions, …). Filled in once the Wiimote is in reporting
    /// mode 0x35; cleared on unplug or disconnect.
    pub ext_data: Option<ExtensionData>,
    pub last_error: Option<String>,
}

/// Per-device state machine for extension identification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionPhase {
    /// We've sent the "0x55 to 0xa400f0" init write; awaiting Ack 0x22.
    InitSent,
    /// Init acked, we've requested the 6-byte ID; awaiting ReadResponse 0x21.
    ReadingId,
    /// Identified — won't redo unless extension is unplugged.
    Identified(ExtensionType),
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
    // HID handle is open but no first input report has arrived yet —
    // on Windows hidapi.open() succeeds even for paired-but-offline
    // Wiimotes, so opening alone is not proof of connectivity.
    let mut pending: HashSet<String> = HashSet::new();
    // Earliest moment at which we'll try opening a given device again.
    let mut next_retry: HashMap<String, Instant> = HashMap::new();
    // Extension identification finite-state machine, per device.
    let mut ext_phase: HashMap<String, ExtensionPhase> = HashMap::new();

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
                    if try_connect(&id, &mut devices, &mut hid, &mut pending) {
                        dirty = true;
                    } else {
                        next_retry.insert(id.clone(), Instant::now() + RETRY_INTERVAL);
                    }
                }
                UiCommand::Disconnect(id) => {
                    let _ = hid.close(&DeviceId(id.clone()));
                    outputs.remove(&id);
                    pending.remove(&id);
                    ext_phase.remove(&id);
                    if let Some(d) = devices.get_mut(&id) {
                        d.connected = false;
                        d.user_disabled = true;
                        d.extension = None;
                        d.ext_data = None;
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
                                    extension: None,
                                    ext_data: None,
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
            // to auto-connect, not already mid-handshake, and past its
            // cooldown.
            let now = Instant::now();
            let candidates: Vec<String> = devices
                .iter()
                .filter(|(id, snap)| {
                    !snap.connected
                        && !snap.user_disabled
                        && !pending.contains(id.as_str())
                        && next_retry.get(id.as_str()).is_none_or(|t| *t <= now)
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in candidates {
                if try_connect(&id, &mut devices, &mut hid, &mut pending) {
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
                    // First report after a tentative open confirms the
                    // device is really online — promote it to connected
                    // and now (and only now) plug the virtual pad.
                    if pending.remove(&id.0) {
                        promote_to_connected(
                            &id.0,
                            &mut devices,
                            &mut outputs,
                            &events_tx,
                        );
                        dirty = true;
                    }

                    // Extension identification FSM ---------------------
                    match &report {
                        InputReport::Status { flags, .. } => {
                            if flags.extension_connected {
                                let already = matches!(
                                    ext_phase.get(&id.0),
                                    Some(ExtensionPhase::Identified(_))
                                        | Some(ExtensionPhase::InitSent)
                                        | Some(ExtensionPhase::ReadingId)
                                );
                                if !already {
                                    let _ = hid.send(
                                        &id,
                                        &OutputReport::WriteRegister {
                                            address: 0x00a4_00f0,
                                            data: vec![0x55],
                                        }
                                        .encode(),
                                    );
                                    ext_phase.insert(
                                        id.0.clone(),
                                        ExtensionPhase::InitSent,
                                    );
                                }
                            } else {
                                let was_present = matches!(
                                    ext_phase.get(&id.0),
                                    Some(ExtensionPhase::Identified(_))
                                );
                                ext_phase.remove(&id.0);
                                if let Some(d) = devices.get_mut(&id.0) {
                                    if d.extension.is_some() || d.ext_data.is_some() {
                                        d.extension = None;
                                        d.ext_data = None;
                                        dirty = true;
                                    }
                                }
                                if let Some(s) = states.get_mut(&id.0) {
                                    s.ext = None;
                                }
                                // Drop back to the no-extension reporting
                                // mode so we stop receiving 16 bytes of
                                // junk extension payload per frame.
                                if was_present {
                                    let _ = hid.send(
                                        &id,
                                        &OutputReport::SetReportingMode {
                                            continuous: true,
                                            mode: 0x31,
                                        }
                                        .encode(),
                                    );
                                }
                            }
                        }
                        InputReport::Ack {
                            report_id, error, ..
                        } => {
                            if *report_id == 0x16
                                && *error == 0
                                && ext_phase.get(&id.0)
                                    == Some(&ExtensionPhase::InitSent)
                            {
                                let _ = hid.send(
                                    &id,
                                    &OutputReport::ReadRegister {
                                        address: 0x00a4_00fa,
                                        count: 6,
                                    }
                                    .encode(),
                                );
                                ext_phase.insert(
                                    id.0.clone(),
                                    ExtensionPhase::ReadingId,
                                );
                            }
                        }
                        InputReport::ReadResponse {
                            error,
                            size,
                            address,
                            data,
                            ..
                        } => {
                            if *error == 0
                                && *address == 0x00fa
                                && *size == 6
                                && ext_phase.get(&id.0)
                                    == Some(&ExtensionPhase::ReadingId)
                            {
                                let mut id_bytes = [0u8; 6];
                                id_bytes.copy_from_slice(&data[..6]);
                                let ext = ExtensionType::from_id(&id_bytes);
                                ext_phase.insert(
                                    id.0.clone(),
                                    ExtensionPhase::Identified(ext),
                                );
                                if let Some(d) = devices.get_mut(&id.0) {
                                    d.extension = Some(ext);
                                    dirty = true;
                                }
                                let _ = events_tx.send(UiEvent::Log {
                                    level: LogLevel::Info,
                                    message: format!(
                                        "extension on {}: {}",
                                        short_id(&id.0),
                                        ext.label()
                                    ),
                                });
                                // Switch to mode 0x35 so the Wiimote
                                // also streams the 16-byte extension
                                // payload alongside buttons + accel.
                                let _ = hid.send(
                                    &id,
                                    &OutputReport::SetReportingMode {
                                        continuous: true,
                                        mode: 0x35,
                                    }
                                    .encode(),
                                );
                            }
                        }
                        InputReport::ButtonsAccelExt { ext, .. } => {
                            if let Some(ExtensionPhase::Identified(et)) =
                                ext_phase.get(&id.0).copied()
                            {
                                let parsed = ExtensionData::parse(et, ext);
                                if let Some(d) = devices.get_mut(&id.0) {
                                    d.ext_data = Some(parsed);
                                    dirty = true;
                                }
                                states.entry(id.0.clone()).or_default().ext =
                                    Some(parsed);
                            }
                        }
                        _ => {}
                    }

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
                                debug!("output update failed: {e}");
                            }
                        }
                    }
                }
                TransportEvent::DeviceLost(id) => {
                    let was_connected =
                        devices.get(&id.0).map(|d| d.connected).unwrap_or(false);
                    if let Some(d) = devices.get_mut(&id.0) {
                        if was_connected {
                            let _ = events_tx.send(UiEvent::Log {
                                level: LogLevel::Info,
                                message: format!("device offline: {}", short_id(&id.0)),
                            });
                        }
                        d.connected = false;
                        d.extension = None;
                        d.ext_data = None;
                    }
                    pending.remove(&id.0);
                    outputs.remove(&id.0);
                    states.remove(&id.0);
                    ext_phase.remove(&id.0);
                    // The io_loop has exited; clean up its handle entry
                    // in the transport so the next `try_connect` spawns
                    // a fresh thread instead of short-circuiting on the
                    // dead-but-still-present handle.
                    let _ = hid.close(&id);
                    // After a real disconnect, retry quickly — likely the
                    // user is toggling the Wiimote off-and-on. After a
                    // tentative open that never produced a report, the
                    // device is genuinely off; back off for longer to
                    // avoid burning cycles re-opening the empty handle.
                    let cooldown = if was_connected {
                        QUICK_RETRY_AFTER_LOSS
                    } else {
                        RETRY_INTERVAL
                    };
                    next_retry.insert(id.0, Instant::now() + cooldown);
                    dirty = true;
                }
                TransportEvent::DeviceFound(_) => {}
                TransportEvent::Error { id, error } => {
                    // The transport itself rarely emits this anymore —
                    // I/O errors that mean "device is gone" are folded
                    // into DeviceLost. Anything that does land here is
                    // treated as a debug-level signal so it doesn't
                    // pollute the UI with OS-locale error strings.
                    debug!(?id, "transport error: {error}");
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

/// Open the HID device and set initial reporting. Returns `true` if
/// the open succeeded — the device is only marked **`pending`** here;
/// promotion to `connected = true` happens when the first input report
/// actually arrives, and *that* is when we plug a virtual controller.
///
/// Plugging ViGEm here would be wrong: on Windows `hid.open()` succeeds
/// even for paired-but-offline Wiimotes, so we'd repeatedly plug-and-
/// unplug a virtual Xbox 360 pad every retry cycle, which both confuses
/// games (XInput drops the controller) and can leave the ViGEmBus
/// driver in a stuck state.
fn try_connect(
    id: &str,
    devices: &mut HashMap<String, DeviceSnapshot>,
    hid: &mut HidTransport,
    pending: &mut HashSet<String>,
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
            // Tentative: confirmed when the first input report arrives.
            pending.insert(id.to_string());
            true
        }
        Err(e) => {
            // Demote to debug-level log: with auto-retry on, this can
            // fire every few seconds when the Wiimote is just off.
            tracing::debug!("open failed: {e}");
            if let Some(d) = devices.get_mut(id) {
                d.last_error = Some(format!("open failed: {e}"));
            }
            false
        }
    }
}

/// Called when the first real input report confirms a paired-and-online
/// device — flips it to `connected`, plugs the ViGEm virtual pad, and
/// surfaces logs. Idempotent in practice: the only call site is the
/// pending → connected transition in the report loop, which fires once.
fn promote_to_connected(
    id: &str,
    devices: &mut HashMap<String, DeviceSnapshot>,
    outputs: &mut HashMap<String, Box<dyn Output>>,
    events_tx: &Sender<UiEvent>,
) {
    if let Some(d) = devices.get_mut(id) {
        d.connected = true;
        d.last_error = None;
        let _ = events_tx.send(UiEvent::Log {
            level: LogLevel::Info,
            message: format!("connected: {} ({})", d.name, short_id(id)),
        });
    }
    match default_output() {
        Ok(out) => {
            outputs.insert(id.to_string(), out);
            let _ = events_tx.send(UiEvent::Log {
                level: LogLevel::Info,
                message: format!("virtual Xbox 360 pad ready for {}", short_id(id)),
            });
        }
        Err(e) => {
            // Technical detail (Win32 codes etc.) only goes to the
            // debug log; the UI sees a clean, actionable message.
            debug!("output init failed: {e}");
            let user_msg =
                "Virtual controller output unavailable — install or restart ViGEmBus";
            if let Some(d) = devices.get_mut(id) {
                d.last_error = Some(user_msg.into());
            }
            let _ = events_tx.send(UiEvent::Log {
                level: LogLevel::Warn,
                message: user_msg.into(),
            });
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
        InputReport::Ack { buttons, .. } | InputReport::ReadResponse { buttons, .. } => {
            (Some(*buttons), None, None, None)
        }
        InputReport::Buttons { buttons } => (Some(*buttons), None, None, None),
        InputReport::ButtonsAccel { buttons, accel } => {
            (Some(*buttons), Some(*accel), None, None)
        }
        InputReport::ButtonsAccelIr {
            buttons,
            accel,
            ir,
        } => (Some(*buttons), Some(*accel), Some(*ir), None),
        InputReport::ButtonsAccelExt { buttons, accel, .. } => {
            (Some(*buttons), Some(*accel), None, None)
        }
    }
}
