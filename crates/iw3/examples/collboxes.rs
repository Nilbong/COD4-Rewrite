//! A map's static models' collision boxes: collboxes <map> <filter>
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let filter = a.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let clip = zone.clip_map().expect("clip map");
    for sm in &clip.static_models {
        let Some(m) = sm.model.and_then(|id| zone.xmodel(id)) else { continue };
        if !m.name.to_ascii_lowercase().contains(&filter) {
            continue;
        }
        println!("{} at {:?} contents {:#x} bounds {:?}..{:?} abs {:?}..{:?}", m.name, sm.origin, m.contents, m.mins, m.maxs, sm.absmin, sm.absmax);
        for b in &m.coll_boxes {
            let top = b.tris.iter().flatten().map(|v| v[2]).fold(f32::MIN, f32::max);
            println!("  box {:?}..{:?} contents {:#x} surf type {}: {} tris, highest corner z {:.1}", b.mins, b.maxs, b.contents, (b.surf_flags >> 20) & 31, b.tris.len(), top);
            for t in b.tris.iter().take(3) {
                println!("    {:?}", t);
            }
        }
    }
    Ok(())
}
