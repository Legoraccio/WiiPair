//! Bluetooth scanner-event handling and BT-state stuck recovery.

use std::time::Instant;

use crossbeam_channel::Sender;
use wiimote_transport::platform::{ScannerEvent, unpair_addr};

use crate::helpers::format_addr;
use crate::{DaemonCtx, LogLevel, UiEvent, log_event};

pub(crate) fn handle_scanner_event(
    ev: ScannerEvent,
    ctx: &mut DaemonCtx,
    events_tx: &Sender<UiEvent>,
) {
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
pub(crate) enum StuckReason {
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
