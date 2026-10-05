//! `cargo run --release -p t4 --example rawfiles -- <zone> [filter] [--print]`:
//! list a zone's raw files (scripts, configs), optionally printing them.
use t4::zone::{AssetType, ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let zone_name = args.first().cloned().unwrap_or_else(|| "common_mp".into());
    let filter = args.get(1).filter(|a| !a.starts_with("--")).cloned().unwrap_or_default();
    let print = args.iter().any(|a| a == "--print");
    let install = t4::Install::locate()?;
    let zone = Zone::parse(&t4::fastfile::load(&install.zone_path(&zone_name))?, ParseOptions::default())?;
    for (_, a) in zone.of_type(AssetType::RawFile) {
        if !a.name.contains(&filter) {
            continue;
        }
        let text = String::from_utf8_lossy(a.root.bytes("buffer"));
        println!("{} ({} bytes)", a.name, text.len());
        if print {
            println!("{text}\n");
        }
    }
    Ok(())
}
