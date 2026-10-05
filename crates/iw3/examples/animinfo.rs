//! Dump an xanim's header: animinfo <zone> <anim>
use anyhow::Result;
use iw3::zone::{Asset, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or("common_mp".into());
    let anim = args.next().unwrap_or("viewmodel_ak47_idle".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    let a = zone.assets.iter().find_map(|a| match a { Asset::Generic(g) if g.name == anim => Some(g), _ => None }).expect("anim");
    let r = &a.root;
    let bc: Vec<i64> = (0..10).map(|i| r.int(&format!("boneCount[{i}]"))).collect();
    println!("{} frames {} fps {} loop {} delta {} boneCount {:?} indexCount {} dataByte {} dataShort {} dataInt {} rShort {} rByte {} rInt {}",
        a.name, r.int("numframes"), r.float("framerate"), r.int("bLoop"), r.int("bDelta"), bc, r.int("indexCount"),
        r.bytes("dataByte").len(), r.bytes("dataShort").len()/2, r.bytes("dataInt").len()/4, r.bytes("randomDataShort").len()/2, r.bytes("randomDataByte").len(), r.bytes("randomDataInt").len()/4);
    let names: Vec<String> = r.bytes("names").chunks(2).map(|c| zone.script_strings.get(u16::from_le_bytes([c[0], c[1]]) as usize).cloned().unwrap_or_default()).collect();
    println!("bones: {names:?}");
    println!("fields: {:?}", r.fields.iter().map(|(n, _)| n).collect::<Vec<_>>());
    Ok(())
}
