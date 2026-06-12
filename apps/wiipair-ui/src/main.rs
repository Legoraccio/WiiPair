// On a release build for Windows we don't want the secondary console
// window to pop up alongside the GUI. Debug builds keep the console
// so `tracing` output stays visible during development. Other
// platforms ignore the attribute.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod cap_self_grant;
mod device_card;
mod device_widgets;
mod dialogs;
mod icon;
mod icons;
mod menubar;
mod widgets;
mod xbox_widget;

use chrono::{DateTime, Local};
use eframe::egui;
use std::collections::VecDeque;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use wiimote_daemon::{
    Daemon, DeviceSnapshot, LogLevel, UiCommand, UiEvent, UiLogLayer, UserFacingError,
};
use wiimote_output::{ProbeFailure, probe_default};

use crate::menubar::MenuAction;

fn main() -> eframe::Result {
    // Wiimote pairing's PIN reply runs on the kernel mgmt socket
    // (only path that carries raw bytes). Binding it needs
    // CAP_NET_ADMIN. If we're missing it, ask polkit to grant it
    // ourselves before any UI is shown — the user sees one password
    // prompt the first time after each rebuild, then we re-exec into
    // the now-capable binary and continue normally.
    #[cfg(target_os = "linux")]
    cap_self_grant::ensure_cap_net_admin();

    // Two sinks for one source: stderr fmt for terminal users, plus
    // a layer that mirrors info/warn/error from `wiimote_*` modules
    // into the UI log panel so the user doesn't have to consult two
    // places to understand what just happened.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(UiLogLayer)
        .init();

    let daemon = match Daemon::start() {
        Ok(d) => d,
        Err(e) => {
            // Fatal: nothing the UI can do without a running daemon.
            // On Windows release the console is hidden, so a plain
            // panic produces a silent crash. Show a dedicated error
            // window with the user-friendly explanation and the raw
            // detail tucked into a collapsing "Technical details".
            eprintln!("daemon failed to start: {e}");
            return show_fatal_error(format!("{e}"));
        }
    };
    // Wire the layer's sink up now that the daemon has created its
    // event channel. Tracing events emitted in the brief window
    // before this point are dropped — that's only the very first
    // frames of `Daemon::start`, which carry no user-relevant info.
    wiimote_daemon::install_ui_log_sender(daemon.log_sender());
    // Probe the platform output backend before showing the UI so a
    // missing ViGEmBus / unwritable /dev/uinput surfaces as a dedicated
    // dialog at startup instead of a cryptic per-row error after the
    // first Wiimote connects.
    let driver_probe = probe_default().err();
    // The device card footer (battery + tilt disc + IR canvas + profile
    // dropdown) and the Wiimote+extension live-state body don't fit
    // below ~820x540 without overlapping each other. The previous
    // 640x420 minimum let the user shrink the window to a state where
    // widgets clipped through one another.
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([960.0, 660.0])
        .with_min_inner_size([820.0, 560.0])
        .with_title("WiiPair");
    if let Some(icon) = icon::load() {
        viewport = viewport.with_icon(icon);
    }
    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "WiiPair",
        opts,
        Box::new(move |_cc| Ok(Box::new(App::new(daemon, driver_probe)))),
    )
}

/// Display a fatal-error window when the daemon (or any other startup
/// dependency) can't initialise. This is the only path that runs
/// without a `Daemon`, so it lives outside `App`. On Windows release
/// builds the console is hidden — a plain panic yields a silent
/// crash, which is why we go through the trouble of standing up a
/// dedicated egui surface just for this case.
fn show_fatal_error(detail: String) -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([540.0, 320.0])
        .with_min_inner_size([460.0, 260.0])
        .with_title("WiiPair — couldn't start");
    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "WiiPair — couldn't start",
        opts,
        Box::new(move |_cc| Ok(Box::new(FatalErrorApp { detail }))),
    )
}

struct FatalErrorApp {
    /// Raw error message — shown only inside the collapsible
    /// "Technical details" so the main copy stays user-facing.
    detail: String,
}

impl eframe::App for FatalErrorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(10.0);
            ui.heading("WiiPair couldn't start");
            ui.add_space(8.0);
            ui.label(UserFacingError::DaemonStartFailed.message());
            ui.add_space(10.0);
            ui.collapsing("Technical details", |ui| {
                ui.label(
                    egui::RichText::new(&self.detail)
                        .monospace()
                        .small()
                        .weak(),
                );
            });
            ui.add_space(16.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
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

/// One persistent banner shown above the device list. `key` is the
/// `UserFacingError`'s dedup key so a flapping daemon doesn't stack
/// multiple copies of the same warning.
struct PersistentWarning {
    key: &'static str,
    message: String,
}

/// Bundles the state of every modal/dialog the UI can show. Pulling
/// these out of `App` keeps the App struct focused on data + plumbing
/// — the driver probe, pairing-stuck recovery, forget-confirm, and
/// About flows are otherwise unrelated to each other and the rest of
/// the UI doesn't care about them between dialog impressions.
#[derive(Default)]
struct Modals {
    /// `Some(addr)` when the daemon told us a pairing attempt is hung;
    /// drives the recovery-instructions dialog.
    pairing_stuck: Option<u64>,
    /// `Some(id)` while the user is on the "are you sure?" forget
    /// dialog. Cleared on confirm/cancel.
    pending_forget: Option<String>,
    /// Set at startup by `probe_default()` when the platform output
    /// backend isn't ready; cleared once the user dismisses the
    /// install-driver dialog.
    driver_probe: Option<ProbeFailure>,
    driver_dialog_dismissed: bool,
    /// Set when Help → About is clicked.
    about_open: bool,
}

struct App {
    daemon: Daemon,
    devices: Vec<DeviceSnapshot>,
    log: VecDeque<LogLine>,
    log_filter: LogFilter,
    /// View → Show log panel toggle. Persists for the lifetime of the
    /// process; defaults to visible.
    log_visible: bool,
    scan_active_until: Option<std::time::Instant>,
    modals: Modals,
    /// Per-frame channel populated by device cards; drained at the end
    /// of `render_devices_panel`. Lets the App intercept Forget into a
    /// confirmation modal while forwarding everything else verbatim.
    card_tx: crossbeam_channel::Sender<UiCommand>,
    card_rx: crossbeam_channel::Receiver<UiCommand>,
    /// Persistent banner derived from `UiEvent::PersistentWarning`.
    /// Survives log eviction so the user notices even after the
    /// originating log line has scrolled off (U12).
    persistent_warnings: Vec<PersistentWarning>,
}

impl App {
    fn new(daemon: Daemon, driver_probe: Option<ProbeFailure>) -> Self {
        let (card_tx, card_rx) = crossbeam_channel::unbounded();
        Self {
            daemon,
            devices: Vec::new(),
            log: VecDeque::with_capacity(LOG_CAPACITY),
            log_filter: LogFilter::default(),
            // Hidden by default — the persistent banners surface the
            // important issues; users open the log via View → Show
            // log panel only when they need the technical detail.
            log_visible: false,
            scan_active_until: None,
            modals: Modals {
                driver_probe,
                ..Modals::default()
            },
            card_tx,
            card_rx,
            persistent_warnings: Vec::new(),
        }
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.daemon.events_rx.try_recv() {
            match ev {
                UiEvent::DeviceListChanged(list) => self.devices = list,
                UiEvent::Log { at, level, message } => {
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
                    self.modals.pairing_stuck = Some(addr);
                }
                UiEvent::PersistentWarning(err) => {
                    let key = err.dedup_key();
                    if !self.persistent_warnings.iter().any(|w| w.key == key) {
                        self.persistent_warnings.push(PersistentWarning {
                            key,
                            message: err.message(),
                        });
                    }
                }
                UiEvent::PersistentWarningResolved(key) => {
                    // The condition cleared on the daemon side — drop
                    // the matching banner (if any). Banners the user
                    // already dismissed are simply not present, so
                    // this retain() is a no-op for them.
                    self.persistent_warnings.retain(|w| w.key != key);
                }
            }
        }
    }

    fn handle_menu_action(&mut self, action: MenuAction, ctx: &egui::Context) {
        match action {
            MenuAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            MenuAction::ToggleLog => self.log_visible = !self.log_visible,
            MenuAction::ClearLog => self.log.clear(),
            MenuAction::OpenAbout => self.modals.about_open = true,
            MenuAction::OpenRepo => dialogs::open_url(dialogs::REPO_URL),
            MenuAction::OpenReleases => dialogs::open_url(dialogs::RELEASES_URL),
            MenuAction::OpenIssues => dialogs::open_url(dialogs::ISSUES_URL),
            MenuAction::StartScan => self.daemon.send_command(UiCommand::StartScan),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();

        self.render_menu_bar(ctx);
        self.render_warning_banners(ctx);
        self.render_status_bar(ctx);
        if self.log_visible {
            self.render_log_panel(ctx);
        }
        self.render_devices_panel(ctx);
        self.render_modals(ctx);

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

impl App {
    fn render_menu_bar(&mut self, ctx: &egui::Context) {
        // Compute the scan-window countdown once per frame and hand
        // it to the menu bar. `Some(N)` while a scan is running,
        // `None` when idle.
        let scan_remaining_s = self
            .scan_active_until
            .and_then(|t| t.checked_duration_since(std::time::Instant::now()))
            .map(|d| d.as_secs() + 1);
        let mut action: Option<MenuAction> = None;
        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            action = menubar::render(ui, self.log_visible, scan_remaining_s);
        });
        if let Some(a) = action {
            self.handle_menu_action(a, ctx);
        }
    }

    fn render_warning_banners(&mut self, ctx: &egui::Context) {
        if self.persistent_warnings.is_empty() {
            return;
        }
        // Track which warning the user clicked Dismiss on; mutate
        // the vec after the show() callback to avoid holding a
        // borrow while iterating.
        let mut dismissed_key: Option<&'static str> = None;
        egui::TopBottomPanel::top("warnings").show(ctx, |ui| {
            for w in &self.persistent_warnings {
                let frame = egui::Frame::none()
                    .fill(egui::Color32::from_rgb(60, 45, 25))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgb(180, 140, 60),
                    ))
                    .rounding(egui::Rounding::same(4.0))
                    .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                    .outer_margin(egui::Margin::symmetric(0.0, 2.0));
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            egui::Color32::from_rgb(240, 200, 100),
                            "⚠",
                        );
                        ui.colored_label(
                            egui::Color32::from_rgb(245, 225, 190),
                            &w.message,
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui
                                    .small_button("Dismiss")
                                    .on_hover_text(
                                        "Hide this warning. It will reappear if the \
                                         underlying condition recurs.",
                                    )
                                    .clicked()
                                {
                                    dismissed_key = Some(w.key);
                                }
                            },
                        );
                    });
                });
            }
        });
        if let Some(k) = dismissed_key {
            self.persistent_warnings.retain(|w| w.key != k);
        }
    }

    fn render_status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("statusbar")
            .frame(
                egui::Frame::none()
                    .fill(ctx.style().visuals.faint_bg_color)
                    .inner_margin(egui::Margin::symmetric(10.0, 4.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let connected = self.devices.iter().filter(|d| d.connected).count();
                    let known = self.devices.len();
                    ui.label(
                        egui::RichText::new(format!(
                            "{connected} connected · {known} known"
                        ))
                        .small()
                        .weak(),
                    );
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "v{}",
                                    env!("CARGO_PKG_VERSION")
                                ))
                                .small()
                                .weak(),
                            );
                        },
                    );
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
                self.render_empty_state(ui);
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
                    self.modals.pending_forget = Some(id);
                }
                other => {
                    self.daemon.send_command(other);
                }
            }
        }
    }

    fn render_empty_state(&mut self, ui: &mut egui::Ui) {
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No Wiimote known yet")
                    .heading()
                    .weak(),
            );
            ui.add_space(8.0);
            ui.label(
                "Click the button below and press 1+2 on the Wiimote, \
                 or pair manually via your OS Bluetooth settings.",
            );
            ui.add_space(14.0);
            let scanning = self.scan_active_until.is_some_and(|t| {
                t.checked_duration_since(std::time::Instant::now()).is_some()
            });
            ui.add_enabled_ui(!scanning, |ui| {
                if ui
                    .add(egui::Button::new("Scan for new devices (30 s)"))
                    .clicked()
                {
                    self.daemon.send_command(UiCommand::StartScan);
                }
            });
        });
    }

    fn render_modals(&mut self, ctx: &egui::Context) {
        // Driver-missing dialog (ViGEmBus / uinput). Shown once at
        // startup; dismissing it sets a sticky flag so it doesn't
        // re-pop on every frame.
        if let Some(failure) = &self.modals.driver_probe {
            if !self.modals.driver_dialog_dismissed
                && dialogs::driver_missing_dialog(ctx, failure)
            {
                self.modals.driver_dialog_dismissed = true;
            }
        }

        // Pairing-stuck recovery dialog.
        if let Some(addr) = self.modals.pairing_stuck {
            if dialogs::pairing_stuck_dialog(ctx, addr) {
                self.modals.pairing_stuck = None;
            }
        }

        // Forget confirmation.
        if let Some(id) = self.modals.pending_forget.clone() {
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
                    self.daemon.send_command(UiCommand::Forget(id));
                }
                self.modals.pending_forget = None;
            }
        }

        // About dialog (Help → About WiiPair…).
        if self.modals.about_open && dialogs::about_dialog(ctx) {
            self.modals.about_open = false;
        }
    }
}
