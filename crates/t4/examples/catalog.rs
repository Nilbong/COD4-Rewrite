//! `cargo run --release -p t4 --example catalog`: load World at War's
//! multiplayer content, list the guns, attachments and characters, and check
//! that every model converts, every texture it uses decodes and every
//! viewmodel animation the guns name decodes.

use std::collections::HashSet;
use t4::catalog::Catalog;
use t4::zone::{AssetType, ParseOptions};

fn main() -> anyhow::Result<()> {
    let install = t4::Install::locate()?;
    let t0 = std::time::Instant::now();
    let zones: Vec<(&str, iw3::zone::Zone)> = ["common_mp"]
        .into_iter()
        .chain(t4::catalog::CHARACTER_ZONES)
        .map(|z| t4::load_iw3(&install, z).map(|c| (z, c)))
        .collect::<anyhow::Result<_>>()?;
    let vfs = install.vfs()?;
    let weapons = t4::weapons::mp_weapons(&vfs);
    // The character scripts are raw files in `common_mp`.
    let common_raw = t4::zone::Zone::parse(&t4::fastfile::load(&install.zone_path("common_mp"))?, ParseOptions::default())?;
    let scripts: Vec<(String, String)> = common_raw
        .of_type(AssetType::RawFile)
        .map(|(_, a)| (a.name.clone(), String::from_utf8_lossy(a.root.bytes("buffer")).into_owned()))
        .collect();
    let scripts: Vec<(&str, &str)> = scripts.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
    let models = |z: &iw3::zone::Zone| -> Vec<String> {
        z.assets.iter().filter(|a| matches!(a, iw3::zone::Asset::XModel(_))).map(|a| a.name().to_owned()).collect()
    };
    let common = models(&zones[0].1);
    let map_models: Vec<(&str, Vec<String>)> = zones[1..].iter().map(|(n, z)| (*n, models(z))).collect();
    let maps: Vec<(&str, Vec<&str>)> = map_models.iter().map(|(n, m)| (*n, m.iter().map(String::as_str).collect())).collect();
    let catalog = Catalog::build(&weapons, &common.iter().map(String::as_str).collect::<Vec<_>>(), &scripts, &maps);
    println!("loaded {} zones in {:.2?}: {} weapon files\n", zones.len(), t0.elapsed(), weapons.len());

    for g in &catalog.guns {
        let atts: Vec<&str> = g.attachments.keys().map(String::as_str).collect();
        println!("{:<22} {:<15} {:<28} {}", g.name, g.class, g.view_model, atts.join(" "));
    }
    println!();
    for e in &catalog.equipment {
        println!("{:<22} {:<15} {:<28} {}", e.name, e.class, e.view_model, e.world_model);
    }
    println!();
    for c in &catalog.characters {
        println!("{:<20} {:<20} {:<36} {:<36} {}", c.name, c.zone, c.body, c.head, c.view_arms);
    }
    println!();

    // Every model present, with surfaces, materials and decodable textures.
    let mut problems = Vec::new();
    let mut textures = HashSet::new();
    for name in catalog.models() {
        let Some((converted, id)) = zones.iter().find_map(|(_, z)| Some((z, z.find(name)?))) else {
            problems.push(format!("missing model {name}"));
            continue;
        };
        let Some(xm) = converted.xmodel(id) else { continue };
        if xm.surfs.is_empty() || xm.surfs.iter().all(|s| s.verts.is_empty()) {
            problems.push(format!("{name}: no geometry"));
        }
        for mat in xm.materials.iter().flatten().filter_map(|&m| converted.material(m)) {
            // `,name` is a reference to another zone's copy.
            let name = mat.name.trim_start_matches(',');
            let Some((z, mat)) = zones.iter().find_map(|(_, z)| z.material(z.find(name)?).map(|m| (z, m))) else {
                problems.push(format!("{}: material {name} not found", xm.name));
                continue;
            };
            for t in &mat.textures {
                if let Some(img) = t.image.and_then(|i| z.image(i)) {
                    textures.insert(img.name.clone());
                }
            }
        }
    }
    let mut decoded = 0;
    for name in &textures {
        let path = format!("images/{}.iwi", name.trim_start_matches(','));
        match vfs.read(&path).ok().flatten().map(|d| iw3::iwi::Iwi::parse(&d)) {
            Some(Ok(_)) => decoded += 1,
            Some(Err(e)) => problems.push(format!("{path}: {e}")),
            // Some images live in the zone itself (`loadDef`).
            None => {
                let inline = zones.iter().any(|(_, z)| z.find(name).and_then(|i| z.image(i)).is_some_and(|i| i.load_def.is_some()));
                if !inline {
                    problems.push(format!("{path}: not found"));
                }
            }
        }
    }
    println!("{} models checked, {decoded}/{} textures decode", catalog.models().count(), textures.len());

    // The guns' viewmodel animations, through CoD4's decoder.
    let (mut anims_ok, mut anims_total) = (0, 0);
    let wanted: HashSet<&str> = catalog
        .guns
        .iter()
        .flat_map(|g| std::iter::once(format!("{}_mp", g.name)).chain(g.attachments.values().cloned()))
        .filter_map(|v| weapons.iter().find(|w| w.name == v))
        .flat_map(|w| w.keys().filter(|k| k.ends_with("Anim")).filter_map(|k| w.anim(k)))
        .collect();
    let common_zone = &zones[0].1;
    for anim in wanted {
        anims_total += 1;
        // Weapon files spell animation names in any case.
        let found = common_zone.assets.iter().find(|a| a.name().eq_ignore_ascii_case(anim));
        let Some(iw3::zone::Asset::Generic(g)) = found else {
            problems.push(format!("animation {anim}: not found"));
            continue;
        };
        match iw3::xanim::XAnim::from_node(&g.name, &g.root, &common_zone.script_strings) {
            Ok(_) => anims_ok += 1,
            Err(e) => problems.push(format!("animation {anim}: {e:#}")),
        }
    }
    println!("{anims_ok}/{anims_total} gun animations decode");
    for p in problems.iter().take(40) {
        println!("  problem: {p}");
    }
    println!("{} problems", problems.len());
    Ok(())
}
