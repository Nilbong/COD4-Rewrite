//! Which xmodels use a material, or have a bone: matusers <part> <zone>...
//! (`part` matched in material and bone names, case-insensitively).
use anyhow::Result;
use iw3::zone::{Asset, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let part = args.next().unwrap_or("laser".into()).to_ascii_lowercase();
    let install = iw3::Install::locate()?;
    for zone_name in args {
        let Ok(data) = iw3::fastfile::load(&install.zone_path(&zone_name)) else {
            println!("{zone_name}: can't read");
            continue;
        };
        let zone = Zone::parse(&data, ParseOptions::default())?;
        for (id, a) in zone.assets.iter().enumerate() {
            let Asset::XModel(_) = a else { continue };
            let Some(x) = zone.xmodel(id) else { continue };
            let bones: Vec<&String> = x.bone_names.iter().filter(|b| b.to_ascii_lowercase().contains(&part)).collect();
            let mats: Vec<String> = (0..x.surfs.len())
                .filter_map(|i| x.materials.get(i).copied().flatten().and_then(|m| zone.material(m)).map(|m| m.name.clone()))
                .filter(|n| n.to_ascii_lowercase().contains(&part))
                .collect();
            if !bones.is_empty() || !mats.is_empty() || x.name.to_ascii_lowercase().contains(&part) {
                println!("{zone_name}: {} bones {:?} mats {:?}", x.name, bones, mats);
            }
        }
    }
    Ok(())
}
