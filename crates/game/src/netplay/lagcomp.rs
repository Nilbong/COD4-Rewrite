//! Lag compensation: a friend's shots are traced against everyone where the
//! friend saw them, not where they are on the host by the time the shot
//! arrives.
//!
//! A guest draws everyone else a little in the past (the interpolation
//! delay) and its shot reaches the host a little later still (its latency).
//! Each of its controls says which host tick it was showing
//! ([`InputCommand::view_tick`]); the host keeps a second of every pawn's
//! poses ([`PoseHistory`]) and, for that friend's bullets, tests the
//! hitboxes where they were then ([`ShotTrace`]). The rewind (latency and
//! interpolation delay together) is capped at [`MAX_REWIND`], so a very
//! laggy player can't shoot people well behind cover.
//!
//! [`InputCommand::view_tick`]: cod4rw_multiplayer::protocol::InputCommand::view_tick

use super::TICK_RATE;
use crate::combat::{Dead, Hitbox, Pawn};
use crate::movement::{Mover, Stance, ViewAngles};
use avian3d::prelude::*;
use bevy::prelude::*;
use std::collections::VecDeque;

/// The furthest back a shot is traced (s): the shooter's latency and their
/// interpolation delay together.
pub const MAX_REWIND: f32 = 0.25;
/// How much pose history each pawn keeps (s).
const KEPT: f32 = 1.0;

/// Where a pawn was, tick by tick (host).
#[derive(Component, Default)]
pub struct PoseHistory(VecDeque<Pose>);

#[derive(Clone, Copy)]
struct Pose {
    tick: f32,
    feet: Vec3,
    yaw: f32,
    stance: Stance,
    lean: f32,
}

impl PoseHistory {
    /// The pose at `tick`, between the two recorded around it.
    fn at(&self, tick: f32) -> Option<Pose> {
        let first = self.0.front()?;
        if tick <= first.tick {
            return Some(*first);
        }
        for (a, b) in self.0.iter().zip(self.0.iter().skip(1)) {
            if tick >= a.tick && tick <= b.tick {
                let t = ((tick - a.tick) / (b.tick - a.tick).max(1e-4)).clamp(0.0, 1.0);
                // A respawn or teleport: no sliding across the map.
                if a.feet.distance(b.feet) > 3.0 {
                    return Some(if t < 0.5 { *a } else { *b });
                }
                let dyaw = (b.yaw - a.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                return Some(Pose {
                    tick,
                    feet: a.feet.lerp(b.feet, t),
                    yaw: a.yaw + dyaw * t,
                    stance: if t < 0.5 { a.stance } else { b.stance },
                    lean: a.lean + (b.lean - a.lean) * t,
                });
            }
        }
        self.0.back().copied()
    }
}

/// A friend's shots this frame are traced at this host tick.
#[derive(Component, Clone, Copy, Debug)]
pub struct Rewind {
    pub tick: f32,
}

/// The host tick now.
pub fn now_tick(time: &Time) -> f32 {
    time.elapsed_secs() * TICK_RATE
}

/// The tick a friend's shot is traced at: what they saw, at most
/// [`MAX_REWIND`] back.
pub fn rewind_to(view_tick: u32, now: f32) -> f32 {
    let oldest = now - MAX_REWIND * TICK_RATE;
    // (Ticks are the host's clock, which a guest can't run ahead of.)
    (view_tick as f32).clamp(oldest, now)
}

/// Host: every pawn's pose each frame, after it moved.
pub(super) fn record(time: Res<Time>, mut pawns: Query<(Entity, &Transform, &Mover, &ViewAngles, Option<&mut PoseHistory>), With<Pawn>>, mut commands: Commands) {
    let tick = now_tick(&time);
    for (e, tf, mover, view, history) in &mut pawns {
        let pose = Pose { tick, feet: tf.translation, yaw: view.yaw, stance: mover.stance, lean: mover.lean };
        match history {
            Some(mut h) => {
                if h.0.back().is_some_and(|p| p.tick >= tick) {
                    continue;
                }
                h.0.push_back(pose);
                while h.0.front().is_some_and(|p| p.tick < tick - KEPT * TICK_RATE) {
                    h.0.pop_front();
                }
            }
            None => {
                commands.entity(e).insert(PoseHistory(VecDeque::from([pose])));
            }
        }
    }
}

/// The bullet trace for [`crate::weapons`]: an ordinary ray cast, except a
/// friend's ([`Rewind`]) on the host, which meets the hitboxes where the
/// friend saw them.
#[derive(bevy::ecs::system::SystemParam)]
pub struct ShotTrace<'w, 's> {
    rewinds: Query<'w, 's, &'static Rewind>,
    pawns: Query<'w, 's, (&'static PoseHistory, &'static Children, Has<Dead>)>,
    hitboxes: Query<'w, 's, (&'static Hitbox, &'static Collider)>,
}

impl ShotTrace<'_, '_> {
    /// Where `shooter`'s shot from `origin` first hits (a predicate as
    /// [`SpatialQuery::cast_ray_predicate`]'s).
    #[allow(clippy::too_many_arguments)]
    pub fn cast(&self, spatial: &SpatialQuery, shooter: Entity, origin: Vec3, ray: Dir3, max: f32, filter: &SpatialQueryFilter, keep: &dyn Fn(Entity) -> bool) -> Option<RayHitData> {
        let Ok(rewind) = self.rewinds.get(shooter) else {
            return spatial.cast_ray_predicate(origin, ray, max, true, filter, keep);
        };
        // The world (and aircraft, sentries) as it is; pawns' hitboxes as
        // they were.
        let now = |e: Entity| keep(e) && self.hitboxes.get(e).map_or(true, |(hb, _)| !self.pawns.contains(hb.owner));
        let mut best = spatial.cast_ray_predicate(origin, ray, max, true, filter, &now);
        for (history, children, dead) in &self.pawns {
            let Some(pose) = history.at(rewind.tick).filter(|_| !dead) else { continue };
            let body = Transform { translation: pose.feet, rotation: Quat::from_rotation_y(pose.yaw), ..default() };
            for child in children.iter() {
                let Ok((hb, collider)) = self.hitboxes.get(child) else { continue };
                if !keep(child) {
                    continue;
                }
                let local = crate::combat::hitbox_local(hb.location, pose.stance, pose.lean);
                let at = body * local;
                let limit = best.as_ref().map_or(max, |b| b.distance);
                if let Some((distance, normal)) = collider.cast_ray(at.translation, at.rotation, origin, *ray, limit, true)
                    && distance < limit
                {
                    best = Some(RayHitData { entity: child, distance, normal });
                }
            }
        }
        if let Some(b) = &best
            && let Ok((hb, _)) = self.hitboxes.get(b.entity)
            && let Ok((history, _, _)) = self.pawns.get(hb.owner)
            && let Some(last) = history.0.back()
        {
            debug!("lagcomp: a friend's shot hit {:?} rewound {:.0} ms", hb.location, (last.tick - rewind.tick) / TICK_RATE * 1000.0);
        }
        best
    }

    /// The far side of what a shot went into (`entry`), traced back from
    /// `beyond`; for a rewound hitbox, as it was.
    #[allow(clippy::too_many_arguments)]
    pub fn back(&self, spatial: &SpatialQuery, shooter: Entity, beyond: Vec3, ray: Dir3, max: f32, filter: &SpatialQueryFilter, entry: Entity) -> Option<RayHitData> {
        if let (Ok(rewind), Ok((hb, collider))) = (self.rewinds.get(shooter), self.hitboxes.get(entry))
            && let Ok((history, _, _)) = self.pawns.get(hb.owner)
            && let Some(pose) = history.at(rewind.tick)
        {
            let at = Transform { translation: pose.feet, rotation: Quat::from_rotation_y(pose.yaw), ..default() } * crate::combat::hitbox_local(hb.location, pose.stance, pose.lean);
            return collider.cast_ray(at.translation, at.rotation, beyond, *ray, max, true).map(|(distance, normal)| RayHitData { entity: entry, distance, normal });
        }
        spatial.cast_ray_predicate(beyond, ray, max, true, filter, &|e| e == entry)
    }
}
