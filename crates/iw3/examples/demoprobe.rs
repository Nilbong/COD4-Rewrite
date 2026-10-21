//! Look at what a demo carries for player traits: demoprobe <file.dm_1>
//! (weapon list, event entity kinds, obituaries, missiles, ranks, commands).
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: demoprobe <file.dm_1>");
    let demo = iw3::demo::read(&std::fs::read(path)?)?;
    let weapons: Vec<&str> = demo.config_strings.get(&2258).map_or(vec![], |s| s.split(' ').collect());
    println!("weapons ({}): {:?}", weapons.len(), weapons);
    let mut types: std::collections::BTreeMap<u32, usize> = Default::default();
    let mut seen = std::collections::HashSet::new();
    let mut obits = 0;
    let mut missiles: std::collections::BTreeMap<(u32, u32), usize> = Default::default();
    let mut pevents: std::collections::BTreeMap<u32, usize> = Default::default();
    for s in &demo.snapshots {
        for e in &s.entities {
            let t = e.e_type();
            *types.entry(t).or_default() += 1;
            if t == 17 + 0x42 && seen.insert((e.number, e.int("eventSequence"), e.int("otherEntityNum"), e.int("attackerEntityNum"), s.server_time / 1000)) && obits < 25 {
                obits += 1;
                let parm = e.int("eventParm");
                println!(
                    "obit t={} victim {} {:?} attacker {} {:?} parm {} {:?}",
                    s.server_time,
                    e.int("otherEntityNum"),
                    demo.name_at(e.int("otherEntityNum"), s.server_time),
                    e.int("attackerEntityNum"),
                    demo.name_at(e.int("attackerEntityNum"), s.server_time),
                    parm,
                    weapons.get(parm as usize)
                );
            }
            if t == 4 {
                *missiles.entry((e.int("weapon"), e.int("ClientNum"))).or_default() += 1;
            }
            if t == 1 {
                for k in 0..4 {
                    *pevents.entry(e.int(&format!("events[{k}]"))).or_default() += 1;
                }
            }
        }
    }
    println!("entity types: {types:?}");
    // Victim 3's entity around its first death.
    if let Some(dt) = std::env::args().nth(2).and_then(|v| v.parse::<i32>().ok()) {
        let who: u32 = std::env::args().nth(3).and_then(|v| v.parse().ok()).unwrap_or(3);
        for s in demo.snapshots.iter().filter(|s| (s.server_time - dt).abs() < 4000) {
            let e = s.entities.iter().find(|e| e.number == who && e.e_type() == 1);
            println!("t {} {:?}", s.server_time - dt, e.map(|e| (e.origin().map(|v| v as i32), format!("{:#x}", e.int("lerp.eFlags")), e.int("weapon"), e.int("legsAnim"), e.int("torsoAnim"))));
        }
    }
    println!("missiles (weapon, owner) -> snapshots: {:?}", missiles.iter().take(30).map(|((w, c), n)| (weapons.get(*w as usize).copied().unwrap_or("?"), *c, *n)).collect::<Vec<_>>());
    println!("player event slots: {pevents:?}");
    let last = demo.snapshots.last().unwrap();
    for c in &last.clients {
        println!("client {} {:?} team {} rank {} prestige {} perks {:#x}", c.number, demo.name_at(c.number, last.server_time), c.team(), c.int("rank"), c.int("prestige"), c.int("perks"));
    }
    let mut cmds: std::collections::BTreeMap<String, (usize, String)> = Default::default();
    for (_, c) in &demo.commands {
        let k: String = c.chars().take_while(|ch| !ch.is_whitespace()).collect();
        let e = cmds.entry(k).or_insert((0, c.chars().take(120).collect()));
        e.0 += 1;
    }
    let mut fs: std::collections::BTreeMap<String, usize> = Default::default();
    for (_, c) in &demo.commands {
        if c.starts_with("f ") || c.starts_with("e ") || c.starts_with("g ") {
            let k: String = c.split(" by ").next().unwrap_or("").chars().take(70).collect();
            *fs.entry(k).or_default() += 1;
        }
    }
    for (k, n) in fs { println!("msg x{n}: {k:?}"); }
    for (k, (n, ex)) in cmds {
        println!("cmd {k:?} x{n}: {ex:?}");
    }
    Ok(())
}
