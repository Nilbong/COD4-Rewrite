//! Raw files (scripts, configs) from a zone: rawfile <zone> <name> [dir]
//! prints each whose name contains `name`, or with `dir` writes them under
//! it at their own paths (`dir/maps/mp/_spawnlogic.gsc`).
use iw3::zone::{Asset, ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("common_mp".into());
    let name = a.next().unwrap_or_default().to_ascii_lowercase();
    let dir = a.next().map(std::path::PathBuf::from);
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?, ParseOptions::default())?;
    for asset in &zone.assets {
        let Asset::RawFile(r) = asset else { continue };
        if !r.name.to_ascii_lowercase().contains(&name) {
            continue;
        }
        match &dir {
            Some(dir) => {
                let path = dir.join(&r.name);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, &r.data)?;
                println!("{}", path.display());
            }
            None => println!("// {}\n{}", r.name, String::from_utf8_lossy(&r.data)),
        }
    }
    Ok(())
}
