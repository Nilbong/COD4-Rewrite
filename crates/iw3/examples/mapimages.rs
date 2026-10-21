//! Images a map's surfaces use (world, static models, other map props;
//! weapons and characters left out), one per line:
//! `<semantic> <image> <material> <user>`. mapimages <zone>
use std::collections::BTreeSet;

use iw3::zone::{Asset, ParseOptions, TextureSemantic, Zone};

fn main() -> anyhow::Result<()> {
    let name = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let install = iw3::Install::locate()?;
    println!("# main {}", install.main_dir().display());
    let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(&name))?, ParseOptions::default())?;
    let world = zone.gfx_world().expect("gfxworld");
    let mut mats: Vec<(iw3::zone::AssetId, String)> = Vec::new();
    for s in &world.surfaces {
        if let Some(m) = s.material {
            mats.push((m, "world".into()));
        }
    }
    let skip = ["viewmodel_", "weapon_", "body_", "head_", "viewhands", "projectile_", "worldmodel_", "character_", "playermodel"];
    for a in &zone.assets {
        if let Asset::XModel(xm) = a {
            if skip.iter().any(|p| xm.name.starts_with(p)) {
                continue;
            }
            for m in xm.materials.iter().flatten() {
                mats.push((*m, xm.name.clone()));
            }
        }
    }
    let mut out = BTreeSet::new();
    for (id, user) in mats {
        let Some(m) = zone.material(id) else { continue };
        for t in &m.textures {
            let sem = match t.semantic {
                TextureSemantic::Color => "color",
                TextureSemantic::Normal => "normal",
                TextureSemantic::Specular => "spec",
                _ => continue,
            };
            let Some(img) = t.image.and_then(|i| zone.image(i)) else { continue };
            out.insert(format!("{sem} {} {} {user}", img.name, m.name));
        }
    }
    for l in out {
        println!("{l}");
    }
    Ok(())
}
