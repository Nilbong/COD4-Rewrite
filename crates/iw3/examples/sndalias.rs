//! Sound aliases' fields: sndalias <zone> <name filter>
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("common_mp".into());
    let filter = a.next().unwrap_or_default();
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?, ParseOptions::default())?;
    for asset in &zone.assets {
        let Asset::Generic(g) = asset else { continue };
        if g.ty != AssetType::Sound || !g.name.contains(&filter) {
            continue;
        }
        for h in g.root.nodes("head") {
            println!(
                "{}: dist {}..{} vol {}..{} flags {:#x} secondary {:?} slave {}",
                g.name,
                h.float("distMin"),
                h.float("distMax"),
                h.float("volMin"),
                h.float("volMax"),
                h.int("flags"),
                h.string("secondaryAliasName"),
                h.float("slavePercentage"),
            );
        }
    }
    Ok(())
}
