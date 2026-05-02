//! Painter-drawn input indicators.
//!
//! Every indicator avoids font glyphs that aren't always present in
//! the bundled egui fonts (▲▼◀▶ / ↑↓←→), so the row renders the same
//! across systems regardless of which fonts the OS happens to ship.

use eframe::egui;

// =====================================================================
// Color palette
// =====================================================================

pub const FRET_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 220, 80);
pub const FRET_RED: egui::Color32 = egui::Color32::from_rgb(225, 70, 70);
pub const FRET_YELLOW: egui::Color32 = egui::Color32::from_rgb(235, 215, 70);
pub const FRET_BLUE: egui::Color32 = egui::Color32::from_rgb(80, 140, 235);
pub const FRET_ORANGE: egui::Color32 = egui::Color32::from_rgb(235, 145, 60);
pub const BASS_COLOR: egui::Color32 = egui::Color32::from_rgb(140, 140, 140);

// =====================================================================
// Arrow primitives
// =====================================================================

#[derive(Copy, Clone)]
pub enum ArrowDir {
    Up,
    Down,
    Left,
    Right,
}

pub fn paint_arrow(
    painter: &egui::Painter,
    center: egui::Pos2,
    dir: ArrowDir,
    size: f32,
    color: egui::Color32,
) {
    let s = size;
    let h = s * 0.7;
    let pts = match dir {
        ArrowDir::Up => vec![
            egui::Pos2::new(center.x, center.y - s),
            egui::Pos2::new(center.x - s, center.y + h),
            egui::Pos2::new(center.x + s, center.y + h),
        ],
        ArrowDir::Down => vec![
            egui::Pos2::new(center.x, center.y + s),
            egui::Pos2::new(center.x - s, center.y - h),
            egui::Pos2::new(center.x + s, center.y - h),
        ],
        ArrowDir::Left => vec![
            egui::Pos2::new(center.x - s, center.y),
            egui::Pos2::new(center.x + h, center.y - s),
            egui::Pos2::new(center.x + h, center.y + s),
        ],
        ArrowDir::Right => vec![
            egui::Pos2::new(center.x + s, center.y),
            egui::Pos2::new(center.x - h, center.y - s),
            egui::Pos2::new(center.x - h, center.y + s),
        ],
    };
    painter.add(egui::Shape::convex_polygon(
        pts,
        color,
        egui::Stroke::NONE,
    ));
}

pub fn arrow_indicator(ui: &mut egui::Ui, dir: ArrowDir, pressed: bool) {
    let size = egui::Vec2::new(28.0, 22.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let bg = if pressed {
        egui::Color32::from_gray(170)
    } else {
        egui::Color32::from_gray(35)
    };
    let stroke_color = if pressed {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_gray(85)
    };
    let arrow_color = if pressed {
        egui::Color32::BLACK
    } else {
        egui::Color32::from_gray(200)
    };
    painter.rect_filled(rect, egui::Rounding::same(3.0), bg);
    painter.rect_stroke(
        rect,
        egui::Rounding::same(3.0),
        egui::Stroke::new(1.0, stroke_color),
    );
    paint_arrow(painter, rect.center(), dir, 5.0, arrow_color);
}

// =====================================================================
// Button-style indicators
// =====================================================================

pub fn button_indicator(
    ui: &mut egui::Ui,
    label: &str,
    pressed: bool,
    color: egui::Color32,
) {
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

pub fn fret_indicator(
    ui: &mut egui::Ui,
    color: egui::Color32,
    pressed: bool,
    label: &str,
) {
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

pub fn pad_indicator(
    ui: &mut egui::Ui,
    color: egui::Color32,
    pressed: bool,
    label: &str,
) {
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

pub fn strum_indicator(ui: &mut egui::Ui, up: bool, down: bool) {
    let size = egui::Vec2::new(38.0, 56.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let dim = egui::Color32::from_gray(80);
    let lit = egui::Color32::WHITE;
    paint_arrow(
        painter,
        egui::Pos2::new(rect.center().x, rect.top() + 14.0),
        ArrowDir::Up,
        8.0,
        if up { lit } else { dim },
    );
    paint_arrow(
        painter,
        egui::Pos2::new(rect.center().x, rect.top() + 36.0),
        ArrowDir::Down,
        8.0,
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

pub fn whammy_bar(ui: &mut egui::Ui, value: u8) {
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

pub fn pm_indicator(ui: &mut egui::Ui, plus: bool, minus: bool) {
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

// =====================================================================
// Battery widget — colorised, non-linear mapping (U2)
// =====================================================================

/// Maps the Wiimote's raw battery byte to a percentage. Per WiiBrew the
/// Wiimote reports ~0xC0 (192) when fresh, ~0x33 (51) when low. The
/// previous `byte/255*100` formula made fresh batteries look 75% — the
/// rescale below makes "100 %" actually correspond to a fresh battery.
fn battery_pct(raw: u8) -> u32 {
    let pct = (raw as f32 - 51.0) / (192.0 - 51.0) * 100.0;
    pct.clamp(0.0, 100.0) as u32
}

pub fn battery_widget(ui: &mut egui::Ui, raw: Option<u8>) {
    let (label, color) = match raw {
        None => ("Battery: —".to_string(), egui::Color32::GRAY),
        Some(r) => {
            let pct = battery_pct(r);
            let color = if pct < 20 {
                FRET_RED
            } else if pct < 50 {
                FRET_ORANGE
            } else {
                FRET_GREEN
            };
            (format!("Battery: {pct}%"), color)
        }
    };
    ui.colored_label(color, label);
}

// =====================================================================
// Tilt visualization — small disc showing where gravity points (U3)
// =====================================================================

/// Render a 36-px disc that shows the Wiimote's tilt direction relative
/// to gravity. Replaces the three raw accel numbers with something
/// readable at a glance.
pub fn tilt_widget(ui: &mut egui::Ui, accel: wiimote_core::Accelerometer) {
    let size = egui::Vec2::splat(36.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let radius = rect.width() * 0.46;

    painter.circle_filled(center, radius, egui::Color32::from_gray(28));
    painter.circle_stroke(
        center,
        radius,
        egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
    );

    // Wiimote rests around (512, 512, 612) on (X, Y, Z) when held flat.
    // The marker's planar position represents tilt: ±220 raw at 45°.
    const CENTER: f32 = 512.0;
    const RANGE: f32 = 220.0;
    let dx = ((accel.x as f32 - CENTER) / RANGE).clamp(-1.0, 1.0);
    let dy = ((accel.y as f32 - CENTER) / RANGE).clamp(-1.0, 1.0);
    let marker = egui::Pos2::new(center.x + dx * radius * 0.85, center.y - dy * radius * 0.85);
    painter.circle_filled(marker, 4.0, egui::Color32::from_rgb(180, 220, 255));
    // Crosshair so neutral position is visually obvious.
    painter.line_segment(
        [
            egui::Pos2::new(center.x - radius * 0.3, center.y),
            egui::Pos2::new(center.x + radius * 0.3, center.y),
        ],
        egui::Stroke::new(0.5, egui::Color32::from_gray(60)),
    );
    painter.line_segment(
        [
            egui::Pos2::new(center.x, center.y - radius * 0.3),
            egui::Pos2::new(center.x, center.y + radius * 0.3),
        ],
        egui::Stroke::new(0.5, egui::Color32::from_gray(60)),
    );
}

// =====================================================================
// IR camera visualization — 4 dots on a 1024x768 plane (U4)
// =====================================================================

pub fn ir_widget(ui: &mut egui::Ui, dots: wiimote_core::IrDots) {
    let size = egui::Vec2::new(64.0, 48.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, egui::Rounding::same(2.0), egui::Color32::from_gray(20));
    painter.rect_stroke(
        rect,
        egui::Rounding::same(2.0),
        egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
    );

    let palette = [
        egui::Color32::from_rgb(255, 230, 80),
        egui::Color32::from_rgb(255, 180, 80),
        egui::Color32::from_rgb(255, 130, 80),
        egui::Color32::from_rgb(255, 80, 80),
    ];
    for (i, dot) in dots.iter().enumerate() {
        if !dot.visible {
            continue;
        }
        // Wiimote IR camera reports X 0..=1023 left→right; Y 0..=767
        // top→bottom on the Wii's coordinate system. We mirror X so
        // the on-screen visualization matches what the user sees.
        let nx = 1.0 - (dot.x as f32 / 1023.0).clamp(0.0, 1.0);
        let ny = (dot.y as f32 / 767.0).clamp(0.0, 1.0);
        let px = rect.left() + nx * rect.width();
        let py = rect.top() + ny * rect.height();
        let r = (dot.size.max(1) as f32 * 0.5 + 1.5).min(4.0);
        painter.circle_filled(egui::Pos2::new(px, py), r, palette[i % 4]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_pct_caps_at_100() {
        assert_eq!(battery_pct(0xC0), 100);
        assert_eq!(battery_pct(0xFF), 100);
    }

    #[test]
    fn battery_pct_floors_at_0() {
        assert_eq!(battery_pct(0x00), 0);
        assert_eq!(battery_pct(0x33), 0);
    }

    #[test]
    fn battery_pct_in_between() {
        let mid = battery_pct(0x80);
        assert!((45..=70).contains(&mid));
    }
}
