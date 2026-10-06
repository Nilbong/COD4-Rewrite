//! Temporary: per map, clip static models and brush contents of interest.
fn main() -> anyhow::Result<()> {
    let install = iw3::Install::locate()?;
    for map in std::env::args().skip(1) {
        let Ok(data) = iw3::fastfile::load(&install.zone_path(&map)) else { continue };
        let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
        let Some(c) = zone.clip_map() else { continue };
        let names: std::collections::BTreeMap<String, usize> = c.static_models.iter().fold(Default::default(), |mut m, s| {
            let n = s.model.and_then(|id| zone.xmodel(id)).map_or("?".to_string(), |x| x.name.clone());
            *m.entry(n).or_default() += 1;
            m
        });
        let count = |bit: i32| c.brushes.iter().filter(|b| b.contents & bit != 0 && b.contents & 1 == 0).count();
        println!("{map}: {} clip static models; non-solid brushes with foliage {}, ai_nosight {}, clipshot {}", c.static_models.len(), count(0x2), count(0x1000), count(0x2000));
        let mut top: Vec<_> = names.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        println!("   e.g. {:?}", &top[..top.len().min(8)]);
    }
    Ok(())
}
