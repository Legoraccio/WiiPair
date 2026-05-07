//! Per-OS Bluetooth scanner: discovery + auto-pair + HID-service enable.
//!
//! On platforms where it isn't implemented yet, [`PlatformScanner::start`]
//! is a no-op so the daemon can run unchanged.
//!
//! All three platform impls (`windows_impl`, `linux_impl`, `macos_impl`)
//! plus the unsupported-platform stub satisfy the [`Scanner`] trait —
//! that trait is the explicit cross-platform contract. The daemon
//! itself goes through the [`PlatformScanner`] type alias re-exported
//! below, but the trait makes it a compile error if one of the
//! implementations forgets to provide a method.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Cross-platform contract every `PlatformScanner` impl must satisfy.
/// Without this trait, divergence between Windows/Linux/macOS shapes is
/// only caught at the daemon's call site — and only on whichever target
/// happens to be compiled first.
pub trait Scanner: Send {
    /// Construct a fresh scanner that will publish discovery / pairing
    /// outcomes into `events`. Failure here means a critical OS-level
    /// dependency (Bluetooth stack, DBus, etc.) is missing.
    fn new(events: crossbeam_channel::Sender<ScannerEvent>) -> anyhow::Result<Self>
    where
        Self: Sized;

    /// Hand the daemon the pause flag so it can suspend active BT
    /// inquiry while at least one Wiimote is connected — inquiry hop
    /// windows otherwise starve the active connection.
    fn pause_handle(&self) -> Arc<AtomicBool>;

    /// Start the background scan thread. Returns `Err` if the OS
    /// refuses to start one (e.g. macOS where the BT scanner isn't
    /// implemented yet).
    fn start(&mut self) -> anyhow::Result<()>;
}

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
    /// The OS thinks the device is paired but the HID service isn't
    /// advertised — almost always Wii Remote Plus on Windows after a
    /// power-cycle, where `BluetoothSetServiceState(HID)` fails until
    /// the device is unpaired and re-paired. The daemon uses this as
    /// a trigger to auto-recover during a manual scan window.
    SdpCacheStale { addr: u64 },
    /// `BluetoothAuthenticateDeviceEx` came back with
    /// `ERROR_GEN_FAILURE` (0x1F) — the BT registry has a stale
    /// `paired=false, connected=true` entry that the OS won't let us
    /// re-auth. Same recovery path as `SdpCacheStale`.
    AuthStuck { addr: u64 },
    /// Non-fatal scanner-level error (e.g. inquiry failed once).
    Error(String),
}

#[cfg(windows)]
mod windows_impl;

#[cfg(windows)]
pub use windows_impl::PlatformScanner;

#[cfg(target_os = "linux")]
mod linux_impl;
#[cfg(target_os = "linux")]
mod linux_mgmt;

#[cfg(target_os = "linux")]
pub use linux_impl::PlatformScanner;

#[cfg(target_os = "macos")]
mod macos_impl;

#[cfg(target_os = "macos")]
pub use macos_impl::PlatformScanner;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub struct PlatformScanner {
    _events: crossbeam_channel::Sender<ScannerEvent>,
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
impl PlatformScanner {
    pub fn new(events: crossbeam_channel::Sender<ScannerEvent>) -> anyhow::Result<Self> {
        Ok(Self { _events: events })
    }

    pub fn pause_handle(&self) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

// Single trait impl that compiles against whichever per-OS
// `PlatformScanner` is gated in by the `cfg`s above. The body just
// forwards to the inherent methods (no trait-method recursion: method
// resolution prefers inherent over trait when both share a name) — so
// adding a method to `Scanner` later will fail to compile on whichever
// platform forgot to implement it inherently, instead of silently
// diverging.
impl Scanner for PlatformScanner {
    fn new(events: crossbeam_channel::Sender<ScannerEvent>) -> anyhow::Result<Self> {
        Self::new(events)
    }
    fn pause_handle(&self) -> Arc<AtomicBool> {
        self.pause_handle()
    }
    fn start(&mut self) -> anyhow::Result<()> {
        self.start()
    }
}

/// Remove a paired device from the OS Bluetooth registry. Used by the
/// `Forget` UI command — without this the next inquiry cycle would
/// re-discover and re-add the device because the OS still considers it
/// paired. Returns `Ok(())` on platforms where unpair isn't wired up
/// yet, so the daemon's higher-level Forget bookkeeping still runs.
#[allow(unused_variables)]
pub fn unpair_addr(addr: u64) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_impl::unpair(addr)
    }
    #[cfg(target_os = "linux")]
    {
        linux_impl::unpair(addr)
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::unpair(addr)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Ok(())
    }
}

/// Parse a canonical `AA:BB:CC:DD:EE:FF` MAC into the `u64` LSB-first
/// representation used by the platform Bluetooth APIs. Returns `None`
/// for anything that doesn't look like a colon-separated 6-byte MAC.
#[must_use]
pub fn mac_to_u64(mac: &str) -> Option<u64> {
    let parts: Vec<&str> = mac.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut bytes = [0u8; 8];
    for (i, p) in parts.iter().enumerate() {
        bytes[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::mac_to_u64;

    #[test]
    fn parses_canonical_mac() {
        // First MAC byte ends up as the LSB of the u64.
        let v = mac_to_u64("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(v.to_le_bytes()[0], 0xAA);
        assert_eq!(v.to_le_bytes()[5], 0xFF);
        assert_eq!(v.to_le_bytes()[6], 0);
    }

    #[test]
    fn rejects_bad_mac() {
        assert!(mac_to_u64("not-a-mac").is_none());
        assert!(mac_to_u64("AA:BB:CC:DD:EE").is_none());
        assert!(mac_to_u64("AA:BB:CC:DD:EE:GG").is_none());
    }
}
