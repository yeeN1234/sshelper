//! The application icon as images (assets/icon, MIT, see its LICENSE).

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

/// Large PNG for the window icon on platforms that take it from eframe
/// (Windows uses the embedded .ico instead, see `titlebar::set_window_icon`).
pub const WINDOW_PNG: &[u8] = include_bytes!("../../../assets/icon/png/sshelper-256.png");

/// The header logo, drawn at 24 points. The designer drew 24 px and smaller
/// separately, so pick the image closest to the physical size instead of
/// scaling one down.
pub fn header_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    const SIZES: [(u32, &[u8]); 5] = [
        (24, include_bytes!("../../../assets/icon/png/sshelper-24.png")),
        (32, include_bytes!("../../../assets/icon/png/sshelper-32.png")),
        (40, include_bytes!("../../../assets/icon/png/sshelper-40.png")),
        (48, include_bytes!("../../../assets/icon/png/sshelper-48.png")),
        (64, include_bytes!("../../../assets/icon/png/sshelper-64.png")),
    ];
    let wanted = (24.0 * ctx.pixels_per_point()).round() as u32;
    let (_, bytes) = SIZES
        .iter()
        .find(|(size, _)| *size >= wanted)
        .unwrap_or(&SIZES[SIZES.len() - 1]);
    let icon = eframe::icon_data::from_png_bytes(bytes).ok()?;
    let image = ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
    Some(ctx.load_texture("sshelper-logo", image, TextureOptions::LINEAR))
}
