//! Mapping profiles describe *how* a Wiimote (and its extension)
//! translates into virtual-controller output: XInput pad mapping for
//! ViGEm, evdev mapping for Linux uinput, keyboard codes for the
//! macOS CGEvent fallback.
//!
//! The profile is a small enum + parameter struct rather than a
//! free-form remap table — three pre-baked layouts ("Wiimote as Xbox
//! 360", "Guitar as Xplorer", "Classic as Xbox 360") cover every
//! supported game we know of without hand-editing JSON.

use serde::{Deserialize, Serialize};
use wiimote_core::ExtensionData;

/// Pre-baked mapping layouts. `Auto` lets the backend pick the
/// best-fit layout based on which extension is currently plugged in
/// (Wiimote → Xbox, Guitar → Xplorer, Drums → Xplorer drums, Classic
/// → Xbox).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MappingProfile {
    #[default]
    Auto,
    /// Bare Wiimote → Xbox 360 pad: tilt → left stick, A/B → A/B,
    /// 1/2 → X/Y, +/− → Start/Back, Home → Guide.
    WiimoteXbox,
    /// Guitar Hero / Rock Band guitar → Xplorer X360 layout. The one
    /// Clone Hero auto-recognises.
    GuitarXplorer,
    /// 5-pad drum kit → Xplorer drums layout.
    DrumsXplorer,
    /// Classic Controller Pro → Xbox 360 pad (face buttons direct,
    /// triggers as analog).
    ClassicXbox,
    /// Bare Wiimote → keyboard fallback (used on macOS where no
    /// virtual-pad output is available).
    WiimoteKeyboard,
    /// Guitar → keyboard fallback.
    GuitarKeyboard,
}

impl MappingProfile {
    /// Short user-facing name for the dropdown.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::WiimoteXbox => "Wiimote → Xbox",
            Self::GuitarXplorer => "Guitar → Xplorer",
            Self::DrumsXplorer => "Drums → Xplorer",
            Self::ClassicXbox => "Classic → Xbox",
            Self::WiimoteKeyboard => "Wiimote → Keyboard",
            Self::GuitarKeyboard => "Guitar → Keyboard",
        }
    }

    /// Profiles offered to the user in the UI dropdown.
    #[must_use]
    pub fn all() -> &'static [MappingProfile] {
        &[
            Self::Auto,
            Self::WiimoteXbox,
            Self::GuitarXplorer,
            Self::DrumsXplorer,
            Self::ClassicXbox,
            Self::WiimoteKeyboard,
            Self::GuitarKeyboard,
        ]
    }

    /// Resolve this profile to the concrete pad layout to drive a virtual
    /// XInput output (Windows ViGEm, Linux uinput).
    ///
    /// `Auto` picks the best-fit layout from `ext`, including drum kits
    /// — Linux already did this; the Windows backend used to fall
    /// through to the bare-Wiimote layout for drums-in-Auto, which we
    /// now harmonise. A specific layout (e.g. `GuitarXplorer`) falls
    /// back to `Wiimote` if the matching extension isn't actually
    /// plugged in. Keyboard profiles aren't applicable on pad
    /// backends, so they collapse to the auto-pad layout — the user
    /// still gets a working pad even after picking a keyboard profile
    /// in the UI.
    #[must_use]
    pub fn resolve_pad(self, ext: Option<&ExtensionData>) -> PadLayout {
        let auto = || match ext {
            Some(ExtensionData::Guitar(_)) => PadLayout::Guitar,
            Some(ExtensionData::Drums(_)) => PadLayout::Drums,
            Some(ExtensionData::Classic(_)) => PadLayout::Classic,
            _ => PadLayout::Wiimote,
        };
        match self {
            Self::Auto => auto(),
            Self::WiimoteXbox => PadLayout::Wiimote,
            Self::GuitarXplorer => match ext {
                Some(ExtensionData::Guitar(_)) => PadLayout::Guitar,
                _ => PadLayout::Wiimote,
            },
            Self::DrumsXplorer => match ext {
                Some(ExtensionData::Drums(_)) => PadLayout::Drums,
                _ => PadLayout::Wiimote,
            },
            Self::ClassicXbox => match ext {
                Some(ExtensionData::Classic(_)) => PadLayout::Classic,
                _ => PadLayout::Wiimote,
            },
            Self::WiimoteKeyboard | Self::GuitarKeyboard => auto(),
        }
    }
}

/// Concrete pad layout the backend should drive — output of
/// [`MappingProfile::resolve_pad`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadLayout {
    Wiimote,
    Guitar,
    Drums,
    Classic,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiimote_core::{ClassicState, DrumsState, GuitarState, NunchukState};

    fn guitar_ext() -> ExtensionData {
        ExtensionData::Guitar(GuitarState::default())
    }
    fn drums_ext() -> ExtensionData {
        ExtensionData::Drums(DrumsState::default())
    }
    fn classic_ext() -> ExtensionData {
        ExtensionData::Classic(ClassicState::default())
    }
    fn nunchuk_ext() -> ExtensionData {
        ExtensionData::Nunchuk(NunchukState {
            stick_x: 128,
            stick_y: 128,
            accel: wiimote_core::Accelerometer { x: 512, y: 512, z: 612 },
            c: false,
            z: false,
        })
    }

    #[test]
    fn auto_picks_layout_from_extension() {
        assert_eq!(MappingProfile::Auto.resolve_pad(None), PadLayout::Wiimote);
        assert_eq!(
            MappingProfile::Auto.resolve_pad(Some(&guitar_ext())),
            PadLayout::Guitar
        );
        assert_eq!(
            MappingProfile::Auto.resolve_pad(Some(&drums_ext())),
            PadLayout::Drums
        );
        assert_eq!(
            MappingProfile::Auto.resolve_pad(Some(&classic_ext())),
            PadLayout::Classic
        );
        assert_eq!(
            MappingProfile::Auto.resolve_pad(Some(&nunchuk_ext())),
            PadLayout::Wiimote
        );
    }

    #[test]
    fn explicit_profile_falls_back_to_wiimote_when_ext_missing() {
        assert_eq!(
            MappingProfile::GuitarXplorer.resolve_pad(None),
            PadLayout::Wiimote
        );
        assert_eq!(
            MappingProfile::ClassicXbox.resolve_pad(Some(&guitar_ext())),
            PadLayout::Wiimote
        );
        assert_eq!(
            MappingProfile::DrumsXplorer.resolve_pad(Some(&classic_ext())),
            PadLayout::Wiimote
        );
    }

    #[test]
    fn keyboard_profiles_collapse_to_auto_on_pad_backend() {
        assert_eq!(
            MappingProfile::WiimoteKeyboard.resolve_pad(Some(&guitar_ext())),
            PadLayout::Guitar
        );
        assert_eq!(
            MappingProfile::GuitarKeyboard.resolve_pad(None),
            PadLayout::Wiimote
        );
    }
}
