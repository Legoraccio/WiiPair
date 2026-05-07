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
    /// Last extension we identified on this device. Tries the new
    /// structural form first via [`ExtensionType`]'s native serde
    /// derive (which preserves `Unknown([u8;6])`); falls back to the
    /// legacy string-label form (`"Nunchuk"`, `"Guitar"`, …) for
    /// configs written before this change.
    #[serde(default, deserialize_with = "deserialize_extension")]
    pub last_extension: Option<ExtensionType>,
    /// User-selected mapping profile. Defaults to `Auto`.
    #[serde(default)]
    pub mapping_profile: MappingProfile,
}

/// Backward-compatible deserializer: accepts either the legacy string
/// (e.g. `"Nunchuk"`) or the structural form
/// (`"Nunchuk"` / `{"Unknown": [..6 bytes..]}`) that the native serde
/// derive on [`ExtensionType`] now produces. The structural form
/// happens to coincide with the legacy strings for known variants —
/// the only thing the legacy path lost was the raw bytes of
/// `Unknown([u8;6])`, which collapsed to the literal string `"Unknown"`
/// (and so doesn't round-trip on either path; we emit `None` for it
/// rather than fabricating a zero-byte ID).
fn deserialize_extension<'de, D>(d: D) -> Result<Option<ExtensionType>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let value: Option<serde_json::Value> = Option::deserialize(d)?;
    let Some(value) = value else { return Ok(None) };
    // Structural form ({"Unknown":[...]}, or one of the unit variants
    // as a bare string) is what `ExtensionType`'s derived
    // Serialize/Deserialize round-trips through. Try it first.
    if let Ok(ext) = serde_json::from_value::<ExtensionType>(value.clone()) {
        return Ok(Some(ext));
    }
    // Legacy form: a plain string label. The unit variants happen to
    // already match (`"Nunchuk"`, `"Guitar"`, …), so the only legacy
    // string we can't structurally parse is `"Unknown"` — we drop that
    // since the original bytes were never persisted.
    match value.as_str() {
        Some("Unknown") => Ok(None),
        Some(other) => Err(D::Error::custom(format!(
            "unrecognised extension label {other:?}"
        ))),
        None => Err(D::Error::custom("expected string or object for extension")),
    }
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
                last_extension: r.snapshot.extension,
                mapping_profile: r.snapshot.mapping_profile,
            })
            .collect(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&cfg) {
        // Atomic write: a crash mid-write must never leave a partial
        // devices.json. Write to a sibling .tmp file then rename — on
        // both POSIX and Windows (`MoveFileExW` with REPLACE_EXISTING),
        // rename within the same directory is atomic and overwrites.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Hydrate a freshly-loaded persisted device into the runtime form.
pub fn into_snapshot(pd: PersistedDevice) -> DeviceSnapshot {
    let path = if pd.path.is_empty() { pd.id.clone() } else { pd.path };
    let mut snap = DeviceSnapshot::new(pd.id, pd.name, path);
    snap.extension = pd.last_extension;
    snap.mapping_profile = pd.mapping_profile;
    snap
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> PersistedDevice {
        serde_json::from_str::<PersistedDevice>(json).unwrap()
    }

    #[test]
    fn round_trips_unknown_extension_with_raw_bytes() {
        // The pre-serde-derive code path collapsed `Unknown([..])` to
        // the literal string "Unknown" and lost the bytes. With the
        // structural form they survive.
        let pd = PersistedDevice {
            id: "AA:BB:CC:DD:EE:FF".into(),
            name: "Wii".into(),
            vendor_id: VID_NINTENDO,
            product_id: PID_WIIMOTE,
            path: String::new(),
            last_extension: Some(ExtensionType::Unknown([1, 2, 3, 4, 5, 6])),
            mapping_profile: MappingProfile::Auto,
        };
        let json = serde_json::to_string(&pd).unwrap();
        let back = parse(&json);
        assert_eq!(
            back.last_extension,
            Some(ExtensionType::Unknown([1, 2, 3, 4, 5, 6]))
        );
    }

    #[test]
    fn legacy_string_label_still_loads_for_known_variants() {
        // Configs written by the pre-serde-derive code stored a string
        // label like "Nunchuk". The `serde::Serialize` derive for
        // unit variants emits the same string, so legacy reads cleanly.
        let json = r#"{
            "id": "x",
            "name": "Wii",
            "vendor_id": 1406,
            "product_id": 774,
            "last_extension": "Nunchuk",
            "mapping_profile": "Auto"
        }"#;
        assert_eq!(parse(json).last_extension, Some(ExtensionType::Nunchuk));
    }

    #[test]
    fn legacy_unknown_label_collapses_to_none() {
        // Pre-serde-derive `Unknown([..])` collapsed to the literal
        // string `"Unknown"` and the bytes were dropped. We can't
        // reconstruct what we never persisted; treat it as no-known-ext.
        let json = r#"{
            "id": "x",
            "name": "Wii",
            "vendor_id": 1406,
            "product_id": 774,
            "last_extension": "Unknown",
            "mapping_profile": "Auto"
        }"#;
        assert_eq!(parse(json).last_extension, None);
    }

    #[test]
    fn missing_extension_field_loads_as_none() {
        let json = r#"{
            "id": "x",
            "name": "Wii",
            "vendor_id": 1406,
            "product_id": 774,
            "mapping_profile": "Auto"
        }"#;
        assert_eq!(parse(json).last_extension, None);
    }
}
