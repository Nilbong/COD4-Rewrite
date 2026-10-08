//! F5: the player seen from behind, over the right shoulder, wearing their
//! character. In first person their body is only seen as its shadow.

use crate::bots::spectate::Spectate;
use crate::loadout::AwaitingClass;
use crate::splitscreen::{LocalSlot, MAX_PLAYERS, PlayerInput, SlotCamera};
use crate::units::u;
use avian3d::prelude::*;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

/// Is the third-person view on? Each local player's.
#[derive(Resource, Default)]
pub struct ThirdPerson(pub [bool; MAX_PLAYERS]);

impl ThirdPerson {
    pub fn on(&self, slot: usize) -> bool {
        self.0.get(slot).copied().unwrap_or(false)
    }
}

/// A local player's body (on its owner): the surfaces to show in third
/// person, and whose it is.
#[derive(Component)]
pub struct LocalBody(pub Vec<Entity>, pub usize);

/// The camera's place behind the eye, in CoD units: behind, right, up.
const BEHIND: f32 = 90.0;
const RIGHT: f32 = 18.0;
const UP: f32 = 10.0;
/// How far it keeps from walls.
const WALL_MARGIN: f32 = 6.0;

/// F5 (pad: View + R3) switches a player's view, except while spectating
/// (which has its own).
pub(super) fn toggle(players: Query<(&LocalSlot, &PlayerInput)>, mut view: ResMut<ThirdPerson>, spectate: Option<Res<Spectate>>) {
    if spectate.is_some() {
        return;
    }
    for (slot, input) in &players {
        if input.live && input.keys.just_pressed(KeyCode::F5) && slot.0 < MAX_PLAYERS {
            view.0[slot.0] = !view.0[slot.0];
            info!("view: {}{}", if view.0[slot.0] { "third person" } else { "first person" }, if slot.0 > 0 { format!(" (player {})", slot.0 + 1) } else { String::new() });
        }
    }
}

/// A player's body: drawn in third person, otherwise only seen by the sun
/// (for its shadow); hidden while picking a class to spawn with. In
/// splitscreen it's on its own layer, which the other players' cameras
/// draw, and its own in third person.
pub(super) fn show_body(
    mut commands: Commands,
    view: Res<ThirdPerson>,
    bodies: Query<(Entity, Ref<LocalBody>)>,
    players: Query<(&LocalSlot, Has<AwaitingClass>)>,
    killcam: Res<crate::killcam::Killcam>,
    mut cameras: Query<(&SlotCamera, &mut RenderLayers)>,
    mut was: Local<[(bool, bool); MAX_PLAYERS]>,
) {
    let split = crate::splitscreen::active();
    for (owner, body) in &bodies {
        let slot = body.1.min(MAX_PLAYERS - 1);
        let waiting = players.iter().find(|p| p.0.0 == slot).is_some_and(|p| p.1);
        // A killcam shows the player's own body too.
        let shown = view.on(slot) || killcam.showing();
        let changed = view.is_changed() || (waiting, killcam.showing()) != was[slot];
        was[slot] = (waiting, killcam.showing());
        // Changed too: a new gun in hand.
        if !changed && !body.is_changed() {
            continue;
        }
        let layers = if split {
            RenderLayers::layer(crate::splitscreen::body_layer(slot))
        } else if shown {
            RenderLayers::layer(0)
        } else {
            RenderLayers::layer(crate::world::SHADOW_PROXY_LAYER)
        };
        for &e in &body.0 {
            commands.entity(e).try_insert(layers.clone());
        }
        commands.entity(owner).insert(if waiting { Visibility::Hidden } else { Visibility::Inherited });
    }
    if split {
        let count = crate::splitscreen::count();
        for (camera, mut layers) in &mut cameras {
            let want = crate::splitscreen::world_layers(camera.0, count, view.on(camera.0));
            if *layers != want {
                *layers = want;
            }
        }
    }
}

/// Pull each third-person camera back from the eye, short of any wall
/// behind.
pub(crate) fn place_camera(
    view: Res<ThirdPerson>,
    spectate: Option<Res<Spectate>>,
    players: Query<&LocalSlot>,
    mut cameras: Query<(&SlotCamera, &mut Transform)>,
    spatial: SpatialQuery,
    killcam: Res<crate::killcam::Killcam>,
    cover: Res<crate::cover::CoverView>,
) {
    if spectate.is_some() {
        return;
    }
    for (camera, mut tf) in &mut cameras {
        let slot = camera.0;
        if !view.on(slot) || !players.iter().any(|s| s.0 == slot) || (slot == 0 && killcam.showing()) {
            continue;
        }
        let eye = tf.translation;
        // 3rd Person TDM: the over-the-shoulder camera ([`crate::cover`]).
        if crate::cover::active() {
            tf.translation = crate::cover::camera_position(eye, tf.rotation, slot, &cover, &spatial);
            continue;
        }
        // The camera looks down -Z: behind is +Z.
        let offset = tf.rotation * Vec3::new(u(RIGHT), u(UP), u(BEHIND));
        let Ok(dir) = Dir3::new(offset) else { continue };
        let len = offset.length();
        let reach = spatial
            .cast_ray(eye, dir, len + u(WALL_MARGIN), true, &crate::collision::sight_filter())
            .map_or(len, |hit| (hit.distance - u(WALL_MARGIN)).clamp(0.0, len));
        tf.translation = eye + dir * reach;
    }
}
