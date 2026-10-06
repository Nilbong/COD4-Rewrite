//! A map's static models whose name contains a filter: smodels <map> <filter>
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let filter = a.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    for sm in &zone.gfx_world().expect("world").static_models {
        let name = sm.model.and_then(|m| zone.xmodel(m)).map_or("", |x| x.name.as_str());
        if name.to_ascii_lowercase().contains(&filter) {
            println!("{name} at [{:.0}, {:.0}, {:.0}]", sm.origin[0], sm.origin[1], sm.origin[2]);
        }
    }
    Ok(())
}
