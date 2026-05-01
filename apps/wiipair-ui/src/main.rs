use eframe::egui;
use std::collections::VecDeque;
use wiimote_core::{Accelerometer, Buttons, IrDots};
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
            .with_inner_size([720.0, 480.0])
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
    level: LogLevel,
    text: String,
}

struct App {
    daemon: Daemon,
    devices: Vec<DeviceSnapshot>,
    log: VecDeque<LogLine>,
}

impl App {
    fn new(daemon: Daemon) -> Self {
        Self {
            daemon,
            devices: Vec::new(),
            log: VecDeque::with_capacity(64),
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
                        level,
                        text: message,
                    });
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
                ui.label(egui::RichText::new("·").weak());
                ui.label("scan ogni 2 s");
            });
        });

        egui::TopBottomPanel::bottom("log").resizable(true).show(ctx, |ui| {
            ui.label(egui::RichText::new("Log").strong());
            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for line in &self.log {
                    let color = match line.level {
                        LogLevel::Info => egui::Color32::LIGHT_GRAY,
                        LogLevel::Warn => egui::Color32::YELLOW,
                        LogLevel::Error => egui::Color32::LIGHT_RED,
                    };
                    ui.colored_label(color, &line.text);
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.devices.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label("Nessun Wiimote rilevato.");
                    ui.label(
                        "Pair via Impostazioni Bluetooth di Windows. \
                         Premi 1+2 sul Wiimote per metterlo in discovery; \
                         in pairing scegli 'Senza codice'.",
                    );
                });
                return;
            }

            for d in self.devices.clone() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        let dot = if d.connected { "●" } else { "○" };
                        let dot_color = if d.connected {
                            egui::Color32::from_rgb(80, 200, 120)
                        } else {
                            egui::Color32::GRAY
                        };
                        ui.colored_label(dot_color, dot);
                        ui.strong(&d.name);
                        ui.label(
                            egui::RichText::new(short(&d.id, 24))
                                .monospace()
                                .weak(),
                        );

                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if d.connected {
                                    if ui.button("Disconnetti").clicked() {
                                        let _ = self.daemon.commands_tx.send(
                                            UiCommand::Disconnect(d.id.clone()),
                                        );
                                    }
                                } else if ui.button("Connetti").clicked() {
                                    let _ = self.daemon.commands_tx.send(
                                        UiCommand::Connect(d.id.clone()),
                                    );
                                }
                            },
                        );
                    });

                    ui.horizontal(|ui| {
                        if let Some(b) = d.battery {
                            let pct = (b as f32) / 255.0 * 100.0;
                            ui.label(format!("Batteria: {pct:.0}%"));
                        } else {
                            ui.label("Batteria: —");
                        }
                        ui.separator();
                        ui.label(format!("Tasti: {}", buttons_str(d.last_buttons)));
                    });
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Accel: {}",
                                accel_str(d.last_accel)
                            ))
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

                    if let Some(err) = &d.last_error {
                        ui.colored_label(egui::Color32::LIGHT_RED, err);
                    }
                });
            }
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
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

fn buttons_str(b: Buttons) -> String {
    if b.is_empty() {
        "—".into()
    } else {
        let mut parts = Vec::new();
        for (flag, name) in [
            (Buttons::A, "A"),
            (Buttons::B, "B"),
            (Buttons::ONE, "1"),
            (Buttons::TWO, "2"),
            (Buttons::PLUS, "+"),
            (Buttons::MINUS, "−"),
            (Buttons::HOME, "Home"),
            (Buttons::UP, "↑"),
            (Buttons::DOWN, "↓"),
            (Buttons::LEFT, "←"),
            (Buttons::RIGHT, "→"),
        ] {
            if b.contains(flag) {
                parts.push(name);
            }
        }
        parts.join(" ")
    }
}
