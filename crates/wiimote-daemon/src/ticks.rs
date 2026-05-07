//! Periodic ticks driven from the daemon main loop: keepalive,
//! pair-stuck watchdog, rumble pulse off, manual scan window expiry.

use std::time::Instant;

use crossbeam_channel::Sender;
use wiimote_core::OutputReport;
use wiimote_transport::hid::HidTransport;
use wiimote_transport::{DeviceId, Transport};

use crate::helpers::format_addr;
use crate::{
    DaemonCtx, KEEPALIVE_INTERVAL, LogLevel, PAIR_STUCK_THRESHOLD, UiEvent, log_event,
};

pub(crate) fn tick_keepalive(now: Instant, ctx: &mut DaemonCtx, hid: &mut HidTransport) {
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

pub(crate) fn tick_pair_stuck(now: Instant, ctx: &mut DaemonCtx, events_tx: &Sender<UiEvent>) {
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

pub(crate) fn tick_rumble_off(now: Instant, ctx: &mut DaemonCtx, hid: &mut HidTransport) {
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

pub(crate) fn tick_manual_scan_window(
    now: Instant,
    ctx: &mut DaemonCtx,
    events_tx: &Sender<UiEvent>,
) {
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
