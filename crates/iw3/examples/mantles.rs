//! Mantle brushes in a map and their surface flags: mantles <map> [cases.txt]
//! With a file, also writes `COD4RW_LIPTEST` cases walking into each one
//! from both sides along its thin axis (`x y z dx dy rise`).
fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&map))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let clip = zone.clip_map().expect("clip map");
    let mut n = 0;
    let mut cases = String::new();
    for (i, b) in clip.brushes.iter().enumerate() {
        if b.contents & 0x1000000 == 0 {
            continue;
        }
        n += 1;
        let axial = b.axial_materials.iter().flatten().filter(|&&m| m >= 0).map(|&m| m as u32);
        let mats: std::collections::BTreeSet<String> = b
            .side_materials
            .iter()
            .copied()
            .chain(axial)
            .filter_map(|m| clip.materials.get(m as usize))
            .map(|m| format!("{}:{:#x}", m.name, m.surface_flags))
            .collect();
        let (dx, dy) = (b.maxs[0] - b.mins[0], b.maxs[1] - b.mins[1]);
        let across = if dx < dy { [1.0f32, 0.0] } else { [0.0, 1.0] };
        let c = [(b.mins[0] + b.maxs[0]) / 2.0, (b.mins[1] + b.maxs[1]) / 2.0];
        for s in [1.0f32, -1.0] {
            let back = dx.min(dy) / 2.0 + 24.0;
            cases.push_str(&format!(
                "{:.1} {:.1} {:.1} {} {} {:.0}
",
                c[0] - s * across[0] * back,
                c[1] - s * across[1] * back,
                b.mins[2] + 1.0,
                s * across[0],
                s * across[1],
                b.maxs[2] - b.mins[2]
            ));
        }
        if n <= 6 {
            println!("brush {i}: contents {:#x} mins {:?} maxs {:?} mats {:?}", b.contents, b.mins, b.maxs, mats);
        }
    }
    println!("{n} mantle brushes");
    if let Some(path) = std::env::args().nth(2) {
        std::fs::write(path, cases)?;
    }
    Ok(())
}
