//! Write a converted map's reflection probe image data to a file:
//! `probedump <map> <index> <out>`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let install = iw4::Install::locate()?;
    let z = iw4::load_map(&install, &a[0])?;
    let w = z.gfx_world().unwrap();
    let p = &w.reflection_probes[a[1].parse::<usize>()?];
    let img = z.image(p.image.unwrap()).unwrap();
    let d = img.load_def.as_ref().unwrap();
    println!("{} {}x{} levels {} flags {} format {} bytes {}", img.name, img.width, img.height, d.level_count, d.flags, d.format, d.data.len());
    std::fs::write(&a[2], &d.data)?;
    Ok(())
}
