//! Bluetooth HID transport for Wiimotes.
//!
//! Two layers, kept distinct:
//!
//! * [`hid`] — cross-platform HID I/O via `hidapi`, for devices that
//!   already appear as HID to the OS.
//! * [`platform`] — per-OS Bluetooth scanning + auto-pairing that turns
//!   a Wiimote-in-the-air into a HID device (Windows: legacy PIN auth
//!   with the Wiimote-MAC-reversed trick + enable of the HID service).

pub mod hid;
pub mod platform;

use thiserror::Error;
use wiimote_core::InputReport;

/// Opaque per-device identifier — currently the OS HID device path,
/// stable for the lifetime of a pairing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Bluetooth MAC formatted `AA:BB:CC:DD:EE:FF` (uppercase) when
    /// available. The daemon uses this as the canonical key — it's
    /// stable across power cycles and Windows path renumbering.
    pub mac: Option<String>,
}

#[derive(Debug)]
pub enum TransportEvent {
    DeviceFound(DeviceInfo),
    DeviceLost(DeviceId),
    Report { id: DeviceId, report: InputReport },
    Error { id: Option<DeviceId>, error: TransportError },
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("device not open: {0}")]
    NotOpen(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("hid error: {0}")]
    Hid(#[from] hidapi::HidError),
}

pub trait Transport: Send {
    /// Send raw bytes (already including the report ID) to the device.
    fn send(&mut self, id: &DeviceId, payload: &[u8]) -> Result<(), TransportError>;
    /// Stop reading from this device and release its handle.
    fn close(&mut self, id: &DeviceId) -> Result<(), TransportError>;
}
