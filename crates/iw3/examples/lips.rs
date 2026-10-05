//! Low thin solid brushes a player walks over (door thresholds, kerbs):
//! lips <map> [cases.txt]. With a file, also writes walk-over test cases
//! for `COD4RW_LIPTEST`: `x y z dx dy` (start feet, direction; CoD units).
fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or("mp_killhouse".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&map))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    let clip = zone.clip_map().expect("clip map");
    let entity = clip.entity_brushes();
    let mut n = 0;
    let mut cases = String::new();
    for (i, b) in clip.brushes.iter().enumerate() {
        if entity.contains(&(i as u32)) || b.contents & 1 == 0 {
            continue;
        }
        let (dx, dy, dz) = (b.maxs[0] - b.mins[0], b.maxs[1] - b.mins[1], b.maxs[2] - b.mins[2]);
        let (thin, long) = (dx.min(dy), dx.max(dy));
        if (1.0..=14.0).contains(&dz) && thin <= 24.0 && (28.0..=140.0).contains(&long) {
            // Something to stand on just below it: a floor brush whose top is at its bottom.
            let floor = clip.brushes.iter().any(|f| {
                f.contents & 1 != 0
                    && (f.maxs[2] - b.mins[2]).abs() < 2.0
                    && f.mins[0] <= b.mins[0] + 1.0
                    && f.maxs[0] >= b.maxs[0] - 1.0
                    && f.mins[1] <= b.mins[1] + 1.0
                    && f.maxs[1] >= b.maxs[1] - 1.0
            });
            if floor {
                n += 1;
                let c = [(b.mins[0] + b.maxs[0]) / 2.0, (b.mins[1] + b.maxs[1]) / 2.0];
                let across = if dx < dy { [1.0, 0.0] } else { [0.0, 1.0] };
                for s in [1.0f32, -1.0] {
                    let back = thin / 2.0 + 32.0;
                    cases.push_str(&format!(
                        "{:.1} {:.1} {:.1} {} {}\n",
                        c[0] - s * across[0] * back,
                        c[1] - s * across[1] * back,
                        b.mins[2] + 1.0,
                        s * across[0],
                        s * across[1]
                    ));
                }
                println!(
                    "brush {i}: centre ({:.0} {:.0} {:.0}) size {:.0}x{:.0}x{:.0} top {:.0}",
                    (b.mins[0] + b.maxs[0]) / 2.0,
                    (b.mins[1] + b.maxs[1]) / 2.0,
                    b.mins[2],
                    dx,
                    dy,
                    dz,
                    b.maxs[2]
                );
            }
        }
    }
    println!("{n} lips");
    if let Some(path) = std::env::args().nth(2) {
        std::fs::write(path, cases)?;
    }
    Ok(())
}
