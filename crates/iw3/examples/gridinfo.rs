//! Summarise a map's light grid: gridinfo <zone>
use anyhow::Result;
use iw3::zone::{LightGrid, ParseOptions, Zone};

fn main() -> Result<()> {
    let zone_name = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let install = iw3::Install::locate()?;
    let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(&zone_name))?, ParseOptions::default())?;
    let w = zone.gfx_world().expect("gfxworld");
    let g = &w.light_grid;
    println!(
        "grid mins {:?} maxs {:?} (world {:?}..{:?}), axes row {} col {}, {} rows, {} raw bytes, {} entries, {} colors",
        g.mins,
        g.maxs,
        LightGrid::world_pos(g.mins.map(u32::from)),
        LightGrid::world_pos(g.maxs.map(u32::from)),
        g.row_axis,
        g.col_axis,
        g.row_data_start.len(),
        g.raw_row_data.len(),
        g.entries.len(),
        g.colors.len()
    );
    println!("map bounds {:?}..{:?}", w.mins, w.maxs);
    let points = g.points();
    let mut seen = std::collections::HashSet::new();
    let dupes = points.iter().filter(|(p, _)| !seen.insert(*p)).count();
    let outside = points
        .iter()
        .filter(|(p, _)| (0..3).any(|i| p[i] < g.mins[i] as u32 || p[i] > g.maxs[i] as u32))
        .count();
    println!("decoded {} points ({dupes} duplicates, {outside} outside bounds)", points.len());
    for axis in 0..3 {
        let mut v: Vec<u32> = points.iter().map(|(p, _)| p[axis]).collect();
        v.sort();
        let at = |f: f32| v.get(((v.len() as f32 - 1.0) * f) as usize).copied().unwrap_or(0);
        println!("axis {axis}: grid coords 0/1/5/50/95/99/100%: {:?}", [0.0, 0.01, 0.05, 0.5, 0.95, 0.99, 1.0].map(at));
    }

    // Brightness by direction class: up, down, sideways.
    let dirs = LightGrid::directions();
    let luma = |c: [u8; 3]| (c[0] as f32 + c[1] as f32 + c[2] as f32) / 3.0;
    let (mut up, mut down, mut side, mut n) = (0.0, 0.0, 0.0, 0.0);
    for (_, e) in &points {
        let Some(c) = g.colors.get(e.colors_index as usize) else { continue };
        for (d, rgb) in dirs.iter().zip(c.0.iter()) {
            if d[2] > 0.9 && d[0].abs() < 0.5 && d[1].abs() < 0.5 {
                up += luma(*rgb);
            } else if d[2] < -0.9 && d[0].abs() < 0.5 && d[1].abs() < 0.5 {
                down += luma(*rgb);
            } else if d[2].abs() < 0.5 {
                side += luma(*rgb) / 2.0;
            }
        }
        n += 4.0;
    }
    println!("mean 8-bit level: facing up {:.1}, down {:.1}, sideways {:.1}", up / n, down / n, side / n);
    for (p, e) in points.iter().step_by(points.len().max(8) / 8) {
        let c = &g.colors[e.colors_index as usize].0;
        println!(
            "point {:?} at {:?}: primary {} trace {:#04x}; up {:?} down {:?} +x {:?} -x {:?}",
            p,
            LightGrid::world_pos(*p),
            e.primary_light_index,
            e.needs_trace,
            c[49],
            c[6],
            c[23],
            c[20]
        );
    }
    // Darkest and brightest spots near the floor (lowest point of each column).
    let mut floor: std::collections::HashMap<(u32, u32), ([u32; 3], f32)> = Default::default();
    for (p, e) in &points {
        let Some(c) = g.colors.get(e.colors_index as usize) else { continue };
        let level = c.0.iter().map(|&rgb| luma(rgb)).sum::<f32>() / 56.0;
        let slot = floor.entry((p[0], p[1])).or_insert((*p, level));
        if p[2] < slot.0[2] {
            *slot = (*p, level);
        }
    }
    let mut spots: Vec<_> = floor.values().copied().collect();
    spots.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mid: Vec<_> = spots.iter().filter(|s| (25.0..40.0).contains(&s.1)).step_by(97).take(6).collect();
    for (p, level) in spots.iter().take(5).chain(spots.iter().rev().take(3)).chain(mid) {
        println!("floor point {:?} mean level {level:.0}", LightGrid::world_pos(*p));
    }
    Ok(())
}
