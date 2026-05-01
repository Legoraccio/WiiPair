//! Wiimote HID protocol — pure parsing/encoding, no I/O.
//!
//! This crate is intentionally platform-agnostic: it knows how to read
//! the bytes that come off a Wiimote and how to format the bytes to send
//! to one, but does not touch Bluetooth or HID directly.

pub mod buttons;
pub mod extension;
pub mod report;

pub use buttons::Buttons;
pub use extension::ExtensionType;
pub use report::{
    Accelerometer, InputReport, IrDot, IrDots, OutputReport, ReportError, StatusFlags, parse_input,
};

/// Nintendo USB/Bluetooth vendor ID.
pub const VID_NINTENDO: u16 = 0x057E;
/// Original Wiimote (RVL-CNT-01).
pub const PID_WIIMOTE: u16 = 0x0306;
/// Wii Remote Plus (RVL-CNT-01-TR), with MotionPlus inside.
pub const PID_WIIMOTE_PLUS: u16 = 0x0330;

/// Returns true if the (vendor, product) pair identifies any Wiimote variant.
pub fn is_wiimote(vid: u16, pid: u16) -> bool {
    vid == VID_NINTENDO && (pid == PID_WIIMOTE || pid == PID_WIIMOTE_PLUS)
}
