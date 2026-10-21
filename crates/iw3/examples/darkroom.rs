//! Spawn points (any mode's) with a roof over them, darkest first by the
//! light grid where a player stands there: darkroom <map>.
use iw3::zone::LightGrid;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let clip = zone.clip_map().expect("clip map");
    let g = &zone.gfx_world().expect("gfx world").light_grid;
    let luma = |c: [u8; 3]| (c[0] as f32 + c[1] as f32 + c[2] as f32) / 3.0;
    let points: Vec<([f32; 3], f32)> = g
        .points()
        .into_iter()
        .filter_map(|(p, e)| Some((LightGrid::world_pos(p), g.colors.get(e.colors_index as usize)?.0.iter().map(|&c| luma(c)).sum::<f32>() / 56.0)))
        .collect();
    let ents = iw3::ents::parse(&zone.map_ents().expect("ents").entity_string);
    let mut out = Vec::new();
    for e in ents.iter().filter(|e| e.get("classname").is_some_and(|c| c.contains("spawn"))) {
        let Some(o) = e.get("origin") else { continue };
        let p: Vec<f32> = o.split_whitespace().filter_map(|v| v.parse().ok()).collect();
        let roofed = clip.brushes.iter().any(|b| b.contents & 1 != 0 && p[0] >= b.mins[0] && p[0] <= b.maxs[0] && p[1] >= b.mins[1] && p[1] <= b.maxs[1] && b.mins[2] > p[2] + 60.0 && b.mins[2] < p[2] + 200.0);
        if !roofed {
            continue;
        }
        let eye = [p[0], p[1], p[2] + 48.0];
        let near = points.iter().min_by(|a, b| {
            let d = |q: &[f32; 3]| (0..3).map(|i| (q[i] - eye[i]).powi(2)).sum::<f32>();
            d(&a.0).total_cmp(&d(&b.0))
        });
        if let Some((_, level)) = near {
            out.push((p.clone(), *level, e.get("classname").unwrap().to_owned(), e.get("angles").unwrap_or("").to_owned()));
        }
    }
    out.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (p, l, c, ang) in out.iter().take(10) {
        println!("{p:?} level {l:.1} {c} angles {ang}");
    }
    Ok(())
}
