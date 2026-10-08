//! Headquarters' background life, as the training level's own script set it
//! up (`maps/killhouse.gsc`): the Black Hawk flying its route over the base
//! (`create_vehicle_from_spawngroup_and_gopath( 8 )`), coming down at the
//! landing spot, then climbing out round the long circuit, again and again,
//! rotors turning and engine running.
//!
//! The perimeter roads' traffic (`ambient_trucks`): every 3 to 6 seconds
//! one of the level's eight vehicle groups (trucks, sedans, a hatchback, a
//! wagon, a Humvee, a bus) sets off along the four roads round the base at
//! 30 to 50 miles an hour, gone at the road's end; a few at once at most.
//!
//! Mac's three recruits run his obstacle course over and over, as they
//! did in the level (`obstacleTrainingCourseThink`): down their lanes, out
//! of the trench, over the walls, under the wire on their bellies, the
//! sprint home, then back to the line.
//!
//! And the level's people at work: the private at his laptop by the range
//! (`chair_guy`, its three seated idles), Gaz at the range, Newcastle at the
//! explosives pit and Mac by the obstacle course, idling.
//!
//! Vehicles follow the level's own nodes (each `target`s the next), on a
//! curve through them, at the speeds the nodes give (miles an hour), easing
//! into the nodes they stop at; they face where they're going, banking into
//! turns.

use crate::models::{AnimPlayer, Skeleton, SpawnModel, spawn_model};
use crate::units::u;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use std::collections::HashMap;

pub(super) fn build(app: &mut App) {
    app.add_systems(
        OnEnter(crate::state::GameState::InGame),
        spawn_vehicles.after(super::spawn_figures).in_set(crate::state::Setup::Spawn),
    )
    .add_systems(Update, (drive, traffic, idle, run_lanes).run_if(|| super::active()));
}

/// A route's point: where (Bevy), the speed from here on (units a second,
/// if the node sets one), and whether the vehicle stops here.
#[derive(Clone, Copy, Debug)]
pub(super) struct Node {
    at: Vec3,
    speed: Option<f32>,
    stop: bool,
}

/// Miles an hour in CoD units (inches) a second.
const MPH: f32 = 17.6;
/// How long a vehicle waits at a stop (the Black Hawk unloading).
const STOP_WAIT: f32 = 8.0;
/// How hard vehicles speed up and slow down (units a second, squared).
const ACCEL: f32 = 260.0;

#[derive(Component)]
struct Vehicle {
    route: Vec<Node>,
    /// The segment it's on (from node `seg` to the next), and how far along
    /// it (0..1).
    seg: usize,
    along: f32,
    speed: f32,
    cruise: f32,
    wait_until: f32,
    /// Smoothed: facing (Bevy yaw of its nose) and bank.
    yaw: f32,
    bank: f32,
    /// Banks into turns (a helicopter) or not.
    banks: bool,
    /// Round and round (a helicopter's circuit), or gone at the end (a
    /// road's).
    looped: bool,
    /// On a road: placed on it yet, and its pitch (smoothed).
    grounded: bool,
    pitch: f32,
}

/// A route from a node's targetname, following `target`s until they end or
/// come back round; looped back to the start.
fn route(ents: &[iw3::ents::Entity], from: &str) -> Vec<Node> {
    let by_name: HashMap<&str, &iw3::ents::Entity> = ents.iter().filter_map(|e| Some((e.get("targetname")?, e))).collect();
    let mut out = Vec::new();
    let mut next = Some(from);
    let mut seen = std::collections::HashSet::new();
    while let Some(name) = next {
        if !seen.insert(name) {
            break;
        }
        let Some(e) = by_name.get(name) else { break };
        let Some(origin) = e.origin() else { break };
        let speed = e.get("speed").and_then(|s| s.trim().parse::<f32>().ok()).map(|mph| mph * MPH);
        let stop = e.get("script_stopnode").is_some_and(|s| s.trim() == "1");
        out.push(Node { at: crate::units::pos(origin), speed, stop });
        next = e.get("target");
    }
    out
}

/// The point `along` (0..1) the segment from node `seg`, on a Catmull-Rom
/// curve through the looped route.
fn point(route: &[Node], seg: usize, along: f32, looped: bool) -> Vec3 {
    let n = route.len();
    let p = |i: isize| route[if looped { i.rem_euclid(n as isize) } else { i.clamp(0, n as isize - 1) } as usize].at;
    let i = seg as isize;
    let (p0, p1, p2, p3) = (p(i - 1), p(i), p(i + 1), p(i + 2));
    let t = along;
    let (t2, t3) = (t * t, t * t * t);
    0.5 * ((2.0 * p1) + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
}

/// The Black Hawk: the level's spawner of spawn group 8, from where it
/// stands along the route its `target` starts.
#[allow(clippy::too_many_arguments)]
fn spawn_vehicles(
    mut commands: Commands,
    hq: Option<Res<super::Headquarters>>,
    time: Res<Time>,
    mut content: ResMut<crate::content::Content>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    (mut camo_materials, mut camos): (ResMut<Assets<crate::gunmodel::CamoMaterial>>, ResMut<crate::gunmodel::WorldCamos>),
) {
    // Debug: `COD4RW_PERF=...,noambient` leaves it all out, to measure it.
    if hq.is_none() || std::env::var("COD4RW_PERF").is_ok_and(|p| p.split(',').any(|o| o.trim() == "noambient")) {
        return;
    }
    let ents = content.map().map_ents().map(|e| iw3::ents::parse(&e.entity_string)).unwrap_or_default();
    let heli = ents.iter().find(|e| e.classname() == "script_vehicle" && e.get("vehicletype") == Some("blackhawk"));
    let Some(heli) = heli else { return };
    let (Some(model), Some(origin), Some(target)) = (heli.get("model"), heli.origin(), heli.get("target")) else { return };
    let mut nodes = vec![Node { at: crate::units::pos(origin), speed: None, stop: false }];
    nodes.extend(route(&ents, target));
    if nodes.len() < 3 {
        return;
    }
    let Some(prepared) = content.model(model, &mut meshes, &mut materials, &mut images, &mut bindposes) else {
        warn!("headquarters: no {model}");
        return;
    };
    let start = nodes[0].at;
    let towards = (nodes[1].at - start).with_y(0.0).normalize_or(Vec3::X);
    let yaw = (-towards.z).atan2(towards.x);
    let entity = commands
        .spawn((Name::new(format!("hq vehicle {model}")), Transform::from_translation(start), Visibility::default()))
        .id();
    let mut skeleton = Skeleton::default();
    spawn_model(&mut commands, &mut skeleton, SpawnModel { model: &prepared, owner: entity, attach_to: None, layers: None, shadows: true });
    // The rotors (`bh_rotors` turns `main_rotor_jnt` and `tail_rotor_jnt`).
    let mut player = AnimPlayer::default();
    if let Some(a) = content.anim("bh_rotors") {
        player.play(a, 0.0);
    }
    let cruise = nodes.iter().find_map(|n| n.speed).unwrap_or(60.0 * MPH);
    commands.entity(entity).insert((
        skeleton,
        player,
        Vehicle { route: nodes, seg: 0, along: 0.0, speed: u(cruise), cruise: u(cruise), wait_until: time.elapsed_secs(), yaw, bank: 0.0, banks: true, looped: true, grounded: false, pitch: 0.0 },
    ));
    sfx.play_on("blackhawk_engine_high", entity);
    info!("headquarters: the Black Hawk flies its route");
    if let Some((roads, spawners)) = traffic_from(&ents) {
        info!("headquarters: {} road vehicles on {} roads", spawners.len(), roads.len());
        commands.insert_resource(Traffic { roads, spawners, next_at: time.elapsed_secs() + 1.0 });
    }
    spawn_workers(&mut commands, &ents, &mut content, (&mut meshes, &mut materials, &mut images, &mut bindposes), &mut camo_materials, &mut camos);
    spawn_runners(&mut commands, &ents, &mut content, (&mut meshes, &mut materials, &mut images, &mut bindposes), &mut camo_materials, &mut camos, time.elapsed_secs());
}

/// The people at work ([`WORKERS`]): at their spawners (or the spot the
/// script moves them to), facing as they do, cycling their idles.
fn spawn_workers(
    commands: &mut Commands,
    ents: &[iw3::ents::Entity],
    content: &mut crate::content::Content,
    (meshes, materials, images, bindposes): (&mut Assets<Mesh>, &mut Assets<StandardMaterial>, &mut Assets<Image>, &mut Assets<SkinnedMeshInverseBindposes>),
    camo_materials: &mut Assets<crate::gunmodel::CamoMaterial>,
    camos: &mut crate::gunmodel::WorldCamos,
) {
    let noteworthy = |name: &str| ents.iter().find(|e| e.get("script_noteworthy") == Some(name));
    let mut count = 0;
    for w in &WORKERS {
        let Some(spawner) = noteworthy(w.who) else { continue };
        let place = w.stand_at.and_then(noteworthy).unwrap_or(spawner);
        let Some(origin) = place.origin() else { continue };
        let yaw = place.angles()[1];
        let models: Vec<_> = w.models.iter().filter_map(|m| content.model(m, meshes, materials, images, bindposes)).collect();
        if models.is_empty() {
            continue;
        }
        let owner = commands
            .spawn((
                super::HqFigure,
                Name::new(format!("hq worker {}", w.who)),
                Transform::from_translation(crate::units::pos(origin)).with_rotation(Quat::from_rotation_y(yaw.to_radians())),
                Visibility::default(),
            ))
            .id();
        let mut skeleton = Skeleton::default();
        for m in &models {
            spawn_model(commands, &mut skeleton, SpawnModel { model: m, owner, attach_to: None, layers: None, shadows: true });
        }
        if let Some(gun) = w.gun {
            let hand = skeleton.joint("tag_weapon_right");
            let mut assets = crate::gunmodel::GunAssets { meshes, materials, images, bindposes, camo_materials };
            let target = crate::gunmodel::GunTarget { owner, attach_to: hand, layers: None };
            crate::gunmodel::spawn_held_gun(commands, content, &mut camos.0, &mut assets, &mut skeleton, gun, 0, target);
        }
        let anims: Vec<_> = w.idles.iter().filter_map(|a| content.anim(a)).collect();
        let mut player = AnimPlayer::default();
        if let Some(first) = anims.first() {
            player.play(first.clone(), 0.0);
            player.time = count as f32 * 1.3;
        }
        commands.entity(owner).insert((skeleton, player, Idles { anims, next_at: 0.0 }));
        count += 1;
    }
    info!("headquarters: {count} people at work");
}

/// Along their routes: at speed, easing into stops and waiting there.
fn drive(
    mut commands: Commands,
    time: Res<Time>,
    spatial: avian3d::prelude::SpatialQuery,
    mut vehicles: Query<(Entity, &mut Vehicle, &mut Transform)>,
) {
    let _t = crate::perf::Probe::start("hq drive");
    let now = time.elapsed_secs();
    let dt = time.delta_secs().min(0.1);
    for (entity, mut v, mut tf) in &mut vehicles {
        if now < v.wait_until || dt <= 0.0 {
            continue;
        }
        let n = v.route.len();
        let looped = v.looped;
        // A road's end: gone.
        if !looped && v.seg + 1 >= n {
            commands.entity(entity).despawn();
            continue;
        }
        let next = (v.seg + 1) % n;
        // The segment's length, along the curve (a few chords).
        let len = (0..4)
            .map(|k| point(&v.route, v.seg, k as f32 / 4.0, looped).distance(point(&v.route, v.seg, (k + 1) as f32 / 4.0, looped)))
            .sum::<f32>()
            .max(1e-3);
        // Slow into a stop (v^2 = 2 a d), else make for the cruise speed.
        let left = (1.0 - v.along) * len;
        let target = if v.route[next].stop { v.cruise.min((2.0 * u(ACCEL) * left).sqrt().max(u(40.0))) } else { v.cruise };
        let step = u(ACCEL) * dt;
        v.speed += (target - v.speed).clamp(-step, step);
        v.along += v.speed * dt / len;
        if v.along >= 1.0 {
            v.along = 0.0;
            v.seg = next;
            if let Some(s) = v.route[next].speed {
                v.cruise = u(s);
            }
            if v.route[next].stop {
                v.speed = 0.0;
                v.wait_until = now + STOP_WAIT;
            }
        }
        if !looped && v.seg + 1 >= n {
            continue;
        }
        let at = point(&v.route, v.seg, v.along, looped);
        let ahead = point(&v.route, v.seg, (v.along + 0.02).min(1.0), looped);
        let dir = (ahead - at).with_y(0.0);
        // Facing where it's going; banking by how fast it turns.
        let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        if dir.length_squared() > 1e-6 {
            let want = (-dir.z).atan2(dir.x);
            let turn = wrap(want - v.yaw);
            let k = 1.0 - (-2.5 * dt).exp();
            v.yaw = wrap(v.yaw + turn * k);
            // (A turn to the left dips the left side: negative about the nose.)
            let bank = if v.banks { (-turn * 1.2).clamp(-0.45, 0.45) } else { 0.0 };
            v.bank += (bank - v.bank) * (1.0 - (-3.0 * dt).exp());
        }
        // Nose down with speed.
        let mut pitch = if v.banks { -0.12 * (v.speed / v.cruise.max(1e-3)).min(1.0) } else { 0.0 };
        let mut at = at;
        // On a road: wheels on its surface, front and back (the nodes are
        // set above the road, and the curve between them over a rise
        // leaves the ground), pitched with the slope, eased so bumps in the
        // collision don't shake it. Without a surface under it, as far
        // under the nodes as the level stands its vehicles.
        if !v.banks {
            let fwd = Vec3::new(v.yaw.cos(), 0.0, -v.yaw.sin());
            let ground = |p: Vec3| {
                let from = p + Vec3::Y * u(ROAD_PROBE_UP);
                spatial
                    .cast_ray(from, Dir3::NEG_Y, u(ROAD_PROBE_UP + ROAD_PROBE_DOWN), true, &crate::collision::movement_filter())
                    .map(|h| from.y - h.distance)
            };
            let half = u(ROAD_WHEELBASE * 0.5);
            let (front, back) = (ground(at + fwd * half), ground(at - fwd * half));
            let target = match (front, back) {
                (Some(f), Some(b)) => {
                    pitch = ((f - b) / (2.0 * half)).atan();
                    (f + b) * 0.5
                }
                (Some(y), None) | (None, Some(y)) => y,
                (None, None) => at.y - u(ROAD_NODE_HEIGHT),
            };
            let k = 1.0 - (-10.0 * dt).exp();
            at.y = if v.grounded { tf.translation.y + (target - tf.translation.y) * k } else { target };
            v.grounded = true;
            pitch = v.pitch + (pitch - v.pitch) * k;
            v.pitch = pitch;
        }
        tf.translation = at;
        tf.rotation = Quat::from_rotation_y(v.yaw) * Quat::from_rotation_x(v.bank) * Quat::from_rotation_z(pitch);
    }
}

// --- the perimeter roads' traffic

/// Road vehicles: how far up and down to look for the road, their wheels'
/// spread, and how far the level's road nodes are over the road (its
/// vehicles stand at 240, the nodes at 261).
const ROAD_PROBE_UP: f32 = 120.0;
const ROAD_PROBE_DOWN: f32 = 400.0;
const ROAD_WHEELBASE: f32 = 110.0;
const ROAD_NODE_HEIGHT: f32 = 21.0;

/// `ambient_trucks`: a group sets off every 3 to 6 seconds, at 30 to 50
/// miles an hour; at most this many vehicles on the roads at once (each a
/// few draws, far off).
const TRAFFIC_EVERY: (f32, f32) = (3.0, 6.0);
const TRAFFIC_MPH: (f32, f32) = (30.0, 50.0);
const TRAFFIC_MAX: usize = 6;

/// The roads (each a route from its start), and the vehicles waiting at each
/// road's start by group (`script_vehiclespawngroup` 0..7): their model and
/// where they stand.
#[derive(Resource)]
struct Traffic {
    roads: HashMap<String, Vec<Node>>,
    spawners: Vec<(u32, String, String, Vec3, f32)>,
    next_at: f32,
}

#[derive(Component)]
struct OnRoad;

/// The level's road vehicles (all but the Black Hawk), from its entities.
pub(super) fn traffic_from(ents: &[iw3::ents::Entity]) -> Option<(HashMap<String, Vec<Node>>, Vec<(u32, String, String, Vec3, f32)>)> {
    let mut roads = HashMap::new();
    let mut spawners = Vec::new();
    for e in ents.iter().filter(|e| e.classname() == "script_vehicle" && e.get("vehicletype") != Some("blackhawk")) {
        let (Some(model), Some(origin), Some(target), Some(group)) =
            (e.get("model"), e.origin(), e.get("target"), e.get("script_vehiclespawngroup").and_then(|g| g.trim().parse::<u32>().ok()))
        else {
            continue;
        };
        if !roads.contains_key(target) {
            let r = route(ents, target);
            if r.len() >= 2 {
                roads.insert(target.to_owned(), r);
            }
        }
        spawners.push((group, model.to_owned(), target.to_owned(), crate::units::pos(origin), e.angles()[1]));
    }
    (!spawners.is_empty()).then_some((roads, spawners))
}

/// Every few seconds a group sets off: each of its vehicles onto the road
/// it stands at, from the road's start.
#[allow(clippy::too_many_arguments)]
fn traffic(
    mut commands: Commands,
    time: Res<Time>,
    traffic: Option<ResMut<Traffic>>,
    content: Option<ResMut<crate::content::Content>>,
    on_road: Query<(), With<OnRoad>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let _t = crate::perf::Probe::start("hq traffic");
    let now = time.elapsed_secs();
    let (Some(mut t), Some(mut content)) = (traffic, content) else { return };
    if now < t.next_at {
        return;
    }
    t.next_at = now + rand::random_range(TRAFFIC_EVERY.0..TRAFFIC_EVERY.1);
    let mut room = TRAFFIC_MAX.saturating_sub(on_road.iter().count());
    let group = rand::random_range(0..8u32);
    let starting: Vec<(String, String)> = t.spawners.iter().filter(|s| s.0 == group).map(|s| (s.1.clone(), s.2.clone())).collect();
    for (model, road) in starting {
        if room == 0 {
            break;
        }
        let Some(nodes) = t.roads.get(&road).cloned() else { continue };
        let Some(prepared) = content.model(&model, &mut meshes, &mut materials, &mut images, &mut bindposes) else { continue };
        let start = nodes[0].at;
        let towards = (nodes[1].at - start).with_y(0.0).normalize_or(Vec3::X);
        let yaw = (-towards.z).atan2(towards.x);
        let mph = rand::random_range(TRAFFIC_MPH.0..TRAFFIC_MPH.1);
        let entity = commands
            .spawn((OnRoad, Name::new(format!("hq traffic {model}")), Transform::from_translation(start), Visibility::default()))
            .id();
        let mut skeleton = Skeleton::default();
        spawn_model(&mut commands, &mut skeleton, SpawnModel { model: &prepared, owner: entity, attach_to: None, layers: None, shadows: true });
        let cruise = u(mph * MPH);
        // The roads' own speeds are the level's defaults: the script sets
        // its own (`setspeed`).
        let nodes = nodes.into_iter().map(|n| Node { speed: None, ..n }).collect();
        commands.entity(entity).insert((
            skeleton,
            Vehicle { route: nodes, seg: 0, along: 0.0, speed: cruise, cruise, wait_until: now, yaw, bank: 0.0, banks: false, looped: false, grounded: false, pitch: 0.0 },
        ));
        room -= 1;
    }
}

// --- people at work

/// A figure cycling through its idles: each played through once (or a few
/// times, the first, the usual one), then another at random.
#[derive(Component)]
struct Idles {
    anims: Vec<std::sync::Arc<iw3::xanim::XAnim>>,
    next_at: f32,
}

fn idle(time: Res<Time>, mut figures: Query<(&mut Idles, &mut AnimPlayer)>) {
    let _t = crate::perf::Probe::start("hq idle");
    let now = time.elapsed_secs();
    for (mut idles, mut player) in &mut figures {
        if now < idles.next_at || idles.anims.is_empty() {
            continue;
        }
        // Mostly the first; now and then another.
        let pick = if idles.anims.len() > 1 && rand::random_bool(0.35) { rand::random_range(1..idles.anims.len()) } else { 0 };
        let anim = idles.anims[pick].clone();
        let loops = if pick == 0 { rand::random_range(2..4) as f32 } else { 1.0 };
        idles.next_at = now + anim.duration().max(0.5) * loops;
        player.play(anim, 0.3);
    }
}

/// The level's people at work, as its script poses them.
pub(super) struct Worker {
    /// Their spawner (`script_noteworthy`), or where to stand them instead
    /// (another entity's `script_noteworthy`).
    pub who: &'static str,
    pub stand_at: Option<&'static str>,
    pub models: &'static [&'static str],
    pub idles: &'static [&'static str],
    pub gun: Option<&'static str>,
}

pub(super) const WORKERS: [Worker; 4] = [
    // `chair_guy_setup`: at the laptop, gun put away.
    Worker {
        who: "chair_guy",
        stand_at: Some("chair_guy_origin"),
        models: &["body_sp_sas_woodland_assault_a", "head_sp_sas_woodland_peter"],
        idles: &["killhouse_laptop_idle", "killhouse_laptop_twitch", "killhouse_laptop_lookup"],
        gun: None,
    },
    Worker { who: "waters", stand_at: None, models: &["body_complete_sp_sas_woodland_gaz"], idles: &["killhouse_gaz_idleA", "killhouse_gaz_idleB"], gun: Some("m4:gl") },
    Worker {
        who: "nwc",
        stand_at: None,
        models: &["body_sp_sas_woodland_assault_a", "head_sp_sas_woodland_todd"],
        idles: &["killhouse_sas_2_idle"],
        gun: Some("mp5:"),
    },
    Worker {
        who: "mac",
        stand_at: None,
        models: &["body_sp_sas_woodland_support_a", "head_sp_sas_woodland_mac"],
        idles: &["killhouse_sas_3_idle"],
        gun: Some("winchester1200:"),
    },
];

// --- Mac's recruits on the obstacle course

/// How they move between their lane's nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gait {
    Jog,
    Sprint,
    Crawl,
    /// Over an obstacle: a jump's arc.
    Hop,
}

impl Gait {
    /// Units a second, and the animation.
    fn speed(self) -> f32 {
        match self {
            Gait::Jog => 190.0,
            Gait::Sprint => 250.0,
            Gait::Crawl => 45.0,
            Gait::Hop => 150.0,
        }
    }

    fn anim(self) -> &'static str {
        match self {
            Gait::Jog => "combat_jog",
            Gait::Sprint => "pb_sprint",
            Gait::Crawl => "pb_prone_crawl",
            Gait::Hop => "pb_runjump_takeoff",
        }
    }
}

/// A hop's height over the straight line (CoD units).
const HOP_HEIGHT: f32 = 30.0;
/// How long they wait at the line between runs.
const LINE_WAIT: (f32, f32) = (4.0, 8.0);

/// A recruit: their lane (each leg's end, and the gait to it), where on it.
#[derive(Component)]
struct Runner {
    legs: Vec<(Vec3, Gait)>,
    leg: usize,
    /// From the leg's start.
    from: Vec3,
    along: f32,
    wait_until: f32,
    yaw: f32,
    anims: HashMap<&'static str, std::sync::Arc<iw3::xanim::XAnim>>,
    idle: Option<std::sync::Arc<iw3::xanim::XAnim>>,
    /// Moving now (units a second): eased towards the gait's own.
    speed: f32,
    /// A transition playing (getting down, up, landing) and until when:
    /// moving at this share of the gait's speed meanwhile.
    transition: Option<(f32, f32)>,
}

/// A lane: its nodes from `obstacle_lane_node<N>` along their `target`s,
/// the obstacles' negotiation spans put in as hops, each leg's gait from
/// the node it starts at (`prone` crawls to the next `stand`, `sprint`
/// sprints), and back to the start along the line.
fn lane(ents: &[iw3::ents::Entity], n: usize) -> Option<Vec<(Vec3, Gait)>> {
    let by_name: HashMap<&str, &iw3::ents::Entity> = ents.iter().filter_map(|e| Some((e.get("targetname")?, e))).collect();
    let mut nodes: Vec<([f32; 3], Option<String>)> = Vec::new();
    let mut next = Some(format!("obstacle_lane_node{n}"));
    while let Some(name) = next.take() {
        let Some(e) = by_name.get(name.as_str()) else { break };
        if nodes.len() > 32 {
            break;
        }
        nodes.push((e.origin()?, e.get("script_noteworthy").map(str::to_owned)));
        next = e.get("target").map(str::to_owned);
    }
    if nodes.len() < 3 {
        return None;
    }
    // The obstacles on this lane: negotiation pairs within a few feet of
    // its first legs' line.
    let spans: Vec<([f32; 3], [f32; 3])> = ents
        .iter()
        .filter(|e| e.classname() == "node_negotiation_begin")
        .filter_map(|b| Some((b.origin()?, by_name.get(b.get("target")?)?.origin()?)))
        .collect();
    let mut legs = Vec::new();
    let mut gait = Gait::Jog;
    for w in nodes.windows(2) {
        let ((a, kind), (b, _)) = (&w[0], &w[1]);
        match kind.as_deref() {
            Some("prone") => gait = Gait::Crawl,
            Some("sprint") => gait = Gait::Sprint,
            Some("stand") => gait = Gait::Jog,
            _ => {}
        }
        // Obstacles on this leg, in order along it.
        let (pa, pb) = (Vec2::new(a[0], a[1]), Vec2::new(b[0], b[1]));
        let dir = (pb - pa).normalize_or_zero();
        let len = pa.distance(pb);
        let mut on: Vec<(f32, [f32; 3], [f32; 3])> = spans
            .iter()
            .filter_map(|&(s, e)| {
                let t = (Vec2::new(s[0], s[1]) - pa).dot(dir);
                let off = (Vec2::new(s[0], s[1]) - pa - dir * t).length();
                (off < 40.0 && t > 0.0 && t < len).then_some((t, s, e))
            })
            .collect();
        on.sort_by(|x, y| x.0.total_cmp(&y.0));
        for (_, s, e) in on {
            legs.push((crate::units::pos(s), gait));
            legs.push((crate::units::pos(e), Gait::Hop));
        }
        legs.push((crate::units::pos(*b), gait));
    }
    // Back to the start along the line.
    legs.push((crate::units::pos(nodes[0].0), Gait::Jog));
    Some(legs)
}

/// The three recruits (`buddy` spawners, `buddy1`..`buddy3`), at their lanes'
/// start, a moment apart.
#[allow(clippy::too_many_arguments)]
fn spawn_runners(
    commands: &mut Commands,
    ents: &[iw3::ents::Entity],
    content: &mut crate::content::Content,
    (meshes, materials, images, bindposes): (&mut Assets<Mesh>, &mut Assets<StandardMaterial>, &mut Assets<Image>, &mut Assets<SkinnedMeshInverseBindposes>),
    camo_materials: &mut Assets<crate::gunmodel::CamoMaterial>,
    camos: &mut crate::gunmodel::WorldCamos,
    now: f32,
) {
    const HEADS: [&str; 3] = ["head_sp_sas_woodland_hugh", "head_sp_sas_woodland_zied", "head_sp_sas_woodland_peter"];
    let mut count = 0;
    for n in 1..=3 {
        let Some(legs) = lane(ents, n) else { continue };
        let models: Vec<_> = ["body_sp_sas_woodland_assault_a", HEADS[n - 1]]
            .iter()
            .filter_map(|m| content.model(m, meshes, materials, images, bindposes))
            .collect();
        if models.is_empty() {
            continue;
        }
        let start = *legs.last().map(|(at, _)| at).unwrap_or(&Vec3::ZERO);
        let owner = commands
            .spawn((super::HqFigure, Name::new(format!("hq recruit {n}")), Transform::from_translation(start), Visibility::default()))
            .id();
        let mut skeleton = Skeleton::default();
        for m in &models {
            spawn_model(commands, &mut skeleton, SpawnModel { model: m, owner, attach_to: None, layers: None, shadows: true });
        }
        let hand = skeleton.joint("tag_weapon_right");
        let mut assets = crate::gunmodel::GunAssets { meshes, materials, images, bindposes, camo_materials };
        let target = crate::gunmodel::GunTarget { owner, attach_to: hand, layers: None };
        crate::gunmodel::spawn_held_gun(commands, content, &mut camos.0, &mut assets, &mut skeleton, "m4:", 0, target);
        let mut anims = HashMap::new();
        for name in [Gait::Jog.anim(), Gait::Sprint.anim(), Gait::Crawl.anim(), Gait::Hop.anim(), DOWN, UP, LAND] {
            if let Some(a) = content.anim(name) {
                anims.insert(name, a);
            }
        }
        let idle = content.anim("killhouse_sas_1_idle");
        let mut player = AnimPlayer::default();
        if let Some(a) = &idle {
            player.play(a.clone(), 0.0);
        }
        let first = legs[0].0;
        let yaw = {
            let d = (first - start).with_y(0.0);
            (-d.z).atan2(d.x)
        };
        commands.entity(owner).insert((
            skeleton,
            player,
            Runner { legs, leg: 0, from: start, along: 0.0, wait_until: now + 2.0 + n as f32 * 1.5, yaw, anims, idle, speed: 0.0, transition: None },
        ));
        count += 1;
    }
    info!("headquarters: {count} recruits on the obstacle course");
}

/// Getting down to crawl, up from it, and landing a jump.
const DOWN: &str = "pb_crouch2prone";
const UP: &str = "pb_prone2crouchrun";
const LAND: &str = "pb_runjump_land";
/// How fast they get going and slow down (units a second, squared).
const RUNNER_ACCEL: f32 = 500.0;

/// An animation's own pace over the ground (units a second), if it moves.
fn own_speed(anim: &iw3::xanim::XAnim) -> Option<f32> {
    let d = anim.delta_at(1.0);
    let own = Vec2::new(d[0], d[1]).length() / anim.duration().max(1e-3);
    (anim.delta && own > 5.0).then_some(own)
}

/// Down the lanes and back, at each gait's own pace (so the feet keep to
/// the ground), easing in and out of it; down onto their bellies and up
/// again around the crawl, a take-off and landing over each obstacle.
fn run_lanes(
    time: Res<Time>,
    spatial: avian3d::prelude::SpatialQuery,
    mut runners: Query<(&mut Runner, &mut Transform, &mut AnimPlayer)>,
) {
    let _t = crate::perf::Probe::start("hq run_lanes");
    let now = time.elapsed_secs();
    let dt = time.delta_secs().min(0.1);
    let filter = crate::collision::movement_filter();
    let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    for (mut r, mut tf, mut player) in &mut runners {
        if now < r.wait_until {
            if let Some(idle) = r.idle.clone() {
                player.ensure(idle, 0.4);
                player.speed = 1.0;
            }
            r.speed = 0.0;
            continue;
        }
        let (to, gait) = r.legs[r.leg];
        let anim = r.anims.get(gait.anim()).cloned();
        // The gait's own pace: its animation's, so the feet don't slide.
        let pace = anim.as_deref().and_then(own_speed).map_or(u(gait.speed()), u).min(u(gait.speed()) * 1.4);
        let share = r.transition.filter(|t| now < t.0).map_or(1.0, |t| t.1);
        let target = pace * share;
        let step = u(RUNNER_ACCEL) * dt;
        r.speed += (target - r.speed).clamp(-step, step);
        let flat = (to - r.from).with_y(0.0);
        let len = flat.length().max(1e-3);
        r.along += r.speed * dt / len;
        if r.along >= 1.0 {
            let from_gait = gait;
            r.from = to;
            r.along = 0.0;
            r.leg += 1;
            if r.leg >= r.legs.len() {
                // Back at the line: a breather, then again.
                r.leg = 0;
                r.wait_until = now + rand::random_range(LINE_WAIT.0..LINE_WAIT.1);
                r.transition = None;
                continue;
            }
            let next = r.legs[r.leg].1;
            // Down onto the belly (in place), up from it (moving off slowly),
            // landing a jump (keeping on).
            let t = match (from_gait, next) {
                (g, Gait::Crawl) if g != Gait::Crawl => Some((DOWN, 0.0)),
                (Gait::Crawl, g) if g != Gait::Crawl => Some((UP, 0.4)),
                (Gait::Hop, g) if g != Gait::Hop => Some((LAND, 0.8)),
                _ => None,
            };
            if let Some((name, share)) = t {
                if let Some(a) = r.anims.get(name).cloned() {
                    r.transition = Some((now + a.duration() * 0.85, share));
                    player.play(a, 0.12);
                    player.speed = 1.0;
                }
            }
            continue;
        }
        // Along the leg; on the ground (or arcing over an obstacle).
        let mut at = r.from.lerp(to, r.along);
        if gait == Gait::Hop {
            at.y += u(HOP_HEIGHT) * (std::f32::consts::PI * r.along).sin();
        } else {
            let above = at + Vec3::Y * u(60.0);
            if let Some(hit) = spatial.cast_ray(above, Dir3::NEG_Y, u(160.0), true, &filter) {
                // Eased, so steps and lips don't jolt.
                let ground = above.y - hit.distance;
                at.y = if r.speed > 0.0 { tf.translation.y + (ground - tf.translation.y) * (1.0 - (-14.0 * dt).exp()) } else { ground };
            }
        }
        tf.translation = at;
        // Facing the way they go (CoD models face +X), turning smoothly.
        if flat.length_squared() > 1e-6 {
            let want = (-flat.z).atan2(flat.x);
            r.yaw = wrap(r.yaw + wrap(want - r.yaw) * (1.0 - (-6.0 * dt).exp()));
        }
        tf.rotation = Quat::from_rotation_y(r.yaw);
        // A transition plays through; then the gait, as fast as they go.
        if r.transition.is_some_and(|t| now < t.0) {
            continue;
        }
        r.transition = None;
        if let Some(anim) = anim {
            let fade = if gait == Gait::Hop { 0.08 } else { 0.25 };
            player.ensure(anim.clone(), fade);
            player.speed = match (gait, own_speed(&anim)) {
                (Gait::Hop, _) => 1.0,
                (_, Some(own)) => (r.speed / u(own)).clamp(0.3, 1.5),
                _ => 1.0,
            };
        }
    }
}
