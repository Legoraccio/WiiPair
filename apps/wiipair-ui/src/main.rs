use chrono::{DateTime, Local};
use crossbeam_channel::Sender;
use eframe::egui;
use std::collections::VecDeque;
use wiimote_core::{
    Accelerometer, Buttons, ClassicButtons, ClassicState, DrumsButtons, DrumsState, ExtensionData,
    ExtensionType, GuitarButtons, GuitarState, IrDots, NunchukState,
};
use wiimote_daemon::{Daemon, DeviceSnapshot, LogLevel, UiCommand, UiEvent};

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let daemon = Daemon::start().expect("daemon failed to start");
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 540.0])
            .with_title("WiiPair"),
        ..Default::default()
    };
    eframe::run_native(
        "WiiPair",
        opts,
        Box::new(move |_cc| Ok(Box::new(App::new(daemon)))),
    )
}

struct LogLine {
    /// Wall-clock when the daemon emitted (or the UI ingested) the
    /// event. Surfaced as a `HH:MM:SS.mmm` prefix in the log pane so
    /// the user can correlate freezes with what the system was doing.
    timestamp: DateTime<Local>,
    level: LogLevel,
    text: String,
}

struct App {
    daemon: Daemon,
    devices: Vec<DeviceSnapshot>,
    log: VecDeque<LogLine>,
    /// `Some(deadline)` while the daemon's manual scan window is open.
    /// Drives the Scan button's disabled state and the countdown text.
    scan_active_until: Option<std::time::Instant>,
}

impl App {
    fn new(daemon: Daemon) -> Self {
        Self {
            daemon,
            devices: Vec::new(),
            log: VecDeque::with_capacity(64),
            scan_active_until: None,
        }
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.daemon.events_rx.try_recv() {
            match ev {
                UiEvent::DeviceListChanged(list) => self.devices = list,
                UiEvent::Log { level, message } => {
                    if self.log.len() >= 64 {
                        self.log.pop_front();
                    }
                    self.log.push_back(LogLine {
                        timestamp: Local::now(),
                        level,
                        text: message,
                    });
                }
                UiEvent::ScanState { active_until } => {
                    self.scan_active_until = active_until;
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("WiiPair");

                // Scan button — opens a 10 s discovery window. Disabled
                // and counting down while a window is already open, so
                // the user gets visible feedback that the scan is live.
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
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
                                    .button("Scan for new devices (10 s)")
                                    .on_hover_text(
                                        "Open a 10 s window to discover and pair new \
                                         Wiimotes. Hold 1+2 on the controller while the \
                                         button is counting down.",
                                    )
                                    .clicked()
                                {
                                    let _ = self
                                        .daemon
                                        .commands_tx
                                        .send(UiCommand::StartScan);
                                }
                            }
                        }
                    },
                );
            });
        });

        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("Log").strong());
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for line in &self.log {
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

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.devices.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label("No Wiimote detected.");
                    ui.label(
                        "Pair via Windows Bluetooth Settings. \
                         Press 1+2 on the Wiimote to enter discovery; \
                         in the pairing dialog choose 'No PIN'.",
                    );
                });
                return;
            }

            for d in self.devices.clone() {
                ui.group(|ui| {
                    render_header(ui, &d, &self.daemon.commands_tx);
                    ui.separator();
                    render_body(ui, &d);
                    render_footer(ui, &d);
                });
            }
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

// =====================================================================
// Per-device row layout
// =====================================================================

fn render_header(ui: &mut egui::Ui, d: &DeviceSnapshot, tx: &Sender<UiCommand>) {
    ui.horizontal(|ui| {
        let dot = if d.connected { "●" } else { "○" };
        let dot_color = if d.connected {
            egui::Color32::from_rgb(80, 200, 120)
        } else {
            egui::Color32::GRAY
        };
        ui.colored_label(dot_color, dot);
        ui.label(egui::RichText::new(device_icon(d)).size(18.0));
        ui.strong(&d.name);
        if let Some(ext) = d.extension {
            ui.label(
                egui::RichText::new(format!("· {}", ext.label()))
                    .color(extension_color(ext)),
            );
        }
        ui.label(
            egui::RichText::new(short(&d.id, 24))
                .monospace()
                .weak(),
        );

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
            },
        );
    });
}

fn render_body(ui: &mut egui::Ui, d: &DeviceSnapshot) {
    match &d.ext_data {
        Some(ExtensionData::Guitar(g)) => render_guitar(ui, d, g),
        Some(ExtensionData::Drums(dr)) => render_drums(ui, d, dr),
        Some(ExtensionData::Nunchuk(n)) => render_nunchuk(ui, d, n),
        Some(ExtensionData::Classic(c)) => render_classic(ui, d, c),
        _ => render_wiimote_only(ui, d),
    }
}

fn render_footer(ui: &mut egui::Ui, d: &DeviceSnapshot) {
    ui.separator();
    ui.horizontal(|ui| {
        if let Some(b) = d.battery {
            let pct = (b as f32) / 255.0 * 100.0;
            ui.label(format!("Battery: {pct:.0}%"));
        } else {
            ui.label("Battery: —");
        }
    });
    if let Some(err) = &d.last_error {
        ui.colored_label(egui::Color32::LIGHT_RED, err);
    }
}

// =====================================================================
// Per-extension panels
// =====================================================================

fn render_guitar(ui: &mut egui::Ui, d: &DeviceSnapshot, g: &GuitarState) {
    ui.horizontal(|ui| {
        // 5 frets — colours roughly match the real GH guitar caps.
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

        // Wiimote-side: Home stays as guide. Show only if pressed.
        if d.last_buttons.contains(Buttons::HOME) {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::LIGHT_BLUE, "Home");
        }
    });
}

fn render_drums(ui: &mut egui::Ui, d: &DeviceSnapshot, dr: &DrumsState) {
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

        if d.last_buttons.contains(Buttons::HOME) {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::LIGHT_BLUE, "Home");
        }
    });
}

fn render_nunchuk(ui: &mut egui::Ui, d: &DeviceSnapshot, n: &NunchukState) {
    ui.horizontal(|ui| {
        button_indicator(ui, "C", n.c, egui::Color32::from_rgb(180, 220, 180));
        button_indicator(ui, "Z", n.z, egui::Color32::from_rgb(180, 220, 180));
        ui.add_space(8.0);
        ui.label(egui::RichText::new(format!(
            "stick: x={:>3}  y={:>3}",
            n.stick_x, n.stick_y
        )).monospace());
        ui.add_space(12.0);
        ui.label(format!("Wiimote: {}", wiimote_button_str(d.last_buttons)));
    });
}

fn render_classic(ui: &mut egui::Ui, _d: &DeviceSnapshot, c: &ClassicState) {
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
            (ClassicButtons::DPAD_UP, "▲", egui::Color32::LIGHT_GRAY),
            (ClassicButtons::DPAD_DOWN, "▼", egui::Color32::LIGHT_GRAY),
            (ClassicButtons::DPAD_LEFT, "◀", egui::Color32::LIGHT_GRAY),
            (ClassicButtons::DPAD_RIGHT, "▶", egui::Color32::LIGHT_GRAY),
        ] {
            button_indicator(ui, name, c.buttons.contains(flag), color);
        }
    });
}

fn render_wiimote_only(ui: &mut egui::Ui, d: &DeviceSnapshot) {
    ui.horizontal_wrapped(|ui| {
        for (flag, name, color) in [
            (Buttons::A, "A", egui::Color32::from_rgb(80, 220, 80)),
            (Buttons::B, "B", egui::Color32::from_rgb(220, 80, 80)),
            (Buttons::ONE, "1", egui::Color32::LIGHT_GRAY),
            (Buttons::TWO, "2", egui::Color32::LIGHT_GRAY),
            (Buttons::PLUS, "+", egui::Color32::LIGHT_GRAY),
            (Buttons::MINUS, "−", egui::Color32::LIGHT_GRAY),
            (Buttons::HOME, "Home", egui::Color32::LIGHT_BLUE),
            (Buttons::UP, "▲", egui::Color32::LIGHT_GRAY),
            (Buttons::DOWN, "▼", egui::Color32::LIGHT_GRAY),
            (Buttons::LEFT, "◀", egui::Color32::LIGHT_GRAY),
            (Buttons::RIGHT, "▶", egui::Color32::LIGHT_GRAY),
        ] {
            button_indicator(ui, name, d.last_buttons.contains(flag), color);
        }
    });
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("Accel: {}", accel_str(d.last_accel)))
                .monospace()
                .small(),
        );
        ui.separator();
        ui.label(
            egui::RichText::new(format!("IR: {}", ir_str(d.last_ir)))
                .monospace()
                .small(),
        );
    });
}

// =====================================================================
// Indicator widgets
// =====================================================================

const FRET_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 220, 80);
const FRET_RED: egui::Color32 = egui::Color32::from_rgb(225, 70, 70);
const FRET_YELLOW: egui::Color32 = egui::Color32::from_rgb(235, 215, 70);
const FRET_BLUE: egui::Color32 = egui::Color32::from_rgb(80, 140, 235);
const FRET_ORANGE: egui::Color32 = egui::Color32::from_rgb(235, 145, 60);
const BASS_COLOR: egui::Color32 = egui::Color32::from_rgb(140, 140, 140);

/// Round, colored fret indicator with a label below. Filled when
/// pressed, ringed when released.
fn fret_indicator(ui: &mut egui::Ui, color: egui::Color32, pressed: bool, label: &str) {
    let size = egui::Vec2::new(40.0, 56.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let center = egui::Pos2::new(rect.center().x, rect.top() + 18.0);
    let radius = 14.0;
    if pressed {
        painter.circle_filled(center, radius, color);
        painter.circle_stroke(center, radius, egui::Stroke::new(2.0, egui::Color32::WHITE));
    } else {
        painter.circle_filled(center, radius, color.linear_multiply(0.18));
        painter.circle_stroke(center, radius, egui::Stroke::new(1.5, color));
    }
    painter.text(
        egui::Pos2::new(rect.center().x, rect.bottom() - 8.0),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        if pressed {
            egui::Color32::WHITE
        } else {
            egui::Color32::LIGHT_GRAY
        },
    );
}

/// Square pad indicator for drums — same idea as fret_indicator but
/// rounded-rectangle shaped.
fn pad_indicator(ui: &mut egui::Ui, color: egui::Color32, pressed: bool, label: &str) {
    let size = egui::Vec2::new(48.0, 56.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let pad_rect = egui::Rect::from_center_size(
        egui::Pos2::new(rect.center().x, rect.top() + 18.0),
        egui::Vec2::splat(28.0),
    );
    let rounding = egui::Rounding::same(4.0);
    if pressed {
        painter.rect_filled(pad_rect, rounding, color);
        painter.rect_stroke(
            pad_rect,
            rounding,
            egui::Stroke::new(2.0, egui::Color32::WHITE),
        );
    } else {
        painter.rect_filled(pad_rect, rounding, color.linear_multiply(0.18));
        painter.rect_stroke(pad_rect, rounding, egui::Stroke::new(1.5, color));
    }
    painter.text(
        egui::Pos2::new(rect.center().x, rect.bottom() - 8.0),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        if pressed {
            egui::Color32::WHITE
        } else {
            egui::Color32::LIGHT_GRAY
        },
    );
}

/// Strum bar: ▲ on top (up) and ▼ below (down), each lights up.
fn strum_indicator(ui: &mut egui::Ui, up: bool, down: bool) {
    let size = egui::Vec2::new(38.0, 56.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let dim = egui::Color32::from_rgb(80, 80, 80);
    let lit = egui::Color32::WHITE;
    painter.text(
        egui::Pos2::new(rect.center().x, rect.top() + 14.0),
        egui::Align2::CENTER_CENTER,
        "▲",
        egui::FontId::proportional(20.0),
        if up { lit } else { dim },
    );
    painter.text(
        egui::Pos2::new(rect.center().x, rect.top() + 36.0),
        egui::Align2::CENTER_CENTER,
        "▼",
        egui::FontId::proportional(20.0),
        if down { lit } else { dim },
    );
    painter.text(
        egui::Pos2::new(rect.center().x, rect.bottom() - 8.0),
        egui::Align2::CENTER_CENTER,
        "Strum",
        egui::FontId::proportional(10.0),
        egui::Color32::LIGHT_GRAY,
    );
}

/// Whammy bar progress with percentage.
fn whammy_bar(ui: &mut egui::Ui, value: u8) {
    ui.allocate_ui(egui::Vec2::new(120.0, 56.0), |ui| {
        ui.vertical(|ui| {
            ui.add_space(14.0);
            let pct = (value as f32 / 31.0).clamp(0.0, 1.0);
            ui.add(
                egui::ProgressBar::new(pct)
                    .desired_width(110.0)
                    .fill(egui::Color32::from_rgb(220, 90, 200)),
            );
            ui.label(
                egui::RichText::new(format!("Whammy {:>3}%", (pct * 100.0) as u32))
                    .small()
                    .weak(),
            );
        });
    });
}

/// Plus/Minus indicators stacked vertically.
fn pm_indicator(ui: &mut egui::Ui, plus: bool, minus: bool) {
    let size = egui::Vec2::new(36.0, 56.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let on = egui::Color32::WHITE;
    let off = egui::Color32::from_rgb(80, 80, 80);
    painter.text(
        egui::Pos2::new(rect.center().x, rect.top() + 16.0),
        egui::Align2::CENTER_CENTER,
        "+",
        egui::FontId::proportional(20.0),
        if plus { on } else { off },
    );
    painter.text(
        egui::Pos2::new(rect.center().x, rect.top() + 38.0),
        egui::Align2::CENTER_CENTER,
        "−",
        egui::FontId::proportional(20.0),
        if minus { on } else { off },
    );
}

/// Generic on/off button rendered as a small rounded label that lights
/// up when pressed.
fn button_indicator(ui: &mut egui::Ui, label: &str, pressed: bool, color: egui::Color32) {
    let bg = if pressed {
        color
    } else {
        color.linear_multiply(0.15)
    };
    let fg = if pressed {
        egui::Color32::BLACK
    } else {
        egui::Color32::LIGHT_GRAY
    };
    let stroke_color = if pressed {
        egui::Color32::WHITE
    } else {
        color.linear_multiply(0.6)
    };
    egui::Frame::default()
        .fill(bg)
        .stroke(egui::Stroke::new(1.0, stroke_color))
        .rounding(egui::Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(6.0, 3.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).color(fg).strong().size(12.0));
        });
}

// =====================================================================
// Helpers
// =====================================================================

fn device_icon(d: &DeviceSnapshot) -> &'static str {
    match d.extension {
        Some(ExtensionType::Guitar) => "🎸",
        Some(ExtensionType::Drums) => "🥁",
        Some(ExtensionType::DjHeroTurntable) => "🎚",
        Some(ExtensionType::Nunchuk) => "🕹",
        Some(ExtensionType::ClassicController)
        | Some(ExtensionType::ClassicControllerPro) => "🎮",
        _ => "🎮",
    }
}

fn extension_color(ext: ExtensionType) -> egui::Color32 {
    match ext {
        ExtensionType::Guitar => egui::Color32::from_rgb(255, 170, 80),
        ExtensionType::Drums => egui::Color32::from_rgb(120, 200, 255),
        ExtensionType::DjHeroTurntable => egui::Color32::from_rgb(220, 120, 255),
        ExtensionType::Nunchuk
        | ExtensionType::ClassicController
        | ExtensionType::ClassicControllerPro
        | ExtensionType::MotionPlus => egui::Color32::from_rgb(180, 220, 180),
        _ => egui::Color32::LIGHT_GRAY,
    }
}

fn short(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let tail = &s[s.len() - max..];
        format!("…{tail}")
    }
}

fn accel_str(a: Accelerometer) -> String {
    format!("x={:>4}  y={:>4}  z={:>4}", a.x, a.y, a.z)
}

fn ir_str(ir: IrDots) -> String {
    let visible = ir.iter().filter(|d| d.visible).count();
    if visible == 0 {
        return "—".into();
    }
    let mut parts = Vec::new();
    for (i, d) in ir.iter().enumerate() {
        if d.visible {
            parts.push(format!("[{i}: {},{}]", d.x, d.y));
        }
    }
    parts.join(" ")
}

fn wiimote_button_str(b: Buttons) -> String {
    if b.is_empty() {
        return "—".into();
    }
    let mut parts = Vec::new();
    for (flag, name) in [
        (Buttons::A, "A"),
        (Buttons::B, "B"),
        (Buttons::ONE, "1"),
        (Buttons::TWO, "2"),
        (Buttons::PLUS, "+"),
        (Buttons::MINUS, "−"),
        (Buttons::HOME, "Home"),
        (Buttons::UP, "▲"),
        (Buttons::DOWN, "▼"),
        (Buttons::LEFT, "◀"),
        (Buttons::RIGHT, "▶"),
    ] {
        if b.contains(flag) {
            parts.push(name);
        }
    }
    parts.join(" ")
}
