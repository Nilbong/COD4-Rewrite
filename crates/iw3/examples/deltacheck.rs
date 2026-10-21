//! Root motion of animations: deltacheck <zone> <anim>...
use anyhow::Result;
use iw3::zone::{Asset, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or("common".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    for anim in args {
        let Some(g) = zone.assets.iter().find_map(|a| match a { Asset::Generic(g) if g.name == anim => Some(g), _ => None }) else { println!("{anim}: none"); continue };
        let x = iw3::xanim::XAnim::from_node(&anim, &g.root, &zone.script_strings)?;
        let d = x.delta_at(1.0);
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        println!("{anim}: duration {:.2}s delta {d:?} -> {:.0} u/s", x.duration(), len / x.duration().max(0.001));
    }
    Ok(())
}
