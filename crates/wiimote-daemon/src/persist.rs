//! On-disk persistence of the known-device list.
//!
//! Lets the UI show offline placeholders for devices the user has
//! paired before, even before the BT scanner has had a chance to
//! re-discover them on a fresh session.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use wiimote_core::{ExtensionType, PID_WIIMOTE, VID_NINTENDO};
use wiimote_output::MappingProfile;

use crate::state::{DeviceRuntime, DeviceSnapshot};

#[derive(Serialize, Deserialize, Clone)]
pub struct PersistedDevice {
    /// Canonical key — the BT MAC when the device exposes a serial
    /// number, otherwise the HID path. Stable across power cycles
    /// and Windows path renumbering.
    pub id: String,
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Current OS HID path. Used to re-open the device with hidapi.
    /// Defaults to `id` when missing for backward-compat with the
    /// previous format that stored only `id` (= path).
    #[serde(default)]
    pub path: String,
    /// Last extension we identified on this device, as a string label.
    /// Lets the UI show the right icon while the device is offline.
    #[serde(default)]
    pub last_extension: Option<String>,
    /// User-selected mapping profile. Defaults to `Auto`.
    #[serde(default)]
    pub mapping_profile: MappingProfile,
}

#[derive(Serialize, Deserialize, Default)]
pub struct PersistedConfig {
    pub devices: Vec<PersistedDevice>,
}

fn config_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("WiiPair").join("devices.json"))
}

pub fn load() -> Vec<PersistedDevice> {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<PersistedConfig>(&s).ok())
        .map(|c| c.devices)
        .unwrap_or_default()
}

pub fn save(devices: &HashMap<String, DeviceRuntime>) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let cfg = PersistedConfig {
        devices: devices
            .values()
            .map(|r| PersistedDevice {
                id: r.snapshot.id.clone(),
                name: r.snapshot.name.clone(),
                vendor_id: VID_NINTENDO,
                product_id: PID_WIIMOTE,
                path: r.snapshot.path.clone(),
                last_extension: r
                    .snapshot
                    .extension
                    .map(|e| ext_to_str(e).to_string()),
                mapping_profile: r.snapshot.mapping_profile,
            })
            .collect(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&cfg) {
        let _ = std::fs::write(&path, json);
    }
}

/// Hydrate a freshly-loaded persisted device into the runtime form.
pub fn into_snapshot(pd: PersistedDevice) -> DeviceSnapshot {
    let last_ext = pd.last_extension.as_deref().and_then(str_to_ext);
    let path = if pd.path.is_empty() { pd.id.clone() } else { pd.path };
    let mut snap = DeviceSnapshot::new(pd.id, pd.name, path);
    snap.extension = last_ext;
    snap.mapping_profile = pd.mapping_profile;
    snap
}

fn ext_to_str(e: ExtensionType) -> &'static str {
    match e {
        ExtensionType::Nunchuk => "Nunchuk",
        ExtensionType::ClassicController => "ClassicController",
        ExtensionType::ClassicControllerPro => "ClassicControllerPro",
        ExtensionType::Guitar => "Guitar",
        ExtensionType::Drums => "Drums",
        ExtensionType::DjHeroTurntable => "DjHeroTurntable",
        ExtensionType::MotionPlus => "MotionPlus",
        ExtensionType::UDrawTablet => "UDrawTablet",
        ExtensionType::TaikoDrum => "TaikoDrum",
        ExtensionType::Unknown(_) => "Unknown",
    }
}

fn str_to_ext(s: &str) -> Option<ExtensionType> {
    match s {
        "Nunchuk" => Some(ExtensionType::Nunchuk),
        "ClassicController" => Some(ExtensionType::ClassicController),
        "ClassicControllerPro" => Some(ExtensionType::ClassicControllerPro),
        "Guitar" => Some(ExtensionType::Guitar),
        "Drums" => Some(ExtensionType::Drums),
        "DjHeroTurntable" => Some(ExtensionType::DjHeroTurntable),
        "MotionPlus" => Some(ExtensionType::MotionPlus),
        "UDrawTablet" => Some(ExtensionType::UDrawTablet),
        "TaikoDrum" => Some(ExtensionType::TaikoDrum),
        _ => None,
    }
}
