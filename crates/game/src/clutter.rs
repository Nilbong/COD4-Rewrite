//! The map's clutter: its dynamic entities (`dynEntDefList`: cinder blocks,
//! cans, bottles, boxes, crates, tyres, pictures, plates), hundreds a map.
//! In CoD4's multiplayer they're the client's alone: bullets knock them
//! about, blasts throw them, the breakable ones (plates, pictures) shatter
//! into their pieces, and players walk through them.
//!
//! Most never move, so each stays a plain static mesh until something
//! disturbs it. Then it gets a [`Moving`] body: a box the size of its model
//! that falls, bounces off and slides along the world (shape casts against
//! the world's colliders; avian's solver isn't running), tips onto its
//! nearest face as it slows and goes back to sleep. The forces are CoD4's
//! (its `dynEnt_*` and `phys_*` settings): a bullet's push is a half-mass
//! bullet at `dynEnt_bulletForce` bouncing off it, aimed a little upwards and
//! off-centre so things spin; a blast pushes everything within its radius
//! (the nearest 20) away and up, weaker towards the edge. Masses, bounce,
//! friction and how hard each kind is pushed come from its `PhysPreset`.

use crate::collision::Layer;
use crate::content::{Content, MAP_ZONE};
use crate::state::{GameState, Setup, in_game};
use crate::units::{self, INCH};
use avian3d::prelude::*;
use bevy::prelude::*;
use iw3::zone::{Asset, AssetId};

pub struct ClutterPlugin;

impl Plugin for ClutterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pieces>()
            .add_systems(OnEnter(GameState::InGame), spawn_clutter.in_set(Setup::Spawn))
            .add_systems(Update, (shots, knives, blasts, step).chain().after(crate::weapons::WeaponSet).run_if(in_game));
        if let Ok(dir) = std::env::var("COD4RW_CLUTTERTEST") {
            app.insert_resource(Test(dir.into())).add_systems(Update, test.before(shots).run_if(in_game));
        }
    }
}

/// CoD4's `dynEnt_bulletForce`: the bullet's speed in its push.
const BULLET_FORCE: f32 = 1000.0;
/// `phys_bulletUpBias` and `phys_bulletSpinScale`: bullets push a little
/// upwards, and as if they hit three times further from the middle.
const BULLET_UP_BIAS: f32 = 0.5;
const BULLET_SPIN_SCALE: f32 = 3.0;
/// The bullet's mass in that push.
const BULLET_MASS: f32 = 0.5;
/// `dynEnt_explodeForce`, `_explodeUpbias`, `_explodeSpinScale` (units off
/// the middle), `_explodeMinForce`, `_explodeMaxEnts`.
const EXPLODE_FORCE: f32 = 12_500.0;
const EXPLODE_UP_BIAS: f32 = 0.5;
const EXPLODE_SPIN_SCALE: f32 = 3.0;
const EXPLODE_MIN_FORCE: f32 = 40.0;
const EXPLODE_MAX: usize = 20;
/// `dynEntPieces_impactForce`: the push a broken thing's pieces get.
const PIECES_FORCE: f32 = 1000.0;
/// At most this many broken pieces about (CoD4 keeps 100).
const MAX_PIECES: usize = 100;
/// `phys_gravity` (units/s²), `phys_dragLinear`, `phys_dragAngular`.
const GRAVITY: f32 = 800.0;
const DRAG_LINEAR: f32 = 0.03;
const DRAG_ANGULAR: f32 = 0.5;
/// Spinning no faster than this (rad/s).
const MAX_SPIN: f32 = 40.0;
/// `phys_minImpactMomentum`: knocks softer than this are silent (mass ×
/// units/s), and a piece sounds no more often than this (s).
const MIN_IMPACT_MOMENTUM: f32 = 250.0;
const SOUND_GAP: f32 = 0.12;
/// Asleep once this slow (m/s, rad/s) for this long (s).
const SLEEP_SPEED: f32 = 0.08;
const SLEEP_SPIN: f32 = 0.3;
const SLEEP_AFTER: f32 = 0.4;
/// Kept off what it rests on by this much (m).
const SKIN: f32 = 0.002;

/// A piece of the map's clutter, by its index in the map's `dynEntDefList`.
#[derive(Component, Clone, Copy, Debug)]
pub struct Clutter(pub usize);

/// How a piece of clutter (or a broken piece) moves and breaks.
#[derive(Component, Clone, Debug)]
pub struct Body {
    /// The box: its middle in the model's space and half its size (Bevy
    /// axes, metres).
    centre: Vec3,
    half: Vec3,
    preset: Preset,
    /// Its impact sounds' prefix (`physics_wood`): `{prefix}_{surface}`.
    sound: String,
    /// Breakable (CoD4's destructible kind): what's left, and what it
    /// breaks into.
    health: Option<i32>,
    destroy_fx: Option<String>,
    destroy_pieces: Vec<Piece>,
}

impl Body {
    /// A loose object (a dead player's gun): a box `half` its size about
    /// `centre` (Bevy, metres), `mass` and its impact sounds' prefix.
    pub fn loose(centre: Vec3, half: Vec3, mass: f32, sound: &str) -> Body {
        Body {
            centre,
            half,
            preset: Preset { mass, ..default() },
            sound: sound.to_owned(),
            health: None,
            destroy_fx: None,
            destroy_pieces: Vec::new(),
        }
    }
}

impl Moving {
    /// Set going at `velocity` (m/s) with `spin` (rad/s).
    pub fn thrown(velocity: Vec3, spin: Vec3) -> Moving {
        Moving { velocity, spin, ..default() }
    }
}

/// A `PhysPreset`'s numbers (CoD units).
#[derive(Clone, Copy, Debug)]
struct Preset {
    mass: f32,
    bounce: f32,
    friction: f32,
    bullet_scale: f32,
    explosive_scale: f32,
    spread: f32,
    upward: f32,
}

impl Default for Preset {
    /// For clutter whose preset is missing: a light box.
    fn default() -> Self {
        Preset { mass: 5.0, bounce: 0.2, friction: 0.6, bullet_scale: 1.0, explosive_scale: 1.0, spread: 0.3, upward: 60.0 }
    }
}

impl From<&iw3::zone::PhysPreset> for Preset {
    fn from(p: &iw3::zone::PhysPreset) -> Self {
        Preset {
            mass: p.mass.max(0.1),
            bounce: p.bounce.clamp(0.0, 1.0),
            friction: p.friction.max(0.0),
            bullet_scale: p.bullet_force_scale.max(0.0),
            explosive_scale: p.explosive_force_scale.max(0.0),
            spread: p.pieces_spread_fraction.clamp(0.0, 1.0),
            upward: p.pieces_upward_velocity,
        }
    }
}

/// One of a breakable's pieces: its model (zone, id) and where it sits on
/// the whole (CoD units, the whole's axes).
#[derive(Clone, Copy, Debug)]
struct Piece {
    zone: usize,
    model: AssetId,
    offset: [f32; 3],
}

/// A piece in motion: velocity (m/s) and spin (rad/s, world axes), and how
/// long it's been nearly still.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Moving {
    velocity: Vec3,
    spin: Vec3,
    still: f32,
    /// On the ground last step, with its normal.
    ground: Option<Vec3>,
    /// When it last made a sound (s).
    sounded: f32,
}

/// A broken piece, oldest first (the oldest goes when there are too many).
#[derive(Resource, Default)]
struct Pieces(std::collections::VecDeque<Entity>);

#[derive(Component)]
struct BrokenPiece;

/// A CoD-space quaternion (x, y, z, w) as the model's forward, left and up
/// axes in CoD space.
pub(crate) fn quat_axes([x, y, z, w]: [f32; 4]) -> [[f32; 3]; 3] {
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + w * z), 2.0 * (x * z - w * y)],
        [2.0 * (x * y - w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + w * x)],
        [2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// A model's box (Bevy axes, metres): middle and half size.
fn model_box(xm: &iw3::zone::XModel) -> (Vec3, Vec3) {
    let (lo, hi) = (units::pos(xm.mins), units::pos(xm.maxs));
    let half = ((hi - lo).abs() / 2.0).max(Vec3::splat(0.01));
    ((lo + hi) / 2.0, half)
}

/// A preset by id, following `,name` references into the other zones, and
/// its sound prefix.
fn preset(content: &Content, zi: usize, id: Option<AssetId>) -> Option<(Preset, String)> {
    let id = id?;
    let zone = content.zone(zi);
    let of = |p: &iw3::zone::PhysPreset| (Preset::from(p), p.sound_prefix.clone());
    match zone.get(id) {
        Asset::PhysPreset(p) if !p.name.starts_with(',') => Some(of(p)),
        a => {
            let name = a.name().trim_start_matches(',');
            content.zones.iter().find_map(|z| match z.find(name).map(|i| z.get(i)) {
                Some(Asset::PhysPreset(p)) if !p.name.starts_with(',') => Some(of(p)),
                _ => None,
            })
        }
    }
}

/// Spawn model `model`'s surfaces under `owner`, as static meshes.
fn spawn_meshes(
    commands: &mut Commands,
    content: &mut Content,
    zi: usize,
    model: AssetId,
    owner: Entity,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) {
    let Some((lod, mats)) = content.zone(zi).xmodel(model).and_then(|xm| Some((xm.lods.first().copied()?, xm.materials.clone()))) else {
        return;
    };
    let first = lod.surf_index as usize;
    for surf in first..first + lod.num_surfs as usize {
        let Some(mat) = mats.get(surf).copied().flatten().and_then(|m| content.material(zi, m, materials, images)) else { continue };
        let Some(mesh) = content.static_mesh(zi, model, surf, meshes) else { continue };
        commands.spawn((Mesh3d(mesh), MeshMaterial3d(mat.handle), ChildOf(owner)));
    }
}

fn spawn_clutter(
    mut commands: Commands,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut pieces: ResMut<Pieces>,
) {
    pieces.0.clear();
    let defs = content.map().clip_map().map(|c| c.dyn_ents.clone()).unwrap_or_default();
    if defs.is_empty() {
        return;
    }
    let root = commands.spawn((Name::new("map clutter"), Transform::default(), Visibility::default())).id();
    let (mut drawn, mut skipped, mut breakable, mut no_preset) = (0, 0, 0, 0);
    for (i, d) in defs.iter().enumerate() {
        // Brush-model pieces (`*n`) aren't drawn yet.
        let Some(model) = d.model else {
            skipped += 1;
            continue;
        };
        let (zi, model) = content.resolve_xmodel(MAP_ZONE, model);
        let Some((name, (centre, half))) = content.zone(zi).xmodel(model).map(|xm| (xm.name.clone(), model_box(xm))) else {
            skipped += 1;
            continue;
        };
        let (preset, sound) = preset(&content, MAP_ZONE, d.phys_preset).unwrap_or_else(|| {
            no_preset += 1;
            (Preset::default(), String::new())
        });
        // CoD4's destructible kind breaks and doesn't move.
        let health = (d.kind == 2 && d.health > 0).then_some(d.health);
        let map = content.zone(MAP_ZONE);
        let destroy_fx = d.destroy_fx.map(|f| map.get(f).name().trim_start_matches(',').to_owned()).filter(|n| !n.is_empty());
        let destroy_pieces = d
            .destroy_pieces
            .and_then(|p| match map.get(p) {
                Asset::XModelPieces(p) => Some(p.pieces.clone()),
                _ => None,
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(m, offset)| {
                let (zone, model) = content.resolve_xmodel(MAP_ZONE, m?);
                Some(Piece { zone, model, offset })
            })
            .collect();
        breakable += health.is_some() as usize;
        let tf = Transform::from_translation(units::pos(d.origin)).with_rotation(units::axis_rotation(quat_axes(d.quat)));
        let body = Body { centre, half, preset, sound, health, destroy_fx, destroy_pieces };
        let owner = commands.spawn((Name::new(format!("clutter {name}")), Clutter(i), body, tf, Visibility::default(), ChildOf(root))).id();
        spawn_meshes(&mut commands, &mut content, zi, model, owner, &mut meshes, &mut materials, &mut images);
        drawn += 1;
    }
    info!(
        "clutter: {drawn} pieces ({breakable} breakable){}{}",
        if skipped > 0 { format!(", {skipped} not drawn") } else { String::new() },
        if no_preset > 0 { format!(", {no_preset} without a physics preset") } else { String::new() }
    );
}

/// The box `body` of a piece at `tf`: its middle in the world.
fn middle(tf: &Transform, body: &Body) -> Vec3 {
    tf.transform_point(body.centre)
}

/// Where a ray (`from`, unit `dir`, up to `max`) first enters a piece's
/// box, if it does.
fn ray_box(from: Vec3, dir: Vec3, max: f32, tf: &Transform, body: &Body) -> Option<f32> {
    let inv = tf.rotation.inverse();
    let o = inv * (from - middle(tf, body));
    let d = inv * dir;
    let (mut near, mut far) = (0.0f32, max);
    for a in 0..3 {
        let (o, d, h) = (o[a], d[a], body.half[a]);
        if d.abs() < 1e-6 {
            if o.abs() > h {
                return None;
            }
            continue;
        }
        let (t1, t2) = ((-h - o) / d, (h - o) / d);
        near = near.max(t1.min(t2));
        far = far.min(t1.max(t2));
        if near > far {
            return None;
        }
    }
    Some(near)
}

/// The box's inertia over its mass, about any axis (roughly: the mean of a
/// box's three).
fn inertia_per_mass(half: Vec3) -> f32 {
    let s = half * 2.0;
    ((s.x * s.x + s.y * s.y + s.z * s.z) / 18.0).max(1e-4)
}

/// Push a piece with momentum `push` (CoD units: mass × units/s) at world
/// point `at`.
fn push(moving: &mut Moving, tf: &Transform, body: &Body, at: Vec3, push: Vec3) {
    let dv = push / body.preset.mass * INCH;
    moving.velocity += dv;
    let r = at - middle(tf, body);
    moving.spin += r.cross(dv) / inertia_per_mass(body.half);
    moving.spin = moving.spin.clamp_length_max(MAX_SPIN);
    moving.still = 0.0;
}

/// A bullet's push (`Phys_ObjBulletImpact`'s momentum: a half-mass bullet
/// at `speed` bouncing off it, aimed a bit up and spun off the middle).
fn bullet_push(moving: &mut Moving, tf: &Transform, body: &Body, hit: Vec3, dir: Vec3, speed: f32) {
    let dir = (dir + Vec3::Y * BULLET_UP_BIAS).normalize_or(dir);
    let relative = speed - (moving.velocity / INCH).dot(dir);
    if relative <= 0.0 {
        return;
    }
    let m = body.preset.mass;
    let momentum = body.preset.bullet_scale * m * relative * 2.0 * BULLET_MASS / (m + BULLET_MASS);
    let c = middle(tf, body);
    push(moving, tf, body, c + (hit - c) * BULLET_SPIN_SCALE, dir * momentum);
}

/// Bullets knock clutter about and break breakables: the first piece along
/// each shot.
#[allow(clippy::type_complexity)]
/// The settings' Clutter Physics: off, clutter stays put.
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

fn shots(
    mut commands: Commands,
    mut shots: MessageReader<crate::weapons::ShotFired>,
    mut clutter: Query<(Entity, &Transform, &mut Body, Option<&mut Moving>), With<Clutter>>,
    mut breaks: Local<Vec<(Entity, Vec3, Vec3)>>,
) {
    breaks.clear();
    if !enabled() {
        shots.clear();
        return;
    }
    for s in shots.read() {
        let Some(weapon) = s.weapon else { continue };
        let along = s.to - s.from;
        let len = along.length();
        let Ok(dir) = Dir3::new(along) else { continue };
        let first = clutter
            .iter()
            .filter(|(_, tf, b, _)| {
                // A sphere around the box first.
                let r = b.half.length();
                let c = middle(tf, b) - s.from;
                let t = c.dot(*dir).clamp(0.0, len);
                (c - *dir * t).length_squared() <= r * r
            })
            .filter_map(|(e, tf, b, _)| ray_box(s.from, *dir, len, tf, b).map(|t| (e, t)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((e, t)) = first else { continue };
        let Ok((_, tf, mut body, moving)) = clutter.get_mut(e) else { continue };
        let hit = s.from + *dir * t;
        if let Some(h) = body.health.as_mut() {
            *h -= weapon.damage.max(1.0) as i32;
            if *h <= 0 {
                breaks.push((e, hit, *dir));
            }
            continue;
        }
        let mut m = moving.map_or_else(Moving::default, |m| *m);
        bullet_push(&mut m, tf, &body, hit, *dir, BULLET_FORCE);
        commands.entity(e).insert(m);
    }
    for &(e, hit, dir) in breaks.iter() {
        commands.queue(move |world: &mut World| shatter(world, e, hit, dir));
    }
}

/// `player_meleeRange` (units) and the damage of a knife (CoD4's melee
/// knocks clutter as a bullet does, `DynEntCl_MeleeEvent`).
const MELEE_RANGE: f32 = 64.0;
const MELEE_DAMAGE: i32 = 135;

/// Knives knock what's in front of them, and break breakables, at the
/// swing's hit.
#[allow(clippy::type_complexity)]
fn knives(
    mut commands: Commands,
    time: Res<Time>,
    swings: Query<(Entity, &Transform, &crate::movement::Mover, &crate::movement::ViewAngles, &crate::melee::Melee)>,
    mut clutter: Query<(Entity, &Transform, &mut Body, Option<&mut Moving>), With<Clutter>>,
    mut done: Local<std::collections::HashMap<Entity, f32>>,
) {
    let now = time.elapsed_secs();
    done.retain(|e, _| swings.contains(*e));
    for (me, tf, mover, view, swing) in &swings {
        if now < swing.hit_at || done.get(&me) == Some(&swing.started) {
            continue;
        }
        done.insert(me, swing.started);
        let eye = mover.eye(tf.translation);
        let dir = view.forward().normalize_or(Vec3::NEG_Z);
        let reach = units::u(MELEE_RANGE);
        let first = clutter
            .iter()
            .filter_map(|(e, tf, b, _)| ray_box(eye, dir, reach, tf, b).map(|t| (e, t)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((e, t)) = first else { continue };
        let Ok((_, ctf, mut body, moving)) = clutter.get_mut(e) else { continue };
        let hit = eye + dir * t;
        if let Some(h) = body.health.as_mut() {
            *h -= MELEE_DAMAGE;
            if *h <= 0 {
                commands.queue(move |world: &mut World| shatter(world, e, hit, dir));
            }
            continue;
        }
        let mut m = moving.map_or_else(Moving::default, |m| *m);
        bullet_push(&mut m, ctf, &body, hit, dir, BULLET_FORCE);
        commands.entity(e).insert(m);
    }
}

/// Blasts throw the clutter near them (CoD4's `DynEntCl_ExplosionEvent`)
/// and break breakables.
#[allow(clippy::type_complexity)]
fn blasts(
    mut commands: Commands,
    mut exploded: MessageReader<crate::explosives::Exploded>,
    mut clutter: Query<(Entity, &Transform, &mut Body, Option<&mut Moving>), With<Clutter>>,
    mut seed: Local<u32>,
) {
    if !enabled() {
        exploded.clear();
        return;
    }
    for x in exploded.read() {
        // CoD units.
        let radius = units::u(x.radius);
        if radius <= 0.0 {
            continue;
        }
        let mut near: Vec<(Entity, f32)> = clutter
            .iter()
            .map(|(e, tf, b, _)| (e, middle(tf, b).distance(x.at)))
            .filter(|&(_, d)| d < radius)
            .collect();
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut moved = 0;
        for (e, d) in near {
            let Ok((_, tf, mut body, moving)) = clutter.get_mut(e) else { continue };
            // Full at the middle, nothing at the edge.
            let scale = 1.0 - d / radius;
            let c = middle(tf, &body);
            let away = ((c - x.at).normalize_or(Vec3::Y) + Vec3::Y * EXPLODE_UP_BIAS).normalize_or(Vec3::Y);
            if let Some(h) = body.health.as_mut() {
                let damage = ((x.inner - x.outer) * scale + x.outer) as i32;
                if damage > 0 {
                    *h -= damage;
                    if *h <= 0 {
                        commands.queue(move |world: &mut World| shatter(world, e, c, away));
                    }
                }
                continue;
            }
            let force = scale * body.preset.explosive_scale * EXPLODE_FORCE;
            if force < EXPLODE_MIN_FORCE || moved >= EXPLODE_MAX {
                continue;
            }
            moved += 1;
            let off = Vec3::new(rand(&mut seed), rand(&mut seed), rand(&mut seed)) * units::u(EXPLODE_SPIN_SCALE);
            let mut m = moving.map_or_else(Moving::default, |m| *m);
            push(&mut m, tf, &body, c + off, away * force);
            commands.entity(e).insert(m);
        }
    }
}

/// Debug aid: with `COD4RW_CLUTTERTEST=<dir>` (and `COD4RW_SPAWN` by some
/// clutter), shoot every piece in view within 12 m, then set off a blast
/// 3 m ahead, screenshotting before and as things fly and settle, then
/// exit.
#[derive(Resource)]
struct Test(std::path::PathBuf);

#[allow(clippy::too_many_arguments)]
fn test(
    mut commands: Commands,
    real: Res<Time<Real>>,
    dir: Res<Test>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    player: Query<(Entity, &crate::weapons::WeaponState), With<crate::player::LocalPlayer>>,
    clutter: Query<(Entity, &Transform, &Body, Option<&Name>), With<Clutter>>,
    mut shots: MessageWriter<crate::weapons::ShotFired>,
    mut exploded: MessageWriter<crate::explosives::Exploded>,
    mut homes: Local<Vec<(Entity, Vec3)>>,
    spatial: SpatialQuery,
    marks: Query<(&Name, &GlobalTransform, &InheritedVisibility, Option<&MeshMaterial3d<StandardMaterial>>)>,
    mut frames: Local<u32>,
    mut step: Local<usize>,
    mut start: Local<Option<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let (Ok(eye), Ok((me, weapon))) = (camera.single(), player.single()) else { return };
    // The clock starts once loading's hitches are over.
    *frames += 1;
    if *frames < 120 {
        return;
    }
    let t = real.elapsed_secs() - *start.get_or_insert(real.elapsed_secs());
    // (when, what): a screenshot's name, or "shoot", "blast", "exit".
    const STEPS: [(f32, &str); 9] = [
        (0.5, "0_before"),
        (1.0, "shoot"),
        (1.25, "1_shot"),
        (2.5, "2_settled"),
        (3.0, "blast"),
        (3.2, "3_blast"),
        (3.7, "4_flying"),
        (6.0, "5_settled"),
        (6.5, "exit"),
    ];
    let Some(&(at, what)) = STEPS.get(*step) else { return };
    if t < at {
        return;
    }
    *step += 1;
    let from = eye.translation();
    let forward = *eye.forward();
    match what {
        "shoot" => {
            let mut n = 0;
            for (e, tf, b, _) in &clutter {
                let to = middle(tf, b);
                let along = to - from;
                if along.length() < 12.0 {
                    homes.push((e, to));
                }
                if along.length() < 12.0 && along.normalize().dot(forward) > 0.8 {
                    let to = to + along.normalize() * 0.5;
                    shots.write(crate::weapons::ShotFired { shooter: me, weapon: Some(weapon.def), from, to, hit_pawn: false, normal: Vec3::Y, hit_world: false });
                    n += 1;
                }
            }
            info!("clutter test: shot {n} pieces");
            // And a fan of shots at whatever's ahead, for the marks they leave.
            let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
            for i in 0..15 {
                let (yaw, pitch) = ((i % 5) as f32 * 0.1 - 0.75, (i / 5) as f32 * 0.08 - 0.35);
                let dir = (forward + right * yaw + Vec3::Y * pitch).normalize();
                let Ok(d) = Dir3::new(dir) else { continue };
                if let Some(h) = spatial.cast_ray(from, d, 60.0, true, &SpatialQueryFilter::from_mask(Layer::World)) {
                    let to = from + dir * h.distance;
                    shots.write(crate::weapons::ShotFired { shooter: me, weapon: Some(weapon.def), from, to, hit_pawn: false, normal: h.normal, hit_world: true });
                }
            }
        }
        "blast" => {
            let flat = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::X);
            let at = from + flat * 3.0 - Vec3::Y * 1.4;
            exploded.write(crate::explosives::Exploded {
                weapon: "frag_grenade_mp".into(),
                owner: me,
                at,
                radius: 256.0,
                inner: 130.0,
                outer: 50.0,
                effect: String::new(),
                sound: String::new(),
            });
            info!("clutter test: blast at {at}");
        }
        "exit" => {
            for &(e, home) in homes.iter() {
                if let Ok((_, tf, b, name)) = clutter.get(e) {
                    let now = middle(tf, b);
                    if now.distance(home) > 0.05 {
                        info!("clutter test: {} moved {:.2} m, height {:.2} -> {:.2}", name.map_or("?", |n| n.as_str()), now.distance(home), home.y, now.y);
                    }
                }
            }
            info!("clutter test: eye at {:.2} m", from.y);
            for (name, tf, vis, mat) in marks.iter().filter(|m| m.0.as_str() == "impact mark").take(5) {
                info!("clutter test: {name} at {} scale {} visible {} material {}", tf.translation(), tf.scale(), vis.get(), mat.is_some());
            }
            info!("clutter test: {} impact marks", marks.iter().filter(|m| m.0.as_str() == "impact mark").count());
            for (dx, dz) in [(0.0, 0.0), (2.0, 0.0), (0.0, 2.0), (-2.0, 0.0), (0.0, -2.0)] {
                let at = from + Vec3::new(dx, 0.0, dz);
                let below = spatial.cast_ray(at, Dir3::NEG_Y, 10.0, true, &SpatialQueryFilter::from_mask(Layer::World));
                let surface = below.map(|h| (h.distance, crate::terrain::surface_at(at - Vec3::Y * h.distance)));
                info!("clutter test: ground {dx},{dz}: {surface:?}");
            }
            exit.write(AppExit::Success);
        }
        name => {
            std::fs::create_dir_all(&dir.0).ok();
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.0.join(format!("{name}.png"))));
        }
    }
}

/// -1..1, from a little generator.
fn rand(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
}

/// A breakable at 0 health: gone, its effect played, its pieces flying off
/// from where it was hit (`hit`, going `dir`).
fn shatter(world: &mut World, e: Entity, hit: Vec3, dir: Vec3) {
    let Some((tf, body)) = world.get::<Transform>(e).copied().zip(world.get::<Body>(e).cloned()) else { return };
    if let Some(c) = world.get::<Clutter>(e) {
        debug!("clutter: {} broke", c.0);
    }
    world.entity_mut(e).despawn();
    if let Some(fx) = &body.destroy_fx {
        let axis = Mat3::from_quat(tf.rotation) * units::basis();
        let frame = crate::fx::Frame { origin: tf.translation, axis };
        if let Some(mut effects) = world.get_resource_mut::<crate::fx::Effects>() {
            effects.play(fx, crate::fx::Anchor::Fixed(frame), crate::fx::FxLayer::World);
        }
    }
    let mut seed = e.index_u32().wrapping_mul(2_654_435_761);
    let spawned: Vec<Entity> = world.resource_scope(|world, mut content: Mut<Content>| {
        world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
            world.resource_scope(|world, mut materials: Mut<Assets<StandardMaterial>>| {
                world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
                    let mut out = Vec::new();
                    for p in &body.destroy_pieces {
                        let Some((centre, half)) = content.zone(p.zone).xmodel(p.model).map(model_box) else { continue };
                        let at = tf.transform_point(units::pos(p.offset));
                        let piece_tf = Transform::from_translation(at).with_rotation(tf.rotation);
                        let piece = Body {
                            centre,
                            half,
                            preset: body.preset,
                            sound: body.sound.clone(),
                            health: None,
                            destroy_fx: None,
                            destroy_pieces: Vec::new(),
                        };
                        let mut m = Moving { velocity: Vec3::Y * units::u(body.preset.upward), ..default() };
                        // Off along the shot, spread apart.
                        let spread = Vec3::new(rand(&mut seed), rand(&mut seed), rand(&mut seed)) * body.preset.spread;
                        let towards = (dir.normalize_or(Vec3::Y) + spread).normalize_or(Vec3::Y);
                        bullet_push(&mut m, &piece_tf, &piece, hit, towards, PIECES_FORCE);
                        let owner = world
                            .spawn((Name::new("broken piece"), Clutter(usize::MAX), BrokenPiece, piece, m, piece_tf, Visibility::default()))
                            .id();
                        let mut commands = world.commands();
                        spawn_meshes(&mut commands, &mut content, p.zone, p.model, owner, &mut meshes, &mut materials, &mut images);
                        out.push(owner);
                    }
                    world.flush();
                    out
                })
            })
        })
    });
    let mut pieces = world.resource_mut::<Pieces>();
    pieces.0.extend(spawned);
    let mut gone = Vec::new();
    while pieces.0.len() > MAX_PIECES {
        gone.extend(pieces.0.pop_front());
    }
    for g in gone {
        if let Ok(e) = world.get_entity_mut(g) {
            e.despawn();
        }
    }
}

/// The sound of `prefix` (a preset's) hitting `surface`, as CoD4 picks it:
/// `{prefix}_{surface}`, else `{prefix}_default`, else `collision_default`
/// for presets without one.
fn impact_alias(prefix: &str, surface: &str, has: impl Fn(&str) -> bool) -> Option<String> {
    let candidates = if prefix.is_empty() {
        vec!["collision_default".to_owned()]
    } else {
        vec![format!("{prefix}_{surface}"), format!("{prefix}_default")]
    };
    candidates.into_iter().find(|a| has(a))
}

/// The local axis (±x, ±y, ±z) of `rotation` pointing most along `down`.
fn lowest_axis(rotation: Quat, down: Vec3) -> Vec3 {
    let axes = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];
    axes.into_iter().max_by(|a, b| (rotation * *a).dot(down).total_cmp(&(rotation * *b).dot(down))).unwrap_or(Vec3::NEG_Y)
}

/// Moving pieces: fall, hit the world, bounce and slide, settle and sleep.
#[allow(clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    time: Res<Time>,
    spatial: SpatialQuery,
    mut moving: Query<(Entity, &mut Transform, &Body, &mut Moving)>,
    surfaces: Query<&crate::collision::Surfaces>,
    mut sfx: ResMut<crate::audio::Sfx>,
    bank: Option<Res<crate::audio::Bank>>,
    mut aliases: Local<std::collections::HashMap<(String, &'static str), Option<String>>>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs().min(1.0 / 20.0);
    if dt <= 0.0 {
        return;
    }
    let filter = SpatialQueryFilter::from_mask(Layer::World);
    for (e, mut tf, body, mut m) in &mut moving {
        let shape = Collider::cuboid(body.half.x * 2.0, body.half.y * 2.0, body.half.z * 2.0);
        m.velocity.y -= units::u(GRAVITY) * dt;
        m.velocity *= 1.0 - (DRAG_LINEAR * dt).min(1.0);
        m.spin *= 1.0 - (DRAG_ANGULAR * dt).min(1.0);

        // Turn about the middle.
        let c = middle(&tf, body);
        if m.spin.length_squared() > 1e-8 {
            tf.rotation = (Quat::from_scaled_axis(m.spin * dt) * tf.rotation).normalize();
        }
        // Tip onto the nearest face when on the ground and slow.
        if let Some(n) = m.ground.filter(|_| m.velocity.length() < 1.5) {
            let low = tf.rotation * lowest_axis(tf.rotation, -n);
            let settle = Quat::from_rotation_arc(low, -n);
            let rate = (6.0 * dt).min(1.0);
            tf.rotation = (Quat::IDENTITY.slerp(settle, rate) * tf.rotation).normalize();
            m.spin *= 1.0 - (8.0 * dt).min(1.0);
        }
        tf.translation = c - tf.rotation * body.centre;

        // Move, a couple of times if it hits something.
        let mut left = m.velocity * dt;
        let mut ground = None;
        for _ in 0..3 {
            let Ok(d) = Dir3::new(left) else { break };
            let dist = left.length();
            let from = middle(&tf, body);
            // Pieces start out touching (pictures in their walls): what
            // they're leaving or sliding along doesn't stop them.
            let config = ShapeCastConfig { ignore_origin_penetration: true, ..ShapeCastConfig::from_max_distance(dist) };
            let Some(hit) = spatial.cast_shape(&shape, from, tf.rotation, d, &config, &filter) else {
                tf.translation += left;
                break;
            };
            let n = hit.normal1;
            tf.translation += *d * (hit.distance - SKIN).max(0.0);
            if hit.distance <= 0.0 {
                // Stuck in it: out a little.
                tf.translation += n * units::u(0.25);
            }
            if n.y > 0.6 {
                ground = Some(n);
            }
            // Bounce off, and rub along it.
            let into = m.velocity.dot(n);
            // A knock: CoD4 sounds it when the speed into what it hit times
            // the mass (CoD units) passes `phys_minImpactMomentum`.
            if -into / INCH * body.preset.mass > MIN_IMPACT_MOMENTUM && now - m.sounded > SOUND_GAP {
                m.sounded = now;
                let surface = surfaces
                    .get(hit.entity)
                    .map_or_else(|_| crate::terrain::surface_at(hit.point1).unwrap_or("default"), |s| s.facing(n));
                let alias = aliases
                    .entry((body.sound.clone(), surface))
                    .or_insert_with(|| impact_alias(&body.sound, surface, |a| bank.as_ref().is_some_and(|b| b.has(a))));
                if let Some(a) = alias {
                    debug!("clutter: knock {a}");
                    sfx.play(a.clone(), Some(hit.point1));
                } else {
                    debug!("clutter: no sound for {}_{surface}", body.sound);
                }
            }
            if into < 0.0 {
                let tangent = m.velocity - n * into;
                let slow = (-into * (1.0 + body.preset.bounce) * body.preset.friction).min(tangent.length());
                m.velocity -= n * into * (1.0 + body.preset.bounce);
                m.velocity -= tangent.normalize_or_zero() * slow;
                // Rolling off what it hit.
                m.spin += n.cross(tangent) * 0.3 / body.half.min_element().max(0.02);
                m.spin = m.spin.clamp_length_max(MAX_SPIN);
                // Small bounces die.
                if -into < 0.5 {
                    let out = m.velocity.dot(n).max(0.0);
                    m.velocity -= n * out;
                }
            }
            let rest = (dist - hit.distance).max(0.0);
            left = (left - n * left.dot(n)).normalize_or_zero() * rest;
        }
        // Resting: stood on the ground rather than in it, after turning.
        if ground.is_none() && m.ground.is_some() {
            // Still touching the ground just under it?
            let down = ShapeCastConfig::from_max_distance(units::u(1.0));
            if let Some(hit) = spatial.cast_shape(&shape, middle(&tf, body), tf.rotation, Dir3::NEG_Y, &down, &filter) {
                if hit.normal1.y > 0.6 {
                    ground = Some(hit.normal1);
                    if m.velocity.y < 0.0 {
                        m.velocity.y = 0.0;
                    }
                }
            }
        }
        m.ground = ground;

        if m.velocity.length() < SLEEP_SPEED && m.spin.length() < SLEEP_SPIN && m.ground.is_some() {
            m.still += dt;
            if m.still > SLEEP_AFTER {
                commands.entity(e).remove::<Moving>();
            }
        } else {
            m.still = 0.0;
        }
        // Fallen out of the world.
        if tf.translation.y < -units::u(20_000.0) {
            warn!("clutter: a piece fell out of the world");
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(half: Vec3, mass: f32) -> Body {
        Body {
            centre: Vec3::ZERO,
            half,
            preset: Preset { mass, ..Preset::default() },
            sound: String::new(),
            health: None,
            destroy_fx: None,
            destroy_pieces: Vec::new(),
        }
    }

    #[test]
    fn quaternions_turn_into_axes() {
        // A quarter turn about up (yaw 90): forward is CoD's +Y.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let [f, l, u] = quat_axes([0.0, 0.0, h, h]);
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(near(f, [0.0, 1.0, 0.0]) && near(l, [-1.0, 0.0, 0.0]) && near(u, [0.0, 0.0, 1.0]), "{f:?} {l:?} {u:?}");
    }

    #[test]
    fn rays_enter_boxes() {
        let b = body(Vec3::splat(0.5), 1.0);
        let tf = Transform::from_xyz(5.0, 0.0, 0.0);
        assert!((ray_box(Vec3::ZERO, Vec3::X, 10.0, &tf, &b).unwrap() - 4.5).abs() < 1e-4);
        assert!(ray_box(Vec3::ZERO, Vec3::Y, 10.0, &tf, &b).is_none());
        assert!(ray_box(Vec3::ZERO, Vec3::X, 4.0, &tf, &b).is_none());
        // Turned 45 degrees: its corner is nearer.
        let turned = tf.with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4));
        assert!(ray_box(Vec3::ZERO, Vec3::X, 10.0, &turned, &b).unwrap() < 4.4);
    }

    #[test]
    fn bullets_push_light_things_harder() {
        let tf = Transform::default();
        let (mut can, mut block) = (Moving::default(), Moving::default());
        bullet_push(&mut can, &tf, &body(Vec3::splat(0.05), 1.0), Vec3::ZERO, Vec3::X, BULLET_FORCE);
        bullet_push(&mut block, &tf, &body(Vec3::splat(0.2), 20.0), Vec3::ZERO, Vec3::X, BULLET_FORCE);
        assert!(can.velocity.length() > block.velocity.length() * 5.0);
        // Pushed along the shot and a little up.
        assert!(can.velocity.x > 0.0 && can.velocity.y > 0.0);
    }

    #[test]
    fn impact_sounds_fall_back() {
        let has = |a: &str| matches!(a, "physics_wood_concrete" | "physics_wood_default" | "collision_default");
        assert_eq!(impact_alias("physics_wood", "concrete", has).as_deref(), Some("physics_wood_concrete"));
        assert_eq!(impact_alias("physics_wood", "metal", has).as_deref(), Some("physics_wood_default"));
        assert_eq!(impact_alias("", "metal", has).as_deref(), Some("collision_default"));
        assert_eq!(impact_alias("physics_tire", "metal", has), None);
    }

    #[test]
    fn settles_on_the_nearest_face() {
        let tipped = Quat::from_rotation_z(0.3);
        assert_eq!(lowest_axis(tipped, Vec3::NEG_Y), Vec3::NEG_Y);
        let on_side = Quat::from_rotation_z(1.4);
        assert_eq!(lowest_axis(on_side, Vec3::NEG_Y), Vec3::NEG_X);
    }
}
