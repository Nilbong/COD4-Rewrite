//! List the materials effects use with their technique sets and blend state: fxmats <zone>
use anyhow::Result;
use iw3::fx::{FxEffectDef, Visual};
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};
use std::collections::BTreeMap;

fn main() -> Result<()> {
    let zone_name = std::env::args().nth(1).unwrap_or("common_mp".into());
    let zone =
        Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?, ParseOptions::default())?;
    let mut used: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for a in &zone.assets {
        let Asset::Generic(g) = a else { continue };
        if g.ty != AssetType::Fx {
            continue;
        }
        let def = FxEffectDef::from_node(&zone, &g.root);
        for e in &def.elems {
            for v in &e.visuals {
                if let Visual::Material(m) = v {
                    used.entry(m.clone()).or_default().push(format!("{:?}", e.elem_type));
                }
            }
        }
    }
    for (name, uses) in &used {
        let Some(m) = zone.assets.iter().find_map(|a| match a {
            Asset::Material(m) if &m.name == name => Some(m),
            _ => None,
        }) else {
            println!("{name}: not found");
            continue;
        };
        let ts = m.technique_set.and_then(|t| zone.technique_set(t)).map_or("-", |t| t.name.as_str());
        let slots: Vec<usize> = (0..34).filter(|&t| m.state_bits_for(t).is_some()).collect();
        let bits = m.state_bits_for(5).or_else(|| m.state_bits.first().copied()).unwrap_or([0, 0]);
        let b0 = bits[0];
        println!(
            "{name} ({ts}) slots {slots:?} src {} dst {} op {} srcA {} dstA {} opA {} atest {:#x} {} uses",
            b0 & 0xf,
            (b0 >> 4) & 0xf,
            (b0 >> 8) & 7,
            (b0 >> 12) & 0xf,
            (b0 >> 16) & 0xf,
            (b0 >> 20) & 7,
            b0 & 0x3000,
            uses.len()
        );
    }
    Ok(())
}
