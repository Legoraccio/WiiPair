//! Bluetooth scanner-event handling and BT-state stuck recovery.

use std::time::Instant;

use crossbeam_channel::Sender;
use wiimote_transport::platform::{ScannerEvent, unpair_addr};

use crate::helpers::format_addr;
use crate::user_msg::UserFacingError;
use crate::{DaemonCtx, LogLevel, UiEvent, clear_persistent_warning, emit_user_error, log_event};

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
            // A successful pair means whatever pair-fail / auto-
            // recovery banner the user is staring at no longer
            // describes reality. Clear them so the UI catches up.
            clear_persistent_warning(events_tx, "pair_timeout");
            clear_persistent_warning(events_tx, "pair_auth");
            clear_persistent_warning(events_tx, "pair_other");
            clear_persistent_warning(events_tx, "auto_recovery");
            let _ = events_tx.send(log_event(
                LogLevel::Info,
                format!("[BT] paired {}", format_addr(addr)),
            ));
        }
        ScannerEvent::PairFailed { addr, reason } => {
            ctx.pair_started.remove(&addr);
            ctx.pair_stuck_signaled.remove(&addr);
            let pretty = format_addr(addr);

            // Some Win32 failure codes are the canonical signature of
            // states WiiPair already auto-recovers via dedicated
            // events. Promoting them to a user-facing banner is
            // misleading — the recovery flow will either succeed (and
            // we'd have shown a panic banner for nothing) or fail in
            // its own dedicated path. Keep the raw line in the log
            // for diagnostics and bail out before the banner.
            //
            // Currently filtered:
            //   * AuthenticateDeviceEx 0x1F (ERROR_GEN_FAILURE) →
            //     ScannerEvent::AuthStuck handles depair+rescan.
            //   * SetServiceState 0x57 (ERROR_INVALID_PARAMETER) →
            //     ScannerEvent::SdpCacheStale handles depair+rescan.
            let lc = reason.to_lowercase();
            let auto_recovered = lc.contains("authenticatedeviceex")
                && (lc.contains("0x0000001f") || lc.contains("error_gen_failure"));
            if auto_recovered {
                let _ = events_tx.send(log_event(
                    LogLevel::Warn,
                    format!("[BT] pair failed {pretty}: {reason}"),
                ));
                return;
            }

            // Classify the OS-level failure into a user-facing
            // category. Whatever the heuristic misses falls through
            // to the generic variant — the raw `reason` still goes
            // into the log line below.
            let kind = if reason.contains("Page Timeout") || lc.contains("timeout") {
                UserFacingError::PairFailedTimeout {
                    addr_pretty: pretty.clone(),
                }
            } else if reason.contains("Authentication") {
                UserFacingError::PairFailedAuth {
                    addr_pretty: pretty.clone(),
                }
            } else {
                UserFacingError::PairFailedOther {
                    addr_pretty: pretty.clone(),
                }
            };
            emit_user_error(
                events_tx,
                kind,
                format!("[BT] pair failed {pretty}: {reason}"),
            );
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
            // Stays Warn-only on purpose: most ScannerEvent::Error
            // payloads are transient per-device hiccups (e.g. the
            // Wii Remote Plus `SetServiceState 0x57` SDP-cache stale
            // case, which already triggers `SdpCacheStale` and the
            // daemon's auto-recovery flow). Promoting these to a
            // banner would scare users about something WiiPair is
            // already handling.
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
            emit_user_error(
                events_tx,
                UserFacingError::AutoRecoveryFailed {
                    addr_pretty: pretty.clone(),
                },
                format!("[BT] {pretty}: auto-recovery failed: {e}"),
            );
        }
    }
}
