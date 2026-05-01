//! Per-OS Bluetooth scanner: discovery + auto-pair + HID-service enable.
//!
//! On platforms where it isn't implemented yet, [`PlatformScanner::start`]
//! is a no-op so the daemon can run unchanged.

#[derive(Debug, Clone)]
pub enum ScannerEvent {
    /// Periodic inquiry surfaced this device. `paired`/`connected` are
    /// the OS's view at the moment of the inquiry.
    Discovered {
        addr: u64,
        name: String,
        paired: bool,
        connected: bool,
    },
    /// Pairing dance has started for this address.
    Pairing { addr: u64 },
    /// Pairing succeeded.
    Paired { addr: u64 },
    /// Pairing failed; `reason` is OS-level.
    PairFailed { addr: u64, reason: String },
    /// HID service was activated on the device — at this point hidapi
    /// enumeration should pick it up on the next refresh.
    HidEnabled { addr: u64 },
    /// Non-fatal scanner-level error (e.g. inquiry failed once).
    Error(String),
}

#[cfg(windows)]
mod windows_impl;

#[cfg(windows)]
pub use windows_impl::PlatformScanner;

#[cfg(not(windows))]
pub struct PlatformScanner {
    _events: crossbeam_channel::Sender<ScannerEvent>,
}

#[cfg(not(windows))]
impl PlatformScanner {
    pub fn new(events: crossbeam_channel::Sender<ScannerEvent>) -> anyhow::Result<Self> {
        Ok(Self { _events: events })
    }

    /// No-op until Linux (BlueZ) and macOS (IOBluetooth) backends land.
    pub fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
