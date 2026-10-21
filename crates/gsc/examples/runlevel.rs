//! Run a level's main script headless: runlevel <level> [seconds]
//! Loads common's and the level's scripts and the level's entities, runs
//! `maps\<level>::main` at 20 Hz with the player at its start, and reports
//! script errors, unknown builtins and what was asked of the game.
use gsc::level::{AnimInfo, AnimLib, Command, Level, PLAYER};
use gsc::vm::{Value, Vm};
use std::collections::HashMap;

/// The zones' animations for the scripts (lengths, notetracks, root
/// motion), decoded on first use, as the game's.
#[derive(Default)]
struct Anims {
    raw: HashMap<String, (iw3::zone::generic::GNode, usize)>,
    strings: Vec<Vec<String>>,
    decoded: HashMap<String, Option<std::rc::Rc<iw3::xanim::XAnim>>>,
}

impl Anims {
    fn get(&mut self, name: &str) -> Option<std::rc::Rc<iw3::xanim::XAnim>> {
        let key = name.to_ascii_lowercase();
        if let Some(a) = self.decoded.get(&key) {
            return a.clone();
        }
        let a = self.raw.get(&key).and_then(|(n, zi)| iw3::xanim::XAnim::from_node(name, n, &self.strings[*zi]).ok()).map(std::rc::Rc::new);
        self.decoded.insert(key, a.clone());
        a
    }
}

impl AnimLib for Anims {
    fn info(&mut self, name: &str) -> Option<AnimInfo> {
        let a = self.get(name)?;
        Some(AnimInfo {
            length: a.duration().max(1.0 / 30.0),
            looping: a.looping,
            delta: a.delta,
            notes: a.notifies.iter().map(|n| (n.time, n.name.clone())).collect(),
            partial: !a.bones.iter().any(|b| b.name.eq_ignore_ascii_case("j_mainroot")),
        })
    }

    fn delta(&mut self, name: &str, frac: f32) -> ([f32; 3], f32) {
        match self.get(name) {
            Some(a) => (a.delta_at(frac), a.delta_yaw_at(frac).to_degrees()),
            None => ([0.0; 3], 0.0),
        }
    }
}

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
    let mut path_nodes = Vec::new();
    let mut shapes: Vec<iw3::brushes::Shape> = Vec::new();
    let mut anims = Anims::default();
    for z in ["code_post_gfx", "common", level_name.as_str()] {
        let zone = iw3::zone::Zone::parse(&iw3::fastfile::load(&install.zone_path(z))?, iw3::zone::ParseOptions::default())?;
        let zi = anims.strings.len();
        anims.strings.push(zone.script_strings.clone());
        for a in &zone.assets {
            if let iw3::zone::Asset::Generic(g) = a
                && g.ty == iw3::zone::AssetType::XAnimParts
                && !g.name.starts_with(',')
            {
                anims.raw.insert(g.name.to_ascii_lowercase(), (g.root.clone(), zi));
            }
        }
        for asset in &zone.assets {
            let iw3::zone::Asset::RawFile(r) = asset else { continue };
            if r.name.ends_with(".gsc") {
                if let Err(e) = vm.add_script(&r.name, &String::from_utf8_lossy(&r.data)) {
                    println!("skip {}: {e}", r.name);
                }
            }
        }
        if z == level_name.as_str() {
            ents = iw3::ents::parse(&zone.map_ents().unwrap().entity_string).into_iter().map(|e| e.fields.into_iter().collect::<Vec<_>>()).collect();
            path_nodes = iw3::paths::from_zone(&zone).into_iter().map(|n| gsc::level::PathNode { origin: n.origin, kind: n.kind, angle: n.angle, links: n.links }).collect();
            shapes = zone.clip_map().map(|c| (0..c.cmodels.len()).map(|n| iw3::brushes::model_shape(c, n)).collect()).unwrap_or_default();
            bounds = zone.clip_map().map(|c| c.cmodels.iter().map(|m| (m.mins, m.maxs)).collect()).unwrap_or_default();
        }
    }
    let unresolved = vm.link();
    println!("loaded in {:.2?}; {} entities, {} brush models; unresolved script calls: {unresolved:?}", t0.elapsed(), ents.len(), bounds.len());
    let mut level = Level::new(&ents, &bounds);
    level.set_brush_shapes(|n| shapes.get(n).cloned().unwrap_or_default());
    level.think_ai = std::env::var_os("THINK").is_some();
    level.anims = Box::new(anims);
    // (As the game sets it: `level.script` comes from it.)
    level.dvars.insert("mapname".into(), level_name.to_string());
    // (A fresh profile's progress, as the game's `campaign::progress`.)
    level.dvars.insert("mis_difficulty".into(), "0".repeat(40));
    level.dvars.insert("mis_01".into(), "0".into());
    println!("{} path nodes", path_nodes.len());
    level.path_nodes = path_nodes;
    if let Ok(j) = std::env::var("JUMPTO") {
        level.dvars.insert("jumpto".into(), j.clone());
        level.dvars.insert("start".into(), j);
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
    // `FOLLOW=<classname part>`: the player walks behind that actor (at
    // `FOLLOW_SPEED`, 250), and when it waits goes to the nearest trigger
    // a script waits on, or else to the nearest enemy and kills it (5 s
    // next to it): a playthrough without the game. Actors' shots hit as
    // the game rolls them (18% x accuracy, 20 damage; the player can't die).
    let follow = std::env::var("FOLLOW").ok();
    // (The map's own places by targetname: a lookat's target is read and
    // deleted by the level's scripts at the start.)
    let places: HashMap<String, [f32; 3]> = ents
        .iter()
        .filter_map(|kv| {
            let get = |k: &str| kv.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
            let o: Vec<f32> = get("origin")?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            let name = get("targetname")?;
            (o.len() == 3).then(|| (name, [o[0], o[1], o[2]]))
        })
        .collect();
    let speed: f32 = std::env::var("FOLLOW_SPEED").ok().and_then(|s| s.parse().ok()).unwrap_or(250.0);
    let mut me = start;
    let mut trail: Vec<(f64, [f32; 3])> = Vec::new();
    let mut way: Vec<[f32; 3]> = Vec::new();
    let mut near: Option<(gsc::level::EntId, f64)> = None;
    // (Triggers stood in 3 s with nothing coming of it: a looping wait,
    // the soundscape's or effects', not the way on.)
    let mut tried: Vec<[f32; 3]> = Vec::new();
    let mut stood: Option<([f32; 3], f64)> = None;
    // (And ones chased 20 s without getting there: out of reach.)
    let mut chased: Vec<([f32; 3], f64)> = Vec::new();
    // (Use triggers pressed: 3 presses with nothing coming of it is enough.)
    let mut pressed: Vec<[f32; 3]> = Vec::new();
    let mut seed = 12345u32;
    let mut roll = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed % 10000) as f32 / 10000.0
    };
    let dist = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    let mut last_report = -100.0;
    while t < secs {
        // (Where a script put the player, `setOrigin`, it is.)
        if let Some(p) = level.ent(PLAYER).map(|e| e.origin)
            && dist(p, me) > 1.0
        {
            me = p;
            way.clear();
        }
        if let Some(who) = &follow
            && level.player_link.is_none()
        {
            let leader = level.ents.iter().find(|e| e.kind == gsc::level::Kind::Ai && e.alive && e.classname.contains(who.as_str())).map(|e| (e.origin, e.linked.is_some()));
            if let Some((at, riding)) = leader {
                if !riding && trail.last().is_none_or(|l| dist(l.1, at) > 8.0) {
                    trail.push((t, at));
                }
                let waiting = trail.iter().rev().find(|p| t - p.0 >= 6.0).is_some_and(|p| dist(p.1, at) < 200.0) || trail.last().is_some_and(|l| t - l.0 > 4.0);
                let mut goal = None;
                // (`RUSH=1`: straight for the nearest trigger a script waits
                // on, a timed run's next mark.)
                let rush = std::env::var_os("RUSH").is_some().then(|| level.awaited_flag_triggers(&vm).into_iter().filter(|p| !tried.iter().any(|q| dist(*p, *q) < 1.0)).min_by(|a, b| dist(*a, me).total_cmp(&dist(*b, me)))).flatten();
                if let Some(r) = rush {
                    goal = Some(r);
                    if (r[0] - me[0]).hypot(r[1] - me[1]) < 24.0 && (r[2] - me[2]).abs() < 96.0 {
                        match stood {
                            Some((p, since)) if dist(p, r) < 1.0 => {
                                if t - since > 3.0 {
                                    tried.push(r);
                                    stood = None;
                                }
                            }
                            _ => stood = Some((r, t)),
                        }
                    }
                } else if waiting {
                    let d = |p: &[f32; 3]| (p[0] - at[0]).hypot(p[1] - at[1]) + (p[2] - at[2]).abs() * 2.0;
                    // (A use trigger by the leader first: what it waits for.)
                    let by_leader = level
                        .ents
                        .iter()
                        .filter(|e| e.classname == "trigger_use" && e.enabled && vm.entity_obj(e.id).is_some_and(|o| vm.waiting_on(o, "trigger")))
                        .map(|e| match e.bounds {
                            Some((mn, mx)) => [e.origin[0] + (mn[0] + mx[0]) * 0.5, e.origin[1] + (mn[1] + mx[1]) * 0.5, e.origin[2] + mn[2] + 1.0],
                            None => e.origin,
                        })
                        .find(|p| dist(*p, at) < 300.0 && !tried.iter().any(|q| dist(*p, *q) < 1.0));
                    if let Some(u) = by_leader {
                        if (u[0] - me[0]).hypot(u[1] - me[1]) < 4.0 {
                            if (t * 20.0) as i64 % 20 == 0 {
                                prints.push(format!("{t:.1}s driver presses Use at {:?} by the leader", u.map(f32::round)));
                                level.use_pressed(&mut vm);
                                pressed.push(u);
                                if pressed.iter().filter(|p| dist(**p, u) < 1.0).count() >= 3 {
                                    tried.push(u);
                                }
                            }
                        } else {
                            goal = Some(u);
                        }
                    }
                    if goal.is_none() && by_leader.is_none() {
                        goal = level.awaited_flag_triggers(&vm).into_iter().filter(|p| d(p) < 2000.0 && !tried.iter().any(|q| dist(*p, *q) < 1.0)).min_by(|a, b| d(a).total_cmp(&d(b)));
                        if let Some(g) = goal {
                            let i = chased.iter().position(|c| dist(c.0, g) < 1.0).unwrap_or_else(|| {
                                chased.push((g, 0.0));
                                chased.len() - 1
                            });
                            chased[i].1 += 0.05;
                            if chased[i].1 > 20.0 {
                                prints.push(format!("{t:.1}s driver gives up on {:?}", g.map(f32::round)));
                                tried.push(g);
                            }
                        }
                    }
                    if let Some(g) = goal
                        && (g[0] - me[0]).hypot(g[1] - me[1]) < 24.0
                        && (g[2] - me[2]).abs() < 96.0
                    {
                        match stood {
                            Some((p, since)) if dist(p, g) < 1.0 => {
                                if t - since > 3.0 {
                                    let inside: Vec<String> = level.ents.iter().filter(|e| e.is_trigger() && (e.contains(me) || e.contains([me[0], me[1], me[2] + 40.0]))).map(|e| format!("{}:{}", e.classname, e.get("targetname").unwrap_or(""))).collect();
                                    prints.push(format!("{t:.1}s driver stood at {:?} for nothing; in {inside:?}; angles {:?}", g.map(f32::round), level.ent(PLAYER).map(|e| e.angles)));
                                    tried.push(g);
                                    stood = None;
                                }
                            }
                            _ => stood = Some((g, t)),
                        }
                    }
                    // (No trigger to walk into: a use one a script waits on,
                    // pressed there.)
                    if goal.is_none() && by_leader.is_none() && !level.ents.iter().any(|e| e.kind == gsc::level::Kind::Ai && e.alive && e.team == "axis" && dist(e.origin, me) < 4000.0) {
                        let usable = level
                            .ents
                            .iter()
                            .filter(|e| e.classname == "trigger_use" && e.enabled && vm.entity_obj(e.id).is_some_and(|o| vm.waiting_on(o, "trigger")))
                            .map(|e| match e.bounds {
                                Some((mn, mx)) => [e.origin[0] + (mn[0] + mx[0]) * 0.5, e.origin[1] + (mn[1] + mx[1]) * 0.5, e.origin[2] + mn[2] + 1.0],
                                None => e.origin,
                            })
                            .filter(|p| dist(*p, me) < 2500.0 && !tried.iter().any(|q| dist(*p, *q) < 1.0))
                            .min_by(|a, b| dist(*a, me).total_cmp(&dist(*b, me)));
                        if let Some(u) = usable {
                            if (u[0] - me[0]).hypot(u[1] - me[1]) < 4.0 {
                                if (t * 20.0) as i64 % 20 == 0 {
                                    prints.push(format!("{t:.1}s driver presses Use at {:?}", u.map(f32::round)));
                                    level.use_pressed(&mut vm);
                                    pressed.push(u);
                                    if pressed.iter().filter(|p| dist(**p, u) < 1.0).count() >= 3 {
                                        tried.push(u);
                                    }
                                }
                            } else {
                                goal = Some(u);
                            }
                        }
                    }
                    let foes_near = level.ents.iter().any(|e| e.kind == gsc::level::Kind::Ai && e.alive && e.team == "axis" && dist(e.origin, me) < 4000.0);
                    if goal.is_none() && by_leader.is_none() && foes_near {
                        let foe = level.ents.iter().filter(|e| e.kind == gsc::level::Kind::Ai && e.alive && e.team == "axis" && dist(e.origin, me) < 4000.0).map(|e| (e.id, e.origin)).min_by(|a, b| dist(a.1, me).total_cmp(&dist(b.1, me)));
                        if let Some((id, p)) = foe {
                            if dist(p, me) > 200.0 {
                                goal = Some(p);
                            } else {
                                match near {
                                    // (Near one or another of them 5 s.)
                                    Some((_, since)) => {
                                        if t - since > 5.0 {
                                            prints.push(format!("{t:.1}s driver kills {id} at {:?}", p.map(f32::round)));
                                            let hp = level.ent(id).map_or(1, |e| e.health.max(1));
                                            level.damaged(&mut vm, id, hp + 1, Some(PLAYER), [0.0; 3], p);
                                            near = None;
                                        }
                                    }
                                    None => near = Some((id, t)),
                                }
                            }
                        }
                    }
                } else {
                    // (Behind it on its trail, 48 back.)
                    goal = trail.iter().rev().find(|p| dist(p.1, at) >= 48.0).map(|p| p.1).filter(|p| dist(*p, me) > 16.0);
                }
                if let Some(g) = goal {
                    // (Made again when the goal moves on, not as an actor
                    // there shifts; not back to a node behind.)
                    if way.last().is_none_or(|l| dist(*l, g) > 64.0) {
                        way = level.route(me, g).unwrap_or_default();
                        if way.len() >= 2 && dist(me, way[1]) < dist(way[0], way[1]) {
                            way.remove(0);
                        }
                        way.push(g);
                    } else if let Some(l) = way.last_mut() {
                        *l = g;
                    }
                    let mut budget = speed * 0.05;
                    while budget > 0.0 && !way.is_empty() {
                        let gap = dist(way[0], me);
                        if gap <= budget {
                            me = way[0];
                            budget -= gap;
                            if way.len() > 1 {
                                way.remove(0);
                            } else {
                                budget = 0.0;
                            }
                        } else {
                            let k = budget / gap;
                            me = [me[0] + (way[0][0] - me[0]) * k, me[1] + (way[0][1] - me[1]) * k, me[2] + (way[0][2] - me[2]) * k];
                            budget = 0.0;
                        }
                    }
                }
                // (In a `trigger_lookat`: looking at its target, as a
                // person would at what the level points out.)
                let look = level
                    .ents
                    .iter()
                    .find(|e| e.get("targetname") == Some("trigger_lookat") && e.enabled && (e.contains(me) || e.contains([me[0], me[1], me[2] + 40.0])))
                    .and_then(|e| e.get("target"))
                    .and_then(|tn| level.ents.iter().find(|x| x.get("targetname") == Some(tn)).map(|x| x.origin).or_else(|| places.get(tn).copied()));
                let angles = look.map_or([0.0; 3], |p| {
                    let d = [p[0] - me[0], p[1] - me[1], p[2] - me[2] - 60.0];
                    [-(d[2].atan2(d[0].hypot(d[1]))).to_degrees(), d[1].atan2(d[0]).to_degrees(), 0.0]
                });
                if look.is_some() && (t * 20.0) as i64 % 10 == 0 {
                    prints.push(format!("{t:.1}s driver looks at {:?} from {:?} ({angles:?})", look.map(|p| p.map(f32::round)), me.map(f32::round)));
                }
                level.set_pose(PLAYER, me, angles);
                if t - last_report >= 30.0 {
                    last_report = t;
                    if std::env::var_os("AWAITED").is_some() {
                        prints.push(format!("{t:.1}s awaited {:?} tried {:?}", level.awaited_flag_triggers(&vm).iter().map(|p| p.map(f32::round)).collect::<Vec<_>>(), tried.iter().map(|p| p.map(f32::round)).collect::<Vec<_>>()));
                    }
                    prints.push(format!("{t:.1}s player at {:?}, leader at {:?}{} going to {:?}", me.map(f32::round), at.map(f32::round), if waiting { " (waiting)" } else { "" }, goal.map(|g| g.map(f32::round))));
                }
            }
        }
        // Walk the player along the given points, a few seconds each.
        if !walk.is_empty() {
            let i = ((t / 4.0) as usize).min(walk.len() - 1);
            level.set_pose(PLAYER, walk[i], [0.0; 3]);
        }
        // `SIGHT=1`: every actor sees every hostile (no walls headless).
        if std::env::var_os("SIGHT").is_some() {
            // (Within 1500 units: walls headless are guessed by distance.)
            let pairs = level.sight_pairs(&vm);
            level.sight = pairs
                .into_iter()
                .map(|(a, b)| {
                    let near = level.ent(a).zip(level.ent(b)).is_some_and(|(x, y)| ((x.origin[0] - y.origin[0]).powi(2) + (x.origin[1] - y.origin[1]).powi(2) + (x.origin[2] - y.origin[2]).powi(2)).sqrt() < 1500.0);
                    ((a, b), near)
                })
                .collect();
        }
        level.update(&mut vm, t);
        vm.run(&mut level, t);
        let mut shots = Vec::new();
        for c in level.commands.drain(..) {
            let k = match &c {
                Command::Call { name, ent, args } => {
                    if *ent == PLAYER && (follow.is_none() || std::env::var_os("PLAYERCALLS").is_some()) {
                        prints.push(format!("{t:.1}s player {name} {args:?}"));
                    }
                    format!("call:{name}")
                }
                Command::Print { text, .. } => { prints.push(format!("{t:.1}s {text}")); "print".into() }
                Command::Objective { index, state, text, at } => { prints.push(format!("{t:.1}s objective {index} {state} {text} {at:?}")); "objective".into() }
                Command::SpawnAi { classname, origin, .. } => {
                    if classname.contains("enemy") {
                        prints.push(format!("{t:.1}s spawn {classname} at {:?}", origin.map(f32::round)));
                    }
                    format!("spawn:{classname}")
                }
                Command::PlaySound { alias, ent, .. } if std::env::var_os("SOUNDS").is_some() => {
                    prints.push(format!("{t:.1}s sound {alias} on {ent:?}"));
                    "PlaySound".into()
                }
                Command::MissionFailed | Command::MissionSuccess | Command::ChangeLevel(_) => { prints.push(format!("{t:.1}s {c:?}")); format!("{c:?}") }
                Command::Shoot { ai, target: Some(tg), at, accuracy, .. } if follow.is_some() && *tg != PLAYER => {
                    shots.push((*ai, *tg, *at, *accuracy));
                    "Shoot".into()
                }
                other => format!("{other:?}").split([' ', '(', '{']).next().unwrap_or("").to_owned(),
            };
            *kinds.entry(k).or_default() += 1;
        }
        for (ai, tg, at, accuracy) in shots {
            if roll() < (0.18 * accuracy).clamp(0.0, 0.95) {
                let from = level.ent(ai).map(|e| e.origin).unwrap_or(at);
                level.damaged(&mut vm, tg, 20, Some(ai), [at[0] - from[0], at[1] - from[1], at[2] - from[2]], at);
            }
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
    // `ACTORS=1`: each live actor at the end (side, where, its animscript, its enemy).
    if std::env::var_os("ACTORS").is_some() {
        for e in level.ents.iter().filter(|e| e.kind == gsc::level::Kind::Ai) {
            let enemy = vm.entity_obj(e.id).map(|o| vm.field(o, "enemy")).and_then(|v| vm.entity_of(&v));
            let script = level.brains.get(&e.id).map(|b| b.script.clone());
            let anim = e.anim_state.anim.as_ref().map(|a| (a.name.clone(), a.scripted.is_some(), a.ended, e.anim_state.overlay.as_ref().map(|o| o.name.clone())));
            println!("actor {} {} {} alive {} at {:?} goal {:?} script {:?} enemy {:?} anim {anim:?}", e.id, e.classname, e.team, e.alive, e.origin.map(f32::round), e.goal.map(|g| g.map(f32::round)), script, enemy);
        }
    }
    // `VEHICLES=1`: each vehicle at the end (where, how it drives).
    if std::env::var_os("VEHICLES").is_some() {
        for e in level.ents.iter().filter(|e| e.kind == gsc::level::Kind::Vehicle) {
            let f = |k: &str| vm.entity_obj(e.id).map(|o| vm.field(o, k).display()).unwrap_or_default();
            println!("vehicle {} {} at {:?} target {} attachedpath {} type {} {:?}", e.id, e.model, e.origin.map(f32::round), f("target"), f("attachedpath"), f("vehicletype"), level.vehicles.get(&e.id).map(|v| (v.moving, v.speed)));
        }
    }
    // `ENT=<targetname>`: those entities' place and bounds.
    if let Ok(want) = std::env::var("ENT") {
        for e in level.ents.iter().filter(|e| e.get("targetname") == Some(want.as_str()) || e.model == want || e.get("script_flag") == Some(want.as_str())) {
            println!("ent {} {} origin {:?} angles {:?} bounds {:?} enabled {} waited {}", e.id, e.classname, e.origin, e.angles, e.bounds, e.enabled, vm.entity_obj(e.id).is_some_and(|o| vm.waiting_on(o, "trigger")));
        }
    }
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
    if let Ok(filter) = std::env::var("THREADS") {
        for t in vm.thread_report().iter().filter(|t| t.contains(&filter)).take(40) {
            println!("  thread {t}");
        }
    }
    let skip = prints.len().saturating_sub(std::env::var("PRINTS").ok().and_then(|s| s.parse().ok()).unwrap_or(40));
    for p in prints.iter().skip(skip) {
        println!("  {p}");
    }
    Ok(())
}
