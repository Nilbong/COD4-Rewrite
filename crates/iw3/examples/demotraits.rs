//! Everything about how each real player in a CoD4X demo played, for
//! `tools/demo_traits.py` (player traits, bot personas):
//! demotraits <file.dm_1> <out dir>
//!
//! Writes `<out>/<demo>/`:
//! - `meta.txt`: map, gametype, length, the weapon list.
//! - `players.csv`: one row per player (client number, name, team, rank,
//!   prestige, perks, final scoreboard).
//! - `tracks/<name>.csv`: the player at every snapshot they're alive in
//!   (20 Hz): position, view (the recorder's yaw convention, as `demodump`),
//!   stance, firing and ADS flags, shots fired since the last snapshot, the
//!   weapon held, sprinting (last footstep was a sprint step), and which
//!   enemies they have line of sight to (`vis`, client numbers), tested
//!   against the map's collision: world brushes, terrain, props.
//! - `events.csv`: kills (weapon or means), grenades and equipment thrown
//!   (thrower: the nearest player), kill streaks called in, flags captured,
//!   pickups, reloads, melee, the final scoreboard.
//!
//! Server bots (`[BOT]...`, `bot0`...) are left out of tracks (their kills
//! stay in the events, by name).
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

// Entity events (bg_public.h).
const EV_ITEM_PICKUP: u32 = 0x9;
const EV_RELOAD: u32 = 0xF;
const EV_RELOAD_FROM_EMPTY: u32 = 0x10;
const EV_PULLBACK_WEAPON: u32 = 0x19;
const EV_FIRE_WEAPON: u32 = 0x1A;
const EV_FIRE_WEAPON_LASTSHOT: u32 = 0x1B;
const EV_MELEE_SWIPE: u32 = 0x1E;
const EV_DETONATE: u32 = 0x3F;
const EV_FOOTSTEP_SPRINT: u32 = 0x48;
const EV_FOOTSTEP_RUN: u32 = 0x49;
const EV_FOOTSTEP_WALK: u32 = 0x4A;
const EV_FOOTSTEP_PRONE: u32 = 0x4B;
const EV_JUMP: u32 = 0x4C;
/// Event entities are `ET_EVENTS + event`.
const ET_EVENTS: u32 = 17;
const EV_OBITUARY: u32 = 0x42;
const CS_WEAPONFILES: usize = 2258;
/// A sprint lasts from a sprint footstep until a step of another kind, or
/// this long without steps.
const STEP_HOLD_MS: i32 = 700;

fn is_bot(name: &str) -> bool {
    name.starts_with("[BOT]") || name.strip_prefix("bot").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn clean(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// Without colour codes (`^1`) and the localisation markers (0x14, 0x15).
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '^' && it.peek().is_some_and(|d| d.is_ascii_digit()) {
            it.next();
        } else if c != '\u{14}' && c != '\u{15}' && c != '"' {
            out.push(c);
        }
    }
    out.trim().to_string()
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(out)) = (args.next(), args.next()) else {
        anyhow::bail!("usage: demotraits <file.dm_1> <out dir>");
    };
    let t_start = std::time::Instant::now();
    let demo = iw3::demo::read(&std::fs::read(&path)?)?;
    let stem = std::path::Path::new(&path).file_stem().unwrap().to_string_lossy().to_string();
    let dir = std::path::Path::new(&out).join(&stem);
    std::fs::create_dir_all(dir.join("tracks"))?;
    let map = demo.server_info("mapname").unwrap_or("?").to_string();
    let gametype = demo.server_info("g_gametype").unwrap_or("?").to_string();
    let weapons: Vec<String> = demo.config_strings.get(&CS_WEAPONFILES).map_or(vec![], |s| s.split(' ').map(String::from).collect());
    let weapon = |i: u32| weapons.get(i as usize).map_or("?", |s| s.as_str());
    let t0 = demo.snapshots.first().map_or(0, |s| s.server_time);
    let secs = |t: i32| (t - t0) as f64 / 1000.0;

    let world = sight::World::load(&map)?;
    eprintln!("{stem}: {map} {gametype}, collision {} brushes {} tris, read in {:?}", world.brushes.len(), world.tris.len(), t_start.elapsed());

    // Who's who at each snapshot, and where.
    struct P {
        cn: u32,
        name: String,
        team: u32,
        eye: [f32; 3],
        chest: [f32; 3],
    }
    let snaps: Vec<Vec<P>> = demo
        .snapshots
        .iter()
        .map(|s| {
            s.entities
                .iter()
                .filter(|e| e.e_type() == 1)
                .filter_map(|e| {
                    let name = demo.name_at(e.number, s.server_time).unwrap_or("");
                    if name.is_empty() {
                        return None;
                    }
                    let team = s.clients.iter().find(|c| c.number == e.number).map_or(0, |c| c.team());
                    let o = e.origin();
                    let flags = e.int("lerp.eFlags");
                    let (eye, chest) = if flags & 0x8 != 0 { (11.0, 8.0) } else if flags & 0x4 != 0 { (40.0, 30.0) } else { (60.0, 46.0) };
                    Some(P { cn: e.number, name: name.to_string(), team, eye: [o[0], o[1], o[2] + eye], chest: [o[0], o[1], o[2] + chest] })
                })
                .collect()
        })
        .collect();

    // Line of sight between enemies at each snapshot, on a few threads.
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
    let chunk = snaps.len().div_ceil(threads).max(1);
    let vis: Vec<Vec<(u32, u32)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = snaps
            .chunks(chunk)
            .map(|part| {
                let world = &world;
                scope.spawn(move || {
                    let mut ray = sight::Ray::new(world);
                    part.iter()
                        .map(|ps| {
                            let mut seen = Vec::new();
                            for (i, a) in ps.iter().enumerate() {
                                for b in &ps[i + 1..] {
                                    // FFA has everyone on team 0; otherwise enemies only.
                                    if (a.team == b.team && a.team != 0) || a.team == 3 || b.team == 3 {
                                        continue;
                                    }
                                    let d = sight::dist(a.eye, b.eye);
                                    if d > 6000.0 {
                                        continue;
                                    }
                                    if ray.clear(a.eye, b.eye) {
                                        seen.push((a.cn, b.cn));
                                        seen.push((b.cn, a.cn));
                                        continue;
                                    }
                                    if ray.clear(a.eye, b.chest) {
                                        seen.push((a.cn, b.cn));
                                    }
                                    if ray.clear(b.eye, a.chest) {
                                        seen.push((b.cn, a.cn));
                                    }
                                }
                            }
                            seen
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    eprintln!("{stem}: sight done in {:?}", t_start.elapsed());

    // Tracks and per-player events.
    let mut events = String::from("t,kind,a,a_name,b,b_name,weapon,extra,x,y,z\n");
    let mut tracks: BTreeMap<String, String> = BTreeMap::new();
    // Per client: last event sequence seen, last footstep (time, kind).
    let mut seq: HashMap<u32, u32> = HashMap::new();
    let mut step: HashMap<u32, (i32, u32)> = HashMap::new();
    struct Info {
        name: String,
        team: BTreeMap<u32, usize>,
        rank: u32,
        prestige: u32,
        perks: BTreeMap<u32, usize>,
        first: f64,
        last: f64,
    }
    let mut info: BTreeMap<(u32, String), Info> = BTreeMap::new();
    for (si, s) in demo.snapshots.iter().enumerate() {
        let t = secs(s.server_time);
        for c in &s.clients {
            let Some(name) = demo.name_at(c.number, s.server_time) else { continue };
            let i = info.entry((c.number, name.to_string())).or_insert_with(|| Info {
                name: name.to_string(),
                team: Default::default(),
                rank: 0,
                prestige: 0,
                perks: Default::default(),
                first: t,
                last: t,
            });
            *i.team.entry(c.team()).or_default() += 1;
            i.rank = i.rank.max(c.int("rank"));
            i.prestige = i.prestige.max(c.int("prestige"));
            if c.team() == 1 || c.team() == 2 || c.team() == 0 {
                *i.perks.entry(c.int("perks")).or_default() += 1;
            }
            i.last = t;
        }
        let seen = &vis[si];
        for e in s.entities.iter().filter(|e| e.e_type() == 1) {
            let name = demo.name_at(e.number, s.server_time).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let cn = e.number;
            let team = s.clients.iter().find(|c| c.number == cn).map_or(0, |c| c.team());
            let (p, a, flags) = (e.origin(), e.angles(), e.int("lerp.eFlags"));
            // New events since the last snapshot.
            let now_seq = e.int("eventSequence");
            let mut shots = 0;
            let mut jump = 0;
            let last = *seq.get(&cn).unwrap_or(&now_seq);
            let new = (now_seq.wrapping_sub(last) & 0xFF).min(4);
            for k in (0..new).rev() {
                let sq = now_seq.wrapping_sub(k + 1);
                let ev = e.int(&format!("events[{}]", sq & 3));
                let parm = e.int(&format!("eventParms[{}]", sq & 3));
                let w = weapon(e.int("weapon"));
                let mut push = |kind: &str, wpn: &str| {
                    let _ = writeln!(events, "{t:.3},{kind},{cn},{},,,{wpn},,{:.0},{:.0},{:.0}", clean(name), p[0], p[1], p[2]);
                };
                match ev {
                    EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT => shots += 1,
                    EV_FOOTSTEP_SPRINT | EV_FOOTSTEP_RUN | EV_FOOTSTEP_WALK | EV_FOOTSTEP_PRONE => {
                        step.insert(cn, (s.server_time, ev));
                    }
                    EV_JUMP => jump = 1,
                    EV_ITEM_PICKUP => push("pickup", weapon(parm)),
                    EV_RELOAD | EV_RELOAD_FROM_EMPTY => push(if ev == EV_RELOAD { "reload" } else { "reload_empty" }, w),
                    EV_PULLBACK_WEAPON => push("cook", w),
                    EV_MELEE_SWIPE => push("melee", w),
                    EV_DETONATE => push("detonate", w),
                    _ => {}
                }
            }
            seq.insert(cn, now_seq);
            if is_bot(name) {
                continue;
            }
            let sprint = step.get(&cn).is_some_and(|&(st, kind)| kind == EV_FOOTSTEP_SPRINT && s.server_time - st <= STEP_HOLD_MS);
            let stance = if flags & 0x8 != 0 { 2 } else { (flags & 0x4 != 0) as u32 };
            let ads = flags & 0x40000 != 0;
            let vis_list: Vec<String> = seen.iter().filter(|v| v.0 == cn).map(|v| v.1.to_string()).collect();
            let rows = tracks
                .entry(clean(name))
                .or_insert_with(|| "t,cn,yaw,pitch,fire,shots,ads,x,y,z,stance,team,weapon,sprint,jump,vis\n".into());
            let _ = writeln!(
                rows,
                "{t:.3},{cn},{:.2},{:.2},{},{shots},{},{:.1},{:.1},{:.1},{stance},{team},{},{},{jump},{}",
                a[1] - 90.0,
                -a[0],
                (flags & 0x40 != 0) as u8,
                ads as u8,
                p[0],
                p[1],
                p[2],
                weapon(e.int("weapon")),
                sprint as u8,
                vis_list.join(";"),
            );
        }
    }

    // Kills: an obituary entity lasts a few snapshots; once each.
    let mut last_obit: HashMap<(u32, u32, u32), i32> = HashMap::new();
    // Thrown things: a missile entity's first snapshot (thrower: nearest).
    let mut live: HashMap<u32, u32> = HashMap::new();
    let explosive = |w: &str| ["grenade", "claymore", "c4", "rpg", "gl_"].iter().any(|k| w.contains(k));
    for (si, s) in demo.snapshots.iter().enumerate() {
        let t = secs(s.server_time);
        let mut now_live = HashMap::new();
        for e in &s.entities {
            let ty = e.e_type();
            if ty == ET_EVENTS + EV_OBITUARY {
                let (victim, attacker, parm) = (e.int("otherEntityNum"), e.int("attackerEntityNum"), e.int("eventParm"));
                let key = (victim, attacker, parm);
                if last_obit.get(&key).is_some_and(|&lt| s.server_time - lt < 1500) {
                    last_obit.insert(key, s.server_time);
                    continue;
                }
                last_obit.insert(key, s.server_time);
                let vn = demo.name_at(victim, s.server_time).unwrap_or("");
                let an = if attacker < 64 { demo.name_at(attacker, s.server_time).unwrap_or("") } else { "" };
                // 0x80 marks a means of death (8 = headshot) instead of a weapon.
                let (wpn, means) = if parm & 0x80 != 0 { (String::new(), parm & 0x7F) } else { (weapon(parm).to_string(), 0) };
                // Where the victim was (their last snapshot).
                let at = demo.snapshots[..=si]
                    .iter()
                    .rev()
                    .take(10)
                    .find_map(|s| s.entities.iter().find(|e| e.number == victim && e.e_type() == 1).map(|e| e.origin()))
                    .unwrap_or([0.0; 3]);
                let _ = writeln!(
                    events,
                    "{t:.3},kill,{attacker},{},{victim},{},{wpn},{means},{:.0},{:.0},{:.0}",
                    clean(an),
                    clean(vn),
                    at[0],
                    at[1],
                    at[2]
                );
            } else if ty == 4 {
                let w = weapon(e.int("weapon"));
                if !explosive(w) {
                    continue;
                }
                now_live.insert(e.number, e.int("weapon"));
                if live.get(&e.number) == Some(&e.int("weapon")) {
                    continue;
                }
                let o = e.origin();
                let thrower = snaps[si]
                    .iter()
                    .map(|p| (sight::dist(p.eye, o), p))
                    .filter(|(d, _)| *d < 250.0)
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, p)| p);
                let kind = if w.contains("claymore") || w.starts_with("c4") { "equip" } else { "throw" };
                let _ = writeln!(
                    events,
                    "{t:.3},{kind},{},{},,,{w},,{:.0},{:.0},{:.0}",
                    thrower.map_or(String::new(), |p| p.cn.to_string()),
                    thrower.map_or(String::new(), |p| clean(&p.name)),
                    o[0],
                    o[1],
                    o[2]
                );
            }
        }
        live = now_live;
    }

    // Server messages: streaks, flags, scoreboard. Each comes a few times.
    let mut last_cmd: HashMap<String, i32> = HashMap::new();
    let mut scoreboard: Option<String> = None;
    for (time, c) in &demo.commands {
        if c.starts_with("b ") {
            scoreboard = Some(c.clone());
            continue;
        }
        if !c.starts_with("f ") {
            continue;
        }
        if last_cmd.get(c).is_some_and(|&lt| time - lt < 2000) {
            continue;
        }
        last_cmd.insert(c.clone(), *time);
        let t = secs(*time);
        let text = plain(&c[2..]);
        let (kind, a, extra) = if let Some((what, who)) = text.split_once(" called in by ").or_else(|| text.split_once(" placed by ")) {
            ("streak", who.to_string(), what.to_string())
        } else if let Some((key, who)) = text.split_once("_CAPTURED_BY") {
            ("capture", who.to_string(), key.to_string())
        } else if let Some((_, who)) = text.split_once("RANK_KILL_STREAK_N") {
            let mut parts = who.splitn(2, |c: char| c.is_whitespace());
            ("killstreak_msg", parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string())
        } else if text.contains(" destroyed ") {
            let (what, who) = text.split_once(" destroyed ").unwrap();
            ("destroyed", who.to_string(), what.to_string())
        } else if let (Some(l), Some(r)) = (text.find(" ["), text.find("] ")) {
            // `attacker [STREAK] victim`: a streak kill.
            if l < r {
                let _ = writeln!(events, "{t:.3},streak_kill,,{},,{},{},,,,", clean(&text[..l]), clean(&text[r + 2..]), &text[l + 2..r]);
            }
            continue;
        } else {
            continue;
        };
        let _ = writeln!(events, "{t:.3},{kind},,{},,,,{},,,", clean(&a), extra.replace(',', " "));
    }
    let mut finals: HashMap<u32, String> = HashMap::new();
    if let Some(b) = &scoreboard {
        let v: Vec<i64> = b.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
        let n = v.first().copied().unwrap_or(0) as usize;
        for i in 0..n {
            let base = 4 + 7 * i;
            if base + 6 < v.len() {
                let (client, score, ping, deaths, kills, assists) = (v[base], v[base + 1], v[base + 2], v[base + 3], v[base + 5], v[base + 6]);
                finals.insert(client as u32, format!("{score},{deaths},{kills},{assists},{ping}"));
            }
        }
    }

    let mut players = String::from("cn,name,raw_name,team,rank,prestige,perks,first,last,score,deaths,kills,assists,ping,bot\n");
    // The latest name each client had gets the scoreboard line.
    let latest: HashMap<u32, String> = info.keys().fold(HashMap::new(), |mut m, (cn, n)| {
        let keep = info[&(*cn, n.clone())].last;
        if m.get(cn).is_none_or(|old: &String| info[&(*cn, old.clone())].last < keep) {
            m.insert(*cn, n.clone());
        }
        m
    });
    for ((cn, n), i) in &info {
        let team = i.team.iter().filter(|(t, _)| **t != 3).max_by_key(|(_, k)| **k).or_else(|| i.team.iter().max_by_key(|(_, k)| **k)).map_or(0, |(t, _)| *t);
        let perks = i.perks.iter().max_by_key(|(_, k)| **k).map_or(0, |(p, _)| *p);
        let fin = if latest.get(cn) == Some(n) { finals.get(cn).cloned() } else { None }.unwrap_or_else(|| ",,,,".into());
        let _ = writeln!(
            players,
            "{cn},{},{:?},{team},{},{},{perks},{:.1},{:.1},{fin},{}",
            clean(&i.name),
            i.name.replace('"', "'"),
            i.rank,
            i.prestige,
            i.first,
            i.last,
            is_bot(&i.name) as u8
        );
    }
    let length = demo.snapshots.last().map_or(0.0, |s| secs(s.server_time));
    std::fs::write(
        dir.join("meta.txt"),
        format!("map={map}\ngametype={gametype}\nlength={length:.1}\nrecorder={}\nweapons={}\n", demo.client_num, weapons.join(" ")),
    )?;
    std::fs::write(dir.join("players.csv"), players)?;
    std::fs::write(dir.join("events.csv"), events)?;
    for (name, rows) in &tracks {
        std::fs::write(dir.join("tracks").join(format!("{name}.csv")), rows)?;
    }
    println!("{stem}: {map} {gametype}, {} players tracked, {length:.0} s, done in {:?}", tracks.len(), t_start.elapsed());
    Ok(())
}

/// Line of sight through the map's collision, as players see: solid and
/// sight-blocking brushes, terrain, and the props' collision.
mod sight {
    use iw3::zone::{ClipMap, Zone};

    const SOLID: i32 = 0x1;
    const FOLIAGE: i32 = 0x2;
    const AI_NOSIGHT: i32 = 0x1000;
    const CLIPSHOT: i32 = 0x2000;
    const CELL: f32 = 128.0;

    pub fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    pub struct World {
        /// Each brush's bounding planes (normal, dist): inside where n.p <= d.
        pub brushes: Vec<(Vec<([f32; 3], f32)>, [f32; 3], [f32; 3])>,
        pub tris: Vec<[[f32; 3]; 3]>,
        /// 2D grid over the play area: items (brush i, or tri brushes.len()+i).
        min: [f32; 2],
        dims: [usize; 2],
        cells: Vec<Vec<u32>>,
    }

    impl World {
        pub fn load(map: &str) -> anyhow::Result<World> {
            let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(map))?;
            let zone = Zone::parse(&data, iw3::zone::ParseOptions::default())?;
            let clip = zone.clip_map().ok_or_else(|| anyhow::anyhow!("{map}: no clip map"))?;
            let mut w = World { brushes: Vec::new(), tris: Vec::new(), min: [0.0; 2], dims: [0; 2], cells: Vec::new() };
            let entity = clip.entity_brushes();
            for (i, b) in clip.brushes.iter().enumerate() {
                if entity.contains(&(i as u32)) || b.contents & (SOLID | CLIPSHOT | FOLIAGE | AI_NOSIGHT) == 0 {
                    continue;
                }
                let mut planes = vec![
                    ([1.0, 0.0, 0.0], b.maxs[0]),
                    ([-1.0, 0.0, 0.0], -b.mins[0]),
                    ([0.0, 1.0, 0.0], b.maxs[1]),
                    ([0.0, -1.0, 0.0], -b.mins[1]),
                    ([0.0, 0.0, 1.0], b.maxs[2]),
                    ([0.0, 0.0, -1.0], -b.mins[2]),
                ];
                planes.extend(b.side_planes.iter().filter_map(|&p| clip.planes.get(p as usize)).map(|p| (p.normal, p.dist)));
                w.brushes.push((planes, b.mins, b.maxs));
            }
            let entity_tris: std::collections::HashSet<usize> = (1..clip.cmodels.len()).flat_map(|n| cmodel_tris(clip, n)).collect();
            for (i, t) in clip.tri_indices.chunks_exact(3).enumerate() {
                if entity_tris.contains(&i) || !t.iter().all(|&v| (v as usize) < clip.verts.len()) {
                    continue;
                }
                w.tris.push([clip.verts[t[0] as usize], clip.verts[t[1] as usize], clip.verts[t[2] as usize]]);
            }
            for sm in &clip.static_models {
                let Some(model) = sm.model.and_then(|id| zone.xmodel(id)) else { continue };
                let rows = sm.inv_scaled_axis;
                let len = (rows[0][0].powi(2) + rows[0][1].powi(2) + rows[0][2].powi(2)).sqrt();
                if !(len > 1e-6) {
                    continue;
                }
                let s2 = 1.0 / (len * len);
                let axes = rows.map(|r| r.map(|v| v * s2));
                let place = |v: [f32; 3]| -> [f32; 3] { std::array::from_fn(|k| sm.origin[k] + axes[0][k] * v[0] + axes[1][k] * v[1] + axes[2][k] * v[2]) };
                for b in &model.coll_boxes {
                    let c = if b.contents != 0 { b.contents } else { model.contents };
                    if c & (SOLID | FOLIAGE) == 0 {
                        continue;
                    }
                    if !b.tris.is_empty() {
                        w.tris.extend(b.tris.iter().map(|t| t.map(place)));
                    } else {
                        // A box: its 12 triangles.
                        let (lo, hi) = (b.mins, b.maxs);
                        let corner = |i: usize| place([if i & 1 != 0 { hi[0] } else { lo[0] }, if i & 2 != 0 { hi[1] } else { lo[1] }, if i & 4 != 0 { hi[2] } else { lo[2] }]);
                        for f in [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]] {
                            w.tris.push([corner(f[0]), corner(f[1]), corner(f[2])]);
                            w.tris.push([corner(f[0]), corner(f[2]), corner(f[3])]);
                        }
                    }
                }
            }
            // The grid covers the world model's bounds.
            let (lo, hi) = clip.cmodels.first().map_or(([-8000.0; 3], [8000.0; 3]), |m| (m.mins, m.maxs));
            let lo = [lo[0].max(-20000.0) - CELL, lo[1].max(-20000.0) - CELL];
            let hi = [hi[0].min(20000.0) + CELL, hi[1].min(20000.0) + CELL];
            w.min = lo;
            w.dims = [((hi[0] - lo[0]) / CELL).ceil() as usize + 1, ((hi[1] - lo[1]) / CELL).ceil() as usize + 1];
            w.cells = vec![Vec::new(); w.dims[0] * w.dims[1]];
            let n_brushes = w.brushes.len();
            let boxes: Vec<([f32; 3], [f32; 3])> = w
                .brushes
                .iter()
                .map(|b| (b.1, b.2))
                .chain(w.tris.iter().map(|t| (std::array::from_fn(|k| t[0][k].min(t[1][k]).min(t[2][k])), std::array::from_fn(|k| t[0][k].max(t[1][k]).max(t[2][k])))))
                .collect();
            for (i, (bmin, bmax)) in boxes.iter().enumerate() {
                let (x0, y0) = w.cell_of(bmin[0], bmin[1]);
                let (x1, y1) = w.cell_of(bmax[0], bmax[1]);
                // Skip the sky box's walls (huge, round the edge).
                if i < n_brushes && (x1 - x0) * (y1 - y0) > 40000 {
                    continue;
                }
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        w.cells[y * w.dims[0] + x].push(i as u32);
                    }
                }
            }
            Ok(w)
        }

        fn cell_of(&self, x: f32, y: f32) -> (usize, usize) {
            let cx = ((x - self.min[0]) / CELL).floor().clamp(0.0, (self.dims[0] - 1) as f32) as usize;
            let cy = ((y - self.min[1]) / CELL).floor().clamp(0.0, (self.dims[1] - 1) as f32) as usize;
            (cx, cy)
        }
    }

    pub struct Ray<'a> {
        world: &'a World,
        stamp: Vec<u32>,
        now: u32,
    }

    impl<'a> Ray<'a> {
        pub fn new(world: &'a World) -> Self {
            Ray { world, stamp: vec![0; world.brushes.len() + world.tris.len()], now: 0 }
        }

        /// Nothing in the way from `a` to `b`.
        pub fn clear(&mut self, a: [f32; 3], b: [f32; 3]) -> bool {
            self.now = self.now.wrapping_add(1);
            if self.now == 0 {
                self.stamp.iter_mut().for_each(|s| *s = 0);
                self.now = 1;
            }
            let w = self.world;
            let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            // Walk the grid cells the segment crosses (2D DDA).
            let (mut cx, mut cy) = w.cell_of(a[0], a[1]);
            let (ex, ey) = w.cell_of(b[0], b[1]);
            let step_x: i64 = if d[0] > 0.0 { 1 } else { -1 };
            let step_y: i64 = if d[1] > 0.0 { 1 } else { -1 };
            let bx = w.min[0] + (cx as f32 + if step_x > 0 { 1.0 } else { 0.0 }) * CELL;
            let by = w.min[1] + (cy as f32 + if step_y > 0 { 1.0 } else { 0.0 }) * CELL;
            let mut t_max_x = if d[0].abs() > 1e-6 { (bx - a[0]) / d[0] } else { f32::INFINITY };
            let mut t_max_y = if d[1].abs() > 1e-6 { (by - a[1]) / d[1] } else { f32::INFINITY };
            let t_dx = if d[0].abs() > 1e-6 { CELL / d[0].abs() } else { f32::INFINITY };
            let t_dy = if d[1].abs() > 1e-6 { CELL / d[1].abs() } else { f32::INFINITY };
            let n_brushes = w.brushes.len() as u32;
            for _ in 0..4096 {
                for &item in &w.cells[cy * w.dims[0] + cx] {
                    let s = &mut self.stamp[item as usize];
                    if *s == self.now {
                        continue;
                    }
                    *s = self.now;
                    let hit = if item < n_brushes { hits_brush(&w.brushes[item as usize], a, d) } else { hits_tri(&w.tris[(item - n_brushes) as usize], a, d) };
                    if hit {
                        return false;
                    }
                }
                if cx == ex && cy == ey {
                    break;
                }
                if t_max_x < t_max_y {
                    if t_max_x > 1.0 {
                        break;
                    }
                    cx = (cx as i64 + step_x).clamp(0, w.dims[0] as i64 - 1) as usize;
                    t_max_x += t_dx;
                } else {
                    if t_max_y > 1.0 {
                        break;
                    }
                    cy = (cy as i64 + step_y).clamp(0, w.dims[1] as i64 - 1) as usize;
                    t_max_y += t_dy;
                }
            }
            true
        }
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn hits_brush(b: &(Vec<([f32; 3], f32)>, [f32; 3], [f32; 3]), a: [f32; 3], d: [f32; 3]) -> bool {
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        for &(n, dist) in &b.0 {
            let s = dot(n, a) - dist;
            let e = s + dot(n, d);
            if s > 0.0 && e > 0.0 {
                return false;
            }
            if s <= 0.0 && e <= 0.0 {
                continue;
            }
            let t = s / (s - e);
            if s > 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return false;
            }
        }
        // Grazing a brush for under a unit doesn't block a whole body.
        (t1 - t0) * dot(d, d).sqrt() > 1.0
    }

    fn hits_tri(t: &[[f32; 3]; 3], a: [f32; 3], d: [f32; 3]) -> bool {
        let sub = |p: [f32; 3], q: [f32; 3]| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
        let cross = |p: [f32; 3], q: [f32; 3]| [p[1] * q[2] - p[2] * q[1], p[2] * q[0] - p[0] * q[2], p[0] * q[1] - p[1] * q[0]];
        let e1 = sub(t[1], t[0]);
        let e2 = sub(t[2], t[0]);
        let p = cross(d, e2);
        let det = dot(e1, p);
        if det.abs() < 1e-8 {
            return false;
        }
        let inv = 1.0 / det;
        let s = sub(a, t[0]);
        let u = dot(s, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return false;
        }
        let q = cross(s, e1);
        let v = dot(d, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return false;
        }
        let tt = dot(e2, q) * inv;
        tt > 1e-3 && tt < 0.999
    }

    fn cmodel_tris(clip: &ClipMap, n: usize) -> Vec<usize> {
        let Some(m) = clip.cmodels.get(n) else { return Vec::new() };
        let mut out = Vec::new();
        let first = m.first_coll_aabb as usize;
        let mut stack: Vec<(usize, u32)> = (first..first + m.coll_aabb_count as usize).map(|i| (i, 0)).collect();
        while let Some((i, depth)) = stack.pop() {
            let Some(node) = clip.aabb_trees.get(i) else { continue };
            if node.child_count > 0 {
                if depth < 64 {
                    let c = node.index.max(0) as usize;
                    stack.extend((c..c + node.child_count as usize).map(|j| (j, depth + 1)));
                }
            } else if let Some(p) = usize::try_from(node.index).ok().and_then(|p| clip.partitions.get(p)) {
                let f = p.first_tri.max(0) as usize;
                out.extend(f..f + p.tri_count as usize);
            }
        }
        out
    }
}
