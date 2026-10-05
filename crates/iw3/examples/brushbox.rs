//! Collision brushes touching a box: brushbox <map> <x0> <y0> <z0> <x1> <y1> <z1> (CoD units)
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let v: Vec<f32> = a[1..7].iter().map(|s| s.parse().unwrap()).collect();
    let (lo, hi) = ([v[0].min(v[3]), v[1].min(v[4]), v[2].min(v[5])], [v[0].max(v[3]), v[1].max(v[4]), v[2].max(v[5])]);
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let clip = zone.clip_map().expect("clip map");
    let entity = clip.entity_brushes();
    for (i, b) in clip.brushes.iter().enumerate() {
        if (0..3).all(|k| b.mins[k] <= hi[k] && lo[k] <= b.maxs[k]) {
            let mats: std::collections::BTreeSet<String> = b
                .side_materials
                .iter()
                .map(|&m| m as i64)
                .chain(b.axial_materials.iter().flatten().map(|&m| m as i64))
                .filter_map(|m| usize::try_from(m).ok().and_then(|m| clip.materials.get(m)).map(|m| m.name.clone()))
                .collect();
            println!(
                "brush {i}: contents {:#x} mins {:?} maxs {:?} sides {} entity {} {mats:?}",
                b.contents, b.mins, b.maxs, b.side_planes.len(), entity.contains(&(i as u32))
            );
        }
    }
    Ok(())
}
