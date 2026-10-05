//! Dump a material's textures: matinfo <zone> <material substring>
use anyhow::Result;
use iw3::zone::{Asset, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or("mp_killhouse".into());
    let pat = args.next().unwrap_or("hands".into());
    let install = iw3::Install::locate()?;
    let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(&zone_name))?, ParseOptions::default())?;
    for a in &zone.assets {
        let Asset::Material(m) = a else { continue };
        if !m.name.contains(&pat) {
            continue;
        }
        let ts = m.technique_set.and_then(|t| zone.technique_set(t)).map(|t| t.name.as_str()).unwrap_or("-");
        println!("{} ({ts})", m.name);
        for t in &m.textures {
            let img = t.image.and_then(|i| zone.image(i));
            println!("  {:?} hash {:08x} -> {:?} embedded {:?}", t.semantic, t.name_hash, img.map(|i| &i.name), img.and_then(|i| i.load_def.as_ref()).map(|d| (d.data.len(), d.format, d.level_count, d.dimensions)));
        }
        for c in &m.constants {
            println!("  const {} = {:?}", c.name, c.literal);
        }
    }
    Ok(())
}
