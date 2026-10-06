//! Temporary: print a rawfile (script) from the zones by name.
fn main() -> anyhow::Result<()> {
    let name = std::env::args().nth(1).unwrap();
    let install = iw3::Install::locate()?;
    for z in ["common_mp"] {
        let Ok(data) = iw3::fastfile::load(&install.zone_path(z)) else { continue };
        let Ok(zone) = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default()) else { continue };
        for asset in &zone.assets {
            let iw3::zone::Asset::RawFile(r) = asset else { continue };
            if r.name.contains(&name) {
                println!("== {}\n{}", r.name, String::from_utf8_lossy(&r.data));
            }
        }
    }
    Ok(())
}
