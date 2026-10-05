//! View-movement statistics of the players in a demo, outside fights:
//! demoview <file.dm_1>
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: demoview <file.dm_1>");
    let demo = iw3::demo::read(&std::fs::read(path)?)?;
    let mut tracks: std::collections::BTreeMap<u32, Vec<(f32, [f32; 3], f32, u32)>> = Default::default();
    for s in &demo.snapshots {
        for e in s.entities.iter().filter(|e| e.e_type() == 1) {
            tracks.entry(e.number).or_default().push(((s.server_time - demo.snapshots[0].server_time) as f32 / 1000.0, e.origin(), e.angles()[1], e.int("lerp.eFlags")));
        }
    }
    let (mut speeds, mut reversals, mut t, mut still) = (Vec::new(), 0, 0.0, 0);
    for track in tracks.values() {
        let mut prev_v: Option<f32> = None;
        for w in track.windows(2) {
            let (a, b) = (w[0], w[1]);
            let dt = b.0 - a.0;
            let moving = ((b.1[0] - a.1[0]).powi(2) + (b.1[1] - a.1[1]).powi(2)).sqrt() / dt.max(1e-3) > 60.0;
            if dt <= 0.0 || dt > 0.06 || !moving || (a.3 | b.3) & (0x40 | 0x40000) != 0 {
                prev_v = None;
                continue;
            }
            let v = ((b.2 - a.2 + 180.0).rem_euclid(360.0) - 180.0) / dt;
            speeds.push(v.abs());
            t += dt;
            still += (v.abs() < 2.0) as usize;
            if let Some(p) = prev_v {
                if v.abs() > 15.0 && p.abs() > 15.0 && (v > 0.0) != (p > 0.0) {
                    reversals += 1;
                }
            }
            if v.abs() > 15.0 {
                prev_v = Some(v);
            }
        }
    }
    speeds.sort_by(f32::total_cmp);
    let q = |f: f32| speeds[((speeds.len() - 1) as f32 * f) as usize].round();
    println!(
        "demo players walking {:.0}s: yaw speed p50 {} p90 {} p99 {} deg/s; still {:.0}%; reversals {:.2}/s",
        t,
        q(0.5),
        q(0.9),
        q(0.99),
        100.0 * still as f32 / speeds.len() as f32,
        reversals as f32 / t
    );
    Ok(())
}
