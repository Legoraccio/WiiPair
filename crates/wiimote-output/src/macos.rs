//! macOS CGEvent keyboard-mapping fallback.
//!
//! Modern macOS requires a signed DriverKit driver to publish a real
//! virtual gamepad — not realistic for an open-source project. Instead
//! we synthesise keyboard events via Quartz CGEvent for the
//! `*Keyboard` mapping profiles. Pad-mapping profiles (`*Xbox`,
//! `*Xplorer`) error out at construction with a clear message.
//!
//! The default `Auto` profile picks `WiimoteKeyboard` so the user gets
//! a working setup out of the box.

use std::collections::HashSet;

use core_foundation::base::TCFType;
use core_graphics::event::{CGEvent, CGEventTapLocation, KeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use crate::profile::MappingProfile;
use crate::{ControllerState, Output};
use wiimote_core::{
    Buttons, ClassicButtons, ClassicState, ExtensionData, GuitarButtons, GuitarState,
};

/// Minimal subset of HID usage codes we emit. Picked to match common
/// Clone-Hero / browser-game keymaps; the user can rebind in-game.
mod keys {
    use core_graphics::event::KeyCode;
    pub const ARROW_UP: KeyCode = 0x7E;
    pub const ARROW_DOWN: KeyCode = 0x7D;
    pub const ARROW_LEFT: KeyCode = 0x7B;
    pub const ARROW_RIGHT: KeyCode = 0x7C;
    pub const KEY_A: KeyCode = 0x00;
    pub const KEY_B: KeyCode = 0x0B;
    pub const KEY_C: KeyCode = 0x08;
    pub const KEY_D: KeyCode = 0x02;
    pub const KEY_E: KeyCode = 0x0E;
    pub const KEY_F: KeyCode = 0x03;
    pub const KEY_G: KeyCode = 0x05;
    pub const KEY_H: KeyCode = 0x04;
    pub const KEY_J: KeyCode = 0x26;
    pub const KEY_K: KeyCode = 0x28;
    pub const KEY_L: KeyCode = 0x25;
    pub const KEY_Q: KeyCode = 0x0C;
    pub const KEY_R: KeyCode = 0x0F;
    pub const KEY_S: KeyCode = 0x01;
    pub const KEY_W: KeyCode = 0x0D;
    pub const KEY_X: KeyCode = 0x07;
    pub const KEY_Z: KeyCode = 0x06;
    pub const RETURN: KeyCode = 0x24;
    pub const SPACE: KeyCode = 0x31;
    pub const ESCAPE: KeyCode = 0x35;
}

pub struct CGEventOutput {
    profile: MappingProfile,
    pressed: HashSet<KeyCode>,
    source: CGEventSource,
}

impl CGEventOutput {
    pub fn new(profile: MappingProfile) -> anyhow::Result<Self> {
        let actual = match profile {
            MappingProfile::Auto => MappingProfile::WiimoteKeyboard,
            // Pad-mapping profiles aren't available on macOS — fail
            // loudly so the user picks a keyboard mapping instead.
            MappingProfile::WiimoteXbox
            | MappingProfile::GuitarXplorer
            | MappingProfile::DrumsXplorer
            | MappingProfile::ClassicXbox => {
                anyhow::bail!(
                    "macOS does not support virtual XInput gamepads without a signed DriverKit driver. \
                     Switch to a keyboard mapping profile in the device card."
                )
            }
            other => other,
        };
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow::anyhow!("CGEventSource::new failed"))?;
        Ok(Self {
            profile: actual,
            pressed: HashSet::new(),
            source,
        })
    }

    fn emit(&self, code: KeyCode, down: bool) {
        if let Ok(ev) = CGEvent::new_keyboard_event(self.source.clone(), code, down) {
            ev.post(CGEventTapLocation::HID);
        }
    }

    fn apply(&mut self, target: HashSet<KeyCode>) {
        // Edge-trigger: send keydown for newly-pressed keys, keyup for
        // ones that just released. Sending both endlessly would flood
        // the system event queue.
        for k in target.difference(&self.pressed) {
            self.emit(*k, true);
        }
        for k in self.pressed.difference(&target) {
            self.emit(*k, false);
        }
        self.pressed = target;
    }
}

impl Drop for CGEventOutput {
    fn drop(&mut self) {
        // Release any keys still held when the output is torn down so
        // the user doesn't end up with a stuck keyboard.
        for k in self.pressed.iter() {
            self.emit(*k, false);
        }
    }
}

impl Output for CGEventOutput {
    fn update(&mut self, state: &ControllerState) -> anyhow::Result<()> {
        let target = match self.profile {
            MappingProfile::WiimoteKeyboard => wiimote_keys(state),
            MappingProfile::GuitarKeyboard => match &state.ext {
                Some(ExtensionData::Guitar(g)) => guitar_keys(g, state),
                _ => wiimote_keys(state),
            },
            // Should be unreachable thanks to the constructor check.
            _ => wiimote_keys(state),
        };
        self.apply(target);
        Ok(())
    }
}

fn wiimote_keys(state: &ControllerState) -> HashSet<KeyCode> {
    let mut s = HashSet::new();
    let b = state.buttons;
    if b.contains(Buttons::UP) {
        s.insert(keys::ARROW_UP);
    }
    if b.contains(Buttons::DOWN) {
        s.insert(keys::ARROW_DOWN);
    }
    if b.contains(Buttons::LEFT) {
        s.insert(keys::ARROW_LEFT);
    }
    if b.contains(Buttons::RIGHT) {
        s.insert(keys::ARROW_RIGHT);
    }
    if b.contains(Buttons::A) {
        s.insert(keys::KEY_Z);
    }
    if b.contains(Buttons::B) {
        s.insert(keys::KEY_X);
    }
    if b.contains(Buttons::ONE) {
        s.insert(keys::KEY_Q);
    }
    if b.contains(Buttons::TWO) {
        s.insert(keys::KEY_W);
    }
    if b.contains(Buttons::PLUS) {
        s.insert(keys::RETURN);
    }
    if b.contains(Buttons::MINUS) {
        s.insert(keys::ESCAPE);
    }
    if b.contains(Buttons::HOME) {
        s.insert(keys::SPACE);
    }
    // Nunchuk surfaces C/Z too.
    if let Some(ExtensionData::Nunchuk(n)) = &state.ext {
        if n.c {
            s.insert(keys::KEY_C);
        }
        if n.z {
            s.insert(keys::KEY_S);
        }
    }
    if let Some(ExtensionData::Classic(c)) = &state.ext {
        return classic_keys(c);
    }
    s
}

fn guitar_keys(g: &GuitarState, state: &ControllerState) -> HashSet<KeyCode> {
    // Clone Hero default layout: F1..F5 frets, Up/Down strum, Enter
    // start, Esc back. We can't emit F-keys without the Carbon F-key
    // codes; substitute A/S/D/F/G frets — same logical layout, can
    // be remapped in CH.
    let mut s = HashSet::new();
    if g.buttons.contains(GuitarButtons::GREEN) {
        s.insert(keys::KEY_A);
    }
    if g.buttons.contains(GuitarButtons::RED) {
        s.insert(keys::KEY_S);
    }
    if g.buttons.contains(GuitarButtons::YELLOW) {
        s.insert(keys::KEY_D);
    }
    if g.buttons.contains(GuitarButtons::BLUE) {
        s.insert(keys::KEY_F);
    }
    if g.buttons.contains(GuitarButtons::ORANGE) {
        s.insert(keys::KEY_G);
    }
    if g.buttons.contains(GuitarButtons::STRUM_UP) {
        s.insert(keys::ARROW_UP);
    }
    if g.buttons.contains(GuitarButtons::STRUM_DOWN) {
        s.insert(keys::ARROW_DOWN);
    }
    if g.buttons.contains(GuitarButtons::PLUS) {
        s.insert(keys::RETURN);
    }
    if g.buttons.contains(GuitarButtons::MINUS) {
        s.insert(keys::ESCAPE);
    }
    if state.buttons.contains(Buttons::HOME) {
        s.insert(keys::SPACE);
    }
    s
}

fn classic_keys(c: &ClassicState) -> HashSet<KeyCode> {
    let mut s = HashSet::new();
    if c.buttons.contains(ClassicButtons::A) {
        s.insert(keys::KEY_Z);
    }
    if c.buttons.contains(ClassicButtons::B) {
        s.insert(keys::KEY_X);
    }
    if c.buttons.contains(ClassicButtons::X) {
        s.insert(keys::KEY_E);
    }
    if c.buttons.contains(ClassicButtons::Y) {
        s.insert(keys::KEY_R);
    }
    if c.buttons.contains(ClassicButtons::ZL) {
        s.insert(keys::KEY_J);
    }
    if c.buttons.contains(ClassicButtons::ZR) {
        s.insert(keys::KEY_K);
    }
    if c.buttons.contains(ClassicButtons::LT) {
        s.insert(keys::KEY_H);
    }
    if c.buttons.contains(ClassicButtons::RT) {
        s.insert(keys::KEY_L);
    }
    if c.buttons.contains(ClassicButtons::PLUS) {
        s.insert(keys::RETURN);
    }
    if c.buttons.contains(ClassicButtons::MINUS) {
        s.insert(keys::ESCAPE);
    }
    if c.buttons.contains(ClassicButtons::HOME) {
        s.insert(keys::SPACE);
    }
    if c.buttons.contains(ClassicButtons::DPAD_UP) {
        s.insert(keys::ARROW_UP);
    }
    if c.buttons.contains(ClassicButtons::DPAD_DOWN) {
        s.insert(keys::ARROW_DOWN);
    }
    if c.buttons.contains(ClassicButtons::DPAD_LEFT) {
        s.insert(keys::ARROW_LEFT);
    }
    if c.buttons.contains(ClassicButtons::DPAD_RIGHT) {
        s.insert(keys::ARROW_RIGHT);
    }
    s
}
