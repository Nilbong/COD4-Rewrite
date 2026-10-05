//! Check that world material textures resolve to iwi files in the iwds.

use anyhow::Result;
use iw3::zone::{ParseOptions, TextureSemantic, Zone};
use std::collections::BTreeSet;

fn main() -> Result<()> {
    let install = iw3::Install::locate()?;
    let t = std::time::Instant::now();
    let vfs = iw3::iwd::Vfs::mount(&install.iwd_paths()?)?;
    println!("mounted iwds in {:?}", t.elapsed());
    let data = iw3::fastfile::load(&install.zone_path("mp_killhouse"))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    let world = zone.gfx_world().unwrap();
    let mats: BTreeSet<_> = world.surfaces.iter().filter_map(|s| s.material).collect();
    let (mut found, mut missing, mut inline) = (0, 0, 0);
    let mut formats = std::collections::BTreeMap::new();
    for &m in &mats {
        let mat = zone.material(m).unwrap();
        for t in &mat.textures {
            let Some(img) = t.image.and_then(|i| zone.image(i)) else { continue };
            if img.load_def.as_ref().is_some_and(|d| !d.data.is_empty()) {
                inline += 1;
                continue;
            }
            let path = format!("images/{}.iwi", img.name);
            match vfs.read(&path)? {
                Some(bytes) => {
                    found += 1;
                    match iw3::iwi::Iwi::parse(&bytes) {
                        Ok(iwi) => {
                            *formats
                                .entry(format!("{:?}{}", iwi.format, if iwi.is_cube() { " cube" } else { "" }))
                                .or_insert(0) += 1
                        }
                        Err(e) => println!("  bad iwi {path}: {e}"),
                    }
                    if t.semantic == TextureSemantic::Color && found < 4 {
                        println!("  {} -> {path}", mat.name);
                    }
                }
                None => {
                    missing += 1;
                    if missing < 10 {
                        println!("  missing {path} (material {})", mat.name);
                    }
                }
            }
        }
    }
    println!("{} world materials; textures found {found}, missing {missing}, inline {inline}", mats.len());
    println!("formats {formats:?}");
    // Sample packed texcoords from a static model to sanity check the half-float decode.
    let sm = world.static_models[0].model.and_then(|m| zone.xmodel(m)).unwrap();
    let v = &sm.surfs[0].verts[..4];
    for v in v {
        println!("  {} uv {:?} n {:?}", sm.name, iw3::unpack::tex_coords(v.tex_coord), iw3::unpack::unit_vec(v.normal));
    }
    Ok(())
}
