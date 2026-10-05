//! Which multiplayer maps parse with a world and collision: mapcheck <map>...
fn main() -> anyhow::Result<()> {
    let install = iw3::Install::locate()?;
    for map in std::env::args().skip(1) {
        let t0 = std::time::Instant::now();
        let result = iw3::fastfile::load(&install.zone_path(&map))
            .and_then(|d| iw3::zone::Zone::parse(&d, iw3::zone::ParseOptions::default()));
        match result {
            Ok(z) => println!(
                "{map}: ok world {} clip {} ({:.1?})",
                z.gfx_world().is_some(),
                z.clip_map().is_some(),
                t0.elapsed()
            ),
            Err(e) => println!("{map}: FAILED {e:#}"),
        }
    }
    Ok(())
}
