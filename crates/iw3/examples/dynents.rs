//! A map's dynamic entities (clutter and breakables) with their physics
//! presets: dynents <zone> [model filter]
use iw3::zone::Asset;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or("mp_crossfire".into());
    let filter = args.next().unwrap_or_default();
    let zone = iw3::zone::Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&name))?, iw3::zone::ParseOptions::default())?;
    let Some(clip) = zone.clip_map() else { return Ok(()) };
    let named = |id: Option<iw3::zone::AssetId>| id.map_or(String::new(), |i| zone.get(i).name().to_owned());
    let mut presets = std::collections::BTreeMap::new();
    for d in &clip.dyn_ents {
        let model = named(d.model);
        if !model.contains(&filter) {
            continue;
        }
        println!(
            "kind {} at [{:.0}, {:.0}, {:.0}] {model} health {} preset {} pieces {} fx {}",
            d.kind,
            d.origin[0],
            d.origin[1],
            d.origin[2],
            d.health,
            named(d.phys_preset),
            named(d.destroy_pieces),
            named(d.destroy_fx)
        );
        if let Some(Asset::PhysPreset(p)) = d.phys_preset.map(|i| zone.get(i)) {
            presets.insert(p.name.clone(), format!("{p:?}"));
        }
    }
    for p in presets.values() {
        println!("{p}");
    }
    Ok(())
}
