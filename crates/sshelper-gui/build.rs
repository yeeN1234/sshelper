//! Embeds the application icon into the Windows executable (Explorer, shortcuts,
//! and the source of the window's title bar / taskbar icon at runtime).

fn main() {
    let ico =
        std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../assets/icon/sshelper.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // Resource id 1: the application icon (see titlebar::set_window_icon).
        res.set_icon(ico.to_str().expect("UTF-8 path"));
        res.set("FileDescription", "sshelper - SSH key deployment");
        res.set("ProductName", "sshelper");
        res.compile()
            .expect("embed Windows resources (needs rc.exe from the Windows SDK)");
    }
}
