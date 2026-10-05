//! Print one bone's track over every frame: animtrack <anim> <bone>
use anyhow::Result;
use iw3::xanim::XAnim;
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let anim = args.next().unwrap_or("viewmodel_ak47_ads_up".into());
    let bone = args.next().unwrap_or("tag_ads".into());
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path("common_mp"))?, ParseOptions::default())?;
    let g = zone
        .assets
        .iter()
        .find_map(|x| match x {
            Asset::Generic(g) if g.ty == AssetType::XAnimParts && g.name == anim => Some(g),
            _ => None,
        })
        .expect("anim");
    let a = XAnim::from_node(&anim, &g.root, &zone.script_strings)?;
    let b = a.bones.iter().find(|b| b.name == bone).expect("bone");
    for f in 0..=a.num_frames {
        println!("{f:3} rot {:?} trans {:?}", b.rot.sample(f as f32), b.trans.sample(f as f32));
    }
    Ok(())
}
