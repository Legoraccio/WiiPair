//! UI command handlers — one function per `UiCommand` variant, plus a
//! tiny dispatcher invoked from the daemon main loop.

use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use tracing::debug;
use wiimote_core::OutputReport;
use wiimote_output::{MappingProfile, output_for_profile};
use wiimote_transport::hid::HidTransport;
use wiimote_transport::platform::{mac_to_u64, unpair_addr};
use wiimote_transport::{DeviceId, Transport};

use crate::helpers::short_id;
use crate::hid_scan::try_connect;
use crate::user_msg::UserFacingError;
use crate::{
    DaemonCtx, IDENTIFY_RUMBLE_MS, LogLevel, MANUAL_SCAN_DURATION, OUTPUT_RETRY_INTERVAL,
    RETRY_INTERVAL, UiCommand, UiEvent, clear_persistent_warning, emit_user_error, log_event,
};

pub(crate) fn handle_command(
    cmd: UiCommand,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    match cmd {
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
    r.output_retry_at = None;
    if was_connected {
        match output_for_profile(profile) {
            Ok(out) => {
                if let Some(r) = ctx.registry.get_mut(&id) {
                    r.output = Some(out);
                    r.snapshot.last_error = None;
                }
                clear_persistent_warning(events_tx, "profile_rebuild");
            }
            Err(e) => {
                debug!("output rebuild failed: {e}");
                if let Some(r) = ctx.registry.get_mut(&id) {
                    r.snapshot.last_error =
                        Some(UserFacingError::ProfileRebuildFailed.message());
                    r.output_retry_at = Some(Instant::now() + OUTPUT_RETRY_INTERVAL);
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
    if try_connect(&id, ctx, hid, events_tx) {
        ctx.dirty = true;
        return;
    }

    // Direct HID open failed. Schedule a normal auto-retry…
    if let Some(r) = ctx.registry.get_mut(&id) {
        r.next_retry = Some(Instant::now() + RETRY_INTERVAL);
    }
    // …and *also* open a 30 s manual-scan window if one isn't already
    // active. The window has two effects that matter for an explicit
    // user-initiated `Connect`:
    //
    //   1. The Bluetooth scanner stops being paused (see the
    //      `pause_inquiry` flag in `lib.rs::run`) and starts running
    //      inquiries again. On Windows that triggers the
    //      `BluetoothSetServiceState(HID_SERVICE_GUID, ENABLE)` call
    //      that reactivates the HID profile for paired-but-offline
    //      Wiimotes.
    //   2. It opens the gate on `handle_bt_state_stuck`, so a Wii
    //      Remote Plus that surfaces a stale SDP cache mid-flow
    //      auto-depairs and the next inquiry re-pairs it from
    //      scratch (provided the user holds 1+2).
    //
    // Skipped if a scan window is already active — don't reset its
    // deadline mid-flight.
    let opened_scan = if ctx.manual_scan_until.is_none() {
        let until = Instant::now() + MANUAL_SCAN_DURATION;
        ctx.manual_scan_until = Some(until);
        ctx.force_rescan = true;
        let _ = events_tx.send(UiEvent::ScanState {
            active_until: Some(until),
        });
        true
    } else {
        false
    };

    let hint = if opened_scan {
        format!(
            "{} not reachable yet — opened a {}s scan window. Hold 1+2 on \
             the Wiimote so the OS can re-bind it; this also auto-recovers \
             Wii Remote Plus units that get stuck on a stale SDP cache.",
            short_id(&id),
            MANUAL_SCAN_DURATION.as_secs(),
        )
    } else {
        format!(
            "{} not reachable via HID — auto-retry will keep trying.",
            short_id(&id),
        )
    };
    let _ = events_tx.send(log_event(LogLevel::Warn, hint));
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
    // Disconnecting frees an XInput slot, so the "4 Wiimotes
    // already connected" banner — if it was up — no longer applies.
    clear_persistent_warning(events_tx, "slot_cap");
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
                clear_persistent_warning(events_tx, "os_unpair");
                let _ = events_tx.send(log_event(
                    LogLevel::Info,
                    format!("unpaired {} from OS", short_id(&id)),
                ));
            }
            Err(e) => {
                emit_user_error(
                    events_tx,
                    UserFacingError::OsUnpairFailed { id: id.clone() },
                    format!("OS unpair failed for {}: {e}", short_id(&id)),
                );
            }
        }
    }

    if let Some(r) = removed {
        // Forgetting a device frees its XInput slot if it had one.
        clear_persistent_warning(events_tx, "slot_cap");
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
