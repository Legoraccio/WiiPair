//! Virtual controller backends.
//!
//! - **Windows**: ViGEmBus virtual Xbox 360 pad (requires the ViGEmBus
//!   driver from <https://github.com/nefarius/ViGEmBus/releases>).
//! - **Linux/macOS**: not yet implemented (Linux: uinput; macOS: CGEvent
//!   keyboard mapping).

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

pub fn default_output() -> anyhow::Result<Box<dyn Output>> {
    #[cfg(windows)]
    {
        Ok(Box::new(windows::ViGEmOutput::new()?))
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("output backend not yet implemented on this platform")
    }
}

#[cfg(windows)]
pub mod windows {
    use super::{ControllerState, Output};
    use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};
    use wiimote_core::{Buttons, ExtensionData, GuitarButtons, GuitarState};

    /// Wiimote accelerometer is centred near 512 on X/Y when held flat
    /// (Z offset by gravity, ~612). Within ±DEADZONE of 512 we treat
    /// the stick as neutral.
    const ACCEL_CENTER: i32 = 512;
    const ACCEL_DEADZONE: i32 = 30;
    /// Approximate deflection at 45° of tilt — full stick at that angle.
    const ACCEL_RANGE: i32 = 220;

    pub struct ViGEmOutput {
        target: Xbox360Wired<Client>,
    }

    impl ViGEmOutput {
        pub fn new() -> anyhow::Result<Self> {
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
            Ok(Self { target })
        }
    }

    impl Output for ViGEmOutput {
        fn update(&mut self, state: &ControllerState) -> anyhow::Result<()> {
            // Pick the right mapping based on what's plugged into the
            // Wiimote. Guitar (GH/RB) gets the Xplorer-style layout
            // that Clone Hero understands out of the box; everything
            // else falls back to the bare-Wiimote tilt-as-stick layout.
            let gamepad = match &state.ext {
                Some(ExtensionData::Guitar(g)) => guitar_gamepad(g, state),
                _ => wiimote_gamepad(state),
            };
            self.target
                .update(&gamepad)
                .map_err(|e| anyhow::anyhow!("vigem update: {e}"))?;
            Ok(())
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
        let raw = (w as i32) * 65535 / 31 - 32768;
        raw.clamp(i16::MIN as i32, i16::MAX as i32) as i16
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
