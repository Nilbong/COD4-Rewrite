//! Map collision from the clipMap: brushes become convex hulls, terrain and
//! patch triangles become one trimesh. Only the world's own brushes: brush
//! entities (triggers, objective models) are stored relative to their
//! entity and aren't solid to players in Team Deathmatch.

use crate::units;
use avian3d::prelude::*;
use bevy::prelude::*;
use iw3::zone::ClipMap;

/// Physics layers. Movement collides with `World | PlayerClip`, bullets with
/// `World | Hitbox`.
#[derive(PhysicsLayer, Default, Clone, Copy, Debug)]
pub enum Layer {
    #[default]
    Default,
    World,
    PlayerClip,
    Hitbox,
    /// Mantle volumes: only mantle checks see them.
    Mantle,
}

pub mod contents {
    pub const SOLID: i32 = 0x1;
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
    SpatialQueryFilter::from_mask([Layer::World, Layer::Hitbox])
}

pub fn sight_filter() -> SpatialQueryFilter {
    SpatialQueryFilter::from_mask([Layer::World])
}

pub fn spawn_collision(commands: &mut Commands, clip: &ClipMap) {
    let root = commands.spawn((Name::new("collision"), Transform::default(), Visibility::Hidden)).id();
    let (mut solid, mut player_clip, mut skipped) = (0, 0, 0);
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
                    ChildOf(root),
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
        commands.spawn((
            collider,
            Surfaces(surfaces),
            CollisionLayers::new(layer, LayerMask::NONE),
            Transform::default(),
            ChildOf(root),
        ));
    }

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
            ChildOf(root),
        ));
    }
    info!("collision: {solid} solid brushes, {player_clip} player clip, {skipped} other skipped");
}

/// Vertices of a brush: intersections of every three bounding planes that
/// lie inside all of them. Works in CoD space.
fn brush_points(clip: &ClipMap, brush: &iw3::zone::Brush) -> Vec<Vec3> {
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
