//! The palette's pictures: each model drawn once, on its own, into a small
//! image (a camera of its own, lit evenly, on a render layer nothing else
//! uses), a couple at a time as the palette shows them. Kept for the match.

use super::layout::Prepared;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

/// Render layers of the two thumbnail slots (past the players' bodies).
const LAYERS: [usize; 2] = [44, 45];
const SIZE: u32 = 128;
/// Frames a thumbnail's camera stays: the first can come out black while
/// the model's pipelines are still compiling.
const FRAMES: u32 = 12;

#[derive(Resource, Default)]
pub struct Thumbs {
    pub images: HashMap<String, Handle<Image>>,
    queue: VecDeque<String>,
    queued: HashSet<String>,
    busy: [Option<Busy>; 2],
}

struct Busy {
    entities: Vec<Entity>,
    frames: u32,
}

impl Thumbs {
    /// Asks for `name`'s picture (no-op once it has one or is waiting).
    pub fn want(&mut self, name: &str) {
        if !self.images.contains_key(name) && self.queued.insert(name.to_owned()) {
            self.queue.push_back(name.to_owned());
        }
    }

    /// Nothing waiting or drawing.
    pub fn idle(&self) -> bool {
        self.queue.is_empty() && self.busy.iter().all(Option::is_none)
    }

    /// Puts `name` back at the end of the queue (its model's on the way).
    pub fn later(&mut self, name: String) {
        if self.queued.insert(name.clone()) {
            self.queue.push_back(name);
        }
    }

    /// Drops what's waiting (another page shown).
    pub fn clear_queue(&mut self) {
        for n in self.queue.drain(..) {
            self.queued.remove(&n);
        }
    }

    /// Everything gone (the match ended).
    pub fn reset(&mut self, commands: &mut Commands) {
        for b in self.busy.iter_mut().filter_map(Option::take) {
            for e in b.entities {
                commands.entity(e).try_despawn();
            }
        }
        *self = Thumbs::default();
    }
}

/// Finishes thumbnails done drawing; returns the free slots, each with the
/// next model wanted (to be prepared and handed to [`start`]).
pub fn step(commands: &mut Commands, thumbs: &mut Thumbs) -> Vec<(usize, String)> {
    let mut next = Vec::new();
    for slot in 0..2 {
        if let Some(b) = thumbs.busy[slot].as_mut() {
            b.frames += 1;
            if b.frames >= FRAMES {
                for e in thumbs.busy[slot].take().unwrap().entities {
                    commands.entity(e).try_despawn();
                }
            }
            continue;
        }
        let Some(name) = thumbs.queue.pop_front() else { continue };
        thumbs.queued.remove(&name);
        next.push((slot, name));
    }
    next
}

/// Draws `name` (prepared as `p`, `None` if it can't be) in `slot`.
pub fn start(commands: &mut Commands, thumbs: &mut Thumbs, images: &mut Assets<Image>, slot: usize, name: String, p: Option<Prepared>) {
    {
        let Some(p) = p else {
            // Nothing to draw: a blank, so it isn't asked for again.
            thumbs.images.insert(name, Handle::default());
            return;
        };
        let layer = RenderLayers::layer(LAYERS[slot]);
        let image = images.add(Image::new_target_texture(SIZE, SIZE, bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb, None));
        // Far below the map, so nothing else is near (only this layer is drawn anyway).
        let at = Vec3::new(slot as f32 * 200.0, -3000.0, 0.0);
        let root = commands.spawn((Name::new(format!("thumbnail {name}")), Transform::from_translation(at), Visibility::default())).id();
        for (material, mesh) in &p.parts {
            commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), Transform::default(), layer.clone(), bevy::light::NotShadowCaster, ChildOf(root)));
        }
        let centre = at + (p.min + p.max) * 0.5;
        let radius = ((p.max - p.min).length() * 0.5).max(0.05);
        let from = centre + Vec3::new(1.0, 0.65, 1.2).normalize() * radius * 2.7;
        let camera = commands
            .spawn((
                Name::new("map editor thumbnail camera"),
                Camera3d::default(),
                Camera { order: -40 - slot as isize, clear_color: ClearColorConfig::Custom(Color::srgb(0.10, 0.11, 0.12)), ..default() },
                bevy::camera::RenderTarget::Image(image.clone().into()),
                Projection::Perspective(PerspectiveProjection { fov: 45f32.to_radians(), near: 0.01, ..default() }),
                Transform::from_translation(from).looking_at(centre, Vec3::Y),
                layer,
                Msaa::Off,
                // Lit evenly by its own light (a sun would be taken for the
                // map's by the systems that look for one).
                AmbientLight { color: Color::WHITE, brightness: 2500.0, affects_lightmapped_meshes: true },
            ))
            .id();
        thumbs.images.insert(name, image);
        thumbs.busy[slot] = Some(Busy { entities: vec![root, camera], frames: 0 });
    }
}
