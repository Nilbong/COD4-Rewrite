//! Parse every script in the campaign's zones: parseall [zone ...]
//! (default: common and all the levels). Prints failures and a summary.
const LEVELS: [&str; 22] = [
    "common", "killhouse", "cargoship", "coup", "blackout", "armada", "bog_a", "hunted", "ac130", "bog_b", "airlift", "aftermath", "village_assault",
    "scoutsniper", "sniperescape", "village_defend", "ambush", "icbm", "launchfacility_a", "launchfacility_b", "jeepride", "airplane",
];

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let zones: Vec<&str> = if args.is_empty() { LEVELS.to_vec() } else { args.iter().map(String::as_str).collect() };
    let install = iw3::Install::locate()?;
    let (mut ok, mut bad, mut lines, mut funcs) = (0, 0, 0, 0);
    let t0 = std::time::Instant::now();
    for z in zones {
        let zone = iw3::zone::Zone::parse(&iw3::fastfile::load(&install.zone_path(z))?, iw3::zone::ParseOptions::default())?;
        for a in &zone.assets {
            let iw3::zone::Asset::RawFile(r) = a else { continue };
            if !r.name.ends_with(".gsc") {
                continue;
            }
            let src = String::from_utf8_lossy(&r.data);
            lines += src.lines().count();
            match gsc::parse(&src) {
                Ok(s) => {
                    ok += 1;
                    funcs += s.functions.len();
                }
                Err(e) => {
                    bad += 1;
                    println!("{z}: {}: {e}", r.name);
                }
            }
        }
    }
    println!("{ok} scripts parsed, {bad} failed; {funcs} functions, {lines} lines in {:.2?}", t0.elapsed());
    Ok(())
}
