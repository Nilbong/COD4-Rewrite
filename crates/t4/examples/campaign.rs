//! `cargo run --release -p t4 --example campaign`: find World at War's
//! campaign characters across the levels, list them, and check that every
//! model they may use converts and every texture it uses decodes.

use std::collections::{BTreeMap, HashSet};
use t4::campaign::{CAMPAIGN_ZONES, ZoneContents};
use t4::zone::{AssetType, ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let install = t4::Install::locate()?;
    let t0 = std::time::Instant::now();
    // Pass 1: each level's model names and raw files.
    let mut contents: Vec<(String, Vec<String>, Vec<(String, String)>)> = Vec::new();
    for name in CAMPAIGN_ZONES {
        let zone = Zone::parse(&t4::fastfile::load(&install.zone_path(name))?, ParseOptions::default())?;
        if let Some(stop) = &zone.stats.stopped_at {
            anyhow::bail!("{name}: parsing stopped at {stop}");
        }
        let models = zone.of_type(AssetType::XModel).map(|(_, a)| a.name.clone()).collect();
        let raw = zone
            .of_type(AssetType::RawFile)
            .filter(|(_, a)| a.name.starts_with("character/") || a.name.starts_with("xmodelalias/"))
            .map(|(_, a)| (a.name.clone(), String::from_utf8_lossy(a.root.bytes("buffer")).into_owned()))
            .collect();
        contents.push((name.to_owned(), models, raw));
    }
    let zones: Vec<ZoneContents> = contents
        .iter()
        .map(|(n, m, r)| (n.as_str(), m.iter().map(String::as_str).collect(), r.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect()))
        .collect();
    let characters = t4::campaign::characters(&zones);
    println!("{} levels read in {:.2?}: {} characters\n", zones.len(), t0.elapsed(), characters.len());
    let first = |c: &[String]| c.first().cloned().unwrap_or_default();
    for c in &characters {
        let l = &c.look;
        let n = |c: &[String]| if c.len() > 1 { format!(" (+{})", c.len() - 1) } else { String::new() };
        println!(
            "{:<32} {:<7} {:<9} {}{} | {}{} | {}{} | {}{}",
            c.name, c.zone, l.voice, first(&l.body), n(&l.body), first(&l.head), n(&l.head), first(&l.hat), n(&l.hat), first(&l.gear), n(&l.gear)
        );
    }

    // Pass 2: convert the zones the characters use and check their models.
    let vfs = install.vfs()?;
    let mut by_zone: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for c in &characters {
        by_zone.entry(c.zone.as_str()).or_default().extend(c.look.models());
    }
    let (mut problems, mut checked, mut decoded, mut textures) = (Vec::new(), 0, 0, 0);
    for (zone_name, models) in by_zone {
        let zone = t4::load_iw3(&install, zone_name)?;
        let mut images = HashSet::new();
        for name in models.into_iter().collect::<HashSet<_>>() {
            checked += 1;
            let Some(xm) = zone.find(name).and_then(|i| zone.xmodel(i)) else {
                problems.push(format!("{zone_name}: missing model {name}"));
                continue;
            };
            if xm.surfs.iter().all(|s| s.verts.is_empty()) {
                problems.push(format!("{name}: no geometry"));
            }
            for mat in xm.materials.iter().flatten().filter_map(|&m| zone.material(m)) {
                for t in &mat.textures {
                    if let Some(img) = t.image.and_then(|i| zone.image(i)) {
                        images.insert((img.name.clone(), img.load_def.is_some()));
                    }
                }
            }
        }
        for (name, inline) in images {
            textures += 1;
            let path = format!("images/{}.iwi", name.trim_start_matches(','));
            match vfs.read(&path).ok().flatten().map(|d| iw3::iwi::Iwi::parse(&d)) {
                Some(Ok(_)) => decoded += 1,
                Some(Err(e)) => problems.push(format!("{path}: {e}")),
                None if inline => decoded += 1,
                None => problems.push(format!("{path}: not found")),
            }
        }
    }
    println!("\n{checked} models checked, {decoded}/{textures} textures decode");
    for p in problems.iter().take(40) {
        println!("  problem: {p}");
    }
    println!("{} problems", problems.len());
    Ok(())
}
