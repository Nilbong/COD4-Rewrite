//! Maps' sun settings: suninfo <zone>...
fn main() -> anyhow::Result<()> {
    for name in std::env::args().skip(1) {
        let zone = iw3::zone::Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&name))?, iw3::zone::ParseOptions::default())?;
        let Some(w) = zone.gfx_world() else { continue };
        let s = &w.sun;
        println!("{name:16} sun_light {:.2} colour {:.2?} ambient {:.2?} scale {:.2} diffuse_fraction {:.2} live {:.2} angles {:.0?}", s.sun_light, s.sun_color, s.ambient_color, s.ambient_scale, s.diffuse_fraction, (s.sun_light - s.ambient_scale) * (1.0 - s.diffuse_fraction), s.angles);
    }
    Ok(())
}
