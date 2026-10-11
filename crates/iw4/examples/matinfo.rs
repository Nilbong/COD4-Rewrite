//! Materials (by part of their name) in a converted MW2 map: `matinfo <map> <part>`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let install = iw4::Install::locate()?;
    let z = if a[0] == "common_mp" {
        let common = iw4::zone::Zone::parse(&iw4::fastfile::load(&install.zone_path("common_mp"))?, Default::default())?;
        iw4::convert::to_iw3(&common, None)
    } else {
        iw4::load_map(&install, &a[0])?
    };
    for m in z.assets.iter().filter_map(|x| match x { iw3::zone::Asset::Material(m) => Some(m), _ => None }).filter(|m| m.name.contains(&a[1])).take(8) {
        println!("{} [{}] bits7 {:x?}", m.name, m.technique_set.and_then(|t| z.technique_set(t)).map_or("", |t| t.name.as_str()), m.state_bits_for(7));
        for t in &m.textures {
            let img = t.image.and_then(|i| z.image(i));
            println!("   {:?} hash {:#x} {:?} {:?}", t.semantic, t.name_hash, img.map(|i| &i.name), img.map(|i| (i.width, i.height, i.load_def.is_some())));
        }
        for c in &m.constants {
            println!("   const {} {:?}", c.name, c.literal);
        }
    }
    Ok(())
}
