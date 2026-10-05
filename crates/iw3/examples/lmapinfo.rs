//! List a map's lightmap images: lmapinfo <zone>
use anyhow::Result;
use iw3::zone::{ParseOptions, Zone};

fn main() -> Result<()> {
    let zone_name = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let install = iw3::Install::locate()?;
    let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(&zone_name))?, ParseOptions::default())?;
    let w = zone.gfx_world().expect("gfxworld");
    for (i, l) in w.lightmaps.iter().enumerate() {
        for (kind, id) in [("primary", l.primary), ("secondary", l.secondary)] {
            let Some(img) = id.and_then(|id| zone.image(id)) else { println!("{i} {kind}: none"); continue };
            let def = img.load_def.as_ref();
            println!(
                "{i} {kind}: {} {}x{} map {:?} embedded bytes {:?} format {:?}",
                img.name, img.width, img.height, img.map_type, def.map(|d| d.data.len()), def.map(|d| d.format)
            );
        }
    }
    let used = w.surfaces.iter().filter(|s| s.lightmap_index != 31 && s.lightmap_index != 255).count();
    println!("{} surfaces, {used} with a lightmap index", w.surfaces.len());
    Ok(())
}
