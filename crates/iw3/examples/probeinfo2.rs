//! Reflection probes' average colour (rgb * alpha, top mip): probeinfo2 <map>
fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&map))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let world = zone.gfx_world().expect("gfx world");
    for (i, p) in world.reflection_probes.iter().enumerate() {
        let Some(img) = p.image.and_then(|id| zone.image(id)) else { continue };
        let Some(def) = img.load_def.as_ref() else { continue };
        let size = img.width as usize;
        // Top mip of each of the six faces: BGRA bytes.
        let mips = (size.max(1) as u32).ilog2() + 1;
        let face: usize = (0..mips).map(|m| ((size >> m).max(1)).pow(2) * 4).sum();
        let mut sum = [0f64; 3];
        let mut n = 0f64;
        for f in 0..6 {
            for px in def.data[f * face..f * face + size * size * 4].chunks_exact(4) {
                let a = px[3] as f64 / 255.0;
                for c in 0..3 {
                    sum[c] += px[2 - c] as f64 / 255.0 * a;
                }
                n += 1.0;
            }
        }
        println!("probe {i} at {:?}: format {} rgb*a mean ({:.3}, {:.3}, {:.3})", p.origin.map(|v| v as i32), def.format, sum[0] / n, sum[1] / n, sum[2] / n);
    }
    Ok(())
}
