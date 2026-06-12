//! Xbox 360 / Xbox One controller mapping preview, rendered from
//! the upstream `gamepad-viewer` (e7d, MIT) sprite assets.
//!
//! The base SVG is the controller body; per-input sprites are
//! sprite-sheets blitted on top with UV mapping picked from
//! `template.css`. Sprites are rasterised once with `resvg`, cached
//! as egui textures (`OnceLock`), and then redrawn every frame as
//! cheap mesh primitives — no per-frame SVG parsing.

use eframe::egui;
use std::sync::OnceLock;
use wiimote_output::XboxState;

// =====================================================================
// Asset bytes — embedded so the binary is self-contained.
// =====================================================================

const BASE_BYTES: &[u8] = include_bytes!("../../../assets/devices/xbox-one/base-white.svg");
const BUMPER_BYTES: &[u8] = include_bytes!("../../../assets/devices/xbox-one/bumper.svg");
const BUTTONS_BYTES: &[u8] = include_bytes!("../../../assets/devices/xbox-one/buttons.svg");
const DPAD_BYTES: &[u8] = include_bytes!("../../../assets/devices/xbox-one/dpad.svg");
const START_SELECT_BYTES: &[u8] = include_bytes!("../../../assets/devices/xbox-one/start-select.svg");
const STICK_BYTES: &[u8] = include_bytes!("../../../assets/devices/xbox-one/stick.svg");
const TRIGGER_BYTES: &[u8] = include_bytes!("../../../assets/devices/xbox-one/trigger.svg");

// =====================================================================
// Layout constants — copied from the upstream `template.css`. All
// coordinates / sizes are in the base controller's 750×630.455 frame;
// `rect_in_canvas` scales them to the actual widget rectangle.
// =====================================================================

const BASE_W: f32 = 750.0;
const BASE_H: f32 = 630.455;

/// Display size for the preview. Picked to match the proportions of
/// the device cards' other zones.
pub const SIZE: egui::Vec2 = egui::Vec2::new(260.0, 218.5);

// Triggers area: top:0, left:151, w:448, h:122.
// Each trigger is 89×122.
const TRIGGER_LEFT_X: f32 = 151.0;
const TRIGGER_RIGHT_X: f32 = 151.0 + 448.0 - 89.0;
const TRIGGER_Y: f32 = 0.0;
const TRIGGER_W: f32 = 89.0;
const TRIGGER_H: f32 = 122.0;

// Bumpers area: top:129, left:107, w:536, h:61. Each 170×61.
const BUMPER_LEFT_X: f32 = 107.0;
const BUMPER_RIGHT_X: f32 = 107.0 + 536.0 - 170.0;
const BUMPER_Y: f32 = 129.0;
const BUMPER_W: f32 = 170.0;
const BUMPER_H: f32 = 61.0;

// Sticks: left at top:0,left:0; right at top:113,left:288 (relative
// to the sticks-container at top:239,left:144).
const STICK_W: f32 = 83.0;
const STICK_H: f32 = 83.0;
const STICK_L_X: f32 = 144.0;
const STICK_L_Y: f32 = 239.0;
const STICK_R_X: f32 = 144.0 + 288.0;
const STICK_R_Y: f32 = 239.0 + 113.0;
/// Maximum stick travel — `template.js` uses ±25 px in the 750×630
/// frame, mirrored here so the preview matches the upstream feel.
const STICK_TRAVEL: f32 = 25.0;

// Buttons area: top:201, left:489, w:155, h:156. Each button 53×53.
const BUTTONS_X: f32 = 489.0;
const BUTTONS_Y: f32 = 201.0;
const BTN_SIZE: f32 = 53.0;
// Per-button positions relative to the button area (from template.css):
//   A: top:102, left:51   B: top:52, right:1 (=> left = 155-53-1 = 101)
//   X: top:52,  left:1    Y: top:1,  left:51
const BTN_A_X: f32 = BUTTONS_X + 51.0;
const BTN_A_Y: f32 = BUTTONS_Y + 102.0;
const BTN_B_X: f32 = BUTTONS_X + 101.0;
const BTN_B_Y: f32 = BUTTONS_Y + 52.0;
const BTN_X_X: f32 = BUTTONS_X + 1.0;
const BTN_X_Y: f32 = BUTTONS_Y + 52.0;
const BTN_Y_X: f32 = BUTTONS_X + 51.0;
const BTN_Y_Y: f32 = BUTTONS_Y + 1.0;

// Start/Back ("arrows") area: top:264, left:306, w:141, h:33.
// Each pill is 33×33 — Back on the left, Start on the right.
const ARROWS_X: f32 = 306.0;
const ARROWS_Y: f32 = 264.0;
const ARROW_SIZE: f32 = 33.0;
const BACK_X: f32 = ARROWS_X;
const BACK_Y: f32 = ARROWS_Y;
const START_X: f32 = ARROWS_X + 141.0 - ARROW_SIZE;
const START_Y: f32 = ARROWS_Y;

// D-pad area: top:345, left:223, w:110, h:111.
// Per-direction frames (size + position relative to the dpad area):
//   up:    34×56 at (38, 1)
//   down:  34×56 at (38, 111-56=55)
//   left:  56×34 at (0, 39)
//   right: 56×34 at (110-56=54, 39)
const DPAD_X: f32 = 223.0;
const DPAD_Y: f32 = 345.0;
const DPAD_UP_X: f32 = DPAD_X + 38.0;
const DPAD_UP_Y: f32 = DPAD_Y + 1.0;
const DPAD_DOWN_X: f32 = DPAD_X + 38.0;
const DPAD_DOWN_Y: f32 = DPAD_Y + 55.0;
const DPAD_LEFT_X: f32 = DPAD_X;
const DPAD_LEFT_Y: f32 = DPAD_Y + 39.0;
const DPAD_RIGHT_X: f32 = DPAD_X + 54.0;
const DPAD_RIGHT_Y: f32 = DPAD_Y + 39.0;

// =====================================================================
// Sprite-sheet UV coordinates (computed from each SVG's own viewBox).
// =====================================================================

const BUTTONS_VB_W: f32 = 212.0;
const BUTTONS_VB_H: f32 = 106.0;
const BTN_FRAME_W: f32 = 53.0;
const BTN_FRAME_H: f32 = 53.0;

const DPAD_VB_W: f32 = 70.66;
const DPAD_VB_H: f32 = 128.036;

const ARROWS_VB_W: f32 = 67.409;
const ARROWS_VB_H: f32 = 33.206;

const STICK_VB_W: f32 = 168.77;
const STICK_VB_H: f32 = 83.383;

// =====================================================================
// Rasterised sprites — built once, kept for the life of the process.
// =====================================================================

struct Sprites {
    base: egui::TextureHandle,
    bumper: egui::TextureHandle,
    buttons: egui::TextureHandle,
    dpad: egui::TextureHandle,
    start_select: egui::TextureHandle,
    stick: egui::TextureHandle,
    trigger: egui::TextureHandle,
}

static SPRITES: OnceLock<Sprites> = OnceLock::new();

/// Render one SVG to an `egui::ColorImage` at `scale` × its native
/// resolution. 2× gives a crisp downscale to the typical card size.
fn rasterise(svg_bytes: &[u8], scale: f32) -> egui::ColorImage {
    let opts = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg_bytes, &opts).expect("embedded SVG must parse");
    let size = tree.size();
    let w = (size.width() * scale).ceil() as u32;
    let h = (size.height() * scale).ceil() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("pixmap allocation");
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap_to_color_image(&pixmap)
}

/// `tiny_skia::Pixmap` data is RGBA premultiplied; egui's
/// `ColorImage::from_rgba_unmultiplied` expects straight alpha. Walk
/// the buffer once and unmultiply the colour channels so anti-aliased
/// edges keep their intended brightness.
fn pixmap_to_color_image(pixmap: &tiny_skia::Pixmap) -> egui::ColorImage {
    let data = pixmap.data();
    let mut buf = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(4) {
        let (r, g, b, a) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        match a {
            0 => buf.extend_from_slice(&[0, 0, 0, 0]),
            255 => buf.extend_from_slice(&[r, g, b, a]),
            _ => {
                let af = f32::from(a) / 255.0;
                let unmult = |c: u8| ((f32::from(c) / af).round().min(255.0)) as u8;
                buf.extend_from_slice(&[unmult(r), unmult(g), unmult(b), a]);
            }
        }
    }
    egui::ColorImage::from_rgba_unmultiplied(
        [pixmap.width() as usize, pixmap.height() as usize],
        &buf,
    )
}

fn load_sprites(ctx: &egui::Context) -> Sprites {
    let scale = 2.0;
    let mk = |name: &str, bytes: &[u8]| -> egui::TextureHandle {
        ctx.load_texture(name, rasterise(bytes, scale), egui::TextureOptions::LINEAR)
    };
    Sprites {
        base: mk("xbox360-base", BASE_BYTES),
        bumper: mk("xbox360-bumper", BUMPER_BYTES),
        buttons: mk("xbox360-buttons", BUTTONS_BYTES),
        dpad: mk("xbox360-dpad", DPAD_BYTES),
        start_select: mk("xbox360-start-select", START_SELECT_BYTES),
        stick: mk("xbox360-stick", STICK_BYTES),
        trigger: mk("xbox360-trigger", TRIGGER_BYTES),
    }
}

// =====================================================================
// Public API
// =====================================================================

pub fn render(ui: &mut egui::Ui, xb: &XboxState) {
    let (canvas, _) = ui.allocate_exact_size(SIZE, egui::Sense::hover());
    let sprites = SPRITES.get_or_init(|| load_sprites(ui.ctx()));
    let painter = ui.painter_at(canvas);

    // 1. Body — full-canvas blit.
    blit(&painter, sprites.base.id(), canvas, FULL_UV, false);

    // 2. Triggers — drawn as analog "fill from the bottom" overlays.
    draw_trigger(&painter, sprites.trigger.id(), canvas, TRIGGER_LEFT_X, xb.lt, false);
    draw_trigger(&painter, sprites.trigger.id(), canvas, TRIGGER_RIGHT_X, xb.rt, true);

    // 3. Bumpers — opacity 0/1 overlays.
    if xb.lb {
        let dest = rect_in_canvas(canvas, BUMPER_LEFT_X, BUMPER_Y, BUMPER_W, BUMPER_H);
        blit(&painter, sprites.bumper.id(), dest, FULL_UV, false);
    }
    if xb.rb {
        let dest = rect_in_canvas(canvas, BUMPER_RIGHT_X, BUMPER_Y, BUMPER_W, BUMPER_H);
        blit(&painter, sprites.bumper.id(), dest, FULL_UV, true);
    }

    // 4. Sticks — always drawn. Pressed state swaps the sprite-sheet
    //    frame; analog values shift the destination position.
    draw_stick(
        &painter,
        sprites.stick.id(),
        canvas,
        egui::Pos2::new(STICK_L_X, STICK_L_Y),
        (xb.lx, xb.ly),
        xb.thumb_l,
    );
    draw_stick(
        &painter,
        sprites.stick.id(),
        canvas,
        egui::Pos2::new(STICK_R_X, STICK_R_Y),
        (xb.rx, xb.ry),
        xb.thumb_r,
    );

    // 5. D-pad — opacity 0/1 per direction.
    if xb.up {
        let dest = rect_in_canvas(canvas, DPAD_UP_X, DPAD_UP_Y, 34.0, 56.0);
        let uv = uv_rect(35.0, 0.0, 34.0, 56.0, DPAD_VB_W, DPAD_VB_H);
        blit(&painter, sprites.dpad.id(), dest, uv, false);
    }
    if xb.down {
        let dest = rect_in_canvas(canvas, DPAD_DOWN_X, DPAD_DOWN_Y, 34.0, 56.0);
        let uv = uv_rect(0.0, 0.0, 34.0, 56.0, DPAD_VB_W, DPAD_VB_H);
        blit(&painter, sprites.dpad.id(), dest, uv, false);
    }
    if xb.left {
        let dest = rect_in_canvas(canvas, DPAD_LEFT_X, DPAD_LEFT_Y, 56.0, 34.0);
        let uv = uv_rect(0.0, 93.0, 56.0, 34.0, DPAD_VB_W, DPAD_VB_H);
        blit(&painter, sprites.dpad.id(), dest, uv, false);
    }
    if xb.right {
        let dest = rect_in_canvas(canvas, DPAD_RIGHT_X, DPAD_RIGHT_Y, 56.0, 34.0);
        let uv = uv_rect(0.0, 57.0, 56.0, 34.0, DPAD_VB_W, DPAD_VB_H);
        blit(&painter, sprites.dpad.id(), dest, uv, false);
    }

    // 6. Face buttons — always drawn (idle row), pressed swaps to row 2.
    draw_face_button(&painter, sprites.buttons.id(), canvas, BTN_A_X, BTN_A_Y, 0.0, xb.a);
    draw_face_button(&painter, sprites.buttons.id(), canvas, BTN_B_X, BTN_B_Y, 53.0, xb.b);
    draw_face_button(&painter, sprites.buttons.id(), canvas, BTN_X_X, BTN_X_Y, 106.0, xb.x);
    draw_face_button(&painter, sprites.buttons.id(), canvas, BTN_Y_X, BTN_Y_Y, 159.0, xb.y);

    // 7. Back / Start — opacity 0/1 overlays.
    if xb.back {
        let dest = rect_in_canvas(canvas, BACK_X, BACK_Y, ARROW_SIZE, ARROW_SIZE);
        let uv = uv_rect(0.0, 0.0, 33.0, 33.0, ARROWS_VB_W, ARROWS_VB_H);
        blit(&painter, sprites.start_select.id(), dest, uv, false);
    }
    if xb.start {
        let dest = rect_in_canvas(canvas, START_X, START_Y, ARROW_SIZE, ARROW_SIZE);
        let uv = uv_rect(33.0, 0.0, 33.0, 33.0, ARROWS_VB_W, ARROWS_VB_H);
        blit(&painter, sprites.start_select.id(), dest, uv, false);
    }
}

// =====================================================================
// Per-input draw helpers
// =====================================================================

fn draw_face_button(
    painter: &egui::Painter,
    tex: egui::TextureId,
    canvas: egui::Rect,
    x: f32,
    y: f32,
    sprite_x: f32,
    pressed: bool,
) {
    let dest = rect_in_canvas(canvas, x, y, BTN_SIZE, BTN_SIZE);
    // Idle is row 0 (sprite_y = 0); pressed is row 1 (sprite_y = 53).
    let sprite_y = if pressed { BTN_FRAME_H } else { 0.0 };
    let uv = uv_rect(sprite_x, sprite_y, BTN_FRAME_W, BTN_FRAME_H, BUTTONS_VB_W, BUTTONS_VB_H);
    blit(painter, tex, dest, uv, false);
}

fn draw_stick(
    painter: &egui::Painter,
    tex: egui::TextureId,
    canvas: egui::Rect,
    base: egui::Pos2,
    axis: (i16, i16),
    pressed: bool,
) {
    // Normalise i16 axis values to ±1.
    let nx = (f32::from(axis.0) / f32::from(i16::MAX)).clamp(-1.0, 1.0);
    let ny = (f32::from(axis.1) / f32::from(i16::MAX)).clamp(-1.0, 1.0);
    // XInput convention: positive Y = up. On screen Y grows downward,
    // so we negate Y so pushing the stick "up" moves the cap up.
    let x = base.x + nx * STICK_TRAVEL;
    let y = base.y - ny * STICK_TRAVEL;

    let dest = rect_in_canvas(canvas, x, y, STICK_W, STICK_H);
    // Idle frame = sprite-sheet x=85; pressed frame = x=0 (per CSS).
    let sprite_x = if pressed { 0.0 } else { 85.0 };
    let uv = uv_rect(sprite_x, 0.0, STICK_W, STICK_H, STICK_VB_W, STICK_VB_H);
    blit(painter, tex, dest, uv, false);
}

fn draw_trigger(
    painter: &egui::Painter,
    tex: egui::TextureId,
    canvas: egui::Rect,
    x: f32,
    value: u8,
    flip_x: bool,
) {
    if value == 0 {
        return;
    }
    // Reveal the trigger sprite from the bottom proportional to the
    // analog reading. The full sprite is TRIGGER_W × TRIGGER_H; for
    // partial pressure we shrink the destination height and bottom-
    // align it, then crop the sprite UV to the matching bottom band.
    let pct = f32::from(value) / 255.0;
    let visible_h = TRIGGER_H * pct;
    let visible_y = TRIGGER_Y + (TRIGGER_H - visible_h);
    let dest = rect_in_canvas(canvas, x, visible_y, TRIGGER_W, visible_h);
    let uv = egui::Rect::from_min_max(
        egui::Pos2::new(0.0, 1.0 - pct),
        egui::Pos2::new(1.0, 1.0),
    );
    blit(painter, tex, dest, uv, flip_x);
}

// =====================================================================
// Geometry helpers
// =====================================================================

const FULL_UV: egui::Rect = egui::Rect {
    min: egui::Pos2 { x: 0.0, y: 0.0 },
    max: egui::Pos2 { x: 1.0, y: 1.0 },
};

/// Map a (x, y, w, h) rect in the 750×630 base frame to a rect inside
/// the on-screen canvas.
fn rect_in_canvas(canvas: egui::Rect, x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
    let sx = canvas.width() / BASE_W;
    let sy = canvas.height() / BASE_H;
    egui::Rect::from_min_size(
        egui::Pos2::new(canvas.left() + x * sx, canvas.top() + y * sy),
        egui::Vec2::new(w * sx, h * sy),
    )
}

/// Build a UV rect from a pixel-space subregion of an SVG with the
/// given viewBox dimensions.
fn uv_rect(x: f32, y: f32, w: f32, h: f32, vb_w: f32, vb_h: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::Pos2::new(x / vb_w, y / vb_h),
        egui::Pos2::new((x + w) / vb_w, (y + h) / vb_h),
    )
}

/// Blit `tex` to `dest` using `uv`. Set `flip_x = true` to mirror
/// horizontally — used for the right bumper / right trigger which
/// share the same sprite as their left counterpart.
fn blit(
    painter: &egui::Painter,
    tex: egui::TextureId,
    dest: egui::Rect,
    uv: egui::Rect,
    flip_x: bool,
) {
    let mut mesh = egui::Mesh::with_texture(tex);
    let color = egui::Color32::WHITE;
    let (uv_lt, uv_rt, uv_lb, uv_rb) = if flip_x {
        (
            egui::Pos2::new(uv.max.x, uv.min.y),
            egui::Pos2::new(uv.min.x, uv.min.y),
            egui::Pos2::new(uv.max.x, uv.max.y),
            egui::Pos2::new(uv.min.x, uv.max.y),
        )
    } else {
        (uv.left_top(), uv.right_top(), uv.left_bottom(), uv.right_bottom())
    };
    let idx = mesh.vertices.len() as u32;
    mesh.vertices.push(egui::epaint::Vertex { pos: dest.left_top(), uv: uv_lt, color });
    mesh.vertices.push(egui::epaint::Vertex { pos: dest.right_top(), uv: uv_rt, color });
    mesh.vertices.push(egui::epaint::Vertex { pos: dest.left_bottom(), uv: uv_lb, color });
    mesh.vertices.push(egui::epaint::Vertex { pos: dest.right_bottom(), uv: uv_rb, color });
    mesh.indices.extend([idx, idx + 1, idx + 2, idx + 2, idx + 1, idx + 3]);
    painter.add(egui::Shape::mesh(mesh));
}
