//! A Modern Warfare 2 weapon's models, animations, tags and values from
//! `common_mp`: `weapinfo <weapon> [key filter]`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let install = iw4::Install::locate()?;
    let z = iw4::zone::Zone::parse(&iw4::fastfile::load(&install.zone_path("common_mp"))?, Default::default())?;
    let all = iw4::weapons::weapons(&z);
    println!("{} weapons", all.len());
    let w = all.iter().find(|w| w.name == a[0]).ok_or_else(|| anyhow::anyhow!("no weapon {}", a[0]))?;
    println!("gun: {:?}\nworld: {:?}\nhands: {}", w.gun_models, w.world_models, w.hand_model);
    println!("anims: {:?}\nhideTags: {:?}\nnotes: {:?}", w.anims, w.hide_tags, w.notetrack_sounds);
    let mut keys: Vec<_> = w.values.iter().filter(|(k, _)| a.get(1).is_none_or(|f| k.to_lowercase().contains(&f.to_lowercase()))).collect();
    keys.sort();
    for (k, v) in keys {
        println!("  {k} = {v}");
    }
    Ok(())
}
