//! The native title bar is drawn by the OS, not egui. On Windows it is
//! coloured through DWM: Windows 11 takes an exact caption colour (so the bar
//! matches the header and fades with it), Windows 10 only the dark / light
//! switch. Other platforms rely on `ViewportCommand::SetTheme`.

use eframe::egui::Color32;

#[cfg(windows)]
pub fn apply(frame: &eframe::Frame, dark: bool, caption: Color32, text: Color32) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_CAPTION_COLOR: u32 = 35; // Windows 11
    const DWMWA_TEXT_COLOR: u32 = 36; // Windows 11

    let Ok(handle) = frame.window_handle() else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get();
    // COLORREF is 0x00BBGGRR.
    let colorref = |c: Color32| u32::from(c.r()) | (u32::from(c.g()) << 8) | (u32::from(c.b()) << 16);
    // Failures (older Windows) are ignored: the bar then keeps its default look.
    set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &i32::from(dark));
    set_attribute(hwnd, DWMWA_CAPTION_COLOR, &colorref(caption));
    set_attribute(hwnd, DWMWA_TEXT_COLOR, &colorref(text));
}

#[cfg(not(windows))]
pub fn apply(_frame: &eframe::Frame, _dark: bool, _caption: Color32, _text: Color32) {}

/// Uses the icon embedded in the executable (resource id 1, see build.rs) for
/// the title bar and taskbar. Windows then picks the designer's hand-tuned
/// 16 / 20 / 24 px images for the current DPI instead of a scaled-down large
/// one. Returns whether it applied, so the caller can retry on a later frame.
#[cfg(windows)]
pub fn set_window_icon(frame: &eframe::Frame) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDpiForWindow(hwnd: isize) -> u32;
        fn GetSystemMetricsForDpi(index: i32, dpi: u32) -> i32;
        fn LoadImageW(instance: isize, name: *const u16, kind: u32, cx: i32, cy: i32, flags: u32) -> isize;
        fn SendMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
    }
    const IMAGE_ICON: u32 = 1;
    const WM_SETICON: u32 = 0x0080;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;
    const SM_CXICON: i32 = 11;
    const SM_CXSMICON: i32 = 49;
    // MAKEINTRESOURCEW(1)
    const APP_ICON: *const u16 = std::ptr::without_provenance(1);

    let Ok(handle) = frame.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    let hwnd = handle.hwnd.get();
    // SAFETY: plain Win32 calls on the live window eframe handed us; the icon
    // handles stay owned by the window (shared resources are never freed).
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let dpi = GetDpiForWindow(hwnd).max(96);
        for (which, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            let size = GetSystemMetricsForDpi(metric, dpi);
            let icon = LoadImageW(module, APP_ICON, IMAGE_ICON, size, size, 0);
            if icon == 0 {
                return false;
            }
            SendMessageW(hwnd, WM_SETICON, which, icon);
        }
    }
    true
}

#[cfg(not(windows))]
pub fn set_window_icon(_frame: &eframe::Frame) -> bool {
    true
}

#[cfg(windows)]
fn set_attribute<T>(hwnd: isize, attribute: u32, value: &T) {
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const std::ffi::c_void, size: u32) -> i32;
    }
    // SAFETY: `hwnd` is the live window eframe handed us, and `value` points
    // to a `T` of exactly `size_of::<T>()` bytes for the duration of the call.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            std::ptr::from_ref(value).cast(),
            std::mem::size_of::<T>() as u32,
        );
    }
}
