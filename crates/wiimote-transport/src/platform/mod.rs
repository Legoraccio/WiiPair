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

    pub fn pause_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
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
