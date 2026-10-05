//! Integer fields of weapons: weaponfields <field>... -- <weapon>...
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (fields, weapons) = (&args[..split], &args[(split + 1).min(args.len())..]);
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path("common_mp"))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    for a in &zone.assets {
        if let iw3::zone::Asset::Generic(g) = a {
            if g.ty == iw3::zone::AssetType::Weapon && weapons.iter().any(|w| *w == g.name) {
                let vals: Vec<String> = fields.iter().map(|f| format!("{f}={}", g.root.int(f))).collect();
                println!("{}: {}", g.name, vals.join(" "));
            }
        }
    }
    Ok(())
}
