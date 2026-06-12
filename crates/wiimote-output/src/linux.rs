//! Linux uinput virtual gamepad backend.
//!
//! Publishes a `/dev/uinput` device that masquerades as a wired Xbox
//! 360 controller (USB vid:045E pid:028E) so SDL, Wine, and the kernel
//! `xpad` joystick layer all pick it up automatically. The vid/pid is
//! the same one Microsoft's driver advertises — every game with X360
//! support already has an entry for it in `gamecontrollerdb`.
//!
//! Keyboard mapping profiles (`WiimoteKeyboard`, `GuitarKeyboard`)
//! aren't supported here; on Linux a virtual gamepad is so easy to
//! produce that emulating keyboard events serves no purpose.
//!
//! Permissions: `/dev/uinput` requires write access. Either run as
//! root (not recommended) or install a udev rule giving the
//! `input` / `plugdev` group write access — see `docs/udev/`.

use std::os::unix::fs::PermissionsExt;

use evdev::{
    AbsInfo, AbsoluteAxisType, AttributeSet, BusType, EventType, InputEvent, InputId, Key,
    UinputAbsSetup,
};

use crate::profile::MappingProfile;
use crate::xbox::{XboxState, map_to_xbox};
use crate::{ControllerState, Output};

/// Microsoft Xbox 360 wired controller — the device games look for.
const XBOX360_VID: u16 = 0x045E;
const XBOX360_PID: u16 = 0x028E;

const ABS_RANGE: i32 = 32767;
const TRIGGER_MAX: i32 = 255;

pub struct UinputOutput {
    device: evdev::uinput::VirtualDevice,
    profile: MappingProfile,
}

impl UinputOutput {
    pub fn new(profile: MappingProfile) -> anyhow::Result<Self> {
        // Pre-flight: surface a clean message when /dev/uinput isn't
        // writable instead of failing inside evdev's open call.
        check_uinput_writable().map_err(|e| {
            anyhow::anyhow!(
                "/dev/uinput not writable: {e}. Add a udev rule giving \
                 your user group write access (see docs/udev/) or run \
                 with permission to write to /dev/uinput."
            )
        })?;

        let mut keys = AttributeSet::<Key>::new();
        for k in &[
            Key::BTN_SOUTH, // A
            Key::BTN_EAST,  // B
            Key::BTN_NORTH, // Y
            Key::BTN_WEST,  // X
            Key::BTN_TL,    // LB
            Key::BTN_TR,    // RB
            Key::BTN_SELECT,
            Key::BTN_START,
            Key::BTN_MODE, // Guide
            Key::BTN_THUMBL,
            Key::BTN_THUMBR,
            Key::BTN_DPAD_UP,
            Key::BTN_DPAD_DOWN,
            Key::BTN_DPAD_LEFT,
            Key::BTN_DPAD_RIGHT,
        ] {
            keys.insert(*k);
        }

        let stick = AbsInfo::new(0, -ABS_RANGE, ABS_RANGE, 16, 128, 1);
        let trigger = AbsInfo::new(0, 0, TRIGGER_MAX, 0, 0, 1);

        let device = evdev::uinput::VirtualDeviceBuilder::new()?
            .name("WiiPair Xbox 360 Controller")
            .input_id(InputId::new(BusType::BUS_USB, XBOX360_VID, XBOX360_PID, 0x0114))
            .with_keys(&keys)?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisType::ABS_X, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisType::ABS_Y, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisType::ABS_RX, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisType::ABS_RY, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisType::ABS_Z, trigger))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisType::ABS_RZ, trigger))?
            .build()?;

        Ok(Self { device, profile })
    }
}

impl Output for UinputOutput {
    fn update(&mut self, state: &ControllerState) -> anyhow::Result<()> {
        let xbox = map_to_xbox(self.profile, state);
        self.device.emit(&xbox_to_events(&xbox))?;
        Ok(())
    }
}

fn check_uinput_writable() -> Result<(), String> {
    let path = std::path::Path::new("/dev/uinput");
    let meta = std::fs::metadata(path).map_err(|e| format!("stat: {e}"))?;
    let mode = meta.permissions().mode();
    if mode & 0o002 == 0 && mode & 0o020 == 0 {
        // Not world- or group-writable. Try opening to check.
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map(|_| ())
            .map_err(|e| format!("open: {e}"))
    } else {
        Ok(())
    }
}

/// Translate the cross-platform [`XboxState`] into the evdev event
/// stream uinput consumes. Pure data shuffling — all the actual
/// mapping logic is in `xbox::map_to_xbox`.
fn xbox_to_events(s: &XboxState) -> Vec<InputEvent> {
    let mut ev = Vec::with_capacity(22);
    let mut k = |key: Key, pressed: bool| {
        ev.push(InputEvent::new(EventType::KEY, key.code(), pressed as i32))
    };
    k(Key::BTN_SOUTH, s.a);
    k(Key::BTN_EAST, s.b);
    k(Key::BTN_WEST, s.x);
    k(Key::BTN_NORTH, s.y);
    k(Key::BTN_TL, s.lb);
    k(Key::BTN_TR, s.rb);
    k(Key::BTN_SELECT, s.back);
    k(Key::BTN_START, s.start);
    k(Key::BTN_MODE, s.guide);
    k(Key::BTN_THUMBL, s.thumb_l);
    k(Key::BTN_THUMBR, s.thumb_r);
    k(Key::BTN_DPAD_UP, s.up);
    k(Key::BTN_DPAD_DOWN, s.down);
    k(Key::BTN_DPAD_LEFT, s.left);
    k(Key::BTN_DPAD_RIGHT, s.right);
    for (axis, val) in [
        (AbsoluteAxisType::ABS_X, i32::from(s.lx)),
        (AbsoluteAxisType::ABS_Y, i32::from(s.ly)),
        (AbsoluteAxisType::ABS_RX, i32::from(s.rx)),
        (AbsoluteAxisType::ABS_RY, i32::from(s.ry)),
        (AbsoluteAxisType::ABS_Z, i32::from(s.lt)),
        (AbsoluteAxisType::ABS_RZ, i32::from(s.rt)),
    ] {
        ev.push(InputEvent::new(EventType::ABSOLUTE, axis.0, val));
    }
    ev
}
