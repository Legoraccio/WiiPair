//! Per-device row layout — header (name + buttons), live-state body
//! (Wiimote zone + extension zone), footer (battery + accel + IR).

use crossbeam_channel::Sender;
use eframe::egui;
use wiimote_core::{
    Buttons, ClassicButtons, ClassicState, DrumsButtons, DrumsState, ExtensionData, GuitarButtons,
    GuitarState, NunchukState,
};
use wiimote_daemon::{DeviceSnapshot, UiCommand};
use wiimote_output::MappingProfile;

use crate::icons::{draw_device_icon, extension_color};
use crate::widgets::{
    arrow_indicator, battery_widget, button_indicator, fret_indicator, ir_widget, pad_indicator,
    pm_indicator, strum_indicator, tilt_widget, whammy_bar, ArrowDir, BASS_COLOR, FRET_BLUE,
    FRET_GREEN, FRET_ORANGE, FRET_RED, FRET_YELLOW,
};

pub fn render_device(ui: &mut egui::Ui, d: &DeviceSnapshot, tx: &Sender<UiCommand>) {
    ui.group(|ui| {
        render_header(ui, d, tx);
        ui.separator();
        render_body(ui, d);
        render_footer(ui, d, tx);
    });
}

fn render_header(ui: &mut egui::Ui, d: &DeviceSnapshot, tx: &Sender<UiCommand>) {
    ui.horizontal(|ui| {
        let dot = if d.connected { "●" } else { "○" };
        let dot_color = if d.connected {
            egui::Color32::from_rgb(80, 200, 120)
        } else {
            egui::Color32::GRAY
        };
        ui.colored_label(dot_color, dot);
        draw_device_icon(ui, d);
        ui.strong(&d.name);
        if let Some(ext) = d.extension {
            ui.label(
                egui::RichText::new(format!("· {}", ext.label())).color(extension_color(ext)),
            );
        }
        // U10: clicking the id copies to clipboard for use with
        // `bluetoothctl` / Windows BT settings / etc.
        let id_label = ui.add(
            egui::Label::new(
                egui::RichText::new(short_id(&d.id, 24)).monospace().weak(),
            )
            .sense(egui::Sense::click()),
        );
        let id_label = id_label.on_hover_text("Click to copy id to clipboard");
        if id_label.clicked() {
            ui.output_mut(|o| o.copied_text = d.id.clone());
        }

        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if d.connected {
                    if ui.button("Disconnect").clicked() {
                        let _ = tx.send(UiCommand::Disconnect(d.id.clone()));
                    }
                } else if ui.button("Connect").clicked() {
                    let _ = tx.send(UiCommand::Connect(d.id.clone()));
                }
                let identify_btn =
                    ui.add_enabled(d.connected, egui::Button::new("Identify"));
                if identify_btn
                    .on_hover_text("Vibrate the controller for ~0.6 s to identify it physically")
                    .clicked()
                {
                    let _ = tx.send(UiCommand::Identify(d.id.clone()));
                }
                if ui
                    .button("Forget")
                    .on_hover_text(
                        "Disconnect, remove from the saved list and unpair from the OS Bluetooth registry.",
                    )
                    .clicked()
                {
                    let _ = tx.send(UiCommand::Forget(d.id.clone()));
                }
            },
        );
    });
}

fn render_body(ui: &mut egui::Ui, d: &DeviceSnapshot) {
    ui.horizontal(|ui| {
        // Zone 1 — primary device (Wii Remote).
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("Wii Remote").small().weak());
            render_wiimote_zone(ui, d);
        });

        ui.separator();

        // Zone 2 — connected extension (or "no extension").
        ui.vertical(|ui| match (&d.ext_data, d.extension) {
            (Some(ExtensionData::Guitar(g)), _) => {
                ui.label(egui::RichText::new("Guitar (GH/RB)").small().weak());
                render_guitar_zone(ui, g);
            }
            (Some(ExtensionData::Drums(dr)), _) => {
                ui.label(egui::RichText::new("Drums (GH/RB)").small().weak());
                render_drums_zone(ui, dr);
            }
            (Some(ExtensionData::Nunchuk(n)), _) => {
                ui.label(egui::RichText::new("Nunchuk").small().weak());
                render_nunchuk_zone(ui, n);
            }
            (Some(ExtensionData::Classic(c)), _) => {
                ui.label(egui::RichText::new("Classic Controller").small().weak());
                render_classic_zone(ui, c);
            }
            (Some(ExtensionData::Unparsed), Some(t)) => {
                ui.label(
                    egui::RichText::new(format!("{} — no parser yet", t.label()))
                        .small()
                        .weak(),
                );
            }
            (None, Some(t)) => {
                ui.label(
                    egui::RichText::new(format!("{} (offline)", t.label()))
                        .small()
                        .weak(),
                );
            }
            (None, None) | (_, None) => {
                ui.label(egui::RichText::new("No extension").small().weak());
            }
        });
    });
}

fn render_footer(ui: &mut egui::Ui, d: &DeviceSnapshot, tx: &crossbeam_channel::Sender<UiCommand>) {
    ui.separator();
    ui.horizontal(|ui| {
        battery_widget(ui, d.battery);
        ui.separator();
        ui.label(egui::RichText::new("Tilt").small().weak());
        tilt_widget(ui, d.last_accel);
        ui.separator();
        ui.label(egui::RichText::new("IR").small().weak());
        ir_widget(ui, d.last_ir);
        ui.separator();
        render_profile_selector(ui, d, tx);
    });
    if let Some(err) = &d.last_error {
        ui.colored_label(egui::Color32::LIGHT_RED, err);
    }
}

fn render_profile_selector(
    ui: &mut egui::Ui,
    d: &DeviceSnapshot,
    tx: &crossbeam_channel::Sender<UiCommand>,
) {
    ui.label(egui::RichText::new("Profile").small().weak());
    let mut chosen = d.mapping_profile;
    egui::ComboBox::from_id_salt(("profile", &d.id))
        .selected_text(chosen.label())
        .show_ui(ui, |ui| {
            for p in MappingProfile::all() {
                ui.selectable_value(&mut chosen, *p, p.label());
            }
        });
    if chosen != d.mapping_profile {
        let _ = tx.send(UiCommand::SetMappingProfile {
            id: d.id.clone(),
            profile: chosen,
        });
    }
}

// =====================================================================
// Wiimote / extension zones
// =====================================================================

fn render_wiimote_zone(ui: &mut egui::Ui, d: &DeviceSnapshot) {
    ui.horizontal(|ui| {
        // D-pad in physical 3-row arrangement.
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.add_space(30.0);
                arrow_indicator(ui, ArrowDir::Up, d.last_buttons.contains(Buttons::UP));
            });
            ui.horizontal(|ui| {
                arrow_indicator(ui, ArrowDir::Left, d.last_buttons.contains(Buttons::LEFT));
                ui.add_space(2.0);
                arrow_indicator(ui, ArrowDir::Right, d.last_buttons.contains(Buttons::RIGHT));
            });
            ui.horizontal(|ui| {
                ui.add_space(30.0);
                arrow_indicator(ui, ArrowDir::Down, d.last_buttons.contains(Buttons::DOWN));
            });
        });

        ui.add_space(6.0);

        // A button (large), B (trigger).
        ui.vertical(|ui| {
            button_indicator(
                ui,
                "A",
                d.last_buttons.contains(Buttons::A),
                egui::Color32::from_rgb(80, 220, 80),
            );
            button_indicator(
                ui,
                "B",
                d.last_buttons.contains(Buttons::B),
                egui::Color32::from_rgb(220, 90, 90),
            );
        });

        ui.add_space(6.0);

        // 1, 2.
        ui.vertical(|ui| {
            button_indicator(
                ui,
                "1",
                d.last_buttons.contains(Buttons::ONE),
                egui::Color32::LIGHT_GRAY,
            );
            button_indicator(
                ui,
                "2",
                d.last_buttons.contains(Buttons::TWO),
                egui::Color32::LIGHT_GRAY,
            );
        });

        ui.add_space(6.0);

        // Plus / Home / Minus column.
        ui.vertical(|ui| {
            button_indicator(
                ui,
                "+",
                d.last_buttons.contains(Buttons::PLUS),
                egui::Color32::LIGHT_GRAY,
            );
            button_indicator(
                ui,
                "Home",
                d.last_buttons.contains(Buttons::HOME),
                egui::Color32::LIGHT_BLUE,
            );
            button_indicator(
                ui,
                "−",
                d.last_buttons.contains(Buttons::MINUS),
                egui::Color32::LIGHT_GRAY,
            );
        });
    });
}

fn render_guitar_zone(ui: &mut egui::Ui, g: &GuitarState) {
    ui.horizontal(|ui| {
        for (flag, color, label) in [
            (GuitarButtons::GREEN, FRET_GREEN, "G"),
            (GuitarButtons::RED, FRET_RED, "R"),
            (GuitarButtons::YELLOW, FRET_YELLOW, "Y"),
            (GuitarButtons::BLUE, FRET_BLUE, "B"),
            (GuitarButtons::ORANGE, FRET_ORANGE, "O"),
        ] {
            fret_indicator(ui, color, g.buttons.contains(flag), label);
        }
        ui.add_space(8.0);
        strum_indicator(
            ui,
            g.buttons.contains(GuitarButtons::STRUM_UP),
            g.buttons.contains(GuitarButtons::STRUM_DOWN),
        );
        ui.add_space(8.0);
        whammy_bar(ui, g.whammy);
        ui.add_space(8.0);
        pm_indicator(
            ui,
            g.buttons.contains(GuitarButtons::PLUS),
            g.buttons.contains(GuitarButtons::MINUS),
        );
    });
}

fn render_drums_zone(ui: &mut egui::Ui, dr: &DrumsState) {
    ui.horizontal(|ui| {
        for (flag, color, label) in [
            (DrumsButtons::RED, FRET_RED, "R"),
            (DrumsButtons::YELLOW, FRET_YELLOW, "Y"),
            (DrumsButtons::BLUE, FRET_BLUE, "B"),
            (DrumsButtons::GREEN, FRET_GREEN, "G"),
            (DrumsButtons::ORANGE, FRET_ORANGE, "O"),
        ] {
            pad_indicator(ui, color, dr.buttons.contains(flag), label);
        }
        ui.add_space(8.0);
        pad_indicator(
            ui,
            BASS_COLOR,
            dr.buttons.contains(DrumsButtons::BASS_PEDAL),
            "Bass",
        );
        ui.add_space(12.0);
        pm_indicator(
            ui,
            dr.buttons.contains(DrumsButtons::PLUS),
            dr.buttons.contains(DrumsButtons::MINUS),
        );
    });
}

fn render_nunchuk_zone(ui: &mut egui::Ui, n: &NunchukState) {
    ui.horizontal(|ui| {
        button_indicator(ui, "C", n.c, egui::Color32::from_rgb(180, 220, 180));
        button_indicator(ui, "Z", n.z, egui::Color32::from_rgb(180, 220, 180));
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!(
                "stick: x={:>3}  y={:>3}",
                n.stick_x, n.stick_y
            ))
            .monospace(),
        );
    });
}

fn render_classic_zone(ui: &mut egui::Ui, c: &ClassicState) {
    ui.horizontal_wrapped(|ui| {
        for (flag, name, color) in [
            (ClassicButtons::A, "A", egui::Color32::from_rgb(80, 220, 80)),
            (ClassicButtons::B, "B", egui::Color32::from_rgb(220, 80, 80)),
            (ClassicButtons::X, "X", egui::Color32::from_rgb(80, 130, 220)),
            (ClassicButtons::Y, "Y", egui::Color32::from_rgb(220, 200, 80)),
            (ClassicButtons::ZL, "ZL", egui::Color32::LIGHT_GRAY),
            (ClassicButtons::ZR, "ZR", egui::Color32::LIGHT_GRAY),
            (ClassicButtons::LT, "L", egui::Color32::LIGHT_GRAY),
            (ClassicButtons::RT, "R", egui::Color32::LIGHT_GRAY),
            (ClassicButtons::PLUS, "+", egui::Color32::LIGHT_GRAY),
            (ClassicButtons::MINUS, "−", egui::Color32::LIGHT_GRAY),
            (ClassicButtons::HOME, "Home", egui::Color32::LIGHT_BLUE),
        ] {
            button_indicator(ui, name, c.buttons.contains(flag), color);
        }
        arrow_indicator(ui, ArrowDir::Up, c.buttons.contains(ClassicButtons::DPAD_UP));
        arrow_indicator(
            ui,
            ArrowDir::Down,
            c.buttons.contains(ClassicButtons::DPAD_DOWN),
        );
        arrow_indicator(
            ui,
            ArrowDir::Left,
            c.buttons.contains(ClassicButtons::DPAD_LEFT),
        );
        arrow_indicator(
            ui,
            ArrowDir::Right,
            c.buttons.contains(ClassicButtons::DPAD_RIGHT),
        );
    });
}

fn short_id(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let tail = &s[s.len() - max..];
        format!("…{tail}")
    }
}
