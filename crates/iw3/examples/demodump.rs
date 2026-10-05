//! Every real player's track in a demo as CSV, in the columns the game's
//! recorder uses (`crates/game/src/bots/record.rs`), for comparing bots with
//! real players (`tools/compare_play.py`):
//! demodump <file.dm_1> <out dir>
//!
//! One file per player, `<demo>-<player>.csv`: t (seconds from the demo's
//! start), yaw and pitch in the recorder's sense (degrees, the game's view
//! angles: CoD yaw - 90, so 0 faces CoD's +Y; pitch + up), fire, ads,
//! feet x y z (CoD units), stance (0 stand, 1 crouch, 2 prone), team.
//! Server bots (`[BOT]...`, `bot0`...) are left out.
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(out)) = (args.next(), args.next()) else {
        anyhow::bail!("usage: demodump <file.dm_1> <out dir>");
    };
    let demo = iw3::demo::read(&std::fs::read(&path)?)?;
    let stem = std::path::Path::new(&path).file_stem().unwrap().to_string_lossy().to_string();
    std::fs::create_dir_all(&out)?;
    let t0 = demo.snapshots.first().map_or(0, |s| s.server_time);
    let mut files: std::collections::BTreeMap<String, String> = Default::default();
    for s in &demo.snapshots {
        let t = (s.server_time - t0) as f64 / 1000.0;
        for e in s.entities.iter().filter(|e| e.e_type() == 1) {
            let name = demo.name_at(e.number, s.server_time).unwrap_or("");
            let bot = name.starts_with("[BOT]")
                || name.strip_prefix("bot").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if bot || name.is_empty() {
                continue;
            }
            let team = s.clients.iter().find(|c| c.number == e.number).map_or(0, |c| c.team());
            let (p, a, flags) = (e.origin(), e.angles(), e.int("lerp.eFlags"));
            let stance = if flags & 0x8 != 0 { 2 } else { (flags & 0x4 != 0) as u32 };
            let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
            let rows = files.entry(clean).or_insert_with(|| "t,yaw,pitch,fire,ads,x,y,z,stance,team\n".into());
            rows.push_str(&format!(
                "{t:.3},{:.2},{:.2},{},{},{:.1},{:.1},{:.1},{stance},{team}\n",
                a[1] - 90.0,
                -a[0],
                (flags & 0x40 != 0) as u8,
                (flags & 0x40000 != 0) as u8,
                p[0],
                p[1],
                p[2],
            ));
        }
    }
    for (name, rows) in &files {
        std::fs::write(std::path::Path::new(&out).join(format!("{stem}-{name}.csv")), rows)?;
    }
    println!("{}: {} players", demo.server_info("mapname").unwrap_or("?"), files.len());
    Ok(())
}
