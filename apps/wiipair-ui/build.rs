//! Build script for the WiiPair UI:
//!
//! 1. Reads `assets/icon.png` (the master 1024x1024 source) and emits
//!    a multi-size `icon.ico` (16/32/48/64/128/256) into `OUT_DIR`.
//! 2. On Windows, hands that `.ico` to `winresource` so the resulting
//!    `wiipair.exe` carries the icon as a Win32 resource — that's
//!    what File Explorer, the taskbar, and the Alt-Tab switcher
//!    pick up.
//!
//! Pure build-time work: no runtime dependency on ImageMagick /
//! Inkscape / external tooling. Other platforms ignore the
//! `winresource` step.

use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent() // apps/
        .and_then(|p| p.parent()) // workspace root
        .expect("workspace root above apps/wiipair-ui");
    let png_path = workspace_root.join("assets").join("icon.png");

    println!("cargo:rerun-if-changed={}", png_path.display());

    // Tell the runtime icon loader whether the PNG exists at compile
    // time. When it doesn't, the binary still builds — it just ships
    // without an icon (see src/icon.rs).
    println!("cargo:rustc-check-cfg=cfg(have_icon)");

    if !png_path.exists() {
        // Fail soft so contributors who haven't grabbed the asset can
        // still iterate — the binary just won't have an icon. CI
        // builds carry the asset and will get the embedded icon.
        println!(
            "cargo:warning=icon source missing at {} — skipping icon embed",
            png_path.display()
        );
        return;
    }
    println!("cargo:rustc-cfg=have_icon");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ico_path = out_dir.join("icon.ico");
    if let Err(e) = generate_ico(&png_path, &ico_path) {
        println!("cargo:warning=icon.ico generation failed: {e}");
        return;
    }

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico_path.to_str().expect("non-utf8 OUT_DIR"));
        if let Err(e) = res.compile() {
            println!("cargo:warning=winresource compile failed: {e}");
        }
    }
    #[cfg(not(windows))]
    {
        let _ = ico_path;
    }
}

fn generate_ico(png_path: &PathBuf, ico_path: &PathBuf) -> Result<(), String> {
    let img = image::open(png_path).map_err(|e| format!("decode {}: {e}", png_path.display()))?;
    let rgba = img.to_rgba8();
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    // Standard sizes covering File Explorer (16/32/48), taskbar (32/48),
    // jump-list large icon (256), and a few in-between for crisp HiDPI.
    for size in [16u32, 32, 48, 64, 128, 256] {
        let resized = image::imageops::resize(
            &rgba,
            size,
            size,
            image::imageops::FilterType::Lanczos3,
        );
        let data =
            ico::IconImage::from_rgba_data(size, size, resized.into_raw());
        icon_dir
            .add_entry(ico::IconDirEntry::encode(&data).map_err(|e| format!("encode {size}: {e}"))?);
    }
    let file = File::create(ico_path).map_err(|e| format!("create {}: {e}", ico_path.display()))?;
    icon_dir
        .write(BufWriter::new(file))
        .map_err(|e| format!("write ico: {e}"))?;
    Ok(())
}
