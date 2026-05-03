//! Per-device runtime state and the registry that owns it.
//!
//! Centralising every per-device map (extension FSM, slot assignment,
//! retry cooldowns, keepalive timestamps, …) into a single
//! [`DeviceRuntime`] struct collapses the 8-10 cleanup lines that each
//! `Forget` / `Disconnect` / `DeviceLost` path used to need into one
//! `runtime.remove(&id)` call. It also makes "what does the daemon
//! know about this device?" trivially answerable without reading
//! HashMap names spread across the file.

use std::collections::HashMap;
use std::time::Instant;

use wiimote_core::{Accelerometer, Buttons, ExtensionData, ExtensionType, IrDots};
use wiimote_output::{ControllerState, MappingProfile, Output};

/// What the UI sees for a given device. The daemon publishes a snapshot
/// of every entry in [`DeviceRegistry`] whenever something changed.
#[derive(Debug, Clone)]
pub struct DeviceSnapshot {
    /// Canonical, stable key — the BT MAC `AA:BB:CC:DD:EE:FF` when the
    /// device exposes a serial number, otherwise the OS HID path. The
    /// daemon uses this as the registry key, and it survives Windows
    /// reassigning a new HID path on the next reconnect.
    pub id: String,
    pub name: String,
    /// Current OS HID path. May change between reconnects on Windows
    /// (collection number / instance ID can be re-issued); we follow
    /// it by re-keying internally on `id` (the BT MAC).
    pub path: String,
    pub connected: bool,
    /// Set when the user explicitly clicked "Disconnect" — auto-retry
    /// stays off until they click "Connect" again. Cleared on Connect.
    pub user_disabled: bool,
    pub last_buttons: Buttons,
    pub last_accel: Accelerometer,
    pub last_ir: IrDots,
    pub battery: Option<u8>,
    /// Type of extension plugged into the Wiimote (Nunchuk, guitar, …).
    /// `None` until the post-status init dance completes, or after the
    /// extension is unplugged.
    pub extension: Option<ExtensionType>,
    /// Live decoded state of the extension (currently held buttons,
    /// stick positions, …). Filled in once the Wiimote is in reporting
    /// mode 0x35; cleared on unplug or disconnect.
    pub ext_data: Option<ExtensionData>,
    /// Mapping profile selected for this device. `Auto` lets the
    /// output backend pick the right layout for whatever extension is
    /// currently plugged in.
    pub mapping_profile: MappingProfile,
    pub last_error: Option<String>,
}

impl DeviceSnapshot {
    pub fn new(id: String, name: String, path: String) -> Self {
        Self {
            id,
            name,
            path,
            connected: false,
            user_disabled: false,
            last_buttons: Buttons::default(),
            last_accel: Accelerometer::default(),
            last_ir: IrDots::default(),
            battery: None,
            extension: None,
            ext_data: None,
            mapping_profile: MappingProfile::default(),
            last_error: None,
        }
    }
}

/// Per-device extension-identification finite-state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionPhase {
    /// We've sent the "0x55 to 0xa400f0" init write; awaiting Ack 0x22.
    InitSent,
    /// Init acked, we've requested the 6-byte ID; awaiting ReadResponse 0x21.
    ReadingId,
    /// Identified — won't redo unless extension is unplugged.
    Identified(ExtensionType),
}

/// Everything the daemon tracks for a single device beyond the
/// public snapshot. Bundled so a single `remove()` does the full
/// teardown that used to require touching 8-10 maps.
pub struct DeviceRuntime {
    pub snapshot: DeviceSnapshot,
    /// Last decoded controller state, fed to the [`Output`] backend.
    pub controller: ControllerState,
    /// Virtual gamepad output target (ViGEm, uinput, CGEvent, …).
    /// `Some` only between `promote_to_connected` and disconnect.
    pub output: Option<Box<dyn Output>>,
    /// HID handle is open but no first input report has arrived yet —
    /// on Windows `hidapi.open()` succeeds even for paired-but-offline
    /// Wiimotes, so opening alone is not proof of connectivity.
    pub pending: bool,
    /// Earliest moment at which we'll try opening this device again.
    pub next_retry: Option<Instant>,
    pub ext_phase: Option<ExtensionPhase>,
    /// Lowest raw whammy value seen so far — used to self-calibrate
    /// the released position to "0 %". Different guitars (and even
    /// different units of the same model) have different rest values.
    pub whammy_baseline: Option<u8>,
    /// Last time we sent a keepalive (RequestStatus) to this device.
    pub last_keepalive: Option<Instant>,
    /// Last input-report arrival time — drives gap-detection.
    pub last_report: Option<Instant>,
    /// Last time we logged a report-gap warning to the UI — throttle.
    pub last_gap_log: Option<Instant>,
    /// 0..=3 player slot assigned to this Wiimote. Drives the LED
    /// pattern and pairs with ViGEm's XInput slot ordering. `None`
    /// until the device is promoted to `connected`.
    pub slot: Option<u8>,
    /// When `Some(t)`, the rumble motor must be turned off at `t`
    /// (end of an Identify pulse).
    pub rumble_off_at: Option<Instant>,
    /// When `output` couldn't be created at promote time (ViGEmBus in
    /// a transient state, uinput permissions race, …) the daemon
    /// retries periodically. `Some(t)` is the next attempt time;
    /// cleared once `output` is populated successfully.
    pub output_retry_at: Option<Instant>,
}

impl DeviceRuntime {
    pub fn new(snapshot: DeviceSnapshot) -> Self {
        Self {
            snapshot,
            controller: ControllerState::default(),
            output: None,
            pending: false,
            next_retry: None,
            ext_phase: None,
            whammy_baseline: None,
            last_keepalive: None,
            last_report: None,
            last_gap_log: None,
            slot: None,
            rumble_off_at: None,
            output_retry_at: None,
        }
    }

    /// Reset all per-session state to the values appropriate for a
    /// disconnected device. Keeps identity (id/name/path/last extension
    /// hint for the icon) so the row stays in the UI as an offline
    /// placeholder.
    pub fn reset_session(&mut self) {
        self.snapshot.connected = false;
        self.snapshot.ext_data = None;
        // Note: we don't clear `extension` here — the UI keeps showing
        // the last-known extension icon while the device is offline.
        self.controller = ControllerState::default();
        self.output = None;
        self.pending = false;
        self.ext_phase = None;
        self.whammy_baseline = None;
        self.last_keepalive = None;
        self.last_report = None;
        self.last_gap_log = None;
        self.slot = None;
        self.rumble_off_at = None;
        self.output_retry_at = None;
    }
}

/// Owns every device the daemon knows about, keyed by the canonical id
/// (BT MAC when available, else HID path).
#[derive(Default)]
pub struct DeviceRegistry {
    map: HashMap<String, DeviceRuntime>,
}

impl DeviceRegistry {
    pub fn get(&self, id: &str) -> Option<&DeviceRuntime> {
        self.map.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut DeviceRuntime> {
        self.map.get_mut(id)
    }

    pub fn insert(&mut self, runtime: DeviceRuntime) {
        self.map.insert(runtime.snapshot.id.clone(), runtime);
    }

    pub fn remove(&mut self, id: &str) -> Option<DeviceRuntime> {
        self.map.remove(id)
    }

    /// Move an existing entry from `old_id` to `new_id`, preserving every
    /// runtime field. Used to migrate legacy snapshots that were keyed
    /// on the HID path before hidapi started returning a stable BT MAC
    /// for them — without this, a fresh enumerate inserts a second entry
    /// under the MAC and the user sees the same device twice in the UI.
    /// No-op if `old_id` doesn't exist or if `new_id` is already taken.
    pub fn rekey(&mut self, old_id: &str, new_id: &str) -> bool {
        if old_id == new_id || self.map.contains_key(new_id) {
            return false;
        }
        if let Some(mut runtime) = self.map.remove(old_id) {
            runtime.snapshot.id = new_id.to_string();
            self.map.insert(new_id.to_string(), runtime);
            true
        } else {
            false
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &DeviceRuntime)> {
        self.map.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &DeviceRuntime> {
        self.map.values()
    }

    pub fn snapshots(&self) -> Vec<DeviceSnapshot> {
        self.map.values().map(|r| r.snapshot.clone()).collect()
    }

    pub fn any_connected(&self) -> bool {
        self.map.values().any(|r| r.snapshot.connected)
    }

    /// Lowest free player slot in 0..=3, or `None` when all four are
    /// taken (XInput's hard cap).
    pub fn lowest_free_slot(&self) -> Option<u8> {
        (0u8..4).find(|s| !self.map.values().any(|r| r.slot == Some(*s)))
    }

    /// Look up the canonical id for a device given its current OS HID
    /// path. The transport sends events keyed by path; the daemon
    /// stores everything keyed by the canonical id, which is stable
    /// across reconnects.
    pub fn id_for_path(&self, path: &str) -> Option<String> {
        self.map
            .iter()
            .find_map(|(id, r)| (r.snapshot.path == path).then(|| id.clone()))
    }
}
