//! Top menu bar — File / View / Help.
//!
//! Lives in its own module so `main.rs` can stay focused on the
//! `eframe::App` glue. The bar is purely view-state: it returns a
//! small `MenuAction` value that `App` reacts to (open About,
//! toggle log panel, …) instead of holding mutable references back
//! into the App struct.

use eframe::egui;

/// Actions a menu item can request from the App. `None` is the
/// frame-by-frame steady state — the bar only emits a value when
/// the user actually clicks something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Quit,
    ToggleLog,
    ClearLog,
    OpenAbout,
    OpenRepo,
    OpenReleases,
    OpenIssues,
    StartScan,
}

/// Render the menu bar.
///
/// * `log_visible` drives the checked-state on View → Show log.
/// * `scan_remaining_s` is `Some(seconds_left)` while a discovery
///   window is active (button reads "Scanning… Ns" and is disabled),
///   `None` when idle (button reads "Scan for new devices" and is
///   clickable).
///
/// Returns `Some(action)` on a click, else `None`.
pub fn render(
    ui: &mut egui::Ui,
    log_visible: bool,
    scan_remaining_s: Option<u64>,
) -> Option<MenuAction> {
    let mut action: Option<MenuAction> = None;
    egui::menu::bar(ui, |ui| {
        ui.menu_button("File", |ui| {
            if ui.button("Quit").clicked() {
                action = Some(MenuAction::Quit);
                ui.close_menu();
            }
        });
        ui.menu_button("View", |ui| {
            // Manual checkbox-style label — `egui` doesn't expose a
            // ready-made "checked menu item" widget that auto-closes
            // on click, so we render the leading marker ourselves.
            let label = if log_visible {
                "✔  Show log panel"
            } else {
                "    Show log panel"
            };
            if ui.button(label).clicked() {
                action = Some(MenuAction::ToggleLog);
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Clear log").clicked() {
                action = Some(MenuAction::ClearLog);
                ui.close_menu();
            }
        });
        ui.menu_button("Help", |ui| {
            if ui.button("About WiiPair…").clicked() {
                action = Some(MenuAction::OpenAbout);
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Open repository").clicked() {
                action = Some(MenuAction::OpenRepo);
                ui.close_menu();
            }
            if ui.button("Latest releases").clicked() {
                action = Some(MenuAction::OpenReleases);
                ui.close_menu();
            }
            if ui.button("Report an issue").clicked() {
                action = Some(MenuAction::OpenIssues);
                ui.close_menu();
            }
        });

        // Right-aligned: the Scan button. Lives in the menu bar so
        // the device list reads from the very top with no extra
        // toolbar row above it.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match scan_remaining_s {
                Some(s) => {
                    let _ = ui.add_enabled(
                        false,
                        egui::Button::new(format!("Scanning… {s:>2}s")),
                    );
                }
                None => {
                    if ui
                        .button("Scan for new devices (30 s)")
                        .on_hover_text(
                            "Open a 30 s window to discover and pair new \
                             Wiimotes. Hold 1+2 on the controller while \
                             the button is counting down.",
                        )
                        .clicked()
                    {
                        action = Some(MenuAction::StartScan);
                    }
                }
            }
        });
    });
    action
}
