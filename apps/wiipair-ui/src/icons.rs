//! Programmatically painted device icons (~28×28).

use eframe::egui;
use wiimote_core::ExtensionType;
use wiimote_daemon::DeviceSnapshot;

use crate::widgets::{FRET_BLUE, FRET_GREEN, FRET_ORANGE, FRET_RED, FRET_YELLOW};

pub fn draw_device_icon(ui: &mut egui::Ui, d: &DeviceSnapshot) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(28.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    match d.extension {
        Some(ExtensionType::Guitar) => paint_guitar_icon(&painter, rect),
        Some(ExtensionType::Drums) => paint_drums_icon(&painter, rect),
        Some(ExtensionType::Nunchuk) => paint_nunchuk_icon(&painter, rect),
        Some(ExtensionType::ClassicController | ExtensionType::ClassicControllerPro) => {
            paint_classic_icon(&painter, rect);
        }
        Some(ExtensionType::DjHeroTurntable) => paint_turntable_icon(&painter, rect),
        _ => paint_wiimote_icon(&painter, rect),
    }
}

fn paint_wiimote_icon(p: &egui::Painter, r: egui::Rect) {
    let body = egui::Color32::from_gray(235);
    let detail = egui::Color32::from_gray(60);
    p.rect_filled(r, egui::Rounding::same(4.0), body);
    p.rect_stroke(
        r,
        egui::Rounding::same(4.0),
        egui::Stroke::new(1.0, egui::Color32::from_gray(140)),
    );
    let cx = r.center().x;
    let dpad = egui::Pos2::new(cx, r.top() + r.height() * 0.22);
    let arm = r.height() * 0.05;
    let span = r.width() * 0.30;
    p.rect_filled(
        egui::Rect::from_center_size(dpad, egui::Vec2::new(span, arm * 2.0)),
        egui::Rounding::ZERO,
        detail,
    );
    p.rect_filled(
        egui::Rect::from_center_size(dpad, egui::Vec2::new(arm * 2.0, span)),
        egui::Rounding::ZERO,
        detail,
    );
    p.circle_filled(
        egui::Pos2::new(cx, r.center().y + r.height() * 0.05),
        r.width() * 0.13,
        detail,
    );
    p.circle_filled(
        egui::Pos2::new(cx, r.bottom() - r.height() * 0.25),
        r.width() * 0.06,
        detail,
    );
    p.circle_filled(
        egui::Pos2::new(cx, r.bottom() - r.height() * 0.12),
        r.width() * 0.06,
        detail,
    );
}

fn paint_guitar_icon(p: &egui::Painter, r: egui::Rect) {
    let body_color = egui::Color32::from_rgb(200, 140, 60);
    let neck_color = egui::Color32::from_rgb(80, 50, 20);
    let body_rect = egui::Rect::from_center_size(
        egui::Pos2::new(r.right() - r.width() * 0.22, r.center().y),
        egui::Vec2::new(r.width() * 0.45, r.height() * 0.7),
    );
    p.rect_filled(body_rect, egui::Rounding::same(8.0), body_color);
    let neck_rect = egui::Rect::from_min_max(
        egui::Pos2::new(r.left() + r.width() * 0.05, r.center().y - r.height() * 0.08),
        egui::Pos2::new(body_rect.left() + 4.0, r.center().y + r.height() * 0.08),
    );
    p.rect_filled(neck_rect, egui::Rounding::ZERO, neck_color);
    let fret_colors = [FRET_GREEN, FRET_RED, FRET_YELLOW, FRET_BLUE, FRET_ORANGE];
    let n = fret_colors.len();
    for (i, c) in fret_colors.iter().enumerate() {
        let t = (i as f32 + 0.5) / n as f32;
        let x = neck_rect.left() + t * neck_rect.width();
        p.circle_filled(egui::Pos2::new(x, neck_rect.center().y), 1.6, *c);
    }
    p.circle_stroke(
        body_rect.center(),
        r.width() * 0.10,
        egui::Stroke::new(1.0, neck_color),
    );
}

fn paint_drums_icon(p: &egui::Painter, r: egui::Rect) {
    let pads = [FRET_RED, FRET_YELLOW, FRET_BLUE, FRET_GREEN];
    let n = pads.len();
    let pad_y = r.top() + r.height() * 0.38;
    let pad_size = r.width() * 0.09;
    for (i, c) in pads.iter().enumerate() {
        let t = (i as f32 + 0.5) / n as f32;
        let x = r.left() + t * r.width();
        p.circle_filled(egui::Pos2::new(x, pad_y), pad_size, *c);
    }
    p.rect_filled(
        egui::Rect::from_center_size(
            egui::Pos2::new(r.center().x, r.bottom() - r.height() * 0.20),
            egui::Vec2::new(r.width() * 0.55, r.height() * 0.18),
        ),
        egui::Rounding::same(2.0),
        egui::Color32::from_gray(110),
    );
}

fn paint_nunchuk_icon(p: &egui::Painter, r: egui::Rect) {
    let body = egui::Color32::from_gray(235);
    let detail = egui::Color32::from_gray(80);
    let body_rect = egui::Rect::from_center_size(
        r.center(),
        egui::Vec2::new(r.width() * 0.45, r.height() * 0.85),
    );
    p.rect_filled(body_rect, egui::Rounding::same(6.0), body);
    p.rect_stroke(
        body_rect,
        egui::Rounding::same(6.0),
        egui::Stroke::new(1.0, egui::Color32::from_gray(140)),
    );
    p.circle_filled(
        egui::Pos2::new(r.center().x, r.top() + r.height() * 0.16),
        r.width() * 0.11,
        detail,
    );
    p.circle_filled(
        egui::Pos2::new(r.center().x, r.center().y + r.height() * 0.08),
        r.width() * 0.05,
        detail,
    );
    p.circle_filled(
        egui::Pos2::new(r.center().x, r.center().y + r.height() * 0.22),
        r.width() * 0.05,
        detail,
    );
}

fn paint_classic_icon(p: &egui::Painter, r: egui::Rect) {
    let body = egui::Color32::from_gray(230);
    p.rect_filled(r, egui::Rounding::same(5.0), body);
    p.rect_stroke(
        r,
        egui::Rounding::same(5.0),
        egui::Stroke::new(1.0, egui::Color32::from_gray(140)),
    );
    let detail = egui::Color32::from_gray(60);
    let dpad = egui::Pos2::new(r.left() + r.width() * 0.28, r.center().y);
    let arm = r.height() * 0.05;
    let span = r.width() * 0.20;
    p.rect_filled(
        egui::Rect::from_center_size(dpad, egui::Vec2::new(span, arm * 2.0)),
        egui::Rounding::ZERO,
        detail,
    );
    p.rect_filled(
        egui::Rect::from_center_size(dpad, egui::Vec2::new(arm * 2.0, span)),
        egui::Rounding::ZERO,
        detail,
    );
    let bx = r.right() - r.width() * 0.28;
    let by = r.center().y;
    p.circle_filled(
        egui::Pos2::new(bx, by - r.height() * 0.10),
        r.width() * 0.05,
        FRET_RED,
    );
    p.circle_filled(
        egui::Pos2::new(bx, by + r.height() * 0.10),
        r.width() * 0.05,
        FRET_BLUE,
    );
    p.circle_filled(
        egui::Pos2::new(bx + r.width() * 0.10, by),
        r.width() * 0.05,
        FRET_GREEN,
    );
    p.circle_filled(
        egui::Pos2::new(bx - r.width() * 0.10, by),
        r.width() * 0.05,
        FRET_YELLOW,
    );
}

fn paint_turntable_icon(p: &egui::Painter, r: egui::Rect) {
    let body = egui::Color32::from_gray(40);
    p.rect_filled(r, egui::Rounding::same(4.0), body);
    p.circle_filled(r.center(), r.width() * 0.36, egui::Color32::from_gray(20));
    p.circle_stroke(
        r.center(),
        r.width() * 0.36,
        egui::Stroke::new(1.0, egui::Color32::from_gray(120)),
    );
    p.circle_filled(r.center(), r.width() * 0.07, FRET_RED);
}

pub fn extension_color(ext: ExtensionType) -> egui::Color32 {
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
