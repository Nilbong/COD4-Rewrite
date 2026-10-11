//! A converted MW2 map's lightmap pair `n` as raw bytes, with its formats:
//! `lmdump <map> <out> <n>` -> `<out>/primaryN.bin`, `<out>/secondaryN.bin`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let z = iw4::load_map(&iw4::Install::locate()?, &a[0])?;
    let w = z.gfx_world().unwrap();
    std::fs::create_dir_all(&a[1])?;
    let n: usize = a[2].parse()?;
    println!("{} lightmaps", w.lightmaps.len());
    let lm = &w.lightmaps[n];
    for (kind, id) in [("primary", lm.primary), ("secondary", lm.secondary)] {
        let Some(img) = id.and_then(|id| z.image(id)) else { continue };
        let d = img.load_def.as_ref().unwrap();
        println!("{kind} {} {}x{} format {} {} bytes", img.name, img.width, img.height, d.format, d.data.len());
        std::fs::write(format!("{}/{kind}{n}.bin", a[1]), &d.data)?;
    }
    Ok(())
}
