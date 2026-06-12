//! Stylised device icons (~28×28).
//!
//! Each icon is intentionally pictographic — a single recognisable
//! silhouette per device family, drawn with painter primitives. We
//! deliberately drop most of the surface detail (player LEDs, fret
//! markers, individual face buttons, …): at this size detail just
//! turns into mush, while a clean silhouette remains readable from
//! across the room.

use eframe::egui;
use wiimote_core::ExtensionType;
use wiimote_daemon::DeviceSnapshot;

/// Body fill used for "neutral" devices (Wiimote, Nunchuk, Classic).
const BODY: egui::Color32 = egui::Color32::from_rgb(235, 235, 235);
/// Outline / detail accent on the body fill.
const DETAIL: egui::Color32 = egui::Color32::from_rgb(45, 45, 45);
/// Edge stroke colour — stays subtle so the silhouette reads as a
/// shape, not a wireframe.
const EDGE: egui::Color32 = egui::Color32::from_gray(140);

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

// =====================================================================
// Wiimote — vertical pill silhouette + dpad + single accent button
// =====================================================================

fn paint_wiimote_icon(p: &egui::Painter, r: egui::Rect) {
    // Body: vertical capsule occupying the full icon. Rounded enough
    // to read as a Wiimote at a glance, narrow enough to leave room
    // for the dpad/A column on the centreline.
    let body = egui::Rect::from_center_size(
        r.center(),
        egui::Vec2::new(r.width() * 0.46, r.height() * 0.96),
    );
    p.rect_filled(body, egui::Rounding::same(r.width() * 0.20), BODY);
    p.rect_stroke(
        body,
        egui::Rounding::same(r.width() * 0.20),
        egui::Stroke::new(1.0, EDGE),
    );

    let cx = r.center().x;
    let dpad_y = body.top() + body.height() * 0.22;
    paint_plus(p, egui::Pos2::new(cx, dpad_y), r.width() * 0.16, DETAIL);

    // A button — the only face control we keep, large enough to read.
    p.circle_filled(
        egui::Pos2::new(cx, body.center().y + body.height() * 0.05),
        r.width() * 0.10,
        DETAIL,
    );
}

// =====================================================================
// Guitar — body + neck silhouette, single sound-hole accent
// =====================================================================

fn paint_guitar_icon(p: &egui::Painter, r: egui::Rect) {
    let body_color = egui::Color32::from_rgb(220, 150, 60);
    let neck_color = egui::Color32::from_rgb(80, 50, 25);

    // Body: a soft round-rect at the lower-right, evoking an
    // electric guitar's lower bout without trying to draw the
    // double-cutaway profile (illegible at 28 px).
    let body = egui::Rect::from_center_size(
        egui::Pos2::new(r.right() - r.width() * 0.26, r.bottom() - r.height() * 0.34),
        egui::Vec2::new(r.width() * 0.55, r.height() * 0.55),
    );
    p.rect_filled(body, egui::Rounding::same(r.width() * 0.22), body_color);

    // Neck: thin diagonal from body up to the top-left, ending in a
    // squared headstock.
    let head = egui::Pos2::new(r.left() + r.width() * 0.14, r.top() + r.height() * 0.16);
    let join = egui::Pos2::new(body.center().x - body.width() * 0.20, body.center().y - body.height() * 0.05);
    paint_thick_line(p, head, join, r.width() * 0.10, neck_color);

    // Headstock cap.
    p.rect_filled(
        egui::Rect::from_center_size(head, egui::Vec2::splat(r.width() * 0.16)),
        egui::Rounding::same(1.5),
        neck_color,
    );

    // Sound hole / pickup — single dark dot at the body centre.
    p.circle_filled(body.center(), r.width() * 0.08, neck_color);
}

// =====================================================================
// Drums — three pads in a row + a kick drum below
// =====================================================================

fn paint_drums_icon(p: &egui::Painter, r: egui::Rect) {
    let pad_color = egui::Color32::from_rgb(220, 220, 230);
    let kick_color = egui::Color32::from_rgb(110, 110, 120);

    // Three pads aligned across the upper third.
    let pad_y = r.top() + r.height() * 0.30;
    let pad_radius = r.width() * 0.10;
    for i in 0..3 {
        let t = (i as f32 + 0.5) / 3.0;
        let cx = r.left() + t * r.width();
        p.circle_filled(egui::Pos2::new(cx, pad_y), pad_radius, pad_color);
        p.circle_stroke(
            egui::Pos2::new(cx, pad_y),
            pad_radius,
            egui::Stroke::new(1.0, DETAIL),
        );
    }

    // Kick drum: wider rounded rectangle hugging the bottom edge.
    let kick = egui::Rect::from_center_size(
        egui::Pos2::new(r.center().x, r.bottom() - r.height() * 0.20),
        egui::Vec2::new(r.width() * 0.78, r.height() * 0.30),
    );
    p.rect_filled(kick, egui::Rounding::same(r.width() * 0.06), kick_color);
    // Inner ring to suggest a head.
    p.circle_stroke(
        kick.center(),
        kick.height() * 0.32,
        egui::Stroke::new(1.0, BODY),
    );
}

// =====================================================================
// Nunchuk — ergonomic pebble with stick on top
// =====================================================================

fn paint_nunchuk_icon(p: &egui::Painter, r: egui::Rect) {
    // Body: tall pill, slightly narrower than the Wiimote so the two
    // are distinguishable side-by-side.
    let body = egui::Rect::from_center_size(
        egui::Pos2::new(r.center().x, r.center().y + r.height() * 0.05),
        egui::Vec2::new(r.width() * 0.40, r.height() * 0.78),
    );
    p.rect_filled(body, egui::Rounding::same(r.width() * 0.18), BODY);
    p.rect_stroke(
        body,
        egui::Rounding::same(r.width() * 0.18),
        egui::Stroke::new(1.0, EDGE),
    );

    // Stick base (large filled circle near the top of the body).
    let stick_center = egui::Pos2::new(body.center().x, body.top() + body.height() * 0.18);
    p.circle_filled(stick_center, r.width() * 0.13, DETAIL);
    // Stick cap on top of the base.
    p.circle_filled(
        egui::Pos2::new(stick_center.x, stick_center.y - r.height() * 0.12),
        r.width() * 0.09,
        BODY,
    );
    p.circle_stroke(
        egui::Pos2::new(stick_center.x, stick_center.y - r.height() * 0.12),
        r.width() * 0.09,
        egui::Stroke::new(1.0, EDGE),
    );
}

// =====================================================================
// Classic Controller — gamepad silhouette (centre body + two grips)
// =====================================================================

fn paint_classic_icon(p: &egui::Painter, r: egui::Rect) {
    // Centre body.
    let body = egui::Rect::from_center_size(
        r.center(),
        egui::Vec2::new(r.width() * 0.70, r.height() * 0.46),
    );
    p.rect_filled(body, egui::Rounding::same(r.width() * 0.10), BODY);

    // Two grip lobes hanging below the body.
    for cx_t in [0.22, 0.78] {
        let lobe = egui::Rect::from_center_size(
            egui::Pos2::new(r.left() + cx_t * r.width(), r.center().y + r.height() * 0.22),
            egui::Vec2::new(r.width() * 0.30, r.height() * 0.42),
        );
        p.rect_filled(lobe, egui::Rounding::same(r.width() * 0.14), BODY);
    }

    // D-pad (left) + face buttons (right) as monochrome accents.
    let dpad_c = egui::Pos2::new(r.left() + r.width() * 0.27, r.center().y);
    paint_plus(p, dpad_c, r.width() * 0.10, DETAIL);

    let buttons_c = egui::Pos2::new(r.right() - r.width() * 0.27, r.center().y);
    let br = r.width() * 0.045;
    let off = r.width() * 0.10;
    p.circle_filled(egui::Pos2::new(buttons_c.x, buttons_c.y - off), br, DETAIL);
    p.circle_filled(egui::Pos2::new(buttons_c.x, buttons_c.y + off), br, DETAIL);
    p.circle_filled(egui::Pos2::new(buttons_c.x - off, buttons_c.y), br, DETAIL);
    p.circle_filled(egui::Pos2::new(buttons_c.x + off, buttons_c.y), br, DETAIL);
}

// =====================================================================
// Turntable — vinyl disc + tonearm
// =====================================================================

fn paint_turntable_icon(p: &egui::Painter, r: egui::Rect) {
    let plate_color = egui::Color32::from_rgb(35, 35, 40);
    let label_color = egui::Color32::from_rgb(220, 80, 100);

    // Vinyl disc dominating the icon.
    let center = r.center();
    let radius = r.width() * 0.40;
    p.circle_filled(center, radius, plate_color);
    p.circle_stroke(center, radius, egui::Stroke::new(1.0, EDGE));
    // Concentric groove hint.
    p.circle_stroke(
        center,
        radius * 0.72,
        egui::Stroke::new(0.6, egui::Color32::from_gray(70)),
    );
    // Centre label.
    p.circle_filled(center, radius * 0.30, label_color);
    p.circle_filled(center, radius * 0.06, plate_color);

    // Tonearm: thin diagonal from the upper-right inward.
    let arm_a = egui::Pos2::new(r.right() - 1.5, r.top() + r.height() * 0.18);
    let arm_b = egui::Pos2::new(center.x + radius * 0.55, center.y - radius * 0.10);
    p.line_segment([arm_a, arm_b], egui::Stroke::new(1.4, BODY));
    p.circle_filled(arm_a, 1.8, BODY);
    p.circle_filled(arm_b, 1.8, BODY);
}

// =====================================================================
// Helpers
// =====================================================================

/// Filled "+" shape (used for d-pads). `arm` is the half-length of
/// each arm; thickness is derived from it so the cross is balanced.
fn paint_plus(p: &egui::Painter, center: egui::Pos2, arm: f32, color: egui::Color32) {
    let thickness = arm * 0.50;
    p.rect_filled(
        egui::Rect::from_center_size(center, egui::Vec2::new(arm * 2.0, thickness)),
        egui::Rounding::ZERO,
        color,
    );
    p.rect_filled(
        egui::Rect::from_center_size(center, egui::Vec2::new(thickness, arm * 2.0)),
        egui::Rounding::ZERO,
        color,
    );
}

/// Thick rounded line between two points — used for the guitar neck.
/// We approximate it with a stroked segment plus rounded end-caps so
/// the join with the headstock and body looks intentional rather
/// than abrupt.
fn paint_thick_line(
    p: &egui::Painter,
    a: egui::Pos2,
    b: egui::Pos2,
    width: f32,
    color: egui::Color32,
) {
    p.line_segment([a, b], egui::Stroke::new(width, color));
    p.circle_filled(a, width * 0.5, color);
    p.circle_filled(b, width * 0.5, color);
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
