//! List world materials with their render state and triangle counts.
use anyhow::Result;
use iw3::zone::{ParseOptions, Zone};
use std::collections::HashMap;

fn main() -> Result<()> {
    let install = iw3::Install::locate()?;
    let data = iw3::fastfile::load(&install.zone_path(&std::env::args().nth(1).unwrap_or("mp_killhouse".into())))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    let w = zone.gfx_world().unwrap();
    let mut tris: HashMap<usize, (u32, f32)> = HashMap::new();
    for s in &w.surfaces {
        if let Some(m) = s.material {
            let e = tris.entry(m).or_default();
            e.0 += s.tri_count as u32;
            // horizontal-ness: bounds z extent small vs xy
            let dz = s.bounds[1][2] - s.bounds[0][2];
            let dxy = (s.bounds[1][0] - s.bounds[0][0]).max(s.bounds[1][1] - s.bounds[0][1]);
            if dz < 8.0 && dxy > 256.0 {
                e.1 += dxy;
            }
        }
    }
    let mut v: Vec<_> = tris.into_iter().collect();
    v.sort_by(|a, b| b.1.1.total_cmp(&a.1.1));
    for (m, (t, flat)) in v.iter().take(25) {
        let mat = zone.material(*m).unwrap();
        let ts = mat.technique_set.and_then(|t| zone.technique_set(t)).map(|t| t.name.as_str()).unwrap_or("-");
        let sb: Vec<String> = [7usize, 8, 4, 5]
            .iter()
            .map(|&t| mat.state_bits_for(t).map(|b| format!("{:08x}:{:08x}", b[0], b[1])).unwrap_or("-".into()))
            .collect();
        let tex: Vec<String> = mat
            .textures
            .iter()
            .map(|t| {
                format!(
                    "{:?}={}",
                    t.semantic,
                    t.image.and_then(|i| zone.image(i)).map(|i| i.name.as_str()).unwrap_or("?")
                )
            })
            .collect();
        println!("{t:6} flat{flat:7.0} {} [{ts}] sb {sb:?} {tex:?}", mat.name);
    }
    Ok(())
}
