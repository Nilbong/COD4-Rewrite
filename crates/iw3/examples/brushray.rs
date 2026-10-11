//! Brushes a horizontal ray from a point crosses: brushray <map> <x> <y> <z> <yaw deg> [len]
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let p: Vec<f32> = a[1..5].iter().map(|s| s.parse().unwrap()).collect();
    let len: f32 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(600.0);
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let clip = zone.clip_map().expect("clip map");
    let (s, c) = p[3].to_radians().sin_cos();
    let mut seen = std::collections::BTreeSet::new();
    let mut t = 0.0;
    while t < len {
        let q = [p[0] + c * t, p[1] + s * t, p[2]];
        for (i, b) in clip.brushes.iter().enumerate() {
            if (0..3).all(|k| q[k] >= b.mins[k] && q[k] <= b.maxs[k]) && b.maxs[0] - b.mins[0] < 2000.0 && seen.insert(i) {
                let mat = clip.materials.get(b.axial_materials[0][0].max(0) as usize);
                println!("at {t:.0}: brush {i} contents {:#x} bounds {:?}..{:?} material {:?}", b.contents, b.mins, b.maxs, mat.map(|m| (&m.name, m.surface_flags >> 20 & 31, m.content_flags)));
            }
        }
        t += 4.0;
    }
    Ok(())
}
