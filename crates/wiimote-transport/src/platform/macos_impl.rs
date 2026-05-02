//! macOS Bluetooth backend.
//!
//! Currently a compile-only stub: the IOBluetooth integration lands in
//! a later milestone. macOS ships a system-wide Bluetooth pairing UI
//! that the user can drive manually; once paired, hidapi sees the
//! Wiimote through IOHIDManager and the rest of the daemon runs.
//!
//! Note: there is no realistic way to expose a virtual XInput pad on
//! modern macOS without a signed DriverKit driver. The macOS output
//! backend therefore falls back to keyboard mapping via CGEvent.

use super::ScannerEvent;
use crossbeam_channel::Sender;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub struct PlatformScanner {
    _events: Sender<ScannerEvent>,
    pause: Arc<AtomicBool>,
}

impl PlatformScanner {
    pub fn new(events: Sender<ScannerEvent>) -> anyhow::Result<Self> {
        Ok(Self {
            _events: events,
            pause: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn pause_handle(&self) -> Arc<AtomicBool> {
        self.pause.clone()
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        anyhow::bail!(
            "macOS BT scanner not implemented yet — pair the Wiimote in System Settings → Bluetooth"
        )
    }
}

pub fn unpair(_addr: u64) -> Result<(), String> {
    Err("unpair not implemented yet on macOS".into())
}
