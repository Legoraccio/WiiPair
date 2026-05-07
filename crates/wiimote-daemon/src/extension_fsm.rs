//! Extension-identification FSM: handles the post-status init → read-id
//! → mode-switch dance for whatever is plugged into the Wiimote's
//! expansion port.

use crossbeam_channel::Sender;
use wiimote_core::{ExtensionData, ExtensionType, InputReport, OutputReport};
use wiimote_transport::hid::HidTransport;
use wiimote_transport::{DeviceId, Transport};

use crate::helpers::short_id;
use crate::state::ExtensionPhase;
use crate::{DaemonCtx, LogLevel, UiEvent, log_event};

pub(crate) fn process_extension_fsm(
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
                }
            }

            // Wiimote spec: every Status response resets the
            // reporting mode back to 0x30 (default, buttons-only).
            // Without re-affirming our mode here, the daemon's 200 ms
            // keepalive `RequestStatus` would silently kill accel +
            // extension data as soon as the first status reply
            // lands — which is exactly what masked the
            // accelerometer + Guitar Hero extension reports on
            // Linux for an entire debug session.
            let mode = match ctx.registry.get(id).and_then(|r| r.ext_phase) {
                Some(ExtensionPhase::Identified(_)) => 0x35,
                _ => 0x31,
            };
            let _ = hid.send(
                path_id,
                &OutputReport::SetReportingMode {
                    continuous: true,
                    mode,
                }
                .encode(),
            );
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
                        let span = 31u32.saturating_sub(u32::from(*baseline)).max(1);
                        let above = g.whammy.saturating_sub(*baseline);
                        g.whammy = ((u32::from(above) * 31) / span) as u8;
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
