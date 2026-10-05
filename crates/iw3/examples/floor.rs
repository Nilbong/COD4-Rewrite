//! Which world surfaces lie under a point (default: the first allies start spawn)?
use anyhow::Result;
use iw3::zone::{ParseOptions, Zone};

fn main() -> Result<()> {
    let install = iw3::Install::locate()?;
    let data = iw3::fastfile::load(&install.zone_path("mp_killhouse"))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    let w = zone.gfx_world().unwrap();
    let ents = iw3::ents::parse(&zone.map_ents().unwrap().entity_string);
    let sp = ents.iter().find(|e| e.classname() == "mp_tdm_spawn_allies_start").unwrap().origin().unwrap();
    println!("spawn {sp:?}");
    for (i, s) in w.surfaces.iter().enumerate() {
        let b = s.bounds;
        if b[0][0] <= sp[0]
            && sp[0] <= b[1][0]
            && b[0][1] <= sp[1]
            && sp[1] <= b[1][1]
            && b[0][2] < sp[2] + 4.0
            && b[1][2] > sp[2] - 64.0
        {
            let m = zone.material(s.material.unwrap()).unwrap();
            // vertex alpha stats
            let first = s.first_vertex as usize;
            let mut alphas = vec![];
            for t in 0..(s.tri_count as usize * 3) {
                let vi = first + w.indices[s.base_index as usize + t] as usize;
                alphas.push(w.vertices[vi].color >> 24);
            }
            let amin = alphas.iter().min().unwrap();
            let amax = alphas.iter().max().unwrap();
            println!(
                "surf {i} {} z {:.0}..{:.0} tris {} flags {:#x} alpha {amin}..{amax} in lit {}",
                m.name,
                b[0][2],
                b[1][2],
                s.tri_count,
                s.flags,
                w.lit_surfs.contains(&(i as u32))
            );
        }
    }
    println!("lit {:?} decal {:?} emissive {:?}", w.lit_surfs, w.decal_surfs, w.emissive_surfs);
    Ok(())
}
