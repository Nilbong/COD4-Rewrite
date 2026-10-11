//! All reflection probes of a converted MW2 map as raw faces (64x64 BGRA,
//! face-major, mip 0) to `<out>/probeN.bin`, and which probe the surfaces
//! near a point use: `probesheet <map> <out> x y z`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let z = iw4::load_map(&iw4::Install::locate()?, &a[0])?;
    let w = z.gfx_world().unwrap();
    std::fs::create_dir_all(&a[1])?;
    for (i, p) in w.reflection_probes.iter().enumerate() {
        let Some(img) = p.image.and_then(|id| z.image(id)) else { continue };
        let d = img.load_def.as_ref().unwrap();
        println!("probe {i} at {:?} {}x{} {} bytes", p.origin, img.width, img.height, d.data.len());
        std::fs::write(format!("{}/probe{i}.bin", a[1]), &d.data)?;
    }
    let pt: Vec<f32> = a[2..5].iter().map(|v| v.parse().unwrap()).collect();
    let mut near = std::collections::BTreeMap::new();
    for s in &w.surfaces {
        let c: Vec<f32> = (0..3).map(|k| (s.bounds[0][k] + s.bounds[1][k]) / 2.0).collect();
        if (0..3).all(|k| (c[k] - pt[k]).abs() < 200.0) {
            let m = s.material.and_then(|m| z.material(m)).map(|m| m.name.clone()).unwrap_or_default();
            *near.entry((s.reflection_probe_index, m)).or_insert(0) += 1;
        }
    }
    for ((p, m), n) in near { println!("probe {p}: {m} x{n}"); }
    Ok(())
}
