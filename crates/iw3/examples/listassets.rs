//! List asset names of a type in a zone: listassets <zone> <type> [filter]
use anyhow::Result;
use iw3::zone::{Asset, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("common_mp".into());
    let ty = a.next().unwrap_or("XAnimParts".into());
    let filter = a.next().unwrap_or_default();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    for asset in &zone.assets {
        let (t, n) = match asset {
            Asset::Generic(g) => (format!("{:?}", g.ty), g.name.clone()),
            Asset::XModel(x) => ("XModel".into(), x.name.clone()),
            Asset::Material(m) => ("Material".into(), m.name.clone()),
            other => (String::from("other"), other.name().to_owned()),
        };
        if t == ty && n.contains(&filter) { println!("{n}"); }
    }
    Ok(())
}
