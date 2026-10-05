//! `cargo run --release -p t5 --example catalog`: load Black Ops' multiplayer
//! content, list the guns, attachments and characters, and check that every
//! model converts and every texture it uses decodes.

use std::collections::HashSet;
use t5::catalog::Catalog;

fn main() -> anyhow::Result<()> {
    let install = t5::Install::locate()?;
    let t0 = std::time::Instant::now();
    let zones: Vec<(&str, iw3::zone::Zone)> = ["common_mp", t5::catalog::HANDS_ZONE]
        .into_iter()
        .chain(t5::catalog::CHARACTER_ZONES)
        .map(|z| t5::load_iw3(&install, z).map(|c| (z, c)))
        .collect::<anyhow::Result<_>>()?;
    let vfs = install.vfs()?;
    let weapons = t5::weapons::mp_weapons(&vfs);
    let models = |z: &iw3::zone::Zone| -> Vec<String> {
        z.assets.iter().filter(|a| matches!(a, iw3::zone::Asset::XModel(_))).map(|a| a.name().to_owned()).collect()
    };
    let common = models(&zones[0].1);
    let map_models: Vec<(&str, Vec<String>)> = zones[2..].iter().map(|(n, z)| (*n, models(z))).collect();
    let maps: Vec<(&str, Vec<&str>)> = map_models.iter().map(|(n, m)| (*n, m.iter().map(String::as_str).collect())).collect();
    let catalog = Catalog::build(&weapons, &common.iter().map(String::as_str).collect::<Vec<_>>(), &maps);
    println!("loaded {} zones in {:.2?}: {} weapon files\n", zones.len(), t0.elapsed(), weapons.len());

    for g in &catalog.guns {
        let atts: Vec<&str> = g.attachments.keys().map(String::as_str).collect();
        println!("{:<12} {:<15} {:<34} {}", g.name, g.class, g.view_model, atts.join(" "));
    }
    println!();
    for f in &catalog.factions {
        println!("{:<14} {:<15} {} bodies, {} heads", f.name, f.zone, f.bodies.len(), f.heads.len());
    }
    println!("view hands: {:?}\n", catalog.view_hands);

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
                problems.push(format!("{name}: material not found"));
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
    for p in problems.iter().take(40) {
        println!("  problem: {p}");
    }
    println!("{} problems", problems.len());
    Ok(())
}
