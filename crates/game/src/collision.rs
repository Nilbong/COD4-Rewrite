//! Map collision from the clipMap: brushes become convex hulls, terrain and
//! patch triangles become one trimesh. Only the world's own brushes: brush
//! entities (triggers, objective models) are stored relative to their
//! entity and aren't solid to players in Team Deathmatch.

use crate::units;
use avian3d::prelude::*;
use bevy::prelude::*;
use iw3::zone::ClipMap;

/// Physics layers. Movement collides with `World | PlayerClip`, bullets with
/// `World | ShotClip | Hitbox`, bots' sight with `World | ShotClip | NoSight`.
#[derive(PhysicsLayer, Default, Clone, Copy, Debug)]
pub enum Layer {
    #[default]
    Default,
    World,
    PlayerClip,
    Hitbox,
    /// Mantle volumes: only mantle checks see them.
    Mantle,
    /// `CONTENTS_CLIPSHOT` brushes that aren't solid: they stop bullets (and
    /// sight), not players.
    ShotClip,
    /// Non-solid `CONTENTS_FOLIAGE` / `CONTENTS_AI_NOSIGHT` brushes: they
    /// only block sight (the level's hedges and bushes).
    NoSight,
}

pub mod contents {
    pub const SOLID: i32 = 0x1;
    pub const FOLIAGE: i32 = 0x2;
    pub const AI_NOSIGHT: i32 = 0x1000;
    pub const CLIPSHOT: i32 = 0x2000;
    pub const GLASS: i32 = 0x10;
    pub const PLAYERCLIP: i32 = 0x10000;
    pub const MANTLE: i32 = 0x1000000;
}

/// Surface flag of `mantle_over` sides: the climb carries on over the top.
const SURF_MANTLE_OVER: i32 = 0x4000000;

/// A mantle volume: players climb what it's on (see `movement::mantle`).
#[derive(Component, Clone, Copy, Debug)]
pub struct MantleSurface {
    /// `mantle_over`: over and down the far side where it drops away.
    pub over: bool,
}

/// CoD4's surface types, by `surfaceFlags >> 20 & 31`. They pick footstep,
/// landing and bullet impact sounds (`step_run_concrete`, ...).
pub const SURFACE_NAMES: [&str; 29] = [
    "default", "bark", "brick", "carpet", "cloth", "concrete", "dirt", "flesh", "foliage", "glass", "grass", "gravel",
    "ice", "metal", "mud", "paper", "plaster", "rock", "sand", "snow", "water", "wood", "asphalt", "ceramic", "plastic",
    "rubber", "cushion", "fruit", "paintedmetal",
];

/// A brush's surface type per axis-aligned face, CoD axes:
/// `[-x, -y, -z, +x, +y, +z]`. Terrain has none (it is "default").
#[derive(Component, Clone, Copy, Debug)]
pub struct Surfaces(pub [u8; 6]);

impl Surfaces {
    /// The surface name of the face a hit `normal` (Bevy space) came out of.
    pub fn facing(&self, normal: Vec3) -> &'static str {
        let n = units::to_cod(normal / units::INCH);
        let axis = (0..3).max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs())).unwrap_or(2);
        let face = if n[axis] >= 0.0 { 3 + axis } else { axis };
        SURFACE_NAMES.get(self.0[face] as usize).copied().unwrap_or("default")
    }
}

fn surface_type(clip: &ClipMap, material: i64) -> u8 {
    usize::try_from(material).ok().and_then(|m| clip.materials.get(m)).map_or(0, |m| ((m.surface_flags >> 20) & 31) as u8)
}

pub fn movement_filter() -> SpatialQueryFilter {
    SpatialQueryFilter::from_mask([Layer::World, Layer::PlayerClip])
}

pub fn bullet_filter() -> SpatialQueryFilter {
    SpatialQueryFilter::from_mask([Layer::World, Layer::ShotClip, Layer::Hitbox])
}

/// What blocks seeing someone, as CoD4's `MASK_AIMTARGET_VISIBILITY`: the
/// world, shot clip, foliage and the "no sight" brushes over hedges and
/// bushes (for bots' eyes).
pub fn ai_sight_filter() -> SpatialQueryFilter {
    SpatialQueryFilter::from_mask([Layer::World, Layer::ShotClip, Layer::NoSight])
}

pub fn sight_filter() -> SpatialQueryFilter {
    SpatialQueryFilter::from_mask([Layer::World])
}

/// Each brush collider's face planes (Bevy space: normal, distance), for
/// traces that want the face they hit rather than a rounded edge
/// (`movement`'s, as CoD4's brush traces).
#[derive(Resource, Default)]
pub struct BrushFaces(pub std::collections::HashMap<Entity, Vec<(Vec3, f32)>>);

impl BrushFaces {
    /// The face of `entity` through `point`, facing a trace along `dir`,
    /// that best matches `normal` (at an edge: a stair's top for a trace
    /// down, its riser for one forward).
    pub fn face(&self, entity: Entity, point: Vec3, normal: Vec3, dir: Vec3) -> Option<Vec3> {
        self.0
            .get(&entity)?
            .iter()
            .filter(|(n, d)| (n.dot(point) - d).abs() < units::u(0.25) && n.dot(dir) < -0.05)
            .max_by(|a, b| a.0.dot(normal).total_cmp(&b.0.dot(normal)))
            .map(|f| f.0)
    }
}

/// The world's colliders never move: static bodies, each its own entity
/// (no parent), so avian keeps them in its static tree and never refits,
/// re-optimises or re-syncs them (with the moving hitboxes in the same tree
/// that cost several ms a frame). Moving colliders are synced by
/// [`sync_moved_colliders`].
fn static_body() -> RigidBody {
    RigidBody::Static
}

/// Avian's own transform-to-position sync walks every collider every frame;
/// this one only those whose transform changed (hitboxes, the helicopter):
/// see [`physics_transform_config`].
pub fn sync_moved_colliders(mut moved: Query<(&GlobalTransform, &mut Position, &mut Rotation), Changed<GlobalTransform>>) {
    for (gt, mut pos, mut rot) in &mut moved {
        let (_, r, t) = gt.to_scale_rotation_translation();
        if pos.0 != t {
            pos.0 = t;
        }
        let r = Rotation::from(r);
        if *rot != r {
            *rot = r;
        }
    }
}

/// Avian's transform syncing, minus the per-frame walks over every collider
/// ([`sync_moved_colliders`] stands in).
pub fn physics_transform_config() -> avian3d::physics_transform::PhysicsTransformConfig {
    avian3d::physics_transform::PhysicsTransformConfig { transform_to_position: false, transform_to_collider_scale: false, ..default() }
}

pub fn spawn_collision(commands: &mut Commands, clip: &ClipMap) {
    let mut faces = BrushFaces::default();
    let (mut solid, mut player_clip, mut shot_clip, mut no_sight, mut skipped) = (0, 0, 0, 0, 0);
    let entity = clip.entity_brushes();
    for (i, brush) in clip.brushes.iter().enumerate() {
        if entity.contains(&(i as u32)) {
            skipped += 1;
            continue;
        }
        if brush.contents & contents::MANTLE != 0 {
            let points: Vec<Vec3> = brush_points(clip, brush).into_iter().map(|p| units::pos(p.to_array())).collect();
            let materials = brush.side_materials.iter().copied().chain(brush.axial_materials.iter().flatten().filter(|&&m| m >= 0).map(|&m| m as u32));
            let over = materials.filter_map(|m| clip.materials.get(m as usize)).any(|m| m.surface_flags & SURF_MANTLE_OVER != 0);
            if let Some(collider) = (points.len() >= 4).then(|| Collider::convex_hull(points)).flatten() {
                commands.spawn((
                    collider,
                    MantleSurface { over },
                    CollisionLayers::new(Layer::Mantle, LayerMask::NONE),
                    Transform::default(),
                    static_body(),
                ));
            }
            continue;
        }
        let layer = if brush.contents & (contents::SOLID | contents::GLASS) != 0 {
            solid += 1;
            Layer::World
        } else if brush.contents & contents::PLAYERCLIP != 0 {
            player_clip += 1;
            Layer::PlayerClip
        } else if brush.contents & contents::CLIPSHOT != 0 {
            shot_clip += 1;
            Layer::ShotClip
        } else if brush.contents & (contents::FOLIAGE | contents::AI_NOSIGHT) != 0 {
            no_sight += 1;
            Layer::NoSight
        } else {
            skipped += 1;
            continue;
        };
        let points = brush_points(clip, brush);
        if points.len() < 4 {
            continue;
        }
        let points: Vec<Vec3> = points.into_iter().map(|p| units::pos(p.to_array())).collect();
        let Some(collider) = Collider::convex_hull(points) else { continue };
        // Bevelled faces fall back to the brush's first other material.
        let fallback = brush.side_materials.first().map_or(0, |&m| surface_type(clip, m as i64));
        let surfaces = std::array::from_fn(|f| {
            let m = brush.axial_materials[f / 3][f % 3];
            if m < 0 { fallback } else { surface_type(clip, m as i64) }
        });
        let e = commands
            .spawn((collider, Surfaces(surfaces), CollisionLayers::new(layer, LayerMask::NONE), Transform::default(), static_body()))
            .id();
        let planes = brush_planes(clip, brush).into_iter().map(|(n, d)| (units::dir(n.to_array()), units::u(d))).collect();
        faces.0.insert(e, planes);
    }
    commands.insert_resource(faces);

    // Terrain / curve patches: an indexed triangle soup.
    if !clip.tri_indices.is_empty() {
        let verts: Vec<Vec3> = clip.verts.iter().map(|&v| units::pos(v)).collect();
        let tris: Vec<[u32; 3]> = clip
            .tri_indices
            .chunks_exact(3)
            .filter(|t| t.iter().all(|&i| (i as usize) < verts.len()))
            .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
            .collect();
        commands.spawn((
            Collider::trimesh(verts, tris),
            CollisionLayers::new(Layer::World, LayerMask::NONE),
            Transform::default(),
            static_body(),
        ));
    }
    info!("collision: {solid} solid brushes, {player_clip} player clip, {shot_clip} shot clip, {no_sight} sight-blocking, {skipped} other skipped");
}

/// The clip map's static models (props with collision: rocks, air
/// conditioners, grass and shrub clumps). CoD4 traces them only for lines
/// (bullets, sight: `CM_PointTraceStaticModels`), never for players moving,
/// by the model's contents: solid ones stop bullets and sight
/// ([`Layer::ShotClip`]), foliage only sight ([`Layer::NoSight`]). Each is
/// its placed bounds, a box (CoD4 traces the model's collision triangles,
/// which this doesn't read yet).
pub fn spawn_static_model_collision(commands: &mut Commands, zone: &iw3::zone::Zone, clip: &ClipMap) {
    let (mut solid, mut foliage) = (0, 0);
    for sm in &clip.static_models {
        let Some(model) = sm.model.and_then(|id| zone.xmodel(id)) else { continue };
        let (layer, surface) = if model.contents & contents::SOLID != 0 {
            solid += 1;
            (Layer::ShotClip, "default")
        } else if model.contents & contents::FOLIAGE != 0 {
            foliage += 1;
            (Layer::NoSight, "foliage")
        } else {
            continue;
        };
        let (a, b) = (units::pos(sm.absmin), units::pos(sm.absmax));
        let (min, max) = (a.min(b), a.max(b));
        let size = max - min;
        if size.min_element() <= 0.0 {
            continue;
        }
        commands.spawn((
            Collider::cuboid(size.x, size.y, size.z),
            Surfaces([surface_index(surface); 6]),
            CollisionLayers::new(layer, LayerMask::NONE),
            Transform::from_translation((min + max) * 0.5),
            Position((min + max) * 0.5),
            static_body(),
        ));
    }
    info!("collision: {solid} solid and {foliage} foliage static models (bullets and sight only)");
}

fn surface_index(name: &str) -> u8 {
    SURFACE_NAMES.iter().position(|n| *n == name).unwrap_or(0) as u8
}

/// Vertices of a brush: intersections of every three bounding planes that
/// lie inside all of them. Works in CoD space.
/// A brush's planes (CoD space): its box's six, then its other sides.
fn brush_planes(clip: &ClipMap, brush: &iw3::zone::Brush) -> Vec<(Vec3, f32)> {
    let mut planes: Vec<(Vec3, f32)> = vec![
        (Vec3::X, brush.maxs[0]),
        (Vec3::NEG_X, -brush.mins[0]),
        (Vec3::Y, brush.maxs[1]),
        (Vec3::NEG_Y, -brush.mins[1]),
        (Vec3::Z, brush.maxs[2]),
        (Vec3::NEG_Z, -brush.mins[2]),
    ];
    for &p in &brush.side_planes {
        if let Some(pl) = clip.planes.get(p as usize) {
            planes.push((Vec3::from(pl.normal), pl.dist));
        }
    }
    planes
}

fn brush_points(clip: &ClipMap, brush: &iw3::zone::Brush) -> Vec<Vec3> {
    let planes = brush_planes(clip, brush);
    let mut out: Vec<Vec3> = Vec::new();
    let n = planes.len();
    for i in 0..n {
        for j in i + 1..n {
            for k in j + 1..n {
                let (n1, d1) = planes[i];
                let (n2, d2) = planes[j];
                let (n3, d3) = planes[k];
                let denom = n1.dot(n2.cross(n3));
                if denom.abs() < 1e-6 {
                    continue;
                }
                let p = (n2.cross(n3) * d1 + n3.cross(n1) * d2 + n1.cross(n2) * d3) / denom;
                if planes.iter().all(|&(pn, pd)| pn.dot(p) - pd <= 0.01)
                    && !out.iter().any(|q| q.distance_squared(p) < 0.01)
                {
                    out.push(p);
                }
            }
        }
    }
    out
}
