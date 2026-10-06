//! The helicopter (`helicopter_mp`, `_helicopter.gsc`): a Cobra for the
//! allies, a Hind for the axis. It comes in along one of the map's paths
//! (`heli_start` script_origins chained by `target`, at the node's
//! `script_airspeed`/`script_accel` in miles an hour, else 30-50 at 15-30,
//! holding `script_delay` seconds where a node has one), hovers two seconds
//! at its destination, circles the map once on the loop path and leaves
//! for a `heli_leave` point. One at a time.
//!
//! Every half second it picks targets (`heli_targeting`): living enemies
//! within 3500 units, five seconds past spawning, at least half of whom the
//! turret can see; the most threatening (nearer, its last attacker, more
//! score, less already shot at) gets the 20 mm turret (`cobra_20mm_mp`):
//! once it's on target, 0.75 s to spin up, a 40-round burst a round every
//! 50 ms (on the last place it saw them if they get out of sight), 1.5 s to
//! reload. The second most gets a rocket (`cobra_FFAR_mp`/`hind_FFAR_mp`)
//! every five seconds while it's ahead and no friendly is near, from three
//! that come back one every ten.
//!
//! It takes 1100 damage; bullets do only 0.3 of theirs until 500 has been
//! done (`heli_armor`). Past 500 it smokes and breaks for the loop path;
//! past 880 the smoke goes black; past 1100 its tail blows, it spins down
//! the map's crash path and explodes. The caller gets its kills, and the
//! owner and the caller's team can't hurt it.

use super::spawn_static_model;
use crate::collision::{self, Layer};
use crate::combat::{Damage, Dead, HitLocation, Hitbox, Pawn, Team, hostile};
use crate::content::Content;
use crate::explosives::{Behaviour, Exploded, ExplosiveDef, ExplosiveDefs, Launch};
use crate::fx::{Anchor, BulletHit, BulletImpact, Effects, Frame, FxLayer};
use crate::movement::Mover;
use crate::units::{self, INCH, u};
use crate::weapons::HitConfirmed;
use avian3d::prelude::*;
use bevy::prelude::*;
use iw3::zone::AssetType;
use std::collections::HashMap;

/// The turret's weapon, which its kills are credited to (`death_helicopter`).
pub const WEAPON: &str = "cobra_20mm_mp";
/// The rockets, by team.
pub const ROCKETS: [&str; 2] = ["cobra_ffar_mp", "hind_ffar_mp"];

/// `heli_update_global_dvars`' defaults.
const MAX_HEALTH: f32 = 1100.0;
const ARMOR: f32 = 500.0;
const ARMOR_BULLET: f32 = 0.3;
const LOW_HEALTH: f32 = MAX_HEALTH * 0.8;
const DEST_WAIT: f32 = 2.0;
const LOOP_MAX: u32 = 1;
const VISUAL_RANGE: f32 = 3500.0;
const TARGETING_DELAY: f32 = 0.5;
const SPAWN_PROTECTION: f32 = 5.0;
const RECOGNITION: f32 = 0.5;
const CLIP: u32 = 40;
const RELOAD: f32 = 1.5;
const SPINUP: f32 = 0.75;
const MISSILE_MAX: u32 = 3;
const MISSILE_ROF: f32 = 5.0;
const MISSILE_REGEN: f32 = 10.0;
/// `10 / heli_rage_missile`: how long the turret may take getting on
/// target before a rocket goes instead.
const MISSILE_SUPPORT: f32 = 10.0 / 5.0;
const MISSILE_CONE: f32 = 0.3;
const FRIENDLY_CARE: f32 = 256.0;
/// `cobra_20mm_mp`: a round every 50 ms doing its `minDamage` (its damage
/// ranges are 0), `impactType` 3.
const FIRE_TIME: f32 = 0.05;
const DAMAGE: f32 = 30.0;
const IMPACT_TYPE: i32 = 3;
/// The turret's `turretRotRate` (degrees a second), and a little spread
/// (ours) so a burst walks.
const TURRET_RATE: f32 = 80.0;
const SPREAD: f32 = 1.0;
/// Vehicle speeds are in miles an hour: inches a second each.
const MPH: f32 = 17.6;
/// `setneargoalnotifydist( 256 )`.
const NEAR_GOAL: f32 = 256.0;
/// `setyawspeed( 75, ... )`, and the spin of a crash.
const YAW_SPEED: f32 = 75.0;
const SPIN_SPEED: f32 = 180.0;
/// How much harder than its `accel` it can steer (ours: the vehicle
/// physics' turning).
const STEER: f32 = 3.0;

/// The map's helicopter paths (`heli_path_graph`).
#[derive(Resource, Default, Debug)]
pub struct HeliPaths {
    nodes: Vec<Node>,
    /// The paths' first nodes in to the first destination (`heli_paths[0]`).
    starts: Vec<usize>,
    loops: Vec<usize>,
    leaves: Vec<Vec3>,
    crashes: Vec<usize>,
}

#[derive(Clone, Debug)]
struct Node {
    /// Bevy space, and the node's yaw (CoD, radians).
    at: Vec3,
    yaw: f32,
    next: Option<usize>,
    /// `script_airspeed`, `script_accel` (mph, mph a second).
    speed: Option<(f32, f32)>,
    delay: Option<f32>,
}

impl HeliPaths {
    /// The map has a way in (CoD4 gives no helicopter otherwise).
    pub fn available(&self) -> bool {
        !self.starts.is_empty()
    }

    fn from_ents(ents: &[iw3::ents::Entity]) -> HeliPaths {
        let mut paths = HeliPaths::default();
        let mut by_name: HashMap<&str, usize> = HashMap::new();
        let mut targets = Vec::new();
        for e in ents {
            let (Some(name), Some(origin)) = (e.get("targetname"), e.origin()) else { continue };
            by_name.entry(name).or_insert(paths.nodes.len());
            let float = |k: &str| e.get(k).and_then(|v| v.parse::<f32>().ok());
            paths.nodes.push(Node {
                at: units::pos(origin),
                yaw: e.angles()[1].to_radians(),
                next: None,
                speed: float("script_airspeed").zip(float("script_accel")),
                delay: float("script_delay"),
            });
            targets.push(e.get("target"));
        }
        for (node, target) in paths.nodes.iter_mut().zip(&targets) {
            node.next = target.and_then(|t| by_name.get(t).copied());
        }
        let named = |name: &str| -> Vec<usize> {
            ents.iter()
                .filter(|e| e.get("targetname") == Some(name) && e.origin().is_some())
                .filter_map(|e| by_name.get(e.get("target")?).copied())
                .collect()
        };
        // The paths in: from each start pointer's node, those reaching the
        // first destination.
        let dest = named("heli_dest").first().map(|&d| paths.nodes[d].at);
        let mut starts = Vec::new();
        for e in ents.iter().filter(|e| e.get("targetname") == Some("heli_start")) {
            let Some(&first) = e.get("target").and_then(|t| by_name.get(t)) else { continue };
            let mut node = Some(first);
            for _ in 0..256 {
                let Some(n) = node else { break };
                if dest.is_none_or(|d| paths.nodes[n].at == d) {
                    starts.push(first);
                    break;
                }
                node = paths.nodes[n].next;
            }
        }
        paths.starts = starts;
        paths.loops = named("heli_loop_start");
        paths.crashes = named("heli_crash_start");
        paths.leaves = ents.iter().filter(|e| e.get("targetname") == Some("heli_leave")).filter_map(|e| e.origin()).map(units::pos).collect();
        paths
    }

    fn is_loop_start(&self, n: usize) -> bool {
        self.loops.iter().any(|&l| self.nodes[l].at == self.nodes[n].at)
    }
}

/// Helicopters called, for [`spawn`] to bring in.
#[derive(Resource, Default)]
pub struct Calls(pub Vec<(Entity, Team)>);

/// A helicopter up: whose and which team's (bots shoot the enemy's).
#[derive(Component)]
pub struct Helicopter {
    pub owner: Entity,
    pub team: Team,
    kind: &'static Kind,
    damage_taken: f32,
    attacker: Option<Entity>,
    life: Life,
    evasive: bool,
    velocity: Vec3,
    /// CoD yaw (radians), and the yaw it turns to when it holds still.
    yaw: f32,
    goal_yaw: Option<f32>,
    leg: Option<Leg>,
    route: Option<Route>,
    wait: Option<(f32, Then)>,
    loops: u32,
    // Targeting.
    next_scan: f32,
    primary: Option<Entity>,
    secondary: Option<Entity>,
    antithreat: HashMap<Entity, f32>,
    // The turret: where it points (Bevy, world), at whom, and the burst.
    turret: Vec3,
    gun_target: Option<Entity>,
    gun: Gun,
    last_seen: Option<Vec3>,
    support: Option<f32>,
    missiles: u32,
    regen: f32,
    next_missile: f32,
    /// `tail_rotor_jnt` and `tag_engine_left`, for effects on them
    /// (`playfxontag`).
    tail: Entity,
    engine: Entity,
    // Smoke and fire trails, a puff every so often.
    next_puff: f32,
    tail_smoke: bool,
    engine_fire: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Life {
    Flying,
    Leaving,
    /// Spinning down the crash path; whether it's on it yet.
    Crashing { on_path: bool, since: f32 },
}

#[derive(Clone, Copy, Debug)]
struct Leg {
    goal: Vec3,
    /// Inches a second, and a second squared.
    speed: f32,
    accel: f32,
    stop: bool,
    /// The path node it's for (none once its route was replaced).
    node: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RouteKind {
    In,
    Loop,
    Crash,
}

#[derive(Clone, Copy, Debug)]
struct Route {
    current: usize,
    /// `heli_fly` waits two seconds before its first leg.
    begin: f32,
    kind: RouteKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Then {
    Continue,
    Evasive,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Gun {
    Idle,
    Aiming,
    SpinUp(f32),
    Firing { shots: u32, next: f32 },
    Reloading(f32),
}

/// A helicopter model and its sounds (`level.heli_sound[team]`); tags in
/// the model's space (CoD axes).
struct Kind {
    model: &'static str,
    rotor: &'static str,
    sounds: &'static str,
    rocket: &'static str,
    flash: [f32; 3],
    engine: [f32; 3],
    tail: [f32; 3],
    store: [f32; 3],
    /// Hitbox capsules: ends and radius.
    body: [([f32; 3], [f32; 3], f32); 2],
}

const COBRA: Kind = Kind {
    model: "vehicle_cobra_helicopter_fly",
    rotor: "mp_cobra_helicopter",
    sounds: "cobra",
    rocket: ROCKETS[0],
    flash: [201.95, 0.69, -148.6],
    engine: [-132.5, 18.9, -93.0],
    tail: [-357.3, -7.4, -60.7],
    store: [20.9, -39.6, -135.9],
    body: [([150.0, 0.0, -115.0], [-120.0, 0.0, -110.0], 42.0), ([-120.0, 0.0, -95.0], [-350.0, 0.0, -70.0], 20.0)],
};

const HIND: Kind = Kind {
    model: "vehicle_mi24p_hind_desert",
    rotor: "mp_hind_helicopter",
    sounds: "hind",
    rocket: ROCKETS[1],
    flash: [250.5, 0.14, -130.0],
    engine: [24.2, 31.6, -29.3],
    tail: [-385.3, 24.2, 24.3],
    store: [-6.0, -103.7, -87.9],
    body: [([200.0, 0.0, -80.0], [-80.0, 0.0, -60.0], 60.0), ([-80.0, 0.0, -40.0], [-400.0, 0.0, 0.0], 25.0)],
};

/// `p` is an enemy of the helicopter: of its owner (`owner`, while there is
/// one), else of its team.
fn enemy(team: Team, owner: Option<&Pawn>, p: &Pawn) -> bool {
    owner.map_or(p.team != team, |o| hostile(o, p))
}

impl Helicopter {
    /// Its turret is mid-burst.
    pub fn firing(&self) -> bool {
        matches!(self.gun, Gun::Firing { .. })
    }

    fn damaged(&self) -> bool {
        self.damage_taken >= ARMOR
    }

    /// `heli_fly( start )`: a route from `start`, after two seconds.
    fn fly(&mut self, start: usize, kind: RouteKind, now: f32) {
        if let Some(leg) = &mut self.leg {
            leg.node = None;
        }
        self.wait = None;
        self.route = Some(Route { current: start, begin: now + 2.0, kind });
    }

    /// `heli_evasive`: round the loop path (or away, without one).
    fn evasive(&mut self, paths: &HeliPaths, now: f32, pos: Vec3) {
        self.evasive = true;
        match paths.loops.first() {
            Some(&l) => self.fly(l, RouteKind::Loop, now),
            None => self.leave(paths, pos),
        }
    }

    /// `heli_leave`: off to a random leave point at 100 mph; it can't be
    /// hurt or fight on the way.
    fn leave(&mut self, paths: &HeliPaths, pos: Vec3) {
        self.life = Life::Leaving;
        self.route = None;
        self.wait = None;
        self.goal_yaw = None;
        let goal = if paths.leaves.is_empty() {
            pos + Vec3::Y * u(8000.0)
        } else {
            paths.leaves[rand::random_range(0..paths.leaves.len())]
        };
        self.leg = Some(Leg { goal, speed: 100.0 * MPH, accel: 45.0 * MPH, stop: true, node: None });
    }
}

/// `heli_think`: bring in the helicopters called, at a random path's start.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn(
    mut commands: Commands,
    time: Res<Time>,
    mut calls: ResMut<Calls>,
    paths: Res<HeliPaths>,
    mut content: Option<ResMut<Content>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let now = time.elapsed_secs();
    for (owner, team) in calls.0.drain(..) {
        let Some(&start) = paths.starts.get(rand::random_range(0..paths.starts.len().max(1))) else { continue };
        let node = &paths.nodes[start];
        let kind: &'static Kind = if team == Team::Allies { &COBRA } else { &HIND };
        let transform = Transform::from_translation(node.at).with_rotation(Quat::from_rotation_y(node.yaw));
        let tag = |commands: &mut Commands, at: [f32; 3]| commands.spawn((Transform::from_translation(units::pos(at)), Visibility::default())).id();
        let (tail, engine) = (tag(&mut commands, kind.tail), tag(&mut commands, kind.engine));
        let heli = commands
            .spawn((
                Name::new("helicopter"),
                Helicopter {
                    owner,
                    team,
                    kind,
                    damage_taken: 0.0,
                    attacker: None,
                    life: Life::Flying,
                    evasive: false,
                    velocity: Vec3::ZERO,
                    yaw: node.yaw,
                    goal_yaw: None,
                    leg: None,
                    route: Some(Route { current: start, begin: now + 2.0, kind: RouteKind::In }),
                    wait: None,
                    loops: 0,
                    next_scan: now,
                    primary: None,
                    secondary: None,
                    antithreat: HashMap::new(),
                    turret: units::dir([node.yaw.cos(), node.yaw.sin(), 0.0]),
                    gun_target: None,
                    gun: Gun::Idle,
                    last_seen: None,
                    support: None,
                    missiles: MISSILE_MAX,
                    regen: now + MISSILE_REGEN,
                    next_missile: now,
                    tail,
                    engine,
                    next_puff: now,
                    tail_smoke: false,
                    engine_fire: false,
                },
                transform,
                Visibility::default(),
            ))
            .id();
        let model = content.as_deref_mut().and_then(|c| {
            spawn_static_model(&mut commands, c, &mut meshes, &mut materials, &mut images, kind.model, Transform::default())
        });
        if let Some(model) = model {
            commands.entity(model).insert(ChildOf(heli));
        }
        commands.entity(tail).insert(ChildOf(heli));
        commands.entity(engine).insert(ChildOf(heli));
        for (a, b, r) in kind.body {
            commands.spawn((
                Hitbox { owner: heli, location: HitLocation::Torso },
                Collider::capsule_endpoints(u(r), units::pos(a), units::pos(b)),
                CollisionLayers::new(Layer::Hitbox, LayerMask::NONE),
                Transform::default(),
                ChildOf(heli),
            ));
        }
        sfx.play_on(kind.rotor, heli);
        info!("helicopter: {} in for {team:?} from CoD {:?}", kind.model, units::to_cod(node.at).map(f32::round));
    }
}

/// Damage to helicopters (`heli_damage_monitor`): only its owner's enemies
/// hurt it, bullets a third as much while its armour holds, blasts
/// ([`Exploded`]) by how near they go off; past the armour it goes evasive,
/// past its health it crashes.
/// A helicopter was shot down, by whom (for the challenges).
#[derive(Message, Clone, Copy, Debug)]
pub struct ShotDown {
    pub by: Entity,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn take_damage(
    time: Res<Time>,
    mut damage: MessageReader<Damage>,
    mut blasts: MessageReader<Exploded>,
    mut helis: Query<(&mut Helicopter, Entity)>,
    pawns: Query<&Pawn>,
    explosives: Res<ExplosiveDefs>,
    paths: Res<HeliPaths>,
    mut hits: MessageWriter<HitConfirmed>,
    mut shot_down: MessageWriter<ShotDown>,
    mut fx: Option<ResMut<Effects>>,
    mut sfx: ResMut<crate::audio::Sfx>,
    transforms: Query<&GlobalTransform>,
) {
    let now = time.elapsed_secs();
    // Blasts reach a helicopter's body (its first hitbox) by how near they
    // go off, from their inner damage to their outer at their radius.
    let mut hurt: Vec<(Entity, Entity, f32, &'static str)> = Vec::new();
    for b in blasts.read() {
        for (h, heli) in &helis {
            let Ok(g) = transforms.get(heli) else { continue };
            let (a, c, r) = h.kind.body[0];
            let centre = g.transform_point(units::pos(((Vec3::from(a) + Vec3::from(c)) * 0.5).to_array()));
            let dist = (b.at.distance(centre) / INCH - r).max(0.0);
            if dist < b.radius && b.inner + b.outer > 0.0 {
                hurt.push((heli, b.owner, b.inner + (b.outer - b.inner) * dist / b.radius.max(1.0), "blast"));
            }
        }
    }
    let shots = damage.read().map(|d| (d.target, d.attacker.unwrap_or(Entity::PLACEHOLDER), d.amount, d.weapon));
    for (target, attacker, amount, weapon) in shots.chain(hurt) {
        let Ok((mut h, _)) = helis.get_mut(target) else { continue };
        if h.life != Life::Flying || attacker == h.owner {
            continue;
        }
        let Ok(by) = pawns.get(attacker) else { continue };
        if !enemy(h.team, pawns.get(h.owner).ok(), by) {
            continue;
        }
        hits.write(HitConfirmed { shooter: attacker, headshot: false });
        h.attacker = Some(attacker);
        let bullet = weapon != "blast"
            && explosives.kill_icon(weapon).is_none()
            && crate::grenades::kill_icon(weapon).is_none()
            && super::kill_icon(weapon).is_none();
        h.damage_taken += if bullet && h.damage_taken < ARMOR { amount * ARMOR_BULLET } else { amount };
        debug!("helicopter: {weapon} {amount:.0} -> {:.0}/{MAX_HEALTH}", h.damage_taken);
        let d = (target, attacker);
        let pos = transforms.get(d.0).map_or(Vec3::ZERO, |g| g.translation());
        if h.damage_taken >= ARMOR && !h.evasive {
            h.evasive(&paths, now, pos);
        }
        if h.damage_taken > MAX_HEALTH {
            // `heli_crash` and `heli_spin`: the tail blows and it spins
            // down the crash path.
            info!("helicopter: shot down by {}", by.name);
            shot_down.write(ShotDown { by: attacker });
            h.life = Life::Crashing { on_path: false, since: now };
            h.goal_yaw = None;
            match paths.crashes.first() {
                Some(&c) => h.fly(c, RouteKind::Crash, now),
                None => h.route = None,
            }
            if let Some(fx) = fx.as_deref_mut() {
                fx.play("explosions/aerial_explosion", Anchor::Bolted(h.tail), FxLayer::World);
            }
            sfx.play_on(format!("{}_helicopter_hit", h.kind.sounds), h.tail);
            h.tail_smoke = true;
        }
    }
}

/// Where the helicopter flies, and what it looks like doing it.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn fly(
    mut commands: Commands,
    time: Res<Time>,
    paths: Res<HeliPaths>,
    mut helis: Query<(Entity, &mut Helicopter, &mut Transform)>,
    sounds: Query<(Entity, &ChildOf), With<AudioPlayer>>,
    mut fx: Option<ResMut<Effects>>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    for (e, mut h, mut tf) in &mut helis {
        let h = &mut *h;
        // The next leg of the route, after its first two seconds.
        if let Some(route) = h.route.filter(|r| now >= r.begin && h.wait.is_none() && h.leg.is_none_or(|l| l.node.is_none())) {
            let cur = &paths.nodes[route.current];
            match cur.next {
                Some(n) => {
                    let next = &paths.nodes[n];
                    let (mph, accel) = cur.speed.unwrap_or_else(|| (rand::random_range(30.0..50.0), rand::random_range(15.0..30.0)));
                    let delay = next.delay.filter(|_| !h.damaged());
                    let stop = next.next.is_none() || delay.is_some();
                    h.goal_yaw = delay.map(|_| next.yaw);
                    h.leg = Some(Leg { goal: next.at + Vec3::Y * u(30.0), speed: mph * MPH, accel: accel * MPH, stop, node: Some(n) });
                }
                // Where the route ends: the crash path's, it explodes; else
                // it holds two seconds facing the node's way, then circles.
                None if route.kind == RouteKind::Crash => {
                    explode(&mut commands, e, h, &tf, fx.as_deref_mut(), &mut sfx);
                    continue;
                }
                None => {
                    h.goal_yaw = Some(cur.yaw);
                    h.route = None;
                    h.wait = Some((now + DEST_WAIT, Then::Evasive));
                }
            }
        }
        if let Some((until, then)) = h.wait.filter(|w| now >= w.0) {
            let _ = until;
            h.wait = None;
            if then == Then::Evasive {
                h.evasive(&paths, now, tf.translation);
            }
        }

        // Steer for the leg's goal, slowing to a stop where it stops.
        let pos = tf.translation;
        let (goal, speed, accel, stop) = match h.leg {
            Some(l) => (l.goal, u(l.speed), u(l.accel), l.stop),
            None => (pos, 0.0, u(25.0 * MPH), true),
        };
        let to = goal - pos;
        let dist = to.length();
        let want = if stop { speed.min((2.0 * accel * dist).sqrt()) } else { speed };
        let desired = to.normalize_or_zero() * want;
        h.velocity = h.velocity.move_towards(desired, STEER * accel * dt);
        tf.translation += h.velocity * dt;

        if let Some(leg) = h.leg {
            let left = (goal - tf.translation).length() / INCH;
            let arrived = if leg.stop { left < 16.0 && h.velocity.length() / INCH < 40.0 } else { left < NEAR_GOAL };
            if arrived {
                h.leg = None;
                if h.life == Life::Leaving {
                    debug!("helicopter: gone");
                    commands.entity(e).despawn();
                    continue;
                }
                if let (Some(n), Some(mut route)) = (leg.node, h.route) {
                    if let Life::Crashing { on_path: false, since } = h.life {
                        // `"path start"`: the engine blows and burns.
                        h.life = Life::Crashing { on_path: true, since };
                        if let Some(fx) = fx.as_deref_mut() {
                            fx.play("explosions/aerial_explosion_large", Anchor::Bolted(h.engine), FxLayer::World);
                        }
                        sfx.play_on(format!("{}_helicopter_secondary_exp", h.kind.sounds), h.engine);
                        h.engine_fire = true;
                    }
                    if leg.stop && paths.nodes[n].next.is_some() {
                        if let Some(delay) = paths.nodes[n].delay {
                            h.wait = Some((now + delay, Then::Continue));
                        }
                    }
                    if route.kind != RouteKind::Crash && paths.is_loop_start(n) {
                        h.loops += 1;
                    }
                    if h.loops >= LOOP_MAX && h.life == Life::Flying {
                        h.leave(&paths, tf.translation);
                    } else {
                        route.current = n;
                        h.route = Some(route);
                    }
                }
            }
        }

        // Heading: the goal yaw holding still, else the way it's going; a
        // crash spins it.
        let crashing = matches!(h.life, Life::Crashing { .. });
        let before = h.yaw;
        if crashing {
            h.yaw += (SPIN_SPEED * 0.9).to_radians() * dt;
        } else {
            let flat = Vec2::new(h.velocity.x, -h.velocity.z);
            let target = h.goal_yaw.filter(|_| flat.length() < u(200.0)).or_else(|| (flat.length() > u(60.0)).then(|| flat.y.atan2(flat.x)));
            if let Some(t) = target {
                let diff = (t - h.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                let step = YAW_SPEED.to_radians() * dt;
                h.yaw += diff.clamp(-step, step);
            }
        }
        // Nose down with speed, banked into turns (up to 30 degrees:
        // `setmaxpitchroll( 30, 30 )`).
        let forward = units::dir([h.yaw.cos(), h.yaw.sin(), 0.0]);
        let forward_speed = h.velocity.dot(forward) / INCH;
        let pitch = (forward_speed / (60.0 * MPH) * 10.0).clamp(-30.0, 30.0).to_radians();
        let turn = (h.yaw - before) / dt.max(1e-4);
        let roll = (turn.to_degrees() * 0.25).clamp(-30.0, 30.0).to_radians();
        tf.rotation = Quat::from_rotation_y(h.yaw) * Quat::from_rotation_z(-pitch) * Quat::from_rotation_x(-roll);

        // `heli_health`'s smoke, and a crash's trails.
        if now >= h.next_puff {
            h.next_puff = now + 0.1;
            if let Some(fx) = fx.as_deref_mut() {
                let engine = if h.engine_fire {
                    Some("fire/fire_smoke_trail_L")
                } else if h.damage_taken >= LOW_HEALTH {
                    Some("smoke/smoke_trail_black_heli")
                } else if h.damaged() {
                    Some("smoke/smoke_trail_white_heli")
                } else {
                    None
                };
                if let Some(name) = engine {
                    fx.play(name, Anchor::Bolted(h.engine), FxLayer::World);
                }
                if h.tail_smoke {
                    fx.play("smoke/smoke_trail_white_heli", Anchor::Bolted(h.tail), FxLayer::World);
                }
            }
        }
        // `spinSoundShortly`: the rotors give way to the dying loop.
        if let Life::Crashing { since, .. } = h.life {
            if now - since >= 0.25 && now - since - dt < 0.25 {
                for (s, parent) in &sounds {
                    if parent.parent() == e {
                        commands.entity(s).despawn();
                    }
                }
                sfx.play_on(format!("{}_helicopter_dying_loop", h.kind.sounds), e);
                sfx.play_on(format!("{}_helicopter_dying_layer", h.kind.sounds), e);
            }
            // No crash path: it goes up where it is.
            if h.route.is_none() && now - since > 2.0 {
                explode(&mut commands, e, h, &tf, fx.as_deref_mut(), &mut sfx);
            }
        }
    }
}

/// `heli_explode`.
fn explode(commands: &mut Commands, e: Entity, h: &Helicopter, tf: &Transform, fx: Option<&mut Effects>, sfx: &mut crate::audio::Sfx) {
    info!("helicopter: exploded");
    if let Some(fx) = fx {
        fx.play("explosions/helicopter_explosion_cobra", Anchor::Fixed(Frame::facing(tf.translation, Vec3::Y, 0.0)), FxLayer::World);
    }
    sfx.play(format!("{}_helicopter_crash", h.kind.sounds), Some(tf.translation));
    commands.entity(e).despawn();
}

/// `canTarget_turret`'s `sightConeTrace`: how much of a pawn (feet, middle,
/// eyes) can be seen from `from`.
fn seen(spatial: &SpatialQuery, from: Vec3, feet: Vec3, eye: Vec3) -> f32 {
    let filter = collision::sight_filter();
    let points = [feet + Vec3::Y * u(8.0), (feet + eye) * 0.5, eye];
    let clear = |p: Vec3| {
        let d = p - from;
        Dir3::new(d).is_ok_and(|dir| spatial.cast_ray(from, dir, d.length() - u(2.0), true, &filter).is_none())
    };
    points.iter().filter(|&&p| clear(p)).count() as f32 / points.len() as f32
}

/// Targeting (`heli_targeting`), the turret (`attack_primary`) and the
/// rockets (`attack_secondary`, `missile_support`).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn fight(
    time: Res<Time>,
    mut helis: Query<(Entity, &mut Helicopter, &Transform)>,
    pawns: Query<(Entity, &Pawn, &Transform, &Mover, Has<Dead>)>,
    hitboxes: Query<&Hitbox>,
    spatial: SpatialQuery,
    mut alive_since: Local<HashMap<Entity, f32>>,
    mut damage: MessageWriter<Damage>,
    mut impacts: MessageWriter<BulletImpact>,
    mut launches: MessageWriter<Launch>,
    mut fx: Option<ResMut<Effects>>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    for (e, _, _, _, dead) in &pawns {
        if dead {
            alive_since.remove(&e);
        } else {
            alive_since.entry(e).or_insert(now);
        }
    }
    let mut rng = rand::rng();
    for (heli, mut h, tf) in &mut helis {
        let h = &mut *h;
        if h.life != Life::Flying {
            continue;
        }
        let owner = pawns.get(h.owner).ok().map(|p| p.1);
        let forward = tf.rotation * Vec3::X;
        let turret_point = tf.translation - Vec3::Y * u(160.0) + forward * u(144.0);
        let aim_point = |p: Vec3| p + Vec3::Y * u(40.0);

        // Rockets come back one every ten seconds (faster when hurt).
        if h.missiles < MISSILE_MAX && now >= h.regen {
            h.missiles += 1;
            let hurt = if h.damage_taken >= LOW_HEALTH { 4.0 } else if h.damaged() { 2.0 } else { 1.0 };
            h.regen = now + MISSILE_REGEN / hurt;
        }

        // Every half second: who it can see, most threatening first.
        if now >= h.next_scan {
            h.next_scan = now + TARGETING_DELAY;
            let mut targets: Vec<(Entity, f32)> = pawns
                .iter()
                .filter(|(p, pawn, ptf, _, dead)| {
                    !dead && *p != h.owner && enemy(h.team, owner, pawn) && ptf.translation.distance(tf.translation) / INCH <= VISUAL_RANGE
                })
                .filter(|(p, ..)| alive_since.get(p).is_some_and(|&t| now - t > SPAWN_PROTECTION))
                .filter(|(_, _, ptf, mover, _)| seen(&spatial, turret_point, ptf.translation, mover.eye(ptf.translation)) >= RECOGNITION)
                .map(|(p, pawn, ptf, ..)| {
                    let dist = ptf.translation.distance(tf.translation) / INCH;
                    let mut threat = (VISUAL_RANGE - dist) / VISUAL_RANGE * 100.0;
                    if h.attacker == Some(p) {
                        threat += 100.0;
                    }
                    // A TDM kill is worth 10.
                    threat += pawn.kills as f32 * 10.0 * 4.0;
                    threat -= h.antithreat.get(&p).copied().unwrap_or(0.0);
                    (p, threat.max(1.0))
                })
                .collect();
            targets.sort_by(|a, b| b.1.total_cmp(&a.1));
            let was = h.primary;
            h.primary = targets.first().map(|t| t.0);
            h.secondary = targets.get(1).map(|t| t.0);
            if h.primary != was {
                debug!("helicopter: targets {:?}", targets);
            }
        }

        // The turret: on its target, spun up, a burst, a reload.
        let target_pos = |t: Entity| pawns.get(t).ok().filter(|p| !p.4).map(|p| (p.2.translation, p.3.eye(p.2.translation)));
        match h.gun {
            Gun::Idle => {
                if let Some(p) = h.primary {
                    h.gun_target = Some(p);
                    h.gun = Gun::Aiming;
                    h.antithreat.remove(&p);
                    // A rocket instead if the turret is slow getting there.
                    let ahead = target_pos(p).is_some_and(|(feet, _)| (feet - tf.translation).normalize_or_zero().dot(forward) >= MISSILE_CONE);
                    h.support = ahead.then_some(now + MISSILE_SUPPORT);
                }
            }
            Gun::Aiming if h.gun_target.and_then(target_pos).is_none() => h.gun = Gun::Idle,
            _ => {}
        }
        // Where the turret wants to point: its target, the last place it
        // saw them, or where it points.
        if matches!(h.gun, Gun::SpinUp(_) | Gun::Firing { .. }) && h.primary.is_some() && h.primary != h.gun_target {
            h.gun_target = h.primary;
        }
        let target = h.gun_target.and_then(target_pos);
        if let Some((feet, eye)) = target {
            if matches!(h.gun, Gun::Firing { .. }) && seen(&spatial, turret_point, feet, eye) < RECOGNITION {
                h.last_seen.get_or_insert(aim_point(feet));
            } else {
                h.last_seen = None;
            }
        }
        let flash = tf.transform_point(units::pos(h.kind.flash));
        let wanted = match (h.last_seen, target) {
            (Some(at), _) => Some(at),
            (None, Some((feet, _))) => Some(aim_point(feet)),
            _ => None,
        };
        if let Some(dir) = wanted.and_then(|w| (w - flash).try_normalize()) {
            let angle = h.turret.angle_between(dir);
            let step = TURRET_RATE.to_radians() * dt;
            h.turret = if angle <= step { dir } else { h.turret.slerp(dir, step / angle).normalize() };
        }
        let on_target = wanted.is_some_and(|w| h.turret.angle_between((w - flash).normalize_or_zero()) < 3f32.to_radians());
        match h.gun {
            Gun::Aiming if on_target => {
                h.support = None;
                h.gun = Gun::SpinUp(now + SPINUP);
            }
            Gun::Aiming => {
                if let (Some(at), Some(t)) = (h.support, h.gun_target) {
                    if now >= at {
                        h.support = None;
                        fire_rocket(h, t, tf, &pawns, &mut launches, &mut sfx);
                    }
                }
            }
            Gun::SpinUp(until) if now >= until => {
                debug!("helicopter: firing at {:?}", h.gun_target.and_then(|t| pawns.get(t).ok()).map(|p| p.1.name.clone()));
                h.gun = Gun::Firing { shots: 0, next: now };
            }
            Gun::Firing { mut shots, mut next } => {
                while shots < CLIP && now >= next {
                    shots += 1;
                    next += FIRE_TIME;
                    shoot(h, heli, flash, &spatial, &hitboxes, &pawns, &mut damage, &mut impacts, fx.as_deref_mut(), &mut sfx, &mut rng);
                }
                h.gun = if shots >= CLIP { Gun::Reloading(now + RELOAD) } else { Gun::Firing { shots, next } };
            }
            Gun::Reloading(until) if now >= until => {
                // Who's been shot at counts for less.
                if let Some(t) = h.gun_target.filter(|&t| target_pos(t).is_some()) {
                    *h.antithreat.entry(t).or_insert(0.0) += 100.0;
                }
                h.last_seen = None;
                h.gun = if h.primary.is_some() && h.primary == h.gun_target { Gun::Aiming } else { Gun::Idle };
            }
            _ => {}
        }

        // The second target: a rocket every five seconds while it's ahead.
        if let Some(t) = h.secondary.filter(|_| now >= h.next_missile) {
            let ahead = target_pos(t).is_some_and(|(feet, _)| (feet - tf.translation).normalize_or_zero().dot(forward) >= MISSILE_CONE);
            if ahead && fire_rocket(h, t, tf, &pawns, &mut launches, &mut sfx) {
                *h.antithreat.entry(t).or_insert(0.0) += 100.0;
                h.next_missile = now + MISSILE_ROF;
            }
        }
    }
}

/// One turret round from `flash` along the turret, a little spread.
#[allow(clippy::too_many_arguments)]
fn shoot(
    h: &Helicopter,
    heli: Entity,
    flash: Vec3,
    spatial: &SpatialQuery,
    hitboxes: &Query<&Hitbox>,
    pawns: &Query<(Entity, &Pawn, &Transform, &Mover, Has<Dead>)>,
    damage: &mut MessageWriter<Damage>,
    impacts: &mut MessageWriter<BulletImpact>,
    fx: Option<&mut Effects>,
    sfx: &mut crate::audio::Sfx,
    rng: &mut impl rand::Rng,
) {
    let r = SPREAD.to_radians() * rng.random::<f32>().sqrt();
    let theta = rng.random_range(0.0..std::f32::consts::TAU);
    let side = h.turret.any_orthonormal_vector();
    let dir = (Quat::from_axis_angle(h.turret, theta) * Quat::from_axis_angle(side, r) * h.turret).normalize();
    if let Some(fx) = fx {
        fx.play("muzzleflashes/cobra_20mm_flash_mp", Anchor::Fixed(Frame::facing(flash, dir, 0.0)), FxLayer::World);
    }
    sfx.play("weap_m197_cannon_fire", Some(flash));
    let not_self = |e: Entity| hitboxes.get(e).map_or(true, |hb| hb.owner != heli);
    let Ok(d) = Dir3::new(dir) else { return };
    let Some(hit) = spatial.cast_ray_predicate(flash, d, u(15000.0), true, &collision::bullet_filter(), &not_self) else { return };
    let to = flash + dir * hit.distance;
    let what = match hitboxes.get(hit.entity) {
        Ok(hb) if pawns.get(hb.owner).is_ok() => {
            if hb.owner != h.owner {
                debug!("helicopter: hit {:?}", pawns.get(hb.owner).map(|p| p.1.name.clone()).ok());
                damage.write(Damage { target: hb.owner, attacker: Some(h.owner), amount: DAMAGE, location: hb.location, weapon: WEAPON });
            }
            BulletHit::Flesh { head: hb.location == HitLocation::Head }
        }
        Ok(_) => BulletHit::Metal,
        Err(_) => BulletHit::World,
    };
    impacts.write(BulletImpact { from: flash, to, normal: hit.normal, impact_type: IMPACT_TYPE, hit: what });
}

/// `missile_support`: a rocket at `target` from the right pylon, unless a
/// friendly is near them or none are left. Whether one went.
fn fire_rocket(
    h: &mut Helicopter,
    target: Entity,
    tf: &Transform,
    pawns: &Query<(Entity, &Pawn, &Transform, &Mover, Has<Dead>)>,
    launches: &mut MessageWriter<Launch>,
    sfx: &mut crate::audio::Sfx,
) -> bool {
    let Ok((_, _, ttf, ..)) = pawns.get(target) else { return false };
    let at = ttf.translation;
    // Its owner's side: teammates, or in free-for-all the owner alone.
    let owner = pawns.get(h.owner).ok().map(|p| p.1);
    let near_friendly = pawns
        .iter()
        .any(|(_, p, ptf, _, dead)| !dead && !enemy(h.team, owner, p) && ptf.translation.distance(at) / INCH <= FRIENDLY_CARE);
    if near_friendly || h.missiles == 0 {
        return false;
    }
    h.missiles -= 1;
    let store = tf.transform_point(units::pos(h.kind.store));
    let dir = (at + Vec3::Y * u(20.0) - store).normalize_or_zero();
    // Clear of its own hitbox before it flies.
    let from = store + dir * u(80.0);
    let flat = Vec2::new(dir.x, -dir.z);
    launches.write(Launch { shooter: h.owner, weapon: h.kind.rocket.into(), from, dir, yaw: flat.y.atan2(flat.x) - std::f32::consts::FRAC_PI_2 });
    sfx.play(format!("weap_{}_missile_fire", h.kind.sounds), Some(store));
    true
}

/// The map's paths, and the rockets as explosives.
pub(super) fn load(mut commands: Commands, content: Res<Content>, mut defs: ResMut<ExplosiveDefs>) {
    let paths = content.map().map_ents().map(|m| HeliPaths::from_ents(&iw3::ents::parse(&m.entity_string))).unwrap_or_default();
    info!(
        "helicopter: {} paths in, {} loop, {} crash, {} leave points",
        paths.starts.len(),
        paths.loops.len(),
        paths.crashes.len(),
        paths.leaves.len()
    );
    commands.insert_resource(paths);
    for name in ROCKETS {
        let Some((zi, w)) = content.generic(AssetType::Weapon, name) else { continue };
        let zone = &content.zones[zi];
        let asset = |f: &str| w.asset(f).map(|i| zone.get(i).name().trim_start_matches(',').to_owned());
        defs.insert(name, ExplosiveDef {
            behaviour: Behaviour::Rocket,
            speed: w.int("iProjectileSpeed") as f32,
            speed_up: 0.0,
            speed_forward: 0.0,
            delay: 0.0,
            arming: 0.0,
            impact_damage: w.int("damage") as f32,
            radius: w.int("iExplosionRadius") as f32,
            inner_damage: w.int("iExplosionInnerDamage") as f32,
            outer_damage: w.int("iExplosionOuterDamage") as f32,
            lifetime: 10.0,
            cone: 180.0,
            model: asset("projectileModel").unwrap_or_default(),
            trail: asset("projTrailEffect").unwrap_or_default(),
            effect: asset("projExplosionEffect").unwrap_or_else(|| "explosions/aerial_explosion".into()),
            sound: "rocket_explode_default".into(),
            display: name,
            icon: "death_helicopter".into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(pairs: &[(&str, &str)]) -> iw3::ents::Entity {
        let s = format!("{{\n{}\n}}\n", pairs.iter().map(|(k, v)| format!("\"{k}\" \"{v}\"")).collect::<Vec<_>>().join("\n"));
        iw3::ents::parse(&s).remove(0)
    }

    #[test]
    fn paths_follow_targets_to_the_destination() {
        let ents = vec![
            ent(&[("targetname", "heli_start"), ("target", "a"), ("origin", "0 0 0")]),
            ent(&[("targetname", "a"), ("target", "b"), ("origin", "100 0 0"), ("script_airspeed", "40"), ("script_accel", "20")]),
            ent(&[("targetname", "b"), ("origin", "200 0 0")]),
            ent(&[("targetname", "heli_dest"), ("target", "b"), ("origin", "0 0 0")]),
            ent(&[("targetname", "heli_loop_start"), ("target", "l1"), ("origin", "0 0 0")]),
            ent(&[("targetname", "l1"), ("target", "l2"), ("origin", "0 500 0")]),
            ent(&[("targetname", "l2"), ("target", "l1"), ("origin", "500 500 0"), ("script_delay", "3")]),
            ent(&[("targetname", "heli_leave"), ("origin", "9000 0 900")]),
        ];
        let p = HeliPaths::from_ents(&ents);
        assert!(p.available());
        let a = p.starts[0];
        assert_eq!(p.nodes[a].speed, Some((40.0, 20.0)));
        let b = p.nodes[a].next.unwrap();
        assert_eq!(p.nodes[b].at, units::pos([200.0, 0.0, 0.0]));
        assert!(p.nodes[b].next.is_none());
        let l1 = p.loops[0];
        assert!(p.is_loop_start(l1));
        let l2 = p.nodes[l1].next.unwrap();
        assert_eq!(p.nodes[l2].delay, Some(3.0));
        assert_eq!(p.nodes[l2].next, Some(l1));
        assert_eq!(p.leaves.len(), 1);
    }
}
