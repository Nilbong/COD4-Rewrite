//! Explosions shake the camera (CoD4's `earthquake`): each local view by
//! the strongest shake reaching it, a mix of three sines on pitch, yaw and
//! roll, fading over its length and with distance from where it went off.
//!
//! Frags shake 0.3 for 0.5 s out to 400 units (`grenade_earthQuake`), C4
//! 0.4 for 0.5 s out to 512 (`c4_earthQuake`); other code sends a
//! [`Quake`] of its own (an airstrike's is 0.7, 0.75 s, 1000 units).

use crate::splitscreen::SlotCamera;
use crate::units::u;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

pub struct QuakePlugin;

impl Plugin for QuakePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Quake>()
            .init_resource::<Quakes>()
            .add_systems(First, undo)
            .add_systems(Update, (from_blasts, start).chain().run_if(crate::state::in_game))
            .add_systems(PostUpdate, shake.before(TransformSystems::Propagate))
            .add_systems(OnExit(crate::state::GameState::InGame), |mut q: ResMut<Quakes>| q.active.clear());
    }
}

/// A shake: how hard (0..1), for how long (s), out to what radius (CoD
/// units), from where.
#[derive(Message, Clone, Copy, Debug)]
pub struct Quake {
    pub at: Vec3,
    pub scale: f32,
    pub length: f32,
    pub radius: f32,
}

/// At most this many shakes at once; a new one takes the weakest's place.
const MAX_QUAKES: usize = 4;
/// Peak turn, in degrees, on pitch, yaw and roll at full strength.
const PEAK: Vec3 = Vec3::new(18.0, 16.0, 10.0);
/// How fast each turns, radians a second.
const RATE: Vec3 = Vec3::new(41.9, 78.5, 62.8);

#[derive(Resource, Default)]
struct Quakes {
    active: Vec<(Quake, f32)>,
    /// A random start for the sines, picked anew while nothing shakes.
    phase: f32,
    /// The turn put on each camera this frame, taken off next frame.
    applied: Vec<(Entity, Quat)>,
}

/// The equipment and grenades' shakes, by weapon.
fn from_blasts(mut blasts: MessageReader<crate::explosives::Exploded>, mut quakes: MessageWriter<Quake>) {
    for b in blasts.read() {
        let (scale, length, radius) = match b.weapon.as_str() {
            "frag_grenade_mp" => (0.3, 0.5, 400.0),
            "c4_mp" => (0.4, 0.5, 512.0),
            _ => continue,
        };
        quakes.write(Quake { at: b.at, scale, length, radius });
    }
}

fn start(time: Res<Time>, mut new: MessageReader<Quake>, mut quakes: ResMut<Quakes>) {
    let now = time.elapsed_secs();
    quakes.active.retain(|(q, t)| now - t < q.length);
    for q in new.read().filter(|q| q.scale > 0.0 && q.length > 0.0 && q.radius > 0.0) {
        if quakes.active.len() >= MAX_QUAKES {
            // The weakest where it went off goes.
            if let Some(i) = (0..quakes.active.len()).min_by(|&a, &b| {
                let (qa, qb) = (quakes.active[a].0, quakes.active[b].0);
                (qa.scale * (1.0 - (now - quakes.active[a].1) / qa.length)).total_cmp(&(qb.scale * (1.0 - (now - quakes.active[b].1) / qb.length)))
            }) {
                quakes.active.swap_remove(i);
            }
        }
        quakes.active.push((*q, now));
    }
}

/// How hard the shakes shake a view at `eye` now: the strongest's
/// strength there, and its fade.
fn strength(active: &[(Quake, f32)], eye: Vec3, now: f32) -> (f32, f32) {
    let mut best = (0.0, 0.0);
    for (q, t) in active {
        let left = 1.0 - (now - t) / q.length;
        if left <= 0.0 {
            continue;
        }
        let fade = left * q.scale;
        let near = 1.0 - eye.distance(q.at) / u(q.radius);
        let size = if near < 0.0 { near / fade } else { near * fade };
        if size > best.0 {
            best = (size, fade);
        }
    }
    (best.0.min(1.0), best.1)
}

fn shake(time: Res<Time>, mut quakes: ResMut<Quakes>, mut cameras: Query<(Entity, &mut Transform), With<SlotCamera>>) {
    let now = time.elapsed_secs();
    let quakes = &mut *quakes;
    quakes.applied.clear();
    if quakes.active.is_empty() {
        quakes.phase = rand::random_range(-std::f32::consts::PI..std::f32::consts::PI);
        return;
    }
    for (e, mut tf) in &mut cameras {
        let (size, fade) = strength(&quakes.active, tf.translation, now);
        if size <= 0.0 {
            continue;
        }
        let turn = (Vec3::splat(quakes.phase) + RATE * now).map(f32::sin) * PEAK * fade * size;
        let q = Quat::from_euler(EulerRot::YXZ, turn.y.to_radians(), turn.x.to_radians(), turn.z.to_radians());
        tf.rotation *= q;
        quakes.applied.push((e, q));
    }
}

/// Take last frame's shake off, so nothing builds up on a camera that
/// isn't re-aimed every frame.
fn undo(mut quakes: ResMut<Quakes>, mut cameras: Query<&mut Transform, With<SlotCamera>>) {
    for (e, q) in std::mem::take(&mut quakes.applied) {
        if let Ok(mut tf) = cameras.get_mut(e) {
            tf.rotation *= q.inverse();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades_with_distance_and_time() {
        let q = Quake { at: Vec3::ZERO, scale: 0.4, length: 0.5, radius: 512.0 };
        let (near, _) = strength(&[(q, 0.0)], Vec3::ZERO, 0.0);
        let (far, _) = strength(&[(q, 0.0)], Vec3::X * u(400.0), 0.0);
        let (later, _) = strength(&[(q, 0.0)], Vec3::ZERO, 0.4);
        assert!(near > far && far > 0.0 && near > later && later > 0.0, "{near} {far} {later}");
        assert_eq!(strength(&[(q, 0.0)], Vec3::X * u(600.0), 0.0).0, 0.0);
        assert_eq!(strength(&[(q, 0.0)], Vec3::ZERO, 0.6).0, 0.0);
    }
}
