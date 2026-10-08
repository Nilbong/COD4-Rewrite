//! A map's brush models (gfx): surfaces, bounds, materials: brushmodels <zone> [index]
use iw3::zone::{ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?, ParseOptions::default())?;
    let only: Option<usize> = a.get(1).and_then(|s| s.parse().ok());
    let w = zone.gfx_world().expect("gfxworld");
    println!("{} brush models, {} surfaces", w.models.len(), w.surfaces.len());
    for (i, m) in w.models.iter().enumerate() {
        if only.is_some_and(|o| o != i) {
            continue;
        }
        println!("model {i}: surfaces {}..+{} bounds {:?}", m.start_surf_index, m.surface_count, m.bounds);
        if only.is_some() {
            for s in w.surfaces.iter().skip(m.start_surf_index as usize).take(m.surface_count as usize) {
                let name = s.material.and_then(|m| zone.material(m)).map_or("?".into(), |m| m.name.clone());
                println!("   {name} bounds {:?}", s.bounds);
            }
        }
    }
    Ok(())
}
