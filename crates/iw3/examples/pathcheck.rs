//! Solid brushes along a walk: pathcheck <map> <x0> <y0> <z0> <x1> <y1> <z1> [radius]
//! Samples the line (the feet, CoD units) and, at each point, heights 2..70
//! above it on a ring of `radius` (default 15, the player's), listing the
//! solid (and player clip) brushes found.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let v: Vec<f32> = a[1..7].iter().map(|s| s.parse().unwrap()).collect();
    let r: f32 = a.get(7).and_then(|s| s.parse().ok()).unwrap_or(15.0);
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let clip = zone.clip_map().expect("clip map");
    let entity = clip.entity_brushes();
    let inside = |b: &iw3::zone::Brush, p: [f32; 3]| {
        (0..3).all(|k| b.mins[k] < p[k] && p[k] < b.maxs[k])
            && b.side_planes.iter().all(|&s| {
                let pl = &clip.planes[s as usize];
                pl.normal[0] * p[0] + pl.normal[1] * p[1] + pl.normal[2] * p[2] - pl.dist < 0.0
            })
    };
    for i in 0..=20 {
        let t = i as f32 / 20.0;
        let f = [v[0] + (v[3] - v[0]) * t, v[1] + (v[4] - v[1]) * t, v[2] + (v[5] - v[2]) * t];
        let mut hits = std::collections::BTreeMap::new();
        for h in [2.0f32, 10.0, 20.0, 35.0, 50.0, 60.0, 70.0] {
            for k in 0..9 {
                let (dx, dy) = if k == 8 { (0.0, 0.0) } else { let a = k as f32 * std::f32::consts::FRAC_PI_4; (a.cos() * r, a.sin() * r) };
                let p = [f[0] + dx, f[1] + dy, f[2] + h];
                for (bi, b) in clip.brushes.iter().enumerate() {
                    if b.contents & 0x10011 != 0 && !entity.contains(&(bi as u32)) && inside(b, p) {
                        hits.entry(bi).or_insert_with(Vec::new).push(h as i32);
                    }
                }
            }
        }
        let list: Vec<String> = hits.iter().map(|(b, hs)| format!("{b}({:#x})@{:?}", clip.brushes[*b].contents, hs.iter().min())).collect();
        println!("t {t:.2} feet ({:.0}, {:.0}, {:.0}): {}", f[0], f[1], f[2], list.join(" "));
    }
    Ok(())
}
