//! Dump a technique set's shaders and arguments: shaderdump <zone> <techset> <out dir>
use anyhow::Result;
use iw3::zone::{Asset, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or("mp_killhouse".into());
    let wanted = args.next().unwrap_or("wc_l_sm_r0c0n0s0".into());
    let out = std::path::PathBuf::from(args.next().expect("output dir"));
    std::fs::create_dir_all(&out)?;
    let install = iw3::Install::locate()?;
    let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(&zone_name))?, ParseOptions::default())?;
    println!("block check: {:?} unresolved {}", zone.stats.block_positions.len(), zone.stats.unresolved_aliases);
    for a in &zone.assets {
        let Asset::TechniqueSet(ts) = a else { continue };
        if ts.name != wanted {
            continue;
        }
        for (i, t) in ts.techniques.iter().enumerate() {
            let Some(t) = t else { continue };
            for (pi, p) in t.passes.iter().enumerate() {
                let vs = p.vertex_shader.as_ref().map(|s| s.name.as_str()).unwrap_or("(ref)");
                let ps = p.pixel_shader.as_ref().map(|s| s.name.as_str()).unwrap_or("(ref)");
                println!("tech {i:2} {} pass {pi}: vs {vs} ps {ps} args {:?}", t.name, p.args);
                for (kind, sh) in [("vs", &p.vertex_shader), ("ps", &p.pixel_shader)] {
                    if let Some(sh) = sh {
                        std::fs::write(out.join(format!("t{i}_p{pi}_{kind}_{}.bin", sh.name)), &sh.program)?;
                    }
                }
            }
        }
    }
    Ok(())
}
