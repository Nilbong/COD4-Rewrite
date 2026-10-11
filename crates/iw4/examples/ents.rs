//! A converted MW2 map's entities by classname (with models): `ents <map>`.
fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or("mp_terminal".into());
    let z = iw4::load_map(&iw4::Install::locate()?, &map)?;
    let ents = iw3::ents::parse(&z.map_ents().unwrap().entity_string);
    let mut counts = std::collections::BTreeMap::new();
    for e in &ents {
        let key = (e.classname().to_owned(), e.get("model").map(|m| if m.starts_with('*') { "*brush".to_owned() } else { "model".to_owned() }).unwrap_or_default(), e.get("targetname").unwrap_or("").to_owned());
        *counts.entry(key).or_insert(0) += 1;
    }
    for ((c, m, t), n) in counts {
        println!("{n:4} {c} {m} {t}");
    }
    Ok(())
}
