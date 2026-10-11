//! Entities whose model starts with a prefix: `entdump <map> <prefix>`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let z = iw4::load_map(&iw4::Install::locate()?, &a[0])?;
    let s = &z.map_ents().unwrap().entity_string;
    for e in iw3::ents::parse(s).iter().filter(|e| e.get("model").is_some_and(|m| m.starts_with(a[1].as_str()))).take(6) {
        println!("{:?}", e);
    }
    Ok(())
}
