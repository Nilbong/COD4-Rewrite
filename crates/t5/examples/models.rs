//! `cargo run --release -p t5 --example models -- common_mp [filter]`: list a zone's models.
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or_else(|| "common_mp".into());
    let filter = args.next().unwrap_or_default().to_ascii_lowercase();
    let install = t5::Install::locate()?;
    let zone = t5::zone::Zone::parse(&t5::fastfile::load(&install.zone_path(&zone_name))?, Default::default())?;
    println!("{zone_name}: stopped at {:?}", zone.stats.stopped_at);
    for (_, a) in zone.of_type(t5::zone::AssetType::XModel) {
        if a.name.to_ascii_lowercase().contains(&filter) {
            println!("{} bones {} surfs {}", a.name, a.root.int("numBones"), a.root.int("numsurfs"));
        }
    }
    Ok(())
}
