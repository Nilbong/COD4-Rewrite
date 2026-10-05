//! `cargo run --release -p t5 --example anims [filter]`: decode every
//! animation in `common_mp` with `iw3::xanim` and report.
fn main() -> anyhow::Result<()> {
    let filter = std::env::args().nth(1).unwrap_or_default();
    let install = t5::Install::locate()?;
    let zone = t5::load_iw3(&install, "common_mp")?;
    let (mut ok, mut failed) = (0, Vec::new());
    for a in &zone.assets {
        let iw3::zone::Asset::Generic(g) = a else { continue };
        if g.ty != iw3::zone::AssetType::XAnimParts || !g.name.contains(&filter) {
            continue;
        }
        match iw3::xanim::XAnim::from_node(&g.name, &g.root, &zone.script_strings) {
            Ok(x) => {
                ok += 1;
                if !filter.is_empty() {
                    println!("{}: {} frames at {} fps, {} bones, {} notifies", x.name, x.num_frames, x.framerate, x.bones.len(), x.notifies.len());
                }
            }
            Err(e) => failed.push(format!("{e:#}")),
        }
    }
    println!("{ok} decoded, {} failed", failed.len());
    for f in failed.iter().take(10) {
        println!("  {f}");
    }
    Ok(())
}
