//! Print a zone's raw files (scripts, configs) whose names contain a filter:
//! rawfile <zone> <filter> [out dir]
fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("common_mp".into());
    let filter = a.next().unwrap_or_default();
    let out = a.next();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    for asset in &zone.assets {
        if let iw3::zone::Asset::RawFile(r) = asset {
            if r.name.contains(&filter) {
                println!("{} ({} bytes)", r.name, r.data.len());
                if let Some(dir) = &out {
                    let path = std::path::Path::new(dir).join(r.name.replace(['/', '\\'], "_"));
                    std::fs::write(path, &r.data)?;
                }
            }
        }
    }
    Ok(())
}
