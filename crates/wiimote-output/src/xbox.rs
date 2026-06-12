//! Cross-platform Xbox 360 pad state — the single source of truth
//! for "what XInput inputs would the current Wiimote+extension state
//! produce?". The Windows ViGEm and Linux uinput backends both feed
//! their platform-native gamepad APIs from this struct, and the UI
//! uses it to render the live mapping preview.
//!
//! Centralising the mapping here eliminates the previous duplication
//! between `lib.rs` (Windows) and `linux.rs` (Linux) — a single
//! mapping change now lands in one place and is automatically
//! reflected in the UI.

use wiimote_core::{
    Buttons, ClassicButtons, ClassicState, DrumsButtons, DrumsState, ExtensionData, GuitarButtons,
    GuitarState, NunchukState,
};

use crate::mapping::{nunchuk_stick_to_axis, tilt_to_stick, whammy_to_axis};
use crate::profile::{MappingProfile, PadLayout};
use crate::ControllerState;

/// Logical Xbox 360 pad frame. Stick axes are `i16` so the value is
/// the same whether the consumer is XInput (which is `i16` natively)
/// or evdev (which widens to `i32` at emit time). Triggers are `u8`
/// (XInput convention).
#[derive(Debug, Clone, Copy, Default)]
pub struct XboxState {
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    pub lb: bool,
    pub rb: bool,
    pub start: bool,
    pub back: bool,
    pub guide: bool,
    pub thumb_l: bool,
    pub thumb_r: bool,
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub lt: u8,
    pub rt: u8,
}

/// Compute the XInput-equivalent state for `state` under `profile`.
/// Returns the "no input" frame when the profile resolves to a layout
/// whose extension isn't actually plugged in.
#[must_use]
pub fn map_to_xbox(profile: MappingProfile, state: &ControllerState) -> XboxState {
    match profile.resolve_pad(state.ext.as_ref()) {
        PadLayout::Wiimote => wiimote(state),
        PadLayout::Guitar => match &state.ext {
            Some(ExtensionData::Guitar(g)) => guitar(g, state),
            _ => wiimote(state),
        },
        PadLayout::Drums => match &state.ext {
            Some(ExtensionData::Drums(d)) => drums(d, state),
            _ => wiimote(state),
        },
        PadLayout::Classic => match &state.ext {
            Some(ExtensionData::Classic(c)) => classic(c, state),
            _ => wiimote(state),
        },
    }
}

fn wiimote(state: &ControllerState) -> XboxState {
    let b = state.buttons;
    let tilt_x = tilt_to_stick(i32::from(state.accel.x));
    let tilt_y = tilt_to_stick(i32::from(state.accel.y));
    let mut s = XboxState {
        a: b.contains(Buttons::A),
        b: b.contains(Buttons::B),
        x: b.contains(Buttons::ONE),
        y: b.contains(Buttons::TWO),
        start: b.contains(Buttons::PLUS),
        back: b.contains(Buttons::MINUS),
        guide: b.contains(Buttons::HOME),
        up: b.contains(Buttons::UP),
        down: b.contains(Buttons::DOWN),
        left: b.contains(Buttons::LEFT),
        right: b.contains(Buttons::RIGHT),
        lx: tilt_x,
        ly: tilt_y,
        ..Default::default()
    };
    // Nunchuk plugged in: its stick takes the left thumbstick (the
    // canonical Wii game layout — Mario Kart Wii, RE4 Wii, Skyward
    // Sword on Dolphin) and Wiimote tilt moves to the right
    // thumbstick. C → LB, Z → LT (full deflection).
    if let Some(ExtensionData::Nunchuk(n)) = &state.ext {
        apply_nunchuk(&mut s, n, tilt_x, tilt_y);
    }
    s
}

fn apply_nunchuk(s: &mut XboxState, n: &NunchukState, tilt_x: i16, tilt_y: i16) {
    s.lx = nunchuk_stick_to_axis(n.stick_x);
    s.ly = nunchuk_stick_to_axis(n.stick_y);
    s.rx = tilt_x;
    s.ry = tilt_y;
    if n.c {
        s.lb = true;
    }
    if n.z {
        s.lt = 255;
    }
}

fn guitar(g: &GuitarState, state: &ControllerState) -> XboxState {
    XboxState {
        a: g.buttons.contains(GuitarButtons::GREEN),
        b: g.buttons.contains(GuitarButtons::RED),
        y: g.buttons.contains(GuitarButtons::YELLOW),
        x: g.buttons.contains(GuitarButtons::BLUE),
        lb: g.buttons.contains(GuitarButtons::ORANGE),
        up: g.buttons.contains(GuitarButtons::STRUM_UP),
        down: g.buttons.contains(GuitarButtons::STRUM_DOWN),
        start: g.buttons.contains(GuitarButtons::PLUS),
        back: g.buttons.contains(GuitarButtons::MINUS),
        // Wiimote HOME stays as the Guide button so the user can
        // always escape into Steam / Clone Hero menus.
        guide: state.buttons.contains(Buttons::HOME),
        rx: whammy_to_axis(g.whammy),
        ..Default::default()
    }
}

fn drums(d: &DrumsState, state: &ControllerState) -> XboxState {
    XboxState {
        a: d.buttons.contains(DrumsButtons::GREEN),
        b: d.buttons.contains(DrumsButtons::RED),
        x: d.buttons.contains(DrumsButtons::BLUE),
        y: d.buttons.contains(DrumsButtons::YELLOW),
        lb: d.buttons.contains(DrumsButtons::ORANGE),
        rb: d.buttons.contains(DrumsButtons::BASS_PEDAL),
        start: d.buttons.contains(DrumsButtons::PLUS),
        back: d.buttons.contains(DrumsButtons::MINUS),
        guide: state.buttons.contains(Buttons::HOME),
        ..Default::default()
    }
}

fn classic(c: &ClassicState, _state: &ControllerState) -> XboxState {
    XboxState {
        a: c.buttons.contains(ClassicButtons::A),
        b: c.buttons.contains(ClassicButtons::B),
        x: c.buttons.contains(ClassicButtons::X),
        y: c.buttons.contains(ClassicButtons::Y),
        lb: c.buttons.contains(ClassicButtons::ZL),
        rb: c.buttons.contains(ClassicButtons::ZR),
        // Classic Controller has digital L/R triggers; map to full
        // analog deflection so games that read either path see them.
        lt: if c.buttons.contains(ClassicButtons::LT) { 255 } else { 0 },
        rt: if c.buttons.contains(ClassicButtons::RT) { 255 } else { 0 },
        start: c.buttons.contains(ClassicButtons::PLUS),
        back: c.buttons.contains(ClassicButtons::MINUS),
        guide: c.buttons.contains(ClassicButtons::HOME),
        up: c.buttons.contains(ClassicButtons::DPAD_UP),
        down: c.buttons.contains(ClassicButtons::DPAD_DOWN),
        left: c.buttons.contains(ClassicButtons::DPAD_LEFT),
        right: c.buttons.contains(ClassicButtons::DPAD_RIGHT),
        ..Default::default()
    }
}
