//! Virtual controller backends.
//!
//! - **Windows**: ViGEmBus virtual Xbox 360 pad (requires the ViGEmBus
//!   driver from <https://github.com/nefarius/ViGEmBus/releases>).
//! - **Linux**: `uinput` virtual Xbox 360 device (`/dev/uinput`).
//! - **macOS**: CGEvent keyboard mapping fallback — modern macOS
//!   requires a signed DriverKit driver for a real virtual gamepad.

mod profile;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

pub use profile::MappingProfile;

use wiimote_core::{Accelerometer, Buttons, ExtensionData, IrDots};

#[derive(Debug, Clone, Copy, Default)]
pub struct ControllerState {
    pub buttons: Buttons,
    pub accel: Accelerometer,
    pub ir: IrDots,
    /// Decoded state of the extension currently plugged into the
    /// Wiimote, if any. Drives instrument-specific output mappings.
    pub ext: Option<ExtensionData>,
}

pub trait Output: Send {
    fn update(&mut self, state: &ControllerState) -> anyhow::Result<()>;
}

/// Cheap smoke test for the platform output backend, run by the UI at
/// startup so the user gets a clear "install ViGEmBus" dialog *before*
/// they pair their first Wiimote — instead of finding out only when
/// the virtual pad fails to appear in their game.
///
/// * Windows: tries to open a ViGEmBus client connection and drops it
///   immediately. Fails when the driver isn't installed or the
///   service isn't running.
/// * Linux: checks that `/dev/uinput` is writable.
/// * macOS: always Ok — the CGEvent backend has no install-time
///   prerequisite (Accessibility permission is requested lazily).
pub fn probe_default() -> Result<(), ProbeFailure> {
    #[cfg(windows)]
    {
        match vigem_client::Client::connect() {
            Ok(_) => Ok(()),
            Err(e) => Err(ProbeFailure {
                kind: ProbeKind::ViGEmBusMissing,
                detail: format!("{e}"),
            }),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
        {
            Ok(_) => Ok(()),
            Err(e) => Err(ProbeFailure {
                kind: ProbeKind::UinputUnavailable,
                detail: format!("{e}"),
            }),
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Ok(())
    }
}

/// Why the platform output backend isn't ready. The kind drives the
/// UI's "how do I fix this?" dialog; `detail` is the raw error string
/// for the log.
#[derive(Debug, Clone)]
pub struct ProbeFailure {
    pub kind: ProbeKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    /// Windows: ViGEmBus driver not installed / service stopped.
    ViGEmBusMissing,
    /// Linux: /dev/uinput not writable (missing udev rule, user not
    /// in the `input` group, or kernel without the uinput module).
    UinputUnavailable,
}

/// Build the platform output backend with a specific mapping profile.
/// On Windows that profile drives ViGEm; on Linux it drives uinput;
/// on macOS the keyboard-mapping profiles select the CGEvent fallback
/// while the pad-mapping profiles error out (no native virtual pad).
#[allow(unused_variables)]
pub fn output_for_profile(profile: MappingProfile) -> anyhow::Result<Box<dyn Output>> {
    #[cfg(windows)]
    {
        Ok(Box::new(windows::ViGEmOutput::new(profile)?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::UinputOutput::new(profile)?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::CGEventOutput::new(profile)?))
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        anyhow::bail!("output backend not yet implemented on this platform")
    }
}

#[cfg(windows)]
pub mod windows {
    use super::{ControllerState, MappingProfile, Output};
    use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};
    use wiimote_core::{
        Buttons, ClassicButtons, ClassicState, ExtensionData, GuitarButtons, GuitarState,
    };

    /// Wiimote accelerometer is centred near 512 on X/Y when held flat
    /// (Z offset by gravity, ~612). Within ±DEADZONE of 512 we treat
    /// the stick as neutral.
    const ACCEL_CENTER: i32 = 512;
    const ACCEL_DEADZONE: i32 = 30;
    /// Approximate deflection at 45° of tilt — full stick at that angle.
    const ACCEL_RANGE: i32 = 220;

    pub struct ViGEmOutput {
        target: Xbox360Wired<Client>,
        profile: MappingProfile,
    }

    impl ViGEmOutput {
        pub fn new(profile: MappingProfile) -> anyhow::Result<Self> {
            let client = Client::connect().map_err(|e| {
                anyhow::anyhow!(
                    "ViGEmBus unavailable ({e}). \
                     Install the driver from \
                     https://github.com/nefarius/ViGEmBus/releases and restart."
                )
            })?;
            let mut target = Xbox360Wired::new(client, TargetId::XBOX360_WIRED);
            target
                .plugin()
                .map_err(|e| anyhow::anyhow!("vigem plugin: {e}"))?;
            target
                .wait_ready()
                .map_err(|e| anyhow::anyhow!("vigem wait_ready: {e}"))?;
            Ok(Self { target, profile })
        }
    }

    impl Output for ViGEmOutput {
        fn update(&mut self, state: &ControllerState) -> anyhow::Result<()> {
            let gamepad = match self.profile {
                MappingProfile::Auto => match &state.ext {
                    Some(ExtensionData::Guitar(g)) => guitar_gamepad(g, state),
                    Some(ExtensionData::Classic(c)) => classic_gamepad(c, state),
                    _ => wiimote_gamepad(state),
                },
                MappingProfile::WiimoteXbox => wiimote_gamepad(state),
                MappingProfile::GuitarXplorer => match &state.ext {
                    Some(ExtensionData::Guitar(g)) => guitar_gamepad(g, state),
                    _ => wiimote_gamepad(state),
                },
                MappingProfile::DrumsXplorer => match &state.ext {
                    Some(ExtensionData::Drums(d)) => drums_gamepad(d, state),
                    _ => wiimote_gamepad(state),
                },
                MappingProfile::ClassicXbox => match &state.ext {
                    Some(ExtensionData::Classic(c)) => classic_gamepad(c, state),
                    _ => wiimote_gamepad(state),
                },
                // Keyboard profiles aren't applicable on Windows + ViGEm —
                // fall back to the auto-pad mapping so the pad is still
                // useful.
                MappingProfile::WiimoteKeyboard | MappingProfile::GuitarKeyboard => {
                    match &state.ext {
                        Some(ExtensionData::Guitar(g)) => guitar_gamepad(g, state),
                        _ => wiimote_gamepad(state),
                    }
                }
            };
            self.target
                .update(&gamepad)
                .map_err(|e| anyhow::anyhow!("vigem update: {e}"))?;
            Ok(())
        }
    }

    fn classic_gamepad(c: &ClassicState, state: &ControllerState) -> XGamepad {
        let mut raw: u16 = 0;
        for (flag, xb) in [
            (ClassicButtons::A, XButtons::A),
            (ClassicButtons::B, XButtons::B),
            (ClassicButtons::X, XButtons::X),
            (ClassicButtons::Y, XButtons::Y),
            (ClassicButtons::ZL, XButtons::LB),
            (ClassicButtons::ZR, XButtons::RB),
            (ClassicButtons::PLUS, XButtons::START),
            (ClassicButtons::MINUS, XButtons::BACK),
            (ClassicButtons::HOME, XButtons::GUIDE),
            (ClassicButtons::DPAD_UP, XButtons::UP),
            (ClassicButtons::DPAD_DOWN, XButtons::DOWN),
            (ClassicButtons::DPAD_LEFT, XButtons::LEFT),
            (ClassicButtons::DPAD_RIGHT, XButtons::RIGHT),
        ] {
            if c.buttons.contains(flag) {
                raw |= xb;
            }
        }
        let _ = state; // Wiimote tilt is ignored when a Classic is plugged.
        XGamepad {
            buttons: XButtons { raw },
            thumb_lx: 0,
            thumb_ly: 0,
            thumb_rx: 0,
            thumb_ry: 0,
            // Classic Controller has digital L/R triggers; map to full-range.
            left_trigger: if c.buttons.contains(ClassicButtons::LT) {
                255
            } else {
                0
            },
            right_trigger: if c.buttons.contains(ClassicButtons::RT) {
                255
            } else {
                0
            },
        }
    }

    fn drums_gamepad(d: &wiimote_core::DrumsState, state: &ControllerState) -> XGamepad {
        use wiimote_core::DrumsButtons;
        let mut raw: u16 = 0;
        // Xplorer drum layout: red→B, yellow→Y, blue→X, green→A,
        // orange→LB (cymbal), bass→RB (kick).
        for (flag, xb) in [
            (DrumsButtons::GREEN, XButtons::A),
            (DrumsButtons::RED, XButtons::B),
            (DrumsButtons::BLUE, XButtons::X),
            (DrumsButtons::YELLOW, XButtons::Y),
            (DrumsButtons::ORANGE, XButtons::LB),
            (DrumsButtons::BASS_PEDAL, XButtons::RB),
            (DrumsButtons::PLUS, XButtons::START),
            (DrumsButtons::MINUS, XButtons::BACK),
        ] {
            if d.buttons.contains(flag) {
                raw |= xb;
            }
        }
        if state.buttons.contains(Buttons::HOME) {
            raw |= XButtons::GUIDE;
        }
        XGamepad {
            buttons: XButtons { raw },
            thumb_lx: 0,
            thumb_ly: 0,
            thumb_rx: 0,
            thumb_ry: 0,
            left_trigger: 0,
            right_trigger: 0,
        }
    }

    fn wiimote_gamepad(state: &ControllerState) -> XGamepad {
        let mut raw: u16 = 0;
        for (flag, xb) in [
            (Buttons::A, XButtons::A),
            (Buttons::B, XButtons::B),
            (Buttons::ONE, XButtons::X),
            (Buttons::TWO, XButtons::Y),
            (Buttons::PLUS, XButtons::START),
            (Buttons::MINUS, XButtons::BACK),
            (Buttons::HOME, XButtons::GUIDE),
            (Buttons::UP, XButtons::UP),
            (Buttons::DOWN, XButtons::DOWN),
            (Buttons::LEFT, XButtons::LEFT),
            (Buttons::RIGHT, XButtons::RIGHT),
        ] {
            if state.buttons.contains(flag) {
                raw |= xb;
            }
        }
        XGamepad {
            buttons: XButtons { raw },
            thumb_lx: tilt_to_stick(state.accel.x as i32),
            thumb_ly: tilt_to_stick(state.accel.y as i32),
            thumb_rx: 0,
            thumb_ry: 0,
            left_trigger: 0,
            right_trigger: 0,
        }
    }

    /// Xplorer X360 guitar layout — the one Clone Hero auto-recognises:
    /// frets onto face buttons, strum onto D-pad, whammy on RX.
    fn guitar_gamepad(g: &GuitarState, state: &ControllerState) -> XGamepad {
        let mut raw: u16 = 0;
        for (flag, xb) in [
            (GuitarButtons::GREEN, XButtons::A),
            (GuitarButtons::RED, XButtons::B),
            (GuitarButtons::YELLOW, XButtons::Y),
            (GuitarButtons::BLUE, XButtons::X),
            (GuitarButtons::ORANGE, XButtons::LB),
            (GuitarButtons::STRUM_UP, XButtons::UP),
            (GuitarButtons::STRUM_DOWN, XButtons::DOWN),
            (GuitarButtons::PLUS, XButtons::START),
            (GuitarButtons::MINUS, XButtons::BACK),
        ] {
            if g.buttons.contains(flag) {
                raw |= xb;
            }
        }
        // Wiimote's HOME button stays as the Xbox guide button so the
        // user can always escape into Steam/CH menus.
        if state.buttons.contains(Buttons::HOME) {
            raw |= XButtons::GUIDE;
        }

        // Whammy: 5-bit (0 = released, 31 = fully pressed) → RX axis
        // (-32768 = released, +32767 = fully pressed). This is what the
        // real Xplorer guitar reports.
        let thumb_rx = whammy_to_axis(g.whammy);

        XGamepad {
            buttons: XButtons { raw },
            thumb_lx: 0,
            thumb_ly: 0,
            thumb_rx,
            thumb_ry: 0,
            left_trigger: 0,
            right_trigger: 0,
        }
    }

    fn whammy_to_axis(w: u8) -> i16 {
        // 5-bit range 0..=31 maps to the full i16 axis range.
        // w.min(31) keeps the formula safe even if the parser ever lets
        // a stray bit through; without it the result is still in range
        // mathematically, the .min() just makes the bound explicit.
        let w = w.min(31) as i32;
        (w * 65535 / 31 - 32768) as i16
    }

    fn tilt_to_stick(raw_axis: i32) -> i16 {
        let delta = raw_axis - ACCEL_CENTER;
        if delta.abs() < ACCEL_DEADZONE {
            return 0;
        }
        let signed = if delta > 0 {
            delta - ACCEL_DEADZONE
        } else {
            delta + ACCEL_DEADZONE
        };
        let span = (ACCEL_RANGE - ACCEL_DEADZONE).max(1);
        let scaled = (signed * i16::MAX as i32) / span;
        scaled.clamp(i16::MIN as i32 + 1, i16::MAX as i32) as i16
    }
}
