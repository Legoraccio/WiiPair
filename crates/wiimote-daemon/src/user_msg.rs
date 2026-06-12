//! User-facing error catalogue.
//!
//! The UI never shows raw `{e}` strings: every message that surfaces
//! outside of the log panel (persistent banners, per-device error
//! chips, fatal-error window) is one of these variants. The raw error
//! detail still flows into the log panel — that's the place users go
//! when they need the technical detail.

use crate::helpers::short_id;

/// Classifies user-visible failures so the UI can render an actionable
/// message without leaking driver-level strings.
#[derive(Debug, Clone)]
pub enum UserFacingError {
    /// Bluetooth scanner couldn't start — no inquiries, no auto-pair.
    /// The Wiimotes the user already paired manually still work.
    BluetoothScannerUnavailable,
    /// Daemon worker thread exited unexpectedly. Surfaced in the log;
    /// also drives the fatal-error window content when start-up fails.
    DaemonStopped,
    /// Daemon couldn't initialise at startup (Bluetooth radio missing,
    /// HID layer init failed). Drives the fatal-error window text.
    DaemonStartFailed,
    /// `BluetoothSetServiceState` / `RemoveDevice` refused to drop the
    /// pairing — the user has to remove it from the OS BT settings.
    OsUnpairFailed { id: String },
    /// Pair attempt failed: Wiimote let go of 1+2 / dropped out of
    /// pairing mode before the OS could complete the handshake.
    PairFailedTimeout { addr_pretty: String },
    /// PIN handshake rejected — Wiimote refused the legacy MAC PIN.
    PairFailedAuth { addr_pretty: String },
    /// Generic pair failure — neither timeout nor auth.
    PairFailedOther { addr_pretty: String },
    /// Auto-recovery (depair stale entry) couldn't unpair the device.
    AutoRecoveryFailed { addr_pretty: String },
    /// Virtual gamepad backend not available (Windows: ViGEmBus
    /// missing/stopped). Drives the per-device error chip.
    OutputBackendUnavailable,
    /// macOS / unsupported platform: no native virtual pad.
    OutputBackendUnsupported,
    /// All four XInput slots are taken — fifth Wiimote refused.
    SlotCapReached,
    /// Profile rebuild after the user picked a new mapping couldn't
    /// reattach the virtual pad.
    ProfileRebuildFailed,
}

impl UserFacingError {
    /// Short, user-friendly sentence. Never contains driver-level
    /// strings — those go into the log alongside this message.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::BluetoothScannerUnavailable => {
                "Bluetooth scanner unavailable — auto-pair is off. \
                 Already-paired Wiimotes still work; new ones must be \
                 paired manually from the OS Bluetooth settings."
                    .into()
            }
            Self::DaemonStopped => {
                "The pairing service stopped unexpectedly. Restart \
                 WiiPair to recover."
                    .into()
            }
            Self::DaemonStartFailed => {
                "WiiPair couldn't start its Bluetooth service. \
                 Check that your Bluetooth adapter is enabled, then \
                 try again."
                    .into()
            }
            Self::OsUnpairFailed { id } => format!(
                "Couldn't remove {} from the OS Bluetooth registry. \
                 Open your system Bluetooth settings and remove it \
                 manually.",
                short_id(id)
            ),
            Self::PairFailedTimeout { addr_pretty } => format!(
                "Pairing of {addr_pretty} timed out. Keep holding 1+2 \
                 on the Wiimote (the 4 LEDs must stay blinking 1→2→3→4) \
                 until pairing completes."
            ),
            Self::PairFailedAuth { addr_pretty } => format!(
                "Pairing of {addr_pretty} was rejected by the controller. \
                 Power-cycle the Wiimote (pull batteries 30 s, reinsert) \
                 and try again."
            ),
            Self::PairFailedOther { addr_pretty } => format!(
                "Pairing of {addr_pretty} failed. See the log for \
                 details, then click 'Scan for new devices' to retry."
            ),
            Self::AutoRecoveryFailed { addr_pretty } => format!(
                "Auto-recovery for {addr_pretty} failed. Remove the \
                 device from your OS Bluetooth settings, then click \
                 'Scan for new devices'."
            ),
            Self::OutputBackendUnavailable => {
                if cfg!(target_os = "windows") {
                    "Virtual controller output unavailable — install or \
                     restart ViGEmBus, then reconnect the Wiimote."
                        .into()
                } else {
                    "Virtual controller output unavailable — check \
                     /dev/uinput permissions, then reconnect the Wiimote."
                        .into()
                }
            }
            Self::OutputBackendUnsupported => {
                "Virtual controller output isn't supported on this \
                 platform yet. Use a keyboard mapping profile instead."
                    .into()
            }
            Self::SlotCapReached => {
                "Four Wiimotes are already connected — XInput supports \
                 at most four. Disconnect one before connecting another."
                    .into()
            }
            Self::ProfileRebuildFailed => {
                "Couldn't apply the new mapping profile to the active \
                 virtual gamepad. WiiPair will retry automatically."
                    .into()
            }
        }
    }

    /// Stable id used by the UI to deduplicate persistent warnings —
    /// the same kind of failure should produce only one banner even if
    /// the daemon emits the message repeatedly.
    #[must_use]
    pub fn dedup_key(&self) -> &'static str {
        match self {
            Self::BluetoothScannerUnavailable => "bt_scanner",
            Self::DaemonStopped => "daemon_stopped",
            Self::DaemonStartFailed => "daemon_start",
            Self::OsUnpairFailed { .. } => "os_unpair",
            Self::PairFailedTimeout { .. } => "pair_timeout",
            Self::PairFailedAuth { .. } => "pair_auth",
            Self::PairFailedOther { .. } => "pair_other",
            Self::AutoRecoveryFailed { .. } => "auto_recovery",
            Self::OutputBackendUnavailable => "output_backend",
            Self::OutputBackendUnsupported => "output_unsupported",
            Self::SlotCapReached => "slot_cap",
            Self::ProfileRebuildFailed => "profile_rebuild",
        }
    }
}
