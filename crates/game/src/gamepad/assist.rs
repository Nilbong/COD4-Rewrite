//! Looking with the right stick, and console-style aim assist for it (never
//! for the mouse):
//!
//! - slowdown: the stick turns slower while the crosshair is on an enemy;
//! - tracking: while you move or aim, the view follows a target under the
//!   crosshair as it (or you) moves;
//! - snap: pulling the aim trigger near an enemy eases the view onto them.
//!
//! Only enemies in sight count (a ray against the world).

use super::{PadFrame, PadSettings};
use crate::bodycam::Gunplay;
use crate::collision;
use crate::combat::{Dead, HitLocation, Hitbox, Pawn};
use crate::movement::{Mover, ViewAngles};
use crate::splitscreen::{LocalSlot, PlayerInput};
use crate::units::u;
use crate::weapons::WeaponState;
use avian3d::prelude::*;
use bevy::prelude::*;
use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

pub struct AssistPlugin;

impl Plugin for AssistPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            stick_look
                .in_set(crate::player::InputSet)
                .before(crate::movement::MovementSet)
                .run_if(crate::state::in_game),
        );
    }
}

/// Stick turn rates in degrees per second at full deflection (hip).
const YAW_RATE: f32 = 220.0;
const PITCH_RATE: f32 = 125.0;
/// Fully aimed, turning slows to this (on top of the zoom).
const ADS_RATE: f32 = 0.6;
/// Holding the stick all the way speeds turning up to this, after a moment.
const TURN_BOOST: f32 = 1.7;
const BOOST_DELAY: f32 = 0.15;
const BOOST_RAMP: f32 = 0.35;

/// A target's assist box from its chest, in CoD units (half sizes).
const BOX_HALF_WIDTH: f32 = 20.0;
const BOX_HALF_HEIGHT: f32 = 34.0;
/// Nothing further than this gets help.
const MAX_RANGE: f32 = 4000.0;
/// Stick speed on a target, hip and aimed.
const SLOWDOWN_HIP: f32 = 0.5;
const SLOWDOWN_ADS: f32 = 0.4;
/// Slowdown fades out to this many box sizes.
const SLOWDOWN_EDGE: f32 = 1.6;
/// Share of a target's movement across the view that the view follows.
const TRACK_HIP: f32 = 0.45;
const TRACK_ADS: f32 = 0.7;
/// Tracking never turns faster than this (degrees per second).
const TRACK_MAX: f32 = 60.0;
/// Aiming snaps to targets within this many box sizes, over this long.
const SNAP_REGION: f32 = 2.6;
const SNAP_TIME: f32 = 0.3;
const SNAP_RATE: f32 = 14.0;

/// A wrapped angle in -PI..PI.
fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

/// The view angles (yaw, pitch) that look along `dir`.
pub fn angles_to(dir: Vec3) -> Vec2 {
    let flat = Vec2::new(dir.x, dir.z).length();
    Vec2::new((-dir.x).atan2(-dir.z), dir.y.atan2(flat))
}

/// An enemy the stick could get help with.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    entity: Entity,
    /// View angles to its chest.
    angles: Vec2,
    /// Distance from the crosshair in assist-box sizes.
    off: f32,
}

#[derive(Default)]
struct Assist {
    /// The target tracked last frame and the angles to it then.
    tracked: Option<(Entity, Vec2)>,
    /// Snapping to a target until a time.
    snap: Option<(Entity, f32)>,
}

/// Each local player's right stick turns their view, with aim assist while
/// they play with a controller.
#[allow(clippy::too_many_arguments)]
fn stick_look(
    time: Res<Time>,
    gunplay: Res<Gunplay>,
    settings: Res<PadSettings>,
    spatial: SpatialQuery,
    mut players: Query<(Entity, &PlayerInput, &Transform, &Mover, &mut ViewAngles, &WeaponState, &Pawn), (With<LocalSlot>, Without<Dead>)>,
    pawns: Query<(Entity, &Pawn), Without<Dead>>,
    hitboxes: Query<(&Hitbox, &GlobalTransform)>,
    mut assists: Local<HashMap<Entity, Assist>>,
) {
    assists.retain(|e, _| players.contains(*e));
    for (entity, input, tf, mover, mut view, weapon, me) in &mut players {
        let assist = assists.entry(entity).or_default();
        if !input.live {
            *assist = Assist::default();
            continue;
        }
        look_one(&time, &gunplay, &settings, &spatial, (input, tf, mover, &mut view, weapon, me), &pawns, &hitboxes, assist);
    }
}

/// One player's stick and aim assist this frame.
#[allow(clippy::too_many_arguments)]
fn look_one(
    time: &Time,
    gunplay: &Gunplay,
    settings: &PadSettings,
    spatial: &SpatialQuery,
    (input, tf, mover, view, weapon, me): (&PlayerInput, &Transform, &Mover, &mut ViewAngles, &WeaponState, &Pawn),
    pawns: &Query<(Entity, &Pawn), Without<Dead>>,
    hitboxes: &Query<(&Hitbox, &GlobalTransform)>,
    assist: &mut Assist,
) {
    let frame: &PadFrame = &input.pad;
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    let ads = weapon.ads;

    // The enemies in the running, nearest the crosshair first.
    let helping = settings.aim_assist && input.pad_kind.is_some();
    let eye = mover.eye(tf.translation);
    let mut candidates = Vec::new();
    if helping {
        let mut chests: HashMap<Entity, (Option<Vec3>, Option<Vec3>)> = HashMap::new();
        for (hb, gt) in hitboxes {
            let e = chests.entry(hb.owner).or_default();
            match hb.location {
                HitLocation::Torso => e.0 = Some(gt.translation()),
                HitLocation::Head => e.1 = Some(gt.translation()),
                HitLocation::Neck | HitLocation::Legs => {}
            }
        }
        for (entity, pawn) in pawns {
            if !crate::combat::hostile(pawn, me) {
                continue;
            }
            let Some(&(Some(torso), head)) = chests.get(&entity) else { continue };
            let chest = head.map_or(torso, |h| torso.lerp(h, 0.35));
            let to = chest - eye;
            let dist = to.length();
            if !(u(16.0)..u(MAX_RANGE)).contains(&dist) {
                continue;
            }
            let angles = angles_to(to);
            let d = Vec2::new(wrap(angles.x - view.yaw), angles.y - view.pitch);
            let half = Vec2::new((u(BOX_HALF_WIDTH) / dist).atan(), (u(BOX_HALF_HEIGHT) / dist).atan());
            let off = (d / half).length();
            if off < SNAP_REGION.max(SLOWDOWN_EDGE) {
                candidates.push((Candidate { entity, angles, off }, chest, dist));
            }
        }
        candidates.sort_by(|a, b| a.0.off.total_cmp(&b.0.off));
    }
    // The nearest one in sight.
    let filter = collision::sight_filter();
    let target = candidates.iter().find(|(_, chest, dist)| {
        Dir3::new(*chest - eye).is_ok_and(|dir| spatial.cast_ray(eye, dir, dist - u(4.0), true, &filter).is_none())
    });
    let target = target.map(|t| t.0);

    // Slowdown on a target.
    let slow = match target {
        Some(t) if t.off < SLOWDOWN_EDGE => {
            let on = SLOWDOWN_HIP + (SLOWDOWN_ADS - SLOWDOWN_HIP) * ads;
            let fade = ((t.off - 1.0) / (SLOWDOWN_EDGE - 1.0)).clamp(0.0, 1.0);
            on + (1.0 - on) * fade
        }
        _ => 1.0,
    };

    // The stick itself.
    let (hip_fov, _) = gunplay.fovs(weapon.def);
    let zoom = crate::player::view_fov(*gunplay, weapon) / hip_fov;
    let boost = 1.0 + (TURN_BOOST - 1.0) * ((frame.look_pinned - BOOST_DELAY) / BOOST_RAMP).clamp(0.0, 1.0) * (1.0 - ads);
    let rate = settings.sensitivity * zoom * (1.0 + (ADS_RATE * settings.ads_sensitivity - 1.0) * ads) * slow;
    let invert = if settings.invert_pitch { -1.0 } else { 1.0 };
    view.yaw -= frame.look.x * (YAW_RATE * boost).to_radians() * rate * dt;
    view.pitch += frame.look.y * PITCH_RATE.to_radians() * rate * dt * invert;

    // Tracking: follow the target's drift across the view while the player
    // is moving or aiming.
    let input = frame.movement.length() > 0.1 || frame.look.length() > 0.05;
    match (target, assist.tracked) {
        (Some(t), Some((e, last))) if e == t.entity && t.off < SLOWDOWN_EDGE && input => {
            let strength = TRACK_HIP + (TRACK_ADS - TRACK_HIP) * ads;
            let fade = 1.0 - (t.off / SLOWDOWN_EDGE).clamp(0.0, 1.0) * 0.5;
            let drift = Vec2::new(wrap(t.angles.x - last.x), t.angles.y - last.y) * strength * fade;
            let max = TRACK_MAX.to_radians() * dt;
            view.yaw += drift.x.clamp(-max, max);
            view.pitch += drift.y.clamp(-max, max);
        }
        _ => {}
    }
    assist.tracked = target.map(|t| (t.entity, t.angles));

    // Snap when the aim trigger is pulled; the stick takes over if pushed.
    if helping && frame.ads_pressed {
        assist.snap = target.filter(|t| t.off < SNAP_REGION).map(|t| (t.entity, now + SNAP_TIME));
        if let Some(t) = target {
            debug!("aim assist: aiming {:.1} boxes from {:?}{}", t.off, t.entity, if assist.snap.is_some() { ", snapping" } else { "" });
        }
    }
    if let Some((e, until)) = assist.snap {
        let current = candidates.iter().find(|c| c.0.entity == e && Some(e) == target.map(|t| t.entity));
        match current {
            Some((c, ..)) if now < until && frame.ads && frame.look.length() < 0.5 => {
                let k = 1.0 - (-SNAP_RATE * dt).exp();
                view.yaw += wrap(c.angles.x - view.yaw) * k;
                view.pitch += (c.angles.y - view.pitch) * k;
            }
            _ => {
                if let Some((c, ..)) = current {
                    debug!("aim assist: snap ended {:.2} deg off", Vec2::new(wrap(c.angles.x - view.yaw), c.angles.y - view.pitch).length().to_degrees());
                }
                assist.snap = None;
            }
        }
    }
    view.pitch = view.pitch.clamp(-85f32.to_radians(), 85f32.to_radians());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_round_trip_through_the_view() {
        for (yaw, pitch) in [(0.0, 0.0), (1.0, 0.3), (-2.5, -0.7), (3.0, 1.2)] {
            let dir = ViewAngles { yaw, pitch }.forward();
            let a = angles_to(dir);
            assert!(wrap(a.x - yaw).abs() < 1e-4 && (a.y - pitch).abs() < 1e-4, "{yaw} {pitch} -> {a}");
        }
    }

    #[test]
    fn wrap_stays_in_range() {
        assert!((wrap(3.0 * PI) - PI).abs() < 1e-4 || (wrap(3.0 * PI) + PI).abs() < 1e-4);
        assert!((wrap(0.5) - 0.5).abs() < 1e-6);
        assert!((wrap(-TAU + 0.25) - 0.25).abs() < 1e-5);
    }
}
