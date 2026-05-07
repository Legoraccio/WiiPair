//! Small utilities shared across the daemon submodules: input-report
//! decomposition, address/id formatting, and the persist-view shim.

use wiimote_core::{Buttons, InputReport};

use crate::state::DeviceRuntime;

/// Decompose an input report into the four optional fields the UI
/// snapshot tracks.
pub(crate) fn decompose(
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

pub(crate) fn format_addr(addr: u64) -> String {
    let b = addr.to_le_bytes();
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        b[5], b[4], b[3], b[2], b[1], b[0]
    )
}

pub(crate) fn short_id(id: &str) -> String {
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
/// `DeviceSnapshot` already clones every field (including
/// `extension`), so a fresh runtime built from it is enough.
pub(crate) fn clone_runtime_view(r: &DeviceRuntime) -> DeviceRuntime {
    DeviceRuntime::new(r.snapshot.clone())
}
