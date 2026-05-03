//! Decode the bundled icon and hand it to eframe so the egui window
//! titlebar, taskbar, and Alt-Tab switcher pick it up. The icon is
//! also embedded as a Win32 resource on Windows via `build.rs`, but
//! that only covers the .exe file metadata — egui uses this PNG for
//! the live window decoration.
//!
//! The PNG is `include_bytes!`'d so the shipped binary stays a single
//! file. `build.rs` emits the `have_icon` cfg only when the asset
//! exists at compile time, so a fresh checkout without `assets/icon.png`
//! still builds (just without an icon).

use eframe::egui;

#[cfg(have_icon)]
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");

/// Returns the icon as eframe's `IconData`, ready to feed to
/// `ViewportBuilder::with_icon`. Falls back to `None` when the PNG is
/// missing or undecodable so a working binary doesn't get held back
/// by an asset hiccup.
#[cfg(have_icon)]
pub fn load() -> Option<egui::IconData> {
    let img = image::load_from_memory(ICON_PNG).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

#[cfg(not(have_icon))]
pub fn load() -> Option<egui::IconData> {
    None
}
