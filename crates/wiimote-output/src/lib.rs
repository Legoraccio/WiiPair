//! Virtual controller backends.
//!
//! - **Windows**: ViGEmBus virtual Xbox 360 pad (requires the ViGEmBus
//!   driver from <https://github.com/nefarius/ViGEmBus/releases>).
//! - **Linux/macOS**: not yet implemented (Linux: uinput; macOS: CGEvent
//!   keyboard mapping).

use wiimote_core::{Accelerometer, Buttons, IrDots};

#[derive(Debug, Clone, Copy, Default)]
pub struct ControllerState {
    pub buttons: Buttons,
    pub accel: Accelerometer,
    pub ir: IrDots,
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
    use wiimote_core::Buttons;

    /// The Wiimote accelerometer is centred near 512 on each axis when held
    /// flat (Z is offset by gravity, ~612). We treat anything within
    /// ±DEADZONE of 512 on X/Y as "neutral" stick.
    const ACCEL_CENTER: i32 = 512;
    const ACCEL_DEADZONE: i32 = 30;
    /// Maximum absolute deflection we expect on tilt mapping; chosen so that
    /// roughly 45° of tilt maps to a fully-deflected stick.
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

            let gamepad = XGamepad {
                buttons: XButtons { raw },
                thumb_lx: tilt_to_stick(state.accel.x as i32),
                thumb_ly: tilt_to_stick(state.accel.y as i32),
                thumb_rx: 0,
                thumb_ry: 0,
                left_trigger: 0,
                right_trigger: 0,
            };
            self.target
                .update(&gamepad)
                .map_err(|e| anyhow::anyhow!("vigem update: {e}"))?;
            Ok(())
        }
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
