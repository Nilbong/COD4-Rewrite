//! Print localized strings' values: locval <zone> <filter>
use anyhow::Result;
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("coup".into());
    let filter = a.next().unwrap_or_default();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    for asset in &zone.assets {
        if let Asset::Generic(g) = asset
            && g.ty == AssetType::LocalizeEntry
            && g.name.contains(&filter)
        {
            println!("{} = {:?}", g.name, g.root.string("value"));
        }
    }
    Ok(())
}
