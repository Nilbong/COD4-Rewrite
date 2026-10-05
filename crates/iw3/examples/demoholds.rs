//! Where the players in a demo stood still and which way they watched:
//! demoholds <file.dm_1>
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: demoholds <file.dm_1>");
    let demo = iw3::demo::read(&std::fs::read(path)?)?;
    println!("map {:?}", demo.server_info("mapname"));
    // (time, pos, yaw, pitch, flags) per (player, name).
    type S = (f32, [f32; 3], f32, f32, u32);
    let mut tracks: std::collections::BTreeMap<(u32, String), Vec<S>> = Default::default();
    let t0 = demo.snapshots.first().map_or(0, |s| s.server_time);
    for s in &demo.snapshots {
        let t = (s.server_time - t0) as f32 / 1000.0;
        for e in s.entities.iter().filter(|e| e.e_type() == 1) {
            let name = demo.name_at(e.number, s.server_time).unwrap_or("?").to_string();
            let a = e.angles();
            tracks.entry((e.number, name)).or_default().push((t, e.origin(), a[1], a[0], e.int("lerp.eFlags")));
        }
    }
    let mut holds: Vec<([f32; 3], f32, Vec<f32>, String)> = Vec::new();
    for ((_, name), track) in &tracks {
        // Runs between gaps or teleports.
        let mut start = 0;
        for i in 1..=track.len() {
            let cut = i == track.len() || {
                let (a, b) = (track[i - 1], track[i]);
                b.0 - a.0 > 0.075 || dist(a.1, b.1) > 60.0
            };
            if !cut {
                continue;
            }
            let run = &track[start..i];
            start = i;
            // Still: within 24u of where the stop began, for 1.5 s or more.
            let mut j = 0;
            while j < run.len() {
                let mut k = j;
                while k + 1 < run.len() && dist(run[k + 1].1, run[j].1) < 24.0 {
                    k += 1;
                }
                let dur = run[k].0 - run[j].0;
                if dur >= 1.5 && run[j].0 - run[0].0 > 2.0 {
                    let yaws = run[j..=k].iter().map(|s| s.2).collect();
                    holds.push((run[j].1, dur, yaws, name.clone()));
                }
                j = k + 1;
            }
        }
    }
    println!("{} holds, {:.0} s", holds.len(), holds.iter().map(|h| h.1).sum::<f32>());
    // Cluster greedily by total time.
    holds.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut clusters: Vec<([f32; 3], f32, Vec<f32>, std::collections::BTreeSet<String>)> = Vec::new();
    for (p, d, y, n) in holds {
        match clusters.iter_mut().find(|c| dist(c.0, p) < 120.0) {
            Some(c) => {
                c.1 += d;
                c.2.extend(y);
                c.3.insert(n);
            }
            None => clusters.push((p, d, y, [n].into())),
        }
    }
    clusters.sort_by(|a, b| b.1.total_cmp(&a.1));
    for c in clusters.iter().take(30) {
        // Yaw histogram in 30° bins.
        let mut bins = [0usize; 12];
        for y in &c.2 {
            bins[((y.rem_euclid(360.0)) / 30.0) as usize % 12] += 1;
        }
        let mut top: Vec<(usize, usize)> = bins.iter().copied().enumerate().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        let top: Vec<String> = top
            .iter()
            .take(3)
            .filter(|t| t.1 * 8 > c.2.len())
            .map(|t| format!("{}°:{}%", t.0 * 30 + 15, 100 * t.1 / c.2.len()))
            .collect();
        println!(
            "({:6.0} {:6.0} {:5.0}) {:5.1}s by {} players, watching {}",
            c.0[0],
            c.0[1],
            c.0[2],
            c.1,
            c.3.len(),
            top.join(" ")
        );
    }
    Ok(())
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
