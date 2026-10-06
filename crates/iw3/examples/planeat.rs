//! Brush faces near a point facing a direction: planeat <map> <x> <y> <z> <nx> <ny> <nz> (CoD)
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let v: Vec<f32> = a[1..7].iter().map(|s| s.parse().unwrap()).collect();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let clip = zone.clip_map().expect("clip map");
    let n = [v[3], v[4], v[5]];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    for (i, b) in clip.brushes.iter().enumerate() {
        if !(0..3).all(|k| b.mins[k] - 40.0 <= v[k] && v[k] <= b.maxs[k] + 40.0) {
            continue;
        }
        for &s in &b.side_planes {
            let pl = &clip.planes[s as usize];
            let dot = (pl.normal[0] * n[0] + pl.normal[1] * n[1] + pl.normal[2] * n[2]) / len;
            if dot > 0.95 {
                let d = pl.normal[0] * v[0] + pl.normal[1] * v[1] + pl.normal[2] * v[2] - pl.dist;
                println!("brush {i} contents {:#x} plane normal {:?} dist {} (point {d:.1} in front) mins {:?} maxs {:?}", b.contents, pl.normal, pl.dist, b.mins, b.maxs);
            }
        }
    }
    println!("{} tris", clip.tri_indices.len() / 3);
    Ok(())
}
