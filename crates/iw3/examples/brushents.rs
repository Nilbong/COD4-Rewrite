//! The map's brush entities (`model` "*N"): brushents <map>
fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&map))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let ents = iw3::ents::parse(&zone.map_ents().unwrap().entity_string);
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for e in &ents {
        if e.get("model").is_some_and(|m| m.starts_with('*')) {
            *counts.entry(format!("{} {}", e.classname(), e.get("targetname").unwrap_or(""))).or_default() += 1;
        }
    }
    for (k, v) in counts {
        println!("{v:3} {k}");
    }
    Ok(())
}
