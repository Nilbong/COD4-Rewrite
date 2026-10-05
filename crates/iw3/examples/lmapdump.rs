//! Dump a map's embedded lightmap pixels to raw files: lmapdump <zone> <out dir>
use anyhow::Result;
use iw3::zone::{ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or("mp_killhouse".into());
    let out = std::path::PathBuf::from(args.next().expect("output dir"));
    std::fs::create_dir_all(&out)?;
    let install = iw3::Install::locate()?;
    let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(&zone_name))?, ParseOptions::default())?;
    let w = zone.gfx_world().expect("gfxworld");
    for (i, l) in w.lightmaps.iter().enumerate() {
        for (kind, id) in [("primary", l.primary), ("secondary", l.secondary)] {
            let Some(img) = id.and_then(|id| zone.image(id)) else { continue };
            let Some(def) = &img.load_def else { continue };
            let name = format!("{i}_{kind}_{}x{}_f{}.raw", img.width, img.height, def.format);
            std::fs::write(out.join(&name), &def.data)?;
            println!("{name}");
        }
    }
    for (i, p) in w.reflection_probes.iter().enumerate() {
        let Some(img) = p.image.and_then(|id| zone.image(id)) else { continue };
        let Some(def) = &img.load_def else { continue };
        std::fs::write(out.join(format!("probe{i}_{}.raw", img.width)), &def.data)?;
    }
    Ok(())
}
