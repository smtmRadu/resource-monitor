//! Embed the app icon and version info in the exe (Explorer, Start Menu,
//! Task Manager and "Apps & features" show them).
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico").set("ProductName", "Resource Monitor").set("FileDescription", "Resource Monitor");
        res.compile().expect("embed exe resources");
    }
}
