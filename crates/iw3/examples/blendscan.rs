//! Tally the blend states (src, dst, op) of maps' materials, with a few
//! example names and technique sets each: blendscan <zone>...
use std::collections::BTreeMap;

use anyhow::Result;
use iw3::zone::{self, Asset, ParseOptions, Zone};

fn main() -> Result<()> {
    let install = iw3::Install::locate()?;
    let mut seen: BTreeMap<(u32, u32, u32), (usize, Vec<String>)> = BTreeMap::new();
    for name in std::env::args().skip(1) {
        let Ok(data) = iw3::fastfile::load(&install.zone_path(&name)) else {
            println!("{name}: not found");
            continue;
        };
        let zone = Zone::parse(&data, ParseOptions::default())?;
        for a in &zone.assets {
            let Asset::Material(m) = a else { continue };
            let bits = [zone::TECHNIQUE_LIT, 8, zone::TECHNIQUE_UNLIT, zone::TECHNIQUE_EMISSIVE]
                .into_iter()
                .find_map(|t| m.state_bits_for(t))
                .or_else(|| m.state_bits.first().copied());
            let Some([b0, _]) = bits else { continue };
            let key = (b0 & 0xf, (b0 >> 4) & 0xf, (b0 >> 8) & 7);
            let ts = m.technique_set.and_then(|t| zone.technique_set(t)).map(|t| t.name.as_str()).unwrap_or("-");
            if std::env::var("TECHSET").is_ok_and(|f| ts.contains(&f)) {
                println!("{name}: {} ({ts}) {key:?} consts {:?}", m.name, m.constants.iter().map(|c| (&c.name, c.literal)).collect::<Vec<_>>());
            }
            let e = seen.entry(key).or_default();
            e.0 += 1;
            let label = format!("{name}:{} ({ts})", m.name);
            if e.1.len() < 6 && !e.1.iter().any(|l| l.ends_with(&format!("({ts})"))) {
                e.1.push(label);
            }
        }
    }
    for ((src, dst, op), (n, examples)) in &seen {
        println!("src {src} dst {dst} op {op}: {n}");
        for e in examples {
            println!("    {e}");
        }
    }
    Ok(())
}
