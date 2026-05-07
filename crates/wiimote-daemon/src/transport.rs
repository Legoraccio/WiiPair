//! Transport-event dispatch: report → registry update → output backend,
//! plus device-lost cleanup.

use std::time::Instant;

use crossbeam_channel::Sender;
use tracing::debug;
use wiimote_core::InputReport;
use wiimote_transport::hid::HidTransport;
use wiimote_transport::{DeviceId, Transport, TransportEvent};

use crate::extension_fsm::process_extension_fsm;
use crate::helpers::{decompose, short_id};
use crate::hid_scan::promote_to_connected;
use crate::{
    DaemonCtx, GAP_LOG_BACKOFF, LogLevel, QUICK_RETRY_AFTER_LOSS, REPORT_GAP_WARN_MS,
    RETRY_INTERVAL, UiEvent, log_event,
};

pub(crate) fn handle_transport_event(
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
