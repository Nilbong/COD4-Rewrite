//! A converted MW2 map's TDM spawns (CoD origin, yaw): `spawns <map>`.
fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or("mp_highrise".into());
    let z = iw4::load_map(&iw4::Install::locate()?, &map)?;
    let ents = iw3::ents::parse(&z.map_ents().unwrap().entity_string);
    for e in ents.iter().filter(|e| e.classname() == "mp_tdm_spawn").take(12) {
        println!("{:?} {:?}", e.origin(), e.get("angles"));
    }
    Ok(())
}
