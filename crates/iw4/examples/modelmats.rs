//! A model's materials and their textures in `common_mp`: `modelmats <model>`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let install = iw4::Install::locate()?;
    let z = iw4::zone::Zone::parse(&iw4::fastfile::load(&install.zone_path("common_mp"))?, Default::default())?;
    let m = z.assets.iter().find(|m| m.ty == iw4::zone::AssetType::XModel && m.name.trim_start_matches(',') == a[0]).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for id in m.root.assets("materialHandles").into_iter().flatten() {
        if !seen.insert(id) { continue; }
        let mat = &z.assets[id];
        let tech = mat.root.asset("techniqueSet").map_or("-".to_owned(), |t| z.assets[t].name.clone());
        println!("{} [{}]", mat.name, tech);
        for t in mat.root.nodes("textureTable") {
            let img = t.node("u").and_then(|u| u.asset("image")).map_or("-".to_owned(), |i| z.assets[i].name.clone());
            println!("   hash {:#010x} sem {} {}", u32::from_le_bytes(t.data[0..4].try_into().unwrap()), t.data[7], img);
        }
        let c = mat.root.bytes("constantTable");
        for k in c.chunks_exact(32) {
            println!("   const {} {:?}", String::from_utf8_lossy(&k[4..16]).trim_end_matches('\0'), (0..4).map(|i| f32::from_le_bytes(k[16+i*4..20+i*4].try_into().unwrap())).collect::<Vec<_>>());
        }
    }
    Ok(())
}
