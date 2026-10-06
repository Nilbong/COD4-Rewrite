//! Bullet impacts by surface: the impact table's effect per row and
//! surface, and whether each impact sound alias exists: impactaudit [map]
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

const SURFACES: [&str; 29] = [
    "default", "bark", "brick", "carpet", "cloth", "concrete", "dirt", "flesh", "foliage", "glass", "grass", "gravel",
    "ice", "metal", "mud", "paper", "plaster", "rock", "sand", "snow", "water", "wood", "asphalt", "ceramic", "plastic",
    "rubber", "cushion", "fruit", "paintedmetal",
];

fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or("mp_crossfire".into());
    let install = iw3::Install::locate()?;
    let zones: Vec<Zone> = ["common_mp", "localized_common_mp", &map]
        .iter()
        .filter_map(|z| Zone::parse(&iw3::fastfile::load(&install.zone_path(z)).ok()?, ParseOptions::default()).ok())
        .collect();
    let table = zones.iter().find_map(|z| {
        z.assets.iter().find_map(|a| match a {
            Asset::Generic(g) if g.ty == AssetType::ImpactFx => Some(iw3::fx::ImpactTable::from_node(z, &g.root)),
            _ => None,
        })
    });
    let sounds: std::collections::HashSet<String> = zones
        .iter()
        .flat_map(|z| z.assets.iter())
        .filter_map(|a| match a {
            Asset::Generic(g) if g.ty == AssetType::Sound => Some(g.name.to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    if let Some(t) = &table {
        println!("impact table {} with {} rows", t.name, t.entries.len());
        for (row, label) in [(0, "bullet_small"), (2, "bullet_large"), (4, "shotgun"), (6, "bullet_ap")] {
            let Some(e) = t.entries.get(row) else { continue };
            let missing: Vec<&str> = SURFACES.iter().enumerate().filter(|(i, _)| e.nonflesh.get(*i).cloned().flatten().is_none()).map(|(_, s)| *s).collect();
            println!("fx row {row} ({label}): no effect for {missing:?}");
            for (i, s) in SURFACES.iter().enumerate() {
                if let Some(Some(n)) = e.nonflesh.get(i) {
                    print!("{s}={n} ");
                }
            }
            println!();
        }
    }
    for kind in ["bullet_small", "bullet_large", "bullet_ap", "bulletspray_small"] {
        let missing: Vec<&str> = SURFACES.iter().filter(|s| !sounds.contains(&format!("{kind}_{s}"))).copied().collect();
        println!("sound {kind}: default {}; missing {missing:?}", sounds.contains(&format!("{kind}_default")));
    }
    Ok(())
}
