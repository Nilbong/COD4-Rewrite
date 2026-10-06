//! A map's primary lights: primarylights <zone>
use anyhow::Result;
use iw3::zone::{ParseOptions, Zone};

fn main() -> Result<()> {
    let zone_name = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?, ParseOptions::default())?;
    let Some(world) = zone.com_world() else { return Ok(()) };
    println!("{}: {} primary lights", zone_name, world.primary_lights.len());
    for l in &world.primary_lights {
        println!(
            "  kind {} colour {:.2?} at {:.0?} radius {:.0} dir {:.2?} cos {:.2}/{:.2} {:?}",
            l.kind, l.color, l.origin, l.radius, l.dir, l.cos_half_fov_outer, l.cos_half_fov_inner, l.def_name
        );
    }
    Ok(())
}
