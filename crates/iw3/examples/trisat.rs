//! Collision triangles near a point: trisat <map> <x> <y> <z> [radius] (CoD)
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let v: Vec<f32> = a[1..4].iter().map(|s| s.parse().unwrap()).collect();
    let r: f32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(40.0);
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let clip = zone.clip_map().expect("clip map");
    for (ti, t) in clip.tri_indices.chunks_exact(3).enumerate() {
        let p: Vec<[f32; 3]> = t.iter().map(|&i| clip.verts[i as usize]).collect();
        let near = p.iter().any(|q| (0..3).all(|k| (q[k] - v[k]).abs() <= r));
        if near {
            let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
            let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
            let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
            println!("tri {ti}: {:?} normal [{:.2}, {:.2}, {:.2}]", p.iter().map(|q| q.map(|x| x.round())).collect::<Vec<_>>(), n[0] / l, n[1] / l, n[2] / l);
        }
    }
    Ok(())
}
