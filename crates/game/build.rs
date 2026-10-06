//! The game's icon (the launcher's, crates/launcher/assets/icon.ico),
//! embedded in cod4rw.exe on Windows.
fn main() {
    println!("cargo:rerun-if-changed=../launcher/assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../launcher/assets/icon.ico");
        res.set("FileDescription", "CoD4 Rewrite");
        res.set("ProductName", "CoD4 Rewrite");
        if let Err(e) = res.compile() {
            println!("cargo:warning=game icon not embedded: {e}");
        }
    }
}
