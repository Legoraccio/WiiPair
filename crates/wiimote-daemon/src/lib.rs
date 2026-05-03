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

mod persist;
mod state;

use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tracing::{debug, info, warn};
use wiimote_core::{
    Buttons, ExtensionData, ExtensionType, InputReport, OutputReport, PID_WIIMOTE, VID_NINTENDO,
};
use wiimote_output::{MappingProfile, output_for_profile};
use wiimote_transport::hid::HidTransport;
use wiimote_transport::platform::{PlatformScanner, ScannerEvent, mac_to_u64, unpair_addr};
use wiimote_transport::{DeviceId, DeviceInfo, Transport, TransportEvent};

use state::{DeviceRegistry, DeviceRuntime, ExtensionPhase};

// =====================================================================
// Public re-exports (UI consumers)
// =====================================================================

pub use state::DeviceSnapshot;

// =====================================================================
// Tunables
// =====================================================================

/// How often we revisit disconnected (but known) devices to try opening
/// them again — picks up Wiimotes that have just been turned back on.
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// Right after a `DeviceLost` event we retry sooner — the Wiimote
/// might be cycled off-and-on quickly.
const QUICK_RETRY_AFTER_LOSS: Duration = Duration::from_millis(800);
/// How often we send a no-op `RequestStatus` to each connected Wiimote
/// to keep the BT-HID link active. Aggressive (200 ms = 5/s) because
/// Windows negotiates BT sniff intervals around 1.28 s by default — a
/// slower keepalive gets queued through sniff windows and lets the
/// link starve between them, showing up as 1.2-1.5 s input freezes.
const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(200);
/// How long a user-initiated scan stays open. While active, the BT
/// scanner runs inquiries even with controllers already connected.
const MANUAL_SCAN_DURATION: Duration = Duration::from_secs(30);
/// If a `Pairing` event hasn't been followed by `Paired`/`PairFailed`
/// within this long, the BT stack is almost certainly hung — surface
/// a recovery dialog. The threshold is conservative because real pair
/// attempts can take 5-10 s on slow chipsets.
const PAIR_STUCK_THRESHOLD: Duration = Duration::from_secs(20);
/// Duration of the rumble pulse for the Identify command.
const IDENTIFY_RUMBLE_MS: u64 = 600;
/// Tick rate of the periodic scan + retry-connect block.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// Inter-arrival gap above which the daemon emits a UI log warning.
const REPORT_GAP_WARN_MS: u128 = 80;
/// Minimum spacing between report-gap log lines per device.
const GAP_LOG_BACKOFF: Duration = Duration::from_millis(800);

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
    UiEvent::Log {
        at: SystemTime::now(),
        level,
        message: message.into(),
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
    Quit,
}

pub struct Daemon {
    pub events_rx: Receiver<UiEvent>,
    pub commands_tx: Sender<UiCommand>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Tell the daemon thread to clean up (turn LEDs off on every
        // connected Wiimote, etc.) before the process exits, then wait
        // for it to actually exit.
        let _ = self.commands_tx.send(UiCommand::Quit);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

impl Daemon {
    pub fn start() -> anyhow::Result<Self> {
        let (events_tx, events_rx) = unbounded();
        let (commands_tx, commands_rx) = unbounded();

        let handle = thread::Builder::new()
            .name("wiimote-daemon".into())
            .spawn(move || {
                if let Err(e) = run(events_tx.clone(), commands_rx) {
                    let _ = events_tx.send(log_event(
                        LogLevel::Error,
                        format!("daemon stopped: {e}"),
                    ));
                }
            })?;

        Ok(Self {
            events_rx,
            commands_tx,
            thread: Some(handle),
        })
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
        while let Ok(cmd) = commands_rx.try_recv() {
            if matches!(cmd, UiCommand::Quit) {
                info!("daemon quitting");
                shutdown(&mut ctx, &mut hid);
                return Ok(());
            }
            handle_command(cmd, &mut ctx, &mut hid, &events_tx);
        }

        // 2a) Bluetooth scanner events ------------------------------------
        while let Ok(ev) = scanner_rx.try_recv() {
            handle_scanner_event(ev, &mut ctx, &events_tx);
        }

        // 2b) Periodic HID enumerate + auto-(re)connect -------------------
        if ctx.force_rescan || last_scan.elapsed() >= SCAN_INTERVAL {
            last_scan = Instant::now();
            tick_periodic_scan(&mut ctx, &mut hid, &events_tx);
        }

        // 3) Transport events ---------------------------------------------
        while let Ok(ev) = transport_rx.try_recv() {
            handle_transport_event(ev, &mut ctx, &mut hid, &events_tx);
        }

        // 4) Periodic ticks -----------------------------------------------
        let now = Instant::now();
        tick_keepalive(now, &mut ctx, &mut hid);
        tick_pair_stuck(now, &mut ctx, &events_tx);
        tick_rumble_off(now, &mut ctx, &mut hid);
        tick_manual_scan_window(now, &mut ctx, &events_tx);

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
                    .zip(ctx.registry.values().map(clone_runtime_view))
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

// =====================================================================
// Command handlers
// =====================================================================

fn handle_command(
    cmd: UiCommand,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    match cmd {
        UiCommand::Quit => unreachable!("handled in main loop"),
        UiCommand::Connect(id) => handle_connect(id, ctx, hid, events_tx),
        UiCommand::Disconnect(id) => handle_disconnect(id, ctx, hid, events_tx),
        UiCommand::Forget(id) => handle_forget(id, ctx, hid, events_tx),
        UiCommand::Identify(id) => handle_identify(id, ctx, hid, events_tx),
        UiCommand::StartScan => handle_start_scan(ctx, events_tx),
        UiCommand::SetMappingProfile { id, profile } => {
            handle_set_profile(id, profile, ctx, events_tx)
        }
    }
}

fn handle_set_profile(
    id: String,
    profile: MappingProfile,
    ctx: &mut DaemonCtx,
    events_tx: &Sender<UiEvent>,
) {
    let Some(r) = ctx.registry.get_mut(&id) else {
        return;
    };
    if r.snapshot.mapping_profile == profile {
        return;
    }
    r.snapshot.mapping_profile = profile;
    // Drop the existing output target so the next promote_to_connected
    // (or the inline rebuild below) creates one with the new mapping.
    let was_connected = r.snapshot.connected;
    r.output = None;
    if was_connected {
        match output_for_profile(profile) {
            Ok(out) => {
                if let Some(r) = ctx.registry.get_mut(&id) {
                    r.output = Some(out);
                }
            }
            Err(e) => {
                debug!("output rebuild failed: {e}");
                if let Some(r) = ctx.registry.get_mut(&id) {
                    r.snapshot.last_error =
                        Some("Could not rebuild virtual gamepad with new profile".into());
                }
            }
        }
    }
    let _ = events_tx.send(log_event(
        LogLevel::Info,
        format!("mapping profile for {} → {}", short_id(&id), profile.label()),
    ));
    ctx.persist_dirty = true;
    ctx.dirty = true;
}

fn handle_connect(
    id: String,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    if let Some(r) = ctx.registry.get_mut(&id) {
        r.snapshot.user_disabled = false;
        r.next_retry = None;
    }
    let _ = events_tx.send(log_event(
        LogLevel::Info,
        format!("connect requested: {}", short_id(&id)),
    ));
    if !try_connect(&id, ctx, hid, events_tx) {
        if let Some(r) = ctx.registry.get_mut(&id) {
            r.next_retry = Some(Instant::now() + RETRY_INTERVAL);
        }
        let _ = events_tx.send(log_event(
            LogLevel::Warn,
            format!(
                "{} not reachable via HID. Windows hasn't activated the HID profile \
                 for this device. Try: unpair from Bluetooth settings, then click \
                 'Scan for new devices' here and press 1+2 on the Wiimote.",
                short_id(&id)
            ),
        ));
    }
    ctx.dirty = true;
}

fn handle_disconnect(
    id: String,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    if let Some(r) = ctx.registry.get(&id) {
        // Turn the player LEDs off before tearing down.
        let p = r.snapshot.path.clone();
        let _ = hid.send(
            &DeviceId(p.clone()),
            &OutputReport::SetLeds {
                leds: 0,
                rumble: false,
            }
            .encode(),
        );
        let _ = hid.close(&DeviceId(p));
    }
    if let Some(r) = ctx.registry.get_mut(&id) {
        r.reset_session();
        r.snapshot.user_disabled = true;
    }
    let _ = events_tx.send(log_event(
        LogLevel::Info,
        format!(
            "disconnected: {} (auto-retry disabled until you click Connect)",
            short_id(&id)
        ),
    ));
    ctx.dirty = true;
}

fn handle_forget(
    id: String,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    if let Some(r) = ctx.registry.get(&id) {
        let p = r.snapshot.path.clone();
        let _ = hid.send(
            &DeviceId(p.clone()),
            &OutputReport::SetLeds {
                leds: 0,
                rumble: false,
            }
            .encode(),
        );
        let _ = hid.close(&DeviceId(p));
    }
    let removed = ctx.registry.remove(&id);

    // If the canonical id is a MAC, ask the OS to drop the pairing.
    // Without this the BT scan re-discovers the still-paired device on
    // the next cycle and adds it back (B6).
    if let Some(addr) = mac_to_u64(&id) {
        match unpair_addr(addr) {
            Ok(()) => {
                let _ = events_tx.send(log_event(
                    LogLevel::Info,
                    format!("unpaired {} from OS", short_id(&id)),
                ));
            }
            Err(e) => {
                let _ = events_tx.send(log_event(
                    LogLevel::Warn,
                    format!("OS unpair failed: {e}"),
                ));
            }
        }
    }

    if let Some(r) = removed {
        let _ = events_tx.send(log_event(
            LogLevel::Info,
            format!("forgot: {} ({})", r.snapshot.name, short_id(&id)),
        ));
    }
    ctx.persist_dirty = true;
    ctx.dirty = true;
}

fn handle_identify(
    id: String,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    let Some(r) = ctx.registry.get_mut(&id) else {
        return;
    };
    let Some(slot) = r.slot else {
        return;
    };
    let leds = 1u8 << slot;
    let path = r.snapshot.path.clone();
    let _ = hid.send(
        &DeviceId(path),
        &OutputReport::SetLeds {
            leds,
            rumble: true,
        }
        .encode(),
    );
    r.rumble_off_at = Some(Instant::now() + Duration::from_millis(IDENTIFY_RUMBLE_MS));
    let _ = events_tx.send(log_event(
        LogLevel::Info,
        format!("identify: {}", short_id(&id)),
    ));
}

fn handle_start_scan(ctx: &mut DaemonCtx, events_tx: &Sender<UiEvent>) {
    let until = Instant::now() + MANUAL_SCAN_DURATION;
    ctx.manual_scan_until = Some(until);
    ctx.force_rescan = true;
    let _ = events_tx.send(UiEvent::ScanState {
        active_until: Some(until),
    });
    let _ = events_tx.send(log_event(
        LogLevel::Info,
        format!(
            "scanning for new devices for {} s…",
            MANUAL_SCAN_DURATION.as_secs()
        ),
    ));
}

// =====================================================================
// Scanner-event handling
// =====================================================================

fn handle_scanner_event(ev: ScannerEvent, ctx: &mut DaemonCtx, events_tx: &Sender<UiEvent>) {
    match ev {
        ScannerEvent::Discovered {
            addr,
            name,
            paired,
            connected,
        } => {
            let _ = events_tx.send(log_event(
                LogLevel::Info,
                format!(
                    "[BT] discovered {name} {} (paired={paired}, connected={connected})",
                    format_addr(addr)
                ),
            ));
        }
        ScannerEvent::Pairing { addr } => {
            ctx.pair_started.insert(addr, Instant::now());
            ctx.pair_stuck_signaled.remove(&addr);
            let _ = events_tx.send(log_event(
                LogLevel::Info,
                format!("[BT] pairing {}…", format_addr(addr)),
            ));
        }
        ScannerEvent::Paired { addr } => {
            ctx.pair_started.remove(&addr);
            ctx.pair_stuck_signaled.remove(&addr);
            let _ = events_tx.send(log_event(
                LogLevel::Info,
                format!("[BT] paired {}", format_addr(addr)),
            ));
        }
        ScannerEvent::PairFailed { addr, reason } => {
            ctx.pair_started.remove(&addr);
            ctx.pair_stuck_signaled.remove(&addr);
            let _ = events_tx.send(log_event(
                LogLevel::Warn,
                format!("[BT] pair failed {}: {reason}", format_addr(addr)),
            ));
        }
        ScannerEvent::HidEnabled { addr } => {
            let _ = events_tx.send(log_event(
                LogLevel::Info,
                format!("[BT] HID service enabled on {}", format_addr(addr)),
            ));
            ctx.force_rescan = true;
        }
        ScannerEvent::SdpCacheStale { addr } => {
            handle_bt_state_stuck(addr, StuckReason::SdpCache, ctx, events_tx)
        }
        ScannerEvent::AuthStuck { addr } => {
            handle_bt_state_stuck(addr, StuckReason::Auth, ctx, events_tx)
        }
        ScannerEvent::Error(e) => {
            let _ = events_tx.send(log_event(LogLevel::Warn, format!("[BT] {e}")));
        }
    }
}

#[derive(Clone, Copy)]
enum StuckReason {
    /// Wii Remote Plus / Windows: HID service entry is stale, every
    /// `BluetoothSetServiceState` returns `ERROR_INVALID_PARAMETER`.
    SdpCache,
    /// Windows: `BluetoothAuthenticateDeviceEx` returns
    /// `ERROR_GEN_FAILURE` because the registry holds a half-paired
    /// `connected=true,paired=false` entry the stack refuses to re-auth.
    Auth,
}

impl StuckReason {
    fn passive_message(self, pretty: &str) -> String {
        match self {
            StuckReason::SdpCache => format!(
                "[BT] {pretty}: HID service not advertised (stale SDP cache). \
                 Click 'Scan for new devices' and press 1+2 to auto-recover."
            ),
            StuckReason::Auth => format!(
                "[BT] {pretty}: stuck auth state (BT registry holds a half-paired \
                 entry). Click 'Scan for new devices' and press 1+2 to auto-recover."
            ),
        }
    }

    fn detected_message(self, pretty: &str) -> String {
        match self {
            StuckReason::SdpCache => format!(
                "[BT] {pretty}: stale SDP cache detected — auto-recovering \
                 (unpairing now; keep holding 1+2 on the Wiimote)…"
            ),
            StuckReason::Auth => format!(
                "[BT] {pretty}: stuck auth state detected (ERROR_GEN_FAILURE) — \
                 auto-recovering (unpairing now; keep holding 1+2 on the Wiimote)…"
            ),
        }
    }
}

/// Auto-recover from a stuck Bluetooth-registry state: depair the
/// device, force a rescan so the next inquiry sees it as fresh, and
/// the rest of the auto-pair flow handles it from there. Gated on a
/// manual scan window being active — outside of that the user isn't
/// expected to be holding 1+2, and depairing a "good but offline"
/// device would just leave them confused.
fn handle_bt_state_stuck(
    addr: u64,
    reason: StuckReason,
    ctx: &mut DaemonCtx,
    events_tx: &Sender<UiEvent>,
) {
    let pretty = format_addr(addr);
    if ctx.manual_scan_until.is_none() {
        let _ = events_tx.send(log_event(LogLevel::Warn, reason.passive_message(&pretty)));
        return;
    }
    // Don't fire repeatedly for the same device within one scan window.
    if !ctx.sdp_recovery_attempted.insert(addr) {
        return;
    }
    let _ = events_tx.send(log_event(
        LogLevel::Info,
        reason.detected_message(&pretty),
    ));
    match unpair_addr(addr) {
        Ok(()) => {
            let _ = events_tx.send(log_event(
                LogLevel::Info,
                format!(
                    "[BT] {pretty}: unpaired. The next inquiry will re-pair \
                     it from scratch — keep holding 1+2."
                ),
            ));
            ctx.force_rescan = true;
            // Drop any existing snapshot that might point at the now-
            // dead pairing — the next inquiry inserts a fresh one.
            if ctx.registry.remove(&pretty).is_some() {
                ctx.persist_dirty = true;
                ctx.dirty = true;
            }
        }
        Err(e) => {
            let _ = events_tx.send(log_event(
                LogLevel::Warn,
                format!(
                    "[BT] {pretty}: auto-recovery failed: {e}. \
                     Remove the device manually from your OS BT settings, \
                     then click 'Scan for new devices'."
                ),
            ));
        }
    }
}

// =====================================================================
// Periodic HID scan + (re)connect
// =====================================================================

fn tick_periodic_scan(ctx: &mut DaemonCtx, hid: &mut HidTransport, events_tx: &Sender<UiEvent>) {
    let was_forced = ctx.force_rescan;
    ctx.force_rescan = false;

    // `hid.enumerate()` walks SetupAPI on Windows and stalls open HID
    // I/O for ~150-300 ms — only call it when nothing is connected,
    // when forced, or during a manual scan window.
    let allow_enum = was_forced
        || !ctx.registry.any_connected()
        || ctx.manual_scan_until.is_some();
    if allow_enum {
        match hid.enumerate() {
            Ok(found) => merge_enumerated(found, ctx, events_tx),
            Err(e) => warn!("scan failed: {e}"),
        }
    }

    let now = Instant::now();
    let candidates: Vec<String> = ctx
        .registry
        .iter()
        .filter(|(_, r)| {
            !r.snapshot.connected
                && !r.snapshot.user_disabled
                && !r.pending
                && r.next_retry.is_none_or(|t| t <= now)
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in candidates {
        if try_connect(&id, ctx, hid, events_tx) {
            if let Some(r) = ctx.registry.get_mut(&id) {
                r.next_retry = None;
            }
            ctx.dirty = true;
        } else if let Some(r) = ctx.registry.get_mut(&id) {
            r.next_retry = Some(now + RETRY_INTERVAL);
        }
    }
}

fn merge_enumerated(found: Vec<DeviceInfo>, ctx: &mut DaemonCtx, events_tx: &Sender<UiEvent>) {
    for f in &found {
        // Canonical key: prefer the BT MAC (stable across reconnects);
        // fall back to the HID path when no serial number is exposed.
        let canonical_id = f.mac.clone().unwrap_or_else(|| f.id.0.clone());

        // Migrate legacy entries that were keyed on the HID path before
        // hidapi started returning a stable serial / MAC for the same
        // device. Without this we'd insert a fresh MAC-keyed entry and
        // leave the path-keyed one floating around as a duplicate.
        if let Some(mac) = &f.mac {
            if mac != &f.id.0 && ctx.registry.get(&f.id.0).is_some() {
                if ctx.registry.rekey(&f.id.0, mac) {
                    let _ = events_tx.send(log_event(
                        LogLevel::Info,
                        format!(
                            "[HID] migrated legacy entry to MAC: {} → {}",
                            short_id(&f.id.0),
                            short_id(mac)
                        ),
                    ));
                    ctx.persist_dirty = true;
                    ctx.dirty = true;
                }
            }
        }

        match ctx.registry.get_mut(&canonical_id) {
            Some(existing) => {
                if existing.snapshot.path != f.id.0 {
                    let _ = events_tx.send(log_event(
                        LogLevel::Info,
                        format!(
                            "[HID] {}: path renumbered, re-binding ({} → {})",
                            short_id(&canonical_id),
                            short_id(&existing.snapshot.path),
                            short_id(&f.id.0),
                        ),
                    ));
                    existing.snapshot.path = f.id.0.clone();
                    ctx.persist_dirty = true;
                    ctx.dirty = true;
                }
            }
            None => {
                let _ = events_tx.send(log_event(
                    LogLevel::Info,
                    format!(
                        "[HID] new device enumerated: {} ({})",
                        f.name,
                        short_id(&canonical_id)
                    ),
                ));
                let snap = DeviceSnapshot::new(
                    canonical_id.clone(),
                    f.name.clone(),
                    f.id.0.clone(),
                );
                ctx.registry.insert(DeviceRuntime::new(snap));
                ctx.persist_dirty = true;
                ctx.dirty = true;
            }
        }
    }
}

// =====================================================================
// Connection lifecycle
// =====================================================================

/// Open the HID device and set initial reporting. Returns `true` if the
/// open succeeded — the device is only marked **`pending`** here;
/// promotion to `connected = true` happens when the first input report
/// actually arrives, and *that* is when we plug a virtual controller.
///
/// Plugging ViGEm here would be wrong: on Windows `hid.open()` succeeds
/// even for paired-but-offline Wiimotes, so we'd repeatedly plug-and-
/// unplug a virtual Xbox 360 pad every retry cycle.
fn try_connect(
    id: &str,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) -> bool {
    let Some(r) = ctx.registry.get(id) else {
        return false;
    };
    let info = DeviceInfo {
        id: DeviceId(r.snapshot.path.clone()),
        name: r.snapshot.name.clone(),
        vendor_id: VID_NINTENDO,
        product_id: PID_WIIMOTE,
        mac: Some(r.snapshot.id.clone()),
    };
    match hid.open(&info) {
        Ok(()) => {
            let _ = hid.send(
                &info.id,
                &OutputReport::SetLeds {
                    leds: 0b0001,
                    rumble: false,
                }
                .encode(),
            );
            // 0x31 = buttons + 3-axis accel, continuous so the watchdog
            // sees a steady stream of reports.
            let _ = hid.send(
                &info.id,
                &OutputReport::SetReportingMode {
                    continuous: true,
                    mode: 0x31,
                }
                .encode(),
            );
            let _ = hid.send(&info.id, &OutputReport::RequestStatus.encode());
            if let Some(r) = ctx.registry.get_mut(id) {
                r.pending = true;
            }
            let _ = events_tx.send(log_event(
                LogLevel::Info,
                format!(
                    "[HID] {}: handle opened, mode 0x31 set, waiting for first report",
                    short_id(id)
                ),
            ));
            true
        }
        Err(e) => {
            // Demote to debug-level on the tracing side: with
            // auto-retry on, this fires every few seconds when the
            // Wiimote is just off. The UI gets a single visible line
            // only on user-initiated Connect (handle_connect).
            //
            // We deliberately *don't* surface this into the per-row
            // last_error: a paired-but-offline Wiimote is the normal
            // resting state, the open-circle dot already conveys it,
            // and a red "Impossibile trovare il file specificato" on
            // every offline row is just noise. last_error stays
            // reserved for genuine misconfigurations (no virtual pad
            // available, slot cap reached, …).
            debug!("open failed: {e}");
            false
        }
    }
}

/// First real input report confirms a paired-and-online device — flip
/// it to `connected`, plug the ViGEm pad, surface logs.
fn promote_to_connected(
    id: &str,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    let Some(slot) = ctx.registry.lowest_free_slot() else {
        // 5th Wiimote refused (B4) — XInput supports at most 4.
        let user_msg = "4 Wiimotes already connected — XInput supports at most 4. \
                        Disconnect one before connecting another.";
        if let Some(r) = ctx.registry.get_mut(id) {
            r.snapshot.last_error = Some(user_msg.into());
        }
        let _ = events_tx.send(log_event(LogLevel::Warn, user_msg));
        if let Some(r) = ctx.registry.get(id) {
            let _ = hid.close(&DeviceId(r.snapshot.path.clone()));
        }
        return;
    };

    let path = ctx.registry.get(id).map(|r| r.snapshot.path.clone());
    if let Some(p) = path {
        let _ = hid.send(
            &DeviceId(p),
            &OutputReport::SetLeds {
                leds: 1 << slot,
                rumble: false,
            }
            .encode(),
        );
    }

    if let Some(r) = ctx.registry.get_mut(id) {
        r.slot = Some(slot);
        r.snapshot.connected = true;
        r.snapshot.last_error = None;
        let _ = events_tx.send(log_event(
            LogLevel::Info,
            format!(
                "connected: {} as Player {} ({})",
                r.snapshot.name,
                slot + 1,
                short_id(id)
            ),
        ));
    }
    let profile = ctx
        .registry
        .get(id)
        .map(|r| r.snapshot.mapping_profile)
        .unwrap_or_default();
    match output_for_profile(profile) {
        Ok(out) => {
            if let Some(r) = ctx.registry.get_mut(id) {
                r.output = Some(out);
            }
            let _ = events_tx.send(log_event(
                LogLevel::Info,
                format!("virtual gamepad ready for {} ({})", short_id(id), profile.label()),
            ));
        }
        Err(e) => {
            // Technical detail (Win32 codes etc.) only goes to the
            // debug log; the UI sees a clean, actionable message.
            debug!("output init failed: {e}");
            let user_msg = match cfg!(target_os = "windows") {
                true => "Virtual controller output unavailable — install or restart ViGEmBus",
                false => "Virtual controller output not implemented on this platform",
            };
            if let Some(r) = ctx.registry.get_mut(id) {
                r.snapshot.last_error = Some(user_msg.into());
            }
            let _ = events_tx.send(log_event(LogLevel::Warn, user_msg));
        }
    }
}

// =====================================================================
// Transport-event handling
// =====================================================================

fn handle_transport_event(
    ev: TransportEvent,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    match ev {
        TransportEvent::Report { id, report } => handle_report(id, report, ctx, hid, events_tx),
        TransportEvent::DeviceLost(path_id) => handle_device_lost(path_id, ctx, hid, events_tx),
        TransportEvent::DeviceFound(_) => {}
        TransportEvent::Error { id, error } => {
            debug!(?id, "transport error: {error}");
        }
    }
}

fn handle_report(
    path_id: DeviceId,
    report: InputReport,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    let id = match ctx.registry.id_for_path(&path_id.0) {
        Some(c) => c,
        None => {
            // Path renumbered between our last enumerate() and the BT
            // link staying alive (B7) — force a rescan so the path is
            // re-bound to its canonical MAC.
            ctx.force_rescan = true;
            return;
        }
    };

    // First report after a tentative open promotes the device to
    // connected and plugs the virtual pad.
    let was_pending = ctx.registry.get(&id).map(|r| r.pending).unwrap_or(false);
    if was_pending {
        let _ = events_tx.send(log_event(
            LogLevel::Info,
            format!(
                "[HID] {}: first input report received — promoting to connected",
                short_id(&id)
            ),
        ));
        if let Some(r) = ctx.registry.get_mut(&id) {
            r.pending = false;
        }
        promote_to_connected(&id, ctx, hid, events_tx);
        ctx.dirty = true;
    }

    log_report_gap(&id, ctx, events_tx);

    process_extension_fsm(&id, &report, &path_id, ctx, hid, events_tx);

    let (buttons, accel, ir, battery) = decompose(&report);
    if let Some(r) = ctx.registry.get_mut(&id) {
        if let Some(b) = buttons {
            r.snapshot.last_buttons = b;
            r.controller.buttons = b;
            ctx.dirty = true;
        }
        if let Some(a) = accel {
            r.snapshot.last_accel = a;
            r.controller.accel = a;
            ctx.dirty = true;
        }
        if let Some(i) = ir {
            r.snapshot.last_ir = i;
            r.controller.ir = i;
        }
        if let Some(bat) = battery {
            r.snapshot.battery = Some(bat);
            ctx.dirty = true;
        }
        let st = r.controller;
        if let Some(out) = r.output.as_mut() {
            if let Err(e) = out.update(&st) {
                debug!("output update failed: {e}");
            }
        }
    }
}

fn log_report_gap(id: &str, ctx: &mut DaemonCtx, events_tx: &Sender<UiEvent>) {
    let now_t = Instant::now();
    let Some(r) = ctx.registry.get_mut(id) else {
        return;
    };
    if let Some(prev) = r.last_report {
        let gap_ms = now_t.duration_since(prev).as_millis();
        if gap_ms > REPORT_GAP_WARN_MS {
            let due = r.last_gap_log.is_none_or(|t| {
                now_t.duration_since(t) >= GAP_LOG_BACKOFF
            });
            if due {
                r.last_gap_log = Some(now_t);
                let _ = events_tx.send(log_event(
                    LogLevel::Warn,
                    format!("report gap: {gap_ms} ms"),
                ));
            }
        }
    }
    r.last_report = Some(now_t);
}

fn process_extension_fsm(
    id: &str,
    report: &InputReport,
    path_id: &DeviceId,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    match report {
        InputReport::Status { flags, .. } => {
            if flags.extension_connected {
                let already = matches!(
                    ctx.registry.get(id).and_then(|r| r.ext_phase),
                    Some(ExtensionPhase::Identified(_))
                        | Some(ExtensionPhase::InitSent)
                        | Some(ExtensionPhase::ReadingId)
                );
                if !already {
                    let _ = events_tx.send(log_event(
                        LogLevel::Info,
                        format!(
                            "[EXT] {}: extension plugged in, sending init handshake (0x55→0xa400f0)",
                            short_id(id)
                        ),
                    ));
                    let _ = hid.send(
                        path_id,
                        &OutputReport::WriteRegister {
                            address: 0x00a4_00f0,
                            data: vec![0x55],
                        }
                        .encode(),
                    );
                    if let Some(r) = ctx.registry.get_mut(id) {
                        r.ext_phase = Some(ExtensionPhase::InitSent);
                    }
                }
            } else {
                let was_present = matches!(
                    ctx.registry.get(id).and_then(|r| r.ext_phase),
                    Some(ExtensionPhase::Identified(_))
                );
                if let Some(r) = ctx.registry.get_mut(id) {
                    r.ext_phase = None;
                    if r.snapshot.extension.is_some() || r.snapshot.ext_data.is_some() {
                        r.snapshot.extension = None;
                        r.snapshot.ext_data = None;
                        ctx.dirty = true;
                        ctx.persist_dirty = true;
                    }
                    r.controller.ext = None;
                }
                if was_present {
                    let _ = events_tx.send(log_event(
                        LogLevel::Info,
                        format!(
                            "[EXT] {}: extension unplugged, reverting to mode 0x31",
                            short_id(id)
                        ),
                    ));
                    // Drop back to the no-extension reporting mode so
                    // we stop receiving 16 bytes of junk extension
                    // payload per frame.
                    let _ = hid.send(
                        path_id,
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
            let phase = ctx.registry.get(id).and_then(|r| r.ext_phase);
            if *report_id == 0x16 && *error == 0 && phase == Some(ExtensionPhase::InitSent) {
                let _ = events_tx.send(log_event(
                    LogLevel::Info,
                    format!(
                        "[EXT] {}: init acked, reading 6-byte extension id from 0xa400fa",
                        short_id(id)
                    ),
                ));
                let _ = hid.send(
                    path_id,
                    &OutputReport::ReadRegister {
                        address: 0x00a4_00fa,
                        count: 6,
                    }
                    .encode(),
                );
                if let Some(r) = ctx.registry.get_mut(id) {
                    r.ext_phase = Some(ExtensionPhase::ReadingId);
                }
            }
        }
        InputReport::ReadResponse {
            error,
            size,
            address,
            data,
            ..
        } => {
            let phase = ctx.registry.get(id).and_then(|r| r.ext_phase);
            if *error == 0
                && *address == 0x00fa
                && *size == 6
                && phase == Some(ExtensionPhase::ReadingId)
            {
                let mut id_bytes = [0u8; 6];
                id_bytes.copy_from_slice(&data[..6]);
                let ext = ExtensionType::from_id(&id_bytes);
                if let Some(r) = ctx.registry.get_mut(id) {
                    r.ext_phase = Some(ExtensionPhase::Identified(ext));
                    r.whammy_baseline = None;
                    r.snapshot.extension = Some(ext);
                    ctx.dirty = true;
                    ctx.persist_dirty = true;
                }
                let id_hex = id_bytes
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let _ = events_tx.send(log_event(
                    LogLevel::Info,
                    format!(
                        "[EXT] {}: identified as {} (id: {})",
                        short_id(id),
                        ext.label(),
                        id_hex
                    ),
                ));
                let _ = events_tx.send(log_event(
                    LogLevel::Info,
                    format!(
                        "[EXT] {}: switching to mode 0x35 (buttons + accel + 16B ext payload)",
                        short_id(id)
                    ),
                ));
                // Switch to mode 0x35 so the Wiimote also streams the
                // 16-byte extension payload alongside buttons + accel.
                let _ = hid.send(
                    path_id,
                    &OutputReport::SetReportingMode {
                        continuous: true,
                        mode: 0x35,
                    }
                    .encode(),
                );
            }
        }
        InputReport::ButtonsAccelExt { ext, .. } => {
            if let Some(r) = ctx.registry.get_mut(id) {
                if let Some(ExtensionPhase::Identified(et)) = r.ext_phase {
                    let mut parsed = ExtensionData::parse(et, ext);
                    if let ExtensionData::Guitar(g) = &mut parsed {
                        let baseline = r.whammy_baseline.get_or_insert(g.whammy);
                        if g.whammy < *baseline {
                            *baseline = g.whammy;
                        }
                        let span = 31u32.saturating_sub(*baseline as u32).max(1);
                        let above = g.whammy.saturating_sub(*baseline);
                        g.whammy = ((above as u32 * 31) / span) as u8;
                    }
                    r.snapshot.ext_data = Some(parsed);
                    r.controller.ext = Some(parsed);
                    ctx.dirty = true;
                }
            }
        }
        _ => {}
    }
}

fn handle_device_lost(
    path_id: DeviceId,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    let id = match ctx.registry.id_for_path(&path_id.0) {
        Some(c) => c,
        None => {
            // Unknown path; just clean up the transport handle.
            let _ = hid.close(&path_id);
            return;
        }
    };
    let was_connected = ctx
        .registry
        .get(&id)
        .map(|r| r.snapshot.connected)
        .unwrap_or(false);
    if was_connected {
        let _ = events_tx.send(log_event(
            LogLevel::Info,
            format!("device offline: {}", short_id(&id)),
        ));
    }
    if let Some(r) = ctx.registry.get_mut(&id) {
        r.reset_session();
        r.next_retry = Some(
            Instant::now()
                + if was_connected {
                    QUICK_RETRY_AFTER_LOSS
                } else {
                    RETRY_INTERVAL
                },
        );
    }
    let _ = hid.close(&path_id);
    ctx.dirty = true;
}

// =====================================================================
// Periodic ticks
// =====================================================================

fn tick_keepalive(now: Instant, ctx: &mut DaemonCtx, hid: &mut HidTransport) {
    let due: Vec<(String, String)> = ctx
        .registry
        .iter()
        .filter(|(_, r)| {
            r.snapshot.connected
                && r.last_keepalive
                    .is_none_or(|t| now.duration_since(t) >= KEEPALIVE_INTERVAL)
        })
        .map(|(id, r)| (id.clone(), r.snapshot.path.clone()))
        .collect();
    for (id, path) in due {
        let _ = hid.send(&DeviceId(path), &OutputReport::RequestStatus.encode());
        if let Some(r) = ctx.registry.get_mut(&id) {
            r.last_keepalive = Some(now);
        }
    }
}

fn tick_pair_stuck(now: Instant, ctx: &mut DaemonCtx, events_tx: &Sender<UiEvent>) {
    let stuck: Vec<u64> = ctx
        .pair_started
        .iter()
        .filter(|(addr, t)| {
            now.duration_since(**t) >= PAIR_STUCK_THRESHOLD
                && !ctx.pair_stuck_signaled.contains(addr)
        })
        .map(|(a, _)| *a)
        .collect();
    for addr in stuck {
        ctx.pair_stuck_signaled.insert(addr);
        let _ = events_tx.send(UiEvent::PairingStuck { addr });
        let _ = events_tx.send(log_event(
            LogLevel::Warn,
            format!(
                "[BT] pairing stuck on {} — see recovery dialog",
                format_addr(addr)
            ),
        ));
    }
}

fn tick_rumble_off(now: Instant, ctx: &mut DaemonCtx, hid: &mut HidTransport) {
    let due: Vec<(String, String, u8)> = ctx
        .registry
        .iter()
        .filter_map(|(id, r)| match r.rumble_off_at {
            Some(t) if t <= now => Some((
                id.clone(),
                r.snapshot.path.clone(),
                r.slot.map(|s| 1u8 << s).unwrap_or(0),
            )),
            _ => None,
        })
        .collect();
    for (id, path, leds) in due {
        let _ = hid.send(
            &DeviceId(path),
            &OutputReport::SetLeds {
                leds,
                rumble: false,
            }
            .encode(),
        );
        if let Some(r) = ctx.registry.get_mut(&id) {
            r.rumble_off_at = None;
        }
    }
}

fn tick_manual_scan_window(now: Instant, ctx: &mut DaemonCtx, events_tx: &Sender<UiEvent>) {
    let Some(t) = ctx.manual_scan_until else {
        return;
    };
    if now >= t {
        ctx.manual_scan_until = None;
        ctx.force_rescan = true;
        // Reset the per-window auto-recovery memo so the user gets a
        // fresh attempt the next time they click Scan.
        ctx.sdp_recovery_attempted.clear();
        let _ = events_tx.send(UiEvent::ScanState { active_until: None });
        let _ = events_tx.send(log_event(LogLevel::Info, "scan window ended"));
    }
}

// =====================================================================
// Helpers
// =====================================================================

/// Decompose an input report into the four optional fields the UI
/// snapshot tracks.
fn decompose(
    r: &InputReport,
) -> (
    Option<Buttons>,
    Option<wiimote_core::Accelerometer>,
    Option<wiimote_core::IrDots>,
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

/// Helper used purely to feed `persist::save` from the registry — the
/// persist layer expects a `HashMap<String, DeviceRuntime>`-shaped view
/// but only reads the snapshot fields, so we hand it cheap clones.
fn clone_runtime_view(r: &DeviceRuntime) -> DeviceRuntime {
    let mut copy = DeviceRuntime::new(r.snapshot.clone());
    copy.snapshot.extension = r.snapshot.extension;
    copy
}
