//! Every multiplayer weapon's move speed scales, grouped: weaponspeeds
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path("common_mp"))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    let mut seen = std::collections::BTreeMap::new();
    for a in &zone.assets {
        if let Asset::Generic(g) = a {
            if g.ty == AssetType::Weapon {
                let w = &g.root;
                let key = (format!("{:.3}", w.float("moveSpeedScale")), format!("{:.3}", w.float("adsMoveSpeedScale")), w.int("weapClass"));
                seen.entry(key).or_insert_with(Vec::new).push(g.name.clone());
            }
        }
    }
    for ((m, a, c), names) in seen {
        println!("move {m} ads {a} class {c}: {}", names.iter().take(14).cloned().collect::<Vec<_>>().join(" "));
    }
    Ok(())
}
