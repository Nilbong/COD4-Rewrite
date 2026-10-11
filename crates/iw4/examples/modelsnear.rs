//! Static models near a point in a converted MW2 map, with their materials'
//! technique sets and lit state bits: `modelsnear <map> x y z [r]`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let z = iw4::load_map(&iw4::Install::locate()?, &a[0])?;
    let p: Vec<f32> = a[1..4].iter().map(|v| v.parse().unwrap()).collect();
    let r: f32 = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(150.0);
    let w = z.gfx_world().unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for s in &w.static_models {
        if (0..3).all(|k| (s.origin[k] - p[k]).abs() < r) {
            let Some(m) = s.model.and_then(|m| z.xmodel(m)) else { continue };
            if !seen.insert(m.name.clone()) { continue; }
            println!("{} flags {:#x}", m.name, s.flags);
            for mat in m.materials.iter().flatten().filter_map(|&i| z.material(i)).collect::<Vec<_>>().iter().take(3) {
                let ts = mat.technique_set.and_then(|t| z.technique_set(t)).map_or("", |t| t.name.as_str());
                println!("   {} [{ts}] lit {:x?} unlit {:x?} entries {:?} statebits {:x?}", mat.name, mat.state_bits_for(7), mat.state_bits_for(4), &mat.state_bits_entry[..14], mat.state_bits);
            }
        }
    }
    Ok(())
}
