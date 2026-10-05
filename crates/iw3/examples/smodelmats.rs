//! A map's static models and their surfaces' materials and blend states:
//! smodelmats <zone> [model substring]
use std::collections::BTreeMap;

use anyhow::Result;
use iw3::zone::{self, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or("mp_killhouse".into());
    let pat = args.next().unwrap_or_default();
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?, ParseOptions::default())?;
    let world = zone.gfx_world().expect("gfxworld");
    // World surfaces whose material matches `WORLD`: where they are.
    if let Ok(wanted) = std::env::var("WORLD") {
        for s in &world.surfaces {
            let Some(mat) = s.material.and_then(|m| zone.material(m)) else { continue };
            if !mat.name.contains(&wanted) {
                continue;
            }
            let first = s.first_vertex.max(0) as usize;
            let start = s.base_index.max(0) as usize;
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for &i in world.indices.get(start..start + s.tri_count as usize * 3).unwrap_or(&[]) {
                let Some(v) = world.vertices.get(first + i as usize) else { continue };
                for k in 0..3 {
                    lo[k] = lo[k].min(v.xyz[k]);
                    hi[k] = hi[k].max(v.xyz[k]);
                }
            }
            println!("world {}: {lo:.0?}..{hi:.0?}", mat.name);
        }
    }
    let mut models: BTreeMap<String, (usize, [f32; 3])> = BTreeMap::new();
    let mut ids = BTreeMap::new();
    for sm in &world.static_models {
        let Some(id) = sm.model else { continue };
        let Some(xm) = zone.xmodel(id) else { continue };
        if !xm.name.contains(&pat) {
            continue;
        }
        models.entry(xm.name.clone()).or_insert((0, sm.origin)).0 += 1;
        if std::env::var("POS").is_ok() {
            println!("{} at {:.0?}", xm.name, sm.origin);
        }
        ids.insert(xm.name.clone(), id);
    }
    for (name, (count, at)) in &models {
        let xm = zone.xmodel(ids[name]).unwrap();
        let lod = xm.lods.first().copied();
        println!("{name} x{count} (first at {at:.0?}) lod0 {lod:?}");
        for (i, m) in xm.materials.iter().enumerate() {
            let Some(mat) = m.and_then(|m| zone.material(m)) else { continue };
            let ts = mat.technique_set.and_then(|t| zone.technique_set(t)).map(|t| t.name.as_str()).unwrap_or("-");
            let bits = [zone::TECHNIQUE_LIT, 8, zone::TECHNIQUE_UNLIT, zone::TECHNIQUE_EMISSIVE]
                .into_iter()
                .find_map(|t| mat.state_bits_for(t))
                .or_else(|| mat.state_bits.first().copied());
            let decoded = bits.map(|[b0, _]| (b0 & 0xf, (b0 >> 4) & 0xf, (b0 >> 8) & 7, b0 & 0x3000));
            let tex: Vec<_> = mat
                .textures
                .iter()
                .map(|t| format!("{:?}={}", t.semantic, t.image.and_then(|i| zone.image(i)).map_or("-", |i| i.name.as_str())))
                .collect();
            let surf = xm.surfs.get(i);
            println!(
                "  surf {i}: {} ({ts}) sort {} bits {:08x?} (src,dst,op,atest) {decoded:?} verts {} entries {:?}\n    {}",
                mat.name,
                mat.sort_key,
                bits,
                surf.map_or(0, |s| s.verts.len()),
                mat.state_bits_entry,
                tex.join(" "),
            );
            if std::env::var("VERTS").is_ok() && mat.sort_key >= 40 {
                for v in surf.map_or(&[][..], |s| &s.verts[..]) {
                    println!("      xyz {:6.1?} color {:?} uv {:?}", v.xyz, iw3::unpack::color(v.color), iw3::unpack::tex_coords(v.tex_coord));
                }
            }
        }
    }
    Ok(())
}
