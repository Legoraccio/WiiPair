//! Virtual controller backends.
//!
//! - **Windows**: ViGEmBus virtual Xbox 360 pad (requires the ViGEmBus
//!   driver from <https://github.com/nefarius/ViGEmBus/releases>).
//! - **Linux**: `uinput` virtual Xbox 360 device (`/dev/uinput`).
//! - **macOS**: CGEvent keyboard mapping fallback — modern macOS
//!   requires a signed DriverKit driver for a real virtual gamepad.

mod mapping;
mod profile;
pub mod xbox;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

pub use profile::{MappingProfile, PadLayout};
pub use xbox::{XboxState, map_to_xbox};

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
    use crate::xbox::{XboxState, map_to_xbox};
    use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};

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
            let xbox = map_to_xbox(self.profile, state);
            self.target
                .update(&to_xgamepad(&xbox))
                .map_err(|e| anyhow::anyhow!("vigem update: {e}"))?;
            Ok(())
        }
    }

    /// Translate the cross-platform [`XboxState`] into vigem-client's
    /// [`XGamepad`]. Pure data shuffling — all the actual mapping
    /// happens in `xbox::map_to_xbox`.
    fn to_xgamepad(s: &XboxState) -> XGamepad {
        let mut raw: u16 = 0;
        for (pressed, bit) in [
            (s.a, XButtons::A),
            (s.b, XButtons::B),
            (s.x, XButtons::X),
            (s.y, XButtons::Y),
            (s.lb, XButtons::LB),
            (s.rb, XButtons::RB),
            (s.start, XButtons::START),
            (s.back, XButtons::BACK),
            (s.guide, XButtons::GUIDE),
            (s.thumb_l, XButtons::LTHUMB),
            (s.thumb_r, XButtons::RTHUMB),
            (s.up, XButtons::UP),
            (s.down, XButtons::DOWN),
            (s.left, XButtons::LEFT),
            (s.right, XButtons::RIGHT),
        ] {
            if pressed {
                raw |= bit;
            }
        }
        XGamepad {
            buttons: XButtons { raw },
            thumb_lx: s.lx,
            thumb_ly: s.ly,
            thumb_rx: s.rx,
            thumb_ry: s.ry,
            left_trigger: s.lt,
            right_trigger: s.rt,
        }
    }
}
