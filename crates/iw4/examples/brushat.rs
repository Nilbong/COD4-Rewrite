//! Collision brushes around a point in a converted MW2 map: `brushat <map> x y z [r]`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let z = iw4::load_map(&iw4::Install::locate()?, &a[0])?;
    let p: Vec<f32> = a[1..4].iter().map(|v| v.parse().unwrap()).collect();
    let r: f32 = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(8.0);
    // Optional box half sizes per axis: x y z r rx ry rz.
    let rs: Vec<f32> = if a.len() >= 8 { a[5..8].iter().map(|v| v.parse().unwrap()).collect() } else { vec![r; 3] };
    let c = z.clip_map().unwrap();
    for (i, b) in c.brushes.iter().enumerate() {
        if (0..3).all(|k| p[k] >= b.mins[k] - rs[k] && p[k] <= b.maxs[k] + rs[k]) {
            let mats: Vec<String> = b.side_materials.iter().chain(b.axial_materials.iter().flatten().map(|m| *m as u32).collect::<Vec<_>>().iter())
                .filter_map(|&m| c.materials.get(m as usize)).map(|m| format!("{}({:#x}/{:#x})", m.name, m.surface_flags, m.content_flags)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            println!("brush {i} {:?}..{:?} contents {:#x} {:?}", b.mins, b.maxs, b.contents, mats);
        }
    }
    Ok(())
}
