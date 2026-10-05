//! Compare two anims' bone rotations at frame 0: animcmp <a> <b>
use anyhow::Result;
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};
use iw3::xanim::{XAnim, RotTrack};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let a = args.next().unwrap_or("viewmodel_ak47_idle".into());
    let b = args.next().unwrap_or("viewmodel_ak47_fire".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path("common_mp"))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    let get = |n: &str| zone.assets.iter().find_map(|x| match x { Asset::Generic(g) if g.ty == AssetType::XAnimParts && g.name == n => Some(XAnim::from_node(n, &g.root, &zone.script_strings).unwrap()), _ => None }).unwrap();
    let (xa, xb) = (get(&a), get(&b));
    for bone in &xa.bones {
        let Some(other) = xb.bones.iter().find(|o| o.name == bone.name) else { continue };
        let kind = |r: &RotTrack| match r { RotTrack::None => "none".to_string(), RotTrack::Keys { frames, values } => format!("{}keys/{}", values.len(), frames.len()) };
        let f: f32 = std::env::var("FRAME").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let (qa, qb) = (bone.rot.sample(0.0), other.rot.sample(f));
        let dot = match (qa, qb) { (Some(x), Some(y)) => (x[0]*y[0]+x[1]*y[1]+x[2]*y[2]+x[3]*y[3]).abs(), _ => -1.0 };
        let (ta, tb) = (bone.trans.sample(0.0), other.trans.sample(f));
        let tdiff = match (ta, tb) { (Some(x), Some(y)) => ((x[0]-y[0]).powi(2)+(x[1]-y[1]).powi(2)+(x[2]-y[2]).powi(2)).sqrt(), (None, None) => 0.0, _ => 999.0 };
        if tdiff > 0.5 && dot != -1.0 || tdiff > 0.5 {
            println!("TRANS {:<22} {:?} vs {:?} (diff {:.1})", bone.name, ta, tb, tdiff);
        }
        if dot < 0.99 && dot != -1.0 {
            println!("{:<22} {} vs {}  dot {:.3}  {:?} {:?}  trans {:?} {:?}", bone.name, kind(&bone.rot), kind(&other.rot), dot, qa, qb, bone.trans.sample(0.0), other.trans.sample(0.0));
        }
    }
    Ok(())
}
