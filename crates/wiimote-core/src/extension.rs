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

use crate::report::Accelerometer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    ///
    /// The Wii extension-ID layout is:
    /// ```text
    /// id[0..2]  device-specific (often 0x00..0x00, but real units —
    ///           especially clones, post-MotionPlus passthrough, or
    ///           freshly-reset Nunchuks — surface 0xFF or other bytes
    ///           here). Used only as a *secondary* discriminator for
    ///           variants that share the same id[2..6] (Classic vs
    ///           Classic Pro, Guitar vs Drums vs DJ Hero turntable).
    /// id[2..4]  always 0xa4 0x20 on a genuine Nintendo / GH/RB
    ///           extension — the family marker.
    /// id[4..6]  the actual model identifier (Nunchuk, Classic, …).
    /// ```
    /// Matching only on `id[2..6]` for the model and leaving `id[0]`
    /// as a soft discriminator is what lets us recognise a Nunchuk
    /// whose first byte is `0xFF` (real-world hardware reports this
    /// after some power-cycle paths) instead of dumping it as
    /// `Unknown`.
    #[must_use]
    pub fn from_id(id: &[u8; 6]) -> Self {
        // Only Nintendo / GH/RB extensions ever set this family
        // marker. Anything else really is unknown.
        if id[2] != 0xa4 || id[3] != 0x20 {
            return Self::Unknown(*id);
        }
        match (id[4], id[5]) {
            // Plain Nunchuk; id[5] varies across batches (0x00 vs 0x01).
            (0x00, 0x00 | 0x01) => Self::Nunchuk,
            // Classic Controller family — Pro variant signalled by
            // id[0] = 0x01.
            (0x01, 0x01) => {
                if id[0] == 0x01 {
                    Self::ClassicControllerPro
                } else {
                    Self::ClassicController
                }
            }
            // Guitar / Drums / DJ Hero turntable share id[4..6] =
            // 0x01 0x03 and disambiguate via id[0].
            (0x01, 0x03) => match id[0] {
                0x01 => Self::Drums,
                0x03 => Self::DjHeroTurntable,
                _ => Self::Guitar,
            },
            // Wii Motion Plus and its passthrough modes.
            (0x04 | 0x05 | 0x07, 0x05) => Self::MotionPlus,
            (0x04, 0x04) => Self::UDrawTablet,
            (0x01, 0x11) => Self::TaikoDrum,
            _ => Self::Unknown(*id),
        }
    }

    /// Short user-facing name. For [`Self::Unknown`] the raw ID bytes
    /// are formatted in hex.
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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

/// Nunchuk live state.
///
/// Per WiiBrew the 6-byte Nunchuk payload is:
/// ```text
/// Byte 0: SX            stick X, 0..=255 (~128 neutral)
/// Byte 1: SY            stick Y, 0..=255 (~128 neutral)
/// Byte 2: AX<9:2>       accel X, high 8 bits
/// Byte 3: AY<9:2>       accel Y, high 8 bits
/// Byte 4: AZ<9:2>       accel Z, high 8 bits
/// Byte 5:
///   bit 7-6: AZ<1:0>    accel Z, low 2 bits
///   bit 5-4: AY<1:0>    accel Y, low 2 bits
///   bit 3-2: AX<1:0>    accel X, low 2 bits
///   bit 1:   BC         C button   (0 = pressed, 1 = released)
///   bit 0:   BZ         Z button   (0 = pressed, 1 = released)
/// ```
/// Stick rests around (128, 128); accel rests around (512, 512, 612)
/// when held with the Z button facing up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NunchukState {
    pub stick_x: u8,
    pub stick_y: u8,
    /// 10-bit raw accelerometer (X, Y, Z). Reconstructed from the
    /// 8 high bits in bytes 2-4 plus the 2 low bits packed in byte 5.
    pub accel: Accelerometer,
    pub c: bool,
    pub z: bool,
}

impl NunchukState {
    #[must_use]
    pub fn parse(b: &[u8; 16]) -> Self {
        // Reconstruct each 10-bit accel axis from the high 8 bits in
        // bytes 2-4 and the 2 low bits packed in byte 5.
        let ax = (u16::from(b[2]) << 2) | u16::from((b[5] >> 2) & 0x03);
        let ay = (u16::from(b[3]) << 2) | u16::from((b[5] >> 4) & 0x03);
        let az = (u16::from(b[4]) << 2) | u16::from((b[5] >> 6) & 0x03);
        Self {
            stick_x: b[0],
            stick_y: b[1],
            accel: Accelerometer { x: ax, y: ay, z: az },
            // Buttons live in byte 5, inverted (1 = released).
            c: (b[5] & 0x02) == 0,
            z: (b[5] & 0x01) == 0,
        }
    }

    #[must_use]
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
    #[must_use]
    pub fn parse(b: &[u8; 16]) -> Self {
        // Buttons in bytes 4-5, inverted: 0 = pressed.
        let raw = u16::from_be_bytes([b[4], b[5]]);
        Self {
            buttons: ClassicButtons::from_bits_truncate(!raw),
        }
    }

    #[must_use]
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
    #[must_use]
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

    #[must_use]
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
    #[must_use]
    pub fn parse(b: &[u8; 16]) -> Self {
        let raw = u16::from_be_bytes([b[4], b[5]]);
        Self {
            buttons: DrumsButtons::from_bits_truncate(!raw),
        }
    }

    #[must_use]
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

    #[test]
    fn nunchuk_first_byte_variants_still_classify() {
        // Real-world ID seen on Nunchuks after some power-cycle paths
        // (and on a few clones): id[0] = 0xFF instead of 0x00. The
        // family marker (id[2..4] = 0xa4 0x20) plus the model bytes
        // (id[4..6] = 0x00 0x00) are what actually identify it.
        assert_eq!(
            ExtensionType::from_id(&[0xff, 0x00, 0xa4, 0x20, 0x00, 0x00]),
            ExtensionType::Nunchuk,
        );
        assert_eq!(
            ExtensionType::from_id(&[0xff, 0xff, 0xa4, 0x20, 0x00, 0x01]),
            ExtensionType::Nunchuk,
        );
    }

    #[test]
    fn classic_pro_distinguished_by_first_byte() {
        // id[4..6] = 0x01 0x01 — Classic Controller. id[0] = 0x01
        // bumps it to the Pro variant; anything else stays Classic.
        assert_eq!(
            ExtensionType::from_id(&[0x00, 0x00, 0xa4, 0x20, 0x01, 0x01]),
            ExtensionType::ClassicController,
        );
        assert_eq!(
            ExtensionType::from_id(&[0x01, 0x00, 0xa4, 0x20, 0x01, 0x01]),
            ExtensionType::ClassicControllerPro,
        );
    }

    #[test]
    fn non_nintendo_family_marker_is_unknown() {
        // Without 0xa4 0x20 in id[2..4] we can't be sure of the
        // protocol; safer to surface as Unknown than guess.
        let id = [0x00, 0x00, 0x12, 0x34, 0x00, 0x00];
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
    fn nunchuk_accel_reconstructs_10_bits() {
        // Synthetic frame: stick neutral, accel ≈ (512, 512, 612), no buttons.
        // Encoded as: AX<9:2>=128 (=0x80), AY<9:2>=128 (=0x80), AZ<9:2>=153 (=0x99).
        // Low bits packed in byte 5: AZ<1:0>=00, AY<1:0>=00, AX<1:0>=00.
        // Buttons released → bits 1+0 = 11.
        let mut b = [0u8; 16];
        b[0] = 128; // stick X neutral
        b[1] = 128; // stick Y neutral
        b[2] = 0x80; // AX high
        b[3] = 0x80; // AY high
        b[4] = 0x99; // AZ high (153 << 2 = 612)
        b[5] = 0b0000_0011; // no extra accel low bits, both buttons released
        let s = NunchukState::parse(&b);
        assert_eq!(s.accel.x, 512);
        assert_eq!(s.accel.y, 512);
        assert_eq!(s.accel.z, 612);
        assert!(!s.c);
        assert!(!s.z);
        assert_eq!(s.stick_x, 128);
        assert_eq!(s.stick_y, 128);
    }

    #[test]
    fn nunchuk_accel_low_bits_packed_in_byte5() {
        // Verify each axis pair correctly steals its low 2 bits from
        // the right slice of byte 5.
        let mut b = [0u8; 16];
        b[2] = 0xFF;
        b[3] = 0xFF;
        b[4] = 0xFF;
        // bits 7-6 = AZ low, bits 5-4 = AY low, bits 3-2 = AX low.
        // Set AX<1:0> = 11, AY<1:0> = 10, AZ<1:0> = 01.
        b[5] = 0b01_10_11_11;
        let s = NunchukState::parse(&b);
        // High 8 bits = 0xFF = 255; low 2 bits make the lower nibble.
        // 255 << 2 = 1020; +3 = 1023, +2 = 1022, +1 = 1021.
        assert_eq!(s.accel.x, 1023);
        assert_eq!(s.accel.y, 1022);
        assert_eq!(s.accel.z, 1021);
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
