//! `sshelper-gui` — graphical interface.

// No console window behind the GUI in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod fonts;
mod logo;
mod theme;
mod titlebar;
mod worker;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("sshelper — SSH 公鑰部署")
            // Fits the whole form and the result view without scrolling.
            .with_inner_size([760.0, 600.0])
            .with_min_inner_size([640.0, 480.0])
            .with_icon(std::sync::Arc::new(
                eframe::icon_data::from_png_bytes(logo::WINDOW_PNG).expect("bundled icon is a valid PNG"),
            )),
        ..Default::default()
    };
    eframe::run_native(
        "sshelper",
        options,
        Box::new(|cc| {
            let font = fonts::install(&cc.egui_ctx);
            Ok(Box::new(app::App::new(&cc.egui_ctx, font.is_some())))
        }),
    )
}
