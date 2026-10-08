//! Ragdolls and corpses (`COD4RW_RAGDOLL=0` turns them off).
//!
//! CoD4 MP plays the death animation, then (`delayStartRagdoll`) goes limp
//! 0.2 s plus 35% of the way into it, or at its `start_ragdoll` notetrack;
//! at once when it died mantling. Its bodies stay as corpses
//! (`clonePlayer`, 8 of them) after the player respawns.
//!
//! avian's solver is off in this game, so the ragdoll is our own: fifteen
//! points at the main joints, held at their distances (and a few kept from
//! folding up), falling under gravity and stopped by the world with rays,
//! then the skeleton's bones turned to follow them. It sleeps once still
//! and after `ragdoll_max_life` (4.5 s). A bullet's push and a blast's
//! throw start it moving. Its gun has dropped ([`crate::pickups`]).

use crate::combat::{Dead, HitLocation, Killed};
use crate::models::{AnimPlayer, Skeleton};
use crate::thirdperson::Body;
use crate::units::u;
use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use std::collections::HashMap;

/// CoD4's `ragdoll_enable` (Options > Game, "Ragdolls"): on unless set to 0.
pub const RAGDOLL_DVAR: &str = "ragdoll_enable";

static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The setting (from the options).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// On unless the setting or `COD4RW_RAGDOLL=0` says off (its test turns
/// it on regardless). Bodies already limp stay so.
pub fn enabled() -> bool {
    if std::env::var_os("COD4RW_RAGDOLLTEST").is_some() {
        return true;
    }
    match std::env::var("COD4RW_RAGDOLL") {
        Ok(v) => v != "0",
        Err(_) => ENABLED.load(std::sync::atomic::Ordering::Relaxed),
    }
}

pub struct RagdollPlugin;

impl Plugin for RagdollPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Recent>()
            .add_systems(Update, note.run_if(crate::state::in_game))
            .add_systems(Update, schedule.after(note).after(crate::thirdperson::BodyAnimSet).run_if(crate::state::in_game).run_if(enabled))
            .add_systems(Update, test.run_if(crate::state::in_game).run_if(|| std::env::var_os("COD4RW_RAGDOLLTEST").is_some()))
            .add_systems(
                PostUpdate,
                (leave_corpses, step)
                    .chain()
                    .after(crate::thirdperson::BodyPoseSet)
                    .before(bevy::transform::TransformSystems::Propagate)
                    .run_if(crate::state::in_game),
            );
    }
}

/// The points: a joint each and its radius (CoD units).
const POINTS: [(&str, f32); 15] = [
    ("j_mainroot", 7.0),
    ("j_spine4", 7.0),
    ("j_head", 5.0),
    ("j_shoulder_le", 4.0),
    ("j_elbow_le", 3.0),
    ("j_wrist_le", 3.0),
    ("j_shoulder_ri", 4.0),
    ("j_elbow_ri", 3.0),
    ("j_wrist_ri", 3.0),
    ("j_hip_le", 5.0),
    ("j_knee_le", 3.5),
    ("j_ankle_le", 3.0),
    ("j_hip_ri", 5.0),
    ("j_knee_ri", 3.5),
    ("j_ankle_ri", 3.0),
];
const ROOT: usize = 0;
const SPINE: usize = 1;
const HEAD: usize = 2;
const SHOULDERS: (usize, usize) = (3, 6);
const HIPS: (usize, usize) = (9, 12);

/// Held at their starting distances: the bones, and braces that keep the
/// torso and hips one piece.
const LINKS: [(usize, usize); 22] = [
    (0, 1),
    (1, 2),
    (1, 3),
    (1, 6),
    (3, 6),
    (0, 3),
    (0, 6),
    (2, 3),
    (2, 6),
    (3, 4),
    (4, 5),
    (6, 7),
    (7, 8),
    (0, 9),
    (0, 12),
    (9, 12),
    (1, 9),
    (1, 12),
    (9, 10),
    (10, 11),
    (12, 13),
    (13, 14),
];
/// Kept at least this share of the limb's length apart (knees and elbows
/// don't fold flat; the neck doesn't fold the head into the chest).
const SPREADS: [(usize, usize, usize, f32); 4] = [(9, 10, 11, 0.55), (12, 13, 14, 0.55), (3, 4, 5, 0.4), (6, 7, 8, 0.4)];

/// The bones turned to follow the points: by the hips' frame, the chest's,
/// or a segment's direction (from point, to point).
#[derive(Clone, Copy)]
enum Drive {
    Hips,
    Chest,
    Segment(usize, usize),
}

const DRIVES: [(&str, Drive); 16] = [
    ("j_mainroot", Drive::Hips),
    ("pelvis", Drive::Hips),
    ("torso_stabilizer", Drive::Chest),
    ("j_spinelower", Drive::Chest),
    ("j_spineupper", Drive::Chest),
    ("j_spine4", Drive::Chest),
    ("j_neck", Drive::Segment(SPINE, HEAD)),
    ("j_head", Drive::Segment(SPINE, HEAD)),
    ("j_shoulder_le", Drive::Segment(3, 4)),
    ("j_elbow_le", Drive::Segment(4, 5)),
    ("j_shoulder_ri", Drive::Segment(6, 7)),
    ("j_elbow_ri", Drive::Segment(7, 8)),
    ("j_hip_le", Drive::Segment(9, 10)),
    ("j_knee_le", Drive::Segment(10, 11)),
    ("j_hip_ri", Drive::Segment(12, 13)),
    ("j_knee_ri", Drive::Segment(13, 14)),
];

/// `ragdoll_fps` is 20; a little finer keeps the feet out of the floor.
const STEP: f32 = 1.0 / 30.0;
const ITERATIONS: usize = 6;
/// `ragdoll_max_life` (s).
const MAX_LIFE: f32 = 4.5;
/// Still this long (moving slower than [`SLEEP_SPEED`] u/s), it sleeps.
const SLEEP_AFTER: f32 = 0.4;
const SLEEP_SPEED: f32 = 4.0;
/// Gravity (u/s²), air drag per step, friction on the ground.
const GRAVITY: f32 = 800.0;
const DAMPING: f32 = 0.995;
const FRICTION: f32 = 0.6;
/// `ragdoll_bullet_force` / `_upbias` and `ragdoll_explode_force` /
/// `_upbias`, as starting speeds (u/s) for the parts hit.
const BULLET_PUSH: f32 = 120.0;
const BULLET_UP: f32 = 0.5;
const BLAST_THROW: f32 = 380.0;
const BLAST_UP: f32 = 0.8;

/// Deaths and blasts of the last moment, for the push a ragdoll starts with.
#[derive(Resource, Default)]
struct Recent {
    deaths: HashMap<Entity, (f32, Option<Entity>, &'static str, HitLocation)>,
    blasts: Vec<(f32, Vec3, f32)>,
}

/// A body about to go limp, and where its points were a frame ago.
#[derive(Component)]
struct Pending {
    at: f32,
    last: Option<(f32, Vec<Vec3>)>,
    push: Vec<Vec3>,
}

#[derive(Component)]
struct Ragdoll {
    pos: Vec<Vec3>,
    prev: Vec<Vec3>,
    radius: Vec<f32>,
    links: Vec<(usize, usize, f32)>,
    spreads: Vec<(usize, usize, f32)>,
    /// Every joint: its parent's index (None: the body itself), its pose's
    /// local transform, its world rotation then, and what drives it.
    order: Vec<usize>,
    parent: Vec<Option<usize>>,
    local: Vec<Transform>,
    rot0: Vec<Quat>,
    drive: Vec<Option<Drive>>,
    root: usize,
    hips0: Quat,
    chest0: Quat,
    seg0: Vec<Vec3>,
    started: f32,
    acc: f32,
    still: f32,
    asleep: bool,
    /// Written since falling asleep (a corpse's pose then stays put).
    settled: bool,
}

/// A body left behind by a pawn that respawned.
#[derive(Component)]
struct Corpse {
    since: f32,
}

fn note(time: Res<Time>, mut recent: ResMut<Recent>, mut killed: MessageReader<Killed>, mut blasts: MessageReader<crate::explosives::Exploded>) {
    let now = time.elapsed_secs();
    for k in killed.read() {
        recent.deaths.insert(k.victim, (now, k.attacker, k.weapon, k.location));
    }
    for b in blasts.read() {
        recent.blasts.push((now, b.at, u(b.radius)));
    }
    recent.blasts.retain(|b| now - b.0 < 1.0);
    recent.deaths.retain(|_, d| now - d.0 < 10.0);
}

/// The dead: when each body goes limp, and the push it starts with.
#[allow(clippy::type_complexity)]
fn schedule(
    mut commands: Commands,
    time: Res<Time>,
    recent: Res<Recent>,
    pawns: Query<(Entity, &Body, &crate::movement::Mover, Has<Dead>)>,
    bodies: Query<(&AnimPlayer, &Skeleton), (Without<Pending>, Without<Ragdoll>)>,
    places: Query<&GlobalTransform>,
) {
    let now = time.elapsed_secs();
    for (pawn, body, mover, dead) in &pawns {
        if !dead {
            continue;
        }
        let Ok((player, skeleton)) = bodies.get(body.0) else { continue };
        if POINTS.iter().any(|(n, _)| skeleton.joint(n).is_none()) {
            continue;
        }
        let death = recent.deaths.get(&pawn);
        let weapon = death.map_or("", |d| d.2);
        let blown = crate::grenades::kill_icon(weapon).is_some() || crate::explosives::is_explosive(weapon) || crate::killstreaks::kill_icon(weapon).is_some();
        // `delayStartRagdoll`: 0.2 s, then the `start_ragdoll` notetrack or
        // 35% of the animation; mantling, at once. Blown up, at once too
        // (so the blast throws it).
        let delay = match &player.anim {
            _ if blown || mover.mantle.is_some() => 0.0,
            Some(a) => 0.2 + a.notifies.iter().find(|n| n.name == "start_ragdoll").map_or(0.35, |n| n.time) * a.duration(),
            None => 0.2,
        };
        // The push: a blast's throw outward and up, or the killing bullet's
        // shove on the part it hit.
        let mut push = vec![Vec3::ZERO; POINTS.len()];
        let at = places.get(pawn).map_or(Vec3::ZERO, |t| t.translation());
        if let Some(&(_, b, r)) = recent.blasts.iter().filter(|b| b.1.distance(at) < b.2 * 1.5 + u(48.0)).min_by(|x, y| x.1.distance(at).total_cmp(&y.1.distance(at))) {
            if blown {
                let falloff = (1.0 - at.distance(b) / r.max(u(1.0))).clamp(0.3, 1.0);
                let out = ((at - b).with_y(0.0).normalize_or_zero() + Vec3::Y * BLAST_UP).normalize_or_zero();
                push.iter_mut().for_each(|p| *p = out * u(BLAST_THROW) * falloff);
            }
        } else if let Some(&(_, Some(attacker), _, location)) = death {
            let from = places.get(attacker).map_or(at, |t| t.translation());
            let dir = ((at - from).with_y(0.0).normalize_or_zero() + Vec3::Y * BULLET_UP).normalize_or_zero();
            let parts: &[usize] = match location {
                HitLocation::Head => &[HEAD],
                HitLocation::Neck => &[HEAD, SPINE],
                HitLocation::Torso => &[SPINE],
                HitLocation::TorsoLower => &[SPINE, ROOT],
                HitLocation::Legs => &[10, 13],
            };
            for &i in parts {
                push[i] = dir * u(BULLET_PUSH);
            }
        }
        commands.entity(body.0).insert((Pending { at: now + delay, last: None, push }, crate::thirdperson::Limp));
    }
}

/// Respawning, a pawn leaves its body behind as a corpse and gets a new one.
#[allow(clippy::type_complexity)]
fn leave_corpses(
    mut commands: Commands,
    time: Res<Time>,
    pawns: Query<(Entity, &Body), Without<Dead>>,
    places: Query<&GlobalTransform>,
    owners: Query<(&Transform, Option<&crate::wardrobe::LocalBody>, Has<Ragdoll>, Has<Pending>)>,
    corpses: Query<(Entity, &Corpse)>,
) {
    let now = time.elapsed_secs();
    let kept = corpses.iter().count();
    let mut left = 0;
    for (pawn, body) in &pawns {
        let Ok((tf, local, ragdoll, pending)) = owners.get(body.0) else { continue };
        if !ragdoll && !pending {
            continue;
        }
        // Where it lay: the pawn as it was (it has moved to its spawn since).
        let world = places.get(pawn).map_or(*tf, |p| p.mul_transform(*tf).compute_transform());
        let mut e = commands.entity(body.0);
        e.remove::<ChildOf>().insert((world, Corpse { since: now }));
        if pending {
            // Not limp yet: now.
            e.insert(Pending { at: now, last: None, push: vec![Vec3::ZERO; POINTS.len()] });
        }
        // The player's own body was drawn for its shadow (or its slot):
        // everyone sees a corpse.
        if let Some(l) = local {
            for &s in &l.0 {
                commands.entity(s).try_insert(RenderLayers::default());
            }
            commands.entity(body.0).remove::<crate::wardrobe::LocalBody>();
        }
        commands.entity(pawn).remove::<Body>();
        left += 1;
    }
    // At most eight, the oldest go.
    let most = crate::settings_apply::corpses();
    if kept + left > most {
        let mut all: Vec<(Entity, f32)> = corpses.iter().map(|(e, c)| (e, c.since)).collect();
        all.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (e, _) in all.into_iter().take(kept + left - most) {
            commands.entity(e).try_despawn();
        }
    }
}

/// Start, step and pose the ragdolls.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    time: Res<Time>,
    spatial: SpatialQuery,
    replay: Res<crate::killcam::ReplayPoses>,
    mut pending: Query<(Entity, &mut Pending, &Skeleton, &Transform, Option<&ChildOf>), Without<Ragdoll>>,
    mut dolls: Query<(Entity, &mut Ragdoll, &Skeleton, &Transform, Option<&ChildOf>, Has<AnimPlayer>)>,
    mut joints: Query<&mut Transform, (Without<Ragdoll>, Without<Pending>)>,
    globals: Query<&GlobalTransform>,
    parents: Query<&ChildOf>,
    mut cost: Local<(f32, f32, u32, usize, usize)>,
) {
    let clock = std::time::Instant::now();
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let filter = crate::collision::movement_filter();
    // The body's world transform now: its pawn's (as last placed) and its own.
    let owner_world = |tf: &Transform, parent: Option<&ChildOf>| -> Transform {
        parent.and_then(|p| globals.get(p.parent()).ok()).map_or(*tf, |g| g.mul_transform(*tf).compute_transform())
    };
    for (e, mut p, skeleton, tf, parent) in &mut pending {
        let points: Vec<Vec3> = POINTS.iter().filter_map(|(n, _)| skeleton.joint(n).and_then(|j| globals.get(j).ok()).map(|g| g.translation())).collect();
        if points.len() != POINTS.len() {
            continue;
        }
        if now < p.at {
            p.last = Some((now, points));
            continue;
        }
        // Moving as the animation had it, and pushed.
        let vel: Vec<Vec3> = match &p.last {
            Some((t, last)) if now - t > 1e-4 => {
                points.iter().zip(last).map(|(a, b)| ((*a - *b) / (now - t)).clamp_length_max(u(400.0))).collect()
            }
            _ => vec![Vec3::ZERO; POINTS.len()],
        };
        let world = owner_world(tf, parent);
        if let Some(doll) = start(skeleton, &points, &vel, &p.push, world, &globals, &parents, &spatial, &filter, now) {
            commands.entity(e).remove::<Pending>().insert((doll, crate::thirdperson::Limp));
        }
    }
    for (e, mut doll, skeleton, tf, parent, animated) in &mut dolls {
        // A killcam replaying it shows the animation as it was.
        if replay.0.contains_key(&e) {
            continue;
        }
        if !doll.asleep {
            doll.acc = (doll.acc + dt).min(STEP * 3.0);
            while doll.acc >= STEP {
                doll.acc -= STEP;
                simulate(&mut doll, &spatial, &filter);
            }
            let fastest = doll.pos.iter().zip(&doll.prev).map(|(a, b)| a.distance(*b) / STEP).fold(0.0, f32::max);
            doll.still = if fastest < u(SLEEP_SPEED) { doll.still + dt } else { 0.0 };
            if doll.still > SLEEP_AFTER || now - doll.started > MAX_LIFE {
                doll.asleep = true;
            }
        }
        // Asleep and no animation overwriting it, the pose stays as written.
        if doll.asleep && doll.settled && !animated {
            continue;
        }
        pose(&doll, skeleton, owner_world(tf, parent), &mut joints);
        doll.settled = doll.asleep;
        // A corpse asleep: no animation to overwrite its pose any more.
        if doll.asleep && animated && parent.is_none() {
            commands.entity(e).remove::<AnimPlayer>();
        }
    }
    // `COD4RW_PERF`: what it costs, every five seconds.
    if std::env::var_os("COD4RW_PERF").is_some() {
        let awake = dolls.iter().filter(|d| !d.1.asleep).count();
        let (spent, since, frames, most_awake, most) = &mut *cost;
        *spent += clock.elapsed().as_secs_f32() * 1000.0;
        *frames += 1;
        *most_awake = (*most_awake).max(awake);
        *most = (*most).max(dolls.iter().count());
        if now - *since > 5.0 {
            info!("ragdoll: {:.3} ms a frame ({} frames), at most {} awake of {}", *spent / *frames as f32, frames, most_awake, most);
            *cost = (0.0, now, 0, 0, 0);
        }
    }
}

/// A frame from an up direction and a side one (x side, y up).
fn frame(up: Vec3, side: Vec3) -> Quat {
    let y = up.normalize_or(Vec3::Y);
    let x = (side - y * side.dot(y)).normalize_or(Vec3::X);
    Quat::from_mat3(&Mat3::from_cols(x, y, x.cross(y)))
}

#[allow(clippy::too_many_arguments)]
fn start(
    skeleton: &Skeleton,
    points: &[Vec3],
    vel: &[Vec3],
    push: &[Vec3],
    owner: Transform,
    globals: &Query<&GlobalTransform>,
    parents: &Query<&ChildOf>,
    spatial: &SpatialQuery,
    filter: &SpatialQueryFilter,
    now: f32,
) -> Option<Ragdoll> {
    let n = skeleton.joints.len();
    let index: HashMap<Entity, usize> = skeleton.joints.iter().enumerate().map(|(i, j)| (j.entity, i)).collect();
    let parent: Vec<Option<usize>> = skeleton.joints.iter().map(|j| parents.get(j.entity).ok().and_then(|c| index.get(&c.parent()).copied())).collect();
    // Parents before children.
    let mut order = Vec::with_capacity(n);
    let mut placed = vec![false; n];
    while order.len() < n {
        let before = order.len();
        for i in 0..n {
            if !placed[i] && parent[i].is_none_or(|p| placed[p]) {
                placed[i] = true;
                order.push(i);
            }
        }
        if order.len() == before {
            return None;
        }
    }
    let world: Vec<Transform> = skeleton.joints.iter().map(|j| globals.get(j.entity).map_or(Transform::IDENTITY, |g| g.compute_transform())).collect();
    let local: Vec<Transform> = (0..n)
        .map(|i| {
            let p = parent[i].map_or(owner, |p| world[p]);
            Transform::from_matrix(p.to_matrix().inverse() * world[i].to_matrix())
        })
        .collect();
    let drive: Vec<Option<Drive>> = skeleton.joints.iter().map(|j| DRIVES.iter().find(|(n, _)| *n == j.name).map(|(_, d)| *d)).collect();
    let root = skeleton.joints.iter().position(|j| j.name == "j_mainroot")?;
    // The points: out of the floor, moving as they were plus the push.
    let radius: Vec<f32> = POINTS.iter().map(|(_, r)| u(*r)).collect();
    let mut pos = points.to_vec();
    for (p, r) in pos.iter_mut().zip(&radius) {
        let from = *p + Vec3::Y * u(24.0);
        if let Some(hit) = spatial.cast_ray(from, Dir3::NEG_Y, u(24.0) + *r, true, filter) {
            p.y = p.y.max(from.y - hit.distance + *r);
        }
    }
    let prev: Vec<Vec3> = pos.iter().zip(vel).zip(push).map(|((p, v), k)| *p - (*v + *k) * STEP).collect();
    let dist = |a: usize, b: usize| points[a].distance(points[b]);
    let links = LINKS.iter().map(|&(a, b)| (a, b, dist(a, b))).collect();
    let spreads = SPREADS.iter().map(|&(a, m, b, f)| (a, b, (dist(a, m) + dist(m, b)) * f)).collect();
    let seg0 = DRIVES
        .iter()
        .map(|(_, d)| match d {
            Drive::Segment(a, b) => (points[*b] - points[*a]).normalize_or_zero(),
            _ => Vec3::ZERO,
        })
        .collect();
    Some(Ragdoll {
        hips0: frame(points[SPINE] - points[ROOT], points[HIPS.0] - points[HIPS.1]),
        chest0: frame(points[SPINE] - points[ROOT], points[SHOULDERS.0] - points[SHOULDERS.1]),
        pos,
        prev,
        radius,
        links,
        spreads,
        order,
        parent,
        rot0: world.iter().map(|w| w.rotation).collect(),
        local,
        drive,
        root,
        seg0,
        started: now,
        acc: 0.0,
        still: 0.0,
        asleep: false,
        settled: false,
    })
}

/// One step: Verlet under gravity, the world in the way, the distances kept.
fn simulate(d: &mut Ragdoll, spatial: &SpatialQuery, filter: &SpatialQueryFilter) {
    let g = Vec3::NEG_Y * u(GRAVITY) * STEP * STEP;
    let before = d.pos.clone();
    for i in 0..d.pos.len() {
        let v = (d.pos[i] - d.prev[i]) * DAMPING;
        d.prev[i] = d.pos[i];
        d.pos[i] += v + g;
    }
    collide(d, &before, spatial, filter, true);
    for _ in 0..ITERATIONS {
        for &(a, b, rest) in &d.links {
            let delta = d.pos[b] - d.pos[a];
            let len = delta.length();
            if len > 1e-5 {
                let fix = delta * (0.5 * (len - rest) / len);
                d.pos[a] += fix;
                d.pos[b] -= fix;
            }
        }
        for &(a, b, min) in &d.spreads {
            let delta = d.pos[b] - d.pos[a];
            let len = delta.length();
            if len < min && len > 1e-5 {
                let fix = delta * (0.5 * (len - min) / len);
                d.pos[a] += fix;
                d.pos[b] -= fix;
            }
        }
        limit_joints(&mut d.pos);
    }
    // The constraints may have pushed points into the world.
    collide(d, &before, spatial, filter, false);
}

/// Knees bend forward and elbows back, and thighs don't swing far behind
/// the hips: a point bent the wrong way goes back onto the line between
/// its neighbours (a little the right side of it).
fn limit_joints(pos: &mut [Vec3]) {
    let up = pos[SPINE] - pos[ROOT];
    // The body's front: left (the left hip from the right) across up.
    let front = |left: Vec3| left.cross(up).normalize_or_zero();
    let hips_front = front(pos[HIPS.0] - pos[HIPS.1]);
    let chest_front = front(pos[SHOULDERS.0] - pos[SHOULDERS.1]);
    // (upper, middle, lower, which way the middle must point, from what)
    let hinges = [(9, 10, 11, 1.0, hips_front), (12, 13, 14, 1.0, hips_front), (3, 4, 5, -1.0, chest_front), (6, 7, 8, -1.0, chest_front)];
    for (a, m, b, sign, f) in hinges {
        if f == Vec3::ZERO {
            continue;
        }
        let line = pos[b] - pos[a];
        let along = (pos[m] - pos[a]).dot(line) / line.length_squared().max(1e-6);
        let on_line = pos[a] + line * along.clamp(0.0, 1.0);
        let out = (pos[m] - on_line).dot(f) * sign;
        let margin = u(0.5);
        if out < margin {
            // Half to the joint, half shared by its ends (they stay put
            // between them).
            let fix = f * sign * (margin - out);
            pos[m] += fix * 0.5;
            pos[a] -= fix * 0.25;
            pos[b] -= fix * 0.25;
        }
    }
    // Thighs: at most about 20 degrees behind the hips.
    for (hip, knee) in [(9, 10), (12, 13)] {
        let thigh = pos[knee] - pos[hip];
        let len = thigh.length();
        let back = -thigh.dot(hips_front);
        let most = len * 0.35;
        if back > most && hips_front != Vec3::ZERO {
            pos[knee] += hips_front * (back - most);
        }
    }
}

/// Stop each point where the world is between where it was and where it's
/// going (a ray, its radius short of the hit), sliding with friction.
fn collide(d: &mut Ragdoll, from: &[Vec3], spatial: &SpatialQuery, filter: &SpatialQueryFilter, friction: bool) {
    for i in 0..d.pos.len() {
        let travel = d.pos[i] - from[i];
        let len = travel.length();
        let Ok(dir) = Dir3::new(travel) else { continue };
        let r = d.radius[i];
        let Some(hit) = spatial.cast_ray(from[i], dir, len + r, true, filter) else { continue };
        if hit.distance <= 0.0 {
            continue;
        }
        let n = hit.normal;
        let at = from[i] + *dir * (hit.distance - r).max(0.0);
        // What's left of the motion, along the surface.
        let rest = d.pos[i] - at;
        d.pos[i] = at + (rest - n * rest.dot(n)).clamp_length_max(r);
        if friction {
            let v = d.pos[i] - d.prev[i];
            let vt = v - n * v.dot(n);
            d.prev[i] = d.pos[i] - vt * (1.0 - FRICTION);
        }
    }
}

/// Turn the skeleton's bones to follow the points; the rest keep their
/// pose under them.
fn pose(d: &Ragdoll, skeleton: &Skeleton, owner: Transform, joints: &mut Query<&mut Transform, (Without<Ragdoll>, Without<Pending>)>) {
    let hips = frame(d.pos[SPINE] - d.pos[ROOT], d.pos[HIPS.0] - d.pos[HIPS.1]) * d.hips0.inverse();
    let chest = frame(d.pos[SPINE] - d.pos[ROOT], d.pos[SHOULDERS.0] - d.pos[SHOULDERS.1]) * d.chest0.inverse();
    let mut world: Vec<Transform> = vec![Transform::IDENTITY; d.local.len()];
    for &i in &d.order {
        let parent = d.parent[i].map_or(owner, |p| world[p]);
        let mut w = Transform::from_matrix(parent.to_matrix() * d.local[i].to_matrix());
        if let Some(drive) = d.drive[i] {
            let turn = match drive {
                Drive::Hips => hips,
                Drive::Chest => chest,
                Drive::Segment(a, b) => {
                    let k = DRIVES.iter().position(|(n, _)| *n == skeleton.joints[i].name).unwrap_or(0);
                    Quat::from_rotation_arc(d.seg0[k], (d.pos[b] - d.pos[a]).normalize_or(d.seg0[k]))
                }
            };
            w.rotation = (turn * d.rot0[i]).normalize();
        }
        if i == d.root {
            w.translation = d.pos[ROOT];
        }
        world[i] = w;
        let local = Transform::from_matrix(parent.to_matrix().inverse() * w.to_matrix());
        if let Ok(mut tf) = joints.get_mut(skeleton.joints[i].entity) {
            *tf = Transform { scale: tf.scale, ..local };
        }
    }
}

/// Debug aid: with `COD4RW_RAGDOLLTEST=<dir>`, at 8 s a bot is put 150
/// units in front of the player (or at `COD4RW_RAGDOLLTEST_AT=x,y,z`, CoD
/// units) and shot dead (`COD4RW_RAGDOLLTEST_BLAST=1`: blown up from in
/// front of it), the view on it; frames over the next five seconds, then
/// exit.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn test(
    mut commands: Commands,
    time: Res<Time>,
    mut recent: ResMut<Recent>,
    mut player: Query<(Entity, &Transform, &crate::movement::Mover, &mut crate::movement::ViewAngles), With<crate::player::LocalPlayer>>,
    mut bots: Query<(Entity, &mut Transform), (With<crate::combat::Pawn>, Without<crate::player::LocalPlayer>)>,
    mut damage: MessageWriter<crate::combat::Damage>,
    spatial: SpatialQuery,
    mut state: Local<(usize, f32, Vec3)>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    const FRAMES: [f32; 12] = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0, 1.3, 1.6, 2.0, 2.5, 3.5, 5.0];
    let Ok(dir) = std::env::var("COD4RW_RAGDOLLTEST") else { return };
    let t = time.elapsed_secs();
    let Ok((me, at, mover, mut view)) = player.single_mut() else { return };
    let eye = mover.eye(at.translation);
    let (step, killed_at, spot) = &mut *state;
    // Keep looking at the spot.
    if *step > 0 {
        let d = *spot + Vec3::Y * u(20.0) - eye;
        view.yaw = (-d.x).atan2(-d.z);
        view.pitch = d.y.atan2(d.with_y(0.0).length());
    }
    if *step == 0 && t >= 8.0 {
        let forward = Quat::from_rotation_y(view.yaw) * Vec3::NEG_Z;
        let wanted = std::env::var("COD4RW_RAGDOLLTEST_AT").unwrap_or_default();
        *spot = if wanted == "stairs" {
            let Some(s) = find_stairs(&spatial, eye) else {
                info!("ragdoll test: no stairs in sight");
                exit.write(AppExit::Success);
                return;
            };
            s
        } else {
            let v: Vec<f32> = wanted.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() == 3 { crate::units::pos([v[0], v[1], v[2]]) } else { at.translation + forward.with_y(0.0).normalize_or_zero() * u(150.0) }
        };
        let Some((victim, mut tf)) = bots.iter_mut().find(|(e, _)| *e != me) else { return };
        tf.translation = *spot;
        let blast = std::env::var_os("COD4RW_RAGDOLLTEST_BLAST").is_some();
        if blast {
            let from = *spot + (eye - *spot).with_y(0.0).normalize_or_zero() * u(60.0);
            recent.blasts.push((t, from, u(256.0)));
        }
        let weapon = if blast { "frag_grenade_mp" } else { "m16_mp" };
        damage.write(crate::combat::Damage { target: victim, attacker: Some(me), amount: 1000.0, location: HitLocation::Torso, weapon });
        info!("ragdoll test: killed {victim:?} at {:?}", crate::units::to_cod(*spot));
        (*step, *killed_at) = (1, t);
        return;
    }
    if *step >= 1 {
        let i = *step - 1;
        if i < FRAMES.len() && t - *killed_at >= FRAMES[i] {
            std::fs::create_dir_all(&dir).ok();
            let path = std::path::Path::new(&dir).join(format!("ragdoll_{i:02}.png"));
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
            *step += 1;
        } else if i >= FRAMES.len() && t - *killed_at > FRAMES[FRAMES.len() - 1] + 0.5 {
            exit.write(AppExit::Success);
        }
    }
}

/// The test's staircase: within 1200 units of `eye` and in sight of it, a
/// run of at least four steps of 5..14 units over 12 units each; the
/// middle of it.
fn find_stairs(spatial: &SpatialQuery, eye: Vec3) -> Option<Vec3> {
    let filter = crate::collision::movement_filter();
    let ground = |p: Vec3| {
        let from = Vec3::new(p.x, eye.y + u(200.0), p.z);
        spatial.cast_ray(from, Dir3::NEG_Y, u(800.0), true, &filter).map(|h| from - Vec3::Y * h.distance)
    };
    let mut best: Option<(f32, Vec3)> = None;
    for gx in -75..=75 {
        for gz in -75..=75 {
            let p = eye + Vec3::new(gx as f32, 0.0, gz as f32) * u(16.0);
            let d = p.with_y(eye.y).distance(eye);
            if d > u(1200.0) || d < u(120.0) || best.is_some_and(|b| b.0 < d) {
                continue;
            }
            for k in 0..8 {
                let dir = Quat::from_rotation_y(k as f32 * std::f32::consts::FRAC_PI_4) * Vec3::X;
                let heights: Vec<f32> = (0..7).filter_map(|i| ground(p + dir * u(12.0) * i as f32).map(|g| g.y)).collect();
                if heights.len() < 7 {
                    continue;
                }
                let steps = heights.windows(2).filter(|w| (u(5.0)..u(14.0)).contains(&(w[1] - w[0]))).count();
                if steps < 4 {
                    continue;
                }
                let Some(mid) = ground(p + dir * u(36.0)) else { continue };
                let target = mid + Vec3::Y * u(30.0);
                let Ok(look) = Dir3::new(target - eye) else { continue };
                if spatial.cast_ray(eye, look, eye.distance(target) - u(8.0), true, &filter).is_none() {
                    best = Some((d, mid + Vec3::Y * u(2.0)));
                }
            }
        }
    }
    best.map(|b| b.1)
}
