//! Which collision brushes contain a point: brushat <map> <x> <y> <z> (CoD units)
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let p: Vec<f32> = a[1..4].iter().map(|s| s.parse().unwrap()).collect();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let clip = zone.clip_map().expect("clip map");
    let entity = clip.entity_brushes();
    println!("{} models, {} entity brushes", clip.cmodels.len(), entity.len());
    for (i, b) in clip.brushes.iter().enumerate() {
        if (0..3).all(|k| b.mins[k] - 1.0 <= p[k] && p[k] <= b.maxs[k] + 1.0) {
            let inside = b.side_planes.iter().all(|&s| {
                let pl = &clip.planes[s as usize];
                pl.normal[0] * p[0] + pl.normal[1] * p[1] + pl.normal[2] * p[2] - pl.dist <= 1.0
            });
            println!("brush {i}: contents {:#x} mins {:?} maxs {:?} sides {} inside planes {inside} entity {}", b.contents, b.mins, b.maxs, b.side_planes.len(), entity.contains(&(i as u32)));
        }
    }
    println!("{} tris", clip.tri_indices.len() / 3);
    Ok(())
}
