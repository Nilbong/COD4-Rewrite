//! Which world surfaces each primary light lights (their bounds and
//! triangles), busiest first: plightsurfs <zone> [count]
use anyhow::Result;
use iw3::zone::{ParseOptions, Zone};

fn main() -> Result<()> {
    let zone_name = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let count: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(8);
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?, ParseOptions::default())?;
    let (Some(com), Some(gfx)) = (zone.com_world(), zone.gfx_world()) else { return Ok(()) };
    let mut per: Vec<(usize, u32, [f32; 3], [f32; 3])> = Vec::new();
    for (i, l) in com.primary_lights.iter().enumerate() {
        if !matches!(l.kind, 2 | 3) {
            continue;
        }
        let mut tris = 0u32;
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for s in gfx.surfaces.iter().filter(|s| s.primary_light_index as usize == i) {
            tris += s.tri_count as u32;
            for k in 0..3 {
                lo[k] = lo[k].min(s.bounds[0][k]);
                hi[k] = hi[k].max(s.bounds[1][k]);
            }
        }
        if tris > 0 {
            per.push((i, tris, lo, hi));
        }
    }
    per.sort_by_key(|p| std::cmp::Reverse(p.1));
    println!("{zone_name}: {} spot/omni lights light surfaces", per.len());
    for (i, tris, lo, hi) in per.iter().take(count) {
        let l = &com.primary_lights[*i];
        println!("  light {i} kind {} at {:.0?} r {:.0} colour {:.2?}: {tris} tris in {:.0?}..{:.0?}", l.kind, l.origin, l.radius, l.color, lo, hi);
    }
    Ok(())
}
