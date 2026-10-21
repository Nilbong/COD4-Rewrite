//! The whole F.N.G. base, open to walk round, with something to do in each
//! part, as the training level's own script set it up (`maps/killhouse.gsc`,
//! `killhouse_code.gsc`):
//!
//! - **Doors:** the ship hangar's door slides open (`moveX( 150 )`) and its
//!   clip goes, and the rifle range's door swings open (`yaw + 88`), so
//!   nothing walls the player into the hangar.
//! - **Pop-up targets:** the level's target brush models turn about their
//!   pivots (`rotatePitch( ±90 )`): down until called up, up until shot.
//! - **Rifle range:** at the stall, the three lane targets pop up one at a
//!   time; hits, shots and accuracy are counted.
//! - **Killhouse:** the deck mock-up's timed run (`deck_training`): five
//!   positions, each popping three hostiles (and maybe friendlies) when
//!   entered, then the finish; under 60 seconds, no friendly hit, with
//!   CoD4's accuracy bonus. The best five times are kept in the stats and
//!   shown on a board at the start.
//! - **Obstacle course:** timed from Mac's start line to the end, past the
//!   wire and the sprint; its best five kept too.
//! - **Travel posts:** a post in each area takes the player to another.
//!
//! In the range and the hangar's mock-up the player is armed ([`armed`]):
//! first person, the class's guns, fire and reload; elsewhere, as before.

use crate::units::{pos, u};
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(super) fn build(app: &mut App) {
    app.add_systems(OnEnter(crate::state::GameState::InGame), setup.after(crate::world::load_map).in_set(crate::state::Setup::Content))
        .add_systems(OnExit(crate::state::GameState::InGame), leave)
        .add_systems(
            Update,
            (zones, debug_driver, shots, hits, rifle_range, deck_run, walkthrough, obstacle_run, animate, pose_parts, travel, class_change, publish)
                .chain()
                .after(crate::weapons::WeaponSet)
                .run_if(|| super::active()),
        );
}

// ---------------------------------------------------------------- the level

/// The brush models Headquarters moves: the doors, and each target with its
/// aim-assist brush (hidden).
pub fn movable_brushes(ents: &[iw3::ents::Entity]) -> HashSet<usize> {
    let model = |e: &iw3::ents::Entity| e.get("model")?.strip_prefix('*')?.parse::<usize>().ok();
    let by_name: HashMap<&str, &iw3::ents::Entity> = ents.iter().filter_map(|e| Some((e.get("targetname")?, e))).collect();
    let mut out = HashSet::new();
    for e in ents.iter().filter(|e| e.classname() == "script_brushmodel") {
        let Some(n) = model(e) else { continue };
        if matches!(e.get("targetname"), Some(DOOR | CLIP | RANGE_DOOR)) {
            out.insert(n);
        }
        if !matches!(e.get("script_noteworthy"), Some(t) if TARGET_KINDS.contains(&t)) {
            continue;
        }
        out.insert(n);
        let aim = e.get("target").and_then(|t| by_name.get(t)).and_then(|pivot| pivot.get("target")).and_then(|t| by_name.get(t)).and_then(|a| model(a));
        out.extend(aim);
    }
    out
}

const DOOR: &str = "ship_hanger_door";
const CLIP: &str = "ship_hanger_clip";
const RANGE_DOOR: &str = "rifle_range_door";
/// The hangar door's slide (`moveX( 40 )`, then `moveX( 110 )`), CoD units.
const DOOR_SLIDE: f32 = 150.0;
/// The range door's swing (degrees of yaw).
const RANGE_DOOR_SWING: f32 = 88.0;
const TARGET_KINDS: [&str; 3] = ["cargoship_target", "deck_target", "rifle_target_dummy"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    /// The deck mock-up's timed run (`deck_target`).
    Deck,
    /// The first walk-through's positions (`position_*_targets`).
    Position,
    /// The rifle range's lane (`rifle_target_dummy`).
    Rifle,
    /// The range's fixed bullseyes (`aim_up_target`, `aim_down_target`):
    /// always up, counted as hits.
    Bullseye,
}

/// A pop-up target: its brush model turning about its pivot.
struct Target {
    model: usize,
    group: Group,
    /// `targetname`: `hostile` / `friendly` on the deck, the position's
    /// group name in the walk-through.
    name: String,
    /// The deck's `script_linkName`.
    link: Option<String>,
    pivot: Vec3,
    pivot_angles: [f32; 3],
    /// Pitch (degrees) from up to down: -90, or +90 for a `reverse` pivot.
    down_pitch: f32,
    /// The brush entity's place as the level has it (up).
    origin: Vec3,
    turn: Quat,
    /// Its bounds about its origin (Bevy space).
    bounds: [Vec3; 2],
    /// 0 down .. 1 up, and which way it's going.
    up: f32,
    raise: bool,
    raised_at: f32,
}

impl Target {
    fn hostile(&self) -> bool {
        self.name != "friendly"
    }

    fn pose(&self) -> (Vec3, Quat) {
        let t = self.up * self.up * (3.0 - 2.0 * self.up);
        let [p, y, r] = self.pivot_angles;
        let base = crate::modes::koth::cod_rotation(self.pivot_angles);
        let now = crate::modes::koth::cod_rotation([p + self.down_pitch * (1.0 - t), y, r]);
        let d = now * base.inverse();
        (self.pivot + d * (self.origin - self.pivot), d * self.turn)
    }

    fn standing(&self) -> bool {
        self.raise && self.up > 0.9
    }
}

/// A trigger's box (CoD units).
#[derive(Clone, Copy, Default)]
struct Box3([f32; 3], [f32; 3]);

impl Box3 {
    fn holds(&self, p: [f32; 3], margin: f32) -> bool {
        (0..3).all(|i| p[i] >= self.0[i] - margin && p[i] <= self.1[i] + margin)
    }
}

#[derive(Resource)]
struct Base {
    targets: Vec<Target>,
    /// Brush models placed differently: model → (place, turn), or hidden.
    fixed: HashMap<usize, Option<(Vec3, Quat)>>,
    triggers: HashMap<String, Box3>,
    /// `area_two_*`'s `script_linkTo` (the deck targets each pops).
    links: HashMap<String, Vec<String>>,
    /// Position walk-through triggers: trigger box, its targets' group.
    positions: Vec<(Box3, String)>,
    /// Where the player is, CoD units, and the time.
    me: Option<[f32; 3]>,
    now: f32,
    /// Shots this frame (a pull of the trigger, however many pellets), and
    /// targets hit (index).
    shot: bool,
    hit: Vec<usize>,
    range: Range,
    deck: Deck,
    obstacle: Obstacle,
    /// The last walk-through pop per group.
    walked: HashMap<String, f32>,
    /// Best times (seconds), best first: the deck and the obstacle course.
    records: [Vec<f32>; 2],
    records_read: bool,
    /// A line in the middle of the screen until then.
    notice: Option<(String, f32)>,
}

#[derive(Default)]
struct Range {
    on: bool,
    hits: u32,
    shots: u32,
    next_at: f32,
    last: Option<usize>,
}

#[derive(Default)]
enum Deck {
    #[default]
    Idle,
    /// "Get ready..." and the start then.
    Ready(f32),
    Running {
        start: f32,
        shots: u32,
        /// 0..5 the positions, 5 the finish.
        stage: usize,
        /// The position's targets still to pop (and when the next does).
        queue: Vec<usize>,
        next_at: f32,
        popped: bool,
        needed: u32,
        hit: u32,
        friendlies: Vec<usize>,
    },
}

#[derive(Default)]
enum Obstacle {
    #[default]
    Idle,
    Running {
        start: f32,
        /// The wire and the sprint passed.
        prone: bool,
        sprint: bool,
    },
}

/// How long a run may take before it's called off (seconds).
const DECK_LIMIT: f32 = 60.0;
const OBSTACLE_LIMIT: f32 = 180.0;
/// CoD4's best time on the deck (`killTimer( 15.85 )`).
const IW_BEST: f32 = 15.85;
/// Where the records are kept (stats): five deck times, then five obstacle
/// course times, in milliseconds (0 none).
pub const RECORD_STATS: i32 = 7800;

static ARMED: AtomicBool = AtomicBool::new(false);

/// Is the player armed here (in the range or the hangar's mock-up)?
pub fn armed() -> bool {
    super::active() && ARMED.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------- areas

/// A part of the base: its name over it, from far, and where a travel post
/// takes the player (CoD units, degrees).
pub struct Area {
    pub name: &'static str,
    pub at: [f32; 3],
    pub yaw: f32,
}

pub const AREAS: [Area; 6] = [
    // (Behind the spawn, not on it: its prompt isn't up on arrival.)
    Area { name: "Hangar", at: [17.0, -1081.0, 16.0], yaw: 144.0 },
    Area { name: "Killhouse", at: [622.0, -24.0, 16.0], yaw: 90.0 },
    Area { name: "Rifle Range", at: [3552.0, -905.0, 16.0], yaw: 90.0 },
    Area { name: "Frag Pit", at: [2150.0, -2100.0, 16.0], yaw: 270.0 },
    Area { name: "Obstacle Course", at: [300.0, -2230.0, 16.0], yaw: 270.0 },
    Area { name: "Helipad", at: [-2750.0, -2700.0, 16.0], yaw: 220.0 },
];

/// How near a post the player must be (across the floor).
const POST_REACH: f32 = 40.0;

/// The armed zones (CoD units): the rifle range's building, the hangar's
/// mock-up (north of the briefing corner).
const ARMED_ZONES: [Box3; 2] = [Box3([2990.0, -1175.0, -50.0], [3720.0, 340.0, 400.0]), Box3([-480.0, -770.0, -50.0], [720.0, 640.0, 700.0])];

static POST: AtomicUsize = AtomicUsize::new(usize::MAX);
static DEST: AtomicUsize = AtomicUsize::new(1);
static GO: AtomicBool = AtomicBool::new(false);

/// The travel post within reach, and where it goes.
pub fn post() -> Option<&'static Area> {
    (POST.load(Ordering::Relaxed) != usize::MAX).then(|| &AREAS[DEST.load(Ordering::Relaxed) % AREAS.len()])
}

/// Use at a post ([`super::stations`] passes it on).
pub(super) fn use_post() -> bool {
    if POST.load(Ordering::Relaxed) == usize::MAX {
        return false;
    }
    GO.store(true, Ordering::Relaxed);
    true
}

/// The next destination (scroll, or a pad's d-pad, at a post).
pub(super) fn next_destination(step: i32) {
    let here = POST.load(Ordering::Relaxed);
    if here == usize::MAX {
        return;
    }
    let n = AREAS.len();
    let mut d = DEST.load(Ordering::Relaxed);
    loop {
        d = (d as i32 + step).rem_euclid(n as i32) as usize;
        if d != here {
            break;
        }
    }
    DEST.store(d, Ordering::Relaxed);
}

// ---------------------------------------------------------------- what the HUD shows

/// What [`crate::ui`]'s HUD shows of the base this frame.
#[derive(Clone, Default)]
pub struct Overlay {
    /// Top of the screen: a run's clock, or the range's count.
    pub timer: Option<String>,
    pub status: Option<String>,
    /// The middle: how a run went.
    pub notice: Option<String>,
    /// Boards in the world: where (CoD units), title, lines.
    pub boards: Vec<([f32; 3], String, Vec<String>)>,
}

static OVERLAY: Mutex<Option<Overlay>> = Mutex::new(None);

/// The base's HUD this frame.
pub fn overlay() -> Overlay {
    OVERLAY.lock().ok().and_then(|o| o.clone()).unwrap_or_default()
}

// ---------------------------------------------------------------- setup

fn setup(mut commands: Commands, hq: Option<Res<super::Headquarters>>, content: Res<crate::content::Content>) {
    if hq.is_none() {
        return;
    }
    let ents = content.map().map_ents().map(|e| iw3::ents::parse(&e.entity_string)).unwrap_or_default();
    let models: Vec<[[f32; 3]; 2]> = content.map().gfx_world().map(|w| w.models.iter().map(|m| m.bounds).collect()).unwrap_or_default();
    let model = |e: &iw3::ents::Entity| e.get("model")?.strip_prefix('*')?.parse::<usize>().ok();
    let by_name: HashMap<&str, &iw3::ents::Entity> = ents.iter().filter_map(|e| Some((e.get("targetname")?, e))).collect();
    let mut fixed = HashMap::new();
    let mut targets = Vec::new();
    let mut triggers = HashMap::new();
    let mut links = HashMap::new();
    let mut positions = Vec::new();
    // (Debug: `COD4RW_HQTARGETS=1` stands every target up, to look at.)
    let all_up = std::env::var_os("COD4RW_HQTARGETS").is_some();
    for e in &ents {
        let Some(n) = model(e) else { continue };
        let origin = e.origin().unwrap_or([0.0; 3]);
        let local = models.get(n).copied().unwrap_or([[0.0; 3]; 2]);
        let place = |o: [f32; 3]| Box3(std::array::from_fn(|i| o[i] + local[0][i]), std::array::from_fn(|i| o[i] + local[1][i]));
        match (e.classname(), e.get("targetname")) {
            ("script_brushmodel", Some(DOOR)) => {
                let o = [origin[0] + DOOR_SLIDE, origin[1], origin[2]];
                fixed.insert(n, Some((pos(o), crate::modes::koth::cod_rotation(e.angles()))));
            }
            ("script_brushmodel", Some(CLIP)) => {
                fixed.insert(n, None);
            }
            ("script_brushmodel", Some(RANGE_DOOR)) => {
                let [p, y, r] = e.angles();
                fixed.insert(n, Some((pos(origin), crate::modes::koth::cod_rotation([p, y + RANGE_DOOR_SWING, r]))));
            }
            ("trigger_multiple", Some(name)) => {
                // (`flag_set` triggers by the flag they set.)
                let name = if name == "flag_set" { e.get("script_flag").unwrap_or(name) } else { name };
                triggers.insert(name.to_owned(), place(origin));
                if let Some(l) = e.get("script_linkTo") {
                    links.insert(name.to_owned(), l.split_whitespace().map(str::to_owned).collect());
                }
                if let Some(t) = e.get("target").filter(|t| t.starts_with("position_")) {
                    positions.push((place(origin), t.to_owned()));
                }
            }
            _ => {}
        }
        if matches!(e.get("targetname"), Some("aim_up_target" | "aim_down_target")) {
            let corners = [pos(local[0]), pos(local[1])];
            targets.push(Target {
                model: n,
                group: Group::Bullseye,
                name: String::new(),
                link: None,
                pivot: pos(origin),
                pivot_angles: [0.0; 3],
                down_pitch: 0.0,
                origin: pos(origin),
                turn: Quat::IDENTITY,
                bounds: [corners[0].min(corners[1]), corners[0].max(corners[1])],
                up: 1.0,
                raise: true,
                raised_at: f32::INFINITY,
            });
            continue;
        }
        let group = match e.get("script_noteworthy") {
            Some("deck_target") => Group::Deck,
            Some("cargoship_target") => Group::Position,
            Some("rifle_target_dummy") => Group::Rifle,
            _ => continue,
        };
        let Some(pivot) = e.get("target").and_then(|t| by_name.get(t)) else { continue };
        if let Some(aim) = pivot.get("target").and_then(|t| by_name.get(t)).and_then(|a| model(a)) {
            fixed.insert(aim, None);
        }
        let reverse = pivot.get("script_noteworthy") == Some("reverse");
        let corners = [pos(local[0]), pos(local[1])];
        targets.push(Target {
            model: n,
            group,
            name: e.get("targetname").unwrap_or_default().to_owned(),
            link: e.get("script_linkName").map(str::to_owned),
            pivot: pos(pivot.origin().unwrap_or(origin)),
            pivot_angles: pivot.angles(),
            down_pitch: if reverse { 90.0 } else { -90.0 },
            origin: pos(origin),
            turn: crate::modes::koth::cod_rotation(e.angles()),
            bounds: [corners[0].min(corners[1]), corners[0].max(corners[1])],
            up: 0.0,
            raise: all_up,
            raised_at: f32::INFINITY,
        });
    }
    info!("headquarters: {} pop-up targets, {} doors and clips moved", targets.len(), fixed.len());
    commands.insert_resource(Base {
        targets,
        fixed,
        triggers,
        links,
        positions,
        me: None,
        now: 0.0,
        shot: false,
        hit: Vec::new(),
        range: Range::default(),
        deck: Deck::Idle,
        obstacle: Obstacle::Idle,
        walked: HashMap::new(),
        records: [Vec::new(), Vec::new()],
        records_read: false,
        notice: None,
    });
}

fn leave(mut commands: Commands) {
    ARMED.store(false, Ordering::Relaxed);
    POST.store(usize::MAX, Ordering::Relaxed);
    if let Ok(mut o) = OVERLAY.lock() {
        *o = None;
    }
    commands.remove_resource::<Base>();
}

// ---------------------------------------------------------------- each frame

/// Where the player is, whether armed, and the records once the stats are
/// up.
fn zones(time: Res<Time>, base: Option<ResMut<Base>>, player: Query<&Transform, With<crate::player::LocalPlayer>>, fe: Option<Res<crate::ui::Frontend>>) {
    let Some(mut base) = base else { return };
    base.now = time.elapsed_secs();
    base.me = player.single().ok().map(|tf| crate::units::to_cod(tf.translation));
    let armed = base.me.is_some_and(|me| ARMED_ZONES.iter().any(|z| z.holds(me, 0.0)));
    ARMED.store(armed, Ordering::Relaxed);
    if !base.records_read
        && let Some(fe) = fe
    {
        base.records_read = true;
        for (k, list) in base.records.iter_mut().enumerate() {
            *list = (0..5).map(|i| fe.hq_record(RECORD_STATS + 10 * k as i32 + i)).filter(|&ms| ms > 0).map(|ms| ms as f32 / 1000.0).collect();
        }
    }
}

/// Debug aid: `COD4RW_HQRUN=deck` runs the killhouse by itself (into
/// each position, aiming at each hostile up and firing), `=range` shoots
/// at the range. [`super::strip`] pulls the trigger ([`debug_fire`]).
fn debug_driver(base: Option<Res<Base>>, mut player: Query<(&mut Transform, &mut crate::movement::Mover, &mut crate::movement::ViewAngles), With<crate::player::LocalPlayer>>) {
    static WHAT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let Some(what) = WHAT.get_or_init(|| std::env::var("COD4RW_HQRUN").ok()) else { return };
    let (Some(base), Ok((mut tf, mut mover, mut view))) = (base, player.single_mut()) else { return };
    if base.now < 8.0 {
        return;
    }
    let mut go = |b: &Box3| {
        let at = pos([(b.0[0] + b.1[0]) * 0.5, (b.0[1] + b.1[1]) * 0.5, b.0[2] + 2.0]);
        if tf.translation.distance(at) > u(30.0) {
            tf.translation = at;
            mover.velocity = Vec3::ZERO;
        }
    };
    let deck = what == "deck";
    if deck {
        let stage = match &base.deck {
            Deck::Idle | Deck::Ready(_) => Some(0),
            Deck::Running { stage, popped: false, .. } => Some(*stage),
            Deck::Running { .. } => None,
        };
        let names = ["area_two_one", "area_two_two", "area_two_three", "area_two_four", "area_two_five", "area_two_finish"];
        if let Some(b) = stage.and_then(|k| base.triggers.get(names[k])) {
            go(b);
        }
    } else if what == "obstacle" {
        let steps = ["obstacle_course_start", "start_course", "obstacleTraining_mantle", "obstacleTraining_prone", "obstacleTraining_sprint", OBSTACLE_END, "obstacle_course_start"];
        let k = (((base.now - 8.0) / 1.5) as usize).min(steps.len() - 1);
        if let Some(b) = base.triggers.get(steps[k]) {
            go(b);
        }
        return;
    } else if let Some(b) = base.triggers.get("rifleTraining_stall") {
        go(b);
    }
    let eye = tf.translation + Vec3::Y * mover.eye_height;
    let want = |t: &Target| if deck { t.group == Group::Deck && t.hostile() } else { matches!(t.group, Group::Rifle | Group::Bullseye) };
    let aim = base
        .targets
        .iter()
        .filter(|t| t.standing() && want(t))
        .map(|t| {
            let (at, turn) = t.pose();
            at + turn * ((t.bounds[0] + t.bounds[1]) * 0.5 + Vec3::Y * u(8.0))
        })
        .min_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye)));
    DEBUG_FIRE.store(aim.is_some(), Ordering::Relaxed);
    if (base.now * 2.0).floor() != ((base.now - 0.02) * 2.0).floor() {
        let standing = base.targets.iter().filter(|t| t.standing()).count();
        let stage = match &base.deck {
            Deck::Running { stage, popped, queue, needed, hit, .. } => format!("stage {stage} popped {popped} queue {} hit {hit}/{needed}", queue.len()),
            _ => "idle".into(),
        };
        info!("hqrun: armed {} at {:?} standing {standing} aim {aim:?} {stage} range {} hits {} shots", ARMED.load(Ordering::Relaxed), crate::units::to_cod(tf.translation), base.range.hits, base.range.shots);
    }
    if let Some(at) = aim {
        let d = (at - eye).normalize_or(Vec3::NEG_Z);
        view.yaw = f32::atan2(-d.x, -d.z);
        view.pitch = d.y.clamp(-1.0, 1.0).asin();
    }
}

static DEBUG_FIRE: AtomicBool = AtomicBool::new(false);

/// [`debug_driver`] has a target in its sights.
pub(super) fn debug_fire() -> bool {
    DEBUG_FIRE.load(Ordering::Relaxed)
}

/// Trigger pulls this frame (one per frame, however many pellets).
fn shots(mut base: Option<ResMut<Base>>, mut fired: MessageReader<crate::weapons::ShotFired>, me: Query<Entity, With<crate::player::LocalPlayer>>) {
    let me = me.single().ok();
    let shot = fired.read().any(|s| Some(s.shooter) == me && s.weapon.is_some());
    if let Some(base) = base.as_mut() {
        base.shot = shot;
    }
}

/// Bullets that stopped on a standing target.
fn hits(mut base: Option<ResMut<Base>>, mut paths: MessageReader<crate::weapons::BulletPath>, me: Query<Entity, With<crate::player::LocalPlayer>>, mut sfx: ResMut<crate::audio::Sfx>) {
    let Some(base) = base.as_mut() else {
        for _ in paths.read() {}
        return;
    };
    let Base { targets, hit, .. } = &mut **base;
    hit.clear();
    let me = me.single().ok();
    let margin = Vec3::splat(u(3.0));
    for path in paths.read().filter(|p| Some(p.shooter) == me) {
        for (i, t) in targets.iter().enumerate() {
            if !t.standing() {
                continue;
            }
            // (Their brushes aren't solid to bullets: the shot's path
            // through the target's box, not where it stopped.)
            let (at, turn) = t.pose();
            let from = turn.inverse() * (path.from - at);
            let to = turn.inverse() * (path.end - at);
            if crosses(from, to, t.bounds[0] - margin, t.bounds[1] + margin) && !hit.contains(&i) {
                hit.push(i);
            }
        }
    }
    for &i in hit.iter() {
        let t = &mut targets[i];
        debug!("headquarters: hit target *{} ({:?} {})", t.model, t.group, t.name);
        t.raise = t.group == Group::Bullseye;
        if t.hostile() {
            sfx.play("killhouse_buzzer", Some(t.origin));
        }
        sfx.play("killhouse_target_up", Some(t.origin));
    }
}

/// Does the segment cross the box (a slab test)?
fn crosses(from: Vec3, to: Vec3, lo: Vec3, hi: Vec3) -> bool {
    let d = to - from;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for k in 0..3 {
        if d[k].abs() < 1e-6 {
            if from[k] < lo[k] || from[k] > hi[k] {
                return false;
            }
            continue;
        }
        let (a, b) = ((lo[k] - from[k]) / d[k], (hi[k] - from[k]) / d[k]);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
        if t0 > t1 {
            return false;
        }
    }
    true
}

fn raise(t: &mut Target, now: f32, sfx: &mut crate::audio::Sfx) {
    if !t.raise {
        t.raise = true;
        t.raised_at = now;
        sfx.play("killhouse_target_up_metal", Some(t.origin));
    }
}

/// At the stall, the lane's targets up one at a time: hit, down, the next.
fn rifle_range(mut base: Option<ResMut<Base>>, mut sfx: ResMut<crate::audio::Sfx>) {
    let Some(base) = base.as_mut() else { return };
    let Base { targets, triggers, range, me, now, shot, hit, .. } = &mut **base;
    let at_stall = me.is_some_and(|me| triggers.get("rifleTraining_stall").is_some_and(|s| s.holds(me, 140.0)));
    if at_stall && !range.on {
        *range = Range { on: true, next_at: *now + 1.0, ..Default::default() };
    }
    let lane: Vec<usize> = targets.iter().enumerate().filter(|(_, t)| t.group == Group::Rifle).map(|(i, _)| i).collect();
    if !at_stall {
        if range.on {
            for &i in &lane {
                targets[i].raise = false;
            }
            range.on = false;
        }
        return;
    }
    if *shot {
        range.shots += 1;
    }
    // (A hit a shot at most: one bullet can pass through two.)
    if hit.iter().any(|&i| matches!(targets[i].group, Group::Rifle | Group::Bullseye)) {
        range.hits += 1;
    }
    if hit.iter().any(|&i| targets[i].group == Group::Rifle) {
        range.next_at = *now + rand::random_range(0.4..1.0);
    }
    // Missed for long enough: down, and another.
    for &i in &lane {
        if targets[i].raise && *now - targets[i].raised_at > 4.0 {
            targets[i].raise = false;
            range.next_at = *now + 0.5;
        }
    }
    if lane.is_empty() || lane.iter().any(|&i| targets[i].raise || targets[i].up > 0.05) || *now < range.next_at {
        return;
    }
    let choices: Vec<usize> = lane.iter().copied().filter(|&i| Some(i) != range.last || lane.len() == 1).collect();
    let pick = choices[rand::random_range(0..choices.len())];
    range.last = Some(pick);
    raise(&mut targets[pick], *now, &mut sfx);
}

fn record(list: &mut Vec<f32>, time: f32) -> usize {
    let at = list.iter().position(|&t| time < t).unwrap_or(list.len());
    list.insert(at, time);
    list.truncate(5);
    at
}

fn save_records(fe: &mut Option<ResMut<crate::ui::Frontend>>, k: usize, list: &[f32]) {
    let Some(fe) = fe.as_mut() else { return };
    for i in 0..5 {
        let ms = list.get(i).map_or(0, |t| (t * 1000.0).round() as i32);
        fe.set_hq_record(RECORD_STATS + 10 * k as i32 + i as i32, ms);
    }
}

fn clock(secs: f32) -> String {
    format!("{}:{:04.1}", (secs / 60.0) as u32, secs % 60.0)
}

/// The deck mock-up's timed run (`deck_training`).
fn deck_run(mut base: Option<ResMut<Base>>, mut sfx: ResMut<crate::audio::Sfx>, mut fe: Option<ResMut<crate::ui::Frontend>>) {
    let Some(base) = base.as_mut() else { return };
    let Base { targets, triggers, links, deck, me, now, shot, hit, records, notice, .. } = &mut **base;
    let now = *now;
    let inside = |name: &str| me.is_some_and(|me| triggers.get(name).is_some_and(|t| t.holds(me, 4.0)));
    const AREAS: [&str; 6] = ["area_two_one", "area_two_two", "area_two_three", "area_two_four", "area_two_five", "area_two_finish"];
    let all_down = |targets: &mut Vec<Target>| {
        for t in targets.iter_mut().filter(|t| t.group == Group::Deck) {
            t.raise = false;
        }
    };
    // Left the hangar's mock-up: called off.
    if !matches!(deck, Deck::Idle) && !ARMED.load(Ordering::Relaxed) {
        *deck = Deck::Idle;
        all_down(targets);
        *notice = Some(("Run called off".into(), now + 3.0));
        return;
    }
    match deck {
        Deck::Idle => {
            if inside(AREAS[0]) && ARMED.load(Ordering::Relaxed) && notice.as_ref().is_none_or(|n| now > n.1) {
                *deck = Deck::Ready(now + 2.0);
                *notice = Some(("Get ready...".into(), now + 2.0));
            }
        }
        Deck::Ready(go) => {
            if now >= *go {
                *notice = Some(("Go go go!".into(), now + 1.2));
                *deck = Deck::Running { start: now, shots: 0, stage: 0, queue: Vec::new(), next_at: 0.0, popped: false, needed: 0, hit: 0, friendlies: Vec::new() };
            }
        }
        Deck::Running { start, shots, stage, queue, next_at, popped, needed, hit: got, friendlies } => {
            if *shot {
                *shots += 1;
            }
            let elapsed = now - *start;
            for &i in hit.iter() {
                if targets[i].group != Group::Deck {
                    continue;
                }
                if !targets[i].hostile() {
                    *notice = Some(("You shot a friendly. Run failed.".into(), now + 4.0));
                    *deck = Deck::Idle;
                    all_down(targets);
                    return;
                }
                *got += 1;
            }
            if elapsed > DECK_LIMIT {
                *notice = Some(("Too slow. Run failed.".into(), now + 4.0));
                *deck = Deck::Idle;
                all_down(targets);
                return;
            }
            if *stage == 5 {
                if inside(AREAS[5]) {
                    // CoD4's accuracy bonus: 20 shots allowed, 0.2 s off
                    // for each of 15 not over them.
                    let excess = shots.saturating_sub(20) as f32;
                    let bonus = ((15.0 - excess) * 0.2).max(0.0);
                    let time = (elapsed - bonus).max(0.0);
                    let place = record(&mut records[0], time);
                    save_records(&mut fe, 0, &records[0]);
                    let best = if place == 0 { "  New personal best!" } else { "" };
                    *notice = Some((format!("Time {}  (accuracy bonus -{bonus:.1} s){best}", clock(time)), now + 6.0));
                    *deck = Deck::Idle;
                }
                return;
            }
            if !*popped {
                if inside(AREAS[*stage]) {
                    // Its targets in a shuffled order: up one by one until
                    // three hostiles are (friendlies among them).
                    let mut list: Vec<usize> = links
                        .get(AREAS[*stage])
                        .map(|l| l.iter().filter_map(|name| targets.iter().position(|t| t.group == Group::Deck && t.link.as_deref() == Some(name.as_str()))).collect())
                        .unwrap_or_default();
                    shuffle(&mut list);
                    let mut hostiles = 0;
                    queue.clear();
                    for i in list {
                        if hostiles == 3 {
                            break;
                        }
                        if targets[i].hostile() {
                            hostiles += 1;
                        }
                        queue.push(i);
                    }
                    *needed = hostiles;
                    *got = 0;
                    friendlies.clear();
                    *popped = true;
                    *next_at = now;
                    if *stage > 0 {
                        *notice = Some((format!("Position {}!", *stage + 1), now + 1.0));
                    }
                }
                return;
            }
            if !queue.is_empty() && now >= *next_at {
                let i = queue.remove(0);
                if !targets[i].hostile() {
                    friendlies.push(i);
                }
                raise(&mut targets[i], now, &mut sfx);
                *next_at = now + rand::random_range(0.25..0.4);
            }
            if queue.is_empty() && *got >= *needed {
                for &i in friendlies.iter() {
                    targets[i].raise = false;
                }
                *stage += 1;
                *popped = false;
                if *stage == 5 {
                    *notice = Some(("Final position, go!".into(), now + 1.2));
                }
            }
        }
    }
}

fn shuffle(list: &mut [usize]) {
    for i in (1..list.len()).rev() {
        list.swap(i, rand::random_range(0..=i));
    }
}

/// The first walk-through's positions: walking into one (armed, no deck
/// run) puts its targets up to shoot, untimed.
fn walkthrough(mut base: Option<ResMut<Base>>, mut sfx: ResMut<crate::audio::Sfx>) {
    let Some(base) = base.as_mut() else { return };
    if !matches!(base.deck, Deck::Idle) || !ARMED.load(Ordering::Relaxed) {
        return;
    }
    let Some(me) = base.me else { return };
    let now = base.now;
    let Base { targets, positions, walked, .. } = &mut **base;
    for (zone, group) in positions.iter() {
        if !zone.holds(me, 4.0) {
            continue;
        }
        let mine = || targets.iter().filter(|t| t.group == Group::Position && &t.name == group);
        if mine().any(|t| t.raise || t.up > 0.05) || walked.get(group).is_some_and(|&t| now - t < 5.0) {
            continue;
        }
        walked.insert(group.clone(), now);
        for t in targets.iter_mut().filter(|t| t.group == Group::Position && &t.name == group) {
            raise(t, now, &mut sfx);
        }
    }
    // Not shot in a while: down again.
    for t in targets.iter_mut().filter(|t| t.group == Group::Position) {
        if t.raise && now - t.raised_at > 20.0 {
            t.raise = false;
        }
    }
}

/// Mac's obstacle course, timed: from the start line, past the wire and the
/// sprint, to the end.
fn obstacle_run(mut base: Option<ResMut<Base>>, mut fe: Option<ResMut<crate::ui::Frontend>>) {
    let Some(base) = base.as_mut() else { return };
    let Base { triggers, obstacle, me, now, records, notice, .. } = &mut **base;
    let inside = |name: &str| me.is_some_and(|me| triggers.get(name).is_some_and(|t| t.holds(me, 4.0)));
    match obstacle {
        Obstacle::Idle => {
            if inside("start_course") {
                *obstacle = Obstacle::Running { start: *now, prone: false, sprint: false };
            }
        }
        Obstacle::Running { start, prone, sprint } => {
            // Back behind the line: not a run.
            if inside("obstacle_course_start") {
                *obstacle = Obstacle::Idle;
                return;
            }
            // On the line: the clock waits.
            if inside("start_course") {
                *start = *now;
                return;
            }
            *prone |= inside("obstacleTraining_prone");
            *sprint |= inside("obstacleTraining_sprint");
            let elapsed = *now - *start;
            if elapsed > OBSTACLE_LIMIT {
                *obstacle = Obstacle::Idle;
                return;
            }
            if *prone && *sprint && inside(OBSTACLE_END) {
                let place = record(&mut records[1], elapsed);
                save_records(&mut fe, 1, &records[1]);
                let best = if place == 0 { "  New personal best!" } else { "" };
                *notice = Some((format!("Obstacle course {}{best}", clock(elapsed)), *now + 6.0));
                *obstacle = Obstacle::Idle;
            }
        }
    }
}

/// The obstacle course's end (the flag its trigger sets).
const OBSTACLE_END: &str = "obstacle_course_end";

/// Targets turning up and down.
fn animate(mut base: Option<ResMut<Base>>, time: Res<Time>) {
    let Some(base) = base.as_mut() else { return };
    let dt = time.delta_secs();
    for t in &mut base.targets {
        // Up in a quarter second; down in a quarter (the range's, slower).
        let rate = if t.raise { 4.0 } else if t.group == Group::Rifle { 1.6 } else { 4.0 };
        t.up = if t.raise { (t.up + rate * dt).min(1.0) } else { (t.up - rate * dt).max(0.0) };
    }
}

/// The moved brush models drawn and solid where they are now.
fn pose_parts(mut commands: Commands, base: Option<Res<Base>>, mut parts: Query<(Entity, &crate::world::ScriptBrushPart, &mut Transform, Option<&mut Visibility>, Has<avian3d::prelude::Collider>, Has<avian3d::prelude::ColliderDisabled>)>) {
    let Some(base) = base else { return };
    let poses: HashMap<usize, (Vec3, Quat)> = base.targets.iter().map(|t| (t.model, t.pose())).collect();
    for (entity, part, mut tf, vis, collider, disabled) in &mut parts {
        let place = match base.fixed.get(&part.0) {
            Some(Some(p)) => Some(*p),
            Some(None) => {
                if let Some(mut v) = vis {
                    v.set_if_neq(Visibility::Hidden);
                }
                if collider && !disabled {
                    commands.entity(entity).insert(avian3d::prelude::ColliderDisabled);
                }
                continue;
            }
            None => poses.get(&part.0).copied(),
        };
        let Some((at, turn)) = place else { continue };
        if tf.translation != at || tf.rotation != turn {
            tf.translation = at;
            tf.rotation = turn;
        }
    }
}

/// The travel posts: which is in reach, and Use there goes.
fn travel(
    base: Option<Res<Base>>,
    mut player: Query<(&mut Transform, &mut crate::movement::Mover, &mut crate::movement::ViewAngles), With<crate::player::LocalPlayer>>,
    spatial: avian3d::prelude::SpatialQuery,
) {
    let Some(me) = base.and_then(|b| b.me) else { return };
    let near = AREAS.iter().position(|a| ((me[0] - a.at[0]).powi(2) + (me[1] - a.at[1]).powi(2)).sqrt() <= POST_REACH && (me[2] - a.at[2]).abs() < 120.0);
    let was = POST.swap(near.unwrap_or(usize::MAX), Ordering::Relaxed);
    if let Some(here) = near
        && was != here
    {
        // At a new post: the next area on.
        DEST.store((here + 1) % AREAS.len(), Ordering::Relaxed);
    }
    if !GO.swap(false, Ordering::Relaxed) || near.is_none() {
        return;
    }
    let dest = &AREAS[DEST.load(Ordering::Relaxed) % AREAS.len()];
    let Ok((mut tf, mut mover, mut view)) = player.single_mut() else { return };
    // Onto the floor there (a little to the side of the post, so its
    // prompt isn't up on arrival).
    let yaw = dest.yaw.to_radians();
    let ahead = [dest.at[0] + yaw.cos() * (POST_REACH + 20.0), dest.at[1] + yaw.sin() * (POST_REACH + 20.0), dest.at[2]];
    let from = pos(ahead) + Vec3::Y * u(100.0);
    let floor = spatial.cast_ray(from, Dir3::NEG_Y, u(400.0), true, &crate::collision::movement_filter()).map_or(pos(ahead), |h| from - Vec3::Y * h.distance);
    tf.translation = floor + Vec3::Y * u(1.0);
    mover.velocity = Vec3::ZERO;
    view.yaw = crate::units::yaw_from_cod_degrees(dest.yaw);
    view.pitch = 0.0;
    info!("headquarters: travelled to the {}", dest.name);
}

/// A class picked at a weapon rack is the player's now (in a match it's
/// the next spawn's).
fn class_change(mut commands: Commands, changed: Query<Entity, (With<crate::player::LocalPlayer>, Changed<crate::loadout::PawnClass>, With<crate::loadout::Loadout>)>, mut armed: Query<&mut crate::weapons::WeaponState, With<crate::player::LocalPlayer>>) {
    for e in &changed {
        commands.entity(e).remove::<crate::loadout::Loadout>();
    }
    // Armed here, the reserve never runs out.
    if ARMED.load(Ordering::Relaxed) {
        for mut w in &mut armed {
            if w.reserve < w.def.max_ammo {
                w.reserve = w.def.max_ammo;
            }
        }
    }
}

/// What the HUD shows this frame.
fn publish(base: Option<ResMut<Base>>) {
    let Some(mut base) = base else { return };
    let now = base.now;
    let mut o = Overlay::default();
    match &base.deck {
        Deck::Running { start, stage, .. } => {
            o.timer = Some(clock(now - start));
            o.status = Some(if *stage < 5 { format!("Killhouse: position {} of 5", stage + 1) } else { "Killhouse: to the finish".into() });
        }
        Deck::Ready(_) => o.timer = Some(clock(0.0)),
        Deck::Idle => {}
    }
    if let Obstacle::Running { start, prone, sprint } = &base.obstacle {
        o.timer = Some(clock(now - start));
        o.status = Some(match (prone, sprint) {
            (false, _) => "Obstacle course: under the wire".into(),
            (true, false) => "Obstacle course: sprint".into(),
            _ => "Obstacle course: to the end".into(),
        });
    }
    if base.range.on {
        let r = &base.range;
        let accuracy = if r.shots > 0 { format!("{:.0}%", 100.0 * r.hits as f32 / r.shots as f32) } else { "-".into() };
        o.status = Some(format!("Hits {}   Shots {}   Accuracy {accuracy}", r.hits, r.shots));
    }
    if let Some((text, until)) = &base.notice {
        if now < *until {
            o.notice = Some(text.clone());
        }
    }
    static LAST: Mutex<String> = Mutex::new(String::new());
    if let (Some(n), Ok(mut last)) = (&o.notice, LAST.lock())
        && *last != *n
    {
        info!("headquarters: \"{n}\" ({})", o.status.clone().unwrap_or_default());
        *last = n.clone();
    }
    if base.notice.as_ref().is_some_and(|n| now >= n.1) {
        base.notice = None;
    }
    let lines = |list: &[f32]| -> Vec<String> {
        let mut out: Vec<String> = (0..5).map(|i| format!("{}.  {}", i + 1, list.get(i).map_or("--".into(), |&t| clock(t)))).collect();
        out.push(String::new());
        out
    };
    let mut deck = lines(&base.records[0]);
    deck.push(format!("Squadron record  {}", clock(IW_BEST)));
    o.boards.push(([700.0, -150.0, 100.0], "KILLHOUSE BEST TIMES".into(), deck));
    let mut course = lines(&base.records[1]);
    course.pop();
    o.boards.push(([300.0, -2170.0, 90.0], "OBSTACLE COURSE BEST TIMES".into(), course));
    if let Ok(mut slot) = OVERLAY.lock() {
        *slot = Some(o);
    }
}
