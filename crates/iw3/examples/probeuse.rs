//! How many world surfaces use each reflection probe: probeuse <map>
fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&map))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let world = zone.gfx_world().expect("gfx world");
    let mut counts = std::collections::BTreeMap::<u8, usize>::new();
    for s in &world.surfaces {
        *counts.entry(s.reflection_probe_index).or_default() += 1;
    }
    println!("{map}: {} probes; surfaces per probe {counts:?}", world.reflection_probes.len());
    Ok(())
}
