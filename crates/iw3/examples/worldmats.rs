//! World surface materials with their technique sets and textures (semantic,
//! name hash, image), by triangle count: worldmats <zone>
use iw3::zone::{ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let name = std::env::args().nth(1).unwrap_or("mp_crash".into());
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&name))?, ParseOptions::default())?;
    let world = zone.gfx_world().expect("gfxworld");
    let mut tris = std::collections::BTreeMap::<String, u32>::new();
    for s in &world.surfaces {
        let Some(m) = s.material.and_then(|m| zone.material(m)) else { continue };
        *tris.entry(m.name.clone()).or_default() += s.tri_count as u32;
    }
    let mut list: Vec<_> = tris.into_iter().collect();
    list.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (mat, n) in list {
        let m = zone.assets.iter().find_map(|a| match a { iw3::zone::Asset::Material(m) if m.name == mat => Some(m), _ => None });
        let Some(m) = m else { continue };
        let t = m.technique_set.and_then(|t| zone.technique_set(t)).map_or("", |t| t.name.as_str());
        let tex: Vec<String> = m
            .textures
            .iter()
            .map(|x| format!("{:?}/{:08x}={}", x.semantic, x.name_hash, x.image.and_then(|i| zone.image(i)).map_or("?", |i| i.name.as_str())))
            .collect();
        println!("{n:7} {mat} [{t}] {}", tex.join(" "));
    }
    Ok(())
}
