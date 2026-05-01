//! Wii extension identification and per-extension data parsing.
//!
//! Identification flow (after the Wiimote reports `extension_connected`
//! in its 0x20 status report):
//! 1. Write byte `0x55` to register `0xa400f0` (disables encryption).
//! 2. Read 6 bytes from `0xa400fa` — this is the extension ID.
//! 3. Decode via [`ExtensionType::from_id`].
//!
//! Once identified, switching the Wiimote to reporting mode 0x35
//! gives a 16-byte extension payload alongside buttons + accel; the
//! per-type parsers in this module decode the first 6 bytes of that
//! payload into [`ExtensionData`]. Button bytes are always inverted
//! (bit clear = pressed), as is convention on Wii extensions.
//!
//! On Wii, Guitar Hero and Rock Band guitars/drums share the same
//! extension IDs — the data layout differs only slightly, so the ID
//! alone tells us "guitar" vs "drums" but not "GH vs RB".

use bitflags::bitflags;

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

// ---------------------------------------------------------------------
// Per-extension data parsing
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum ExtensionData {
    Nunchuk(NunchukState),
    Classic(ClassicState),
    Guitar(GuitarState),
    Drums(DrumsState),
    /// Extension was identified but we don't have a parser yet
    /// (Motion Plus, DJ Hero, uDraw, …).
    Unparsed,
}

impl ExtensionData {
    /// Decode the 16-byte extension payload from a 0x35 report given
    /// the previously-identified extension type.
    pub fn parse(ext: ExtensionType, raw: &[u8; 16]) -> Self {
        match ext {
            ExtensionType::Nunchuk => Self::Nunchuk(NunchukState::parse(raw)),
            ExtensionType::ClassicController | ExtensionType::ClassicControllerPro => {
                Self::Classic(ClassicState::parse(raw))
            }
            ExtensionType::Guitar => Self::Guitar(GuitarState::parse(raw)),
            ExtensionType::Drums => Self::Drums(DrumsState::parse(raw)),
            _ => Self::Unparsed,
        }
    }

    /// Names of the buttons currently held on the extension. Useful for
    /// quick UI display.
    pub fn pressed_button_labels(&self) -> Vec<&'static str> {
        match self {
            Self::Nunchuk(s) => s.pressed_buttons(),
            Self::Classic(s) => s.pressed_buttons(),
            Self::Guitar(s) => s.pressed_buttons(),
            Self::Drums(s) => s.pressed_buttons(),
            Self::Unparsed => Vec::new(),
        }
    }
}

// --- Nunchuk ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NunchukState {
    pub stick_x: u8,
    pub stick_y: u8,
    pub c: bool,
    pub z: bool,
}

impl NunchukState {
    pub fn parse(b: &[u8; 16]) -> Self {
        // Buttons live in byte 5, inverted (1 = released).
        Self {
            stick_x: b[0],
            stick_y: b[1],
            c: (b[5] & 0x02) == 0,
            z: (b[5] & 0x01) == 0,
        }
    }

    pub fn pressed_buttons(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.c {
            v.push("C");
        }
        if self.z {
            v.push("Z");
        }
        v
    }
}

// --- Classic Controller ----------------------------------------------

bitflags! {
    /// 16-bit packed buttons of the Classic Controller.
    /// Bits 9 and 8 (0x0200, 0x0100) are unused/reserved.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
    pub struct ClassicButtons: u16 {
        const DPAD_RIGHT = 0x8000;
        const DPAD_DOWN  = 0x4000;
        const LT         = 0x2000;
        const MINUS      = 0x1000;
        const HOME       = 0x0800;
        const PLUS       = 0x0400;
        const RT         = 0x0200;
        const ZL         = 0x0080;
        const B          = 0x0040;
        const Y          = 0x0020;
        const A          = 0x0010;
        const X          = 0x0008;
        const ZR         = 0x0004;
        const DPAD_LEFT  = 0x0002;
        const DPAD_UP    = 0x0001;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClassicState {
    pub buttons: ClassicButtons,
}

impl ClassicState {
    pub fn parse(b: &[u8; 16]) -> Self {
        // Buttons in bytes 4-5, inverted: 0 = pressed.
        let raw = u16::from_be_bytes([b[4], b[5]]);
        Self {
            buttons: ClassicButtons::from_bits_truncate(!raw),
        }
    }

    pub fn pressed_buttons(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        for (flag, name) in [
            (ClassicButtons::A, "A"),
            (ClassicButtons::B, "B"),
            (ClassicButtons::X, "X"),
            (ClassicButtons::Y, "Y"),
            (ClassicButtons::ZL, "ZL"),
            (ClassicButtons::ZR, "ZR"),
            (ClassicButtons::LT, "L"),
            (ClassicButtons::RT, "R"),
            (ClassicButtons::PLUS, "+"),
            (ClassicButtons::MINUS, "−"),
            (ClassicButtons::HOME, "Home"),
            (ClassicButtons::DPAD_UP, "▲"),
            (ClassicButtons::DPAD_DOWN, "▼"),
            (ClassicButtons::DPAD_LEFT, "◀"),
            (ClassicButtons::DPAD_RIGHT, "▶"),
        ] {
            if self.buttons.contains(flag) {
                v.push(name);
            }
        }
        v
    }
}

// --- Guitar (GH/RB) ---------------------------------------------------

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
    pub struct GuitarButtons: u16 {
        // High byte (byte 4):
        const PLUS       = 0x0400; // bit 2
        const MINUS      = 0x1000; // bit 4
        const STRUM_DOWN = 0x4000; // bit 6
        // Low byte (byte 5):
        const STRUM_UP   = 0x0001; // bit 0
        const YELLOW     = 0x0008; // bit 3
        const GREEN      = 0x0010; // bit 4
        const BLUE       = 0x0020; // bit 5
        const RED        = 0x0040; // bit 6
        const ORANGE     = 0x0080; // bit 7
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GuitarState {
    /// 6-bit analog stick X, 0..63 (~32 neutral).
    pub stick_x: u8,
    pub stick_y: u8,
    /// 5-bit touch bar (GH:WT only); 0x0F when absent.
    pub touch_bar: u8,
    /// 5-bit whammy bar position, 0..31 (0 = released).
    pub whammy: u8,
    pub buttons: GuitarButtons,
}

impl GuitarState {
    pub fn parse(b: &[u8; 16]) -> Self {
        let raw = u16::from_be_bytes([b[4], b[5]]);
        Self {
            stick_x: b[0] & 0x3F,
            stick_y: b[1] & 0x3F,
            touch_bar: b[2] & 0x1F,
            whammy: b[3] & 0x1F,
            buttons: GuitarButtons::from_bits_truncate(!raw),
        }
    }

    pub fn pressed_buttons(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        for (flag, name) in [
            (GuitarButtons::GREEN, "Green"),
            (GuitarButtons::RED, "Red"),
            (GuitarButtons::YELLOW, "Yellow"),
            (GuitarButtons::BLUE, "Blue"),
            (GuitarButtons::ORANGE, "Orange"),
            (GuitarButtons::STRUM_UP, "Strum↑"),
            (GuitarButtons::STRUM_DOWN, "Strum↓"),
            (GuitarButtons::PLUS, "+"),
            (GuitarButtons::MINUS, "−"),
        ] {
            if self.buttons.contains(flag) {
                v.push(name);
            }
        }
        v
    }
}

// --- Drums (GH/RB) ----------------------------------------------------

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
    pub struct DrumsButtons: u16 {
        // High byte (byte 4):
        const PLUS       = 0x0400; // bit 2
        const MINUS      = 0x1000; // bit 4
        // Low byte (byte 5):
        const BASS_PEDAL = 0x0004; // bit 2
        const GREEN      = 0x0008; // bit 3
        const BLUE       = 0x0010; // bit 4
        const YELLOW     = 0x0020; // bit 5
        const RED        = 0x0040; // bit 6
        const ORANGE     = 0x0080; // bit 7
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DrumsState {
    pub buttons: DrumsButtons,
}

impl DrumsState {
    pub fn parse(b: &[u8; 16]) -> Self {
        let raw = u16::from_be_bytes([b[4], b[5]]);
        Self {
            buttons: DrumsButtons::from_bits_truncate(!raw),
        }
    }

    pub fn pressed_buttons(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        for (flag, name) in [
            (DrumsButtons::RED, "Red"),
            (DrumsButtons::YELLOW, "Yellow"),
            (DrumsButtons::BLUE, "Blue"),
            (DrumsButtons::GREEN, "Green"),
            (DrumsButtons::ORANGE, "Orange"),
            (DrumsButtons::BASS_PEDAL, "Bass"),
            (DrumsButtons::PLUS, "+"),
            (DrumsButtons::MINUS, "−"),
        ] {
            if self.buttons.contains(flag) {
                v.push(name);
            }
        }
        v
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

    fn ext_buf(byte4: u8, byte5: u8) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[4] = byte4;
        buf[5] = byte5;
        buf
    }

    #[test]
    fn nunchuk_buttons_inverted() {
        // C+Z pressed → byte 5 bit 1 = 0 (C), bit 0 = 0 (Z); rest of byte 5 = 1.
        let buf = {
            let mut b = [0u8; 16];
            b[5] = 0xFC; // 1111_1100
            b
        };
        let s = NunchukState::parse(&buf);
        assert!(s.c);
        assert!(s.z);
    }

    #[test]
    fn guitar_green_and_strum_up() {
        // Green = byte 5 bit 4, Strum↑ = byte 5 bit 0. Inverted.
        // byte 5 raw = !(0x10 | 0x01) & 0xFF = 0xEE
        let buf = ext_buf(0xFF, 0xEE);
        let s = GuitarState::parse(&buf);
        assert!(s.buttons.contains(GuitarButtons::GREEN));
        assert!(s.buttons.contains(GuitarButtons::STRUM_UP));
        assert!(!s.buttons.contains(GuitarButtons::RED));
    }

    #[test]
    fn drums_bass_and_orange() {
        // Bass = byte 5 bit 2, Orange = byte 5 bit 7. Inverted.
        // byte 5 raw = !(0x04 | 0x80) & 0xFF = 0x7B
        let buf = ext_buf(0xFF, 0x7B);
        let s = DrumsState::parse(&buf);
        assert!(s.buttons.contains(DrumsButtons::BASS_PEDAL));
        assert!(s.buttons.contains(DrumsButtons::ORANGE));
        assert!(!s.buttons.contains(DrumsButtons::RED));
    }

    #[test]
    fn classic_a_and_dpad_up() {
        // A = byte 5 bit 4, D-pad Up = byte 5 bit 0. Inverted.
        // byte 5 raw = !(0x10 | 0x01) & 0xFF = 0xEE
        let buf = ext_buf(0xFF, 0xEE);
        let s = ClassicState::parse(&buf);
        assert!(s.buttons.contains(ClassicButtons::A));
        assert!(s.buttons.contains(ClassicButtons::DPAD_UP));
    }
}
