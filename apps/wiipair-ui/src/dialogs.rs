//! Top-level modal dialogs.

use eframe::egui;
use wiimote_output::{ProbeFailure, ProbeKind};

/// Shown at startup when the platform output backend isn't ready
/// (Windows: ViGEmBus not installed; Linux: /dev/uinput not writable).
/// Returns true when the user dismisses the dialog. The caller stores
/// that as a sticky flag so the dialog doesn't keep re-appearing.
pub fn driver_missing_dialog(ctx: &egui::Context, failure: &ProbeFailure) -> bool {
    let mut close = false;
    let title = match failure.kind {
        ProbeKind::ViGEmBusMissing => "ViGEmBus not detected",
        ProbeKind::UinputUnavailable => "/dev/uinput not writable",
    };
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_min_width(480.0);
            match failure.kind {
                ProbeKind::ViGEmBusMissing => render_vigem_missing(ui),
                ProbeKind::UinputUnavailable => render_uinput_unavailable(ui),
            }
            ui.add_space(8.0);
            ui.collapsing("Technical details", |ui| {
                ui.label(
                    egui::RichText::new(&failure.detail)
                        .monospace()
                        .small()
                        .weak(),
                );
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Got it").clicked() {
                    close = true;
                }
                if failure.kind == ProbeKind::ViGEmBusMissing
                    && ui
                        .button("Open ViGEmBus releases page")
                        .on_hover_text(
                            "Opens https://github.com/nefarius/ViGEmBus/releases in your browser",
                        )
                        .clicked()
                {
                    open_url("https://github.com/nefarius/ViGEmBus/releases");
                }
            });
        });
    close
}

fn render_vigem_missing(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(
            "WiiPair couldn't talk to the ViGEmBus driver. Without it the \
             app can still read Wiimote input, but it can't expose a \
             virtual Xbox 360 pad to your games.",
        )
        .strong(),
    );
    ui.add_space(6.0);
    ui.label("To fix it:");
    ui.add_space(2.0);
    ui.label(
        "1.  Download ViGEmBus_Setup_*_x64.msi from the ViGEmBus releases page.",
    );
    ui.label("2.  Run the installer and accept the driver signature prompt.");
    ui.label("3.  Reboot Windows so the driver service starts cleanly.");
    ui.label("4.  Re-launch WiiPair.");
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "If you've already installed ViGEmBus, check that the \
             'Nefarius Virtual Gamepad Bus' service is running \
             (services.msc) and that no other tool — HidHide, \
             HidGuardian — is hiding it.",
        )
        .weak(),
    );
}

fn render_uinput_unavailable(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(
            "WiiPair can't write to /dev/uinput, so it can't publish a \
             virtual Xbox 360 pad. Wiimote input will still be visible \
             in the UI, but no game will see a controller.",
        )
        .strong(),
    );
    ui.add_space(6.0);
    ui.label("To fix it:");
    ui.add_space(2.0);
    ui.label(
        "1.  Install the udev rule shipped with WiiPair:\n     \
         sudo cp docs/udev/99-wiipair.rules /etc/udev/rules.d/",
    );
    ui.label(
        "2.  sudo udevadm control --reload && sudo udevadm trigger",
    );
    ui.label("3.  sudo usermod -aG input \"$USER\"");
    ui.label("4.  Log out and back in so the new group sticks, then re-run WiiPair.");
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "If your kernel doesn't ship the uinput module, run \
             'sudo modprobe uinput' first; some minimal distros need \
             to enable it manually.",
        )
        .weak(),
    );
}

/// Open `url` in the user's default browser. Best-effort: if the
/// platform helper isn't installed (xdg-open missing on minimal
/// Linux setups, etc.) we silently degrade — there's nowhere
/// useful to surface a "couldn't spawn xdg-open" message in this
/// flow.
pub fn open_url(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// Repository home — used by the menu bar's "Open repository" entry
/// and as the canonical link inside the About dialog.
pub const REPO_URL: &str = "https://github.com/Legoraccio/WiiPair";
/// Direct deep-link to the new-issue form, opened by Help → Report
/// an issue.
pub const ISSUES_URL: &str = "https://github.com/Legoraccio/WiiPair/issues";
/// Releases page used by Help → Latest releases.
pub const RELEASES_URL: &str = "https://github.com/Legoraccio/WiiPair/releases";

/// Modal "About WiiPair" dialog. Returns true once the user dismisses
/// it so `App` can clear the open flag.
pub fn about_dialog(ctx: &egui::Context) -> bool {
    let mut close = false;
    egui::Window::new("About WiiPair")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_min_width(380.0);
            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                ui.heading("WiiPair");
                ui.label(
                    egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                        .small()
                        .weak(),
                );
                ui.add_space(8.0);
                ui.label(
                    "Bridges Bluetooth Wii controllers to virtual Xbox 360 \
                     pads on the desktop, so XInput-aware games see them as \
                     standard controllers.",
                );
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                if ui
                    .link(egui::RichText::new(REPO_URL).monospace())
                    .on_hover_text("Open the WiiPair repository on GitHub")
                    .clicked()
                {
                    open_url(REPO_URL);
                }
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if ui.link("Releases").clicked() {
                        open_url(RELEASES_URL);
                    }
                    ui.label("·");
                    if ui.link("Report an issue").clicked() {
                        open_url(ISSUES_URL);
                    }
                });
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("License: MIT")
                        .small()
                        .weak(),
                );
                ui.add_space(10.0);
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
    close
}

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
                if cfg!(target_os = "windows")
                    && ui
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
