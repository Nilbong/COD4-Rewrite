//! Animations that move `j_mainroot` but not `torso_stabilizer` (campaign
//! style): rootanims <zone> [prefix]
use iw3::xanim::XAnim;
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?, ParseOptions::default())?;
    let prefix = a.get(1).cloned().unwrap_or_default();
    let (mut n, mut hit) = (0, 0);
    for asset in &zone.assets {
        let Asset::Generic(g) = asset else { continue };
        if g.ty != AssetType::XAnimParts || !g.name.starts_with(&prefix) {
            continue;
        }
        let Ok(x) = XAnim::from_node(&g.name, &g.root, &zone.script_strings) else { continue };
        let has = |b: &str| x.bones.iter().any(|t| t.name == b);
        n += 1;
        if has("j_mainroot") && !has("torso_stabilizer") {
            hit += 1;
            println!("{}", g.name);
        }
    }
    println!("{hit} of {n}");
    Ok(())
}
