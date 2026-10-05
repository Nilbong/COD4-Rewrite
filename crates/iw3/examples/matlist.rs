//! List world materials whose technique set matches a pattern, with their
//! constants and textures: matlist <zone> <pattern>
use anyhow::Result;
use iw3::zone::{ParseOptions, Zone};
use std::collections::BTreeMap;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or("mp_killhouse".into());
    let pat = args.next().unwrap_or("falloff".into());
    let install = iw3::Install::locate()?;
    let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(&zone_name))?, ParseOptions::default())?;
    let w = zone.gfx_world().expect("gfxworld");
    let mut tris: BTreeMap<String, u32> = BTreeMap::new();
    let mut bounds: BTreeMap<String, ([f32; 3], [f32; 3])> = BTreeMap::new();
    for s in &w.surfaces {
        let Some(m) = s.material.and_then(|m| zone.material(m)) else { continue };
        let ts = m.technique_set.and_then(|t| zone.technique_set(t)).map(|t| t.name.clone()).unwrap_or_default();
        if ts.contains(&pat) || m.name.contains(&pat) {
            *tris.entry(format!("{} ({ts})", m.name)).or_default() += s.tri_count as u32;
            let first = s.first_vertex.max(0) as usize;
            let start = s.base_index.max(0) as usize;
            let mut cs = [0f32; 4];
            let n = s.tri_count as usize * 3;
            for &i in &w.indices[start..start + n] {
                let c = iw3::unpack::color(w.vertices[first + i as usize].color);
                for k in 0..4 { cs[k] += c[k] / n as f32; }
            }
            println!("    surf vc avg {:.3?}", cs);
            let b = bounds.entry(m.name.clone()).or_insert(([f32::MAX; 3], [f32::MIN; 3]));
            for k in 0..3 {
                b.0[k] = b.0[k].min(s.bounds[0][k]);
                b.1[k] = b.1[k].max(s.bounds[1][k]);
            }
        }
    }
    for (name, t) in &tris {
        let mname = name.split(' ').next().unwrap();
        println!("{t:6} {name}  bounds {:?}", bounds.get(mname));
        if let Some(m) = zone.assets.iter().find_map(|a| match a { iw3::zone::Asset::Material(m) if m.name == mname => Some(m), _ => None }) {
            for c in &m.constants {
                println!("        const {:?}", c);
            }
            for t in &m.textures {
                println!("        tex {:?} {:?}", t.semantic, t.image.and_then(|i| zone.image(i)).map(|i| &i.name));
            }
        }
    }
    Ok(())
}
