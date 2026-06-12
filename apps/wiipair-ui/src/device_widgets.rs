//! Pictographic device widgets — Wiimote, Nunchuk, Classic, Guitar,
//! Drums. Each function paints a stylised silhouette of the
//! corresponding controller and lights up the inputs that are
//! currently pressed, mirroring the approach taken by
//! [`crate::xbox_widget`] for the output preview.

use eframe::egui;
use wiimote_core::{
    Buttons, ClassicButtons, ClassicState, DrumsButtons, DrumsState, GuitarButtons, GuitarState,
    NunchukState,
};
use wiimote_daemon::DeviceSnapshot;

/// Wiimote silhouette is taller-than-wide; everything else gets a
/// landscape-ish slot so they line up with the Xbox preview to the
/// right of the card.
pub const WIIMOTE_SIZE: egui::Vec2 = egui::Vec2::new(110.0, 160.0);
pub const NUNCHUK_SIZE: egui::Vec2 = egui::Vec2::new(150.0, 160.0);
pub const CLASSIC_SIZE: egui::Vec2 = egui::Vec2::new(220.0, 160.0);
pub const GUITAR_SIZE: egui::Vec2 = egui::Vec2::new(220.0, 160.0);
pub const DRUMS_SIZE: egui::Vec2 = egui::Vec2::new(220.0, 160.0);

const BODY_LIGHT: egui::Color32 = egui::Color32::from_rgb(235, 235, 235);
const BODY_DARK: egui::Color32 = egui::Color32::from_rgb(40, 40, 45);
const BODY_EDGE: egui::Color32 = egui::Color32::from_rgb(95, 95, 105);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(180, 180, 190);
const ACCENT_OFF: egui::Color32 = egui::Color32::from_rgb(70, 75, 80);

const A_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 220, 100);
const B_RED: egui::Color32 = egui::Color32::from_rgb(220, 75, 75);
const X_BLUE: egui::Color32 = egui::Color32::from_rgb(70, 140, 230);
const Y_YELLOW: egui::Color32 = egui::Color32::from_rgb(230, 200, 70);

const FRET_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 220, 80);
const FRET_RED: egui::Color32 = egui::Color32::from_rgb(225, 70, 70);
const FRET_YELLOW: egui::Color32 = egui::Color32::from_rgb(235, 215, 70);
const FRET_BLUE: egui::Color32 = egui::Color32::from_rgb(80, 140, 235);
const FRET_ORANGE: egui::Color32 = egui::Color32::from_rgb(235, 145, 60);
const BASS_COLOR: egui::Color32 = egui::Color32::from_rgb(140, 140, 140);

// =====================================================================
// Wiimote — vertical silhouette + dpad / A / B / 1+2 / +/Home/− / LEDs
// =====================================================================

pub fn paint_wiimote(ui: &mut egui::Ui, d: &DeviceSnapshot) {
    let (rect, _) = ui.allocate_exact_size(WIIMOTE_SIZE, egui::Sense::hover());
    let p = ui.painter_at(rect);
    let b = d.last_buttons;

    // Body — vertical capsule.
    let body = egui::Rect::from_center_size(
        rect.center(),
        egui::Vec2::new(rect.width() * 0.62, rect.height() * 0.96),
    );
    p.rect_filled(body, egui::Rounding::same(rect.width() * 0.18), BODY_LIGHT);
    p.rect_stroke(
        body,
        egui::Rounding::same(rect.width() * 0.18),
        egui::Stroke::new(1.0, BODY_EDGE),
    );

    let cx = body.center().x;

    // D-pad (top of body).
    let dpad_c = egui::Pos2::new(cx, body.top() + body.height() * 0.13);
    paint_dpad(
        &p,
        dpad_c,
        body.width() * 0.20,
        b.contains(Buttons::UP),
        b.contains(Buttons::DOWN),
        b.contains(Buttons::LEFT),
        b.contains(Buttons::RIGHT),
    );

    // A button — large green disc just below the dpad.
    let a_c = egui::Pos2::new(cx, body.top() + body.height() * 0.30);
    paint_disc(&p, a_c, body.width() * 0.18, "A", b.contains(Buttons::A), A_GREEN);

    // +, Home, − cluster.
    paint_pill(
        &p,
        egui::Pos2::new(cx - body.width() * 0.22, body.top() + body.height() * 0.42),
        "+",
        b.contains(Buttons::PLUS),
    );
    paint_pill(
        &p,
        egui::Pos2::new(cx + body.width() * 0.22, body.top() + body.height() * 0.42),
        "−",
        b.contains(Buttons::MINUS),
    );
    let home_c = egui::Pos2::new(cx, body.top() + body.height() * 0.48);
    paint_disc(&p, home_c, body.width() * 0.10, "⌂", b.contains(Buttons::HOME), B_RED);

    // 1 / 2 buttons.
    paint_pill(
        &p,
        egui::Pos2::new(cx, body.top() + body.height() * 0.60),
        "1",
        b.contains(Buttons::ONE),
    );
    paint_pill(
        &p,
        egui::Pos2::new(cx, body.top() + body.height() * 0.69),
        "2",
        b.contains(Buttons::TWO),
    );

    // B trigger — drawn as a small chip on the left edge to convey
    // "back-side trigger" without leaving its state invisible.
    let b_chip = egui::Rect::from_center_size(
        egui::Pos2::new(body.left() - 4.0, body.center().y),
        egui::Vec2::new(8.0, 24.0),
    );
    paint_chip(&p, b_chip, "B", b.contains(Buttons::B), B_RED);

    // Player slot LEDs along the bottom.
    let led_y = body.bottom() - body.height() * 0.06;
    for i in 0..4 {
        let t = (i as f32 + 0.5) / 4.0;
        let cx_led = body.left() + t * body.width();
        let on = false; // We don't surface the slot byte to the UI yet.
        paint_led(&p, egui::Pos2::new(cx_led, led_y), on);
    }
}

// =====================================================================
// Nunchuk — body + stick disc + C / Z triggers
// =====================================================================

pub fn paint_nunchuk(ui: &mut egui::Ui, n: &NunchukState) {
    let (rect, _) = ui.allocate_exact_size(NUNCHUK_SIZE, egui::Sense::hover());
    let p = ui.painter_at(rect);

    // Body — pebble shape on the right side, narrower neck on top.
    let body = egui::Rect::from_center_size(
        egui::Pos2::new(
            rect.left() + rect.width() * 0.50,
            rect.center().y + rect.height() * 0.08,
        ),
        egui::Vec2::new(rect.width() * 0.55, rect.height() * 0.78),
    );
    p.rect_filled(body, egui::Rounding::same(rect.width() * 0.16), BODY_LIGHT);
    p.rect_stroke(
        body,
        egui::Rounding::same(rect.width() * 0.16),
        egui::Stroke::new(1.0, BODY_EDGE),
    );

    // Stick (top of body).
    let stick_c = egui::Pos2::new(body.center().x, body.top() + body.height() * 0.22);
    paint_stick(&p, stick_c, body.width() * 0.30, n.stick_x, n.stick_y);

    // C / Z triggers — "C" on top edge, "Z" on the left edge as the
    // index-finger trigger. Both painted as chips with the same
    // visual language as the Wiimote's B trigger.
    let c_chip = egui::Rect::from_center_size(
        egui::Pos2::new(body.center().x, body.top() - 5.0),
        egui::Vec2::new(28.0, 8.0),
    );
    paint_chip(&p, c_chip, "C", n.c, A_GREEN);

    let z_chip = egui::Rect::from_center_size(
        egui::Pos2::new(body.left() - 4.0, body.top() + body.height() * 0.30),
        egui::Vec2::new(8.0, 22.0),
    );
    paint_chip(&p, z_chip, "Z", n.z, B_RED);

    // Stick raw values (small monospace, helps debugging).
    p.text(
        egui::Pos2::new(body.center().x, body.bottom() - 12.0),
        egui::Align2::CENTER_CENTER,
        format!("{:>3},{:>3}", n.stick_x, n.stick_y),
        egui::FontId::monospace(9.0),
        TEXT_DIM,
    );
}

// =====================================================================
// Classic Controller — gamepad silhouette + dpad / face / shoulders /
// triggers / start-back-home cluster
// =====================================================================

pub fn paint_classic(ui: &mut egui::Ui, c: &ClassicState) {
    let (rect, _) = ui.allocate_exact_size(CLASSIC_SIZE, egui::Sense::hover());
    let p = ui.painter_at(rect);
    let b = c.buttons;

    // Body shape — central rect + two grip lobes (Xbox-ish silhouette
    // but with the Classic Controller's lighter colour).
    let centre = rect.center();
    let body = egui::Rect::from_center_size(
        egui::Pos2::new(centre.x, centre.y),
        egui::Vec2::new(rect.width() * 0.86, rect.height() * 0.50),
    );
    p.rect_filled(body, egui::Rounding::same(rect.height() * 0.18), BODY_LIGHT);
    p.rect_stroke(
        body,
        egui::Rounding::same(rect.height() * 0.18),
        egui::Stroke::new(1.0, BODY_EDGE),
    );
    for cx_t in [0.18, 0.82] {
        let lobe = egui::Rect::from_center_size(
            egui::Pos2::new(rect.left() + cx_t * rect.width(), centre.y + rect.height() * 0.20),
            egui::Vec2::new(rect.width() * 0.30, rect.height() * 0.40),
        );
        p.rect_filled(lobe, egui::Rounding::same(rect.height() * 0.20), BODY_LIGHT);
        p.rect_stroke(
            lobe,
            egui::Rounding::same(rect.height() * 0.20),
            egui::Stroke::new(1.0, BODY_EDGE),
        );
    }

    // Shoulders (top edge): L outer, ZL inner on the left; ZR inner,
    // R outer on the right.
    let bumper_y = body.top() - 4.0;
    let bw = rect.width() * 0.10;
    let bh = 8.0;
    paint_chip(
        &p,
        egui::Rect::from_center_size(
            egui::Pos2::new(rect.left() + rect.width() * 0.18, bumper_y),
            egui::Vec2::new(bw, bh),
        ),
        "L",
        b.contains(ClassicButtons::LT),
        ACCENT_OFF,
    );
    paint_chip(
        &p,
        egui::Rect::from_center_size(
            egui::Pos2::new(rect.left() + rect.width() * 0.30, bumper_y),
            egui::Vec2::new(bw, bh),
        ),
        "ZL",
        b.contains(ClassicButtons::ZL),
        ACCENT_OFF,
    );
    paint_chip(
        &p,
        egui::Rect::from_center_size(
            egui::Pos2::new(rect.right() - rect.width() * 0.30, bumper_y),
            egui::Vec2::new(bw, bh),
        ),
        "ZR",
        b.contains(ClassicButtons::ZR),
        ACCENT_OFF,
    );
    paint_chip(
        &p,
        egui::Rect::from_center_size(
            egui::Pos2::new(rect.right() - rect.width() * 0.18, bumper_y),
            egui::Vec2::new(bw, bh),
        ),
        "R",
        b.contains(ClassicButtons::RT),
        ACCENT_OFF,
    );

    // D-pad on the left.
    let dpad_c = egui::Pos2::new(
        rect.left() + rect.width() * 0.25,
        body.top() + body.height() * 0.45,
    );
    paint_dpad(
        &p,
        dpad_c,
        body.height() * 0.14,
        b.contains(ClassicButtons::DPAD_UP),
        b.contains(ClassicButtons::DPAD_DOWN),
        b.contains(ClassicButtons::DPAD_LEFT),
        b.contains(ClassicButtons::DPAD_RIGHT),
    );

    // Face buttons (Wii Classic disposition: A right-bottom, B
    // bottom, X top, Y left).
    let face_c = egui::Pos2::new(
        rect.right() - rect.width() * 0.25,
        body.top() + body.height() * 0.45,
    );
    let r = body.height() * 0.14;
    let off = body.height() * 0.22;
    paint_disc(
        &p,
        egui::Pos2::new(face_c.x + off, face_c.y),
        r,
        "A",
        b.contains(ClassicButtons::A),
        A_GREEN,
    );
    paint_disc(
        &p,
        egui::Pos2::new(face_c.x, face_c.y + off),
        r,
        "B",
        b.contains(ClassicButtons::B),
        B_RED,
    );
    paint_disc(
        &p,
        egui::Pos2::new(face_c.x, face_c.y - off),
        r,
        "X",
        b.contains(ClassicButtons::X),
        X_BLUE,
    );
    paint_disc(
        &p,
        egui::Pos2::new(face_c.x - off, face_c.y),
        r,
        "Y",
        b.contains(ClassicButtons::Y),
        Y_YELLOW,
    );

    // Centre cluster: Minus / Home / Plus.
    let centre_y = body.bottom() - 14.0;
    paint_pill(
        &p,
        egui::Pos2::new(centre.x - 22.0, centre_y),
        "−",
        b.contains(ClassicButtons::MINUS),
    );
    paint_pill(
        &p,
        egui::Pos2::new(centre.x, centre_y),
        "⌂",
        b.contains(ClassicButtons::HOME),
    );
    paint_pill(
        &p,
        egui::Pos2::new(centre.x + 22.0, centre_y),
        "+",
        b.contains(ClassicButtons::PLUS),
    );
}

// =====================================================================
// Guitar — body + neck + 5 colour-coded frets + strum bar + whammy
// =====================================================================

pub fn paint_guitar(ui: &mut egui::Ui, g: &GuitarState) {
    let (rect, _) = ui.allocate_exact_size(GUITAR_SIZE, egui::Sense::hover());
    let p = ui.painter_at(rect);
    let b = g.buttons;

    // Body (lower-right).
    let body = egui::Rect::from_center_size(
        egui::Pos2::new(rect.right() - rect.width() * 0.18, rect.center().y),
        egui::Vec2::new(rect.width() * 0.32, rect.height() * 0.78),
    );
    p.rect_filled(
        body,
        egui::Rounding::same(rect.height() * 0.30),
        egui::Color32::from_rgb(220, 150, 60),
    );

    // Neck — long horizontal bar from the left edge into the body.
    let neck = egui::Rect::from_min_max(
        egui::Pos2::new(
            rect.left() + 6.0,
            rect.center().y - rect.height() * 0.10,
        ),
        egui::Pos2::new(body.left() + 6.0, rect.center().y + rect.height() * 0.10),
    );
    p.rect_filled(neck, egui::Rounding::same(2.0), BODY_DARK);

    // 5 frets along the neck.
    let fret_colors = [FRET_GREEN, FRET_RED, FRET_YELLOW, FRET_BLUE, FRET_ORANGE];
    let fret_buttons = [
        GuitarButtons::GREEN,
        GuitarButtons::RED,
        GuitarButtons::YELLOW,
        GuitarButtons::BLUE,
        GuitarButtons::ORANGE,
    ];
    let n = fret_colors.len();
    for (i, (color, flag)) in fret_colors.iter().zip(fret_buttons.iter()).enumerate() {
        let t = (i as f32 + 0.5) / n as f32;
        let cx = neck.left() + t * neck.width();
        let cy = neck.center().y;
        let pressed = b.contains(*flag);
        let r = neck.height() * 0.34;
        if pressed {
            p.circle_filled(egui::Pos2::new(cx, cy), r, *color);
            p.circle_stroke(
                egui::Pos2::new(cx, cy),
                r,
                egui::Stroke::new(1.5, egui::Color32::WHITE),
            );
        } else {
            p.circle_filled(egui::Pos2::new(cx, cy), r, color.linear_multiply(0.20));
            p.circle_stroke(egui::Pos2::new(cx, cy), r, egui::Stroke::new(1.0, *color));
        }
    }

    // Strum bar — twin arrows on the body, just left of centre.
    let strum_c = egui::Pos2::new(body.left() + body.width() * 0.30, body.center().y);
    paint_strum(&p, strum_c, 14.0, b.contains(GuitarButtons::STRUM_UP), b.contains(GuitarButtons::STRUM_DOWN));

    // Whammy — vertical fill bar on the right side of the body.
    let whammy_rect = egui::Rect::from_center_size(
        egui::Pos2::new(body.right() - 12.0, body.center().y),
        egui::Vec2::new(8.0, body.height() * 0.55),
    );
    let pct = (f32::from(g.whammy) / 31.0).clamp(0.0, 1.0);
    p.rect_filled(whammy_rect, egui::Rounding::same(2.0), ACCENT_OFF);
    if pct > 0.0 {
        let fill_h = whammy_rect.height() * pct;
        let fill = egui::Rect::from_min_max(
            egui::Pos2::new(whammy_rect.left(), whammy_rect.bottom() - fill_h),
            whammy_rect.max,
        );
        p.rect_filled(fill, egui::Rounding::same(2.0), egui::Color32::from_rgb(220, 90, 200));
    }
    p.rect_stroke(whammy_rect, egui::Rounding::same(2.0), egui::Stroke::new(1.0, BODY_EDGE));
    p.text(
        egui::Pos2::new(whammy_rect.center().x, whammy_rect.top() - 6.0),
        egui::Align2::CENTER_CENTER,
        "W",
        egui::FontId::proportional(8.0),
        TEXT_DIM,
    );

    // +/− pills near the body (mirroring the real guitar's shoulder).
    paint_pill(
        &p,
        egui::Pos2::new(body.center().x - 6.0, body.bottom() - 12.0),
        "−",
        b.contains(GuitarButtons::MINUS),
    );
    paint_pill(
        &p,
        egui::Pos2::new(body.center().x + 18.0, body.bottom() - 12.0),
        "+",
        b.contains(GuitarButtons::PLUS),
    );
}

fn paint_strum(p: &egui::Painter, centre: egui::Pos2, half: f32, up: bool, down: bool) {
    let dim = egui::Color32::from_gray(70);
    let lit = egui::Color32::WHITE;
    let arrow = |dir_up: bool, lit_now: bool| {
        let color = if lit_now { lit } else { dim };
        let tip_y = if dir_up { centre.y - half } else { centre.y + half };
        let base_y = if dir_up { centre.y - half * 0.1 } else { centre.y + half * 0.1 };
        let pts = vec![
            egui::Pos2::new(centre.x, tip_y),
            egui::Pos2::new(centre.x - half * 0.7, base_y),
            egui::Pos2::new(centre.x + half * 0.7, base_y),
        ];
        p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
    };
    arrow(true, up);
    arrow(false, down);
}

// =====================================================================
// Drums — 4 coloured pads + cymbal + bass pedal + +/−
// =====================================================================

pub fn paint_drums(ui: &mut egui::Ui, dr: &DrumsState) {
    let (rect, _) = ui.allocate_exact_size(DRUMS_SIZE, egui::Sense::hover());
    let p = ui.painter_at(rect);
    let b = dr.buttons;

    // 5 round pads in a wide arc across the top half. Layout:
    // GREEN | RED | YELLOW | BLUE | ORANGE-cymbal.
    let pads = [
        (FRET_RED, "R", DrumsButtons::RED),
        (FRET_YELLOW, "Y", DrumsButtons::YELLOW),
        (FRET_BLUE, "B", DrumsButtons::BLUE),
        (FRET_GREEN, "G", DrumsButtons::GREEN),
        (FRET_ORANGE, "O", DrumsButtons::ORANGE),
    ];
    let n = pads.len();
    let pad_y = rect.top() + rect.height() * 0.32;
    let pad_r = rect.height() * 0.15;
    for (i, (color, label, flag)) in pads.iter().enumerate() {
        let t = (i as f32 + 0.5) / n as f32;
        let cx = rect.left() + t * rect.width();
        let pressed = b.contains(*flag);
        let centre = egui::Pos2::new(cx, pad_y);
        if pressed {
            p.circle_filled(centre, pad_r, *color);
            p.circle_stroke(centre, pad_r, egui::Stroke::new(2.0, egui::Color32::WHITE));
            p.text(
                centre,
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(pad_r * 0.9),
                egui::Color32::BLACK,
            );
        } else {
            p.circle_filled(centre, pad_r, color.linear_multiply(0.18));
            p.circle_stroke(centre, pad_r, egui::Stroke::new(1.5, *color));
            p.text(
                centre,
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(pad_r * 0.9),
                *color,
            );
        }
    }

    // Bass pedal — wide rectangle along the bottom.
    let bass = egui::Rect::from_center_size(
        egui::Pos2::new(rect.center().x, rect.bottom() - rect.height() * 0.20),
        egui::Vec2::new(rect.width() * 0.70, rect.height() * 0.18),
    );
    let bass_pressed = b.contains(DrumsButtons::BASS_PEDAL);
    if bass_pressed {
        p.rect_filled(bass, egui::Rounding::same(4.0), BASS_COLOR);
        p.rect_stroke(
            bass,
            egui::Rounding::same(4.0),
            egui::Stroke::new(2.0, egui::Color32::WHITE),
        );
    } else {
        p.rect_filled(bass, egui::Rounding::same(4.0), BASS_COLOR.linear_multiply(0.20));
        p.rect_stroke(bass, egui::Rounding::same(4.0), egui::Stroke::new(1.5, BASS_COLOR));
    }
    p.text(
        bass.center(),
        egui::Align2::CENTER_CENTER,
        "BASS",
        egui::FontId::proportional(10.0),
        if bass_pressed { egui::Color32::BLACK } else { BASS_COLOR },
    );

    // +/− pills on the right edge.
    paint_pill(
        &p,
        egui::Pos2::new(rect.right() - 14.0, rect.top() + 14.0),
        "−",
        b.contains(DrumsButtons::MINUS),
    );
    paint_pill(
        &p,
        egui::Pos2::new(rect.right() - 14.0, rect.top() + 32.0),
        "+",
        b.contains(DrumsButtons::PLUS),
    );
}

// =====================================================================
// "No extension" placeholder so the column stays consistent.
// =====================================================================

pub fn paint_no_extension(ui: &mut egui::Ui, label: &str) {
    let (rect, _) = ui.allocate_exact_size(NUNCHUK_SIZE, egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_stroke(
        rect.shrink(2.0),
        egui::Rounding::same(8.0),
        egui::Stroke::new(1.0, BODY_EDGE),
    );
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(12.0),
        TEXT_DIM,
    );
}

// =====================================================================
// Painter helpers — small reusable primitives.
// =====================================================================

fn paint_disc(
    p: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    label: &str,
    on: bool,
    accent: egui::Color32,
) {
    let fill = if on { accent } else { accent.linear_multiply(0.18) };
    let stroke = if on { egui::Color32::WHITE } else { accent.linear_multiply(0.6) };
    p.circle_filled(c, r, fill);
    p.circle_stroke(c, r, egui::Stroke::new(1.2, stroke));
    p.text(
        c,
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(r * 1.2),
        if on { egui::Color32::BLACK } else { accent },
    );
}

fn paint_pill(p: &egui::Painter, c: egui::Pos2, label: &str, on: bool) {
    let r = egui::Rect::from_center_size(c, egui::Vec2::new(20.0, 11.0));
    let fill = if on {
        egui::Color32::from_rgb(220, 220, 230)
    } else {
        ACCENT_OFF
    };
    p.rect_filled(r, egui::Rounding::same(5.5), fill);
    p.rect_stroke(r, egui::Rounding::same(5.5), egui::Stroke::new(1.0, BODY_EDGE));
    p.text(
        c,
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(8.0),
        if on { egui::Color32::BLACK } else { TEXT_DIM },
    );
}

fn paint_chip(
    p: &egui::Painter,
    r: egui::Rect,
    label: &str,
    on: bool,
    off_fill: egui::Color32,
) {
    let fill = if on {
        egui::Color32::from_rgb(220, 220, 230)
    } else {
        off_fill
    };
    p.rect_filled(r, egui::Rounding::same(2.0), fill);
    p.rect_stroke(r, egui::Rounding::same(2.0), egui::Stroke::new(1.0, BODY_EDGE));
    p.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(r.height() * 0.85),
        if on { egui::Color32::BLACK } else { TEXT_DIM },
    );
}

fn paint_led(p: &egui::Painter, c: egui::Pos2, on: bool) {
    let r = egui::Rect::from_center_size(c, egui::Vec2::new(8.0, 4.0));
    let fill = if on { A_GREEN } else { BODY_DARK };
    p.rect_filled(r, egui::Rounding::same(1.0), fill);
    p.rect_stroke(r, egui::Rounding::same(1.0), egui::Stroke::new(0.5, BODY_EDGE));
}

fn paint_dpad(
    p: &egui::Painter,
    centre: egui::Pos2,
    half: f32,
    up: bool,
    down: bool,
    left: bool,
    right: bool,
) {
    let arm_long = half;
    let arm_short = half * 0.55;

    p.rect_filled(
        egui::Rect::from_center_size(centre, egui::Vec2::new(arm_short, arm_long * 2.0)),
        egui::Rounding::ZERO,
        ACCENT_OFF,
    );
    p.rect_filled(
        egui::Rect::from_center_size(centre, egui::Vec2::new(arm_long * 2.0, arm_short)),
        egui::Rounding::ZERO,
        ACCENT_OFF,
    );

    let lit = egui::Color32::from_rgb(220, 220, 230);
    let arm = |dx: f32, dy: f32, w: f32, h: f32| {
        egui::Rect::from_center_size(
            egui::Pos2::new(centre.x + dx, centre.y + dy),
            egui::Vec2::new(w, h),
        )
    };
    if up {
        p.rect_filled(arm(0.0, -arm_long * 0.5, arm_short, arm_long), egui::Rounding::ZERO, lit);
    }
    if down {
        p.rect_filled(arm(0.0, arm_long * 0.5, arm_short, arm_long), egui::Rounding::ZERO, lit);
    }
    if left {
        p.rect_filled(arm(-arm_long * 0.5, 0.0, arm_long, arm_short), egui::Rounding::ZERO, lit);
    }
    if right {
        p.rect_filled(arm(arm_long * 0.5, 0.0, arm_long, arm_short), egui::Rounding::ZERO, lit);
    }
}

fn paint_stick(p: &egui::Painter, centre: egui::Pos2, radius: f32, raw_x: u8, raw_y: u8) {
    p.circle_filled(centre, radius, BODY_DARK);
    p.circle_stroke(centre, radius, egui::Stroke::new(1.0, BODY_EDGE));
    let dx = ((f32::from(raw_x) - 128.0) / 100.0).clamp(-1.0, 1.0);
    let dy = ((f32::from(raw_y) - 128.0) / 100.0).clamp(-1.0, 1.0);
    let cap = egui::Pos2::new(centre.x + dx * radius * 0.55, centre.y - dy * radius * 0.55);
    p.circle_filled(cap, radius * 0.55, ACCENT_OFF);
    p.circle_stroke(cap, radius * 0.55, egui::Stroke::new(1.0, BODY_EDGE));
}
