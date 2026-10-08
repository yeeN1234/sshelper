//! Fonts: egui's defaults, the Phosphor icon font, and a system CJK font as
//! fallback (egui bundles no CJK glyphs). `SSHELPER_FONT=<path>` overrides the
//! CJK font search.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};

/// (path, preferred face index inside a .ttc collection)
fn candidates() -> Vec<(PathBuf, u32)> {
    let mut list: Vec<(PathBuf, u32)> = Vec::new();
    if let Some(custom) = std::env::var_os("SSHELPER_FONT") {
        list.push((custom.into(), 0));
    }
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
        let fonts = PathBuf::from(windir).join("Fonts");
        // Microsoft JhengHei (Traditional), then YaHei / MingLiU / SimSun.
        for name in ["msjh.ttc", "msjh.ttf", "msyh.ttc", "mingliu.ttc", "simsun.ttc"] {
            list.push((fonts.join(name), 0));
        }
    }
    #[cfg(target_os = "macos")]
    for path in [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ] {
        list.push((path.into(), 0));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Noto Sans CJK collections: face 3 is Traditional Chinese.
        for path in [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
        ] {
            list.push((path.into(), 3));
        }
        for path in [
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ] {
            list.push((path.into(), 0));
        }
    }
    list
}

/// Number of faces in a TrueType collection (`ttcf` header), 1 otherwise.
fn face_count(bytes: &[u8]) -> u32 {
    match bytes {
        [b't', b't', b'c', b'f', _, _, _, _, a, b, c, d, ..] => u32::from_be_bytes([*a, *b, *c, *d]),
        _ => 1,
    }
}

fn find_cjk() -> Option<(PathBuf, FontData)> {
    candidates().into_iter().find_map(|(path, index)| {
        let bytes = std::fs::read(&path).ok()?;
        let index = if index < face_count(&bytes) { index } else { 0 };
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        Some((path, data))
    })
}

/// Installs the icon font and the first CJK font found; returns the CJK
/// font's path, `None` when there is none.
pub fn install(ctx: &Context) -> Option<PathBuf> {
    let mut fonts = FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    let found = find_cjk().map(|(path, data)| {
        fonts.font_data.insert("system-cjk".into(), Arc::new(data));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("system-cjk".into());
        }
        path
    });
    ctx.set_fonts(fonts);
    found
}
