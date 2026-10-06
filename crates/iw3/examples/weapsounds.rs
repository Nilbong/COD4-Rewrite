//! A weapon's sound fields: weapsounds <weapon>...
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path("common_mp"))?, ParseOptions::default())?;
    for name in std::env::args().skip(1) {
        for a in &zone.assets {
            let Asset::Generic(g) = a else { continue };
            if g.ty != AssetType::Weapon || g.name != name {
                continue;
            }
            let fields: Vec<String> = g
                .root
                .fields
                .iter()
                .map(|(f, _)| f.as_str())
                .filter(|f| f.contains("Sound"))
                .filter_map(|f| {
                    let s = g.root.node(f)?.node("name")?.string("soundName")?;
                    (!s.is_empty()).then(|| format!("{f}={s}"))
                })
                .collect();
            println!("{name}: {}", fields.join(" "));
        }
    }
    Ok(())
}
