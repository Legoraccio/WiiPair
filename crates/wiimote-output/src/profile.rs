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
}

