//! Per-device row layout — header (name + buttons), live-state body
//! (Wiimote zone + extension zone + Xbox 360 preview), footer
//! (battery + accel + IR + profile).

use crossbeam_channel::Sender;
use eframe::egui;
use wiimote_core::ExtensionData;
use wiimote_daemon::{DeviceSnapshot, UiCommand};
use wiimote_output::{ControllerState, MappingProfile, map_to_xbox};

use crate::device_widgets;
use crate::icons::{draw_device_icon, extension_color};
use crate::widgets::{battery_widget, ir_widget, tilt_widget};

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
                // Right-to-left order: Forget (danger) ← separator ←
                // Identify ← primary (Connect/Disconnect). Reading
                // left-to-right on screen this becomes
                // primary | Identify | Forget — primary action first,
                // destructive last.
                let danger = egui::Button::new(
                    egui::RichText::new("Forget").color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(150, 60, 60));
                if ui
                    .add(danger)
                    .on_hover_text(
                        "Disconnect, remove from the saved list and unpair from the OS Bluetooth registry.",
                    )
                    .clicked()
                {
                    let _ = tx.send(UiCommand::Forget(d.id.clone()));
                }
                let identify_btn =
                    ui.add_enabled(d.connected, egui::Button::new("Identify"));
                if identify_btn
                    .on_hover_text("Vibrate the controller for ~0.6 s to identify it physically")
                    .clicked()
                {
                    let _ = tx.send(UiCommand::Identify(d.id.clone()));
                }
                ui.separator();
                if d.connected {
                    if ui.button("Disconnect").clicked() {
                        let _ = tx.send(UiCommand::Disconnect(d.id.clone()));
                    }
                } else if ui.button("Connect").clicked() {
                    let _ = tx.send(UiCommand::Connect(d.id.clone()));
                }
            },
        );
    });

    // Inline warning chip — only shown when the daemon attached a
    // user-facing error to this device. The string is always a
    // pre-formatted UserFacingError message (no raw {e}).
    if let Some(err) = &d.last_error {
        render_error_chip(ui, err);
    }
}

fn render_error_chip(ui: &mut egui::Ui, message: &str) {
    let frame = egui::Frame::none()
        .fill(egui::Color32::from_rgb(60, 30, 30))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(170, 80, 80)))
        .rounding(egui::Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
        .outer_margin(egui::Margin::symmetric(0.0, 2.0));
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.colored_label(egui::Color32::from_rgb(255, 170, 170), "⚠");
            ui.colored_label(egui::Color32::from_rgb(255, 220, 220), message);
        });
    });
}

fn render_body(ui: &mut egui::Ui, d: &DeviceSnapshot) {
    ui.horizontal(|ui| {
        // Zone 1 — primary device (Wii Remote) painted as a stylised
        // silhouette with live button highlights.
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("Wii Remote").small().weak());
            device_widgets::paint_wiimote(ui, d);
        });

        ui.separator();

        // Zone 2 — connected extension. Same pictographic style as the
        // Wiimote zone for visual consistency across the row.
        ui.vertical(|ui| match (&d.ext_data, d.extension) {
            (Some(ExtensionData::Guitar(g)), _) => {
                ui.label(egui::RichText::new("Guitar (GH/RB)").small().weak());
                device_widgets::paint_guitar(ui, g);
            }
            (Some(ExtensionData::Drums(dr)), _) => {
                ui.label(egui::RichText::new("Drums (GH/RB)").small().weak());
                device_widgets::paint_drums(ui, dr);
            }
            (Some(ExtensionData::Nunchuk(n)), _) => {
                ui.label(egui::RichText::new("Nunchuk").small().weak());
                device_widgets::paint_nunchuk(ui, n);
            }
            (Some(ExtensionData::Classic(c)), _) => {
                ui.label(egui::RichText::new("Classic Controller").small().weak());
                device_widgets::paint_classic(ui, c);
            }
            (Some(ExtensionData::Unparsed), Some(t)) => {
                ui.label(egui::RichText::new(t.label()).small().weak());
                device_widgets::paint_no_extension(ui, "no parser yet");
            }
            (None, Some(t)) => {
                ui.label(egui::RichText::new(t.label()).small().weak());
                device_widgets::paint_no_extension(ui, "offline");
            }
            (None, None) | (_, None) => {
                ui.label(egui::RichText::new("No extension").small().weak());
                device_widgets::paint_no_extension(ui, "—");
            }
        });

        // Zone 3 — Xbox 360 mapping preview, anchored to the right
        // edge so the user can see the live result of the active
        // mapping profile alongside the source inputs.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Xbox 360 output").small().weak());
                let cs = ControllerState {
                    buttons: d.last_buttons,
                    accel: d.last_accel,
                    ir: d.last_ir,
                    ext: d.ext_data,
                };
                let xb = map_to_xbox(d.mapping_profile, &cs);
                crate::xbox_widget::render(ui, &xb);
            });
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
    // `last_error` is rendered as an inline chip above (right under
    // the device header) — keeping it close to the device name puts
    // it where the user is already looking when something is wrong.
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

fn short_id(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let tail = &s[s.len() - max..];
        format!("…{tail}")
    }
}
