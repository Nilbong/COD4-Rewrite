//! A converted MW2 map's worldspawn keys: `worldspawn <map>`.
fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or("mp_terminal".into());
    let z = iw4::load_map(&iw4::Install::locate()?, &map)?;
    let ents = iw3::ents::parse(&z.map_ents().unwrap().entity_string);
    println!("{:?}", ents.iter().find(|e| e.classname() == "worldspawn"));
    for l in z.com_world().unwrap().primary_lights.iter().take(3) { println!("{l:?}"); }
    Ok(())
}
