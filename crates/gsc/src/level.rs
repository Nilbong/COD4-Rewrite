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

/// What the game knows of an animation (`%name`).
#[derive(Clone, Debug, Default)]
pub struct AnimInfo {
    /// Seconds.
    pub length: f32,
    pub looping: bool,
    /// Has root motion.
    pub delta: bool,
    /// Notetracks: (fraction 0..1, name), in order.
    pub notes: Vec<(f32, String)>,
    /// Moves some bones only, not the body (a face's talking): it plays
    /// over the body's animation, never in its place.
    pub partial: bool,
}

/// The game's animations, for the scripts' builtins.
pub trait AnimLib {
    fn info(&mut self, name: &str) -> Option<AnimInfo>;
    /// Root motion from the animation's start to `frac`: (offset in its own
    /// axes, x forward y left z up; turn in degrees).
    fn delta(&mut self, name: &str, frac: f32) -> ([f32; 3], f32);
}

/// No animations (headless runs): every one a second long, no motion.
pub struct NoAnims;

impl AnimLib for NoAnims {
    fn info(&mut self, _name: &str) -> Option<AnimInfo> {
        None
    }
    fn delta(&mut self, _name: &str, _frac: f32) -> ([f32; 3], f32) {
        ([0.0; 3], 0.0)
    }
}

/// An animation an entity plays.
#[derive(Clone, Debug, Default)]
pub struct EntAnim {
    pub name: String,
    /// The notify its notetracks and end come on (`animscripted`'s, a
    /// flagged anim's), if any.
    pub notify: Option<String>,
    /// When it started (script time), and how fast (0 holds it).
    pub start: f64,
    pub rate: f32,
    /// Where it started from: progress (0..1) at `start`.
    pub from: f32,
    pub info: AnimInfo,
    /// `animscripted`: the entity follows the root motion from here (in
    /// the world, or relative to what it's linked to: see `anchored`).
    pub scripted: Option<([f32; 3], [f32; 3])>,
    /// `scripted` is in its link parent's (tag's) axes: a passenger's
    /// seat anim rides with the car.
    pub anchored: bool,
    /// Notetracks passed so far (and the end sent).
    pub next_note: usize,
    pub ended: bool,
}

impl EntAnim {
    /// Progress (0..1) at `now`.
    pub fn frac(&self, now: f64) -> f32 {
        let len = self.info.length.max(1e-3);
        let f = self.from + ((now - self.start) as f32 * self.rate) / len;
        if self.info.looping { f.rem_euclid(1.0) } else { f.clamp(0.0, 1.0) }
    }
}

/// Animation and tag state of an entity.
#[derive(Clone, Debug, Default)]
pub struct AnimState {
    pub anim: Option<EntAnim>,
    /// Played over `anim` while an `animscripted` one has the root (CoD's
    /// tree blends them: a car's drive under its idle bumps).
    pub overlay: Option<EntAnim>,
    /// Tags as the game last drew them, in the entity's own axes (offset:
    /// x forward y left z up; angles relative), so they follow it with no
    /// lag; and the tags the scripts asked for (the game reports those).
    pub tags: HashMap<String, ([f32; 3], [f32; 3])>,
    pub wanted: Vec<String>,
    /// Linked to a tag of what it's linked to.
    pub link_tag: Option<String>,
    /// Models attached (`attach`): (model, tag).
    pub attached: Vec<(String, String)>,
}

/// The player carried by an entity (`playerlinkto` / `playerlinktodelta`).
#[derive(Clone, Debug)]
pub struct PlayerLink {
    pub to: EntId,
    pub tag: Option<String>,
    /// How far the view may turn: right, left, up, down (degrees).
    pub arcs: [f32; 4],
    /// `playerlinktodelta`: the view turns with the tag.
    pub delta: bool,
    /// On a vehicle's gun (`useby`): its weapon fires, not the player's.
    pub turret: bool,
    /// Where the player sits from the tag (its axes: forward, left, up).
    pub offset: [f32; 3],
}

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
    /// Its brushes' true shape (local planes), where known.
    pub shape: Option<Rc<Vec<Vec<([f32; 3], f32)>>>>,
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
    pub anim_state: AnimState,
}

impl Ent {
    /// A tag where it is now (world origin, angles); the entity's own
    /// place if the game hasn't drawn it.
    pub fn tag(&self, name: &str) -> ([f32; 3], [f32; 3]) {
        match self.anim_state.tags.get(name) {
            Some((o, a)) => (local_to_world(self.origin, self.angles, *o), [self.angles[0] + a[0], self.angles[1] + a[1], self.angles[2] + a[2]]),
            None => (self.origin, self.angles),
        }
    }

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
            let inside_box = (0..3).all(|i| p[i] >= mn[i] + self.origin[i] && p[i] <= mx[i] + self.origin[i]);
            // (Within its box: inside one of its brushes, if their shape is known.)
            return inside_box
                && self.shape.as_ref().is_none_or(|shape| {
                    let l = [p[0] - self.origin[0], p[1] - self.origin[1], p[2] - self.origin[2]];
                    shape.iter().any(|planes| planes.iter().all(|(n, d)| n[0] * l[0] + n[1] * l[1] + n[2] * l[2] <= d + 1.0))
                });
        }
        false
    }
}

/// What the scripts asked of the game, in order.
#[derive(Clone, Debug)]
pub enum Command {
    /// `setblur(radius, time)`: the whole view blurred (pixels at 640x480).
    Blur { radius: f32, time: f32 },
    /// The player's `shellshock(name, seconds)` (`shock/<name>.shock`); 0
    /// seconds stops it.
    Shellshock { name: String, duration: f32 },
    /// An actor fires at its enemy (`shoot`): its weapon, the target
    /// (an entity, or a point), how sure its aim is (accuracy x).
    Shoot { ai: EntId, target: Option<EntId>, at: [f32; 3], weapon: String, accuracy: f32 },
    /// Spawn an AI from a spawner (the id it will have).
    SpawnAi { spawner: EntId, ai: EntId, origin: [f32; 3], angles: [f32; 3], team: String, classname: String, model: String, force: bool },
    Delete(EntId),
    SetModel(EntId, String),
    Show(EntId, bool),
    Solid(EntId, bool),
    /// (With `notify`: the game says when it ends, [`Level::sound_ends`].)
    PlaySound { ent: Option<EntId>, alias: String, at: [f32; 3], looped: bool, notify: Option<String> },
    StopSound(EntId),
    PlayFx { fx: String, at: [f32; 3], forward: [f32; 3] },
    /// An effect kept going (`playloopedfx`, `spawnfx` + `triggerfx`):
    /// again every `delay` seconds (0: the effect's own looping).
    LoopFx { ent: EntId, fx: String, at: [f32; 3], forward: [f32; 3], delay: f32 },
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
    pub anims: Box<dyn AnimLib>,
    pub player_link: Option<PlayerLink>,
    /// Debug: notetracks sent whose name holds this, logged in `trace`.
    pub note_filter: Option<String>,
    pub trace: Vec<String>,
    pub hud: HashMap<ObjId, HudElem>,
    /// String tables by name (`maps/_intel_items.csv`): rows of cells.
    pub tables: HashMap<String, Vec<Vec<String>>>,
    /// The actors think here (the engine's side under the animscripts:
    /// [`Level::think_actors`]), not in the game.
    pub think_ai: bool,
    pub brains: HashMap<EntId, Brain>,
    /// The level's path nodes and their links, as the map compiled them
    /// (its GameWorldSp), for the actors' routes.
    pub path_nodes: Vec<PathNode>,
    /// Who sees whom this frame (looker, looked at), as the game traced it.
    pub sight: HashMap<(EntId, EntId), bool>,
    /// The level's weapons as their defs say (by name, lower case).
    pub weapons: HashMap<String, WeaponInfo>,
    /// The player's and actors' velocity, from how they moved last frame
    /// (and where they were then).
    velocity: HashMap<EntId, ([f32; 3], [f32; 3], f64)>,
    /// Mounted guns the scripts made unusable.
    unusable_turrets: std::collections::HashSet<EntId>,
    /// Vehicles' guns (`setvehweapon`), by vehicle.
    pub vehicle_weapons: HashMap<EntId, String>,
    /// Missiles flying (`fireweapon`).
    missiles: Vec<Missile>,
    /// Vehicles moving (on their paths, or a helicopter to its goal).
    pub vehicles: HashMap<EntId, VehMove>,
    /// Vehicles' turrets the scripts aim (`setTurretTargetEnt/Vec`).
    pub turret_aims: HashMap<EntId, TurretAim>,
    /// Entities gone at a time (missiles' flights).
    timed_deletes: Vec<(EntId, f64)>,
    /// Grenades down: when they go off, their weapon, whose they are.
    grenades: Vec<(EntId, f64, String, Option<EntId>)>,
    /// The first frame's trigger touches are done.
    first_touch: bool,
    /// Actors' grenade throws checked, waiting on their `throwGrenade()`:
    /// from (the hand), velocity.
    throws: HashMap<EntId, ([f32; 3], [f32; 3])>,
    /// Sounds' notifies (`playsound( alias, notify )`): when, on whom.
    sound_notifies: Vec<(f64, EntId, String)>,
    /// The player's weapons (as given), the one up, and their clips.
    pub player_weapons: Vec<String>,
    pub player_current: Option<String>,
    pub player_ammo: HashMap<String, i32>,
    /// Their ammo besides the clip.
    pub player_stock: HashMap<String, i32>,
    /// The game's word on the player each frame: how far aimed down the
    /// sights (0..1), their stance, the CoD4 commands they hold
    /// (`+attack`, `+speed_throw`, `+activate`, `+melee`, `+frag`...).
    pub player_ads: f32,
    pub player_stance: &'static str,
    pub player_held: Vec<String>,
    /// Actors suppressed until then (bullets of the other side passing
    /// close).
    suppressed: HashMap<EntId, f64>,
    /// Script entities bullets hurt (`setcandamage( true )`).
    damageable: std::collections::HashSet<EntId>,
    /// `notifyoncommand`: a command's notifies on the player.
    command_notifies: HashMap<String, Vec<String>>,
    /// `spawnfx` entities' effect and facing, for `triggerfx`.
    spawned_fx: HashMap<EntId, (String, [f32; 3])>,
    /// Threat bias groups made (`createthreatbiasgroup`).
    bias_groups: std::collections::HashSet<String>,
    pub objectives: HashMap<i32, (String, String, Option<[f32; 3]>)>,
    pub missing: HashMap<String, u32>,
}

/// A script HUD element's engine side: what it shows and its tweens (the
/// rest, place, alignment, colour, alpha, sort, is in its script fields:
/// [`Level::hud_snapshot`]).
#[derive(Clone, Debug, Default)]
pub struct HudElem {
    pub text: String,
    /// `setshader(material, width, height)`.
    pub shader: Option<(String, f32, f32)>,
    /// `fadeovertime`: (start, seconds, alpha and colour then).
    pub fade: Option<(f64, f32, f32, [f32; 3])>,
    /// `moveovertime`: (start, seconds, x and y then).
    pub moving: Option<(f64, f32, f32, f32)>,
    /// `scaleovertime`: (start, seconds, size then).
    pub scaling: Option<(f64, f32, f32, f32)>,
    /// `setpulsefx(ms per letter, ms until it decays, ms decaying)`, from when.
    pub pulse: Option<(f64, f32, f32, f32)>,
}

/// A HUD element as it shows now ([`Level::hud_snapshot`]); x, y and sizes
/// in CoD's 640x480 virtual screen.
#[derive(Clone, Debug)]
pub struct HudDraw {
    pub x: f32,
    pub y: f32,
    pub align: (String, String),
    pub screen_align: (String, String),
    pub alpha: f32,
    pub color: [f32; 3],
    pub sort: f32,
    pub foreground: bool,
    pub text: String,
    /// How many letters of it show (`setpulsefx` typing them on).
    pub letters: Option<usize>,
    pub font: String,
    pub font_scale: f32,
    pub glow: Option<([f32; 3], f32)>,
    pub shader: Option<(String, f32, f32)>,
}

/// Keys whose values stay strings (others become numbers where they look
/// like them).
const STRING_KEYS: &[&str] = &[
    "classname",
    "targetname",
    "target",
    "script_noteworthy",
    "script_linkname",
    "script_linkto",
    "model",
    "script_flag",
    "script_parameters",
    "script_animation",
    "script_noteworthy2",
    "script_vehicleride",
    // (Names the scripts key arrays and flags by, "0" included.)
    "script_aigroup",
    "script_squadname",
    "script_deathflag",
    "script_friendname",
    "script_flag_true",
    "script_flag_false",
    "script_flag_set",
    "script_flag_wait",
    "script_flag_clear",
    "script_ender",
    "script_fallback_group",
];

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
            anims: Box::new(NoAnims),
            player_link: None,
            note_filter: None,
            trace: Vec::new(),
            hud: HashMap::new(),
            tables: HashMap::new(),
            think_ai: false,
            brains: HashMap::new(),
            path_nodes: Vec::new(),
            sight: HashMap::new(),
            weapons: HashMap::new(),
            velocity: HashMap::new(),
            bias_groups: Default::default(),
            spawned_fx: HashMap::new(),
            timed_deletes: Vec::new(),
            grenades: Vec::new(),
            first_touch: false,
            throws: HashMap::new(),
            sound_notifies: Vec::new(),
            player_weapons: Vec::new(),
            player_current: None,
            player_ammo: HashMap::new(),
            player_stock: HashMap::new(),
            player_ads: 0.0,
            player_stance: "stand",
            player_held: Vec::new(),
            damageable: std::collections::HashSet::new(),
            suppressed: HashMap::new(),
            command_notifies: HashMap::new(),
            vehicles: HashMap::new(),
            turret_aims: HashMap::new(),
            missiles: Vec::new(),
            vehicle_weapons: HashMap::new(),
            unusable_turrets: Default::default(),
            objectives: HashMap::new(),
            missing: HashMap::new(),
        };
        for kv in entity_string {
            let get = |k: &str| kv.iter().find(|(a, _)| a.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
            let classname = get("classname").unwrap_or_default().to_ascii_lowercase();
            // (The sun as the map has it: the engine's sun dvar.)
            if classname == "worldspawn"
                && let Some(sun) = get("sundirection")
            {
                level.dvars.insert("r_lighttweaksundirection".into(), sun.to_owned());
            }
            if classname == "worldspawn"
                && let Some(c) = get("suncolor")
            {
                level.dvars.insert("r_lighttweaksuncolor".into(), c.to_owned());
            }
            // (Lights with a name are the scripts' to move and dim.)
            if classname.starts_with("node_") || classname == "script_struct" || classname == "worldspawn" || (classname == "light" && get("targetname").is_none()) {
                continue;
            }
            let model = get("model").unwrap_or_default().to_owned();
            let bounds = model.strip_prefix('*').and_then(|n| n.parse::<usize>().ok()).and_then(|n| brush_bounds.get(n).copied());
            let radius = (classname == "trigger_radius").then(|| (num(get("radius")).unwrap_or(0.0), num(get("height")).unwrap_or(0.0)));
            // (An actor without the spawner flag is a live one from the start.)
            let kind = if classname.starts_with("actor_") {
                if num(get("spawnflags")).unwrap_or(0.0) as i32 & 1 != 0 { Kind::Spawner } else { Kind::Ai }
            } else if classname == "script_vehicle" {
                Kind::Vehicle
            } else {
                Kind::Map
            };
            let id = level.next_id;
            level.next_id += 1;
            level.by_id.insert(id, level.ents.len());
            // (A vehicle is alive until its health runs out, as CoD4's
            // `isalive`: `guy_unload` deletes the riders of a dead one.)
            let vehicle = kind == Kind::Vehicle;
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
                shape: None,
                radius,
                spawnflags: num(get("spawnflags")).unwrap_or(0.0) as i32,
                team: String::new(),
                health: 100,
                max_health: 100,
                alive: vehicle,
                linked: None,
                move_pos: None,
                move_rot: None,
                enabled: true,
                hint: String::new(),
                touching: false,
                fired: false,
                goal: None,
                anim_state: AnimState::default(),
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
            // `count` is every entity's (0), an actor spawner's 1 unless
            // it says.
            // (So is `spawnflags`.)
            if !e.kv.iter().any(|(k, _)| k.eq_ignore_ascii_case("spawnflags")) {
                vm.set_field(o, "spawnflags", Value::Int(0));
            }
            if !e.kv.iter().any(|(k, _)| k.eq_ignore_ascii_case("count")) {
                let spawner = e.get("classname").is_some_and(|c| c.to_ascii_lowercase().starts_with("actor_"));
                vm.set_field(o, "count", Value::Int(spawner as i32));
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
                // (Every node has these, as the engine's.)
                if !kv.iter().any(|(k, _)| k.eq_ignore_ascii_case("angles")) {
                    vm.set_field(o, "angles", Value::Vector([0.0; 3]));
                }
                if !kv.iter().any(|(k, _)| k.eq_ignore_ascii_case("spawnflags")) {
                    vm.set_field(o, "spawnflags", Value::Int(0));
                }
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
            shape: None,
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
                anim_state: AnimState::default(),
            kv: Vec::new(),
        });
        self.finish_spawning(vm);
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
        // (Debug: `GSC_DELETE_TRACE=<name part or id>`: who removes it.)
        if let Ok(want) = std::env::var("GSC_DELETE_TRACE")
            && let Some(e) = self.ent(id)
            && (want == id.to_string() || [e.get("targetname"), e.get("script_noteworthy"), Some(e.classname.as_str())].iter().flatten().any(|n| n.contains(&want)))
        {
            eprintln!("gsc delete {id} ({}) at {:.2} by {}", e.classname, self.time, vm.current_stack());
        }
        if let Some(i) = self.by_id.remove(&id) {
            self.ents.swap_remove(i);
            if let Some(moved) = self.ents.get(i) {
                self.by_id.insert(moved.id, i);
            }
        }
        // What was linked to it is free (the player off a rope's mover).
        if self.player_link.as_ref().is_some_and(|l| l.to == id) {
            self.player_link = None;
        }
        for e in &mut self.ents {
            if e.linked.is_some_and(|(to, _, _)| to == id) {
                e.linked = None;
                e.anim_state.link_tag = None;
            }
        }
        vm.free_entity(id);
        self.commands.push(Command::Delete(id));
    }

    /// Advance movers and links to `now`; the player's position and the
    /// AI's come from the game ([`Level::set_pose`]) before this.
    pub fn update(&mut self, vm: &mut Vm, now: f64) {
        let dt = (now - self.time).clamp(0.0, 0.25) as f32;
        self.time = now;
        self.turn_turrets(vm, dt);
        for e in self.ents.iter().filter(|e| matches!(e.kind, Kind::Player | Kind::Ai)) {
            let v = self.velocity.entry(e.id).or_insert(([0.0; 3], e.origin, now));
            let dt = (now - v.2) as f32;
            if dt > 0.0 {
                v.0 = [0, 1, 2].map(|i| (e.origin[i] - v.1[i]) / dt);
                v.1 = e.origin;
                v.2 = now;
            }
        }
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
        // Animations: root motion, notetracks, ends.
        let mut notes: Vec<(EntId, String, String)> = Vec::new();
        let ids: Vec<EntId> = self.ents.iter().filter(|e| e.anim_state.anim.is_some()).map(|e| e.id).collect();
        for id in ids {
            self.pose_scripted(id, now);
            let Some(e) = self.ent_mut(id) else { continue };
            for a in [&mut e.anim_state.anim, &mut e.anim_state.overlay].into_iter().flatten() {
            let f = a.frac(now);
            let raw = a.from + ((now - a.start) as f32 * a.rate) / a.info.length.max(1e-3);
            // (A scripted one is done after a full round, looping or not,
            // as CoD's `animscripted`: `anim_loop` starts each round anew,
            // and when it's told to stop the actor is its own again.)
            let scripted_round = a.scripted.is_some() && a.info.looping && raw >= 1.0;
            if scripted_round && !a.ended {
                a.ended = true;
                if let Some(n) = a.notify.clone() {
                    while a.next_note < a.info.notes.len() {
                        notes.push((id, n.clone(), a.info.notes[a.next_note].1.clone()));
                        a.next_note += 1;
                    }
                    notes.push((id, n, "end".into()));
                }
                continue;
            }
            if scripted_round {
                continue;
            }
            if let Some(n) = a.notify.clone() {
                while a.next_note < a.info.notes.len() && a.info.notes[a.next_note].0 <= f {
                    notes.push((id, n.clone(), a.info.notes[a.next_note].1.clone()));
                    a.next_note += 1;
                }
                if a.info.looping && raw >= 1.0 {
                    // Round again: its notetracks again.
                    a.from = f;
                    a.start = now;
                    a.next_note = 0;
                    notes.push((id, n.clone(), "end".into()));
                } else if !a.info.looping && f >= 1.0 && !a.ended {
                    a.ended = true;
                    notes.push((id, n, "end".into()));
                }
            }
            }
        }
        for (id, n, note) in notes {
            if let Some(f) = &self.note_filter
                && (note.contains(f.as_str()) || n.contains(f.as_str()))
            {
                let model = self.ent(id).map(|e| e.model.clone()).unwrap_or_default();
                self.trace.push(format!("{now:.1}s ent {id} ({model}) notify {n:?} {note:?}"));
            }
            if let Some(o) = vm.entity_obj(id) {
                vm.notify(o, &n, vec![Value::str(&note)]);
            }
        }
        // Linked entities follow (one level deep is all the scripts use),
        // to a tag where the game has drawn it.
        // Twice: a chain (rig on the car, the car on its path) settles.
        for _ in 0..2 {
        let poses: HashMap<EntId, ([f32; 3], [f32; 3])> = self.ents.iter().map(|e| (e.id, (e.origin, e.angles))).collect();
        let tags: HashMap<(EntId, String), ([f32; 3], [f32; 3])> =
            self.ents.iter().flat_map(|e| e.anim_state.tags.keys().map(move |t| ((e.id, t.clone()), e.tag(t)))).collect();
        for e in &mut self.ents {
            let tagged = e.linked.and_then(|(to, _, _)| e.anim_state.link_tag.as_ref().and_then(|t| tags.get(&(to, t.clone())).copied()));
            let anchored = e.anim_state.anim.as_ref().is_some_and(|a| a.anchored && a.scripted.is_some());
            if let Some((to, offset, angles)) = e.linked.filter(|_| !anchored)
                && let Some((o, a)) = tagged.as_ref().or(poses.get(&to))
            {
                let (f, r, u) = crate::vm::angle_vectors(*a);
                e.origin = [
                    o[0] + f[0] * offset[0] - r[0] * offset[1] + u[0] * offset[2],
                    o[1] + f[1] * offset[0] - r[1] * offset[1] + u[1] * offset[2],
                    o[2] + f[2] * offset[0] - r[2] * offset[1] + u[2] * offset[2],
                ];
                e.angles = if old_links() { [a[0] + angles[0], a[1] + angles[1], a[2] + angles[2]] } else { compose_angles(*a, angles) };
            }
        }
        }
        let anchored: Vec<EntId> = self.ents.iter().filter(|e| e.anim_state.anim.as_ref().is_some_and(|a| a.anchored)).map(|e| e.id).collect();
        for id in anchored {
            self.pose_scripted(id, now);
        }
        for (id, ev) in done {
            if let Some(o) = vm.entity(id).as_obj() {
                vm.notify(o, ev, Vec::new());
            }
        }
        self.drive_vehicles(vm, now);
        if self.think_ai {
            self.think_actors(vm, now);
        }
        self.fly_missiles(vm, now);
        self.fuse_grenades(vm, now);
        // Sounds ended.
        if std::env::var_os("GSC_SOUND_TRACE").is_some() && (now * 1.0).floor() != ((now - 0.05) * 1.0).floor() {
            for s in &self.sound_notifies {
                eprintln!("gsc sound pending on {} for {} at {:.2} (now {now:.2})", s.1, s.2, s.0);
            }
        }
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.sound_notifies).into_iter().partition(|s| s.0 <= now);
        self.sound_notifies = waiting;
        for (_, id, n) in done {
            if std::env::var_os("GSC_SOUND_TRACE").is_some() {
                eprintln!("gsc sound on {id} at {now:.2}: {n} (object {:?})", vm.entity_obj(id));
            }
            if let Some(o) = vm.entity_obj(id) {
                vm.notify(o, &n, Vec::new());
                if std::env::var_os("GSC_SOUND_TRACE").is_some() && vm.waiting_on(o, &n) {
                    eprintln!("gsc sound on {id}: still waited on after its notify! {:?}", vm.thread_report().iter().filter(|t| t.contains(&n)).collect::<Vec<_>>());
                }
            }
        }
        // Missiles' flights over.
        let (over, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.timed_deletes).into_iter().partition(|(_, t)| *t <= now);
        self.timed_deletes = keep;
        for (id, _) in over {
            if let Some(o) = vm.entity_obj(id) {
                vm.notify(o, "death", Vec::new());
            }
            self.remove(vm, id);
        }
        self.touch_triggers(vm);
    }

    /// Triggers the player is in fire.
    fn touch_triggers(&mut self, vm: &mut Vm) {
        let player = self.ent(PLAYER).map(|p| p.origin).unwrap_or_default();
        // Walked over: grenades, and ammo for a gun they carry (CoD4's
        // touch pickups).
        let carried = self.player_weapons.clone();
        let grenade = |w: &str| self.weapons.get(w).is_some_and(|i| i.class == "grenade");
        if self.player_link.is_none()
            && let Some(item) = self.item_near(player, 40.0, |w| grenade(w) || carried.iter().any(|c| c == w))
        {
            self.pick_up(vm, item);
        }
        // Actors set off the triggers their spawnflags let them (1 axis,
        // 2 allies, 4 neutral); 8 keeps the player out.
        let actors: Vec<(EntId, [f32; 3], i32)> = self
            .ents
            .iter()
            .filter(|e| e.kind == Kind::Ai && e.alive)
            .map(|e| (e.id, e.origin, match e.team.as_str() { "axis" => 1, "allies" => 2, "neutral" => 4, _ => 0 }))
            .collect();
        let mut fired: Vec<(EntId, EntId)> = Vec::new();
        for e in &mut self.ents {
            if !e.is_trigger() || !e.enabled || matches!(e.classname.as_str(), "trigger_use" | "trigger_damage" | "trigger_hurt") {
                continue;
            }
            let player_ok = e.spawnflags & 8 == 0;
            let inside = player_ok && (e.contains(player) || e.contains([player[0], player[1], player[2] + 40.0]));
            let by = if inside {
                Some(PLAYER)
            } else if e.spawnflags & 7 != 0 && e.classname != "trigger_lookat" {
                actors.iter().find(|(_, at, side)| e.spawnflags & side != 0 && (e.contains(*at) || e.contains([at[0], at[1], at[2] + 40.0]))).map(|a| a.0)
            } else {
                None
            };
            if let Some(by) = by
                && !(e.classname == "trigger_once" && e.fired)
            {
                fired.push((e.id, by));
                e.fired = true;
            }
            e.touching = inside;
        }
        for (id, by) in fired {
            let who = vm.entity(by);
            if let Some(o) = vm.entity(id).as_obj() {
                vm.notify(o, "trigger", vec![who]);
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

    /// The nearest `weapon_*` item within `reach` of the player's feet (or
    /// waist) that `want`s its weapon name.
    fn item_near(&self, at: [f32; 3], reach: f32, want: impl Fn(&str) -> bool) -> Option<EntId> {
        self.ents
            .iter()
            .filter(|e| e.kind == Kind::Map && e.classname.starts_with("weapon_") && want(&e.classname[7..].to_ascii_lowercase()))
            .map(|e| (e.id, (0..3).map(|i| (e.origin[i] - at[i] - if i == 2 { 30.0 } else { 0.0 }).powi(2)).sum::<f32>().sqrt()))
            .filter(|(_, d)| *d <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// The player takes a `weapon_*` item: its weapon (or its ammo) theirs,
    /// switched to if new and no grenade; the item told `trigger` and gone.
    fn pick_up(&mut self, vm: &mut Vm, item: EntId) {
        let Some(w) = self.ent(item).map(|e| e.classname[7..].to_ascii_lowercase()) else { return };
        let info = self.weapons.get(&w).cloned().unwrap_or_default();
        let new = !self.player_weapons.contains(&w);
        if new {
            self.player_weapons.push(w.clone());
            self.player_ammo.insert(w.clone(), info.clip_size);
        }
        // (A grenade lying there is one grenade; a gun, its rounds.)
        let stock = self.player_stock.entry(w.clone()).or_insert(0);
        let add = if info.class == "grenade" { 1 } else { info.clip_size.max(1) * 3 };
        *stock = (*stock + add).min(info.max_ammo.max(info.clip_size).max(add));
        let gun = Value::str(&w);
        // (Debug: `GSC_PICKUP_TRACE=1`: what was picked up, and how much.)
        if std::env::var_os("GSC_PICKUP_TRACE").is_some() {
            eprintln!("gsc pickup {w} at {:.1}: stock {:?} (max {}, clip {})", self.time, self.player_stock.get(&w), info.max_ammo, info.clip_size);
        }
        self.forward(Some(PLAYER), "giveweapon", std::slice::from_ref(&gun));
        if new && info.class != "grenade" {
            self.player_current = Some(w.clone());
            self.forward(Some(PLAYER), "switchtoweapon", std::slice::from_ref(&gun));
        }
        if let Some(o) = vm.entity_obj(item) {
            let me = vm.entity(PLAYER);
            vm.notify(o, "trigger", vec![me]);
        }
        self.remove(vm, item);
    }

    /// Triggers that set a flag a script is waiting on (`flag_wait`), or
    /// that a script waits to see touched: their middles (a test driver's
    /// next stop).
    pub fn awaited_flag_triggers(&self, vm: &Vm) -> Vec<[f32; 3]> {
        let level = vm.level;
        self.ents
            .iter()
            .filter(|e| e.is_trigger() && e.enabled && e.spawnflags & 8 == 0 && !matches!(e.classname.as_str(), "trigger_use" | "trigger_damage" | "trigger_hurt"))
            .filter(|e| {
                e.get("script_flag").is_some_and(|f| vm.waiting_on(level, f))
                    // (Or one a script waits to see touched, but not a
                    // failure's: the wrong way, a death.)
                    // (Not the soundscape's, always waited on.)
                    || (vm.entity_obj(e.id).is_some_and(|o| vm.waiting_on(o, "trigger"))
                        && e.get("ambient").is_none()
                        && !["wrong", "kill", "death", "fail", "nojump", "hurt"].iter().any(|w| e.get("targetname").is_some_and(|n| n.contains(w))))
            })
            .map(|e| match e.bounds {
                Some((mn, mx)) => [e.origin[0] + (mn[0] + mx[0]) * 0.5, e.origin[1] + (mn[1] + mx[1]) * 0.5, e.origin[2] + mn[2] + 1.0],
                None => e.origin,
            })
            .collect()
    }

    /// Where the damageable script brushes are (their bounds' middles),
    /// for a test's aim.
    pub fn damageable_points(&self, vm: &Vm) -> Vec<[f32; 3]> {
        self.ents
            .iter()
            .filter(|e| self.damageable.contains(&e.id) || e.classname == "trigger_damage" && e.enabled)
            // (Ones a script waits to see hit.)
            .filter(|e| vm.entity_obj(e.id).is_some_and(|o| vm.waiting_on(o, "damage") || vm.waiting_on(o, "trigger")))
            .filter_map(|e| {
                let (mn, mx) = e.bounds?;
                Some(local_to_world(e.origin, e.angles, [(mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, (mn[2] + mx[2]) * 0.5]))
            })
            .collect()
    }

    /// A bullet flew from `from` to `to`, shot by someone of `team`: the
    /// other side's actors it passed within 64 units of the head of (not
    /// the one it hit) are suppressed for 2 s (CoD4's suppression, which
    /// keeps an animscript in cover).
    pub fn bullet_passed(&mut self, from: [f32; 3], to: [f32; 3], team: &str) {
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let len2 = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).max(1e-6);
        let until = self.time + 2.0;
        for e in self.ents.iter().filter(|e| e.kind == Kind::Ai && e.alive && !e.team.is_empty() && e.team != team) {
            let head = [e.origin[0], e.origin[1], e.origin[2] + 56.0];
            let k = (((head[0] - from[0]) * d[0] + (head[1] - from[1]) * d[1] + (head[2] - from[2]) * d[2]) / len2).clamp(0.0, 1.0);
            let near = [from[0] + d[0] * k, from[1] + d[1] * k, from[2] + d[2] * k];
            let dist = ((near[0] - head[0]).powi(2) + (near[1] - head[1]).powi(2) + (near[2] - head[2]).powi(2)).sqrt();
            if k < 0.98 && dist < 64.0 {
                self.suppressed.insert(e.id, until);
            }
        }
    }

    /// The player's bullet from `from` to `to` (where it stopped in the
    /// world, or its range): the first damageable script brush it passes
    /// through (`setcandamage`, or a `trigger_damage`), by its true shape
    /// turned with the entity, is told `damage` (and a trigger, `trigger`).
    pub fn player_shot(&mut self, vm: &mut Vm, from: [f32; 3], to: [f32; 3], amount: i32) {
        self.player_hit(vm, from, to, amount, "MOD_RIFLE_BULLET");
    }

    /// The player's melee (`+melee`): a stab from the eye `dir`ward, 64
    /// units (CoD4's `player_meleeRange`), on the same script brushes.
    pub fn player_melee(&mut self, vm: &mut Vm, eye: [f32; 3], dir: [f32; 3]) {
        let to = [eye[0] + dir[0] * 64.0, eye[1] + dir[1] * 64.0, eye[2] + dir[2] * 64.0];
        self.player_hit(vm, eye, to, 135, "MOD_MELEE");
    }

    fn player_hit(&mut self, vm: &mut Vm, from: [f32; 3], to: [f32; 3], amount: i32, cause: &str) {
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let mut best: Option<(f32, EntId)> = None;
        // (CoD4's means of death, numbered as `Respond_trigger_damage`
        // reads them against a trigger's spawnflags: 1 no pistol, 2 no
        // rifle, 4 no projectiles, 8 no 14, 16 no splash, 32 no melee.)
        let mod_n = match cause {
            "MOD_PISTOL_BULLET" => 1,
            "MOD_MELEE" => 7,
            _ => 2,
        };
        let responds = |sf: i32| {
            (sf & 1 == 0 || mod_n != 1)
                && (sf & 2 == 0 || mod_n != 2)
                && (sf & 4 == 0 || !(3..=6).contains(&mod_n))
                && (sf & 0x10 == 0 || mod_n != 4 && mod_n != 6)
                && (sf & 0x20 == 0 || mod_n != 7)
                && (sf & 0x100 == 0 || mod_n != 0 && (mod_n <= 8 || mod_n > 13))
        };
        for e in &self.ents {
            let trigger = e.classname == "trigger_damage" && e.enabled && responds(e.spawnflags);
            if !(trigger || self.damageable.contains(&e.id)) {
                continue;
            }
            let Some(shape) = &e.shape else { continue };
            let p0 = world_to_local(e.origin, e.angles, from);
            let p1 = world_to_local(e.origin, e.angles, to);
            let dl = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            for planes in shape.iter() {
                let (mut enter, mut exit) = (0.0_f32, 1.0_f32);
                let mut miss = false;
                for (n, dist) in planes {
                    let start = n[0] * p0[0] + n[1] * p0[1] + n[2] * p0[2] - dist;
                    let along = n[0] * dl[0] + n[1] * dl[1] + n[2] * dl[2];
                    if along.abs() < 1e-6 {
                        if start > 0.0 {
                            miss = true;
                            break;
                        }
                    } else {
                        let t = -start / along;
                        if along < 0.0 { enter = enter.max(t) } else { exit = exit.min(t) }
                    }
                }
                if !miss && enter <= exit && best.is_none_or(|b| enter < b.0) {
                    best = Some((enter, e.id));
                }
            }
        }
        // (Debug: `GSC_SHOT_TRACE=1`: each shot, and what it hit.)
        if std::env::var_os("GSC_SHOT_TRACE").is_some() {
            let near: Vec<String> = self
                .ents
                .iter()
                .filter(|e| self.damageable.contains(&e.id) && e.shape.is_some())
                .filter(|e| (0..3).map(|i| (e.origin[i] - to[i]).powi(2)).sum::<f32>() < 300.0 * 300.0)
                .map(|e| {
                    let l = world_to_local(e.origin, e.angles, to);
                    let planes: Vec<String> = e.shape.as_ref().map(|s| s.iter().take(1).flat_map(|b| b.iter().map(|(n, d)| format!("{:?}{:.0}", n.map(|x| (x * 10.0).round() / 10.0), d))).collect()).unwrap_or_default();
                    format!("{} {:?} {:?} bounds {:?} to-local {:?} planes {planes:?}", e.id, e.origin.map(f32::round), e.angles.map(f32::round), e.bounds, l.map(f32::round))
                })
                .collect();
            eprintln!("gsc shot {:?} -> {:?}: {:?} of {near:?}", from.map(f32::round), to.map(f32::round), best);
        }
        let Some((t, id)) = best else { return };
        let point = [from[0] + d[0] * t, from[1] + d[1] * t, from[2] + d[2] * t];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(0.001);
        let dir = [d[0] / len, d[1] / len, d[2] / len];
        let Some(o) = vm.entity_obj(id) else { return };
        let me = vm.entity(PLAYER);
        vm.notify(o, "damage", vec![Value::Int(amount), me.clone(), Value::Vector(dir), Value::Vector(point), Value::str(cause)]);
        if let Some(sf) = self.ent(id).filter(|e| e.classname == "trigger_damage").map(|e| e.spawnflags) {
            vm.notify(o, "trigger", vec![me]);
            // (0x200: gone once triggered.)
            if sf & 0x200 != 0 {
                self.remove(vm, id);
            }
        }
    }

    /// A sound with a notify plays for `seconds` from now (the game's word).
    pub fn sound_ends(&mut self, ent: EntId, notify: &str, seconds: f32) {
        let time = self.time;
        if let Some(s) = self.sound_notifies.iter_mut().rev().find(|s| s.1 == ent && s.2 == notify) {
            // (No sound is a minute long: a bad length can't hold a script.)
            s.0 = time + seconds.clamp(0.05, 60.0) as f64;
        }
    }

    /// The player threw a grenade (`weapon`, from `origin`): a "grenade"
    /// entity the scripts can follow, one fewer carried, and the player told
    /// `grenade_fire` (grenade, weapon). [`Level::grenade_moved`] and
    /// [`Level::grenade_gone`] keep it as the game's.
    pub fn player_grenade(&mut self, vm: &mut Vm, weapon: &str, origin: [f32; 3]) -> EntId {
        let id = self.add(blank("grenade", origin));
        if let Some(n) = self.player_stock.get_mut(weapon) {
            *n = (*n - 1).max(0);
        }
        let g = vm.entity(id);
        if let Some(o) = vm.entity_obj(PLAYER) {
            vm.notify(o, "grenade_fire", vec![g, Value::str(weapon)]);
        }
        id
    }

    pub fn grenade_moved(&mut self, id: EntId, origin: [f32; 3]) {
        if let Some(e) = self.ent_mut(id) {
            e.origin = origin;
        }
    }

    /// It went off: told `death`, and gone.
    pub fn grenade_gone(&mut self, vm: &mut Vm, id: EntId) {
        if let Some(o) = vm.entity_obj(id) {
            vm.notify(o, "death", Vec::new());
        }
        self.remove(vm, id);
    }

    /// A script menu the player answered (`scriptMenuResponse`): they're
    /// told `menuresponse` (menu, response).
    pub fn menu_response(&mut self, vm: &mut Vm, menu: &str, response: &str) {
        if let Some(o) = vm.entity_obj(PLAYER) {
            vm.notify(o, "menuresponse", vec![Value::str(menu), Value::str(response)]);
        }
    }

    /// The player used a command (`+attack`, `weapnext`, `+activate`...):
    /// what `notifyoncommand` bound to it is told.
    pub fn player_command(&mut self, vm: &mut Vm, command: &str) {
        let Some(list) = self.command_notifies.get(command).cloned() else { return };
        if let Some(o) = vm.entity_obj(PLAYER) {
            for n in list {
                vm.notify(o, &n, Vec::new());
            }
        }
    }

    /// The player pressed Use: the use trigger they're in fires.
    pub fn use_pressed(&mut self, vm: &mut Vm) {
        let player = self.ent(PLAYER).map(|p| p.origin).unwrap_or_default();
        // In a use trigger: that's what Use does (its hint is the one
        // shown), before a gun lying near.
        let hit: Vec<EntId> = self
            .ents
            .iter()
            .filter(|e| e.classname == "trigger_use" && e.enabled && (e.contains(player) || e.contains([player[0], player[1], player[2] + 40.0])))
            .map(|e| e.id)
            .collect();
        if self.player_link.is_none() && !hit.is_empty() {
            let me = vm.entity(PLAYER);
            for id in hit {
                if let Some(o) = vm.entity(id).as_obj() {
                    vm.notify(o, "trigger", vec![me.clone()]);
                }
            }
            return;
        }
        // A weapon lying in reach: picked up (CoD4's Use on an item).
        if self.player_link.is_none()
            && let Some(item) = self.item_near(player, 64.0, |_| true)
        {
            self.pick_up(vm, item);
            return;
        }
        // Off a mounted gun; or onto one in reach.
        if let Some(l) = self.player_link.as_ref().filter(|l| l.turret)
            && self.ent(l.to).is_some_and(|e| e.classname == "misc_turret")
        {
            self.player_link = None;
            return;
        }
        if self.player_link.is_none()
            && let Some(t) = self.usable_turret(vm, player)
        {
            let num = |k: &str, d: f32| vm.entity_obj(t).and_then(|o| vm.field(o, k).as_f32()).unwrap_or(d);
            let weapon = vm.entity_obj(t).map(|o| vm.field(o, "weaponinfo").display().to_ascii_lowercase()).unwrap_or_default();
            self.vehicle_weapons.insert(t, weapon);
            // The view is the gun's tag_player (as CoD4's view on a turret).
            if let Some(e) = self.ent_mut(t)
                && !e.anim_state.wanted.iter().any(|w| w == "tag_player")
            {
                e.anim_state.wanted.push("tag_player".to_owned());
            }
            self.player_link = Some(PlayerLink {
                to: t,
                tag: Some("tag_player".to_owned()),
                arcs: [num("rightarc", 60.0), num("leftarc", 60.0), num("toparc", 30.0), num("bottomarc", 30.0)],
                delta: false,
                turret: true,
                offset: [0.0; 3],
            });
            return;
        }
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

    /// A mounted gun the player can take, in reach (48 units flat).
    fn usable_turret(&self, vm: &Vm, at: [f32; 3]) -> Option<EntId> {
        self.ents
            .iter()
            .filter(|e| e.classname == "misc_turret" && !e.hidden && !self.unusable_turrets.contains(&e.id))
            .filter(|e| vm.entity_obj(e.id).is_none_or(|o| !vm.field(o, "weaponinfo").display().is_empty()))
            .find(|e| (e.origin[0] - at[0]).hypot(e.origin[1] - at[1]) < 48.0 && (e.origin[2] - at[2]).abs() < 80.0)
            .map(|e| e.id)
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
        self.damaged_at(vm, id, amount, attacker, dir, at, "torso_upper");
    }

    /// Damage where it hit (CoD4's location names: `head`, `torso_upper`,
    /// `left_leg_upper`...): an actor's engine fields for its pain and
    /// death scripts (`damageTaken`, `damageyaw`, `damagelocation`).
    #[allow(clippy::too_many_arguments)]
    pub fn damaged_at(&mut self, vm: &mut Vm, id: EntId, amount: i32, attacker: Option<EntId>, dir: [f32; 3], at: [f32; 3], location: &str) {
        if let Some(e) = self.ent(id)
            && e.kind == Kind::Ai
            && let Some(o) = vm.entity_obj(id)
        {
            // The way the shot went against the way it faces.
            let yaw = if dir[0] == 0.0 && dir[1] == 0.0 { 180.0 } else { dir[1].atan2(dir[0]).to_degrees() };
            let rel = (yaw - e.angles[1] + 540.0).rem_euclid(360.0) - 180.0;
            vm.set_field(o, "damagetaken", Value::Int(amount));
            vm.set_field(o, "damageyaw", Value::Float(rel));
            vm.set_field(o, "damagelocation", Value::str(location));
            vm.set_field(o, "damagemod", Value::str("MOD_RIFLE_BULLET"));
            vm.set_field(o, "damagedir", Value::Vector(dir));
        }
        // Pain, as the engine's: the animscript is cut off for `pain`
        // (not under a scripted animation, not when it says no).
        let alive_after = self.ent(id).is_some_and(|e| e.alive && e.health - amount > 0 && e.kind == Kind::Ai);
        let scripted = self.ent(id).and_then(|e| e.anim_state.anim.as_ref()).is_some_and(|a| a.scripted.is_some() && !a.ended);
        if alive_after
            && !scripted
            && self.think_ai
            && let Some(o) = vm.entity_obj(id)
            && vm.field(o, "allowpain").as_i32().unwrap_or(1) != 0
        {
            let b = self.brains.entry(id).or_default();
            if !b.pain {
                b.pain = true;
                b.script.clear();
                vm.notify(o, "killanimscript", Vec::new());
            }
        }
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
    // (CoD4's start points run at the end of the first frame, after the
    // player has touched the triggers they begin in: the squad's spawn
    // triggers, at War Room's vents.)
    fn before_frame_end(&mut self, vm: &mut Vm) {
        if !self.first_touch {
            self.first_touch = true;
            self.touch_triggers(vm);
        }
    }

    fn call(&mut self, vm: &mut Vm, name: &str, this: Option<&Value>, args: &[Value]) -> Result<Value, String> {
        let s = |i: usize| args.get(i).and_then(Value::as_str).unwrap_or_default().to_owned();
        let f = |i: usize| args.get(i).and_then(Value::as_f32).unwrap_or(0.0);
        let v = |i: usize| args.get(i).and_then(Value::as_vec).unwrap_or_default();
        let me = self.this_ent(vm, this);
        let now = self.time;
        Ok(match name {
            // ---- Finding things ----
            // (Vehicle path nodes aren't entities to these: `getvehiclenode`'s.)
            "getent" => {
                let found: Vec<EntId> = self.matching(vm, &s(0), &s(1)).into_iter().filter(|id| !self.is_vehicle_node(*id)).collect();
                found.first().map_or(Value::Undefined, |&id| vm.entity(id))
            }
            "getentarray" => {
                if args.is_empty() {
                    let ids: Vec<EntId> = self.ents.iter().filter(|e| e.kind != Kind::Player && !e.classname.contains("vehicle_node")).map(|e| e.id).collect();
                    Value::array(ids.into_iter().map(|id| vm.entity(id)))
                } else {
                    let found: Vec<EntId> = self.matching(vm, &s(0), &s(1)).into_iter().filter(|id| !self.is_vehicle_node(*id)).collect();
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
            // The map's sun colour (r, g, b), as an array the scripts step.
            "getmapsunlight" => {
                let c = vec3(self.dvars.get("r_lighttweaksuncolor").map(String::as_str)).unwrap_or([1.0; 3]);
                Value::array(c.into_iter().map(Value::Float))
            }
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
                Value::Float(self.anims.info(&n).map_or(1.0, |i| i.length))
            }
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
            "setblur" => {
                self.commands.push(Command::Blur { radius: f(0), time: f(1) });
                Value::Undefined
            }
            "shellshock" | "stopshellshock" => {
                let (name, duration) = if name == "shellshock" { (s(0), f(1)) } else { (String::new(), 0.0) };
                self.commands.push(Command::Shellshock { name, duration });
                Value::Undefined
            }
            "visionsetnaked" | "visionsetnight" => {
                self.commands.push(Command::Vision { name: s(0), time: f(1) });
                Value::Undefined
            }
            "setvolfog" | "setdepthoffield" | "setviewmodeldepthoffield" | "settimescale" => Value::Undefined,
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
            // (fx, delay, origin, cull distance, forward, up)
            "playloopedfx" => {
                let fx = self.fx.get(f(0) as usize).cloned().unwrap_or_default();
                let at = v(2);
                let id = self.add(blank("fx_looped", at));
                let forward = args.get(4).and_then(Value::as_vec).unwrap_or([1.0, 0.0, 0.0]);
                self.commands.push(Command::LoopFx { ent: id, fx, at, forward, delay: f(1) });
                vm.entity(id)
            }
            // (fx, origin, forward, up): started by `triggerfx`.
            "spawnfx" => {
                let fx = self.fx.get(f(0) as usize).cloned().unwrap_or_default();
                let mut e = blank("fx_spawned", v(1));
                e.model = String::new();
                let forward = args.get(2).and_then(Value::as_vec).unwrap_or([1.0, 0.0, 0.0]);
                let id = self.add(e);
                self.spawned_fx.insert(id, (fx, forward));
                vm.entity(id)
            }
            "triggerfx" => {
                if let Some(id) = self.arg_ent(vm, args, 0)
                    && let Some((fx, forward)) = self.spawned_fx.get(&id).cloned()
                {
                    let at = self.ent(id).map_or([0.0; 3], |e| e.origin);
                    self.commands.push(Command::LoopFx { ent: id, fx, at, forward, delay: 0.0 });
                }
                Value::Undefined
            }
            "earthquake" => {
                self.commands.push(Command::Earthquake { scale: f(0), time: f(1), at: v(2), radius: f(3) });
                Value::Undefined
            }
            "physicsexplosionsphere" | "physicsjolt" | "physicsjitter" | "radiusdamage" | "magicbullet" | "badplace_cylinder" | "badplace_arc" | "badplace_delete" | "setignoremegroup" | "setthreatbias" | "clearallcorpses" | "setphysicsgravitydir" | "setsaveddvarif" => {
                self.commands.push(Command::Call { ent: 0, name: name.to_owned(), args: args.to_vec() });
                Value::Undefined
            }
            // An actor's grenade (its `grenadeweapon`) set down at a point
            // (`magicgrenademanual(origin, velocity, fuse ms)`) or landing
            // at one (`magicgrenade(from, to, fuse s)`): a "grenade" there
            // until its fuse, then its blast. (A claymore stays: it goes off
            // by its own trigger.)
            "magicgrenade" | "magicgrenademanual" => {
                let weapon = me.and_then(|id| vm.entity_obj(id)).map(|o| vm.field(o, "grenadeweapon").display().to_ascii_lowercase()).filter(|w| !w.is_empty() && w != "undefined").unwrap_or_else(|| "fraggrenade".into());
                let (at, fuse) = if name == "magicgrenade" { (v(1), args.get(2).and_then(Value::as_f32).unwrap_or(3.0)) } else { (v(0), args.get(2).and_then(Value::as_f32).map_or(3.0, |ms| ms / 1000.0)) };
                let mut e = blank("grenade", at);
                e.model = self.weapons.get(&weapon).map(|w| w.world_model.clone()).unwrap_or_default();
                let id = self.add(e);
                self.grenades.push((id, self.time + fuse as f64, weapon, me));
                Value::Undefined
            }
            // ---- An actor's grenade throw (`animscripts\combat_utility`'s
            // TryGrenadeThrow): a velocity that lands it there, kept for its
            // `throwGrenade()` (the game throws it).
            "checkgrenadethrow" | "checkgrenadethrowpos" => {
                let Some(id) = me else { return Ok(Value::Undefined) };
                let Some(e) = self.ent(id) else { return Ok(Value::Undefined) };
                let arm = v(0);
                let from = [e.origin[0] + arm[0], e.origin[1] + arm[1], e.origin[2] + arm[2]];
                let target = if name == "checkgrenadethrowpos" {
                    Some(v(2))
                } else {
                    // At its enemy, somewhere within the given range of it.
                    let range = f(2).max(0.0);
                    let enemy = vm.entity_obj(id).map(|o| vm.field(o, "enemy")).and_then(|v| vm.entity_of(&v)).and_then(|t| self.ent(t)).map(|t| t.origin);
                    enemy.map(|t| {
                        let a = vm.random_float() * std::f32::consts::TAU;
                        let r = vm.random_float() * range;
                        [t[0] + a.cos() * r, t[1] + a.sin() * r, t[2]]
                    })
                };
                let Some(to) = target else { return Ok(Value::Undefined) };
                let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
                let flat = d[0].hypot(d[1]);
                // Out of an arm's reach (CoD4 throws to ~1000 units) or right
                // at its feet: no throw.
                if !(200.0..=1250.0).contains(&flat) {
                    return Ok(Value::Undefined);
                }
                const GRAVITY: f32 = 800.0;
                let base = (2.0 * (flat * flat + d[2] * d[2]).sqrt() / GRAVITY).sqrt();
                let t = base * match s(1).as_str() {
                    "min time" => 0.8,
                    "max time" => 1.3,
                    _ => 1.0,
                };
                let vel = [d[0] / t, d[1] / t, (d[2] + 0.5 * GRAVITY * t * t) / t];
                if (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt() > 1100.0 {
                    return Ok(Value::Undefined);
                }
                self.throws.insert(id, (from, vel));
                Value::Vector(vel)
            }
            "throwgrenade" => {
                if let Some(id) = me
                    && let Some((from, vel)) = self.throws.remove(&id)
                {
                    self.commands.push(Command::Call { ent: id, name: "throwgrenade".into(), args: vec![Value::Vector(from), Value::Vector(vel)] });
                }
                Value::Undefined
            }
            "getthreatbias" => Value::Int(0),
            // The world's collision, when the game lends it ([`with_tracer`]):
            // headless, nothing in the way.
            "bullettrace" => {
                let (from, to) = (v(0), v(1));
                let hit = trace(from, to, TraceKind::Bullet);
                let (fraction, position, normal) = hit.map_or((1.0, to, [0.0, 0.0, 0.0]), |h| (h.0, h.1, h.2));
                // CoD's is an array keyed by name.
                let mut a = crate::vm::Array::default();
                a.set(crate::vm::Key::Str("fraction".into()), Value::Float(fraction));
                a.set(crate::vm::Key::Str("position".into()), Value::Vector(position));
                a.set(crate::vm::Key::Str("normal".into()), Value::Vector(normal));
                a.set(crate::vm::Key::Str("surfacetype".into()), Value::str(if hit.is_some() { "default" } else { "none" }));
                Value::Array(Rc::new(a))
            }
            "bullettracepassed" => Value::Int(i32::from(trace(v(0), v(1), TraceKind::Bullet).is_none())),
            "sighttracepassed" => Value::Int(i32::from(trace(v(0), v(1), TraceKind::Sight).is_none())),
            "physicstrace" | "playerphysicstrace" => Value::Vector(trace(v(0), v(1), TraceKind::Physics).map_or(v(1), |h| h.1)),
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
            "setshader" | "fadeovertime" | "moveovertime" | "scaleovertime" | "setpulsefx" if this.and_then(Value::as_obj).is_some_and(|o| self.hud.contains_key(&o)) => {
                let o = this.and_then(Value::as_obj).unwrap();
                let num = |i: usize| args.get(i).and_then(Value::as_f32).unwrap_or(0.0);
                let fnum = |n: &str| vm.field(o, n).as_f32();
                let now = self.time;
                let (alpha, color, x, y) = (fnum("alpha").unwrap_or(1.0), vm.field(o, "color").as_vec().unwrap_or([1.0; 3]), fnum("x").unwrap_or(0.0), fnum("y").unwrap_or(0.0));
                let shown = self.hud_alpha_color(o, alpha, color);
                let at = self.hud_xy(o, x, y);
                let h = self.hud.get_mut(&o).unwrap();
                match name {
                    "setshader" => {
                        h.shader = Some((args.first().map(Value::display).unwrap_or_default(), num(1), num(2)));
                        h.scaling = None;
                    }
                    "fadeovertime" => h.fade = Some((now, num(0).max(0.0), shown.0, shown.1)),
                    "moveovertime" => h.moving = Some((now, num(0).max(0.0), at.0, at.1)),
                    "scaleovertime" => {
                        let (w0, h0) = h.shader.as_ref().map_or((0.0, 0.0), |s| (s.1, s.2));
                        h.scaling = Some((now, num(0).max(0.0), w0, h0));
                        if let Some(sh) = &mut h.shader {
                            (sh.1, sh.2) = (num(1), num(2));
                        }
                    }
                    _ => h.pulse = Some((now, num(0), num(1), num(2))),
                }
                Value::Undefined
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
            // (model, targetname, vehicle type, origin, angles)
            "spawnvehicle" => {
                let mut e = blank("script_vehicle", v(3));
                e.kind = Kind::Vehicle;
                e.alive = true;
                e.model = args.first().map(Value::display).unwrap_or_default();
                e.angles = v(4);
                let id = self.add(e);
                let o = vm.entity(id);
                if let Some(obj) = o.as_obj() {
                    vm.set_field(obj, "targetname", args.get(1).cloned().unwrap_or_default());
                    vm.set_field(obj, "vehicletype", args.get(2).cloned().unwrap_or_default());
                    vm.set_field(obj, "classname", Value::str("script_vehicle"));
                }
                o
            }
            "spawnturret" => {
                let id = self.add(blank(name, v(1)));
                vm.entity(id)
            }
            "getentnum" | "getentitynumber" => Value::Int(me.unwrap_or(0) as i32),
            // (Told "death" first, as CoD4's ScrCmd_Delete: its death
            // threads run after it's freed, `self` undefined to them, an
            // actor's squad letting it go, a vehicle's riders deleted.)
            "delete" => {
                if let Some(id) = me {
                    if id != PLAYER
                        && let Some(o) = vm.entity_obj(id)
                    {
                        vm.notify(o, "death", Vec::new());
                    }
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
                let tag = args.get(1).and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_ascii_lowercase);
                if let (Some(to), Some(t)) = (to, &tag)
                    && let Some(p) = self.ent_mut(to)
                    && !p.anim_state.wanted.contains(t)
                {
                    p.anim_state.wanted.push(t.clone());
                }
                if let (Some(id), Some(to)) = (me, to) {
                    let (o, a) = self
                        .ent(to)
                        .map(|t| match &tag {
                            Some(tg) => t.tag(tg),
                            None => (t.origin, t.angles),
                        })
                        .unwrap_or_default();
                    let mine = self.ent(id).map(|e| (e.origin, e.angles)).unwrap_or_default();
                    // Keep where it is relative to the parent unless told.
                    let offset = args.get(2).and_then(Value::as_vec).unwrap_or_else(|| {
                        let d = [mine.0[0] - o[0], mine.0[1] - o[1], mine.0[2] - o[2]];
                        let (f, r, u) = crate::vm::angle_vectors(a);
                        [d[0] * f[0] + d[1] * f[1] + d[2] * f[2], -(d[0] * r[0] + d[1] * r[1] + d[2] * r[2]), d[0] * u[0] + d[1] * u[1] + d[2] * u[2]]
                    });
                    // (Its turn relative to the parent's, composed as
                    // rotations: a yawed parent pitching turns it right.)
                    let angles = args.get(3).and_then(Value::as_vec).unwrap_or_else(|| if old_links() { [mine.1[0] - a[0], mine.1[1] - a[1], mine.1[2] - a[2]] } else { relative_angles(a, mine.1) });
                    // (No offset given, it keeps where it is relative to the tag,
                    // as CoD's `linkto` does.)
                    if let Some(e) = self.ent_mut(id) {
                        e.linked = Some((to, offset, angles));
                        e.anim_state.link_tag = tag;
                    }
                }
                Value::Undefined
            }
            "unlink" => {
                // A scripted animation kept relative to the parent's tag
                // stays where it is in the world (riders getting out).
                let world = me.and_then(|id| self.ent(id)).and_then(|e| {
                    let a = e.anim_state.anim.as_ref().filter(|a| a.anchored)?;
                    let (org, ang) = a.scripted?;
                    let (to, _, _) = e.linked?;
                    let tag = e.anim_state.link_tag.clone();
                    let (po, pa) = self.ent(to).map(|p| tag.as_deref().map_or((p.origin, p.angles), |t| p.tag(t)))?;
                    Some((local_to_world(po, pa, org), [pa[0] + ang[0], pa[1] + ang[1], pa[2] + ang[2]]))
                });
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    if let Some(a) = e.anim_state.anim.as_mut().filter(|a| a.anchored) {
                        a.anchored = false;
                        if world.is_some() {
                            a.scripted = world;
                        }
                    }
                    e.linked = None;
                    e.anim_state.link_tag = None;
                }
                // (Not off a gun: using a turret isn't a link.)
                if me == Some(PLAYER) && !self.player_link.as_ref().is_some_and(|l| l.turret) {
                    self.player_link = None;
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
            "setcandamage" => {
                if let Some(id) = me {
                    if args.first().is_some_and(Value::truthy) {
                        self.damageable.insert(id);
                    } else {
                        self.damageable.remove(&id);
                    }
                }
                Value::Undefined
            }
            "enablelinkto" | "setcontents" | "setshadowhint" | "laseron" | "laseroff" | "useanimtree" | "dontinterpolate" | "setlightintensity" | "setlightcolor" | "setlightradius" => Value::Undefined,
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
            // A vehicle used by the player: on its gunner's seat (`tag_player`),
            // free to turn its gun.
            "useby" if me.and_then(|id| self.ent(id)).is_some_and(|e| e.kind == Kind::Vehicle) && self.arg_ent(vm, args, 0) == Some(PLAYER) => {
                let id = me.unwrap_or_default();
                let tag = "tag_player".to_owned();
                if let Some(v) = self.ent_mut(id)
                    && !v.anim_state.wanted.contains(&tag)
                {
                    v.anim_state.wanted.push(tag.clone());
                }
                self.player_link = Some(PlayerLink { to: id, tag: Some(tag), arcs: [180.0, 180.0, 60.0, 60.0], delta: false, turret: true, offset: [0.0; 3] });
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
                // `playsound( alias, notify )`: told when it ends (the next
                // frame unless the game knows its length).
                let notify = (name != "playloopsound" && args.len() > 1).then(|| s(1)).filter(|n| !n.is_empty());
                if let (Some(n), Some(id)) = (&notify, me) {
                    if std::env::var_os("GSC_SOUND_TRACE").is_some() {
                        eprintln!("gsc sound {} on {id} at {:.2}: {n} expected", s(0), self.time);
                    }
                    self.sound_notifies.push((self.time + 0.05, id, n.clone()));
                }
                self.commands.push(Command::PlaySound { ent: me, alias: s(0), at, looped: name == "playloopsound", notify });
                Value::Undefined
            }
            "stoploopsound" | "stopsounds" => {
                if let Some(id) = me {
                    self.commands.push(Command::StopSound(id));
                }
                Value::Undefined
            }
            "iswaitingonsound" => Value::Int(0),
            "attach" | "detach" | "detachall" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id)) {
                    let (m, t) = (s(0), s(1).to_ascii_lowercase());
                    match name {
                        "attach" => e.anim_state.attached.push((m, t)),
                        "detach" => e.anim_state.attached.retain(|(am, at)| !(am.eq_ignore_ascii_case(&m) && (t.is_empty() || *at == t))),
                        _ => e.anim_state.attached.clear(),
                    }
                }
                self.forward(me, name, args);
                Value::Undefined
            }
            // An actor's animscript for now is the script's function (CoD4's
            // `animcustom`): the one running is told `killanimscript`.
            "animcustom" => {
                if let (Some(id), Some(Value::Func(f))) = (me, args.first())
                    && self.ent(id).is_some_and(|e| e.kind == Kind::Ai)
                    && let Some(o) = vm.entity_obj(id)
                {
                    vm.notify(o, "killanimscript", Vec::new());
                    let b = self.brains.entry(id).or_default();
                    b.script = "custom".into();
                    b.custom = Some(*f);
                    b.animmode = None;
                    b.orient = None;
                    vm.spawn_thread(*f, Value::Object(o), Vec::new());
                }
                Value::Undefined
            }
            "hidepart" | "showpart" | "setanimlimited" | "setanimknoblimited" | "setanimknoblimitedrestart" | "setflaggedanimlimited" => {
                self.forward(me, name, args);
                Value::Undefined
            }
            // ---- Animation ----
            "animscripted" => {
                // (notify, origin, angles, anim)
                let Some(id) = me else { return Ok(Value::Undefined) };
                // An actor's animscript gives way (the engine pushes its
                // scripted state).
                if let Some(b) = self.brains.get_mut(&id)
                    && b.script != "scripted"
                {
                    b.script = "scripted".into();
                    if let Some(o) = vm.entity_obj(id) {
                        vm.notify(o, "killanimscript", Vec::new());
                    }
                }
                let anim = anim_name(args.get(3));
                let info = self.anims.info(&anim).unwrap_or(AnimInfo { length: 1.0, ..AnimInfo::default() });
                let (org, ang) = (v(1), v(2));
                // Linked: the base is kept relative to the parent's tag.
                let parent = self.ent(id).and_then(|e| e.linked.map(|(to, _, _)| (to, e.anim_state.link_tag.clone())));
                let anchor = parent.and_then(|(to, tag)| self.ent(to).map(|p| tag.as_deref().map_or((p.origin, p.angles), |t| p.tag(t))));
                let (base, anchored) = match anchor {
                    Some((po, pa)) => ((world_to_local(po, pa, org), [ang[0] - pa[0], ang[1] - pa[1], ang[2] - pa[2]]), true),
                    None => ((org, ang), false),
                };
                if let Some(e) = self.ent_mut(id) {
                    e.anim_state.anim = Some(EntAnim { name: anim, notify: Some(s(0)), start: now, rate: 1.0, from: 0.0, info, scripted: Some(base), anchored, next_note: 0, ended: false });
                }
                self.pose_scripted(id, now);
                Value::Undefined
            }
            "stopanimscripted" => {
                if let Some(e) = me.and_then(|id| self.ent_mut(id))
                    && let Some(a) = &mut e.anim_state.anim
                {
                    a.scripted = None;
                }
                Value::Undefined
            }
            "setanim" | "setanimknob" | "setanimknoball" | "setanimknobrestart" | "setanimknoballrestart" | "setanimrestart" | "setflaggedanim" | "setflaggedanimknob" | "setflaggedanimknoball" | "setflaggedanimknobrestart" | "setflaggedanimknoballrestart" | "setflaggedanimrestart" => {
                // [notify,] anim, weight, blend time, rate: the anim in charge
                // (no blending tree yet: the newest full-weight one shows).
                let Some(id) = me else { return Ok(Value::Undefined) };
                let flagged = name.starts_with("setflagged");
                let k = usize::from(flagged);
                let anim = anim_name(args.get(k));
                // (The `knoball`s name the tree's root after the animation:
                // `setanimknoball( anim, root, weight, time, rate )`.)
                let w = k + 1 + usize::from(name.contains("knoball"));
                let weight = args.get(w).and_then(Value::as_f32).unwrap_or(1.0);
                let rate = args.get(w + 2).and_then(Value::as_f32).unwrap_or(1.0);
                if weight <= 0.0 || anim.is_empty() {
                    return Ok(Value::Undefined);
                }
                let restart = name.contains("restart");
                let known = self.anims.info(&anim);
                // A blend node of the tree (`%body`, `%run_n_gun`) when the
                // game has the animations: no pose of its own; a flag on it
                // goes with the animation showing (its heaviest one).
                if known.is_none() && self.think_ai {
                    if flagged
                        && let Some(a) = self.ent_mut(id).and_then(|e| e.anim_state.anim.as_mut()).filter(|a| a.scripted.is_none())
                    {
                        a.notify = Some(s(0));
                    }
                    return Ok(Value::Undefined);
                }
                // (No weighted blend yet: a plain `setanim` of a light one,
                // like a run's sideways part, doesn't take over.)
                if !name.contains("knob") && !flagged && weight < 0.5 && self.think_ai {
                    return Ok(Value::Undefined);
                }
                let info = known.unwrap_or(AnimInfo { length: 1.0, ..AnimInfo::default() });
                let notify = flagged.then(|| s(0));
                if let Some(e) = self.ent_mut(id) {
                    // Under an `animscripted` that's still going, it's an
                    // overlay; the scripted one keeps the root. So is a
                    // face's (`_anim`'s talking).
                    let scripted = info.partial || e.anim_state.anim.as_ref().is_some_and(|a| a.scripted.is_some() && !a.ended && a.name != anim);
                    // (An actor's animscript can't lay a whole-body pose over
                    // its scripted animation, a casual idle bending it in two;
                    // only a face's.)
                    if scripted && !info.partial && e.kind == Kind::Ai {
                        return Ok(Value::Undefined);
                    }
                    if scripted {
                        let same = e.anim_state.overlay.as_ref().is_some_and(|a| a.name == anim);
                        if !(same && !restart) {
                            e.anim_state.overlay = Some(EntAnim { name: anim, notify, start: now, rate, from: 0.0, info, scripted: None, anchored: false, next_note: 0, ended: false });
                        } else if let Some(a) = &mut e.anim_state.overlay {
                            let f = a.frac(now);
                            a.from = f;
                            a.start = now;
                            a.rate = rate;
                            if notify.is_some() {
                                a.notify = notify;
                            }
                        }
                        return Ok(Value::Undefined);
                    }
                    let same = e.anim_state.anim.as_ref().is_some_and(|a| a.name == anim);
                    if same && !restart {
                        // Keep its place; take the new rate and notify.
                        let a = e.anim_state.anim.as_mut().expect("same");
                        let f = a.frac(now);
                        a.from = f;
                        a.start = now;
                        a.rate = rate;
                        if notify.is_some() {
                            a.notify = notify;
                        }
                    } else {
                        e.anim_state.anim = Some(EntAnim { name: anim, notify, start: now, rate, from: 0.0, info, scripted: None, anchored: false, next_note: 0, ended: false });
                    }
                }
                Value::Undefined
            }
            "clearanim" | "makefakeai" => Value::Undefined,
            // (Every stance is allowed until the scripts' `allowedstances`
            // or the node say otherwise: not tracked yet.)
            "isstanceallowed" => Value::Int(1),
            // (No enemies in sight yet: far away.)
            "getclosestenemysqdist" => Value::Float(1.0e8),
            "trackscriptstate" => Value::Undefined,
            "getnegotiationstartnode" | "getnegotiationendnode" => Value::Undefined,
            // Is a point within its goal (radius flat, goalheight up/down)?
            "isingoal" => {
                let p = v(0);
                let inside = me.and_then(|id| self.ent(id)).zip(me.and_then(|id| vm.entity_obj(id))).is_some_and(|(e, o)| {
                    let g = e.goal.unwrap_or(e.origin);
                    let r = vm.field(o, "goalradius").as_f32().unwrap_or(4.0);
                    let h = vm.field(o, "goalheight").as_f32().unwrap_or(80.0);
                    (p[0] - g[0]).hypot(p[1] - g[1]) <= r && (p[2] - g[2]).abs() <= h
                });
                Value::Int(i32::from(inside))
            }
            // (Accuracy against the player as the engine keeps it: not yet;
            // nor AI event listeners.)
            "updateplayersightaccuracy" | "addaieventlistener" => Value::Undefined,
            // (No traces for it here yet: room to move.)
            "maymovetopoint" | "maymovefrompointtopoint" => Value::Int(1),
            // Into cover: the arrival state runs `cover_arrival` (as the
            // engine's COVERARRIVAL), the path dropped.
            "startcoverarrival" => {
                if let Some(b) = me.and_then(|id| self.brains.get_mut(&id)) {
                    b.arrival = Some((v(0), f(1)));
                }
                Value::Undefined
            }
            // The best cover near its goal: the cover node in its goal radius
            // facing the enemy most (none if there's no such node).
            "findbestcovernode" | "getcovernode" | "findcovernode" => {
                let best = me.and_then(|id| self.best_cover(vm, id));
                best.map_or(Value::Undefined, Value::Object)
            }
            "usecovernode" => {
                let node = args.first().and_then(Value::as_obj);
                let at = node.and_then(|n| vm.field(n, "origin").as_vec());
                if let (Some(id), Some(at)) = (me, at) {
                    if let Some(e) = self.ent_mut(id) {
                        e.goal = Some(at);
                    }
                    if let Some(b) = self.brains.get_mut(&id) {
                        b.goal_node = node;
                        b.approach_sent = false;
                    }
                }
                Value::Int(i32::from(at.is_some()))
            }
            // Reacquiring an enemy out of sight, as CoD4's exposed state:
            // a step aside to where it'd see them...
            "reacquirestep" => {
                let to = me.and_then(|id| { let side = vm.random_float(); self.reacquire_step(vm, id, f(0), side) });
                let now = self.time;
                if let (Some(id), Some(to)) = (me, to)
                    && let Some(b) = self.brains.get_mut(&id)
                {
                    b.reacquire = Some((to, false, now));
                }
                Value::Int(i32::from(to.is_some()))
            }
            // ...a cover node of the best ten...
            "findreacquirenode" => {
                let list = me.map(|id| self.cover_list(vm, id, 10)).unwrap_or_default();
                if let Some(b) = me.and_then(|id| self.brains.get_mut(&id)) {
                    b.reacquire_nodes = list;
                }
                Value::Undefined
            }
            "getreacquirenode" => {
                let n = me.and_then(|id| self.brains.get_mut(&id)).and_then(|b| (!b.reacquire_nodes.is_empty()).then(|| b.reacquire_nodes.remove(0)));
                n.map_or(Value::Undefined, Value::Object)
            }
            // (A move to it inside its goal: the scripts' goal stays.)
            "usereacquirenode" => {
                let at = args.first().and_then(Value::as_obj).and_then(|n| vm.field(n, "origin").as_vec());
                let now = self.time;
                if let (Some(id), Some(at)) = (me, at)
                    && let Some(b) = self.brains.get_mut(&id)
                {
                    b.reacquire = Some((at, false, now));
                }
                Value::Int(i32::from(at.is_some()))
            }
            // ...or running at them: straight for them (as far as its
            // goal allows), or to a point in its goal that sees them,
            // within half the way.
            "findreacquiredirectpath" | "findreacquireproximatepath" => {
                let path = me.and_then(|id| self.reacquire_path(vm, id, name == "findreacquiredirectpath"));
                if let Some(b) = me.and_then(|id| self.brains.get_mut(&id)) {
                    b.reacquire_path = path;
                }
                Value::Undefined
            }
            "reacquiremove" => {
                let now = self.time;
                let started = me.and_then(|id| self.brains.get_mut(&id)).and_then(|b| {
                    let to = b.reacquire_path.take()?;
                    b.reacquire = Some((to, true, now));
                    Some(())
                });
                Value::Int(i32::from(started.is_some()))
            }
            // (Trimming the path to where it first sees them: the move
            // stops on sight instead.)
            "trimpathtoattack" => Value::Int(0),
            "flagenemyunattackable" => Value::Undefined,
            // A vehicle's missile (from its tag): an entity for the scripts
            // for its flight (a few seconds), told to the game.
            "maketurretunusable" | "maketurretusable" => {
                if let Some(id) = me {
                    if name == "maketurretunusable" {
                        self.unusable_turrets.insert(id);
                    } else {
                        self.unusable_turrets.remove(&id);
                    }
                }
                self.forward(me, name, args);
                Value::Undefined
            }
            "setvehweapon" => {
                if let Some(id) = me {
                    self.vehicle_weapons.insert(id, s(0).to_ascii_lowercase());
                }
                Value::Undefined
            }
            // A missile's new target (and offset).
            "missile_settarget" => {
                let target = self.arg_ent(vm, args, 0);
                let offset = args.get(1).and_then(Value::as_vec).unwrap_or_default();
                if let Some(m) = self.missiles.iter_mut().find(|m| Some(m.id) == me) {
                    m.target = target;
                    m.offset = offset;
                    m.until = self.time + 10.0;
                }
                Value::Undefined
            }
            // (tag, target, offset): it flies there (3000 units a second),
            // gone on arrival; with no target, for 3 s along its facing.
            "fireweapon" => {
                let tag = args.first().and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_ascii_lowercase);
                let (at, ang) = me.and_then(|id| self.ent(id)).map_or(([0.0; 3], [0.0; 3]), |e| tag.as_deref().map_or((e.origin, e.angles), |t| e.tag(t)));
                let id = self.add(blank("rocket", at));
                let target = self.arg_ent(vm, args, 1);
                let offset = args.get(2).and_then(Value::as_vec).unwrap_or_default();
                let (f, _, _) = crate::vm::angle_vectors(ang);
                self.missiles.push(Missile { id, target, offset, dir: f, until: self.time + if target.is_some() { 10.0 } else { 3.0 } });
                self.forward(me, name, args);
                vm.entity(id)
            }
            // ---- Vehicles: their paths and speeds, as the engine drives them ----
            "attachpath" | "startpath" => {
                if let Some(id) = me {
                    let node = self.arg_ent(vm, args, 0);
                    let v = self.vehicles.entry(id).or_default();
                    if let Some(n) = node {
                        v.next = Some(n);
                        v.from = None;
                    }
                    if name == "startpath" {
                        v.moving = true;
                        // Going from its node at that node's speed, unless set.
                        if v.target <= 0.0 {
                            let start = v.from.or(v.next);
                            let mph = start.and_then(|n| self.ent(n)).and_then(|e| e.get("speed")).and_then(|x| x.trim().parse::<f32>().ok());
                            if let (Some(mph), Some(v)) = (mph, self.vehicles.get_mut(&id)) {
                                v.target = mph * MPH;
                                v.resume = v.target;
                                v.accel = 10.0 * MPH;
                            }
                        }
                    }
                    // Attached: it stands at the node, facing along the path.
                    if name == "attachpath"
                        && let Some(n) = node
                        && let Some(at) = self.ent(n).map(|e| e.origin)
                    {
                        let ahead = self.next_vehicle_node(n).and_then(|m| self.ent(m)).map(|e| e.origin);
                        if let Some(e) = self.ent_mut(id) {
                            e.origin = at;
                            if let Some(a) = ahead {
                                e.angles[1] = (a[1] - at[1]).atan2(a[0] - at[0]).to_degrees();
                            }
                        }
                        let after = self.next_vehicle_node(n);
                        if let Some(v) = self.vehicles.get_mut(&id) {
                            v.from = Some(n);
                            v.next = after;
                        }
                    }
                }
                Value::Undefined
            }
            // (miles an hour, acceleration, deceleration: CoD's 17.6 units a
            // second to the mph)
            "setspeed" | "setspeedimmediate" | "resumespeed" | "vehicle_setspeed" => {
                if let Some(id) = me {
                    let v = self.vehicles.entry(id).or_default();
                    // (`resumeSpeed( accel )`: back to its path's speed,
                    // which a `setSpeed` doesn't change.)
                    match name {
                        "resumespeed" => {
                            // (No path speed known yet: as it is.)
                            if v.resume > 0.0 {
                                v.target = v.resume;
                            }
                            v.accel = f(0).max(1.0) * MPH;
                        }
                        _ => {
                            v.target = f(0).max(0.0) * MPH;
                            v.accel = if name == "setspeedimmediate" { 0.0 } else { args.get(1).and_then(Value::as_f32).unwrap_or(10.0).max(0.0) * MPH };
                            if name == "setspeedimmediate" {
                                v.speed = v.target;
                            }
                        }
                    }
                }
                Value::Undefined
            }
            "getspeed" => Value::Float(me.and_then(|id| self.vehicles.get(&id)).map_or(0.0, |v| v.speed)),
            "getspeedmph" => Value::Float(me.and_then(|id| self.vehicles.get(&id)).map_or(0.0, |v| v.speed / MPH)),
            "setswitchnode" => {
                if std::env::var_os("VEH_DEBUG").is_some() {
                    eprintln!("veh {me:?} setswitchnode {:?} {:?} t={:.2}", self.arg_ent(vm, args, 0), self.arg_ent(vm, args, 1), self.time);
                }
                if let (Some(id), Some(a), Some(b)) = (me, self.arg_ent(vm, args, 0), self.arg_ent(vm, args, 1)) {
                    self.vehicles.entry(id).or_default().switch = Some((a, b));
                }
                Value::Undefined
            }
            "setwaitnode" => {
                if let Some(id) = me {
                    let node = self.arg_ent(vm, args, 0);
                    self.vehicles.entry(id).or_default().wait = node;
                }
                Value::Undefined
            }
            // A helicopter's goal: (where, stop there), or a node's place
            // (an entity's or a struct's).
            "setvehgoalpos" | "setvehgoalnode" => {
                if let Some(id) = me {
                    let goal = if name == "setvehgoalnode" {
                        match self.arg_ent(vm, args, 0) {
                            Some(n) => self.ent(n).map_or([0.0; 3], |e| e.origin),
                            None => args.first().and_then(Value::as_obj).and_then(|o| vm.field(o, "origin").as_vec()).unwrap_or_default(),
                        }
                    } else {
                        v(0)
                    };
                    if std::env::var_os("VEH_DEBUG").is_some() {
                        eprintln!("veh {id} {name} {goal:?} t={:.2}", self.time);
                    }
                    let v = self.vehicles.entry(id).or_default();
                    v.goal = Some(goal);
                    v.near_sent = false;
                    if v.target <= 0.0 {
                        v.target = 60.0 * MPH;
                        v.resume = v.target;
                    }
                }
                Value::Undefined
            }
            "setneargoalnotifydist" => {
                if let Some(id) = me {
                    self.vehicles.entry(id).or_default().near = f(0);
                }
                Value::Undefined
            }
            "createthreatbiasgroup" => {
                self.bias_groups.insert(s(0).to_ascii_lowercase());
                self.forward(me, name, args);
                Value::Undefined
            }
            "threatbiasgroupexists" => Value::Int(i32::from(self.bias_groups.contains(&s(0).to_ascii_lowercase()))),
            "getvelocity" => Value::Vector(me.and_then(|id| self.velocity.get(&id)).map_or([0.0; 3], |v| v.0)),
            // Sight as the game traced it this frame.
            "cansee" => {
                let them = self.arg_ent(vm, args, 0);
                Value::Int(i32::from(me.zip(them).is_some_and(|(a, b)| self.sight.get(&(a, b)).copied().unwrap_or(false))))
            }
            "canshoot" => {
                let enemy = me.and_then(|id| vm.entity_obj(id)).and_then(|o| vm.entity_of(&vm.field(o, "enemy")));
                Value::Int(i32::from(me.zip(enemy).is_some_and(|(a, b)| self.sight.get(&(a, b)).copied().unwrap_or(false))))
            }
            // Fire at the enemy (or the point given).
            "shoot" => {
                if let Some(id) = me
                    && let Some(o) = vm.entity_obj(id)
                {
                    // Where its gun points (CoD4's trackLoop): its shootEnt,
                    // shootPos, cqb_target, or else its enemy. A point linked
                    // to someone (an execution's mark) is them.
                    let ent_field = |f: &str| vm.entity_of(&vm.field(o, f)).filter(|t| self.ent(*t).is_some());
                    let aim = ent_field("shootent").or_else(|| ent_field("cqb_target"));
                    let enemy = ent_field("enemy");
                    let shoot_pos = vm.field(o, "shootpos").as_vec();
                    let target = match aim {
                        Some(t) => self.ent(t).and_then(|e| e.linked).map(|l| l.0).filter(|p| self.ent(*p).is_some_and(|e| e.kind == Kind::Ai || *p == PLAYER)).or(Some(t)),
                        None if shoot_pos.is_some() => None,
                        None => enemy,
                    };
                    let at = args.get(1).and_then(Value::as_vec).or(shoot_pos.filter(|_| aim.is_none())).or_else(|| {
                        let t = self.ent(aim.or(enemy)?)?;
                        Some(if aim.is_some() { t.origin } else { [t.origin[0], t.origin[1], t.origin[2] + 48.0] })
                    });
                    let enemy = target;
                    // (Its target's `attackerAccuracy` scales it: 0 on a
                    // hero under `magic_bullet_shield`, never hit.)
                    let against = enemy.and_then(|t| vm.entity_obj(t)).map(|to| vm.field(to, "attackeraccuracy")).and_then(|v| v.as_f32()).unwrap_or(1.0);
                    let weapon = vm.field(o, "weapon").display();
                    let accuracy = vm.field(o, "accuracy").as_f32().unwrap_or(1.0) * args.first().and_then(Value::as_f32).unwrap_or(1.0) * against;
                    if let Some(at) = at {
                        self.commands.push(Command::Shoot { ai: id, target: enemy.filter(|_| args.get(1).is_none()), at, weapon, accuracy });
                    }
                }
                Value::Undefined
            }
            // The way it moves against the way it faces (-180..180).
            "getmotionangle" => {
                let rel = me.and_then(|id| Some((self.brains.get(&id)?.motion?, self.ent(id)?.angles[1]))).map_or(0.0, |(m, y)| (m - y + 540.0).rem_euclid(360.0) - 180.0);
                Value::Float(rel)
            }
            // The animscript's say over the actor's motion and facing.
            "animmode" => {
                if let Some(b) = me.and_then(|id| self.brains.get_mut(&id)) {
                    let m = s(0).to_ascii_lowercase();
                    b.animmode = (m != "none").then_some(m);
                }
                Value::Undefined
            }
            "orientmode" => {
                if let Some(b) = me.and_then(|id| self.brains.get_mut(&id)) {
                    let m = s(0).to_ascii_lowercase();
                    b.orient = (m != "face default").then(|| (m, args.get(1).cloned().unwrap_or_default()));
                }
                Value::Undefined
            }
            "setanimtime" => {
                // (anim, frac)
                if let Some(e) = me.and_then(|id| self.ent_mut(id))
                    && let Some(a) = &mut e.anim_state.anim
                {
                    a.from = f(1).clamp(0.0, 1.0);
                    a.start = now;
                    let from = a.from;
                    a.next_note = a.info.notes.iter().take_while(|(t, _)| *t < from).count();
                }
                if let Some(id) = me {
                    self.pose_scripted(id, now);
                }
                Value::Undefined
            }
            "getanimtime" => {
                let want = anim_name(args.first());
                let e = me.and_then(|id| self.ent(id));
                Value::Float(e.and_then(|e| e.anim_state.anim.as_ref()).filter(|a| a.name == want).map_or(0.0, |a| a.frac(now)))
            }
            "getnotetracktimes" => {
                // (anim, note): the fractions where it is.
                let anim = anim_name(args.first());
                let note = s(1);
                let info = self.anims.info(&anim).unwrap_or_default();
                Value::array(info.notes.iter().filter(|(_, n)| *n == note).map(|(t, _)| Value::Float(*t)))
            }
            "animhasnotetrack" => {
                let anim = anim_name(args.first());
                let note = s(1);
                Value::Int(i32::from(self.anims.info(&anim).is_some_and(|i| i.notes.iter().any(|(_, n)| *n == note))))
            }
            "getstartorigin" | "getstartangles" => {
                // (origin, angles, anim): where the anim puts its entity first.
                let anim = anim_name(args.get(2));
                let (d, yaw) = self.anims.delta(&anim, 0.0);
                let (o, a) = (v(0), v(1));
                if name == "getstartorigin" { Value::Vector(local_to_world(o, a, d)) } else { Value::Vector([a[0], a[1] + yaw, a[2]]) }
            }
            "getmovedelta" | "getangledelta" => {
                // (anim, from, to)
                let anim = anim_name(args.first());
                let (a, b) = (args.get(1).and_then(Value::as_f32).unwrap_or(0.0), args.get(2).and_then(Value::as_f32).unwrap_or(1.0));
                let (da, ya) = self.anims.delta(&anim, a);
                let (db, yb) = self.anims.delta(&anim, b);
                if name == "getmovedelta" { Value::Vector([db[0] - da[0], db[1] - da[1], db[2] - da[2]]) } else { Value::Float(yb - ya) }
            }
            "getattachsize" => Value::Int(me.and_then(|id| self.ent(id)).map_or(0, |e| e.anim_state.attached.len() as i32)),
            "getattachmodelname" | "getattachtagname" => {
                let i = f(0) as usize;
                let a = me.and_then(|id| self.ent(id)).and_then(|e| e.anim_state.attached.get(i).cloned());
                a.map_or(Value::Undefined, |(m, t)| Value::str(if name == "getattachmodelname" { &m } else { &t }))
            }
            "gettagorigin" | "gettagangles" => {
                let tag = s(0).to_ascii_lowercase();
                let Some(id) = me else { return Ok(Value::Undefined) };
                let e = self.ent_mut(id).ok_or("no entity")?;
                if !e.anim_state.wanted.contains(&tag) {
                    e.anim_state.wanted.push(tag.clone());
                }
                let (o, mut a) = e.tag(&tag);
                // An actor aiming (its animscript's `shootPos`): its gun
                // points there, as the aim blends would put it (no weighted
                // blend tree yet to do it).
                let aiming = e.kind == Kind::Ai && matches!(tag.as_str(), "tag_weapon" | "tag_flash" | "tag_weapon_right");
                if aiming
                    && let Some(p) = vm.entity_obj(id).and_then(|ob| vm.field(ob, "shootpos").as_vec())
                {
                    let d = [p[0] - o[0], p[1] - o[1], p[2] - o[2]];
                    a = crate::vm::std_lib::vector_to_angles(d);
                }
                Value::Vector(if name == "gettagorigin" { o } else { a })
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
                // (CoD4 has room for 32 actors: past that, a spawn fails;
                // a forced one makes room, deleting a deletable actor.)
                if self.ents.iter().filter(|e| e.kind == Kind::Ai && e.alive).count() >= MAX_ACTORS
                    && !(name == "stalingradspawn" && self.make_actor_room(vm))
                {
                    return Ok(Value::Undefined);
                }
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
                ai.kv = sp.kv.iter().filter(|(k, _)| k.eq_ignore_ascii_case("classname")).cloned().collect();
                ai.angles = sp.angles;
                ai.model = sp.model.clone();
                ai.team = team.clone();
                ai.alive = true;
                ai.spawnflags = sp.spawnflags;
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
                self.init_actor(vm, ai_id, id, &sp, team, name == "stalingradspawn");
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
                if let Some(b) = me.and_then(|id| self.brains.get_mut(&id)) {
                    b.goal_node = if name == "setgoalnode" { args.first().and_then(Value::as_obj) } else { None };
                    b.approach_sent = false;
                }
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
            // ---- The player's weapons, as the scripts gave them ----
            "giveweapon" | "takeweapon" | "takeallweapons" | "switchtoweapon" | "setweaponammoclip" if me == Some(PLAYER) => {
                let w = s(0).to_ascii_lowercase();
                match name {
                    "giveweapon" => {
                        if !self.player_weapons.contains(&w) {
                            self.player_weapons.push(w.clone());
                            let (clip, most) = self.weapons.get(&w).map_or((1, 0), |i| (i.clip_size, i.max_ammo));
                            self.player_ammo.insert(w.clone(), clip);
                            self.player_stock.insert(w.clone(), most);
                        }
                        if self.player_current.is_none() {
                            self.player_current = Some(w);
                        }
                    }
                    "takeweapon" => {
                        self.player_weapons.retain(|x| *x != w);
                        if self.player_current.as_ref() == Some(&w) {
                            self.player_current = self.player_weapons.first().cloned();
                        }
                    }
                    "takeallweapons" => {
                        self.player_weapons.clear();
                        self.player_current = None;
                    }
                    "switchtoweapon" => {
                        if self.player_weapons.contains(&w) {
                            self.player_current = Some(w);
                        }
                    }
                    _ => {
                        self.player_ammo.insert(w, args.get(1).and_then(Value::as_i32).unwrap_or(0));
                    }
                }
                self.forward(me, name, args);
                Value::Undefined
            }
            "getcurrentweapon" => Value::str(self.player_current.as_deref().unwrap_or("none")),
            "getweaponslistprimaries" => {
                let w: Vec<Value> = self.player_weapons.iter().filter(|w| self.weapons.get(*w).is_none_or(|i| i.class != "grenade")).map(|w| Value::str(w)).collect();
                Value::array(w)
            }
            "getweaponslist" => Value::array(self.player_weapons.iter().map(|w| Value::str(w)).collect::<Vec<_>>()),
            "hasweapon" if me == Some(PLAYER) => Value::Int(i32::from(self.player_weapons.contains(&s(0).to_ascii_lowercase()))),
            "getweaponammoclip" if me == Some(PLAYER) => Value::Int(self.player_ammo.get(&s(0).to_ascii_lowercase()).copied().unwrap_or(0)),
            "getweaponammostock" if me == Some(PLAYER) => Value::Int(self.player_stock.get(&s(0).to_ascii_lowercase()).copied().unwrap_or(0)),
            "setweaponammostock" if me == Some(PLAYER) => {
                self.player_stock.insert(s(0).to_ascii_lowercase(), args.get(1).and_then(Value::as_i32).unwrap_or(0));
                self.forward(me, name, args);
                Value::Undefined
            }
            "givemaxammo" if me == Some(PLAYER) => {
                let w = s(0).to_ascii_lowercase();
                let most = self.weapons.get(&w).map_or(90, |i| i.max_ammo.max(i.clip_size));
                self.player_stock.insert(w, most);
                self.forward(me, name, args);
                Value::Undefined
            }
            // `notifyoncommand( notify, command )`: the player's pressing
            // that command tells them so ([`Level::player_command`]).
            "notifyoncommand" if me == Some(PLAYER) => {
                let list = self.command_notifies.entry(s(1).to_ascii_lowercase()).or_default();
                let n = s(0);
                if !list.contains(&n) {
                    list.push(n);
                }
                Value::Undefined
            }
            "hasweapon" => Value::Int(0),
            "getweaponammoclip" | "getweaponammostock" | "getfractionmaxammo" => Value::Int(0),
            "getstance" if me != Some(PLAYER) => Value::str("stand"),
            // The player's state (the game's word, [`Level::player_ads`]).
            "playerads" if me == Some(PLAYER) => Value::Float(self.player_ads),
            "getstance" if me == Some(PLAYER) => Value::str(self.player_stance),
            "attackbuttonpressed" | "adsbuttonpressed" | "usebuttonpressed" | "meleebuttonpressed" | "fragbuttonpressed" | "secondaryoffhandbuttonpressed" | "isfiring" | "ismeleeing" | "isthrowinggrenade"
                if me == Some(PLAYER) =>
            {
                let cmd = match name {
                    "attackbuttonpressed" | "isfiring" => "+attack",
                    "adsbuttonpressed" => "+speed_throw",
                    "usebuttonpressed" => "+activate",
                    "meleebuttonpressed" | "ismeleeing" => "+melee",
                    "fragbuttonpressed" | "isthrowinggrenade" => "+frag",
                    _ => "+smoke",
                };
                Value::Int(i32::from(self.player_held.iter().any(|h| h == cmd)))
            }
            "isgodmode" | "isthrowinggrenade" | "ismeleeing" | "isfiring" | "isragdoll" | "buttonpressed" | "playerads" | "usebuttonpressed" | "attackbuttonpressed" | "adsbuttonpressed" | "meleebuttonpressed" | "fragbuttonpressed" | "secondaryoffhandbuttonpressed" => Value::Int(0),
            "getplayerviewheight" => Value::Float(60.0),
            "isweapondetonationtimed" => Value::Int(0),
            // From the weapon's def (CoD4's numbers when unknown).
            "weaponclass" | "weaponclipsize" | "weaponisboltaction" | "weaponissemiauto" | "weaponfiretime" | "weaponburstcount" | "weaponfightdist" | "weaponmaxdist" => {
                let w = self.weapons.get(&s(0).to_ascii_lowercase()).cloned().unwrap_or_default();
                match name {
                    "weaponclass" => Value::str(&w.class),
                    "weaponclipsize" => Value::Int(w.clip_size),
                    "weaponisboltaction" => Value::Int(i32::from(w.bolt_action)),
                    "weaponissemiauto" => Value::Int(i32::from(w.fire_type == 1)),
                    "weaponfiretime" => Value::Float(w.fire_time),
                    "weaponburstcount" => Value::Int(match w.fire_type {
                        2 => 2,
                        3 => 3,
                        4 => 4,
                        _ => 0,
                    }),
                    "weaponfightdist" => Value::Float(w.fight_dist),
                    _ => Value::Float(w.max_dist),
                }
            }
            "getweaponmodel" | "getweaponclipmodel" => Value::str(""),
            // (table, column to search, value, column to return): "" if no row.
            "tablelookup" => {
                let name = args.first().map(Value::display).unwrap_or_default().to_ascii_lowercase().replace('\\', "/");
                let col = |i: usize| args.get(i).and_then(Value::as_i32).unwrap_or(0).max(0) as usize;
                let want = args.get(2).map(Value::display).unwrap_or_default();
                let found = self.tables.get(&name).and_then(|rows| {
                    rows.iter().find(|r| r.get(col(1)).is_some_and(|c| c.eq_ignore_ascii_case(&want))).and_then(|r| r.get(col(3)).cloned())
                });
                Value::str(&found.unwrap_or_default())
            }
            // Bullets of the other side have passed close lately
            // ([`Level::bullet_passed`]).
            // (Not one told to ignore it.)
            "issuppressed" => {
                let ignores = me.and_then(|id| vm.entity_obj(id)).is_some_and(|o| vm.field(o, "ignoresuppression").truthy());
                Value::Int(i32::from(!ignores && me.and_then(|id| self.suppressed.get(&id)).is_some_and(|t| *t > self.time)))
            }
            // An enemy it knows of (its enemy, seen) within the radius of
            // the point.
            "isknownenemyinradius" => {
                let (at, r) = (v(0), f(1));
                let known = me.and_then(|id| {
                    let o = vm.entity_obj(id)?;
                    let enemy = vm.entity_of(&vm.field(o, "enemy"))?;
                    let e = self.ent(enemy)?;
                    self.sight.get(&(id, enemy)).copied().unwrap_or(false).then_some(e.origin)
                });
                Value::Int(i32::from(known.is_some_and(|p| ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2) + (p[2] - at[2]).powi(2)).sqrt() <= r)))
            }
            "canattackenemynode" => Value::Int(i32::from(me.is_some_and(|id| {
                let enemy = vm.entity_obj(id).map(|o| vm.field(o, "enemy")).and_then(|v| vm.entity_of(&v));
                enemy.is_some_and(|t| self.sight.get(&(id, t)).copied().unwrap_or(false))
            }))),
            "isknownenemyinvolume" => Value::Int(0),
            "getthreatbiasgroup" => Value::str(""),
            "getturret" | "getturretowner" | "getvehicleowner" => Value::Undefined,
            "nearnode" => Value::Undefined,
            "getanglestolikelyenemypath" => Value::Undefined,
            "playerlinkto" | "playerlinktodelta" | "playerlinktoabsolute" => {
                // (ent, tag, view fraction, right, left, up, down arcs)
                if let Some(to) = self.arg_ent(vm, args, 0) {
                    let tag = args.get(1).and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_ascii_lowercase);
                    if let (Some(t), Some(p)) = (&tag, self.ent_mut(to))
                        && !p.anim_state.wanted.contains(t)
                    {
                        p.anim_state.wanted.push(t.clone());
                    }
                    let arc = |i: usize| args.get(i).and_then(Value::as_f32).unwrap_or(if name == "playerlinktoabsolute" { 0.0 } else { 180.0 });
                    self.player_link = Some(PlayerLink { to, tag, arcs: [arc(3), arc(4), arc(5), arc(6)], delta: name == "playerlinktodelta", turret: false, offset: [0.0; 3] });
                }
                self.forward(me, name, args);
                Value::Undefined
            }
            // Everything an AI, the player, a vehicle or a turret does that
            // the game carries out.
            // A vehicle's turret aimed: it turns there, and says so
            // (`turret_on_target`, `turret_rotate_stopped`) on arriving.
            "setturrettargetent" | "setturrettargetvec" | "clearturrettarget" => {
                if let Some(id) = me {
                    let a = self.turret_aims.entry(id).or_default();
                    match name {
                        "setturrettargetent" => {
                            a.ent = args.first().and_then(|v| vm.entity_of(v));
                            a.on_target = false;
                        }
                        "setturrettargetvec" => {
                            a.ent = None;
                            a.at = v(0);
                            a.on_target = false;
                        }
                        _ => {
                            a.ent = None;
                            a.on_target = true;
                        }
                    }
                }
                self.forward(me, name, args);
                Value::Undefined
            }
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
            // As the map spells it (an AI takes its spawner's); scripts
            // compare it case and all.
            "classname" => Value::str(e.get("classname").unwrap_or(&e.classname)),
            "code_classname" => Value::str(&e.classname),
            "model" => Value::str(&e.model),
            "health" => Value::Int(e.health),
            "maxhealth" => Value::Int(e.max_health),
            "team" if matches!(e.kind, Kind::Ai | Kind::Player) => Value::str(&e.team),
            // An actor's look axes (CoD4's `lookforward`, `lookright`,
            // `lookup`): its facing, level.
            "lookforward" | "lookright" | "lookup" if e.kind == Kind::Ai => {
                let (f, r, u) = crate::vm::angle_vectors([0.0, e.angles[1], 0.0]);
                Value::Vector(match name {
                    "lookforward" => f,
                    "lookright" => r,
                    _ => u,
                })
            }
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
    /// An `animscripted` entity where its animation's root motion has it now.
    fn pose_scripted(&mut self, id: EntId, now: f64) {
        let Some(e) = self.ent(id) else { return };
        let Some(a) = &e.anim_state.anim else { return };
        let Some((mut org, mut ang)) = a.scripted else { return };
        if a.anchored {
            let parent = e.linked.map(|(to, _, _)| (to, e.anim_state.link_tag.clone()));
            if let Some((po, pa)) = parent.and_then(|(to, tag)| self.ent(to).map(|p| tag.as_deref().map_or((p.origin, p.angles), |t| p.tag(t)))) {
                org = local_to_world(po, pa, org);
                ang = [pa[0] + ang[0], pa[1] + ang[1], pa[2] + ang[2]];
            }
        }
        if !a.info.delta {
            if let Some(e) = self.ent_mut(id) {
                e.origin = org;
                e.angles = ang;
            }
            return;
        }
        let (name, f) = (a.name.clone(), a.frac(now));
        let (d, yaw) = self.anims.delta(&name, f);
        if let Some(e) = self.ent_mut(id) {
            e.origin = local_to_world(org, ang, d);
            e.angles = [ang[0], ang[1] + yaw, ang[2]];
        }
    }

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
        "disableinvulnerability", "enablehealthshield", "setnormalhealth", "setstance", "enterprone", "exitprone",
        "setmovespeedscale", "setflashbanged", "setflashbangimmunity", "playrumbleonentity", "playrumblelooponentity", "stoprumble", "pushplayer",
        "playersetgroundreferenceent", "disableaimassist", "enableaimassist", "openmenu", "closemenu", "setclientdvar", "setblurforplayer",
        "notifyoncommand", "forceviewmodelanimation", "setspawnerteam", "setengagementmindist", "setengagementmaxdist", "settargetentity",
        "cleartargetentity", "setentitytarget", "clearentitytarget", "setthreatbiasgroup", "clearenemy", "setlookatent", "clearlookatent",
        "setlookattext", "setfriendlychain", "setruntopos", "setyawspeed", "settargetyaw", "cleartargetyaw",
        "setgoalyaw", "cleargoalyaw", "startragdoll", "shootblank", "startfiring", "stopfiring", "useturret", "stopuseturret",
        "shootturret", "clearturrettarget", "startpath", "attachpath", "resumespeed", "setspeed",
        "setwaitspeed", "freevehicle", "makevehicleunusable", "makevehicleusable",
        "sethoverparams", "joltbody", "setfixednodesafevolume", "clearfixednodesafevolume", "dropweapon",
        "setdefaultdroppitch", "restoredefaultdroppitch", "setproneanimnodes", "pushplayer",
    ];
    PREFIXES.iter().any(|p| n.starts_with(p)) || NAMES.contains(&n) || n.starts_with("set") || n.starts_with("clear") || n.starts_with("disable") || n.starts_with("enable")
}

/// An `%anim` argument's name.
fn anim_name(v: Option<&Value>) -> String {
    match v {
        Some(Value::Anim(n, _)) => n.to_string(),
        Some(Value::Str(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// A world point into an entity's axes (x forward, y left, z up).
pub fn world_to_local(origin: [f32; 3], angles: [f32; 3], p: [f32; 3]) -> [f32; 3] {
    let (f, r, u) = crate::vm::angle_vectors(angles);
    let d = [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]];
    let dot = |a: [f32; 3]| d[0] * a[0] + d[1] * a[1] + d[2] * a[2];
    [dot(f), -dot(r), dot(u)]
}

/// A point in an entity's axes (x forward, y left, z up) to the world.
/// Debug (bisecting): `GSC_OLD_LINKS=1` adds linked angles as before.
fn old_links() -> bool {
    std::env::var_os("GSC_OLD_LINKS").is_some()
}

/// What a trace is stopped by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceKind {
    Bullet,
    Sight,
    Physics,
    /// An actor's feet on the ground: the world, not the player's clip.
    Actor,
}

/// A trace's hit: fraction of the way, where, the surface's normal.
pub type TraceHit = (f32, [f32; 3], [f32; 3]);

type Tracer<'a> = dyn Fn([f32; 3], [f32; 3], TraceKind) -> Option<TraceHit> + 'a;

thread_local! {
    static TRACER: std::cell::Cell<Option<*const Tracer<'static>>> = const { std::cell::Cell::new(None) };
}

/// Run `f` with the world's collision lent to the scripts' traces
/// (`bulletTrace`, `physicsTrace`, `sightTracePassed`): the game's ray
/// casts, borrowed only for the call.
pub fn with_tracer<R>(tracer: &Tracer<'_>, f: impl FnOnce() -> R) -> R {
    // SAFETY: the pointer is only read inside `f`, while `tracer` is alive;
    // the previous one is put back after.
    let p: *const Tracer<'static> = unsafe { std::mem::transmute::<*const Tracer<'_>, *const Tracer<'static>>(tracer as *const _) };
    let old = TRACER.with(|t| t.replace(Some(p)));
    let out = f();
    TRACER.with(|t| t.set(old));
    out
}

fn trace(from: [f32; 3], to: [f32; 3], kind: TraceKind) -> Option<TraceHit> {
    let p = TRACER.with(|t| t.get())?;
    // SAFETY: set only for the extent of `with_tracer`'s call.
    unsafe { (*p)(from, to, kind) }
}

/// An orientation's axes: forward, left, up (CoD's `AnglesToAxis`).
fn axes(a: [f32; 3]) -> [[f32; 3]; 3] {
    let (f, r, u) = crate::vm::angle_vectors(a);
    [f, [-r[0], -r[1], -r[2]], u]
}

/// Axes back to angles (CoD's `AxisToAngles`).
fn axes_angles(m: [[f32; 3]; 3]) -> [f32; 3] {
    let [f, l, u] = m;
    let pitch = (-f[2]).atan2((f[0] * f[0] + f[1] * f[1]).sqrt()).to_degrees();
    let yaw = f[1].atan2(f[0]).to_degrees();
    let roll = l[2].atan2(u[2]).to_degrees();
    [pitch, yaw, roll]
}

/// `local` turned by `parent`: a child's angles from its parent's and its
/// own relative to it.
pub fn compose_angles(parent: [f32; 3], local: [f32; 3]) -> [f32; 3] {
    let p = axes(parent);
    let l = axes(local);
    let world = |v: [f32; 3]| [0, 1, 2].map(|i| p[0][i] * v[0] + p[1][i] * v[1] + p[2][i] * v[2]);
    axes_angles([world(l[0]), world(l[1]), world(l[2])])
}

/// `child`'s angles relative to `parent`'s (the inverse of [`compose_angles`]).
pub fn relative_angles(parent: [f32; 3], child: [f32; 3]) -> [f32; 3] {
    let p = axes(parent);
    let c = axes(child);
    let local = |v: [f32; 3]| [0, 1, 2].map(|k| p[k][0] * v[0] + p[k][1] * v[1] + p[k][2] * v[2]);
    axes_angles([local(c[0]), local(c[1]), local(c[2])])
}

pub fn local_to_world(origin: [f32; 3], angles: [f32; 3], l: [f32; 3]) -> [f32; 3] {
    let (f, r, u) = crate::vm::angle_vectors(angles);
    [
        origin[0] + f[0] * l[0] - r[0] * l[1] + u[0] * l[2],
        origin[1] + f[1] * l[0] - r[1] * l[1] + u[1] * l[2],
        origin[2] + f[2] * l[0] - r[2] * l[1] + u[2] * l[2],
    ]
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
        "node_cover_wide_left" => "Cover Wide Left",
        "node_cover_wide_right" => "Cover Wide Right",
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
        shape: None,
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
                anim_state: AnimState::default(),
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
        // Three numbers: a vector (`start_color`).
        if v.split_whitespace().count() == 3
            && let Some(x) = vec3(Some(v))
        {
            return Value::Vector(x);
        }
    }
    Value::str(v)
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Script HUD elements.
impl Level {
    /// HUD element `o`'s alpha and colour shown now, from its fields.
    fn hud_alpha_color(&self, o: ObjId, alpha: f32, color: [f32; 3]) -> (f32, [f32; 3]) {
        match self.hud.get(&o).and_then(|h| h.fade) {
            Some((t0, d, a0, c0)) if d > 0.0 && ((self.time - t0) as f32) < d => {
                let f = ((self.time - t0) as f32 / d).clamp(0.0, 1.0);
                (a0 + (alpha - a0) * f, [0, 1, 2].map(|i| c0[i] + (color[i] - c0[i]) * f))
            }
            _ => (alpha, color),
        }
    }

    fn hud_xy(&self, o: ObjId, x: f32, y: f32) -> (f32, f32) {
        match self.hud.get(&o).and_then(|h| h.moving) {
            Some((t0, d, x0, y0)) if d > 0.0 && ((self.time - t0) as f32) < d => {
                let f = ((self.time - t0) as f32 / d).clamp(0.0, 1.0);
                (x0 + (x - x0) * f, y0 + (y - y0) * f)
            }
            _ => (x, y),
        }
    }

    /// The HUD elements as they show now, back to front.
    pub fn hud_snapshot(&self, vm: &Vm) -> Vec<HudDraw> {
        let mut out: Vec<HudDraw> = self
            .hud
            .iter()
            .map(|(&o, h)| {
                let f = |n: &str| vm.field(o, n);
                let num = |n: &str, d: f32| f(n).as_f32().unwrap_or(d);
                let st = |n: &str, d: &str| f(n).as_str().map_or(d.to_owned(), str::to_ascii_lowercase);
                let (mut alpha, color) = self.hud_alpha_color(o, num("alpha", 1.0), f("color").as_vec().unwrap_or([1.0; 3]));
                let (x, y) = self.hud_xy(o, num("x", 0.0), num("y", 0.0));
                let mut letters = None;
                // Letters type on, hold, then fade.
                if let Some((t0, per, decay_at, decay)) = h.pulse {
                    let ms = ((self.time - t0) * 1000.0) as f32;
                    if per > 0.0 {
                        letters = Some((ms / per).max(0.0) as usize);
                    }
                    if ms > decay_at {
                        alpha *= (1.0 - (ms - decay_at) / decay.max(1.0)).clamp(0.0, 1.0);
                    }
                }
                let shader = h.shader.clone().map(|(m, w, hh)| match h.scaling {
                    Some((t0, d, w0, h0)) if d > 0.0 && ((self.time - t0) as f32) < d => {
                        let k = ((self.time - t0) as f32 / d).clamp(0.0, 1.0);
                        (m, w0 + (w - w0) * k, h0 + (hh - h0) * k)
                    }
                    _ => (m, w, hh),
                });
                let glow = (num("glowalpha", 0.0) > 0.0).then(|| (f("glowcolor").as_vec().unwrap_or([1.0; 3]), num("glowalpha", 0.0)));
                HudDraw {
                    x,
                    y,
                    align: (st("alignx", "left"), st("aligny", "top")),
                    screen_align: (st("horzalign", "subleft"), st("vertalign", "subtop")),
                    alpha: alpha.clamp(0.0, 1.0),
                    color,
                    sort: num("sort", 0.0),
                    foreground: f("foreground").as_i32().unwrap_or(0) != 0,
                    text: h.text.clone(),
                    letters,
                    font: st("font", "default"),
                    font_scale: num("fontscale", 1.0),
                    glow,
                    shader,
                }
            })
            .collect();
        out.sort_by(|a, b| (a.foreground, a.sort).partial_cmp(&(b.foreground, b.sort)).unwrap_or(std::cmp::Ordering::Equal));
        out
    }
}

/// Actors coming to life.
impl Level {
    /// As the engine before the level's `main` (`Actor_FinishSpawningAll`):
    /// each spawner's type runs its `spawner()`, and each actor placed in
    /// the map comes to life.
    fn finish_spawning(&mut self, vm: &mut Vm) {
        let actors: Vec<(EntId, Kind, Ent)> =
            self.ents.iter().filter(|e| matches!(e.kind, Kind::Spawner | Kind::Ai)).map(|e| (e.id, e.kind.clone(), e.clone())).collect();
        for (id, kind, e) in actors {
            let aitype = e.classname.trim_start_matches("actor_").to_ascii_lowercase();
            let me = vm.entity(id);
            if kind == Kind::Spawner {
                if let Some(f) = vm.func(&format!("aitype/{aitype}"), "spawner") {
                    vm.call_now(self, f, me, Vec::new());
                }
                continue;
            }
            let team = spawner_team(&e.classname).to_owned();
            if let Some(a) = self.ent_mut(id) {
                a.team = team.clone();
                a.alive = true;
                a.health = 150;
                a.max_health = 150;
                a.bounds = Some(([-15.0, -15.0, 0.0], [15.0, 15.0, 70.0]));
            }
            if let Some(o) = me.as_obj() {
                vm.set_field(o, "team", Value::str(&team));
            }
            self.init_actor(vm, id, id, &e, team, false);
        }
    }

    /// An actor's scripts, as the engine starts them: its type's `main`
    /// (team, weapons, character), then `animscripts/init` (the AI's
    /// script state); and the game told.
    fn init_actor(&mut self, vm: &mut Vm, ai_id: EntId, spawner: EntId, sp: &Ent, team: String, force: bool) {
        let ai_val = vm.entity(ai_id);
        // The engine's own actor fields, at their spawn values.
        if let Some(o) = ai_val.as_obj() {
            vm.set_field(o, "suppressionmeter", Value::Float(0.0));
            vm.set_field(o, "lastenemysighttime", Value::Int(0));
        }
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
        self.commands.push(Command::SpawnAi { spawner, ai: ai_id, origin: sp.origin, angles: sp.angles, team, classname: sp.classname.clone(), model, force });
    }
}

/// An actor's engine side: the animscript it runs and how it moves.
#[derive(Clone, Debug, Default)]
pub struct Brain {
    /// `animscripts/<script>::main` running (`stop`, `move`, `scripted`...).
    pub script: String,
    /// `animcustom( func )`: that function is its animscript until done.
    pub custom: Option<crate::vm::FuncId>,
    /// `stop`, `stop_soon`, `walk`, `run` (the script's `movemode`).
    pub movemode: String,
    /// The script's `animmode` (`gravity`, `zonly_physics`...), or code.
    pub animmode: Option<String>,
    /// The script's `orientmode` and its argument.
    pub orient: Option<(String, Value)>,
    /// Where its main animation was last frame (name, fraction).
    last: Option<(String, f32)>,
    /// The way it's going (world yaw), while it has somewhere to go.
    pub motion: Option<f32>,
    /// Its route's points to the goal it was made for.
    route: Vec<[f32; 3]>,
    /// Where its seen enemy stands (it faces it when the script doesn't say).
    face_enemy: Option<[f32; 3]>,
    /// Hurt: its pain script runs until it's done (CoD4's pain state).
    pub pain: bool,
    /// The node its goal is (`setgoalnode`), and the node it holds (`self.node`).
    pub goal_node: Option<ObjId>,
    pub claimed: Option<ObjId>,
    /// Told `corner_approach` for this goal; arriving into cover
    /// (`startcoverarrival`: where, facing).
    approach_sent: bool,
    pub arrival: Option<([f32; 3], f32)>,
    route_goal: Option<[f32; 3]>,
    /// Lost sight of its enemy (CoD4's exposed reacquire): where it's
    /// stepping or running to now (whether it stops on seeing the enemy,
    /// since when), the path found to go by (`FindReacquire*Path`), and
    /// the cover nodes left to try (`FindReacquireNode`).
    reacquire: Option<([f32; 3], bool, f64)>,
    reacquire_path: Option<[f32; 3]>,
    reacquire_nodes: Vec<ObjId>,
    /// Last think.
    at: f64,
}

/// The actors' think, as the engine's under the animscripts: each picks
/// its animscript as CoD4's exposed non-combat state does (`move` while
/// its goal is away, `stop` there; `scripted` under `animscripted`),
/// moves by its animation's root motion (along the line to the goal in
/// code-moved mode) and turns to face where it goes.
/// Not yet: combat, cover nodes and arrivals, pathfinding over the nodes
/// (a straight line for now), pain.
impl Level {
    fn think_actors(&mut self, vm: &mut Vm, now: f64) {
        // The dead play their death (CoD4's death state) once.
        let dead: Vec<EntId> = self.ents.iter().filter(|e| e.kind == Kind::Ai && !e.alive).map(|e| e.id).collect();
        for id in dead {
            let Some(o) = vm.entity_obj(id) else { continue };
            let b = self.brains.entry(id).or_default();
            if b.script != "death" {
                let mut b2 = std::mem::take(b);
                Self::set_anim_script(vm, o, &mut b2, "death", "stop");
                self.brains.insert(id, b2);
            }
        }
        let ids: Vec<EntId> = self.ents.iter().filter(|e| e.kind == Kind::Ai && e.alive).map(|e| e.id).collect();
        for id in ids {
            let Some(o) = vm.entity_obj(id) else { continue };
            self.sense(vm, id, o);
            let mut b = self.brains.remove(&id).unwrap_or_else(|| Brain { at: now, ..Brain::default() });
            let dt = ((now - b.at) as f32).clamp(0.0, 0.25);
            b.at = now;
            let Some(e) = self.ent(id) else { continue };
            let scripted = e.anim_state.anim.as_ref().is_some_and(|a| a.scripted.is_some() && !a.ended);
            // (Riding something, linked: no path, no think of its own,
            // as the engine.)
            if e.linked.is_some() {
                b.last = None;
                self.brains.insert(id, b);
                continue;
            }
            if scripted {
                let (goal, origin) = (e.goal.unwrap_or(e.origin), e.origin);
                // (Into it: the animscript it had is ended, as the engine's
                // `animscripted` does, and what that left playing over the
                // body (a casual idle, a turn) is gone with it.)
                if b.script != "scripted" {
                    vm.notify(o, "killanimscript", Vec::new());
                    if let Some(e) = self.ent_mut(id)
                        && e.anim_state.overlay.as_ref().is_some_and(|ov| !ov.info.partial)
                    {
                        e.anim_state.overlay = None;
                    }
                }
                b.script = "scripted".into();
                b.last = None;
                // (At its goal all the same: told so, as the engine does
                // whatever animation plays.)
                let radius = vm.field(o, "goalradius").as_f32().unwrap_or(4.0).max(1.0);
                let at_goal = (goal[0] - origin[0]).hypot(goal[1] - origin[1]) <= radius;
                self.brains.insert(id, b);
                if at_goal {
                    vm.notify(o, "goal", Vec::new());
                }
                continue;
            }
            // Its goal (where it stands until told), and how near counts.
            let real_goal = e.goal.unwrap_or(e.origin);
            let real_radius = vm.field(o, "goalradius").as_f32().unwrap_or(4.0).max(1.0);
            // Reacquiring its enemy: there first, till there, seeing them
            // (a run at them), or 6 s on.
            if let Some((to, stop_on_sight, since)) = b.reacquire {
                let seen = vm.entity_of(&vm.field(o, "enemy")).is_some_and(|t| self.sight.get(&(id, t)).copied().unwrap_or(false));
                let there = (to[0] - e.origin[0]).hypot(to[1] - e.origin[1]) < 16.0;
                if there || now - since > 6.0 || (stop_on_sight && seen && now - since > 0.5) {
                    b.reacquire = None;
                }
            }
            let (goal, radius) = b.reacquire.map_or((real_goal, real_radius), |r| (r.0, 16.0));
            let walkdist = vm.field(o, "walkdist").as_f32().unwrap_or(0.0);
            let flat = [goal[0] - e.origin[0], goal[1] - e.origin[1]];
            let dist = (flat[0] * flat[0] + flat[1] * flat[1]).sqrt();
            // Its route over the path nodes (made again when the goal moves).
            let origin = e.origin;
            if dist > radius && b.route_goal != Some(goal) {
                b.route_goal = Some(goal);
                b.route = match self.route(origin, goal) {
                    Some(r) => r,
                    None => {
                        if !self.path_nodes.is_empty() {
                            vm.notify(o, "bad_path", vec![Value::Vector(goal)]);
                        }
                        vec![goal]
                    }
                };
            }
            if dist <= radius {
                b.route.clear();
                b.route_goal = None;
            }
            // Points passed (within 16 units, flat).
            while b.route.len() > 1 && {
                let p = b.route[0];
                (p[0] - origin[0]).hypot(p[1] - origin[1]) < 16.0
            } {
                b.route.remove(0);
            }
            let next = b.route.first().copied().unwrap_or(goal);
            let way = [next[0] - origin[0], next[1] - origin[1]];
            let way_len = way[0].hypot(way[1]).max(0.001);
            b.motion = (dist > radius).then(|| way[1].atan2(way[0]).to_degrees());
            // The path's fields.
            if dist > radius {
                vm.set_field(o, "lookaheaddir", Value::Vector([way[0] / way_len, way[1] / way_len, 0.0]));
                vm.set_field(o, "pathgoalpos", Value::Vector(goal));
            } else {
                vm.set_field(o, "pathgoalpos", Value::Undefined);
            }
            // Arriving into cover: `cover_arrival` until it's done, moved by
            // its animation.
            if let Some((_, yaw)) = b.arrival {
                if b.script != "cover_arrival" {
                    Self::set_anim_script(vm, o, &mut b, "cover_arrival", "stop");
                    b.animmode = Some("zonly_physics".into());
                    b.orient = Some(("face angle".into(), Value::Float(yaw)));
                    b.route.clear();
                } else if vm.func("animscripts/cover_arrival", "main").is_none_or(|f| !vm.running(f, o)) {
                    b.arrival = None;
                }
            }
            // Its path ends at the cover node it's going to hold, and its
            // move script asked: `corner_approach`, the way it comes in.
            if b.arrival.is_none()
                && !b.approach_sent
                && dist > radius
                && b.goal_node.is_some_and(|n| cover_script(vm.field(n, "type").as_str().unwrap_or(""), false).is_some())
                && vm.field(o, "requestarrivalnotify").truthy()
            {
                let from = if b.route.len() >= 2 { b.route[b.route.len() - 2] } else { origin };
                let d = [goal[0] - from[0], goal[1] - from[1], 0.0];
                let l = d[0].hypot(d[1]).max(0.001);
                b.approach_sent = true;
                vm.set_field(o, "requestarrivalnotify", Value::Undefined);
                vm.notify(o, "corner_approach", vec![Value::Vector([d[0] / l, d[1] / l, 0.0])]);
            }
            // In pain: that until its script is done.
            let arriving = b.arrival.is_some();
            if b.pain && !arriving {
                if b.script != "pain" {
                    Self::set_anim_script(vm, o, &mut b, "pain", "stop");
                    vm.notify(o, "pain", Vec::new());
                } else if vm.func("animscripts/pain", "main").is_none_or(|f| !vm.running(f, o)) {
                    b.pain = false;
                }
                if b.pain {
                    b.last = None;
                    self.brains.insert(id, b);
                    continue;
                }
            }
            // An enemy in sight: fight (CoD4's exposed combat), moving if
            // its goal is away.
            let enemy = vm.entity_of(&vm.field(o, "enemy")).filter(|t| self.ent(*t).is_some_and(|t| t.alive));
            let enemy_seen = enemy.is_some_and(|t| self.sight.get(&(id, t)).copied().unwrap_or(false));
            // At its goal: the node it holds (its goal node, or a cover
            // node it stands on), and that node's script, as CoD4's
            // `GetStopAnim` / `AnimCombat`.
            let at_goal = dist <= radius;
            let claimed = if at_goal { b.goal_node.or_else(|| self.node_at(vm, origin)) } else { None };
            if claimed != b.claimed {
                b.claimed = claimed;
                vm.set_field(o, "node", claimed.map_or(Value::Undefined, Value::Object));
            }
            let cover = claimed.and_then(|n| cover_script(vm.field(n, "type").as_str().unwrap_or(""), enemy.is_some()));
            let (script, movemode) = if arriving {
                ("cover_arrival", "stop")
            } else if !at_goal {
                ("move", if walkdist > 0.0 && dist < walkdist && enemy.is_none() { "walk" } else { "run" })
            } else if let Some(c) = cover {
                (c, "stop")
            } else if enemy.is_some() {
                ("combat", "stop")
            } else {
                ("stop", "stop")
            };
            if enemy_seen {
                vm.set_field(o, "lastenemysighttime", Value::Int((now * 1000.0) as i32));
            }
            if enemy_seen && b.orient.is_none() {
                b.face_enemy = enemy.and_then(|t| self.ent(t)).map(|t| t.origin);
            } else {
                b.face_enemy = None;
            }
            // (Under `animcustom`, its function decides, till it's done.)
            if let Some(f) = b.custom {
                if vm.running(f, o) {
                } else {
                    b.custom = None;
                    b.script.clear();
                    b.animmode = None;
                    b.orient = None;
                }
            }
            if b.custom.is_none() {
                Self::set_anim_script(vm, o, &mut b, script, movemode);
            }
            // How far its animation carried it this frame.
            let Some(e) = self.ent(id) else { continue };
            let anim = e.anim_state.anim.as_ref().map(|a| (a.name.clone(), a.frac(now), a.info.delta));
            let mut step = ([0.0f32; 3], 0.0f32);
            if let Some((name, f, true)) = &anim {
                let (d1, y1) = self.anims.delta(name, *f);
                match &b.last {
                    Some((last, f0)) if last == name && *f >= *f0 => {
                        let (d0, y0) = self.anims.delta(name, *f0);
                        step = ([d1[0] - d0[0], d1[1] - d0[1], d1[2] - d0[2]], y1 - y0);
                    }
                    Some((last, f0)) if last == name => {
                        // (Looped round.)
                        let (d0, y0) = self.anims.delta(name, *f0);
                        let (de, ye) = self.anims.delta(name, 1.0);
                        step = ([de[0] - d0[0] + d1[0], de[1] - d0[1] + d1[1], de[2] - d0[2] + d1[2]], ye - y0 + y1);
                    }
                    _ => {}
                }
            }
            b.last = anim.map(|(n, f, _)| (n, f));
            let Some(e) = self.ent_mut(id) else { continue };
            let yaw = e.angles[1];
            let mut face = None;
            match b.animmode.as_deref() {
                // Code moves it: the animation's pace, the way to its goal.
                None | Some("normal") => {
                    if b.script == "move" && dist > radius {
                        let mut pace = (step.0[0] * step.0[0] + step.0[1] * step.0[1]).sqrt();
                        // (A move with no root motion showing yet: a run's pace.)
                        if pace < 0.01 {
                            pace = dt * if movemode == "walk" { 70.0 } else { 190.0 };
                        }
                        let k = (pace / way_len).min(1.0);
                        e.origin[0] += way[0] * k;
                        e.origin[1] += way[1] * k;
                        // (Heights from the route's points: stairs, ramps.)
                        e.origin[2] += (next[2] - e.origin[2]) * k;
                        face = Some(way[1].atan2(way[0]).to_degrees());
                    }
                }
                Some("nophysics") => {}
                // The animation moves it (only turns it, for angle deltas).
                Some(mode) => {
                    let (sy, cy) = yaw.to_radians().sin_cos();
                    if mode != "angle deltas" {
                        e.origin[0] += step.0[0] * cy - step.0[1] * sy;
                        e.origin[1] += step.0[0] * sy + step.0[1] * cy;
                        if mode == "nogravity" {
                            e.origin[2] += step.0[2];
                        }
                    }
                    e.angles[1] += step.1;
                }
            }
            // On the ground, as the engine's actor physics keeps it (not
            // at a spawner's or node's height, a counter's top): the floor
            // under it, from a step above its feet to well below.
            if !matches!(b.animmode.as_deref(), Some("nophysics" | "nogravity")) {
                let o = e.origin;
                if let Some(hit) = trace([o[0], o[1], o[2] + 18.0], [o[0], o[1], o[2] - 256.0], TraceKind::Actor) {
                    e.origin[2] = hit.1[2];
                }
            }
            // Facing: the script's say, else where it goes.
            let want = match &b.orient {
                Some((m, arg)) if m == "face angle" => arg.as_f32(),
                Some((m, arg)) if m == "face point" => arg.as_vec().map(|p| (p[1] - e.origin[1]).atan2(p[0] - e.origin[0]).to_degrees()),
                Some((m, arg)) if m == "face direction" => arg.as_vec().map(|v| v[1].atan2(v[0]).to_degrees()),
                Some((m, _)) if m == "face motion" => face,
                Some((m, _)) if m == "face enemy" || m == "face enemy or motion" => {
                    b.face_enemy.map(|p| (p[1] - e.origin[1]).atan2(p[0] - e.origin[0]).to_degrees()).or(face)
                }
                Some(_) => None,
                None => b.face_enemy.map(|p| (p[1] - e.origin[1]).atan2(p[0] - e.origin[0]).to_degrees()).or(face),
            };
            if let Some(w) = want {
                // At most 15 degrees a 50 ms frame.
                let diff = (w - e.angles[1] + 540.0).rem_euclid(360.0) - 180.0;
                let most = 300.0 * dt;
                e.angles[1] += diff.clamp(-most, most);
            }
            let at_goal = b.reacquire.is_none() && dist <= radius;
            self.brains.insert(id, b);
            if at_goal {
                vm.notify(o, "goal", Vec::new());
            }
        }
    }

    /// Run `animscripts/<script>::main` on the actor if it isn't already
    /// (the old one told `killanimscript`); its `movemode` kept.
    fn set_anim_script(vm: &mut Vm, o: ObjId, b: &mut Brain, script: &str, movemode: &str) {
        if b.script != script {
            vm.notify(o, "killanimscript", Vec::new());
            b.script = script.to_owned();
            b.animmode = None;
            b.orient = None;
            b.movemode = movemode.to_owned();
            vm.set_field(o, "movemode", Value::str(movemode));
            if let Some(f) = vm.func(&format!("animscripts/{script}"), "main") {
                vm.spawn_thread(f, Value::Object(o), Vec::new());
            }
        } else if b.movemode != movemode {
            b.movemode = movemode.to_owned();
            vm.set_field(o, "movemode", Value::str(movemode));
            vm.notify(o, "movemode", Vec::new());
        }
    }
}

/// A path node as the map compiled it: where, its type (CoD4's
/// `nodeType`: 1 plain, 2..9 cover...), and its links (node, distance).
#[derive(Clone, Debug)]
pub struct PathNode {
    pub origin: [f32; 3],
    pub kind: i32,
    pub angle: f32,
    pub links: Vec<(usize, f32)>,
}

/// Routes over the path nodes.
impl Level {
    /// The node nearest a point (within 80 units up or down, as CoD4's
    /// node tests allow, if any is).
    fn nearest_node(&self, p: [f32; 3]) -> Option<usize> {
        let d = |n: &PathNode| {
            let dz = (n.origin[2] - p[2]).abs();
            (n.origin[0] - p[0]).hypot(n.origin[1] - p[1]) + if dz > 80.0 { 10_000.0 + dz } else { dz }
        };
        self.path_nodes.iter().enumerate().min_by(|a, b| d(a.1).total_cmp(&d(b.1))).map(|(i, _)| i)
    }

    /// From a point to a goal over the nodes: the points to walk through
    /// (the goal last); None when no nodes join them.
    pub fn route(&self, from: [f32; 3], to: [f32; 3]) -> Option<Vec<[f32; 3]>> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let (start, end) = (self.nearest_node(from)?, self.nearest_node(to)?);
        let n = self.path_nodes.len();
        let h = |i: usize| {
            let a = self.path_nodes[i].origin;
            let b = self.path_nodes[end].origin;
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let mut cost = vec![f32::INFINITY; n];
        let mut prev = vec![usize::MAX; n];
        let mut open = BinaryHeap::new();
        cost[start] = 0.0;
        open.push(Reverse(((h(start) * 16.0) as u64, start)));
        while let Some(Reverse((_, i))) = open.pop() {
            if i == end {
                break;
            }
            for &(j, d) in &self.path_nodes[i].links {
                if j >= n {
                    continue;
                }
                let c = cost[i] + d.max(1.0);
                if c < cost[j] {
                    cost[j] = c;
                    prev[j] = i;
                    open.push(Reverse((((c + h(j)) * 16.0) as u64, j)));
                }
            }
        }
        if !cost[end].is_finite() {
            return None;
        }
        let mut nodes = vec![end];
        while let Some(&i) = nodes.last()
            && prev[i] != usize::MAX
        {
            nodes.push(prev[i]);
        }
        nodes.reverse();
        let mut out: Vec<[f32; 3]> = nodes.into_iter().map(|i| self.path_nodes[i].origin).collect();
        out.push(to);
        Some(out)
    }
}

/// What the actors notice.
impl Level {
    /// Are two sides enemies? (allies and axis; neutral fights no one.)
    fn hostile(a: &str, b: &str) -> bool {
        matches!((a, b), ("allies", "axis") | ("axis", "allies"))
    }

    /// The actor's enemy, as CoD4's senses pick it: the nearest hostile it
    /// sees (not `ignoreme`; none if it `ignoreall`s); kept once picked
    /// while that one lives. Tells `enemy` when it changes.
    fn sense(&mut self, vm: &mut Vm, id: EntId, o: ObjId) {
        let Some(me) = self.ent(id) else { return };
        let (team, at) = (me.team.clone(), me.origin);
        let current = vm.entity_of(&vm.field(o, "enemy")).filter(|t| self.ent(*t).is_some_and(|t| t.alive || t.kind == Kind::Player));
        if vm.field(o, "ignoreall").as_i32().unwrap_or(0) != 0 {
            if current.is_some() {
                vm.set_field(o, "enemy", Value::Undefined);
            }
            return;
        }
        let ignored = |vm: &Vm, t: EntId| vm.entity_obj(t).is_some_and(|to| vm.field(to, "ignoreme").as_i32().unwrap_or(0) != 0);
        let best = self
            .ents
            .iter()
            .filter(|t| t.id != id && (t.kind == Kind::Player || (t.kind == Kind::Ai && t.alive)))
            .filter(|t| {
                let side = if t.kind == Kind::Player { "allies" } else { t.team.as_str() };
                Self::hostile(&team, side)
            })
            .filter(|t| self.sight.get(&(id, t.id)).copied().unwrap_or(false) && !ignored(vm, t.id))
            .map(|t| (t.id, (t.origin[0] - at[0]).powi(2) + (t.origin[1] - at[1]).powi(2) + (t.origin[2] - at[2]).powi(2)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(t, _)| t);
        let keep = current.filter(|t| !ignored(vm, *t));
        let pick = keep.or(best);
        if pick != current {
            let v = pick.map_or(Value::Undefined, |t| vm.entity(t));
            vm.set_field(o, "enemy", v.clone());
            if pick.is_some() {
                vm.notify(o, "enemy", vec![v]);
            }
        }
    }

    /// Pairs the game should trace this frame: each living actor and the
    /// hostiles near enough to matter (within `maxsightdistsqrd`).
    pub fn sight_pairs(&self, vm: &Vm) -> Vec<(EntId, EntId)> {
        // (The player's on every actor: whether a forced spawn may delete
        // it unseen, [`Level::make_actor_room`].)
        let mut out: Vec<(EntId, EntId)> = self.ents.iter().filter(|e| e.kind == Kind::Ai && e.alive).map(|e| (PLAYER, e.id)).collect();
        for a in self.ents.iter().filter(|e| e.kind == Kind::Ai && e.alive) {
            let far = vm.entity_obj(a.id).and_then(|o| vm.field(o, "maxsightdistsqrd").as_f32()).unwrap_or(8192.0 * 8192.0);
            for t in self.ents.iter().filter(|t| t.id != a.id && (t.kind == Kind::Player || (t.kind == Kind::Ai && t.alive))) {
                let side = if t.kind == Kind::Player { "allies" } else { t.team.as_str() };
                if !Self::hostile(&a.team, side) {
                    continue;
                }
                let d2 = (t.origin[0] - a.origin[0]).powi(2) + (t.origin[1] - a.origin[1]).powi(2) + (t.origin[2] - a.origin[2]).powi(2);
                if d2 <= far {
                    out.push((a.id, t.id));
                }
            }
        }
        out
    }
}

/// A weapon as the scripts ask about it (`weaponclass`, `weaponfiretime`...).
#[derive(Clone, Debug)]
pub struct WeaponInfo {
    /// `rifle`, `mg`, `smg`, `spread`, `pistol`, `grenade`, `rocketlauncher`...
    pub class: String,
    pub clip_size: i32,
    /// Seconds between shots.
    pub fire_time: f32,
    /// 0 full auto, 1 single shot, 2..4 bursts.
    pub fire_type: i32,
    pub bolt_action: bool,
    pub fight_dist: f32,
    pub max_dist: f32,
    /// Its model in the world (a grenade set down).
    pub world_model: String,
    /// Most ammo carried (`maxAmmo`).
    pub max_ammo: i32,
    /// Its blast: effect, sound, radius, most and least damage.
    pub blast: Option<(String, String, f32, f32, f32)>,
}

impl Default for WeaponInfo {
    fn default() -> Self {
        WeaponInfo { class: "rifle".into(), clip_size: 30, fire_time: 0.1, fire_type: 0, bolt_action: false, fight_dist: 768.0, max_dist: 4096.0, world_model: String::new(), max_ammo: 0, blast: None }
    }
}

impl WeaponInfo {
    /// CoD4's `weapClass` number as the scripts name it.
    pub fn class_name(n: i32) -> &'static str {
        ["rifle", "mg", "smg", "spread", "pistol", "grenade", "rocketlauncher", "turret", "non-player", "item"].get(n.max(0) as usize).copied().unwrap_or("rifle")
    }
}

/// The animscript for holding a node of this type (CoD4's
/// `Actor_Cover_GetAttackScript`); none for plain path nodes.
fn cover_script(node_type: &str, threatened: bool) -> Option<&'static str> {
    Some(match node_type {
        "Cover Stand" => "cover_stand",
        "Cover Crouch" | "Cover Crouch Window" | "Turret" => "cover_crouch",
        "Cover Prone" => "cover_prone",
        "Cover Right" => "cover_right",
        "Cover Left" => "cover_left",
        "Cover Wide Right" => "cover_wide_right",
        "Cover Wide Left" => "cover_wide_left",
        "Conceal Stand" => "concealment_stand",
        "Conceal Crouch" => "concealment_crouch",
        "Conceal Prone" => "concealment_prone",
        "Guard" => {
            if threatened {
                "combat"
            } else {
                "stop"
            }
        }
        _ => return None,
    })
}

/// Vehicles' turrets.
impl Level {
    /// Each aimed turret turned toward its target (90 degrees a second
    /// across, 45 up and down), told `turret_on_target` and
    /// `turret_rotate_stopped` once it's there (within a degree).
    fn turn_turrets(&mut self, vm: &mut Vm, dt: f32) {
        let ids: Vec<EntId> = self.turret_aims.keys().copied().collect();
        for id in ids {
            let Some(from) = self.ent(id).map(|e| [e.origin[0], e.origin[1], e.origin[2] + 64.0]) else {
                self.turret_aims.remove(&id);
                continue;
            };
            let Some(a) = self.turret_aims.get(&id).cloned() else { continue };
            if a.on_target {
                continue;
            }
            let to = match a.ent {
                Some(t) => match self.ent(t) {
                    Some(e) => e.origin,
                    None => continue,
                },
                None => a.at,
            };
            let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
            let want_yaw = d[1].atan2(d[0]).to_degrees();
            let want_pitch = -(d[2].atan2(d[0].hypot(d[1]))).to_degrees();
            let yaw = a.yaw.unwrap_or_else(|| self.ent(id).map_or(0.0, |e| e.angles[1]));
            let dy = (want_yaw - yaw + 540.0).rem_euclid(360.0) - 180.0;
            let dp = want_pitch - a.pitch;
            let (sy, sp) = (90.0 * dt, 45.0 * dt);
            let yaw = yaw + dy.clamp(-sy, sy);
            let pitch = a.pitch + dp.clamp(-sp, sp);
            let there = dy.abs() <= 1.0 && dp.abs() <= 1.0;
            if let Some(m) = self.turret_aims.get_mut(&id) {
                m.yaw = Some(yaw);
                m.pitch = pitch;
                m.on_target = there;
            }
            if there && let Some(o) = vm.entity_obj(id) {
                vm.notify(o, "turret_on_target", Vec::new());
                vm.notify(o, "turret_rotate_stopped", Vec::new());
            }
        }
    }
}

/// Nodes the actors hold.
impl Level {
    /// A cover node within 16 units (flat, 80 up or down) of a point: one
    /// an actor arriving there takes.
    fn node_at(&self, vm: &Vm, p: [f32; 3]) -> Option<ObjId> {
        self.nodes
            .iter()
            .filter(|(o, _, _)| vm.field(*o, "type").as_str().is_some_and(|t| cover_script(t, false).is_some()))
            .filter_map(|(o, _, _)| {
                let n = vm.field(*o, "origin").as_vec()?;
                let flat = (n[0] - p[0]).hypot(n[1] - p[1]);
                (flat < 16.0 && (n[2] - p[2]).abs() < 80.0).then_some((*o, flat))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(o, _)| o)
    }
}

/// Cover the actors look for.
impl Level {
    /// The cover node within its goal radius that best faces its enemy
    /// (or the nearest, with no enemy).
    fn best_cover(&self, vm: &Vm, id: EntId) -> Option<ObjId> {
        self.cover_list(vm, id, 1).into_iter().next()
    }

    /// The best `n` cover nodes within its goal radius, best first.
    fn cover_list(&self, vm: &Vm, id: EntId, n: usize) -> Vec<ObjId> {
        let (Some(e), Some(o)) = (self.ent(id), vm.entity_obj(id)) else { return Vec::new() };
        let goal = e.goal.unwrap_or(e.origin);
        let radius = vm.field(o, "goalradius").as_f32().unwrap_or(4.0).max(64.0);
        let enemy = vm.entity_of(&vm.field(o, "enemy")).and_then(|t| self.ent(t)).map(|t| t.origin);
        let taken: std::collections::HashSet<ObjId> = self.brains.iter().filter(|(k, _)| **k != id).filter_map(|(_, b)| b.claimed.or(b.goal_node)).collect();
        let list: Vec<(ObjId, f32)> = self.nodes
            .iter()
            .filter(|(n, _, _)| !taken.contains(n))
            .filter(|(n, _, _)| vm.field(*n, "type").as_str().is_some_and(|t| t != "Guard" && cover_script(t, false).is_some()))
            .filter_map(|(n, _, _)| {
                let p = vm.field(*n, "origin").as_vec()?;
                if (p[0] - goal[0]).hypot(p[1] - goal[1]) > radius {
                    return None;
                }
                let near = (p[0] - e.origin[0]).hypot(p[1] - e.origin[1]);
                // Facing the enemy: the node's forward against the way to it.
                let facing = enemy.map_or(0.0, |t| {
                    let yaw = vm.field(*n, "angles").as_vec().map_or(0.0, |a| a[1]).to_radians();
                    let d = [t[0] - p[0], t[1] - p[1]];
                    let l = d[0].hypot(d[1]).max(0.001);
                    (yaw.cos() * d[0] + yaw.sin() * d[1]) / l
                });
                Some((*n, near - facing * 400.0))
            })
            .collect::<Vec<_>>();
        let mut list = list;
        list.sort_by(|a, b| a.1.total_cmp(&b.1));
        list.into_iter().take(n).map(|(n, _)| n).collect()
    }

    /// Where its enemy is looked at: the player's eye, an actor's chest.
    fn look_pos(&self, t: EntId) -> Option<[f32; 3]> {
        let e = self.ent(t)?;
        Some([e.origin[0], e.origin[1], e.origin[2] + if t == PLAYER { 60.0 } else { 48.0 }])
    }

    /// A step of `dist` to one side or the other (randomly first) of the
    /// way to its enemy, that it has room for and that sees them (CoD4's
    /// exposed reacquire step).
    fn reacquire_step(&self, vm: &Vm, id: EntId, dist: f32, side: f32) -> Option<[f32; 3]> {
        let e = self.ent(id)?;
        let o = vm.entity_obj(id)?;
        let t = vm.entity_of(&vm.field(o, "enemy"))?;
        let look = self.look_pos(t)?;
        let to = self.ent(t)?.origin;
        let fw = [to[0] - e.origin[0], to[1] - e.origin[1]];
        let l = fw[0].hypot(fw[1]);
        if l <= 1.0 {
            return None;
        }
        let across = [fw[1] / l, -fw[0] / l];
        let first = if side < 0.5 { 1.0 } else { -1.0 };
        [first, -first].into_iter().find_map(|sign| {
            let p = [e.origin[0] + across[0] * dist * sign, e.origin[1] + across[1] * dist * sign, e.origin[2]];
            let knee = |q: [f32; 3]| [q[0], q[1], q[2] + 18.0];
            let room = trace(knee(e.origin), knee(p), TraceKind::Physics).is_none();
            let sees = trace([p[0], p[1], p[2] + 60.0], look, TraceKind::Sight).is_none();
            (room && sees).then_some(p)
        })
    }

    /// A path's end toward its enemy: them, clipped to its goal (direct),
    /// or the path node in its goal nearest it that sees them within half
    /// the way (proximate).
    fn reacquire_path(&self, vm: &Vm, id: EntId, direct: bool) -> Option<[f32; 3]> {
        let e = self.ent(id)?;
        let o = vm.entity_obj(id)?;
        let t = vm.entity_of(&vm.field(o, "enemy"))?;
        let to = self.ent(t)?.origin;
        let goal = e.goal.unwrap_or(e.origin);
        let r = vm.field(o, "goalradius").as_f32().unwrap_or(4.0).max(1.0);
        if direct {
            let d = [to[0] - goal[0], to[1] - goal[1]];
            let l = d[0].hypot(d[1]);
            // (Stopping short of them, not on top.)
            let k = if l <= r { (l - 64.0).max(0.0) / l.max(1.0) } else { r / l };
            return Some([goal[0] + d[0] * k, goal[1] + d[1] * k, if l <= r { to[2] } else { goal[2] }]);
        }
        let look = self.look_pos(t)?;
        let half = ((to[0] - e.origin[0]).powi(2) + (to[1] - e.origin[1]).powi(2) + (to[2] - e.origin[2]).powi(2)).sqrt() * 0.5;
        let mut near: Vec<([f32; 3], f32)> = self
            .path_nodes.iter().map(|n| n.origin)
            .filter(|p| (p[0] - goal[0]).hypot(p[1] - goal[1]) <= r)
            .filter(|p| ((p[0] - to[0]).powi(2) + (p[1] - to[1]).powi(2) + (p[2] - to[2]).powi(2)).sqrt() <= half)
            .map(|p| (p, (p[0] - e.origin[0]).hypot(p[1] - e.origin[1])))
            .collect();
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        near.into_iter().take(8).find(|(p, _)| trace([p[0], p[1], p[2] + 60.0], look, TraceKind::Sight).is_none()).map(|(p, _)| p)
    }
}

/// Actors alive at once, as CoD4's actor slots.
const MAX_ACTORS: usize = 32;

impl Level {
    /// A forced spawn with every actor slot taken: a deletable actor (no
    /// "undeletable" flag, 4) is told `death` and deleted, as CoD4 picks
    /// it: the farthest the player can't see, else the farthest behind
    /// them, else the farthest outside 45 degrees of their view, else the
    /// farthest.
    fn make_actor_room(&mut self, vm: &mut Vm) -> bool {
        let Some(p) = self.ent(PLAYER) else { return false };
        // (Debug, with `GSC_DELETE_TRACE`: the actors holding the slots.)
        if std::env::var_os("GSC_DELETE_TRACE").is_some() {
            for e in self.ents.iter().filter(|e| e.kind == Kind::Ai && e.alive) {
                let f = |k: &str| vm.entity_obj(e.id).map(|o| vm.field(o, k).display().to_string()).unwrap_or_default();
                let sp = vm.entity_obj(e.id).map(|o| vm.field(o, "spawner")).and_then(|v| v.as_obj()).map(|o| vm.field(o, "targetname").display().to_string()).unwrap_or_default();
                eprintln!("gsc slot {} {} {} at {:?} spawner {sp} deleteai {} noteworthy {} goal {:?}", e.id, e.classname, f("targetname"), e.origin.map(f32::round), f("script_deleteai"), f("script_noteworthy"), self.brains.get(&e.id).map(|b| (b.goal_node.is_some(), b.script.clone())));
                eprintln!("    linked {:?} anim {:?} goal {:?} radius {}", e.linked, e.anim_state.anim.as_ref().map(|a| (a.name.clone(), a.scripted.is_some(), a.ended)), e.goal.map(|g| g.map(f32::round)), f("goalradius"));
            }
        }
        let eye = [p.origin[0], p.origin[1], p.origin[2] + 60.0];
        let (fw, _, _) = crate::vm::angle_vectors(p.angles);
        let d2 = |o: [f32; 3]| (0..3).map(|i| (o[i] - eye[i]).powi(2)).sum::<f32>();
        let along = |o: [f32; 3]| (0..3).map(|i| fw[i] * (o[i] - eye[i])).sum::<f32>();
        let deletable: Vec<(EntId, [f32; 3])> = self.ents.iter().filter(|e| e.kind == Kind::Ai && e.alive && e.spawnflags & 4 == 0).map(|e| (e.id, e.origin)).collect();
        let farthest = |pick: &dyn Fn(EntId, [f32; 3]) -> bool| deletable.iter().filter(|(id, o)| pick(*id, *o)).max_by(|a, b| d2(a.1).total_cmp(&d2(b.1))).map(|(id, _)| *id);
        let Some(id) = farthest(&|id, _| self.sight.get(&(PLAYER, id)) == Some(&false))
            .or_else(|| farthest(&|_, o| along(o) < 0.0))
            .or_else(|| farthest(&|_, o| along(o).powi(2) < d2(o) * 0.5))
            .or_else(|| farthest(&|_, _| true))
        else {
            return false;
        };
        if let Some(o) = vm.entity_obj(id) {
            vm.notify(o, "death", Vec::new());
        }
        self.remove(vm, id);
        true
    }
}

/// Units a second to a mile an hour.
const MPH: f32 = 17.6;

/// A vehicle's turret turning to its target: the target (an entity, or a
/// point), where it points (world yaw and pitch), and whether it's been
/// told it got there.
#[derive(Clone, Debug, Default)]
pub struct TurretAim {
    pub ent: Option<EntId>,
    pub at: [f32; 3],
    pub yaw: Option<f32>,
    pub pitch: f32,
    pub on_target: bool,
}

/// A vehicle's driving: on its path (from one node to the next) or to a
/// helicopter's goal, at its speed (units a second) going to its target.
#[derive(Clone, Debug, Default)]
pub struct VehMove {
    pub from: Option<EntId>,
    pub next: Option<EntId>,
    pub moving: bool,
    pub speed: f32,
    pub target: f32,
    resume: f32,
    accel: f32,
    /// `setwaitnode`: tell `reached_wait_node` there.
    wait: Option<EntId>,
    /// `setswitchnode`: at the first, on along the path from the second.
    switch: Option<(EntId, EntId)>,
    /// Told `reached_end_node` (it waits there, a switch may yet come).
    end_told: bool,
    goal: Option<[f32; 3]>,
    near: f32,
    near_sent: bool,
    at: f64,
}

/// Vehicles, as the engine drives them.
impl Level {
    /// A vehicle path node (`info_vehicle_node`)?
    fn is_vehicle_node(&self, id: EntId) -> bool {
        self.ent(id).is_some_and(|e| e.classname.contains("vehicle_node"))
    }

    /// The path node after this one (its `target`).
    fn next_vehicle_node(&self, n: EntId) -> Option<EntId> {
        let target = self.ent(n)?.get("target")?.to_owned();
        self.ents.iter().find(|e| e.classname.contains("vehicle_node") && e.get("targetname") == Some(target.as_str())).map(|e| e.id)
    }

    fn drive_vehicles(&mut self, vm: &mut Vm, now: f64) {
        let ids: Vec<EntId> = self.vehicles.keys().copied().collect();
        for id in ids {
            let Some(mut v) = self.vehicles.remove(&id) else { continue };
            let dt = ((now - v.at) as f32).clamp(0.0, 0.25);
            v.at = now;
            if self.ent(id).is_none() {
                continue;
            }
            // Speed toward its target.
            if v.accel <= 0.0 {
                v.speed = v.target;
            } else {
                let step = v.accel * dt;
                v.speed += (v.target - v.speed).clamp(-step, step);
            }
            let mut left = v.speed * dt;
            let mut notes: Vec<(&str, Vec<Value>)> = Vec::new();
            if let Some(goal) = v.goal {
                // To its goal, flying straight.
                let at = self.ent(id).map(|e| e.origin).unwrap_or(goal);
                let d = [goal[0] - at[0], goal[1] - at[1], goal[2] - at[2]];
                let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let k = if dist > 0.0 { (left / dist).min(1.0) } else { 1.0 };
                if let Some(e) = self.ent_mut(id) {
                    e.origin = [at[0] + d[0] * k, at[1] + d[1] * k, at[2] + d[2] * k];
                    if d[0].hypot(d[1]) > 1.0 {
                        e.angles[1] = d[1].atan2(d[0]).to_degrees();
                    }
                }
                let remaining = dist * (1.0 - k);
                if !v.near_sent && v.near > 0.0 && remaining <= v.near {
                    v.near_sent = true;
                    notes.push(("near_goal", Vec::new()));
                }
                if remaining <= 1.0 {
                    v.goal = None;
                    notes.push(("goal", Vec::new()));
                }
            } else if v.moving {
                // Along its path, node by node.
                let mut guard = 0;
                while left > 0.0 && guard < 8 {
                    guard += 1;
                    // (At its last node: on along a switch made there, as a
                    // detour's, else done.)
                    if v.next.is_none()
                        && let Some((at, to)) = v.switch
                        && Some(at) == v.from
                    {
                        v.switch = None;
                        v.next = Some(to);
                    }
                    let Some(next) = v.next else {
                        if !v.end_told {
                            v.end_told = true;
                            notes.push(("reached_end_node", Vec::new()));
                        }
                        break;
                    };
                    v.end_told = false;
                    let Some(to) = self.ent(next).map(|e| e.origin) else { break };
                    let at = self.ent(id).map(|e| e.origin).unwrap_or(to);
                    let d = [to[0] - at[0], to[1] - at[1], to[2] - at[2]];
                    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if dist > left {
                        let k = left / dist;
                        if let Some(e) = self.ent_mut(id) {
                            e.origin = [at[0] + d[0] * k, at[1] + d[1] * k, at[2] + d[2] * k];
                            if d[0].hypot(d[1]) > 0.5 {
                                e.angles[1] = d[1].atan2(d[0]).to_degrees();
                            }
                        }
                        left = 0.0;
                    } else {
                        // Reached it.
                        left -= dist;
                        if let Some(e) = self.ent_mut(id) {
                            e.origin = to;
                        }
                        if v.wait == Some(next) {
                            v.wait = None;
                            notes.push(("reached_wait_node", Vec::new()));
                        }
                        // The node's own speed (mph), as the engine takes it.
                        if let Some(mph) = self.ent(next).and_then(|e| e.get("speed")).and_then(|v| v.trim().parse::<f32>().ok()) {
                            v.target = mph.max(0.0) * MPH;
                            v.resume = v.target;
                            if v.accel <= 0.0 {
                                v.accel = 10.0 * MPH;
                            }
                        }
                        v.from = Some(next);
                        v.next = match v.switch {
                            Some((at, to)) if at == next => {
                                v.switch = None;
                                Some(to)
                            }
                            _ => self.next_vehicle_node(next),
                        };
                    }
                }
            }
            // A ground vehicle on the ground under its path (CoD4 settles
            // them on the terrain; its nodes can be a little up): a car on a
            // distant road, not floating over it. Aircraft keep their paths.
            let ty = vm.entity_obj(id).map(|o| vm.field(o, "vehicletype").display().to_ascii_lowercase()).unwrap_or_default();
            const AIR: [&str; 16] = ["heli", "blackhawk", "cobra", "hind", "mi17", "mi28", "ch46", "seaknight", "littlebird", "mh53", "harrier", "f15", "mig", "c130", "plane", "pavelow"];
            if v.moving && !ty.is_empty() && !AIR.iter().any(|a| ty.contains(a))
                && let Some(e) = self.ent_mut(id)
                && let Some(hit) = trace([e.origin[0], e.origin[1], e.origin[2] + 48.0], [e.origin[0], e.origin[1], e.origin[2] - 200.0], TraceKind::Actor)
            {
                e.origin[2] = hit.1[2];
            }
            self.vehicles.insert(id, v);
            if let Some(o) = vm.entity_obj(id) {
                for (ev, args) in notes {
                    vm.notify(o, ev, args);
                }
            }
        }
    }
}

/// A missile in flight: to its target (and offset) or along its way.
#[derive(Clone, Debug)]
struct Missile {
    id: EntId,
    target: Option<EntId>,
    offset: [f32; 3],
    dir: [f32; 3],
    until: f64,
}

impl Level {
    fn fly_missiles(&mut self, vm: &mut Vm, now: f64) {
        const SPEED: f32 = 3000.0;
        let dt = 0.05_f32;
        let missiles = std::mem::take(&mut self.missiles);
        for mut m in missiles {
            let Some(at) = self.ent(m.id).map(|e| e.origin) else { continue };
            let goal = m.target.and_then(|t| self.ent(t)).map(|t| [t.origin[0] + m.offset[0], t.origin[1] + m.offset[1], t.origin[2] + m.offset[2]]);
            let d = goal.map_or(m.dir, |g| [g[0] - at[0], g[1] - at[1], g[2] - at[2]]);
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(0.001);
            let step = SPEED * dt;
            let arrived = goal.is_some() && len <= step;
            if let Some(e) = self.ent_mut(m.id) {
                let k = step.min(len) / len;
                e.origin = [at[0] + d[0] * k, at[1] + d[1] * k, at[2] + d[2] * k];
            }
            m.dir = [d[0] / len, d[1] / len, d[2] / len];
            // (Past its target point it flies on; a missile ends on impact
            // or running out, not on getting there.)
            if arrived {
                m.target = None;
                m.until = m.until.min(now + 3.0);
            }
            if now >= m.until {
                if let Some(o) = vm.entity_obj(m.id) {
                    vm.notify(o, "death", Vec::new());
                }
                self.remove(vm, m.id);
            } else {
                self.missiles.push(m);
            }
        }
    }
}

/// The player on a vehicle's gun.
impl Level {
    /// The vehicle the player mans, and its gun: `setvehweapon`'s, or the
    /// level's weapon named for its type (`seaknight_mark19`).
    pub fn player_turret(&self, vm: &Vm) -> Option<(EntId, String)> {
        let link = self.player_link.as_ref().filter(|l| l.turret)?;
        let id = link.to;
        if let Some(w) = self.vehicle_weapons.get(&id) {
            return Some((id, w.clone()));
        }
        let ty = vm.entity_obj(id).map(|o| vm.field(o, "vehicletype").display().to_ascii_lowercase()).unwrap_or_default();
        let base = ty.split('_').next().unwrap_or("").to_owned();
        let w = self.weapons.keys().filter(|w| !base.is_empty() && w.starts_with(&base)).min().cloned()?;
        Some((id, w))
    }

    /// The player's turret fired: its vehicle is told (`turret_fire`).
    pub fn turret_fired(&mut self, vm: &mut Vm, vehicle: EntId) {
        if let Some(o) = vm.entity_obj(vehicle) {
            vm.notify(o, "turret_fire", Vec::new());
        }
    }

    /// A blast: every living actor within `radius` hurt, more at the middle
    /// (`radiusdamage`'s falloff), by the player.
    pub fn blast(&mut self, vm: &mut Vm, at: [f32; 3], radius: f32, most: f32, least: f32) {
        self.blast_by(vm, at, radius, most, least, Some(PLAYER));
    }

    /// Grenades whose fuse is out go off.
    fn fuse_grenades(&mut self, vm: &mut Vm, now: f64) {
        let (out, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.grenades).into_iter().partition(|g| g.1 <= now && !g.2.contains("claymore"));
        self.grenades = keep;
        for (id, _, weapon, by) in out {
            let Some(at) = self.ent(id).map(|e| e.origin) else { continue };
            if let Some((fx, sound, radius, most, least)) = self.weapons.get(&weapon).and_then(|w| w.blast.clone()) {
                if !fx.is_empty() {
                    self.commands.push(Command::PlayFx { fx, at, forward: [0.0, 0.0, 1.0] });
                }
                if !sound.is_empty() {
                    self.commands.push(Command::PlaySound { ent: None, alias: sound, at, looped: false, notify: None });
                }
                self.blast_by(vm, at, radius, most, least, by);
            }
            if let Some(o) = vm.entity_obj(id) {
                vm.notify(o, "explode", vec![Value::Vector(at)]);
            }
            self.remove(vm, id);
        }
    }

    /// A blast's damage to the actors within its radius, by whom.
    pub fn blast_by(&mut self, vm: &mut Vm, at: [f32; 3], radius: f32, most: f32, least: f32, by: Option<EntId>) {
        // (Actors and vehicles, as CoD4's radius damage.)
        let hit: Vec<(EntId, f32, [f32; 3])> = self
            .ents
            .iter()
            .filter(|e| matches!(e.kind, Kind::Ai | Kind::Vehicle) && e.alive)
            .filter_map(|e| {
                let d = [e.origin[0] - at[0], e.origin[1] - at[1], e.origin[2] + 40.0 - at[2]];
                let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                (dist < radius).then(|| (e.id, most + (least - most) * dist / radius, d))
            })
            .collect();
        for (id, amount, dir) in hit {
            self.damaged_at(vm, id, amount.round() as i32, by, dir, at, "torso_upper");
        }
    }
}

impl Level {
    /// Brush models' true shapes (model `*n`'s brushes as planes), for
    /// the entities that use them.
    pub fn set_brush_shapes(&mut self, shape_of: impl Fn(usize) -> Vec<Vec<([f32; 3], f32)>>) {
        for e in &mut self.ents {
            if let Some(n) = e.model.strip_prefix('*').and_then(|n| n.parse::<usize>().ok()) {
                let shape = shape_of(n);
                if !shape.is_empty() {
                    e.shape = Some(Rc::new(shape));
                }
            }
        }
    }
}

#[cfg(test)]
mod angle_tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        let fa = axes(a);
        let fb = axes(b);
        (0..3).all(|i| (0..3).all(|j| (fa[i][j] - fb[i][j]).abs() < 1e-3))
    }

    #[test]
    fn compose_and_relative_round_trip() {
        for (p, c) in [([0.0, 90.0, 0.0], [0.0, 0.0, 0.0]), ([-90.0, 90.0, 0.0], [10.0, 30.0, 5.0]), ([20.0, -45.0, 10.0], [0.0, 120.0, -30.0])] {
            let rel = relative_angles(p, c);
            assert!(close(compose_angles(p, rel), c), "{p:?} {c:?} -> {rel:?}");
        }
        // Yaw only adds, as before.
        assert!(close(compose_angles([0.0, 90.0, 0.0], [0.0, 30.0, 0.0]), [0.0, 120.0, 0.0]));
    }
}
