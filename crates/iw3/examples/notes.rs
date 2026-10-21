//! An xanim's notetracks: notes <zone> <anim>
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let zone = iw3::zone::Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?, iw3::zone::ParseOptions::default())?;
    for asset in &zone.assets {
        if let iw3::zone::Asset::Generic(g) = asset
            && g.ty == iw3::zone::AssetType::XAnimParts
            && g.name == a[1]
        {
            let x = iw3::xanim::XAnim::from_node(&g.name, &g.root, &zone.script_strings)?;
            println!("{} frames {} at {} fps, {:.2}s", x.name, x.num_frames, x.framerate, x.duration());
            for n in &x.notifies {
                println!("  {:.3} {:?}", n.time, n.name);
            }
        }
    }
    Ok(())
}
