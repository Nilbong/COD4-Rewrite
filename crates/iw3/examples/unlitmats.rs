//! World materials drawn by an unlit technique, per map: unlitmats <zone>...
use iw3::zone::{ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let install = iw3::Install::locate()?;
    for name in std::env::args().skip(1) {
        let Ok(data) = iw3::fastfile::load(&install.zone_path(&name)) else { continue };
        let zone = Zone::parse(&data, ParseOptions::default())?;
        let Some(w) = zone.gfx_world() else { continue };
        let mut found = std::collections::BTreeMap::<String, usize>::new();
        for s in &w.surfaces {
            let Some(m) = s.material.and_then(|m| zone.material(m)) else { continue };
            let t = m.technique_set.and_then(|t| zone.technique_set(t)).map_or("", |t| t.name.as_str());
            if t.contains("unlit") {
                *found.entry(format!("{} [{t}]", m.name)).or_default() += 1;
            }
        }
        println!("{name}: {:?}", found);
    }
    Ok(())
}
