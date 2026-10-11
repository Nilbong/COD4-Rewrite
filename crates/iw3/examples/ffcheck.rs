//! Whether fastfiles at any path (custom maps in `usermaps/`) parse, with
//! a world and collision, and what they hold: ffcheck <path.ff>...
use iw3::zone::{Asset, ParseOptions, Zone};
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    for path in std::env::args().skip(1) {
        let t0 = std::time::Instant::now();
        let result = iw3::fastfile::load(std::path::Path::new(&path)).and_then(|d| Zone::parse(&d, ParseOptions::default()));
        match result {
            Ok(z) => {
                let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
                for a in &z.assets {
                    let name = format!("{:?}", a);
                    let kind = name.split(['(', ' ', '{']).next().unwrap_or("?").to_owned();
                    *kinds.entry(kind).or_default() += 1;
                }
                let ents = z.assets.iter().find_map(|a| match a {
                    Asset::MapEnts(m) => Some(m.entity_string.clone()),
                    _ => None,
                });
                let count = |c: &str| ents.as_deref().map_or(0, |e| e.matches(&format!("\"classname\" \"{c}\"")).count());
                println!(
                    "  spawns: tdm {} dm {} dom {} sd_attack {} sab_allies {}",
                    count("mp_tdm_spawn"),
                    count("mp_dm_spawn"),
                    count("mp_dom_spawn"),
                    count("mp_sd_spawn_attacker"),
                    count("mp_sab_spawn_allies")
                );
                println!(
                    "{path}: ok world {} clip {} com {} ({:.1?}) {kinds:?}",
                    z.gfx_world().is_some(),
                    z.clip_map().is_some(),
                    z.com_world().is_some(),
                    t0.elapsed()
                );
            }
            Err(e) => println!("{path}: FAILED {e:#}"),
        }
    }
    let _ = Asset::RawFile;
    Ok(())
}
