//! Summarise a CoD4X demo: demoinfo <file.dm_1>
use anyhow::Result;
use std::collections::BTreeMap;

fn main() -> Result<()> {
    let path = std::env::args().nth(1).expect("usage: demoinfo <file.dm_1>");
    let t0 = std::time::Instant::now();
    let demo = iw3::demo::read(&std::fs::read(&path)?)?;
    println!(
        "protocol {}, map {:?}, gametype {:?}, recorded by client {}, read in {:?}",
        demo.protocol,
        demo.server_info("mapname"),
        demo.server_info("g_gametype"),
        demo.client_num,
        t0.elapsed()
    );
    println!("{} frames, {} snapshots, {} server commands", demo.frames.len(), demo.snapshots.len(), demo.commands.len());
    if let (Some(a), Some(b)) = (demo.snapshots.first(), demo.snapshots.last()) {
        println!("server time {} .. {} ({:.0} s)", a.server_time, b.server_time, (b.server_time - a.server_time) as f32 / 1000.0);
    }
    // How often each player entity shows up, and their teams.
    let mut seen: BTreeMap<u32, usize> = BTreeMap::new();
    let mut types: BTreeMap<u32, usize> = BTreeMap::new();
    for s in &demo.snapshots {
        for e in &s.entities {
            *types.entry(e.e_type().min(17)).or_default() += 1;
            if e.e_type() == 1 {
                *seen.entry(e.number).or_default() += 1;
            }
        }
    }
    println!("entity types (count over snapshots): {types:?}");
    let teams: BTreeMap<u32, u32> =
        demo.snapshots.last().map(|s| s.clients.iter().map(|c| (c.number, c.team())).collect()).unwrap_or_default();
    for (n, count) in &seen {
        let name = demo.names.get(n).map_or("?", |(name, _)| name.as_str());
        println!("  player {n:2} {name:20} team {:?} in {count} snapshots", teams.get(n));
    }
    // A few positions of one player, to eyeball.
    if let Some((&n, _)) = seen.iter().max_by_key(|(_, c)| **c) {
        let track: Vec<_> = demo
            .snapshots
            .iter()
            .filter_map(|s| s.entities.iter().find(|e| e.number == n && e.e_type() == 1).map(|e| (s.server_time, e.origin(), e.angles())))
            .collect();
        for (t, o, a) in track.iter().step_by(track.len().max(6) / 6) {
            println!("    t {t} origin {o:?} angles {a:?}");
        }
    }
    // Decoding check: players move smoothly between snapshots, except when
    // they respawn.
    let mut last: BTreeMap<u32, (i32, [f32; 3])> = BTreeMap::new();
    let (mut steps, mut jumps, mut fast) = (0, 0, 0);
    for s in &demo.snapshots {
        for e in s.entities.iter().filter(|e| e.e_type() == 1) {
            let o = e.origin();
            if let Some((t, p)) = last.get(&e.number) {
                if s.server_time - t <= 100 {
                    let d = ((o[0] - p[0]).powi(2) + (o[1] - p[1]).powi(2) + (o[2] - p[2]).powi(2)).sqrt();
                    steps += 1;
                    jumps += (d > 300.0) as usize;
                    fast += (d > 40.0 && d <= 300.0) as usize;
                }
            }
            last.insert(e.number, (s.server_time, o));
        }
    }
    println!("continuity: {steps} steps of <=100 ms, {fast} over 40 units (faster than sprinting), {jumps} over 300 (respawns)");
    for f in demo.frames.iter().step_by(demo.frames.len().max(4) / 4) {
        println!("  frame t {} origin {:?} angles {:?}", f.command_time, f.origin, f.angles);
    }
    for (t, c) in demo.commands.iter().take(8) {
        println!("  cmd @{t}: {c}");
    }
    Ok(())
}
