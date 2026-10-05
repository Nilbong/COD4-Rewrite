//! A map's entities with any key or value containing a filter: ents <map> <filter>
fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let map = a.next().unwrap_or("mp_killhouse".into());
    let filter = a.next().unwrap_or_default().to_ascii_lowercase();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&map))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    for e in iw3::ents::parse(&zone.map_ents().unwrap().entity_string) {
        if e.fields.iter().any(|(k, v)| k.to_ascii_lowercase().contains(&filter) || v.to_ascii_lowercase().contains(&filter)) {
            let mut kv: Vec<_> = e.fields.iter().collect();
            kv.sort();
            println!("{}", kv.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("  "));
        }
    }
    Ok(())
}
