//! Run a level's main script headless: runlevel <level> [seconds]
//! Loads common's and the level's scripts and the level's entities, runs
//! `maps\<level>::main` at 20 Hz with the player at its start, and reports
//! script errors, unknown builtins and what was asked of the game.
use gsc::level::{Command, Level, PLAYER};
use gsc::vm::{Value, Vm};
use std::collections::HashMap;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let level_name = a.next().unwrap_or("cargoship".into());
    let secs: f64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let walk: Vec<[f32; 3]> = a.next().map(|s| s.split(';').filter_map(|p| {
        let v: Vec<f32> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    }).collect()).unwrap_or_default();
    let install = iw3::Install::locate()?;
    let mut vm = Vm::new();
    let t0 = std::time::Instant::now();
    let mut ents = Vec::new();
    let mut bounds = Vec::new();
    for z in ["common", level_name.as_str()] {
        let zone = iw3::zone::Zone::parse(&iw3::fastfile::load(&install.zone_path(z))?, iw3::zone::ParseOptions::default())?;
        for asset in &zone.assets {
            let iw3::zone::Asset::RawFile(r) = asset else { continue };
            if r.name.ends_with(".gsc") {
                if let Err(e) = vm.add_script(&r.name, &String::from_utf8_lossy(&r.data)) {
                    println!("skip {}: {e}", r.name);
                }
            }
        }
        if z != "common" {
            ents = iw3::ents::parse(&zone.map_ents().unwrap().entity_string).into_iter().map(|e| e.fields.into_iter().collect::<Vec<_>>()).collect();
            bounds = zone.clip_map().map(|c| c.cmodels.iter().map(|m| (m.mins, m.maxs)).collect()).unwrap_or_default();
        }
    }
    let unresolved = vm.link();
    println!("loaded in {:.2?}; {} entities, {} brush models; unresolved script calls: {unresolved:?}", t0.elapsed(), ents.len(), bounds.len());
    let mut level = Level::new(&ents, &bounds);
    if let Ok(j) = std::env::var("JUMPTO") {
        level.dvars.insert("jumpto".into(), j);
    }
    level.install(&mut vm, &ents);
    let start = level.ents.iter().find(|e| e.classname == "info_player_start").map(|e| e.origin).unwrap_or_default();
    level.set_pose(PLAYER, start, [0.0; 3]);
    let main = vm.func(&format!("maps/{level_name}"), "main").expect("no main");
    let level_obj = Value::Object(vm.level);
    vm.spawn_thread(main, level_obj, Vec::new());
    let t1 = std::time::Instant::now();
    let mut t = 0.0;
    let mut kinds: HashMap<String, u32> = HashMap::new();
    let mut prints = Vec::new();
    while t < secs {
        // Walk the player along the given points, a few seconds each.
        if !walk.is_empty() {
            let i = ((t / 4.0) as usize).min(walk.len() - 1);
            level.set_pose(PLAYER, walk[i], [0.0; 3]);
        }
        level.update(&mut vm, t);
        vm.run(&mut level, t);
        for c in level.commands.drain(..) {
            let k = match &c {
                Command::Call { name, ent, args } => {
                    if *ent == PLAYER {
                        prints.push(format!("{t:.1}s player {name} {args:?}"));
                    }
                    format!("call:{name}")
                }
                Command::Print { text, .. } => { prints.push(format!("{t:.1}s {text}")); "print".into() }
                Command::Objective { index, state, text, at } => { prints.push(format!("{t:.1}s objective {index} {state} {text} {at:?}")); "objective".into() }
                Command::SpawnAi { classname, .. } => format!("spawn:{classname}"),
                Command::MissionFailed | Command::MissionSuccess | Command::ChangeLevel(_) => { prints.push(format!("{t:.1}s {c:?}")); format!("{c:?}") }
                other => format!("{other:?}").split([' ', '(', '{']).next().unwrap_or("").to_owned(),
            };
            *kinds.entry(k).or_default() += 1;
        }
        t += 0.05;
    }
    println!("ran {secs}s of script in {:.2?}; {} threads alive, {} AI alive", t1.elapsed(), vm.live_threads(), level.ents.iter().filter(|e| e.alive && e.id != PLAYER).count());
    let mut errors: HashMap<String, u32> = HashMap::new();
    for e in &vm.errors {
        *errors.entry(e.clone()).or_default() += 1;
    }
    let mut errors: Vec<_> = errors.into_iter().collect();
    errors.sort_by(|a, b| b.1.cmp(&a.1));
    println!("{} script errors ({} distinct):", vm.errors.len(), errors.len());
    for (e, n) in errors.iter().take(30) {
        println!("  {n:4}x {e}");
    }
    let mut missing: Vec<_> = vm.missing.iter().collect();
    missing.sort_by(|a, b| b.1.cmp(a.1));
    println!("unknown builtins: {}", missing.iter().map(|(n, c)| format!("{n}({c})")).collect::<Vec<_>>().join(" "));
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    println!("commands: {}", kinds.iter().map(|(n, c)| format!("{n}({c})")).collect::<Vec<_>>().join(" "));
    for p in prints.iter().take(40) {
        println!("  {p}");
    }
    Ok(())
}
