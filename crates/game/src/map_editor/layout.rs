//! A map's layout: the objects placed on it with the map editor, saved as
//! one line each in `crates/game/assets/maplayouts/<map>.txt` (built in,
//! `build.rs`), or `%LOCALAPPDATA%/cod4rw/maplayouts/<map>.txt`, which wins.
//!
//! A line: `model  x y z  qx qy qz qw  scale  [params]`: the model's name,
//! where it stands (CoD units, CoD axes, as the map's own entities), its
//! turn (Bevy space) and size, then any settings of its kind. `#` starts a
//! comment. Names starting `@` aren't models ([`Kind`]): `@light` (params:
//! red, green, blue, brightness, range in metres, shadows 0/1),
//! `@fx:<effect>`, `@spawn:<tdm|dm|allies|axis>`, `@hide:<model>` (the
//! map's own model standing at that spot, left out as the map loads) and
//! `@actor:<character>/<gun>/<idle>` (a person, [`super::actors`]).
//!
//! Headquarters keeps its own layout, `hq_killhouse` ([`key`]), apart from
//! the campaign's F.N.G. on the same level.

use crate::collision::{Layer, Surfaces};
use crate::content::Content;
use crate::units;
use avian3d::prelude::*;
use bevy::prelude::*;

include!(concat!(env!("OUT_DIR"), "/maplayouts.rs"));

/// One placed object.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub model: String,
    /// Bevy space.
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: f32,
    /// The kind's settings (a light's colour...).
    pub params: Vec<f32>,
}

/// What a line places.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind<'a> {
    Model(&'a str),
    Light,
    Fx(&'a str),
    Spawn(&'a str),
    Hide(&'a str),
    Actor(&'a str),
}

pub const LIGHT: &str = "@light";
/// A new light's settings: warm white, brightness 1, 8 m, no shadows.
pub const LIGHT_DEFAULTS: [f32; 6] = [1.0, 0.85, 0.65, 1.0, 8.0, 0.0];

impl Placed {
    pub fn kind(&self) -> Kind<'_> {
        let m = self.model.as_str();
        if m == LIGHT {
            Kind::Light
        } else if let Some(f) = m.strip_prefix("@fx:") {
            Kind::Fx(f)
        } else if let Some(k) = m.strip_prefix("@spawn:") {
            Kind::Spawn(k)
        } else if let Some(h) = m.strip_prefix("@hide:") {
            Kind::Hide(h)
        } else if m.starts_with(super::actors::PREFIX) {
            Kind::Actor(m)
        } else {
            Kind::Model(m)
        }
    }

    /// Setting `i`, or the kind's default.
    pub fn param(&self, i: usize) -> f32 {
        self.params.get(i).copied().unwrap_or_else(|| if self.kind() == Kind::Light { LIGHT_DEFAULTS.get(i).copied().unwrap_or(0.0) } else { 0.0 })
    }

    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.pos).with_rotation(self.rot).with_scale(Vec3::splat(self.scale))
    }
}

/// The objects placed on this map, as spawned ([`PlacedObject`] entities
/// carry their index).
#[derive(Resource, Default, Clone)]
pub struct Layout {
    pub objects: Vec<Placed>,
}

/// A placed object's root entity: its index in [`Layout::objects`].
#[derive(Component, Clone, Copy)]
pub struct PlacedObject(pub usize);

/// A placed model's box (model space, Bevy axes, unscaled): for picking
/// and the selection outline.
#[derive(Component, Clone, Copy)]
pub struct ObjectBounds {
    pub min: Vec3,
    pub max: Vec3,
}

pub fn parse(text: &str) -> Vec<Placed> {
    text.lines()
        .filter_map(|l| {
            let l = l.split('#').next()?.trim();
            let mut it = l.split_whitespace();
            let model = it.next()?.to_owned();
            let n: Vec<f32> = it.filter_map(|v| v.parse().ok()).collect();
            if n.len() < 8 || n.iter().any(|v| !v.is_finite()) {
                return None;
            }
            let rot = Quat::from_xyzw(n[3], n[4], n[5], n[6]);
            Some(Placed {
                model,
                pos: units::pos([n[0], n[1], n[2]]),
                rot: if rot.length_squared() > 1e-6 { rot.normalize() } else { Quat::IDENTITY },
                scale: n[7].clamp(0.05, 20.0),
                params: n[8..].to_vec(),
            })
        })
        .collect()
}

pub fn write(map: &str, objects: &[Placed]) -> String {
    let mut text = format!("# {map}'s placed objects, from the map editor (F7): model, x y z (CoD units), turn (quaternion), scale.\n");
    for o in objects {
        let p = units::to_cod(o.pos);
        let q = o.rot;
        text.push_str(&format!(
            "{} {:.2} {:.2} {:.2} {:.5} {:.5} {:.5} {:.5} {:.3}",
            o.model, p[0], p[1], p[2], q.x, q.y, q.z, q.w, o.scale
        ));
        for v in &o.params {
            text.push_str(&format!(" {v:.3}"));
        }
        text.push('\n');
    }
    text
}

fn project_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("crates/game/assets/maplayouts")
}

fn user_dir() -> std::path::PathBuf {
    // (Tests: `COD4RW_MAPLAYOUT_DIR`.)
    if let Some(d) = std::env::var_os("COD4RW_MAPLAYOUT_DIR") {
        return d.into();
    }
    std::env::var_os("LOCALAPPDATA").map_or_else(|| std::path::PathBuf::from("maplayouts"), |d| std::path::PathBuf::from(d).join("cod4rw").join("maplayouts"))
}

/// The map's saved layout text: the user's copy, the project's, the one
/// built in; `None` without one.
pub fn saved_text(map: &str) -> Option<String> {
    let name = format!("{map}.txt");
    [user_dir().join(&name), project_dir().join(&name)]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .or_else(|| MAPLAYOUTS.iter().find(|(n, _)| *n == name).map(|(_, t)| (*t).to_owned()))
}

/// What the host's layout is for this match online (`netplay`): used
/// instead of this player's own files, so everyone plays the same map.
static OVERRIDE: std::sync::RwLock<Option<(String, String)>> = std::sync::RwLock::new(None);

/// Plays `map` with `text` as its layout (`None`: the player's own again).
pub fn set_override(map_and_text: Option<(String, String)>) {
    if let Ok(mut o) = OVERRIDE.write() {
        *o = map_and_text;
    }
}

/// The name `map`'s layout is saved under: Headquarters' own on its level
/// (not the campaign's mission there), else the map's.
pub fn key(map: &str) -> String {
    let campaign = crate::campaign::ACTIVE.load(std::sync::atomic::Ordering::Relaxed);
    if map == crate::hq::MAP && !campaign { format!("hq_{map}") } else { map.to_owned() }
}

/// The layout `map` is played with: the host's online, else the saved one.
pub fn text_for(map: &str) -> Option<String> {
    if let Some((m, t)) = OVERRIDE.read().ok().and_then(|o| o.clone())
        && m == map
    {
        return Some(t);
    }
    saved_text(map)
}

/// The map's own models left out (`@hide:` lines), CoD origins: read once
/// a match, as the map loads.
static HIDDEN: std::sync::RwLock<(u32, Vec<(String, Vec3)>)> = std::sync::RwLock::new((u32::MAX, Vec::new()));

fn hidden_list(objects: &[Placed]) -> Vec<(String, Vec3)> {
    objects
        .iter()
        .filter_map(|o| match o.kind() {
            Kind::Hide(m) => Some((m.to_owned(), Vec3::from(units::to_cod(o.pos)))),
            _ => None,
        })
        .collect()
}

/// Whether the map's own `model` standing at `origin` (CoD space) is left
/// out of this match ([`crate::world`], [`crate::props`], [`crate::collision`]).
pub fn hidden(model: &str, origin: [f32; 3]) -> bool {
    let id = crate::atmos::climate::match_id();
    let stale = HIDDEN.read().map_or(true, |h| h.0 != id);
    if stale {
        let map = key(&crate::atmos::climate::map_name());
        let list = text_for(&map).map(|t| hidden_list(&parse(&t))).unwrap_or_default();
        if let Ok(mut h) = HIDDEN.write() {
            *h = (id, list);
        }
    }
    let at = Vec3::from(origin);
    HIDDEN.read().is_ok_and(|h| h.1.iter().any(|(m, p)| m == model && p.distance(at) < 2.0))
}

/// Saves into the project when run from it (so it ships), else the user's
/// folder. The message says where.
pub fn save(map: &str, objects: &[Placed]) -> String {
    let dir = if project_dir().is_dir() && std::env::var_os("COD4RW_MAPLAYOUT_DIR").is_none() { project_dir() } else { user_dir() };
    let path = dir.join(format!("{map}.txt"));
    match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, write(map, objects))) {
        Ok(()) => format!("Saved {} objects to {}", objects.len(), path.display()),
        Err(e) => format!("Couldn't save: {e}"),
    }
}

/// A short fingerprint of the layout (for checking players match online).
pub fn hash(objects: &[Placed]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for line in write("", objects).lines().skip(1) {
        line.hash(&mut h);
    }
    h.finish()
}

/// What a model needs to be drawn and be solid, prepared once.
#[derive(Clone)]
pub struct Prepared {
    pub parts: Vec<(Handle<StandardMaterial>, Handle<Mesh>)>,
    /// Model space, Bevy axes.
    pub min: Vec3,
    pub max: Vec3,
    /// Collision boxes (model space, Bevy axes: centre, half size) and
    /// whether they're solid (else foliage: sight only).
    pub boxes: Vec<(Vec3, Vec3)>,
    pub solid: bool,
    /// A surface for impacts.
    pub surface: u8,
}

/// Prepares `name` from the loaded zones; `None` if it isn't there or has
/// nothing to draw.
pub fn prepare(content: &mut Content, name: &str, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Option<Prepared> {
    let parts = crate::props::static_parts(content, name, meshes, materials, images)?;
    let (zi, id) = content.find(name)?;
    let (zi, id) = content.resolve_xmodel(zi, id);
    let xm = content.zone(zi).xmodel(id)?;
    let corner = |a: [f32; 3], b: [f32; 3]| {
        let (a, b) = (units::pos(a), units::pos(b));
        (a.min(b), a.max(b))
    };
    let (min, max) = corner(xm.mins, xm.maxs);
    let foliage = xm.contents & crate::collision::contents::FOLIAGE != 0 && xm.contents & crate::collision::contents::SOLID == 0;
    let mut boxes: Vec<(Vec3, Vec3)> = xm
        .coll_boxes
        .iter()
        .filter(|b| (0..3).all(|i| b.maxs[i] > b.mins[i]))
        .map(|b| {
            let (a, z) = corner(b.mins, b.maxs);
            ((a + z) * 0.5, (z - a) * 0.5)
        })
        .collect();
    // Without its own collision, its box: what's placed should be solid
    // (grass and bushes aside).
    let plant = super::catalog::category(name) == super::catalog::Category::Plants;
    if boxes.is_empty() && !plant && (max - min).min_element() > 0.0 {
        boxes.push(((min + max) * 0.5, (max - min) * 0.5));
    }
    let typed = xm.coll_boxes.first().map_or(0, |b| ((b.surf_flags >> 20) & 31) as usize);
    let surface = if typed != 0 { typed as u8 } else { 0 };
    Some(Prepared { parts, min, max, boxes, solid: !foliage && !plant, surface })
}

/// A placed effect (started by `map_editor::start_fx`, stopped as the
/// entity goes).
#[derive(Component)]
pub struct PlacedFx(pub String);

/// Picking boxes of things that aren't models: a light, an effect, a
/// spawn point (a player's size).
pub fn marker_bounds(kind: Kind) -> (Vec3, Vec3) {
    match kind {
        Kind::Spawn(_) => (Vec3::new(-0.4, 0.0, -0.4), Vec3::new(0.4, 1.8, 0.4)),
        Kind::Actor(_) => super::actors::bounds(),
        _ => (Vec3::splat(-0.2), Vec3::splat(0.2)),
    }
}

/// Spawns a placed light, effect or spawn point (no model); `None` for the
/// others.
pub fn spawn_marker(commands: &mut Commands, placed: &Placed, index: usize, root: Option<Entity>) -> Option<Entity> {
    let kind = placed.kind();
    if matches!(kind, Kind::Model(_) | Kind::Hide(_) | Kind::Actor(_)) {
        return None;
    }
    let (min, max) = marker_bounds(kind);
    let mut e = commands.spawn((
        Name::new(format!("placed {}", placed.model)),
        PlacedObject(index),
        ObjectBounds { min, max },
        Transform::from_translation(placed.pos).with_rotation(placed.rot),
        Visibility::default(),
    ));
    if let Some(root) = root {
        e.insert(ChildOf(root));
    }
    match kind {
        Kind::Light => {
            let p = |i| placed.param(i);
            e.insert(PointLight {
                color: Color::srgb(p(0), p(1), p(2)),
                intensity: 400_000.0 * p(3).max(0.0),
                range: p(4).clamp(0.5, 60.0),
                shadow_maps_enabled: p(5) > 0.5,
                ..default()
            });
        }
        Kind::Fx(name) => {
            e.insert(PlacedFx(name.to_owned()));
        }
        _ => {}
    }
    Some(e.id())
}

/// Spawns placed object `index`; returns its root.
pub fn spawn(commands: &mut Commands, p: &Prepared, placed: &Placed, index: usize, root: Option<Entity>) -> Entity {
    let tf = placed.transform();
    let mut e = commands.spawn((
        Name::new(format!("placed {}", placed.model)),
        PlacedObject(index),
        ObjectBounds { min: p.min, max: p.max },
        tf,
        Visibility::default(),
    ));
    if let Some(root) = root {
        e.insert(ChildOf(root));
    }
    // Solid as the world (players, bullets, bots' paths) or, for foliage,
    // to sight only. The scale is in the shapes (colliders don't take it
    // from the transform here: `collision::physics_transform_config`).
    if !p.boxes.is_empty() {
        let s = placed.scale;
        let shapes: Vec<(Vec3, Quat, Collider)> =
            p.boxes.iter().map(|(c, h)| (*c * s, Quat::IDENTITY, Collider::cuboid(h.x * 2.0 * s, h.y * 2.0 * s, h.z * 2.0 * s))).collect();
        let layer = if p.solid { Layer::World } else { Layer::NoSight };
        e.insert((
            RigidBody::Static,
            Collider::compound(shapes),
            CollisionLayers::new(layer, LayerMask::NONE),
            Surfaces([p.surface; 6]),
            Position(placed.pos),
            Rotation(placed.rot),
        ));
    }
    let id = e.id();
    for (material, mesh) in &p.parts {
        commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), Transform::default(), ChildOf(id)));
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let a = vec![
            Placed { model: "com_barrel_blue".into(), pos: Vec3::new(1.0, 2.0, -3.0), rot: Quat::from_rotation_y(0.7), scale: 1.25, params: vec![] },
            Placed { model: LIGHT.into(), pos: Vec3::ZERO, rot: Quat::IDENTITY, scale: 1.0, params: vec![1.0, 0.5, 0.25, 2.0, 6.0, 1.0] },
        ];
        let b = parse(&write("mp_test", &a));
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].kind(), Kind::Light);
        assert_eq!(b[1].params, a[1].params);
        assert_eq!(b[0].model, a[0].model);
        assert!(b[0].pos.distance(a[0].pos) < 0.01);
        assert!(b[0].rot.angle_between(a[0].rot) < 0.001);
        assert!((b[0].scale - 1.25).abs() < 1e-3);
        assert_eq!(hash(&a), hash(&b));
    }

    #[test]
    fn bad_lines_skipped() {
        assert!(parse("# nothing\nmodel 1 2\n\nx 0 0 0 0 0 0 1 nan\n").is_empty());
    }
}
