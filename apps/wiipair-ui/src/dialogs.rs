//! Top-level modal dialogs.

use eframe::egui;

/// Recovery instructions when a BT pair attempt is wedged inside the
/// driver. The text is parameterised by OS so Linux/macOS users don't
/// see Windows-only steps (U5).
pub fn pairing_stuck_dialog(ctx: &egui::Context, addr: u64) -> bool {
    let mut close = false;
    egui::Window::new("Pairing stuck")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_min_width(440.0);
            ui.label(
                egui::RichText::new(format!(
                    "Pairing of {} is taking too long.",
                    format_bt_addr(addr)
                ))
                .strong(),
            );
            ui.add_space(6.0);
            ui.label(stuck_intro());
            ui.add_space(4.0);
            for step in stuck_steps() {
                ui.label(step);
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(stuck_outro())
                    .weak(),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Got it").clicked() {
                    close = true;
                }
                if cfg!(target_os = "windows") {
                    if ui
                        .button("Open Bluetooth settings")
                        .on_hover_text(
                            "Opens the Windows Bluetooth & devices page so you can toggle the radio off/on.",
                        )
                        .clicked()
                    {
                        let _ = std::process::Command::new("cmd")
                            .args(["/C", "start", "ms-settings:bluetooth"])
                            .spawn();
                    }
                }
            });
        });
    close
}

/// Confirmation modal for the irreversible Forget action (U6). Returns
/// `Some(true)` when the user confirmed, `Some(false)` when they
/// cancelled, `None` while still open.
pub fn confirm_forget_dialog(
    ctx: &egui::Context,
    device_name: &str,
    device_id: &str,
) -> Option<bool> {
    let mut decision: Option<bool> = None;
    egui::Window::new("Forget device?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.label(
                egui::RichText::new(format!("Forget '{device_name}' ({device_id})?"))
                    .strong(),
            );
            ui.add_space(4.0);
            ui.label(
                "This will disconnect, drop the device from the saved list, and \
                 unpair it from your OS Bluetooth registry. To use it again you'll \
                 have to re-pair (press 1+2 and click Scan).",
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
                let danger =
                    egui::Button::new(egui::RichText::new("Forget").color(egui::Color32::WHITE))
                        .fill(egui::Color32::from_rgb(180, 70, 70));
                if ui.add(danger).clicked() {
                    decision = Some(true);
                }
            });
        });
    decision
}

fn stuck_intro() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows' Bluetooth stack is wedged in the driver. We can't unstick it from here. To recover:"
    } else if cfg!(target_os = "linux") {
        "BlueZ has stalled on the pairing handshake. To recover:"
    } else {
        "The OS Bluetooth stack has stalled on the pairing handshake. To recover:"
    }
}

fn stuck_steps() -> Vec<&'static str> {
    if cfg!(target_os = "windows") {
        vec![
            "1.  Reset Bluetooth: Settings → Bluetooth & devices → toggle off, wait 5 seconds, toggle back on.",
            "2.  Power-cycle the Wiimote: pull the batteries for 30 seconds, reinsert.",
            "3.  Press 1+2 (NOT the red sync button under the battery cover). The 4 LEDs must blink in sequence 1→2→3→4 — not all at once.",
            "4.  Click \"Scan for new devices\" again here.",
        ]
    } else if cfg!(target_os = "linux") {
        vec![
            "1.  Reset Bluetooth: `sudo systemctl restart bluetooth` in a terminal.",
            "2.  Power-cycle the Wiimote: pull the batteries for 30 seconds, reinsert.",
            "3.  Press 1+2 — the 4 LEDs must blink in sequence 1→2→3→4.",
            "4.  Pair manually with `bluetoothctl` if the auto-pair keeps failing.",
        ]
    } else {
        vec![
            "1.  Toggle Bluetooth off and on in System Settings → Bluetooth.",
            "2.  Power-cycle the Wiimote: pull the batteries for 30 seconds, reinsert.",
            "3.  Press 1+2 — the 4 LEDs must blink in sequence 1→2→3→4.",
            "4.  Re-pair from System Settings → Bluetooth.",
        ]
    }
}

fn stuck_outro() -> &'static str {
    if cfg!(target_os = "windows") {
        "If pairing still hangs after this, close and reopen WiiPair — \
         the BT stack has likely been permanently confused and a process \
         restart clears its state."
    } else {
        "If pairing still hangs, close and reopen WiiPair — a fresh process \
         clears whatever stale state the OS BT stack accumulated."
    }
}

fn format_bt_addr(addr: u64) -> String {
    let b = addr.to_le_bytes();
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        b[5], b[4], b[3], b[2], b[1], b[0]
    )
}
