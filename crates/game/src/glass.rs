//! Modern Warfare 2's breakable glass: MW2 maps keep their windows, glass
//! doors and banisters out of the world's geometry, as panes of its glass
//! system ([`crate::mw2::take_glass`]). Each is drawn as its polygon, stops
//! players (not bullets), and shatters when shot, knifed or caught in a
//! blast: gone, with a shower of shards and the sound of breaking glass.

use crate::content::{Content, MAP_ZONE};
use crate::state::{GameState, Setup, in_game};
use crate::units;
use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

pub struct GlassPlugin;

impl Plugin for GlassPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::InGame), spawn_glass.in_set(Setup::Spawn))
            .add_systems(Update, (shots, knives, blasts, fall).chain().after(crate::weapons::WeaponSet).run_if(in_game));
        // Debug: `COD4RW_GLASSBREAK=<seconds>` breaks every pane then.
        if let Some(at) = std::env::var("COD4RW_GLASSBREAK").ok().and_then(|v| v.parse::<f32>().ok()) {
            app.add_systems(Update, (move |mut commands: Commands, panes: Query<(Entity, &Pane)>, mut sfx: ResMut<crate::audio::Sfx>, time: Res<Time>, mut done: Local<bool>| {
                if *done || time.elapsed_secs() < at {
                    return;
                }
                *done = true;
                info!("glass: breaking {} panes", panes.iter().count());
                for (e, p) in &panes {
                    shatter(&mut commands, e, p, p.centre, -p.normal, &mut sfx, None, time.elapsed_secs());
                }
            }).run_if(in_game));
        }
    }
}

/// A pane (Bevy space): its corners, plane and how far it reaches.
#[derive(Component)]
struct Pane {
    corners: Vec<Vec3>,
    normal: Vec3,
    centre: Vec3,
    radius: f32,
    material: Handle<StandardMaterial>,
    /// Its collider (stops players), if one could be made.
    blocker: Option<Entity>,
}

/// A falling shard of a broken pane.
#[derive(Component)]
struct Shard {
    velocity: Vec3,
    spin: Vec3,
    until: f32,
}

/// `phys_gravity` (units/s²).
const GRAVITY: f32 = 800.0;
/// How long shards fall before they're gone (s).
const SHARD_LIFE: f32 = 2.5;
const SHARDS: usize = 14;
/// `player_meleeRange` (units).
const MELEE_RANGE: f32 = 64.0;
/// Breaking glass: the first of these the sound bank has.
const BREAK_SOUNDS: [&str; 5] = ["glass_pane_break", "glass_break", "glass_pane_shatter", "bullet_large_glass", "bullet_small_glass"];

fn spawn_glass(
    mut commands: Commands,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let panes = crate::mw2::take_glass();
    if panes.is_empty() {
        return;
    }
    let root = commands.spawn((Name::new("glass"), Transform::default(), Visibility::default())).id();
    // Two-sided copies of the panes' materials.
    let mut two_sided: std::collections::HashMap<usize, Handle<StandardMaterial>> = Default::default();
    let mut drawn = 0;
    for p in &panes {
        let Some(mat_id) = p.material else { continue };
        let material = match two_sided.get(&mat_id) {
            Some(h) => h.clone(),
            None => {
                let Some(m) = content.material(MAP_ZONE, mat_id, &mut materials, &mut images) else { continue };
                let mut copy = materials.get(&m.handle).cloned().unwrap_or_default();
                copy.double_sided = true;
                copy.cull_mode = None;
                let h = materials.add(copy);
                two_sided.insert(mat_id, h.clone());
                h
            }
        };
        let corners: Vec<Vec3> = p.corners.iter().map(|c| units::pos(*c)).collect();
        let normal = units::dir(p.normal).normalize_or(Vec3::Y);
        let centre = corners.iter().copied().sum::<Vec3>() / corners.len() as f32;
        let radius = corners.iter().map(|c| c.distance(centre)).fold(0.0, f32::max);
        // A fan from the first corner (the panes are convex).
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, corners.iter().map(|c| c.to_array()).collect::<Vec<_>>());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![normal.to_array(); corners.len()]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, p.uvs.clone());
        mesh.insert_indices(Indices::U32((1..corners.len() as u32 - 1).flat_map(|k| [0, k, k + 1]).collect()));
        // Solid to players: the pane, thickened a little.
        let half = units::u(p.half_thickness.max(0.5));
        let hull: Vec<Vec3> = corners.iter().flat_map(|c| [*c + normal * half, *c - normal * half]).collect();
        let blocker = Collider::convex_hull(hull).map(|c| {
            commands
                .spawn((Name::new("glass blocker"), RigidBody::Static, c, CollisionLayers::new(crate::collision::Layer::PlayerClip, LayerMask::NONE), Transform::default()))
                .id()
        });
        commands.spawn((
            Name::new("glass pane"),
            Pane { corners, normal, centre, radius, material: material.clone(), blocker },
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            bevy::light::NotShadowCaster,
            Transform::default(),
            ChildOf(root),
        ));
        drawn += 1;
    }
    info!("glass: {drawn} of {} panes", panes.len());
}

/// Where a ray first crosses a pane, if it does (distance along it).
fn ray_pane(from: Vec3, dir: Vec3, max: f32, p: &Pane) -> Option<f32> {
    let denom = dir.dot(p.normal);
    if denom.abs() < 1e-5 {
        return None;
    }
    let t = (p.centre - from).dot(p.normal) / denom;
    // (A little past where the shot ended: it may have stopped on
    // something touching the glass.)
    if !(0.0..=max + units::u(2.0)).contains(&t) {
        return None;
    }
    let hit = from + dir * t;
    if hit.distance(p.centre) > p.radius {
        return None;
    }
    // Inside: on the same side of every edge.
    let n = p.corners.len();
    let mut sign = 0.0f32;
    for k in 0..n {
        let (a, b) = (p.corners[k], p.corners[(k + 1) % n]);
        let s = (b - a).cross(hit - a).dot(p.normal);
        if s.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = s.signum();
        } else if s.signum() != sign {
            return None;
        }
    }
    Some(t)
}

fn shots(
    mut commands: Commands,
    mut shots: MessageReader<crate::weapons::ShotFired>,
    panes: Query<(Entity, &Pane)>,
    mut sfx: ResMut<crate::audio::Sfx>,
    bank: Option<Res<crate::audio::Bank>>,
    time: Res<Time>,
) {
    for s in shots.read() {
        let along = s.to - s.from;
        let Ok(dir) = Dir3::new(along) else { continue };
        // (A bullet goes on through: every pane on its way breaks.)
        for (e, p) in &panes {
            if let Some(t) = ray_pane(s.from, *dir, along.length(), p) {
                shatter(&mut commands, e, p, s.from + *dir * t, *dir, &mut sfx, bank.as_deref(), time.elapsed_secs());
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn knives(
    mut commands: Commands,
    time: Res<Time>,
    swings: Query<(Entity, &Transform, &crate::movement::Mover, &crate::movement::ViewAngles, &crate::melee::Melee)>,
    panes: Query<(Entity, &Pane)>,
    mut sfx: ResMut<crate::audio::Sfx>,
    bank: Option<Res<crate::audio::Bank>>,
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
        let first = panes.iter().filter_map(|(e, p)| ray_pane(eye, dir, units::u(MELEE_RANGE), p).map(|t| (e, p, t))).min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((e, p, t)) = first {
            shatter(&mut commands, e, p, eye + dir * t, dir, &mut sfx, bank.as_deref(), now);
        }
    }
}

fn blasts(
    mut commands: Commands,
    mut exploded: MessageReader<crate::explosives::Exploded>,
    panes: Query<(Entity, &Pane)>,
    mut sfx: ResMut<crate::audio::Sfx>,
    bank: Option<Res<crate::audio::Bank>>,
    time: Res<Time>,
) {
    for x in exploded.read() {
        let radius = units::u(x.radius);
        for (e, p) in &panes {
            if p.centre.distance(x.at) < radius + p.radius * 0.5 {
                let dir = (p.centre - x.at).normalize_or(Vec3::Y);
                shatter(&mut commands, e, p, p.centre, dir, &mut sfx, bank.as_deref(), time.elapsed_secs());
            }
        }
    }
}

fn rand(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
}

/// Gone, with a shower of shards: triangles cut from the pane around where
/// it was hit, falling and tumbling away along the shot.
#[allow(clippy::too_many_arguments)]
fn shatter(commands: &mut Commands, e: Entity, p: &Pane, hit: Vec3, dir: Vec3, sfx: &mut crate::audio::Sfx, bank: Option<&crate::audio::Bank>, now: f32) {
    let Ok(mut pane) = commands.get_entity(e) else { return };
    pane.despawn();
    if let Some(b) = p.blocker {
        commands.entity(b).despawn();
    }
    if let Some(alias) = BREAK_SOUNDS.iter().find(|a| bank.is_some_and(|b| b.has(a))) {
        sfx.play(*alias, Some(hit));
    }
    let mut seed = e.index_u32().wrapping_mul(2_654_435_761) ^ now.to_bits();
    let u = (p.corners[1] - p.corners[0]).normalize_or(Vec3::X);
    let v = p.normal.cross(u);
    let size = (p.radius * 0.25).clamp(0.04, 0.3);
    for _ in 0..SHARDS {
        // Somewhere on the pane, nearer the hit.
        let at = p.centre.lerp(hit, 0.5) + (u * rand(&mut seed) + v * rand(&mut seed)) * p.radius;
        let tri: Vec<[f32; 3]> = (0..3).map(|_| ((u * rand(&mut seed) + v * rand(&mut seed)) * size).to_array()).collect();
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, tri);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![p.normal.to_array(); 3]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        let velocity = dir * units::u(60.0 + 140.0 * (rand(&mut seed) + 0.5))
            + Vec3::new(rand(&mut seed), rand(&mut seed) + 0.3, rand(&mut seed)) * units::u(120.0);
        let spin = Vec3::new(rand(&mut seed), rand(&mut seed), rand(&mut seed)) * 20.0;
        let material = p.material.clone();
        commands.queue(move |world: &mut World| {
            let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);
            world.spawn((
                Name::new("glass shard"),
                Shard { velocity, spin, until: now + SHARD_LIFE },
                Mesh3d(mesh),
                MeshMaterial3d(material),
                bevy::light::NotShadowCaster,
                Transform::from_translation(at),
            ));
        });
    }
}

fn fall(mut commands: Commands, time: Res<Time>, mut shards: Query<(Entity, &mut Shard, &mut Transform)>) {
    let (dt, now) = (time.delta_secs(), time.elapsed_secs());
    for (e, mut s, mut tf) in &mut shards {
        if now > s.until {
            commands.entity(e).despawn();
            continue;
        }
        s.velocity.y -= units::u(GRAVITY) * dt;
        tf.translation += s.velocity * dt;
        let spin = s.spin;
        tf.rotate(Quat::from_scaled_axis(spin * dt));
    }
}
