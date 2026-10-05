//! List a map's reflection probes and sky image: probeinfo <zone>
use anyhow::Result;
use iw3::zone::{ParseOptions, Zone};

fn main() -> Result<()> {
    let zone_name = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let install = iw3::Install::locate()?;
    let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(&zone_name))?, ParseOptions::default())?;
    let w = zone.gfx_world().expect("gfxworld");
    let describe = |id: Option<iw3::zone::AssetId>| {
        id.and_then(|i| zone.image(i)).map(|img| {
            let d = img.load_def.as_ref();
            format!(
                "{} {}x{}x{} {:?} levels {:?} bytes {:?} format {:?}",
                img.name, img.width, img.height, img.depth, img.map_type,
                d.map(|d| d.level_count), d.map(|d| d.data.len()), d.map(|d| d.format)
            )
        })
    };
    println!("sky: {:?}", describe(w.sky_image));
    let mut used = std::collections::BTreeMap::new();
    for s in &w.surfaces {
        *used.entry(s.reflection_probe_index).or_insert(0) += 1;
    }
    println!("{} probes; surfaces per probe index: {:?}", w.reflection_probes.len(), used);
    for (i, p) in w.reflection_probes.iter().enumerate().take(4) {
        println!("probe {i} at {:?}: {:?}", p.origin, describe(p.image));
    }
    Ok(())
}
