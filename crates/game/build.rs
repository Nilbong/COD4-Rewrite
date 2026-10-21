//! The game's icon (the launcher's, crates/launcher/assets/icon.ico),
//! embedded in cod4rw.exe on Windows.
fn main() {
    println!("cargo:rerun-if-changed=../launcher/assets/icon.ico");
    maplooks();
    maplayouts();
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

/// The saved map looks (`assets/maplooks/*.txt`, the Map Look Editor's),
/// embedded as `MAPLOOKS: &[(file name, text)]` in `$OUT_DIR/maplooks.rs`.
fn maplooks() {
    let dir = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets").join("maplooks");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "txt")).collect())
        .unwrap_or_default();
    entries.sort();
    let mut out = String::from("pub const MAPLOOKS: &[(&str, &str)] = &[\n");
    for p in entries {
        println!("cargo:rerun-if-changed={}", p.display());
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        out.push_str(&format!("    ({name:?}, include_str!({:?})),\n", p.display().to_string()));
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("maplooks.rs");
    std::fs::write(dest, out).unwrap();
}

/// The map editor's saved layouts (`assets/maplayouts/*.txt`), embedded as
/// `MAPLAYOUTS: &[(file name, text)]` in `$OUT_DIR/maplayouts.rs`.
fn maplayouts() {
    let dir = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets").join("maplayouts");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "txt")).collect())
        .unwrap_or_default();
    entries.sort();
    let mut out = String::from("pub const MAPLAYOUTS: &[(&str, &str)] = &[
");
    for p in entries {
        println!("cargo:rerun-if-changed={}", p.display());
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        out.push_str(&format!("    ({name:?}, include_str!({:?})),
", p.display().to_string()));
    }
    out.push_str("];
");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("maplayouts.rs");
    std::fs::write(dest, out).unwrap();
}
