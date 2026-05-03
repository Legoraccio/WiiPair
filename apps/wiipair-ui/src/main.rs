// On a release build for Windows we don't want the secondary console
// window to pop up alongside the GUI. Debug builds keep the console
// so `tracing` output stays visible during development. Other
// platforms ignore the attribute.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod device_card;
mod dialogs;
mod icons;
mod widgets;

use chrono::{DateTime, Local};
use eframe::egui;
use std::collections::VecDeque;
use wiimote_daemon::{Daemon, DeviceSnapshot, LogLevel, UiCommand, UiEvent};
use wiimote_output::{ProbeFailure, probe_default};

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let daemon = Daemon::start().expect("daemon failed to start");
    // Probe the platform output backend before showing the UI so a
    // missing ViGEmBus / unwritable /dev/uinput surfaces as a dedicated
    // dialog at startup instead of a cryptic per-row error after the
    // first Wiimote connects.
    let driver_probe = probe_default().err();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 600.0])
            .with_min_inner_size([640.0, 420.0])
            .with_title("WiiPair"),
        ..Default::default()
    };
    eframe::run_native(
        "WiiPair",
        opts,
        Box::new(move |_cc| Ok(Box::new(App::new(daemon, driver_probe)))),
    )
}

/// How many recent log lines to retain in the on-screen scrollback.
const LOG_CAPACITY: usize = 256;

struct LogLine {
    timestamp: DateTime<Local>,
    level: LogLevel,
    text: String,
}

#[derive(Default, PartialEq)]
struct LogFilter {
    info: bool,
    warn: bool,
    err: bool,
}

impl LogFilter {
    fn shows(&self, level: LogLevel) -> bool {
        // When no filter is set we show everything; when any filter is
        // checked we treat it as a positive include.
        if !self.info && !self.warn && !self.err {
            return true;
        }
        match level {
            LogLevel::Info => self.info,
            LogLevel::Warn => self.warn,
            LogLevel::Error => self.err,
        }
    }
}

struct App {
    daemon: Daemon,
    devices: Vec<DeviceSnapshot>,
    log: VecDeque<LogLine>,
    log_filter: LogFilter,
    scan_active_until: Option<std::time::Instant>,
    /// `Some(addr)` when the daemon told us a pairing attempt is hung;
    /// drives the recovery-instructions dialog.
    pairing_stuck: Option<u64>,
    /// `Some(id)` while the user is on the "are you sure?" forget
    /// dialog. Cleared on confirm/cancel.
    pending_forget: Option<String>,
    /// Per-frame channel populated by device cards; drained at the end
    /// of `render_devices_panel`. Lets the App intercept Forget into a
    /// confirmation modal while forwarding everything else verbatim.
    card_tx: crossbeam_channel::Sender<UiCommand>,
    card_rx: crossbeam_channel::Receiver<UiCommand>,
    /// Persistent banner derived from the log: surfaces "scanner
    /// disabled" / "ViGEmBus unavailable" so the user notices even
    /// after the original log line has scrolled off (U12).
    persistent_warnings: Vec<String>,
    /// Set at startup by `probe_default()` when the platform output
    /// backend isn't ready; cleared once the user dismisses the
    /// install-driver dialog.
    driver_probe: Option<ProbeFailure>,
    driver_dialog_dismissed: bool,
}

impl App {
    fn new(daemon: Daemon, driver_probe: Option<ProbeFailure>) -> Self {
        let (card_tx, card_rx) = crossbeam_channel::unbounded();
        Self {
            daemon,
            devices: Vec::new(),
            log: VecDeque::with_capacity(LOG_CAPACITY),
            log_filter: LogFilter::default(),
            scan_active_until: None,
            pairing_stuck: None,
            pending_forget: None,
            card_tx,
            card_rx,
            persistent_warnings: Vec::new(),
            driver_probe,
            driver_dialog_dismissed: false,
        }
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.daemon.events_rx.try_recv() {
            match ev {
                UiEvent::DeviceListChanged(list) => self.devices = list,
                UiEvent::Log { at, level, message } => {
                    Self::maybe_set_persistent_warning(
                        &mut self.persistent_warnings,
                        level,
                        &message,
                    );
                    if self.log.len() >= LOG_CAPACITY {
                        self.log.pop_front();
                    }
                    self.log.push_back(LogLine {
                        timestamp: DateTime::<Local>::from(at),
                        level,
                        text: message,
                    });
                }
                UiEvent::ScanState { active_until } => {
                    self.scan_active_until = active_until;
                }
                UiEvent::PairingStuck { addr } => {
                    self.pairing_stuck = Some(addr);
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();

        self.render_header(ctx);
        self.render_log_panel(ctx);
        self.render_devices_panel(ctx);
        self.render_modals(ctx);

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

impl App {
    /// Promote a small set of one-time log warnings into a persistent
    /// banner — once the BT scanner has died or ViGEmBus is missing,
    /// the user shouldn't have to scroll back through hundreds of
    /// lines to find the message.
    fn maybe_set_persistent_warning(
        warnings: &mut Vec<String>,
        level: LogLevel,
        message: &str,
    ) {
        if !matches!(level, LogLevel::Warn | LogLevel::Error) {
            return;
        }
        let triggers: &[&str] = &[
            "scanner disabled",
            "Virtual controller output unavailable",
            "Virtual controller output not implemented",
            "ViGEmBus",
        ];
        if !triggers.iter().any(|t| message.contains(t)) {
            return;
        }
        if warnings.iter().any(|w| w == message) {
            return;
        }
        warnings.push(message.to_string());
    }

    fn render_header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            // Persistent warning banner (U12) — survives log eviction.
            for w in &self.persistent_warnings {
                let frame = egui::Frame::none()
                    .fill(egui::Color32::from_rgb(60, 30, 30))
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0));
                frame.show(ui, |ui| {
                    ui.colored_label(egui::Color32::from_rgb(255, 200, 200), w);
                });
            }
            ui.horizontal(|ui| {
                ui.heading("WiiPair");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let now = std::time::Instant::now();
                    let remaining = self
                        .scan_active_until
                        .and_then(|t| t.checked_duration_since(now));
                    match remaining {
                        Some(left) => {
                            let _ = ui.add_enabled(
                                false,
                                egui::Button::new(format!(
                                    "Scanning… {:>2}s",
                                    left.as_secs() + 1
                                )),
                            );
                        }
                        None => {
                            if ui
                                .button("Scan for new devices (30 s)")
                                .on_hover_text(
                                    "Open a 30 s window to discover and pair new \
                                     Wiimotes. Hold 1+2 on the controller while the \
                                     button is counting down.",
                                )
                                .clicked()
                            {
                                let _ = self.daemon.commands_tx.send(UiCommand::StartScan);
                            }
                        }
                    }
                });
            });
        });
    }

    fn render_log_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .min_height(60.0)
            .default_height(160.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Log").strong());
                    ui.separator();
                    ui.checkbox(&mut self.log_filter.info, "Info");
                    ui.checkbox(&mut self.log_filter.warn, "Warn");
                    ui.checkbox(&mut self.log_filter.err, "Error");
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.button("Clear").clicked() {
                                self.log.clear();
                            }
                        },
                    );
                });
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for line in &self.log {
                            if !self.log_filter.shows(line.level) {
                                continue;
                            }
                            let color = match line.level {
                                LogLevel::Info => egui::Color32::LIGHT_GRAY,
                                LogLevel::Warn => egui::Color32::YELLOW,
                                LogLevel::Error => egui::Color32::LIGHT_RED,
                            };
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(
                                        line.timestamp
                                            .format("%H:%M:%S%.3f")
                                            .to_string(),
                                    )
                                    .monospace()
                                    .weak(),
                                );
                                ui.colored_label(color, &line.text);
                            });
                        }
                    });
            });
    }

    fn render_devices_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.devices.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label("No Wiimote known yet.");
                    ui.label(
                        "Click 'Scan for new devices' and press 1+2 on the Wiimote, \
                         or pair manually via your OS Bluetooth settings.",
                    );
                });
                return;
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let devices = self.devices.clone();
                    for d in &devices {
                        crate::device_card::render_device(ui, d, &self.card_tx);
                    }
                });
        });

        // Drain card-emitted commands. Forget pops a confirmation
        // dialog (U6); everything else passes through unchanged.
        while let Ok(cmd) = self.card_rx.try_recv() {
            match cmd {
                UiCommand::Forget(id) => {
                    self.pending_forget = Some(id);
                }
                other => {
                    let _ = self.daemon.commands_tx.send(other);
                }
            }
        }
    }

    fn render_modals(&mut self, ctx: &egui::Context) {
        // Driver-missing dialog (ViGEmBus / uinput). Shown once at
        // startup; dismissing it sets a sticky flag so it doesn't
        // re-pop on every frame.
        if let Some(failure) = &self.driver_probe {
            if !self.driver_dialog_dismissed
                && dialogs::driver_missing_dialog(ctx, failure)
            {
                self.driver_dialog_dismissed = true;
            }
        }

        // Pairing-stuck recovery dialog.
        if let Some(addr) = self.pairing_stuck {
            if dialogs::pairing_stuck_dialog(ctx, addr) {
                self.pairing_stuck = None;
            }
        }

        // Forget confirmation.
        if let Some(id) = self.pending_forget.clone() {
            let name = self
                .devices
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| "(unknown)".into());
            if let Some(decision) =
                dialogs::confirm_forget_dialog(ctx, &name, &id)
            {
                if decision {
                    let _ = self.daemon.commands_tx.send(UiCommand::Forget(id));
                }
                self.pending_forget = None;
            }
        }
    }
}

