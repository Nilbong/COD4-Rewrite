//! A level as its scripts see it, without the game: the map's entities
//! (from its entity string), path nodes, structs and spawners; the player
//! and AI the game reports; triggers, movers, flags of the engine's kind,
//! objectives and HUD elements. It provides the builtins that only need
//! that ([`Host`]); what has to happen in the game (spawning AI, sounds,
//! effects, the player's weapons...) it queues as [`Command`]s, and the
//! game feeds back what happened (positions, touches, damage, deaths).

use crate::vm::{Host, ObjId, Value, Vm};
use std::collections::HashMap;
use std::rc::Rc;

pub type EntId = u64;

/// The player's entity id.
pub const PLAYER: EntId = 1 << 32;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A map entity (script_origin, script_model, script_brushmodel,
    /// trigger_*, info_*...).
    Map,
    /// An actor spawner (`actor_*`), with spawns left.
    Spawner,
    Player,
    Ai,
    /// Made by `spawn()`.
    Spawned,
    /// `script_vehicle`.
    Vehicle,
}

#[derive(Clone, Debug)]
pub struct Mover {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub start: f64,
    pub time: f64,
    /// Rotation: (from, to) angles, or a constant rate.
    pub rotate: bool,
    pub velocity: Option<[f32; 3]>,
}

#[derive(Clone, Debug)]
pub struct Ent {
    pub id: EntId,
    pub kind: Kind,
    pub classname: String,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub model: String,
    pub hidden: bool,
    pub solid: bool,
    /// Brush model bounds (local), for triggers and `istouching`.
    pub bounds: Option<([f32; 3], [f32; 3])>,
    /// `trigger_radius`: radius and height.
    pub radius: Option<(f32, f32)>,
    pub spawnflags: i32,
    pub team: String,
    pub health: i32,
    pub max_health: i32,
    pub alive: bool,
    pub linked: Option<(EntId, [f32; 3], [f32; 3])>,
    pub move_pos: Option<Mover>,
    pub move_rot: Option<Mover>,
    /// Triggers: on (the script's `trigger_off` moves them away instead,
    /// but `triggerenable` exists too), and the hint for use triggers.
    pub enabled: bool,
    pub hint: String,
    /// The player was in it last frame (a `trigger_once`'s fired).
    pub touching: bool,
    pub fired: bool,
    /// AI: its goal (`setgoalnode` / `setgoalpos` / `setgoalentity` /
    /// `setgoalvolume`) as a point, and goal radius.
    pub goal: Option<[f32; 3]>,
    pub kv: Vec<(String, String)>,
}

impl Ent {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.kv.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    pub fn is_trigger(&self) -> bool {
        self.classname.starts_with("trigger_")
    }

    /// Is a point inside (triggers: their brush or radius).
    pub fn contains(&self, p: [f32; 3]) -> bool {
        if let Some((r, h)) = self.radius {
            let d = ((p[0] - self.origin[0]).powi(2) + (p[1] - self.origin[1]).powi(2)).sqrt();
            return d <= r && p[2] >= self.origin[2] && p[2] <= self.origin[2] + h;
        }
        if let Some((mn, mx)) = self.bounds {
            return (0..3).all(|i| p[i] >= mn[i] + self.origin[i] && p[i] <= mx[i] + self.origin[i]);
        }
        false
    }
}

/// What the scripts asked of the game, in order.
#[derive(Clone, Debug)]
pub enum Command {
    /// Spawn an AI from a spawner (the id it will have).
    SpawnAi { spawner: EntId, ai: EntId, origin: [f32; 3], angles: [f32; 3], team: String, classname: String, model: String, force: bool },
    Delete(EntId),
    SetModel(EntId, String),
    Show(EntId, bool),
    Solid(EntId, bool),
    PlaySound { ent: Option<EntId>, alias: String, at: [f32; 3], looped: bool },
    StopSound(EntId),
    PlayFx { fx: String, at: [f32; 3], forward: [f32; 3] },
    PlayFxOnTag { fx: String, ent: EntId, tag: String },
    Earthquake { scale: f32, time: f32, at: [f32; 3], radius: f32 },
    Print { text: String, bold: bool },
    Objective { index: i32, state: String, text: String, at: Option<[f32; 3]> },
    ObjectiveCurrent(i32),
    MissionFailed,
    MissionSuccess,
    ChangeLevel(String),
    Fog { near: f32, half: f32, color: [f32; 3], time: f32 },
    Vision { name: String, time: f32 },
    Music(Option<String>),
    /// AI and player methods the game carries out (name, entity, args as
    /// text/numbers): `setgoalpos`, `giveweapon`, `allowprone`...
    Call { ent: EntId, name: String, args: Vec<Value> },
}

pub struct Level {
    pub ents: Vec<Ent>,
    by_id: HashMap<EntId, usize>,
    /// Path nodes, as script objects (made at [`Level::install`]).
    nodes: Vec<(ObjId, String, HashMap<String, String>)>,
    next_id: EntId,
    pub commands: Vec<Command>,
    pub dvars: HashMap<String, String>,
    pub fx: Vec<String>,
    /// AI ids and their teams, as the game spawned them.
    pub time: f64,
    /// Animation lengths, if the game knows them (name → seconds).
    pub anim_length: Box<dyn Fn(&str) -> Option<f32>>,
    pub hud: HashMap<ObjId, HudElem>,
    pub objectives: HashMap<i32, (String, String, Option<[f32; 3]>)>,
    pub missing: HashMap<String, u32>,
}

#[derive(Clone, Debug, Default)]
pub struct HudElem {
    pub text: String,
    pub alpha: f32,
}

/// Keys whose values stay strings (others become numbers where they look
/// like them).
const STRING_KEYS: [&str; 12] =
    ["classname", "targetname", "target", "script_noteworthy", "script_linkname", "script_linkto", "model", "script_flag", "script_parameters", "script_animation", "script_noteworthy2", "script_vehicleride"];

impl Level {
    /// From the map's entity string; `brush_bounds[n]` is brush model
    /// `*n`'s bounds.
    pub fn new(entity_string: &[Vec<(String, String)>], brush_bounds: &[([f32; 3], [f32; 3])]) -> Level {
        let mut level = Level {
            ents: Vec::new(),
            by_id: HashMap::new(),
            nodes: Vec::new(),
            next_id: 1,
            commands: Vec::new(),
            dvars: HashMap::new(),
            fx: Vec::new(),
            time: 0.0,
            anim_length: Box::new(|_| None),
            hud: HashMap::new(),
            objectives: HashMap::new(),
            missing: HashMap::new(),
        };
        for kv in entity_string {
            let get = |k: &str| kv.iter().find(|(a, _)| a.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
            let classname = get("classname").unwrap_or_default().to_ascii_lowercase();
            if classname.starts_with("node_") || classname == "script_struct" || classname == "worldspawn" || classname == "light" {
                continue;
            }
            let model = get("model").unwrap_or_default().to_owned();
            let bounds = model.strip_prefix('*').and_then(|n| n.parse::<usize>().ok()).and_then(|n| brush_bounds.get(n).copied());
            let radius = (classname == "trigger_radius").then(|| (num(get("radius")).unwrap_or(0.0), num(get("height")).unwrap_or(0.0)));
            let kind = if classname.starts_with("actor_") {
                Kind::Spawner
            } else if classname == "script_vehicle" {
                Kind::Vehicle
            } else {
                Kind::Map
            };
            let id = level.next_id;
            level.next_id += 1;
            level.by_id.insert(id, level.ents.len());
            level.ents.push(Ent {
                id,
                kind,
                classname,
                origin: vec3(get("origin")).unwrap_or_default(),
                angles: vec3(get("angles")).unwrap_or_default(),
                model,
                hidden: false,
                solid: true,
                bounds,
                radius,
                spawnflags: num(get("spawnflags")).unwrap_or(0.0) as i32,
                team: String::new(),
                health: 100,
                max_health: 100,
                alive: false,
                linked: None,
                move_pos: None,
                move_rot: None,
                enabled: true,
                hint: String::new(),
                touching: false,
                fired: false,
                goal: None,
                kv: kv.clone(),
            });
        }
        level.next_id = 100_000;
        level
    }

    /// Make the map's script objects in `vm`: each entity's fields, the
    /// structs into `level.struct`, the player.
    pub fn install(&mut self, vm: &mut Vm, entity_string: &[Vec<(String, String)>]) {
        for e in &self.ents {
            let o = vm.entity(e.id);
            let o = o.as_obj().unwrap_or(vm.level);
            for (k, v) in &e.kv {
                let k = k.to_ascii_lowercase();
                if matches!(k.as_str(), "origin" | "angles") {
                    continue;
                }
                vm.set_field(o, &k, field_value(&k, v));
            }
        }
        let mut structs = Vec::new();
        for kv in entity_string {
            let class = kv.iter().find(|(k, _)| k == "classname").map(|(_, v)| v.to_ascii_lowercase()).unwrap_or_default();
            if class != "script_struct" && !class.starts_with("node_") {
                continue;
            }
            let o = vm.alloc(crate::vm::ObjKind::Struct);
            for (k, v) in kv {
                let k = k.to_ascii_lowercase();
                vm.set_field(o, &k, field_value(&k, v));
            }
            if class.starts_with("node_") {
                let ty = node_type(&class);
                vm.set_field(o, "type", Value::str(ty));
                let fields: HashMap<String, String> = kv.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
                self.nodes.push((o, class, fields));
            } else {
                structs.push(Value::Object(o));
            }
        }
        let level = vm.level;
        vm.set_field(level, "struct", Value::array(structs));
        let player = vm.entity(PLAYER);
        if let Some(p) = player.as_obj() {
            vm.set_field(p, "classname", Value::str("player"));
        }
        self.by_id.insert(PLAYER, self.ents.len());
        self.ents.push(Ent {
            id: PLAYER,
            kind: Kind::Player,
            classname: "player".into(),
            origin: [0.0; 3],
            angles: [0.0; 3],
            model: String::new(),
            hidden: false,
            solid: true,
            bounds: Some(([-15.0, -15.0, 0.0], [15.0, 15.0, 70.0])),
            radius: None,
            spawnflags: 0,
            team: "allies".into(),
            health: 100,
            max_health: 100,
            alive: true,
            linked: None,
            move_pos: None,
            move_rot: None,
            enabled: true,
            hint: String::new(),
            touching: false,
            fired: false,
            goal: None,
            kv: Vec::new(),
        });
    }

    pub fn ent(&self, id: EntId) -> Option<&Ent> {
        self.by_id.get(&id).map(|&i| &self.ents[i])
    }

    pub fn ent_mut(&mut self, id: EntId) -> Option<&mut Ent> {
        self.by_id.get(&id).map(|&i| &mut self.ents[i])
    }

    /// A new entity (an AI from a spawner, or `spawn()`'s).
    pub fn add(&mut self, mut e: Ent) -> EntId {
        e.id = self.next_id;
        self.next_id += 1;
        self.by_id.insert(e.id, self.ents.len());
        let id = e.id;
        self.ents.push(e);
        id
    }

    fn remove(&mut self, vm: &mut Vm, id: EntId) {
        if id == PLAYER {
            return;
        }
        if let Some(i) = self.by_id.remove(&id) {
            self.ents.swap_remove(i);
            if let Some(moved) = self.ents.get(i) {
                self.by_id.insert(moved.id, i);
            }
        }
        vm.free_entity(id);
        self.commands.push(Command::Delete(id));
    }

    /// Advance movers and links to `now`; the player's position and the
    /// AI's come from the game ([`Level::set_pose`]) before this.
    pub fn update(&mut self, vm: &mut Vm, now: f64) {
        self.time = now;
        let mut done = Vec::new();
        for e in &mut self.ents {
            if let Some(m) = &e.move_pos {
                let t = if m.time <= 0.0 { 1.0 } else { ((now - m.start) / m.time).clamp(0.0, 1.0) as f32 };
                e.origin = lerp(m.from, m.to, t);
                if t >= 1.0 {
                    e.move_pos = None;
                    done.push((e.id, "movedone"));
                }
            }
            if let Some(m) = &e.move_rot {
                if let Some(v) = m.velocity {
                    let dt = (now - m.start) as f32;
                    e.angles = [m.from[0] + v[0] * dt, m.from[1] + v[1] * dt, m.from[2] + v[2] * dt];
                    if m.time > 0.0 && now - m.start >= m.time {
                        e.move_rot = None;
                        done.push((e.id, "rotatedone"));
                    }
                } else {
                    let t = if m.time <= 0.0 { 1.0 } else { ((now - m.start) / m.time).clamp(0.0, 1.0) as f32 };
                    e.angles = lerp(m.from, m.to, t);
                    if t >= 1.0 {
                        e.move_rot = None;
                        done.push((e.id, "rotatedone"));
                    }
                }
            }
        }
        // Linked entities follow (one level deep is all the scripts use).
        let poses: HashMap<EntId, ([f32; 3], [f32; 3])> = self.ents.iter().map(|e| (e.id, (e.origin, e.angles))).collect();
        for e in &mut self.ents {
            if let Some((to, offset, angles)) = e.linked
                && let Some((o, a)) = poses.get(&to)
            {
                let (f, r, u) = crate::vm::angle_vectors(*a);
                e.origin = [
                    o[0] + f[0] * offset[0] - r[0] * offset[1] + u[0] * offset[2],
                    o[1] + f[1] * offset[0] - r[1] * offset[1] + u[1] * offset[2],
                    o[2] + f[2] * offset[0] - r[2] * offset[1] + u[2] * offset[2],
                ];
                e.angles = [a[0] + angles[0], a[1] + angles[1], a[2] + angles[2]];
            }
        }
        for (id, ev) in done {
            if let Some(o) = vm.entity(id).as_obj() {
                vm.notify(o, ev, Vec::new());
            }
        }
        // Triggers the player is in.
        let player = self.ent(PLAYER).map(|p| p.origin).unwrap_or_default();
        let mut fired = Vec::new();
        for e in &mut self.ents {
            if !e.is_trigger() || !e.enabled || matches!(e.classname.as_str(), "trigger_use" | "trigger_damage" | "trigger_hurt") {
                continue;
            }
            let inside = e.contains(player) || e.contains([player[0], player[1], player[2] + 40.0]);
            if inside && !(e.classname == "trigger_once" && e.fired) {
                fired.push(e.id);
                e.fired = true;
            }
            e.touching = inside;
        }
        let me = vm.entity(PLAYER);
        for id in fired {
            if let Some(o) = vm.entity(id).as_obj() {
                vm.notify(o, "trigger", vec![me.clone()]);
            }
        }
    }

    /// The game's word on where an entity (the player, an AI) is.
    pub fn set_pose(&mut self, id: EntId, origin: [f32; 3], angles: [f32; 3]) {
        if let Some(e) = self.ent_mut(id) {
            e.origin = origin;
            e.angles = angles;
        }
    }

    /// The player pressed Use: the use trigger they're in fires.
    pub fn use_pressed(&mut self, vm: &mut Vm) {
        let player = self.ent(PLAYER).map(|p| p.origin).unwrap_or_default();
        let hit: Vec<EntId> = self
            .ents
            .iter()
            .filter(|e| e.classname == "trigger_use" && e.enabled && (e.contains(player) || e.contains([player[0], player[1], player[2] + 40.0])))
            .map(|e| e.id)
            .collect();
        let me = vm.entity(PLAYER);
        for id in hit {
            if let Some(o) = vm.entity(id).as_obj() {
                vm.notify(o, "trigger", vec![me.clone()]);
            }
        }
    }

    /// The use trigger the player is in, and its hint.
    pub fn use_hint(&self) -> Option<&str> {
        let player = self.ent(PLAYER)?.origin;
        self.ents
            .iter()
            .find(|e| e.classname == "trigger_use" && e.enabled && (e.contains(player) || e.contains([player[0], player[1], player[2] + 40.0])))
            .map(|e| e.hint.as_str())
    }

    /// An entity was hurt (`damage` to its script, and `death` when dead).
    pub fn damaged(&mut self, vm: &mut Vm, id: EntId, amount: i32, attacker: Option<EntId>, dir: [f32; 3], at: [f32; 3]) {
        let Some(e) = self.ent_mut(id) else { return };
        e.health -= amount;
        let dead = e.health <= 0 && e.alive;
        if dead {
            e.alive = false;
        }
        let attacker = attacker.map_or(Value::Undefined, |a| vm.entity(a));
        if let Some(o) = vm.entity(id).as_obj() {
            vm.notify(o, "damage", vec![Value::Int(amount), attacker.clone(), Value::Vector(dir), Value::Vector(at), Value::str("MOD_RIFLE_BULLET")]);
            if dead {
                vm.notify(o, "death", vec![attacker]);
            }
        }
    }

    fn entities_value<'a>(&self, vm: &mut Vm, it: impl Iterator<Item = &'a Ent>) -> Value {
        let ids: Vec<EntId> = it.map(|e| e.id).collect();
        Value::array(ids.into_iter().map(|id| vm.entity(id)))
    }

    /// Entities whose `key` is `value` (`getent`'s): `targetname`,
    /// `classname`, `script_noteworthy`, `target`, `script_linkname`...
    fn matching(&self, vm: &Vm, value: &str, key: &str) -> Vec<EntId> {
        let key = key.to_ascii_lowercase();
        self.ents
            .iter()
            .filter(|e| match key.as_str() {
                "classname" => e.classname.eq_ignore_ascii_case(value) || (e.kind == Kind::Ai && value == "actor"),
                _ => {
                    // The script may have changed it (`ent.targetname = ...`).
                    let o = vm.entity_obj(e.id);
                    let v = o.map(|o| vm.field(o, &key));
                    match v {
                        Some(Value::Str(s)) => &*s == value,
                        Some(Value::Int(i)) => i.to_string() == value,
                        _ => false,
                    }
                }
            })
            .map(|e| e.id)
            .collect()
    }

    fn nodes_matching(&self, value: &str, key: &str) -> Vec<ObjId> {
        let key = key.to_ascii_lowercase();
        self.nodes.iter().filter(|(_, _, f)| f.get(&key).is_some_and(|v| v == value)).map(|(o, _, _)| *o).collect()
    }

    fn this_ent(&self, vm: &Vm, this: Option<&Value>) -> Option<EntId> {
        vm.entity_of(this?)
    }

    fn arg_ent(&self, vm: &Vm, args: &[Value], i: usize) -> Option<EntId> {
        vm.entity_of(args.get(i)?)
    }
}

impl Host for Level {
    fn call(&mut self, vm: &mut Vm, name: &str, this: Option<&Value>, args: &[Value]) -> Result<Value, String> {
        let s = |i: usize| args.get(i).and_then(Value::as_str).unwrap_or_default().to_owned();
        let f = |i: usize| args.get(i).and_then(Value::as_f32).unwrap_or(0.0);
        let v = |i: usize| args.get(i).and_then(Value::as_vec).unwrap_or_default();
        let me = self.this_ent(vm, this);
        let now = self.time;
        Ok(match name {
            // ---- Finding things ----
            "getent" => {
                let found = self.matching(vm, &s(0), &s(1));
                found.first().map_or(Value::Undefined, |&id| vm.entity(id))
            }
            "getentarray" => {
                if args.is_empty() {
                    let ids: Vec<EntId> = self.ents.iter().filter(|e| e.kind != Kind::Player).map(|e| e.id).collect();
                    Value::array(ids.into_iter().map(|id| vm.entity(id)))
                } else {
                    let found = self.matching(vm, &s(0), &s(1));
                    Value::array(found.into_iter().map(|id| vm.entity(id)))
                }
            }
            "getnode" => self.nodes_matching(&s(0), &s(1)).first().map_or(Value::Undefined, |&o| Value::Object(o)),
            "getnodearray" => Value::array(self.nodes_matching(&s(0), &s(1)).into_iter().map(Value::Object)),
            "getallnodes" => Value::array(self.nodes.iter().map(|(o, _, _)| Value::Object(*o))),
            "getvehiclenode" | "getvehiclenodearray" | "getallvehiclenodes" => {
                // `info_vehicle_node`s are map entities here.
                let found: Vec<EntId> = if name == "getallvehiclenodes" {
                    self.ents.iter().filter(|e| e.classname.contains("vehicle_node")).map(|e| e.id).collect()
                } else {
                    self.matching(vm, &s(0), &s(1)).into_iter().filter(|id| self.ent(*id).is_some_and(|e| e.classname.contains("vehicle_node"))).collect()
                };
                if name == "getvehiclenode" {
                    found.first().map_or(Value::Undefined, |&id| vm.entity(id))
                } else {
                    Value::array(found.into_iter().map(|id| vm.entity(id)))
                }
            }
            "getspawnerarray" => self.entities_value(vm, self.ents.iter().filter(|e| e.kind == Kind::Spawner).collect::<Vec<_>>().into_iter()),
            "getspawnerteamarray" => {
                let team = s(0);
                let ids: Vec<EntId> = self.ents.iter().filter(|e| e.kind == Kind::Spawner && spawner_team(&e.classname) == team).map(|e| e.id).collect();
                Value::array(ids.into_iter().map(|id| vm.entity(id)))
            }
            "getaiarray" | "getaispeciesarray" => {
                let team = s(0);
                let ids: Vec<EntId> = self.ents.iter().filter(|e| e.kind == Kind::Ai && e.alive && (team.is_empty() || team == "all" || e.team == team)).map(|e| e.id).collect();
                Value::array(ids.into_iter().map(|id| vm.entity(id)))
            }
            "isalive" => Value::Int(i32::from(self.arg_ent(vm, args, 0).and_then(|id| self.ent(id)).is_some_and(|e| e.alive))),
            "isai" => Value::Int(i32::from(self.arg_ent(vm, args, 0).and_then(|id| self.ent(id)).is_some_and(|e| e.kind == Kind::Ai))),
            "isplayer" => Value::Int(i32::from(self.arg_ent(vm, args, 0) == Some(PLAYER))),
            "issentient" => Value::Int(i32::from(self.arg_ent(vm, args, 0).and_then(|id| self.ent(id)).is_some_and(|e| matches!(e.kind, Kind::Ai | Kind::Player)))),
            "isspawner" => Value::Int(i32::from(self.arg_ent(vm, args, 0).and_then(|id| self.ent(id)).is_some_and(|e| e.kind == Kind::Spawner))),
            // ---- Dvars and such ----
            "getdvar" => Value::str(self.dvars.get(&s(0).to_ascii_lowercase()).map_or("", |v| v.as_str())),
            "getdvarint" => Value::Int(self.dvars.get(&s(0).to_ascii_lowercase()).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) as i32),
            "getdvarfloat" => Value::Float(self.dvars.get(&s(0).to_ascii_lowercase()).and_then(|v| v.parse().ok()).unwrap_or(0.0)),
            "getdebugdvar" => Value::str(""),
            "getdebugdvarint" => Value::Int(0),
            "setdvar" | "setsaveddvar" | "setmissiondvar" => {
                let val = args.get(1).map(Value::display).unwrap_or_default();
                self.dvars.insert(s(0).to_ascii_lowercase(), val);
                Value::Undefined
            }
            "getdifficulty" => Value::str("medium"),
            "issaverecentlyloaded" => Value::Int(0),
            "issavesuccessful" => Value::Int(1),
            "savegame" | "savegamenocommit" => Value::Int(1),
            "commitsave" | "updategamerprofile" | "giveachievement" | "uploadscore" | "uploadtime" | "setculldist" | "setsundirection" | "setsunlight" | "resetsunlight" | "lerpsundirection" => Value::Undefined,
            "getmapsunlight" => Value::Vector([1.0, 1.0, 1.0]),
            "getnorthyaw" => Value::Float(0.0),
            "soundexists" => Value::Int(1),
            "getkeybinding" | "getcommandfromkey" => Value::str(""),
            "openfile" => Value::Int(-1),
            "closefile" | "fprintln" | "fprintfields" => Value::Undefined,
            "createprintchannel" | "setprintchannel" => Value::Undefined,
            "getanimlength" => {
                let n = match args.first() {
                    Some(Value::Anim(n, _)) => n.to_string(),
                    _ => String::new(),
                };
                Value::Float((self.anim_length)(&n).unwrap_or(1.0))
            }
            "getnotetracktimes" => Value::array([]),
            "animhasnotetrack" => Value::Int(0),
            "getstartorigin" => Value::Vector(v(0)),
            "getstartangles" => Value::Vector(v(1)),
            "getmovedelta" | "getangledelta" => Value::Vector([0.0; 3]),
            // ---- Loading and precache: nothing to do ----
            n if n.starts_with("precache") => Value::Undefined,
            "loadfx" => {
                self.fx.push(s(0));
                Value::Int(self.fx.len() as i32 - 1)
            }
            "setminimap" | "ambientstop" | "setambienteq" | "deactivateeq" | "eqoff" | "seteq" | "seteqlerp" | "setreverb" | "deactivatereverb" | "setenginevolume" | "setsoundblend" => Value::Undefined,
            "ambientplay" => {
                self.commands.push(Command::Music(Some(s(0))));
                Value::Undefined
            }
            "musicplay" => {
                self.commands.push(Command::Music(Some(s(0))));
                Value::Undefined
            }
            "musicstop" => {
                self.commands.push(Command::Music(None));
                Value::Undefined
            }
            "setexpfog" => {
                self.commands.push(Command::Fog { near: f(0), half: f(1), color: [f(2), f(3), f(4)], time: f(5) });
                Value::Undefined
            }
            "visionsetnaked" | "visionsetnight" => {
                self.commands.push(Command::Vision { name: s(0), time: f(1) });
                Value::Undefined
            }
            "setvolfog" | "setblur" | "setdepthoffield" | "setviewmodeldepthoffield" | "settimescale" => Value::Undefined,
            // ---- Effects and sounds ----
            "playfx" => {
                let fx = self.fx.get(f(0) as usize).cloned().unwrap_or_default();
                self.commands.push(Command::PlayFx { fx, at: v(1), forward: args.get(2).and_then(Value::as_vec).unwrap_or([1.0, 0.0, 0.0]) });
                Value::Undefined
            }
            "playfxontag" => {
                let fx = self.fx.get(f(0) as usize).cloned().unwrap_or_default();
                if let Some(ent) = self.arg_ent(vm, args, 1) {
                    self.commands.push(Command::PlayFxOnTag { fx, ent, tag: s(2) });
                }
                Value::Undefined
            }
            "playloopedfx" | "spawnfx" => {
                let fx = self.fx.get(f(0) as usize).cloned().unwrap_or_default();
                let at = if name == "spawnfx" { v(1) } else { v(2) };
                let id = self.add(blank("fx_looped", at));
                self.commands.push(Command::PlayFx { fx, at, forward: [1.0, 0.0, 0.0] });
                vm.entity(id)
            }
            "triggerfx" => Value::Undefined,
            "earthquake" => {
                self.commands.push(Command::Earthquake { scale: f(0), time: f(1), at: v(2), radius: f(3) });
                Value::Undefined
            }
            "physicsexplosionsphere" | "physicsjolt" | "physicsjitter" | "radiusdamage" | "magicgrenade" | "magicgrenademanual" | "magicbullet" | "badplace_cylinder" | "badplace_arc" | "badplace_delete" | "setignoremegroup" | "createthreatbiasgroup" | "setthreatbias" | "clearallcorpses" | "setphysicsgravitydir" | "setsaveddvarif" => {
                self.commands.push(Command::Call { ent: 0, name: name.to_owned(), args: args.to_vec() });
                Value::Undefined
            }
            "getthreatbias" => Value::Int(0),
            "bullettrace" => {
                let st = vm.spawn_struct();
                if let Some(o) = st.as_obj() {
                    vm.set_field(o, "fraction", Value::Float(1.0));
                    vm.set_field(o, "position", Value::Vector(v(1)));
                    vm.set_field(o, "surfacetype", Value::str("none"));
                }
                // CoD's is an array keyed by name.
                let mut a = crate::vm::Array::default();
                a.set(crate::vm::Key::Str("fraction".into()), Value::Float(1.0));
                a.set(crate::vm::Key::Str("position".into()), Value::Vector(v(1)));
                a.set(crate::vm::Key::Str("surfacetype".into()), Value::str("none"));
                Value::Array(Rc::new(a))
            }
            "bullettracepassed" | "sighttracepassed" => Value::Int(1),
            "physicstrace" | "playerphysicstrace" => Value::Vector(v(1)),
            // ---- Text, objectives, the mission ----
            "iprintln" | "iprintlnbold" => {
                self.commands.push(Command::Print { text: args.first().map(Value::display).unwrap_or_default(), bold: name == "iprintlnbold" });
                Value::Undefined
            }
            "objective_add" => {
                let at = args.get(3).and_then(Value::as_vec);
                let text = args.get(2).map(Value::display).unwrap_or_default();
                self.objectives.insert(f(0) as i32, (s(1), text.clone(), at));
                self.commands.push(Command::Objective { index: f(0) as i32, state: s(1), text, at });
                Value::Undefined
            }
            "objective_state" | "objective_string" | "objective_position" | "objective_delete" => {
                let idx = f(0) as i32;
                let entry = self.objectives.entry(idx).or_insert_with(|| ("active".into(), String::new(), None));
                match name {
                    "objective_state" => entry.0 = s(1),
                    "objective_string" => entry.1 = args.get(1).map(Value::display).unwrap_or_default(),
                    "objective_position" => entry.2 = args.get(1).and_then(Value::as_vec),
                    _ => entry.0 = "empty".into(),
                }
                let (state, text, at) = entry.clone();
                self.commands.push(Command::Objective { index: idx, state, text, at });
                Value::Undefined
            }
            "objective_current" => {
                self.commands.push(Command::ObjectiveCurrent(f(0) as i32));
                Value::Undefined
            }
            "objective_additionalposition" | "objective_icon" | "objective_team" | "objective_ring" => Value::Undefined,
            "missionfailed" => {
                self.commands.push(Command::MissionFailed);
                Value::Undefined
            }
            "missionsuccess" => {
                self.commands.push(Command::MissionSuccess);
                Value::Undefined
            }
            "changelevel" => {
                self.commands.push(Command::ChangeLevel(s(0)));
                Value::Undefined
            }
            // ---- HUD elements ----
            "newhudelem" | "newclienthudelem" | "newteamhudelem" => {
                let o = vm.spawn_struct();
                if let Some(id) = o.as_obj() {
                    self.hud.insert(id, HudElem::default());
                    vm.set_field(id, "alpha", Value::Float(1.0));
                }
                o
            }
            "settext" | "setshader" | "settimer" | "settimerup" | "settenthstimer" | "settenthstimerup" | "setvalue" | "fadeovertime" | "moveovertime" | "scaleovertime" | "changefontscaleovertime" | "setpulsefx" | "setclock" | "setclockup" | "setplayernamestring" | "setmapnamestring" | "setgametypestring" | "setwaypoint" | "cleartargetent" | "settargetent" => {
                if let (Some(o), "settext") = (this.and_then(Value::as_obj), name) {
                    if let Some(h) = self.hud.get_mut(&o) {
                        h.text = args.first().map(Value::display).unwrap_or_default();
                    }
                }
                Value::Undefined
            }
            "destroy" => {
                if let Some(o) = this.and_then(Value::as_obj) {
                    self.hud.remove(&o);
                }
                Value::Undefined
            }
            "clearalltextafterhudelem" => Value::Undefined,
            // ---- Entities ----
            "spawn" => {
                let id = self.add(blank(&s(0), v(1)));
                let e = vm.entity(id);
                if let Some(o) = e.as_obj() {
                    vm.set_field(o, "classname", Value::str(&s(0)));
                }
                e
            }
            "spawnstruct" => vm.spawn_struct(),
            "spawnvehicle" | "spawnturret" => {
                let id = self.add(blank(name, v(2)));
                vm.entity(id)
            }
            "getentnum" | "getentitynumber" => Value::Int(me.unwrap_or(0) as i32),
            "delete" => {
                if let Some(id) = me {
                    self.remove(vm, id);
                }
                Value::Undefined
            }
            "hide" | "show" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    e.hidden = name == "hide";
                    let id = e.id;
                    self.commands.push(Command::Show(id, name == "show"));
                }
                Value::Undefined
            }
            "solid" | "notsolid" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    e.solid = name == "solid";
                    let id = e.id;
                    self.commands.push(Command::Solid(id, name == "solid"));
                }
                Value::Undefined
            }
            "setmodel" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    e.model = s(0);
                    let id = e.id;
                    self.commands.push(Command::SetModel(id, s(0)));
                }
                Value::Undefined
            }
            "getorigin" => me.and_then(|id| self.ent(id)).map_or(Value::Undefined, |e| Value::Vector(e.origin)),
            "geteye" | "getshootatpos" => me.and_then(|id| self.ent(id)).map_or(Value::Undefined, |e| Value::Vector([e.origin[0], e.origin[1], e.origin[2] + 60.0])),
            "getplayerangles" => me.and_then(|id| self.ent(id)).map_or(Value::Undefined, |e| Value::Vector(e.angles)),
            "setorigin" | "teleport" | "forceteleport" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    e.origin = v(0);
                    if let Some(a) = args.get(1).and_then(Value::as_vec) {
                        e.angles = a;
                    }
                }
                self.forward(me, name, args);
                Value::Undefined
            }
            "setplayerangles" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    e.angles = v(0);
                }
                self.forward(me, name, args);
                Value::Undefined
            }
            "linkto" => {
                let to = self.arg_ent(vm, args, 0);
                if let (Some(id), Some(to)) = (me, to) {
                    let (o, a) = self.ent(to).map(|t| (t.origin, t.angles)).unwrap_or_default();
                    let mine = self.ent(id).map(|e| (e.origin, e.angles)).unwrap_or_default();
                    // Keep where it is relative to the parent unless told.
                    let offset = args.get(2).and_then(Value::as_vec).unwrap_or_else(|| {
                        let d = [mine.0[0] - o[0], mine.0[1] - o[1], mine.0[2] - o[2]];
                        let (f, r, u) = crate::vm::angle_vectors(a);
                        [d[0] * f[0] + d[1] * f[1] + d[2] * f[2], -(d[0] * r[0] + d[1] * r[1] + d[2] * r[2]), d[0] * u[0] + d[1] * u[1] + d[2] * u[2]]
                    });
                    let angles = args.get(3).and_then(Value::as_vec).unwrap_or([mine.1[0] - a[0], mine.1[1] - a[1], mine.1[2] - a[2]]);
                    if let Some(e) = self.ent_mut(id) {
                        e.linked = Some((to, offset, angles));
                    }
                }
                Value::Undefined
            }
            "unlink" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    e.linked = None;
                }
                self.forward(me, name, args);
                Value::Undefined
            }
            "moveto" | "movex" | "movey" | "movez" | "movegravity" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    let from = e.origin;
                    let (to, time) = match name {
                        "moveto" => (v(0), f(1)),
                        "movex" => ([from[0] + f(0), from[1], from[2]], f(1)),
                        "movey" => ([from[0], from[1] + f(0), from[2]], f(1)),
                        "movez" => ([from[0], from[1], from[2] + f(0)], f(1)),
                        _ => {
                            let vel = v(0);
                            ([from[0] + vel[0] * f(1), from[1] + vel[1] * f(1), from[2]], f(1))
                        }
                    };
                    e.move_pos = Some(Mover { from, to, start: now, time: time as f64, rotate: false, velocity: None });
                }
                Value::Undefined
            }
            "rotateto" | "rotateyaw" | "rotatepitch" | "rotateroll" | "rotatevelocity" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    let from = e.angles;
                    let (to, time, vel) = match name {
                        "rotateto" => (v(0), f(1), None),
                        "rotateyaw" => ([from[0], from[1] + f(0), from[2]], f(1), None),
                        "rotatepitch" => ([from[0] + f(0), from[1], from[2]], f(1), None),
                        "rotateroll" => ([from[0], from[1], from[2] + f(0)], f(1), None),
                        _ => (from, f(1), Some(v(0))),
                    };
                    e.move_rot = Some(Mover { from, to, start: now, time: time as f64, rotate: true, velocity: vel });
                }
                Value::Undefined
            }
            "istouching" => {
                let other = self.arg_ent(vm, args, 0).and_then(|id| self.ent(id));
                let mine = me.and_then(|id| self.ent(id));
                let touching = match (mine, other) {
                    (Some(a), Some(b)) if b.is_trigger() || b.bounds.is_some() && b.kind == Kind::Map => b.contains(a.origin) || b.contains([a.origin[0], a.origin[1], a.origin[2] + 40.0]),
                    (Some(a), Some(b)) => a.contains(b.origin),
                    _ => false,
                };
                Value::Int(i32::from(touching))
            }
            "enablelinkto" | "setcandamage" | "setcontents" | "setshadowhint" | "laseron" | "laseroff" | "useanimtree" | "dontinterpolate" | "setlightintensity" | "setlightcolor" | "setlightradius" => Value::Undefined,
            "getlightintensity" => Value::Float(1.0),
            "sethintstring" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    e.hint = args.first().map(Value::display).unwrap_or_default();
                }
                Value::Undefined
            }
            "setcursorhint" | "usetriggerrequirelookat" | "makeusable" | "makeunusable" => Value::Undefined,
            "triggeron" | "triggerenable" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    e.enabled = true;
                }
                Value::Undefined
            }
            "triggeroff" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    e.enabled = false;
                }
                Value::Undefined
            }
            "useby" => {
                if let Some(o) = this.and_then(Value::as_obj) {
                    let by = args.first().cloned().unwrap_or_default();
                    vm.notify(o, "trigger", vec![by]);
                }
                Value::Undefined
            }
            "playsound" | "playsoundasmaster" | "playloopsound" | "playlocalsound" => {
                let at = me.and_then(|id| self.ent(id)).map(|e| e.origin).unwrap_or_default();
                self.commands.push(Command::PlaySound { ent: me, alias: s(0), at, looped: name == "playloopsound" });
                // `playsound(alias, notify)`: done at once (we don't know
                // how long it is).
                if name == "playsound" && args.len() > 1
                    && let Some(o) = this.and_then(Value::as_obj)
                {
                    let ev = s(1);
                    vm.notify(o, &ev, Vec::new());
                }
                Value::Undefined
            }
            "stoploopsound" | "stopsounds" => {
                if let Some(id) = me {
                    self.commands.push(Command::StopSound(id));
                }
                Value::Undefined
            }
            "iswaitingonsound" => Value::Int(0),
            "attach" | "detach" | "detachall" | "hidepart" | "showpart" | "setanim" | "setanimknob" | "setanimknoball" | "setanimknobrestart" | "setanimknoballrestart" | "setanimrestart" | "setflaggedanim" | "setflaggedanimknob" | "setflaggedanimknoball" | "setflaggedanimknobrestart" | "setflaggedanimknoballrestart" | "setflaggedanimrestart" | "setanimlimited" | "setanimtime" | "clearanim" | "animscripted" | "animcustom" | "stopanimscripted" | "setanimknoblimited" | "setanimknoblimitedrestart" | "setflaggedanimlimited" => {
                self.forward(me, name, args);
                Value::Undefined
            }
            "getattachsize" => Value::Int(0),
            "gettagorigin" | "gettagangles" => {
                let e = me.and_then(|id| self.ent(id));
                e.map_or(Value::Undefined, |e| Value::Vector(if name == "gettagorigin" { e.origin } else { e.angles }))
            }
            "localtoworldcoords" => {
                let e = me.and_then(|id| self.ent(id)).map(|e| (e.origin, e.angles)).unwrap_or_default();
                let (fw, r, u) = crate::vm::angle_vectors(e.1);
                let l = v(0);
                Value::Vector([
                    e.0[0] + fw[0] * l[0] - r[0] * l[1] + u[0] * l[2],
                    e.0[1] + fw[1] * l[0] - r[1] * l[1] + u[1] * l[2],
                    e.0[2] + fw[2] * l[0] - r[2] * l[1] + u[2] * l[2],
                ])
            }
            // ---- Spawners and AI ----
            "dospawn" | "stalingradspawn" => {
                let Some(id) = me else { return Ok(Value::Undefined) };
                let Some(sp) = self.ent(id).cloned() else { return Ok(Value::Undefined) };
                let count = vm.entity_obj(id).map(|o| vm.field(o, "count")).and_then(|c| c.as_i32()).unwrap_or(1);
                if count <= 0 && sp.kind == Kind::Spawner {
                    return Ok(Value::Undefined);
                }
                if let Some(o) = vm.entity_obj(id) {
                    vm.set_field(o, "count", Value::Int(count - 1));
                }
                let team = spawner_team(&sp.classname).to_owned();
                let mut ai = blank(&sp.classname, sp.origin);
                ai.kind = Kind::Ai;
                ai.angles = sp.angles;
                ai.model = sp.model.clone();
                ai.team = team.clone();
                ai.alive = true;
                ai.health = 150;
                ai.max_health = 150;
                ai.bounds = Some(([-15.0, -15.0, 0.0], [15.0, 15.0, 70.0]));
                let ai_id = self.add(ai);
                let ai_val = vm.entity(ai_id);
                // The spawner's script fields carry over (targetname, script_*).
                if let (Some(from), Some(to)) = (vm.entity_obj(id), ai_val.as_obj()) {
                    let fields: Vec<(Rc<str>, Value)> = vm.object(from).map(|o| o.fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
                    for (k, v) in fields {
                        if &*k != "count" {
                            vm.set_field(to, &k, v);
                        }
                    }
                    vm.set_field(to, "team", Value::str(&team));
                    vm.set_field(to, "spawner", Value::Object(from));
                }
                // As the engine: the actor's type script (team, weapons,
                // character), then the AI's script state.
                let aitype = sp.classname.trim_start_matches("actor_").to_ascii_lowercase();
                if let Some(f) = vm.func(&format!("aitype/{aitype}"), "main") {
                    vm.call_now(self, f, ai_val.clone(), Vec::new());
                }
                if let Some(f) = vm.func("animscripts/init", "main") {
                    vm.call_now(self, f, ai_val.clone(), Vec::new());
                }
                let model = self.ent(ai_id).map_or(sp.model.clone(), |e| e.model.clone());
                let team = ai_val.as_obj().and_then(|o| vm.field(o, "team").as_str().map(str::to_owned)).unwrap_or(team);
                if let Some(e) = self.ent_mut(ai_id) {
                    e.team = team.clone();
                }
                self.commands.push(Command::SpawnAi { spawner: id, ai: ai_id, origin: sp.origin, angles: sp.angles, team, classname: sp.classname.clone(), model, force: name == "stalingradspawn" });
                if let Some(o) = vm.entity_obj(id) {
                    vm.notify(o, "spawned", vec![ai_val.clone()]);
                }
                ai_val
            }
            // codescripts\character (not in the zones: the engine's own).
            "get_random_character" => Value::Int((vm.random_int(f(0).max(1.0) as u32)) as i32),
            "new" | "save" | "load" | "precache" => Value::Undefined,
            "spawnfailed" => Value::Int(i32::from(!args.first().is_some_and(Value::is_defined))),
            "setgoalnode" | "setgoalpos" | "setgoalentity" | "setgoalvolume" | "setgoalvolumeauto" => {
                let goal = match name {
                    "setgoalnode" => args.first().and_then(Value::as_obj).and_then(|o| vm.field(o, "origin").as_vec()),
                    "setgoalpos" => args.first().and_then(Value::as_vec),
                    _ => self.arg_ent(vm, args, 0).and_then(|id| self.ent(id)).map(|e| e.origin),
                };
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    e.goal = goal;
                }
                if let Some(g) = goal {
                    self.forward(me, "setgoalpos", &[Value::Vector(g)]);
                }
                Value::Undefined
            }
            "dodamage" => {
                if let Some(id) = me {
                    let attacker = self.arg_ent(vm, args, 2);
                    let at = v(1);
                    self.damaged(vm, id, f(0) as i32, attacker, [0.0; 3], at);
                    self.forward(me, name, args);
                }
                Value::Undefined
            }
            "kill" => {
                if let Some(id) = me {
                    let hp = self.ent(id).map_or(0, |e| e.health.max(1));
                    self.damaged(vm, id, hp, None, [0.0; 3], [0.0; 3]);
                    self.forward(me, name, args);
                }
                Value::Undefined
            }
            "getcurrentweapon" => Value::str("mp5_silencer"),
            "getweaponslistprimaries" | "getweaponslist" => Value::array([]),
            "hasweapon" => Value::Int(0),
            "getweaponammoclip" | "getweaponammostock" | "getfractionmaxammo" => Value::Int(0),
            "getstance" => Value::str("stand"),
            "isgodmode" | "isthrowinggrenade" | "ismeleeing" | "isfiring" | "isragdoll" | "buttonpressed" | "playerads" | "usebuttonpressed" | "attackbuttonpressed" | "adsbuttonpressed" | "meleebuttonpressed" | "fragbuttonpressed" | "secondaryoffhandbuttonpressed" => Value::Int(0),
            "getplayerviewheight" => Value::Float(60.0),
            "isweapondetonationtimed" => Value::Int(0),
            "weaponclass" => Value::str("rifle"),
            "weaponclipsize" => Value::Int(30),
            "weaponisboltaction" | "weaponissemiauto" => Value::Int(0),
            "getweaponmodel" | "getweaponclipmodel" => Value::str(""),
            "tablelookup" => Value::str(""),
            "issuppressed" | "cansee" | "canshoot" | "isknownenemyinradius" | "isknownenemyinvolume" | "canattackenemynode" => Value::Int(0),
            "getthreatbiasgroup" => Value::str(""),
            "getspeed" | "getspeedmph" => Value::Float(0.0),
            "getturret" | "getturretowner" | "getvehicleowner" => Value::Undefined,
            "nearnode" => Value::Undefined,
            "getanglestolikelyenemypath" => Value::Undefined,
            // Everything an AI, the player, a vehicle or a turret does that
            // the game carries out.
            n if is_forwarded(n) => {
                self.forward(me, name, args);
                Value::Undefined
            }
            _ => return Err(name.to_owned()),
        })
    }

    fn get_field(&mut self, _vm: &mut Vm, entity: u64, name: &str) -> Option<Value> {
        let e = self.ent(entity)?;
        Some(match name {
            "origin" => Value::Vector(e.origin),
            "angles" => Value::Vector(e.angles),
            "classname" => Value::str(if e.kind == Kind::Ai { "actor" } else { &e.classname }),
            "code_classname" => Value::str(&e.classname),
            "model" => Value::str(&e.model),
            "health" => Value::Int(e.health),
            "maxhealth" => Value::Int(e.max_health),
            "team" if matches!(e.kind, Kind::Ai | Kind::Player) => Value::str(&e.team),
            "spawnflags" => Value::Int(e.spawnflags),
            _ => return None,
        })
    }

    fn set_field(&mut self, _vm: &mut Vm, entity: u64, name: &str, value: &Value) -> bool {
        let Some(e) = self.ent_mut(entity) else { return false };
        match name {
            "origin" => {
                if let Some(v) = value.as_vec() {
                    e.origin = v;
                }
            }
            "angles" => {
                if let Some(v) = value.as_vec() {
                    e.angles = v;
                }
            }
            "health" => e.health = value.as_i32().unwrap_or(e.health),
            "maxhealth" => e.max_health = value.as_i32().unwrap_or(e.max_health),
            "team" if matches!(e.kind, Kind::Ai | Kind::Player) => e.team = value.as_str().unwrap_or_default().to_owned(),
            _ => return false,
        }
        let id = e.id;
        if matches!(name, "health" | "team" | "origin" | "angles") && matches!(self.ent(id).map(|e| &e.kind), Some(Kind::Ai | Kind::Player)) {
            self.commands.push(Command::Call { ent: id, name: format!("field:{name}"), args: vec![value.clone()] });
        }
        true
    }
}

impl Level {
    fn forward(&mut self, me: Option<EntId>, name: &str, args: &[Value]) {
        if let Some(ent) = me {
            self.commands.push(Command::Call { ent, name: name.to_owned(), args: args.to_vec() });
        }
    }
}

/// Builtins on the player, AI, vehicles and turrets that the game handles
/// (queued as [`Command::Call`]).
fn is_forwarded(n: &str) -> bool {
    const PREFIXES: [&str; 6] = ["allow", "setgoal", "setturret", "setveh", "setvehicle", "playerlinkto"];
    const NAMES: &[&str] = &[
        "giveweapon", "takeweapon", "takeallweapons", "switchtoweapon", "switchtooffhand", "setweaponammoclip", "setweaponammostock", "givemaxammo",
        "setoffhandsecondaryclass", "setactionslot", "setviewmodel", "freezecontrols", "disableweapons", "enableweapons", "enableinvulnerability",
        "disableinvulnerability", "enablehealthshield", "setnormalhealth", "shellshock", "stopshellshock", "setstance", "enterprone", "exitprone",
        "setmovespeedscale", "setflashbanged", "setflashbangimmunity", "playrumbleonentity", "playrumblelooponentity", "stoprumble", "pushplayer",
        "playersetgroundreferenceent", "disableaimassist", "enableaimassist", "openmenu", "closemenu", "setclientdvar", "setblurforplayer",
        "notifyoncommand", "forceviewmodelanimation", "setspawnerteam", "setengagementmindist", "setengagementmaxdist", "settargetentity",
        "cleartargetentity", "setentitytarget", "clearentitytarget", "setthreatbiasgroup", "clearenemy", "setlookatent", "clearlookatent",
        "setlookattext", "setfriendlychain", "orientmode", "animmode", "setruntopos", "setyawspeed", "settargetyaw", "cleartargetyaw",
        "setgoalyaw", "cleargoalyaw", "startragdoll", "shoot", "shootblank", "startfiring", "stopfiring", "useturret", "stopuseturret",
        "maketurretusable", "maketurretunusable", "shootturret", "clearturrettarget", "startpath", "attachpath", "resumespeed", "setspeed",
        "setspeedimmediate", "setswitchnode", "setwaitnode", "setwaitspeed", "freevehicle", "makevehicleunusable", "makevehicleusable",
        "sethoverparams", "joltbody", "setfixednodesafevolume", "clearfixednodesafevolume", "setneargoalnotifydist", "dropweapon", "fireweapon",
        "setdefaultdroppitch", "restoredefaultdroppitch", "setproneanimnodes", "pushplayer",
    ];
    PREFIXES.iter().any(|p| n.starts_with(p)) || NAMES.contains(&n) || n.starts_with("set") || n.starts_with("clear") || n.starts_with("disable") || n.starts_with("enable")
}

fn spawner_team(classname: &str) -> &'static str {
    if classname.starts_with("actor_enemy") || classname.starts_with("actor_axis") {
        "axis"
    } else if classname.starts_with("actor_ally") {
        "allies"
    } else {
        "neutral"
    }
}

fn node_type(class: &str) -> &'static str {
    match class {
        "node_cover_stand" => "Cover Stand",
        "node_cover_crouch" => "Cover Crouch",
        "node_cover_crouch_window" => "Cover Crouch Window",
        "node_cover_left" => "Cover Left",
        "node_cover_right" => "Cover Right",
        "node_cover_prone" => "Cover Prone",
        "node_concealment_stand" => "Conceal Stand",
        "node_concealment_crouch" => "Conceal Crouch",
        "node_concealment_prone" => "Conceal Prone",
        "node_guard" => "Guard",
        "node_ambush" => "Ambush",
        "node_exposed" => "Exposed",
        "node_turret" => "Turret",
        "node_scripted" => "Scripted",
        "node_negotiation_begin" => "Begin",
        "node_negotiation_end" => "End",
        "node_balcony" => "Balcony",
        _ => "Path",
    }
}

fn blank(classname: &str, origin: [f32; 3]) -> Ent {
    Ent {
        id: 0,
        kind: Kind::Spawned,
        classname: classname.to_owned(),
        origin,
        angles: [0.0; 3],
        model: String::new(),
        hidden: false,
        solid: true,
        bounds: None,
        radius: None,
        spawnflags: 0,
        team: String::new(),
        health: 100,
        max_health: 100,
        alive: false,
        linked: None,
        move_pos: None,
        move_rot: None,
        enabled: true,
        hint: String::new(),
        touching: false,
        fired: false,
        goal: None,
        kv: Vec::new(),
    }
}

fn num(s: Option<&str>) -> Option<f32> {
    s?.trim().parse().ok()
}

fn vec3(s: Option<&str>) -> Option<[f32; 3]> {
    let v: Vec<f32> = s?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

/// A map key's value as the scripts read it: vectors for `origin`/`angles`,
/// numbers where it's a number (not for names).
fn field_value(key: &str, v: &str) -> Value {
    if matches!(key, "origin" | "angles")
        && let Some(x) = vec3(Some(v))
    {
        return Value::Vector(x);
    }
    if !STRING_KEYS.contains(&key) {
        if let Ok(i) = v.trim().parse::<i32>() {
            return Value::Int(i);
        }
        if let Ok(f) = v.trim().parse::<f32>()
            && v.contains('.')
        {
            return Value::Float(f);
        }
    }
    Value::str(v)
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
