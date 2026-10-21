//! Spawn points with a roof over them (a solid brush within 200 units
//! above): roofed <map>.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let clip = zone.clip_map().expect("clip map");
    let ents = iw3::ents::parse(&zone.map_ents().expect("ents").entity_string);
    for e in ents.iter().filter(|e| e.get("classname").is_some_and(|c| c.starts_with("mp_tdm_spawn") || c.starts_with("mp_dm_spawn"))) {
        let Some(o) = e.get("origin") else { continue };
        let p: Vec<f32> = o.split_whitespace().filter_map(|v| v.parse().ok()).collect();
        let roof = clip.brushes.iter().filter(|b| b.contents & 1 != 0 && p[0] >= b.mins[0] && p[0] <= b.maxs[0] && p[1] >= b.mins[1] && p[1] <= b.maxs[1] && b.mins[2] > p[2] + 60.0 && b.mins[2] < p[2] + 200.0).count();
        if roof > 0 {
            println!("{} {:?} angles {:?} roof brushes {roof}", e.get("classname").unwrap(), p, e.get("angles"));
        }
    }
    Ok(())
}
