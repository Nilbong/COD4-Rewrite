//! The launcher's icon (assets/icon.ico, made from assets/icon.png), embedded
//! in Launcher.exe on Windows.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "CoD4 Rewrite Launcher");
        res.set("ProductName", "CoD4 Rewrite");
        if let Err(e) = res.compile() {
            println!("cargo:warning=launcher icon not embedded: {e}");
        }
    }
}
