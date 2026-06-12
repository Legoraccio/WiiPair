//! Footer widgets: battery, tilt disc, IR-camera canvas.
//!
//! These three live in the device-card footer and complement the
//! pictographic body widgets in `device_widgets.rs`. Each one is
//! drawn with painter primitives only — no font glyphs that
//! aren't always present in the bundled egui fonts (▲▼◀▶ / ↑↓←→) —
//! so every install renders the same.

use eframe::egui;

const FRET_RED: egui::Color32 = egui::Color32::from_rgb(225, 70, 70);
const FRET_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 220, 80);
const FRET_ORANGE: egui::Color32 = egui::Color32::from_rgb(235, 145, 60);

// =====================================================================
// Battery widget — colorised, non-linear mapping (U2)
// =====================================================================

/// Maps the Wiimote's raw battery byte to a percentage. Per WiiBrew the
/// Wiimote reports ~0xC0 (192) when fresh, ~0x33 (51) when low. The
/// previous `byte/255*100` formula made fresh batteries look 75% — the
/// rescale below makes "100 %" actually correspond to a fresh battery.
fn battery_pct(raw: u8) -> u32 {
    let pct = (f32::from(raw) - 51.0) / (192.0 - 51.0) * 100.0;
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
    let dx = ((f32::from(accel.x) - CENTER) / RANGE).clamp(-1.0, 1.0);
    let dy = ((f32::from(accel.y) - CENTER) / RANGE).clamp(-1.0, 1.0);
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
        let nx = 1.0 - (f32::from(dot.x) / 1023.0).clamp(0.0, 1.0);
        let ny = (f32::from(dot.y) / 767.0).clamp(0.0, 1.0);
        let px = rect.left() + nx * rect.width();
        let py = rect.top() + ny * rect.height();
        let r = (f32::from(dot.size.max(1)) * 0.5 + 1.5).min(4.0);
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
