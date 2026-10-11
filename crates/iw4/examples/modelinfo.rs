//! A model's LODs and surfaces in a zone: `modelinfo <zone> <model>`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let install = iw4::Install::locate()?;
    let z = iw4::zone::Zone::parse(&iw4::fastfile::load(&install.zone_path(&a[0]))?, Default::default())?;
    for m in z.assets.iter().filter(|m| m.ty == iw4::zone::AssetType::XModel && m.name.trim_start_matches(',') == a[1]) {
        println!("{:?} numLods {} numsurfs {}", m.name, m.root.int("numLods"), m.root.int("numsurfs"));
        for (i, l) in m.root.nodes("lodInfo").iter().enumerate() {
            let ms = l.nodes("modelSurfs");
            println!("  lod {i}: numsurfs {} surfIndex {} modelSurfs {:?} raw ptr {:#x}", l.int("numsurfs"), l.int("surfIndex"),
                ms.first().map(|s| (s.string("name"), s.nodes("surfs").iter().map(|x| (x.int("vertCount"), x.bytes("verts0").len())).collect::<Vec<_>>())),
                u32::from_le_bytes(l.data[8..12].try_into().unwrap()));
        }
    }
    Ok(())
}
