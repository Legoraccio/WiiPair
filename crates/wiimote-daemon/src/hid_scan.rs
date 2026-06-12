//! Periodic HID enumeration, (re)connect attempts, output-target retry.

use std::time::Instant;

use crossbeam_channel::Sender;
use tracing::{debug, warn};
use wiimote_core::{OutputReport, PID_WIIMOTE, VID_NINTENDO};
use wiimote_output::{MappingProfile, output_for_profile};
use wiimote_transport::hid::HidTransport;
use wiimote_transport::{DeviceId, DeviceInfo, Transport};

use crate::helpers::short_id;
use crate::state::DeviceRuntime;
use crate::user_msg::UserFacingError;
use crate::{
    DaemonCtx, DeviceSnapshot, LogLevel, OUTPUT_RETRY_INTERVAL, RETRY_INTERVAL, UiEvent,
    clear_persistent_warning, emit_user_error, log_event,
};

pub(crate) fn tick_periodic_scan(
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
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
            if mac != &f.id.0
                && ctx.registry.get(&f.id.0).is_some()
                && ctx.registry.rekey(&f.id.0, mac)
            {
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

/// Open the HID device and set initial reporting. Returns `true` if the
/// open succeeded — the device is only marked **`pending`** here;
/// promotion to `connected = true` happens when the first input report
/// actually arrives, and *that* is when we plug a virtual controller.
///
/// Plugging ViGEm here would be wrong: on Windows `hid.open()` succeeds
/// even for paired-but-offline Wiimotes, so we'd repeatedly plug-and-
/// unplug a virtual Xbox 360 pad every retry cycle.
pub(crate) fn try_connect(
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
pub(crate) fn promote_to_connected(
    id: &str,
    ctx: &mut DaemonCtx,
    hid: &mut HidTransport,
    events_tx: &Sender<UiEvent>,
) {
    let Some(slot) = ctx.registry.lowest_free_slot() else {
        // 5th Wiimote refused (B4) — XInput supports at most 4.
        let user_msg = UserFacingError::SlotCapReached.message();
        if let Some(r) = ctx.registry.get_mut(id) {
            r.snapshot.last_error = Some(user_msg.clone());
        }
        emit_user_error(
            events_tx,
            UserFacingError::SlotCapReached,
            format!("slot cap reached, refusing {}", short_id(id)),
        );
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
            // Backend is alive — nothing left to warn about.
            clear_persistent_warning(events_tx, "output_backend");
            clear_persistent_warning(events_tx, "output_unsupported");
            let _ = events_tx.send(log_event(
                LogLevel::Info,
                format!("virtual gamepad ready for {} ({})", short_id(id), profile.label()),
            ));
        }
        Err(e) => {
            // Technical detail (Win32 codes etc.) only goes to the
            // debug log; the UI sees a clean, actionable message.
            debug!("output init failed: {e}");
            let kind = if cfg!(any(target_os = "windows", target_os = "linux")) {
                UserFacingError::OutputBackendUnavailable
            } else {
                UserFacingError::OutputBackendUnsupported
            };
            let user_msg = kind.message();
            if let Some(r) = ctx.registry.get_mut(id) {
                r.snapshot.last_error = Some(user_msg.clone());
                // Don't give up — ViGEmBus often comes back within a
                // few seconds (driver restart, transient state on
                // first ever connect). The retry tick clears the
                // error once it succeeds.
                r.output_retry_at = Some(Instant::now() + OUTPUT_RETRY_INTERVAL);
            }
            emit_user_error(
                events_tx,
                kind,
                format!("output init failed for {}: {e}", short_id(id)),
            );
        }
    }
}

/// Periodically retry `output_for_profile` for devices that are
/// connected but couldn't get a virtual gamepad target the first
/// time. Clears `last_error` and emits a recovery log line on
/// success, otherwise schedules the next attempt.
pub(crate) fn tick_output_retry(now: Instant, ctx: &mut DaemonCtx, events_tx: &Sender<UiEvent>) {
    let due: Vec<(String, MappingProfile)> = ctx
        .registry
        .iter()
        .filter(|(_, r)| {
            r.snapshot.connected
                && r.output.is_none()
                && r.output_retry_at.is_some_and(|t| t <= now)
        })
        .map(|(id, r)| (id.clone(), r.snapshot.mapping_profile))
        .collect();
    for (id, profile) in due {
        match output_for_profile(profile) {
            Ok(out) => {
                if let Some(r) = ctx.registry.get_mut(&id) {
                    r.output = Some(out);
                    r.output_retry_at = None;
                    r.snapshot.last_error = None;
                    ctx.dirty = true;
                }
                // ViGEmBus / uinput came back — drop the global
                // banner along with the per-device chip we already
                // cleared above.
                clear_persistent_warning(events_tx, "output_backend");
                clear_persistent_warning(events_tx, "output_unsupported");
                let _ = events_tx.send(log_event(
                    LogLevel::Info,
                    format!(
                        "virtual gamepad ready for {} (recovered after retry)",
                        short_id(&id)
                    ),
                ));
            }
            Err(e) => {
                debug!("output retry for {id} failed: {e}");
                if let Some(r) = ctx.registry.get_mut(&id) {
                    r.output_retry_at = Some(now + OUTPUT_RETRY_INTERVAL);
                }
            }
        }
    }
}
