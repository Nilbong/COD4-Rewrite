//! The airstrike (`airstrike_mp`, `_hardpoints.gsc`'s `doArtillery`): the
//! caller picks a spot on the map (the player on the full-screen map,
//! [`Selecting`]; bots give one), and two seconds later three MiG-29s
//! (`vehicle_mig29_desert`) cross it from a random heading, 850 units up at
//! 7000 units/s, 1.5-2.5 s apart. Each drops a cluster bomb
//! (`projectile_cbu97_clusterbomb`) a second before it would pass 1500
//! units beyond the spot; 1.05 s later the bomb bursts
//! (`explosions/clusterbomb`), and half a second after that its 12
//! bomblets land along the heading (traces 55 down to 5 degrees below it),
//! each doing 200 falling to 30 damage over 512 units (four times less
//! reach indoors) to whoever it can see. The caller gets the kills
//! (`artillery_mp`). One airstrike at a time.

use super::spawn_static_model;
use crate::combat::{Damage, Dead, HitLocation, Pawn};
use crate::content::Content;
use crate::fx::{Anchor, Effects, FxLayer, Frame};
use crate::units::{self, u};
use avian3d::prelude::SpatialQuery;
use bevy::prelude::*;

/// The weapon kills are credited to (its kill icon is `death_airstrike`).
pub const WEAPON: &str = "artillery_mp";

/// `callStrike`: half the flight, the bomb's lead past the spot, height and
/// speed (units, units/s).
const HALF_PATH: f32 = 24000.0;
const BOMB_LEAD: f32 = 1500.0;
const FLY_HEIGHT: f32 = 850.0;
const FLY_SPEED: f32 = 7000.0;
/// The bomb leaves the plane at the bomb time less a second, falls ahead
/// at 7000/1.5 units/s, and bursts 1.05 s later.
const BOMB_SPEED: f32 = FLY_SPEED / 1.5;
const BOMB_FALL: f32 = 1.05;
/// Bomblets: 12, landing 0.5 s after the burst, 0.05 s apart, along
/// traces from 55 to 5 degrees below the heading.
const BOMBLETS: u32 = 12;
const BOMBLET_DELAY: f32 = 0.5;
/// `losRadiusDamage( traceHit + (0,0,16), 512, 200, 30 )`.
const RADIUS: f32 = 512.0;
const MAX_DAMAGE: f32 = 200.0;
const MIN_DAMAGE: f32 = 30.0;
/// `wait 2` after calling, and the strike's whole time (`wait 8.5` after).
const DELAY: f32 = 2.0;
const IN_PROGRESS: f32 = DELAY + 8.5;
const GRAVITY: f32 = 800.0;

/// The player is picking where on the full-screen map.
#[derive(Resource, Clone, Copy, Debug)]
pub struct Selecting {
    pub owner: Entity,
}

/// Airstrikes under way.
#[derive(Resource, Default)]
pub struct Airstrikes {
    strikes: Vec<Strike>,
}

struct Strike {
    owner: Entity,
    /// The ground point, and the heading (Bevy, horizontal).
    target: Vec3,
    dir: Vec3,
    called: f32,
    /// When each plane starts, and the planes started.
    plane_times: [f32; 3],
    planes: Vec<Plane>,
}

struct Plane {
    entity: Option<Entity>,
    start: Vec3,
    end: Vec3,
    spawned: f32,
    fx: bool,
    bomb: Option<Entity>,
    bomblets: u32,
    burst: Option<Vec3>,
}

impl Airstrikes {
    /// One is under way (`level.airstrikeInProgress`).
    pub fn in_progress(&self, now: f32) -> bool {
        self.strikes.iter().any(|s| now - s.called < IN_PROGRESS)
    }

    /// How dangerous a strike under way makes `at` for spawning
    /// (`getAirstrikeDanger`): from the planes' coming until they're gone,
    /// 1 inside 300 units of a spot pushed 675 units along the heading,
    /// falling to 0 at 450, the circle stretched six times along it; more
    /// than one strike adds up.
    pub fn danger(&self, at: Vec3, now: f32) -> f32 {
        let (near, far, push, stretch) = (300.0, 450.0, 1.5, 6.0);
        self.strikes
            .iter()
            .filter(|s| (DELAY..IN_PROGRESS).contains(&(now - s.called)))
            .map(|s| {
                let center = s.target + s.dir * u(push * far);
                let diff = (at - center).with_y(0.0) / u(1.0);
                let along = diff.dot(s.dir) * s.dir;
                let dist = (diff - along + along / stretch).length();
                if dist > far {
                    0.0
                } else if dist < near {
                    1.0
                } else {
                    1.0 - (dist - near) / (far - near)
                }
            })
            .sum()
    }

    /// Call one on `target` (a ground point, Bevy space).
    pub fn call(&mut self, owner: Entity, target: Vec3, now: f32) {
        let yaw = rand::random::<f32>() * std::f32::consts::TAU;
        let dir = units::dir([yaw.cos(), yaw.sin(), 0.0]);
        let first = now + DELAY;
        let second = first + rand::random_range(1.5..2.5);
        let third = second + rand::random_range(1.5..2.5);
        self.strikes.push(Strike { owner, target, dir, called: now, plane_times: [first, second, third], planes: Vec::new() });
    }
}

/// `doPlaneStrike`'s path, with its randomness: up to 100 units sideways at
/// the start and 150 at the end, and up to `rise` units higher.
fn plane_path(target: Vec3, dir: Vec3, rise: f32) -> (Vec3, Vec3) {
    let jitter = |r: f32| Vec3::new(rand::random_range(-r..r), 0.0, rand::random_range(-r..r)) * u(1.0);
    let up = Vec3::Y * u(FLY_HEIGHT + rand::random_range(0.0..rise));
    (target - dir * u(HALF_PATH) + up + jitter(100.0), target + dir * u(HALF_PATH) + up + jitter(150.0))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    mut commands: Commands,
    time: Res<Time>,
    mut strikes: ResMut<Airstrikes>,
    mut content: Option<ResMut<Content>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut fx: Option<ResMut<Effects>>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut movers: Query<&mut Transform, Without<Pawn>>,
    pawns: Query<(Entity, &Pawn, &Transform), Without<Dead>>,
    spatial: SpatialQuery,
    mut damage: MessageWriter<Damage>,
    mut quakes: MessageWriter<crate::quake::Quake>,
) {
    let now = time.elapsed_secs();
    let fly_time = 2.0 * HALF_PATH / FLY_SPEED;
    let drop_at = (HALF_PATH + BOMB_LEAD) / FLY_SPEED - 1.0;
    let filter = crate::collision::sight_filter();
    for strike in &mut strikes.strikes {
        // Planes due.
        while strike.planes.len() < 3 && now >= strike.plane_times[strike.planes.len()] {
            let rise = if strike.planes.is_empty() { 500.0 } else { 200.0 };
            let (start, end) = plane_path(strike.target, strike.dir, rise);
            let rotation = Quat::from_rotation_arc(Vec3::X, strike.dir);
            let entity = content.as_deref_mut().and_then(|c| {
                spawn_static_model(&mut commands, c, &mut meshes, &mut materials, &mut images, "vehicle_mig29_desert", Transform::from_translation(start).with_rotation(rotation))
            });
            strike.planes.push(Plane { entity, start, end, spawned: now, fx: false, bomb: None, bomblets: 0, burst: None });
        }
        for plane in &mut strike.planes {
            let age = now - plane.spawned;
            let pos = plane.start.lerp(plane.end, (age / fly_time).min(1.0));
            if let Some(e) = plane.entity {
                if age >= fly_time {
                    commands.entity(e).despawn();
                    plane.entity = None;
                } else {
                    if let Ok(mut tf) = movers.get_mut(e) {
                        tf.translation = pos;
                    }
                    // `playPlaneFx`: afterburner and contrails (once it exists).
                    if !plane.fx && age > 0.05 {
                        plane.fx = true;
                        if let Some(fx) = fx.as_deref_mut() {
                            for name in ["fire/jet_afterburner", "smoke/jet_contrail"] {
                                fx.play(name, Anchor::Bolted(e), FxLayer::World);
                            }
                        }
                    }
                }
            }
            // The bomb: dropped, falling ahead, bursting.
            let since_drop = age - drop_at;
            if since_drop < 0.0 {
                continue;
            }
            let launch = plane.start.lerp(plane.end, (drop_at / fly_time).min(1.0));
            let bomb_at = |t: f32| launch + strike.dir * u(BOMB_SPEED) * t - Vec3::Y * u(0.5 * GRAVITY * t * t);
            if plane.bomb.is_none() && plane.burst.is_none() {
                sfx.play("veh_mig29_sonic_boom", Some(launch));
                let rotation = Quat::from_rotation_arc(Vec3::X, strike.dir);
                plane.bomb = content.as_deref_mut().and_then(|c| {
                    spawn_static_model(&mut commands, c, &mut meshes, &mut materials, &mut images, "projectile_cbu97_clusterbomb", Transform::from_translation(launch).with_rotation(rotation))
                });
                if plane.bomb.is_none() {
                    plane.bomb = Some(Entity::PLACEHOLDER);
                }
            }
            if since_drop < BOMB_FALL {
                if let Some(mut tf) = plane.bomb.and_then(|b| movers.get_mut(b).ok()) {
                    tf.translation = bomb_at(since_drop);
                }
                continue;
            }
            let burst = *plane.burst.get_or_insert_with(|| {
                let at = bomb_at(BOMB_FALL);
                if let Some(b) = plane.bomb.take().filter(|&b| b != Entity::PLACEHOLDER) {
                    commands.entity(b).despawn();
                }
                if let Some(fx) = fx.as_deref_mut() {
                    fx.play("explosions/clusterbomb", Anchor::Fixed(Frame::facing(at, strike.dir, 0.0)), FxLayer::World);
                }
                // `callStrike_bombEffect`'s earthquake.
                quakes.write(crate::quake::Quake { at, scale: 0.7, length: 0.75, radius: 1000.0 });
                at
            });
            // The bomblets, one every 0.05 s.
            while plane.bomblets < BOMBLETS && since_drop >= BOMB_FALL + BOMBLET_DELAY + plane.bomblets as f32 * 0.05 {
                let i = plane.bomblets as f32;
                plane.bomblets += 1;
                let pitch = (55.0 - 50.0 / BOMBLETS as f32 * i).to_radians();
                let yaw = rand::random_range(-5f32..5.0).to_radians();
                let flat = Quat::from_rotation_y(yaw) * strike.dir;
                let down = (flat * pitch.cos() - Vec3::Y * pitch.sin()).normalize();
                let Ok(d) = Dir3::new(down) else { continue };
                let Some(hit) = spatial.cast_ray(burst, d, u(10000.0), true, &filter) else { continue };
                let point = burst + down * hit.distance + Vec3::Y * u(16.0);
                if plane.bomblets % 3 == 1 {
                    sfx.play("artillery_impact", Some(point));
                }
                splash(point, strike.owner, &pawns, &spatial, &mut damage);
            }
        }
    }
    strikes.strikes.retain(|s| now - s.called < IN_PROGRESS || s.planes.iter().any(|p| p.entity.is_some() || p.bomblets < BOMBLETS));
}

/// `losRadiusDamage`: everyone the blast can see within the radius, the
/// damage falling with distance; someone with open sky above who can't
/// see the blast from above their head counts as indoors, four times
/// further away.
fn splash(at: Vec3, owner: Entity, pawns: &Query<(Entity, &Pawn, &Transform), Without<Dead>>, spatial: &SpatialQuery, damage: &mut MessageWriter<Damage>) {
    let filter = crate::collision::sight_filter();
    let clear = |from: Vec3, to: Vec3| {
        let d = to - from;
        Dir3::new(d).map_or(true, |dir| spatial.cast_ray(from, dir, d.length() - u(1.0), true, &filter).is_none())
    };
    for (e, _, tf) in pawns {
        let center = tf.translation + Vec3::Y * u(40.0);
        let mut dist = at.distance(center) / u(1.0);
        if dist > RADIUS || !clear(at, center) {
            continue;
        }
        let head = tf.translation + Vec3::Y * u(130.0);
        if clear(tf.translation + Vec3::Y * u(1.0), head) && !clear(head, at + Vec3::Y * u(130.0 - 16.0)) {
            dist *= 4.0;
            if dist > RADIUS {
                continue;
            }
        }
        let amount = MAX_DAMAGE + (MIN_DAMAGE - MAX_DAMAGE) * dist / RADIUS;
        damage.write(Damage { target: e, attacker: Some(owner), amount, location: HitLocation::Torso, weapon: WEAPON });
    }
}
