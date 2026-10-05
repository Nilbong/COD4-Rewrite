//! Summarise envMapParms material constants: envparms <zone>
use anyhow::Result;
use iw3::zone::{Asset, ParseOptions, Zone};
use std::collections::BTreeMap;

fn main() -> Result<()> {
    let zone_name = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let install = iw3::Install::locate()?;
    let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(&zone_name))?, ParseOptions::default())?;
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for a in &zone.assets {
        let Asset::Material(m) = a else { continue };
        for c in &m.constants {
            if c.name.starts_with("envMapParms") {
                *seen.entry(format!("{:?}", c.literal)).or_default() += 1;
            }
        }
    }
    for (k, v) in &seen {
        println!("{v:4} {k}");
    }
    Ok(())
}
