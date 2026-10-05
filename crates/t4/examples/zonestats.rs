//! `cargo run --release -p t4 --example zonestats -- common_mp`: parse a
//! World at War zone and summarise what loaded.

use std::collections::BTreeMap;
use t4::zone::{ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let name = std::env::args().nth(1).unwrap_or_else(|| "common_mp".into());
    let install = t4::Install::locate()?;
    let t0 = std::time::Instant::now();
    let data = t4::fastfile::load(&install.zone_path(&name))?;
    let t1 = t0.elapsed();
    let zone = Zone::parse(&data, ParseOptions::default())?;
    println!("{name}: {} bytes, inflated in {t1:.2?}, parsed in {:.2?}", data.len(), t0.elapsed() - t1);
    let s = &zone.stats;
    println!(
        "top level {}/{}; unresolved aliases {} strings {}; stopped at {:?}",
        s.top_level_parsed, s.top_level_total, s.unresolved_aliases, s.unresolved_strings, s.stopped_at
    );
    println!("blocks parsed {:?}\nblocks header {:?}", s.block_positions, zone.block_sizes);
    let mut counts = BTreeMap::new();
    for a in &zone.assets {
        *counts.entry(format!("{:?}", a.ty)).or_insert(0) += 1;
    }
    println!("{counts:?}");
    Ok(())
}
