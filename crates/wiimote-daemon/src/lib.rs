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

mod commands;
mod extension_fsm;
mod helpers;
mod hid_scan;
mod persist;
mod scanner;
mod state;
mod ticks;
mod transport;
pub mod ui_log;

pub use ui_log::{UiLogLayer, install_ui_log_sender};

use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tracing::{info, warn};
use wiimote_core::OutputReport;
use wiimote_output::MappingProfile;
use wiimote_transport::hid::HidTransport;
use wiimote_transport::platform::PlatformScanner;
use wiimote_transport::{DeviceId, Transport};

use state::{DeviceRegistry, DeviceRuntime};

// =====================================================================
// Public re-exports (UI consumers)
// =====================================================================

pub use state::DeviceSnapshot;

// =====================================================================
// Tunables
// =====================================================================

/// How often we revisit disconnected (but known) devices to try opening
/// them again — picks up Wiimotes that have just been turned back on.
pub(crate) const RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// Right after a `DeviceLost` event we retry sooner — the Wiimote
/// might be cycled off-and-on quickly.
pub(crate) const QUICK_RETRY_AFTER_LOSS: Duration = Duration::from_millis(800);
/// How often we send a no-op `RequestStatus` to each connected Wiimote
/// to keep the BT-HID link active. Aggressive (200 ms = 5/s) because
/// Windows negotiates BT sniff intervals around 1.28 s by default — a
/// slower keepalive gets queued through sniff windows and lets the
/// link starve between them, showing up as 1.2-1.5 s input freezes.
pub(crate) const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(200);
/// How long a user-initiated scan stays open. While active, the BT
/// scanner runs inquiries even with controllers already connected.
pub(crate) const MANUAL_SCAN_DURATION: Duration = Duration::from_secs(30);
/// If a `Pairing` event hasn't been followed by `Paired`/`PairFailed`
/// within this long, the BT stack is almost certainly hung — surface
/// a recovery dialog. The threshold is conservative because real pair
/// attempts can take 5-10 s on slow chipsets.
pub(crate) const PAIR_STUCK_THRESHOLD: Duration = Duration::from_secs(20);
/// Duration of the rumble pulse for the Identify command.
pub(crate) const IDENTIFY_RUMBLE_MS: u64 = 600;
/// Tick rate of the periodic scan + retry-connect block.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// Inter-arrival gap above which the daemon emits a UI log warning.
pub(crate) const REPORT_GAP_WARN_MS: u128 = 80;
/// Minimum spacing between report-gap log lines per device.
pub(crate) const GAP_LOG_BACKOFF: Duration = Duration::from_millis(800);
/// How long to wait between successive `output_for_profile` retries
/// when ViGEmBus / uinput is transiently unavailable. Three seconds
/// covers ViGEmBus' driver-restart time without flooding the log.
pub(crate) const OUTPUT_RETRY_INTERVAL: Duration = Duration::from_secs(3);

// =====================================================================
// Public types
// =====================================================================

#[derive(Debug, Clone)]
pub enum UiEvent {
    DeviceListChanged(Vec<DeviceSnapshot>),
    /// A log line. `at` is captured at the moment the daemon emits the
    /// event, so a backed-up channel doesn't smear timestamps across
    /// the batch the UI drains in one frame.
    Log {
        at: SystemTime,
        level: LogLevel,
        message: String,
    },
    /// User-initiated discovery window state. `Some(deadline)` means a
    /// scan is currently active until that instant; `None` means no
    /// scan window is active.
    ScanState { active_until: Option<Instant> },
    /// A pairing attempt has been in progress for an unusually long
    /// time — Windows' BT stack is likely stuck. The UI uses this to
    /// pop a recovery-instructions dialog.
    PairingStuck { addr: u64 },
}

fn log_event(level: LogLevel, message: impl Into<String>) -> UiEvent {
    let message = message.into();
    // Mirror to tracing so the same line shows up on stderr when the
    // user runs the binary from a terminal. The `LOG_TARGET_DIRECT`
    // marker tells the UI tracing layer to skip these — they're
    // already going to the UI via the `UiEvent::Log` we return below,
    // and we don't want them counted twice.
    match level {
        LogLevel::Info => {
            tracing::info!(target: ui_log::LOG_TARGET_DIRECT, "{message}")
        }
        LogLevel::Warn => {
            tracing::warn!(target: ui_log::LOG_TARGET_DIRECT, "{message}")
        }
        LogLevel::Error => {
            tracing::error!(target: ui_log::LOG_TARGET_DIRECT, "{message}")
        }
    }
    UiEvent::Log {
        at: SystemTime::now(),
        level,
        message,
    }
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
    /// Drop a device entirely from the persisted list (and disconnect
    /// it first if it's currently connected).
    Forget(String),
    /// Make the device announce itself: rumble pulse on Wiimotes; for
    /// devices without rumble we briefly flash the player LEDs.
    Identify(String),
    /// Open a discovery window in which the BT scanner runs inquiries
    /// even while controllers are already connected.
    StartScan,
    /// Change the mapping profile for this device. Re-creates the
    /// virtual gamepad if currently connected so the new layout
    /// applies immediately.
    SetMappingProfile {
        id: String,
        profile: MappingProfile,
    },
}

pub struct Daemon {
    pub events_rx: Receiver<UiEvent>,
    /// `None` once we've initiated shutdown — dropping the Sender is
    /// our shutdown signal, so we explicitly take it during `Drop`
    /// before joining the worker thread.
    commands_tx: Option<Sender<UiCommand>>,
    /// Clone handed to `UiLogLayer` so the tracing bridge can post
    /// log lines into the same channel the daemon already feeds.
    events_tx: Sender<UiEvent>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Drop the command Sender first — that disconnects the channel
        // and lets the daemon worker exit its run loop cleanly (turn
        // LEDs off on connected Wiimotes, drain pending writes). Then
        // we wait for it to actually exit.
        self.commands_tx.take();
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

impl Daemon {
    pub fn start() -> anyhow::Result<Self> {
        let (events_tx, events_rx) = unbounded();
        let (commands_tx, commands_rx) = unbounded();

        let events_tx_thread = events_tx.clone();
        let handle = thread::Builder::new()
            .name("wiimote-daemon".into())
            .spawn(move || {
                if let Err(e) = run(events_tx_thread.clone(), commands_rx) {
                    let _ = events_tx_thread.send(log_event(
                        LogLevel::Error,
                        format!("daemon stopped: {e}"),
                    ));
                }
            })?;

        Ok(Self {
            events_rx,
            commands_tx: Some(commands_tx),
            events_tx,
            thread: Some(handle),
        })
    }

    /// Forward a command to the daemon's worker thread. Silently drops
    /// the command if the daemon has already begun shutdown — by then
    /// there's nothing left to act on it.
    pub fn send_command(&self, cmd: UiCommand) {
        if let Some(tx) = self.commands_tx.as_ref() {
            let _ = tx.send(cmd);
        }
    }

    /// Hand out a sender clone so the tracing-to-UI bridge can post
    /// log lines into the same channel the daemon already feeds.
    #[must_use]
    pub fn log_sender(&self) -> Sender<UiEvent> {
        self.events_tx.clone()
    }
}

// =====================================================================
// Daemon-loop state — every map keyed on a device id used to live in
// run() as a free local; bundling them into one struct cuts the
// teardown ceremony in `Forget`/`Disconnect`/`DeviceLost` from ~10
// lines down to one `registry.remove()`.
// =====================================================================

struct DaemonCtx {
    registry: DeviceRegistry,
    /// When `Some(t)`, BT inquiry is forced on until `t` regardless of
    /// whether a Wiimote is connected. Set by `UiCommand::StartScan`.
    manual_scan_until: Option<Instant>,
    /// Active pairing attempts and when they started — used to detect
    /// BT-stack hangs and surface a recovery dialog.
    pair_started: HashMap<u64, Instant>,
    /// Addresses we've already notified about — prevents emitting
    /// `PairingStuck` repeatedly for the same hung attempt.
    pair_stuck_signaled: HashSet<u64>,
    /// Addresses for which we've auto-unpaired during the current scan
    /// window. Reset when the scan window closes so the user can retry
    /// after the next "Scan for new devices" click.
    sdp_recovery_attempted: HashSet<u64>,
    /// Set when the persisted on-disk config is stale.
    persist_dirty: bool,
    /// Set when the UI snapshot is stale.
    dirty: bool,
    /// Set when a transport report arrived under an unknown HID path —
    /// triggers an HID re-enumeration on the next scan tick (B7).
    force_rescan: bool,
}

impl DaemonCtx {
    fn new() -> Self {
        Self {
            registry: DeviceRegistry::default(),
            manual_scan_until: None,
            pair_started: HashMap::new(),
            pair_stuck_signaled: HashSet::new(),
            sdp_recovery_attempted: HashSet::new(),
            persist_dirty: false,
            // Initial snapshot flush ensures the UI sees the persisted
            // offline placeholders without waiting for a real change.
            dirty: true,
            force_rescan: false,
        }
    }
}

// =====================================================================
// Main loop
// =====================================================================

fn run(events_tx: Sender<UiEvent>, commands_rx: Receiver<UiCommand>) -> anyhow::Result<()> {
    let (transport_tx, transport_rx) = unbounded();
    let mut hid = HidTransport::new(transport_tx)?;

    let (scanner_tx, scanner_rx) = unbounded();
    let mut scanner = PlatformScanner::new(scanner_tx)?;
    let scan_pause = scanner.pause_handle();
    if let Err(e) = scanner.start() {
        warn!("bluetooth scanner not available: {e}");
        let _ = events_tx.send(log_event(
            LogLevel::Warn,
            format!("scanner disabled: {e}"),
        ));
    }

    let mut ctx = DaemonCtx::new();

    // Restore persisted devices as offline placeholders — the auto-
    // retry loop picks them up if they come online.
    let mut restored = 0usize;
    for pd in persist::load() {
        let snap = persist::into_snapshot(pd);
        ctx.registry.insert(DeviceRuntime::new(snap));
        restored += 1;
    }

    let mut last_scan = Instant::now()
        .checked_sub(SCAN_INTERVAL)
        .unwrap_or_else(Instant::now);

    info!("daemon started");
    let _ = events_tx.send(log_event(
        LogLevel::Info,
        format!(
            "daemon started: {} known device{} restored, auto-retry every {}s, \
             keepalive every {}ms",
            restored,
            if restored == 1 { "" } else { "s" },
            RETRY_INTERVAL.as_secs(),
            KEEPALIVE_INTERVAL.as_millis()
        ),
    ));

    loop {
        // 1) UI commands ---------------------------------------------------
        // The Sender side living on the `Daemon` struct is dropped when
        // the UI exits — that disconnects this Receiver and is our
        // shutdown signal.
        loop {
            match commands_rx.try_recv() {
                Ok(cmd) => crate::commands::handle_command(cmd, &mut ctx, &mut hid, &events_tx),
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    info!("daemon quitting (commands channel disconnected)");
                    shutdown(&mut ctx, &mut hid);
                    return Ok(());
                }
            }
        }

        // 2a) Bluetooth scanner events ------------------------------------
        while let Ok(ev) = scanner_rx.try_recv() {
            crate::scanner::handle_scanner_event(ev, &mut ctx, &events_tx);
        }

        // 2b) Periodic HID enumerate + auto-(re)connect -------------------
        if ctx.force_rescan || last_scan.elapsed() >= SCAN_INTERVAL {
            last_scan = Instant::now();
            crate::hid_scan::tick_periodic_scan(&mut ctx, &mut hid, &events_tx);
        }

        // 3) Transport events ---------------------------------------------
        while let Ok(ev) = transport_rx.try_recv() {
            crate::transport::handle_transport_event(ev, &mut ctx, &mut hid, &events_tx);
        }

        // 4) Periodic ticks -----------------------------------------------
        let now = Instant::now();
        crate::ticks::tick_keepalive(now, &mut ctx, &mut hid);
        crate::ticks::tick_pair_stuck(now, &mut ctx, &events_tx);
        crate::ticks::tick_rumble_off(now, &mut ctx, &mut hid);
        crate::ticks::tick_manual_scan_window(now, &mut ctx, &events_tx);
        crate::hid_scan::tick_output_retry(now, &mut ctx, &events_tx);

        // 5) Pause active BT inquiry while controllers are connected and
        // the user isn't explicitly asking to find new ones.
        let pause_inquiry = ctx.registry.any_connected() && ctx.manual_scan_until.is_none();
        scan_pause.store(pause_inquiry, std::sync::atomic::Ordering::Relaxed);

        // 6) Flush state to UI / disk -------------------------------------
        if ctx.dirty {
            let list = ctx.registry.snapshots();
            let _ = events_tx.send(UiEvent::DeviceListChanged(list));
            if ctx.persist_dirty {
                let map: HashMap<String, _> = ctx
                    .registry
                    .iter()
                    .map(|(k, _)| k.clone())
                    .zip(ctx.registry.values().map(crate::helpers::clone_runtime_view))
                    .collect();
                persist::save(&map);
                ctx.persist_dirty = false;
            }
            ctx.dirty = false;
        }

        thread::sleep(Duration::from_millis(10));
    }
}

/// Best-effort: turn LEDs off on every still-connected Wiimote so they
/// don't sit lit-up after we exit. No-ops on any device that's already
/// gone.
fn shutdown(ctx: &mut DaemonCtx, hid: &mut HidTransport) {
    let connected_paths: Vec<String> = ctx
        .registry
        .values()
        .filter(|r| r.snapshot.connected)
        .map(|r| r.snapshot.path.clone())
        .collect();
    for path in connected_paths {
        let _ = hid.send(
            &DeviceId(path),
            &OutputReport::SetLeds {
                leds: 0,
                rumble: false,
            }
            .encode(),
        );
    }
    // Brief pause for the io_loops to drain those writes.
    thread::sleep(Duration::from_millis(100));
}
