//! The map editor's palette index: every model with geometry in each of the
//! install's level zones (multiplayer and single-player), with its box, and
//! (to a second file) every animation's name per zone.
//!
//! editorindex <models out> [anims out]
//!
//! Lines: `z <name> <compressed bytes>` per zone (numbered from 0 in
//! order), then `<model> <zone,zone..> minx miny minz maxx maxy maxz bones`
//! (CoD units), once per model; anims: `<zone> <anim>`.
use anyhow::Result;
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};
use std::collections::BTreeMap;
use std::fmt::Write as _;

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let out = a.next().unwrap_or("catalog.txt".into());
    let anims_out = a.next();
    let install = iw3::Install::locate()?;
    let dir = install.root.join("zone").join("english");
    let mut zones: Vec<(String, u64)> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().strip_suffix(".ff")?.to_owned();
            let skip = name.ends_with("_load") || name.starts_with("localized") || name.starts_with("code_") || name.starts_with("ui") || name == "simplecredits";
            (!skip).then(|| (name, e.metadata().map(|m| m.len()).unwrap_or(0)))
        })
        .collect();
    zones.sort();
    let (mut text, mut anims) = (String::new(), String::new());
    let mut models: BTreeMap<String, (Vec<usize>, String)> = BTreeMap::new();
    let mut kept = Vec::new();
    for (zone, bytes) in &zones {
        let t0 = std::time::Instant::now();
        let parsed = iw3::fastfile::load(&install.zone_path(zone)).and_then(|d| Zone::parse(&d, ParseOptions::default()));
        let z = match parsed {
            Ok(z) => z,
            Err(e) => {
                eprintln!("{zone}: {e}");
                continue;
            }
        };
        let zi = kept.len();
        kept.push((zone.clone(), *bytes));
        let mut n = 0;
        for asset in &z.assets {
            match asset {
                Asset::XModel(m) if !m.lods.is_empty() && !m.surfs.is_empty() => {
                    let (lo, hi) = (m.mins, m.maxs);
                    let e = models.entry(m.name.to_ascii_lowercase()).or_insert_with(|| (Vec::new(), format!("{:.0} {:.0} {:.0} {:.0} {:.0} {:.0} {}", lo[0], lo[1], lo[2], hi[0], hi[1], hi[2], m.num_bones)));
                    if !e.0.contains(&zi) {
                        e.0.push(zi);
                    }
                    n += 1;
                }
                Asset::Generic(g) if g.ty == AssetType::XAnimParts => {
                    writeln!(anims, "{zone} {}", g.name)?;
                }
                _ => {}
            }
        }
        eprintln!("{zone}: {n} models ({:.1} s)", t0.elapsed().as_secs_f32());
    }
    for (zone, bytes) in &kept {
        writeln!(text, "z {zone} {bytes}")?;
    }
    for (name, (zs, rest)) in &models {
        let zs: Vec<String> = zs.iter().map(|z| z.to_string()).collect();
        writeln!(text, "{name} {} {rest}", zs.join(","))?;
    }
    std::fs::write(&out, text)?;
    if let Some(p) = anims_out {
        std::fs::write(p, anims)?;
    }
    Ok(())
}
