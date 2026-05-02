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
use crate::{ControllerState, Output};
use wiimote_core::{
    Buttons, ClassicButtons, ClassicState, DrumsButtons, DrumsState, ExtensionData, GuitarButtons,
    GuitarState,
};

/// Microsoft Xbox 360 wired controller — the device games look for.
const XBOX360_VID: u16 = 0x045E;
const XBOX360_PID: u16 = 0x028E;

const ABS_RANGE: i32 = 32767;
const TRIGGER_MAX: i32 = 255;
const ACCEL_CENTER: i32 = 512;
const ACCEL_DEADZONE: i32 = 30;
const ACCEL_RANGE: i32 = 220;

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
        let frame = build_frame(self.profile, state);
        let events = frame.to_events();
        self.device.emit(&events)?;
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

// =====================================================================
// Logical frame: the stable XInput-style state we then turn into evdev
// events. Splitting "compute mapping" from "emit events" keeps the
// per-profile logic straightforward.
// =====================================================================

#[derive(Default)]
struct Frame {
    a: bool,
    b: bool,
    x: bool,
    y: bool,
    lb: bool,
    rb: bool,
    select: bool,
    start: bool,
    guide: bool,
    dup: bool,
    ddown: bool,
    dleft: bool,
    dright: bool,
    lx: i32,
    ly: i32,
    rx: i32,
    ry: i32,
    lt: i32,
    rt: i32,
}

impl Frame {
    fn to_events(&self) -> Vec<InputEvent> {
        let mut ev = Vec::with_capacity(16);
        let mut k = |key: Key, pressed: bool| {
            ev.push(InputEvent::new(EventType::KEY, key.code(), pressed as i32))
        };
        k(Key::BTN_SOUTH, self.a);
        k(Key::BTN_EAST, self.b);
        k(Key::BTN_WEST, self.x);
        k(Key::BTN_NORTH, self.y);
        k(Key::BTN_TL, self.lb);
        k(Key::BTN_TR, self.rb);
        k(Key::BTN_SELECT, self.select);
        k(Key::BTN_START, self.start);
        k(Key::BTN_MODE, self.guide);
        k(Key::BTN_DPAD_UP, self.dup);
        k(Key::BTN_DPAD_DOWN, self.ddown);
        k(Key::BTN_DPAD_LEFT, self.dleft);
        k(Key::BTN_DPAD_RIGHT, self.dright);
        for (axis, val) in [
            (AbsoluteAxisType::ABS_X, self.lx),
            (AbsoluteAxisType::ABS_Y, self.ly),
            (AbsoluteAxisType::ABS_RX, self.rx),
            (AbsoluteAxisType::ABS_RY, self.ry),
            (AbsoluteAxisType::ABS_Z, self.lt),
            (AbsoluteAxisType::ABS_RZ, self.rt),
        ] {
            ev.push(InputEvent::new(EventType::ABSOLUTE, axis.0, val));
        }
        ev
    }
}

fn build_frame(profile: MappingProfile, state: &ControllerState) -> Frame {
    let pick_layout = || -> MappingProfile {
        match profile {
            MappingProfile::Auto => match &state.ext {
                Some(ExtensionData::Guitar(_)) => MappingProfile::GuitarXplorer,
                Some(ExtensionData::Drums(_)) => MappingProfile::DrumsXplorer,
                Some(ExtensionData::Classic(_)) => MappingProfile::ClassicXbox,
                _ => MappingProfile::WiimoteXbox,
            },
            // Keyboard profiles aren't applicable on Linux — fall back
            // to the gamepad mapping that best matches the extension.
            MappingProfile::WiimoteKeyboard => MappingProfile::WiimoteXbox,
            MappingProfile::GuitarKeyboard => MappingProfile::GuitarXplorer,
            other => other,
        }
    };
    match pick_layout() {
        MappingProfile::WiimoteXbox => wiimote_frame(state),
        MappingProfile::GuitarXplorer => match &state.ext {
            Some(ExtensionData::Guitar(g)) => guitar_frame(g, state),
            _ => wiimote_frame(state),
        },
        MappingProfile::DrumsXplorer => match &state.ext {
            Some(ExtensionData::Drums(d)) => drums_frame(d, state),
            _ => wiimote_frame(state),
        },
        MappingProfile::ClassicXbox => match &state.ext {
            Some(ExtensionData::Classic(c)) => classic_frame(c, state),
            _ => wiimote_frame(state),
        },
        // Should be unreachable after pick_layout.
        _ => wiimote_frame(state),
    }
}

fn wiimote_frame(state: &ControllerState) -> Frame {
    let b = state.buttons;
    Frame {
        a: b.contains(Buttons::A),
        b: b.contains(Buttons::B),
        x: b.contains(Buttons::ONE),
        y: b.contains(Buttons::TWO),
        start: b.contains(Buttons::PLUS),
        select: b.contains(Buttons::MINUS),
        guide: b.contains(Buttons::HOME),
        dup: b.contains(Buttons::UP),
        ddown: b.contains(Buttons::DOWN),
        dleft: b.contains(Buttons::LEFT),
        dright: b.contains(Buttons::RIGHT),
        lx: tilt_to_axis(state.accel.x as i32),
        ly: tilt_to_axis(state.accel.y as i32),
        ..Default::default()
    }
}

fn guitar_frame(g: &GuitarState, state: &ControllerState) -> Frame {
    Frame {
        a: g.buttons.contains(GuitarButtons::GREEN),
        b: g.buttons.contains(GuitarButtons::RED),
        y: g.buttons.contains(GuitarButtons::YELLOW),
        x: g.buttons.contains(GuitarButtons::BLUE),
        lb: g.buttons.contains(GuitarButtons::ORANGE),
        dup: g.buttons.contains(GuitarButtons::STRUM_UP),
        ddown: g.buttons.contains(GuitarButtons::STRUM_DOWN),
        start: g.buttons.contains(GuitarButtons::PLUS),
        select: g.buttons.contains(GuitarButtons::MINUS),
        guide: state.buttons.contains(Buttons::HOME),
        rx: whammy_to_axis(g.whammy),
        ..Default::default()
    }
}

fn drums_frame(d: &DrumsState, state: &ControllerState) -> Frame {
    Frame {
        a: d.buttons.contains(DrumsButtons::GREEN),
        b: d.buttons.contains(DrumsButtons::RED),
        x: d.buttons.contains(DrumsButtons::BLUE),
        y: d.buttons.contains(DrumsButtons::YELLOW),
        lb: d.buttons.contains(DrumsButtons::ORANGE),
        rb: d.buttons.contains(DrumsButtons::BASS_PEDAL),
        start: d.buttons.contains(DrumsButtons::PLUS),
        select: d.buttons.contains(DrumsButtons::MINUS),
        guide: state.buttons.contains(Buttons::HOME),
        ..Default::default()
    }
}

fn classic_frame(c: &ClassicState, _state: &ControllerState) -> Frame {
    Frame {
        a: c.buttons.contains(ClassicButtons::A),
        b: c.buttons.contains(ClassicButtons::B),
        x: c.buttons.contains(ClassicButtons::X),
        y: c.buttons.contains(ClassicButtons::Y),
        lb: c.buttons.contains(ClassicButtons::ZL),
        rb: c.buttons.contains(ClassicButtons::ZR),
        lt: if c.buttons.contains(ClassicButtons::LT) {
            TRIGGER_MAX
        } else {
            0
        },
        rt: if c.buttons.contains(ClassicButtons::RT) {
            TRIGGER_MAX
        } else {
            0
        },
        start: c.buttons.contains(ClassicButtons::PLUS),
        select: c.buttons.contains(ClassicButtons::MINUS),
        guide: c.buttons.contains(ClassicButtons::HOME),
        dup: c.buttons.contains(ClassicButtons::DPAD_UP),
        ddown: c.buttons.contains(ClassicButtons::DPAD_DOWN),
        dleft: c.buttons.contains(ClassicButtons::DPAD_LEFT),
        dright: c.buttons.contains(ClassicButtons::DPAD_RIGHT),
        ..Default::default()
    }
}

fn whammy_to_axis(w: u8) -> i32 {
    let w = w.min(31) as i32;
    w * (2 * ABS_RANGE) / 31 - ABS_RANGE
}

fn tilt_to_axis(raw: i32) -> i32 {
    let delta = raw - ACCEL_CENTER;
    if delta.abs() < ACCEL_DEADZONE {
        return 0;
    }
    let signed = if delta > 0 {
        delta - ACCEL_DEADZONE
    } else {
        delta + ACCEL_DEADZONE
    };
    let span = (ACCEL_RANGE - ACCEL_DEADZONE).max(1);
    (signed * ABS_RANGE / span).clamp(-ABS_RANGE, ABS_RANGE)
}
