//! `cargo run --release -p t4 --example materials -- <zone> [filter]`: World
//! at War materials (after conversion) whose name contains `filter`, with
//! their first texture's image. With a third argument, each image's top mip
//! is also written there as `<image>_<w>x<h>.<format>` (`rgba`, or the
//! compressed blocks as `dxt1`/`dxt3`/`dxt5`).

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("common_mp".into());
    let filter = a.next().unwrap_or_default().to_ascii_lowercase();
    let dump = a.next();
    let install = t4::Install::locate()?;
    let vfs = install.vfs()?;
    let zone = t4::load_iw3(&install, &zone_name)?;
    for asset in &zone.assets {
        let iw3::zone::Asset::Material(m) = asset else { continue };
        if !m.name.to_ascii_lowercase().contains(&filter) {
            continue;
        }
        let image = m.textures.first().and_then(|t| t.image).and_then(|i| zone.image(i)).map_or("-", |i| i.name.as_str());
        println!("{:<40} {image}", m.name);
        if let (Some(dir), Ok(Some(bytes))) = (&dump, vfs.read(&format!("images/{image}.iwi"))) {
            let iwi = iw3::iwi::Iwi::parse(&bytes)?;
            let (ext, data) = match iwi.to_rgba8(0) {
                Some(rgba) => ("rgba".to_owned(), rgba),
                None => (format!("{:?}", iwi.format).to_ascii_lowercase(), iwi.levels[0].clone()),
            };
            std::fs::write(format!("{dir}/{image}_{}x{}.{ext}", iwi.width, iwi.height), data)?;
        }
    }
    Ok(())
}
