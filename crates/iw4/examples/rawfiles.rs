//! A converted MW2 map's raw files and their sizes: `rawfiles <map>`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let z = iw4::load_map(&iw4::Install::locate()?, &a[0])?;
    for r in z.assets.iter().filter_map(|x| match x { iw3::zone::Asset::RawFile(r) => Some(r), _ => None }) {
        println!("{} {} bytes: {:?}", r.name, r.data.len(), String::from_utf8_lossy(&r.data[..r.data.len().min(if std::env::var_os("FULL").is_some() { 4000 } else { 60 })]));
    }
    Ok(())
}
