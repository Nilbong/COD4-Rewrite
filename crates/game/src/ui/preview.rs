//! Create a Class weapon previews: the class's gun, with its attachments and
//! camo, rendered in 3D to a texture that the menu draws in place of the
//! weapon's 2D picture. Drag it with the mouse to turn it.
//!
//! The gun is put together, attachments, camo and all, by
//! [`crate::gunmodel`]. A spec starting `char:` shows a character instead
//! ([`super::figures`]).

use crate::content::Content;
use crate::gunmodel::{CamoCache, CamoMaterial, GunAssets, GunTarget};
use crate::models::Skeleton;
use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use iw3::zone::{ParseOptions, Zone};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Render layers for the previews start here (one per preview).
const FIRST_LAYER: usize = 8;
/// Previews at most: two per custom class, the popups' one, the three
/// supply drop cards and characters.
const MAX_SLOTS: usize = 20;
/// Texture height at least; the width follows the menu item's shape.
const TARGET_HEIGHT: u32 = 512;
/// Larger previews (the characters) render this much finer than their size
/// on screen, so they stay sharp at high resolutions, up to a limit.
const SUPERSAMPLE: f32 = 1.5;
const MAX_TARGET: u32 = 2048;
const FOV: f32 = 0.35;
/// Starting view: a three-quarter angle from slightly above, muzzle towards
/// the viewer.
const DEFAULT_YAW: f32 = 0.35;
const DEFAULT_PITCH: f32 = 0.2;
/// Radians per pixel dragged.
const DRAG_SPEED: f32 = 0.01;
/// Tilt limit: steeper, the gun's ends leave the small picture frame.
const MAX_PITCH: f32 = 0.6;

enum Loading {
    NotStarted,
    /// `common_mp` (weapon defs and models) parsing on a background thread.
    Running(Arc<iw3::iwd::Vfs>, JoinHandle<anyhow::Result<Zone>>),
    Ready(Box<Content>),
    Failed,
}

struct Slot {
    /// The weapon stat this preview shows (201: class 1 primary, ...).
    key: i32,
    image: Handle<Image>,
    camera: Entity,
    pivot: Entity,
    model: Option<Entity>,
    /// (weapon def, camo) on show.
    showing: Option<(String, usize)>,
    yaw: f32,
    pitch: f32,
    /// Where the menu drew it this frame, in window pixels.
    rect: Option<(Vec2, Vec2)>,
}

struct Request {
    key: i32,
    weapon: String,
    camo: usize,
    pos: Vec2,
    size: Vec2,
}

#[derive(Default)]
pub struct GunPreviews {
    loading: Option<Loading>,
    /// One light rig shared by every preview's layer.
    lights: Vec<Entity>,
    camos: CamoCache,
    slots: Vec<Slot>,
    requests: Vec<Request>,
    /// The preview being dragged and the last cursor position.
    drag: Option<(i32, Vec2)>,
    /// The preview drawn last (on top) the latest frame.
    top: Option<i32>,
    figures: super::figures::Figures,
    /// Black Ops' guns' content, loaded when one is first shown.
    bo1: crate::bo1::MatchContent,
    /// World at War's guns' content, likewise.
    waw: crate::waw::MatchContent,
    /// Modern Warfare 2's guns' content, likewise.
    mw2: crate::mw2guns::MatchContent,
}

impl GunPreviews {
    /// Start loading weapon content in the background.
    pub fn start(&mut self, vfs: Arc<iw3::iwd::Vfs>) {
        if self.loading.is_some() {
            return;
        }
        let task = std::thread::spawn(|| {
            let install = iw3::Install::locate()?;
            let data = iw3::fastfile::load(&install.zone_path("common_mp"))?;
            Zone::parse(&data, ParseOptions::default())
        });
        self.loading = Some(Loading::Running(vfs, task));
    }

    fn content(&mut self) -> Option<&mut Content> {
        loaded(&mut self.loading)
    }

    /// Ask for a preview this frame. Returns its texture once the gun is on
    /// show; until then the menu draws the 2D picture.
    pub fn request(&mut self, key: i32, weapon: &str, camo: usize, pos: Vec2, size: Vec2) -> Option<Handle<Image>> {
        self.requests.push(Request { key, weapon: weapon.to_owned(), camo, pos, size });
        let slot = self.slots.iter().find(|s| s.key == key)?;
        slot.showing.as_ref().filter(|(w, _)| w == weapon).map(|_| slot.image.clone())
    }

    /// Is `weapon` on show in preview `key` (not still loading)?
    pub fn shown(&self, key: i32, weapon: &str) -> bool {
        self.slots.iter().any(|s| s.key == key && s.showing.as_ref().is_some_and(|(w, _)| w == weapon))
    }

    /// Build preview `key`'s gun again (its custom camo was edited).
    pub fn refresh(&mut self, key: i32) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.key == key) {
            s.showing = None;
        }
    }

    /// Mouse dragging: returns true while a preview is being turned.
    pub fn drag(&mut self, cursor: Option<Vec2>, pressed: bool, just_pressed: bool, over_item: bool) -> bool {
        let Some(p) = cursor else { return self.drag.is_some() };
        if !pressed {
            self.drag = None;
            return false;
        }
        if just_pressed && !over_item {
            let hit = self.slots.iter().find(|s| {
                s.rect.is_some_and(|(pos, size)| p.x >= pos.x && p.y >= pos.y && p.x < pos.x + size.x && p.y < pos.y + size.y)
            });
            self.drag = hit.map(|s| (s.key, p));
        }
        let Some((key, last)) = self.drag else { return false };
        if let Some(s) = self.slots.iter_mut().find(|s| s.key == key) {
            let d = p - last;
            s.yaw += d.x * DRAG_SPEED;
            s.pitch = (s.pitch + d.y * DRAG_SPEED).clamp(-MAX_PITCH, MAX_PITCH);
        }
        self.drag = Some((key, p));
        true
    }

    /// Where the first preview on show was drawn (for the debug aid).
    pub fn first_rect(&self) -> Option<(Vec2, Vec2)> {
        self.slots.iter().find_map(|s| s.rect)
    }

    /// Turn the preview drawn on top (a popup's over the class's) as a drag
    /// of `d` pixels would: the pad's right stick.
    pub fn turn_top(&mut self, d: Vec2) {
        if let Some(s) = self.slots.iter_mut().find(|s| Some(s.key) == self.top && s.rect.is_some()) {
            s.yaw += d.x * DRAG_SPEED;
            s.pitch = (s.pitch + d.y * DRAG_SPEED).clamp(-MAX_PITCH, MAX_PITCH);
        }
    }

    /// Weapon content is still loading.
    pub fn busy(&self) -> bool {
        matches!(self.loading, Some(Loading::Running(..)))
            || self.figures.busy()
            || matches!(&self.bo1, crate::bo1::MatchContent::Loading(t) if !t.is_finished())
            || self.waw.busy()
            || self.mw2.busy()
    }

    /// Remove every preview entity (leaving the menus).
    pub fn clear(&mut self, commands: &mut Commands) {
        // (Leaving a match, they may be gone already: `crate::session`.)
        for s in self.slots.drain(..) {
            commands.entity(s.camera).try_despawn();
            commands.entity(s.pivot).try_despawn();
        }
        for e in self.lights.drain(..) {
            commands.entity(e).try_despawn();
        }
        self.requests.clear();
        self.drag = None;
        self.loading = None;
        self.camos = CamoCache::default();
        self.figures = Default::default();
        self.bo1 = Default::default();
        self.waw = Default::default();
        self.mw2 = Default::default();
    }
}

/// The weapon content, once loaded.
fn loaded(loading: &mut Option<Loading>) -> Option<&mut Content> {
    if let Some(Loading::Running(_, task)) = loading {
        if task.is_finished() {
            let Some(Loading::Running(vfs, task)) = loading.replace(Loading::Failed) else { unreachable!() };
            match task.join() {
                Ok(Ok(zone)) => *loading = Some(Loading::Ready(Box::new(Content::new(vec![zone], vfs)))),
                Ok(Err(e)) => warn!("ui: weapon previews unavailable: {e:#}"),
                Err(_) => warn!("ui: weapon preview loading panicked"),
            }
        }
    }
    match loading {
        Some(Loading::Ready(c)) => Some(c),
        _ => None,
    }
}

/// Build what this frame's requests need: cameras, render targets and guns.
pub fn update(
    commands: &mut Commands,
    previews: &mut GunPreviews,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    camo_materials: &mut Assets<CamoMaterial>,
    cameras: &mut Query<&mut Camera>,
    transforms: &mut Query<&mut Transform>,
) {
    let requests = std::mem::take(&mut previews.requests);
    previews.top = requests.last().map(|r| r.key);
    for s in &mut previews.slots {
        s.rect = None;
    }
    if previews.content().is_none() {
        return;
    }
    for r in requests {
        let size = target_size(r.size);
        let index = match previews.slots.iter().position(|s| s.key == r.key) {
            Some(i) => i,
            None if previews.slots.len() < MAX_SLOTS => {
                if previews.lights.is_empty() {
                    previews.lights = spawn_lights(commands);
                }
                let layer = FIRST_LAYER + previews.slots.len();
                previews.slots.push(new_slot(commands, images, r.key, size, layer));
                previews.slots.len() - 1
            }
            None => continue,
        };
        if let Some(mut img) = images.get_mut(&previews.slots[index].image) {
            if img.size() != size {
                img.resize(Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 });
            }
        }
        let wanted = (r.weapon.clone(), r.camo);
        if previews.slots[index].showing.as_ref() != Some(&wanted) {
            // Characters wait for their zone to load, Black Ops' and World at
            // War's guns for their content.
            let gun = |is: fn(&str) -> bool| r.weapon.split('|').any(|p| is(crate::gunmodel::parse(p.trim_start_matches("gun:")).0));
            let (bo1_gun, waw_gun, mw2_gun) = (gun(crate::bo1::is_bo1), gun(crate::waw::is_waw), gun(crate::mw2guns::is_mw2));
            if !previews.figures.ready(&r.weapon)
                || (bo1_gun && previews.bo1.get().is_none())
                || (waw_gun && previews.waw.get().is_none())
                || (mw2_gun && previews.mw2.get().is_none())
            {
                previews.slots[index].rect = Some((r.pos, r.size));
                continue;
            }
            let layer = FIRST_LAYER + index;
            let (pivot, camera, old) = {
                let s = &mut previews.slots[index];
                (s.pivot, s.camera, s.model.take())
            };
            if let Some(old) = old {
                commands.entity(old).despawn();
            }
            let mut assets = GunAssets { meshes, materials, images, bindposes, camo_materials };
            let gun = if r.weapon.starts_with("char:") {
                let Some(common) = loaded(&mut previews.loading) else { continue };
                let (figures, camos) = (&mut previews.figures, &mut previews.camos);
                // The gun's own game's content, if not CoD4's.
                let other = match (previews.bo1.get().filter(|_| bo1_gun), previews.waw.get().filter(|_| waw_gun), previews.mw2.get().filter(|_| mw2_gun)) {
                    (Some(c), ..) | (_, Some(c), _) | (.., Some(c)) => Some(c),
                    _ => None,
                };
                super::figures::spawn(commands, figures, common, other, camos, &mut assets, &r.weapon, layer, pivot, FOV)
            } else {
                spawn_gun(commands, previews, &mut assets, &r.weapon, r.camo, layer, pivot)
            };
            if gun.is_none() {
                warn!("ui: preview of {} made nothing", r.weapon);
            }
            let s = &mut previews.slots[index];
            s.showing = Some(wanted);
            if let Some((model, distance)) = gun {
                s.model = Some(model);
                let framed = Transform::from_xyz(0.0, 0.0, distance).looking_at(Vec3::ZERO, Vec3::Y);
                if r.weapon.starts_with("char:") {
                    // Through a command: a slot made this frame has no camera yet.
                    commands.entity(camera).insert(framed);
                } else if let Ok(mut tf) = transforms.get_mut(camera) {
                    *tf = framed;
                }
            }
        }
        previews.slots[index].rect = Some((r.pos, r.size));
    }
    for s in &previews.slots {
        if let Ok(mut cam) = cameras.get_mut(s.camera) {
            let on = s.rect.is_some();
            if cam.is_active != on {
                cam.is_active = on;
            }
        }
        if let Ok(mut tf) = transforms.get_mut(s.pivot) {
            tf.rotation = Quat::from_rotation_x(s.pitch) * Quat::from_rotation_y(s.yaw);
        }
    }
}

/// Render target size for a menu rect (window pixels): its shape, at least
/// [`TARGET_HEIGHT`] tall, finer for big rects; heights step by 64 so window
/// resizes don't reallocate it every frame.
fn target_size(rect: Vec2) -> UVec2 {
    let aspect = (rect.x / rect.y.max(1.0)).clamp(0.5, 4.0);
    let height = ((rect.y * SUPERSAMPLE / 64.0).ceil() as u32 * 64).clamp(TARGET_HEIGHT, MAX_TARGET);
    let width = ((height as f32 * aspect).round() as u32).min(MAX_TARGET * 2);
    UVec2::new(width, height)
}

fn new_slot(commands: &mut Commands, images: &mut Assets<Image>, key: i32, size: UVec2, layer: usize) -> Slot {
    let image = images.add(Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None));
    let layers = RenderLayers::layer(layer);
    let camera = commands
        .spawn((
            Name::new(format!("gun preview {key}")),
            Camera3d::default(),
            Camera { order: -20 + layer as isize, clear_color: ClearColorConfig::Custom(Color::NONE), ..default() },
            RenderTarget::Image(image.clone().into()),
            Projection::from(PerspectiveProjection { fov: FOV, near: 0.01, far: 50.0, ..default() }),
            Tonemapping::AgX,
            AmbientLight { brightness: 250.0, ..default() },
            Transform::from_xyz(0.0, 0.0, 2.0).looking_at(Vec3::ZERO, Vec3::Y),
            layers.clone(),
        ))
        .id();
    let pivot = commands.spawn((Name::new("gun preview pivot"), Transform::default(), Visibility::default(), layers)).id();
    Slot { key, image, camera, pivot, model: None, showing: None, yaw: DEFAULT_YAW, pitch: DEFAULT_PITCH, rect: None }
}

/// Key light from the upper left, a dimmer fill and a rim light behind, on
/// every preview layer.
fn spawn_lights(commands: &mut Commands) -> Vec<Entity> {
    let layers = RenderLayers::from_layers(&(FIRST_LAYER..FIRST_LAYER + MAX_SLOTS).collect::<Vec<_>>());
    [(Vec3::new(-0.5, -0.6, -0.8), 5000.0), (Vec3::new(0.8, -0.1, -0.5), 1500.0), (Vec3::new(0.2, -0.4, 1.0), 3000.0)]
        .map(|(dir, lux)| {
            commands
                .spawn((
                    Name::new("gun preview light"),
                    DirectionalLight { illuminance: lux, shadow_maps_enabled: false, ..default() },
                    Transform::default().looking_to(dir, Vec3::Y),
                    layers.clone(),
                ))
                .id()
        })
        .to_vec()
}

/// Spawn a gun under `pivot`, centred. Returns the model and a camera
/// distance that frames it.
fn spawn_gun(
    commands: &mut Commands,
    previews: &mut GunPreviews,
    a: &mut GunAssets,
    spec: &str,
    camo: usize,
    layer: usize,
    pivot: Entity,
) -> Option<(Entity, f32)> {
    // Black Ops' and World at War's guns come from their own content.
    let weapon = crate::gunmodel::parse(spec).0;
    let content = if crate::bo1::is_bo1(weapon) {
        previews.bo1.get()?
    } else if crate::waw::is_waw(weapon) {
        previews.waw.get()?
    } else if crate::mw2guns::is_mw2(weapon) {
        previews.mw2.get()?
    } else {
        loaded(&mut previews.loading)?
    };
    let layers = RenderLayers::layer(layer);
    let owner = commands.spawn((Name::new(spec.to_owned()), Visibility::default(), layers.clone(), ChildOf(pivot))).id();
    let mut skeleton = Skeleton::default();
    let target = GunTarget { owner, attach_to: None, layers: Some(layers) };
    let Some((lo, hi)) = crate::gunmodel::spawn_gun(commands, content, &mut previews.camos, a, &mut skeleton, spec, camo, target)
    else {
        commands.entity(owner).despawn();
        return None;
    };
    let (center, half) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
    commands.entity(owner).insert((skeleton, facing_left(center)));

    // Fit the gun's length to the width (2:1 target) and its height to the
    // height, with some margin.
    let (tan_v, tan_h) = ((FOV * 0.5).tan(), (FOV * 0.5).tan() * 2.0);
    let distance = (half.x / tan_h).max(half.y / tan_v) / 0.85 + half.z;
    Some((owner, distance))
}

/// Models face +X; turn them around so the barrel points left, as in the
/// game's own weapon pictures, centred on the pivot.
fn facing_left(center: Vec3) -> Transform {
    let turn = Quat::from_rotation_y(std::f32::consts::PI);
    Transform::from_rotation(turn).with_translation(-(turn * center))
}
