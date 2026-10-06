//! World surfaces near a point, by material: surfat <zone> <x> <y> <z> [radius]
use iw3::zone::{ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?, ParseOptions::default())?;
    let p: Vec<f32> = a[1..4].iter().map(|s| s.parse().unwrap()).collect();
    let r: f32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(200.0);
    let world = zone.gfx_world().expect("gfxworld");
    let mut found = std::collections::BTreeMap::<String, (usize, u32, [[f32; 3]; 2])>::new();
    for s in &world.surfaces {
        let [lo, hi] = s.bounds;
        let d: f32 = (0..3).map(|i| (p[i] - p[i].clamp(lo[i], hi[i])).powi(2)).sum::<f32>().sqrt();
        if d > r {
            continue;
        }
        let name = s.material.and_then(|m| zone.material(m)).map_or("?".into(), |m| {
            let t = m.technique_set.and_then(|t| zone.technique_set(t)).map_or("", |t| t.name.as_str());
            format!("{} [{t}]", m.name)
        });
        let e = found.entry(name).or_insert((0, 0, s.bounds));
        e.0 += 1;
        e.1 += s.tri_count as u32;
        for i in 0..3 {
            e.2[0][i] = e.2[0][i].min(lo[i]);
            e.2[1][i] = e.2[1][i].max(hi[i]);
        }
    }
    for (name, (n, tris, b)) in found {
        println!("{name}: {n} surfaces, {tris} tris, bounds {:?}..{:?}", b[0].map(f32::round), b[1].map(f32::round));
    }
    Ok(())
}
