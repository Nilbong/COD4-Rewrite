//! Assets whose name contains text: `assetnames <zone> <text>...`; with
//! `DUMP=<raw file>` that raw file's text instead.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let install = iw4::Install::locate()?;
    let z = iw4::zone::Zone::parse(&iw4::fastfile::load(&install.zone_path(&a[0]))?, Default::default())?;
    if let Ok(name) = std::env::var("DUMP") {
        let c = iw4::convert::to_iw3(&z, None);
        for r in c.assets.iter().filter_map(|x| match x { iw3::zone::Asset::RawFile(r) if r.name == name => Some(r), _ => None }) {
            println!("{}", String::from_utf8_lossy(&r.data));
        }
        return Ok(());
    }
    for m in z.assets.iter().filter(|m| a[1..].iter().any(|t| m.name.contains(t.as_str()))) {
        println!("{:?} {:?}", m.ty, m.name);
    }
    Ok(())
}
