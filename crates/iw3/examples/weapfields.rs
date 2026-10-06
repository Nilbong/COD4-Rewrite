//! Named numeric fields of weapons: weapfields <field>[,<field>...] <weapon>...
//! (paths as the schema names them, `locationDamageMultipliers[3]` for an
//! element; `name[a..b]` for a run of them).
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let fields: Vec<String> = a
        .next()
        .unwrap_or_default()
        .split(',')
        .flat_map(|f| match f.split_once('[').and_then(|(n, r)| Some((n, r.strip_suffix(']')?.split_once("..")?))) {
            Some((n, (lo, hi))) => {
                let (lo, hi): (usize, usize) = (lo.parse().unwrap_or(0), hi.parse().unwrap_or(0));
                (lo..hi).map(|i| format!("{n}[{i}]")).collect::<Vec<_>>()
            }
            None => vec![f.to_owned()],
        })
        .collect();
    let weapons: Vec<String> = a.collect();
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path("common_mp"))?, ParseOptions::default())?;
    for asset in &zone.assets {
        let Asset::Generic(g) = asset else { continue };
        if g.ty != AssetType::Weapon || !weapons.iter().any(|w| *w == g.name) {
            continue;
        }
        let values: Vec<String> = fields.iter().map(|f| format!("{f}={}", g.root.float(f))).collect();
        println!("{}: {}", g.name, values.join(" "));
    }
    Ok(())
}
