//! Wii extension identification.
//!
//! Standard sequence after the Wiimote reports `extension_connected`
//! in its 0x20 status report:
//! 1. Write byte `0x55` to register `0xa400f0` (disables encryption).
//! 2. Read 6 bytes from `0xa400fa` — this is the extension ID.
//! 3. Decode via [`ExtensionType::from_id`].
//!
//! On Wii, both Guitar Hero and Rock Band guitars/drums share the same
//! extension IDs — the data layout differs only slightly, so the ID
//! alone tells us "guitar" vs "drums" but not "GH vs RB".

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionType {
    Nunchuk,
    ClassicController,
    ClassicControllerPro,
    /// 5-fret guitar — covers Guitar Hero (Wii) and Rock Band (Wii).
    Guitar,
    /// 4-pad drum kit — covers Guitar Hero (Wii) and Rock Band (Wii).
    Drums,
    DjHeroTurntable,
    /// Wii Motion Plus, optionally with a passthrough extension.
    MotionPlus,
    UDrawTablet,
    TaikoDrum,
    /// Recognized as some kind of extension but the ID is not in our
    /// table — surfaced verbatim so the user can read the bytes.
    Unknown([u8; 6]),
}

impl ExtensionType {
    /// Map a 6-byte plain (post-0x55-init) extension ID to a known type.
    pub fn from_id(id: &[u8; 6]) -> Self {
        match id {
            [0x00, 0x00, 0xa4, 0x20, 0x00, 0x00] => Self::Nunchuk,
            [0x00, 0x00, 0xa4, 0x20, 0x00, 0x01] => Self::Nunchuk,
            [0x00, 0x00, 0xa4, 0x20, 0x01, 0x01] => Self::ClassicController,
            [0x01, 0x00, 0xa4, 0x20, 0x01, 0x01] => Self::ClassicControllerPro,
            [0x00, 0x00, 0xa4, 0x20, 0x01, 0x03] => Self::Guitar,
            [0x01, 0x00, 0xa4, 0x20, 0x01, 0x03] => Self::Drums,
            [0x03, 0x00, 0xa4, 0x20, 0x01, 0x03] => Self::DjHeroTurntable,
            [0x00, 0x00, 0xa4, 0x20, 0x04, 0x05] => Self::MotionPlus,
            [0x00, 0x00, 0xa4, 0x20, 0x05, 0x05] => Self::MotionPlus,
            [0x00, 0x00, 0xa4, 0x20, 0x07, 0x05] => Self::MotionPlus,
            [0x00, 0x00, 0xa4, 0x20, 0x04, 0x04] => Self::UDrawTablet,
            [0x00, 0x00, 0xa4, 0x20, 0x01, 0x11] => Self::TaikoDrum,
            other => Self::Unknown(*other),
        }
    }

    /// Short user-facing name. For [`Self::Unknown`] the raw ID bytes
    /// are formatted in hex.
    pub fn label(&self) -> String {
        match self {
            Self::Nunchuk => "Nunchuk".into(),
            Self::ClassicController => "Classic Controller".into(),
            Self::ClassicControllerPro => "Classic Controller Pro".into(),
            Self::Guitar => "Guitar (GH/RB)".into(),
            Self::Drums => "Drums (GH/RB)".into(),
            Self::DjHeroTurntable => "DJ Hero Turntable".into(),
            Self::MotionPlus => "Motion Plus".into(),
            Self::UDrawTablet => "uDraw Tablet".into(),
            Self::TaikoDrum => "Taiko Drum".into(),
            Self::Unknown(id) => format!(
                "Unknown ({:02x} {:02x} {:02x} {:02x} {:02x} {:02x})",
                id[0], id[1], id[2], id[3], id[4], id[5]
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_extensions() {
        assert_eq!(
            ExtensionType::from_id(&[0x00, 0x00, 0xa4, 0x20, 0x00, 0x00]),
            ExtensionType::Nunchuk
        );
        assert_eq!(
            ExtensionType::from_id(&[0x00, 0x00, 0xa4, 0x20, 0x01, 0x03]),
            ExtensionType::Guitar
        );
        assert_eq!(
            ExtensionType::from_id(&[0x01, 0x00, 0xa4, 0x20, 0x01, 0x03]),
            ExtensionType::Drums
        );
    }

    #[test]
    fn unknown_id_is_passed_through() {
        let id = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x00];
        assert_eq!(ExtensionType::from_id(&id), ExtensionType::Unknown(id));
    }
}
