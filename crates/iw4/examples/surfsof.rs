//! A model's LOD surface names in a zone: `surfsof <zone> <model>`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let install = iw4::Install::locate()?;
    let z = iw4::zone::Zone::parse(&iw4::fastfile::load(&install.zone_path(&a[0]))?, Default::default())?;
    for m in z.assets.iter().filter(|m| m.name.trim_start_matches(',') == a[1]) {
        println!("{:?} {:?}", m.ty, m.name);
        for l in m.root.nodes("lodInfo") {
            for s in l.nodes("modelSurfs") {
                println!("  surfs {:?} n {}", s.string("name"), s.nodes("surfs").len());
            }
        }
    }
    Ok(())
}
