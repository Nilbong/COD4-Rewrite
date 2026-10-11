//! The gun's weight: a physically driven viewmodel, worked out once a frame
//! per local player for the camera ([`crate::player::follow_camera`]) and
//! the viewmodel ([`crate::viewmodel::weapon_angles`]) to add on top of
//! CoD4's own sway and bob.
//!
//! The gun hangs on springs (one per axis, for where it sits and how it's
//! turned), a little under critically damped, with a mass by the weapon's
//! class: a pistol answers quickly, a machine gun or a sniper rifle drags.
//! What moves it:
//! - looking around: the gun trails the view and overshoots a touch as it
//!   catches up;
//! - moving: it lags as you start and stop (the body's acceleration), sits
//!   back against the way you're going, tilts into strafes and dips when
//!   you change direction;
//! - jumping and landing: it lifts as you leave the ground and drops when
//!   you land, by how hard;
//! - crouching and going prone: a dip as the eye drops, and a settle;
//! - the stride: a bob with the steps, heavier at a sprint (the sprint
//!   animations carry the gun), and a slow breath when still;
//! - firing: each shot kicks the same springs, so recoil and sway mix.
//! All of it fades to a tenth while aiming down the sights (and the springs
//! stiffen), so aiming stays steady. It only moves the gun model: the view,
//! the crosshair and where bullets go are the camera's, unchanged.
//!
//! The camera keeps its own small dips at footfalls and landings.
//! `COD4RW_FP_WEIGHT=0..2` scales the gun's movement (1 by default).

use crate::movement::{Mover, Stance, ViewAngles};
use crate::units::INCH;
use crate::weapons::WeaponState;
use bevy::prelude::*;

pub(super) fn build(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (attach, update)
            .chain()
            .before(crate::player::follow_camera)
            .before(crate::viewmodel::weapon_angles)
            .run_if(crate::state::in_game),
    );
}

/// What a local player's camera and gun add this frame.
#[derive(Component, Default, Debug)]
pub struct Feel {
    /// The camera: up (metres), pitch up and roll (radians).
    pub cam_up: f32,
    pub cam_pitch: f32,
    pub cam_roll: f32,
    /// The gun: angles (CoD degrees: pitch down, yaw left, roll right side
    /// down) and shift (CoD inches: forward, left, up).
    pub gun_angles: Vec3,
    pub gun_shift: Vec3,
    state: State,
}

#[derive(Default, Debug)]
struct State {
    last_view: Option<Vec2>,
    /// Velocity last frame (inches a second, view frame: forward, left, up).
    last_vel: Option<Vec3>,
    last_eye: Option<f32>,
    was_on_ground: bool,
    last_vy: f32,
    last_shots: Option<u32>,
    /// The gun's springs: where it sits (forward, left, up inches) and how
    /// it's turned (pitch down, yaw left, roll degrees), and their speeds.
    pos: Vec3,
    pos_v: Vec3,
    rot: Vec3,
    rot_v: Vec3,
    breath: f32,
    /// The camera's footfall and landing dips (0..), and their speeds.
    step: f32,
    step_vel: f32,
    land: f32,
    land_vel: f32,
    last_step: i32,
    kick_side: f32,
}

/// The springs' natural frequency for a gun of mass 1 (radians a second)
/// and their damping ratio (under 1: a slight overshoot).
const OMEGA: f32 = 13.0;
const ZETA: f32 = 0.62;
/// Running speed in CoD units a second: movement's full strength.
const RUN_SPEED: f32 = 190.0;

/// A gun's mass by its class (`weapClass`): how slowly it answers.
fn mass(w: &WeaponState) -> f32 {
    if w.def.ads_overlay.is_some() {
        return 1.35; // sniper rifles
    }
    match w.def.class {
        4 => 0.6,  // pistol
        2 => 0.8,  // SMG
        3 => 1.1,  // shotgun
        1 => 1.45, // machine gun
        6 => 1.5,  // rocket launcher
        _ => 1.0,  // rifle
    }
}

/// `COD4RW_FP_WEIGHT`: how strongly the gun moves (0..2, default 1).
fn strength() -> f32 {
    static S: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *S.get_or_init(|| std::env::var("COD4RW_FP_WEIGHT").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0f32).clamp(0.0, 2.0))
}

fn attach(mut commands: Commands, players: Query<Entity, (With<crate::splitscreen::LocalSlot>, Without<Feel>)>) {
    for e in &players {
        commands.entity(e).insert(Feel::default());
    }
}

/// A spring towards zero: stiffness and damping ratio.
fn spring(x: &mut f32, v: &mut f32, k: f32, zeta: f32, dt: f32) {
    let c = 2.0 * zeta * k.sqrt();
    // Semi-implicit Euler, in small steps for stiff springs.
    let steps = ((dt / 0.004).ceil() as usize).clamp(1, 16);
    let h = dt / steps as f32;
    for _ in 0..steps {
        *v += (-k * *x - c * *v) * h;
        *x += *v * h;
    }
}

/// A spring of each axis towards `target`.
fn spring3(x: &mut Vec3, v: &mut Vec3, target: Vec3, k: f32, zeta: f32, dt: f32) {
    for i in 0..3 {
        let mut d = x[i] - target[i];
        spring(&mut d, &mut v[i], k, zeta, dt);
        x[i] = d + target[i];
    }
}

fn update(time: Res<Time>, mut players: Query<(&Mover, &ViewAngles, &WeaponState, &mut Feel)>) {
    let dt = time.delta_secs().min(0.05);
    if dt <= 0.0 {
        return;
    }
    let (sway_on, bob_on) = (super::sway(), super::rich_bob());
    let weight = strength();
    for (mover, view, weapon, mut feel) in &mut players {
        let ads = weapon.ads.clamp(0.0, 1.0);
        let m = mass(weapon);
        // Aiming: a tenth of everything, on stiffer springs.
        let amp = (1.0 - 0.9 * ads) * weight;
        let s = &mut feel.state;
        let mut pos_target = Vec3::ZERO;
        let mut rot_target = Vec3::ZERO;

        // The view's turn, CoD degrees a second (pitch down, yaw left).
        let now = Vec2::new(view.pitch, view.yaw);
        let turn = s.last_view.map_or(Vec2::ZERO, |l| {
            let d = now - l;
            let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            Vec2::new(-wrap(d.x), wrap(d.y)).map(f32::to_degrees) / dt
        });
        s.last_view = Some(now);
        // A scoped sniper rifle doesn't sway while scoped (its overlay).
        let scoped = weapon.def.ads_overlay.is_some() && ads > 0.0;
        if sway_on && !scoped {
            // The gun trails the turn: turned back against it, held back a
            // little off-centre, rolled into it. Heavier guns trail more.
            // None at all aimed in: the sights stay on the camera (the user
            // didn't want them dragging behind it).
            let hip = (1.0 - ads).powi(2);
            let lag = (turn * 0.012 * m * hip).clamp(Vec2::splat(-7.0), Vec2::splat(7.0));
            rot_target += Vec3::new(-lag.x, -lag.y, lag.y * 0.35);
            pos_target += Vec3::new(0.0, -lag.y * 0.12, lag.x * 0.1);
        }

        // Movement, in the view's frame (forward, left, up), inches/s.
        let (yaw_s, yaw_c) = view.yaw.sin_cos();
        let v = mover.velocity / INCH;
        let local = Vec3::new(-(v.x * yaw_s + v.z * yaw_c), -(v.x * yaw_c - v.z * yaw_s), v.y);
        let speed = Vec2::new(local.x, local.y).length();
        if bob_on {
            let accel = s.last_vel.map_or(Vec3::ZERO, |l| (local - l) / dt);
            let flat = Vec3::new(accel.x, accel.y, 0.0);
            // Inertia: pushed against the body's acceleration as you start,
            // stop and turn, with a dip as the direction changes.
            s.pos_v -= flat * dt * 0.05 * m * amp;
            s.pos_v.z -= flat.length() * dt * 0.02 * m * amp;
            s.rot_v.x += flat.length() * dt * 0.08 * m * amp;
            // Held back against the way you're going, tilted into strafes.
            let run = (speed / RUN_SPEED).min(1.5);
            pos_target += Vec3::new(-local.x * 0.004, -local.y * 0.006, -run * 0.25) * amp;
            rot_target.z += (-local.y / RUN_SPEED).clamp(-1.5, 1.5) * 3.5 * amp;
            rot_target.y += (local.y / RUN_SPEED).clamp(-1.5, 1.5) * 1.2 * amp;

            // The stride: a bob with the steps, heavier when sprinting.
            let stance_k = match mover.stance {
                Stance::Stand => if mover.sprinting { 1.7 } else { 1.0 },
                Stance::Crouch => 0.7,
                Stance::Prone => 1.2,
            };
            let walk = if mover.on_ground { run.min(1.2) * stance_k } else { 0.0 };
            let c = mover.bob_angle();
            pos_target += Vec3::new(0.0, c.sin() * 0.35, (2.0 * c).sin().abs() * -0.45) * walk * amp;
            rot_target += Vec3::new((2.0 * c).sin() * 0.7, c.sin() * 0.6, c.sin() * 1.3) * walk * amp;
            if mover.stance == Stance::Prone {
                rot_target.y += c.sin() * 1.5 * run.min(1.0) * amp;
            }
            // (Sprinting: CoD4's sprint animations carry the gun lower; the
            // heavier bob above is all that's added.)

            // Breathing, while still (and, a little, aiming).
            s.breath += dt;
            let still = 1.0 - run.min(1.0);
            let b = (s.breath * std::f32::consts::TAU * 0.24).sin();
            let breath = still * weight * (1.0 - 0.75 * ads);
            rot_target.x += b * 0.35 * breath;
            pos_target.z += b * 0.06 * breath;

            // Jumping and landing.
            if s.was_on_ground && !mover.on_ground && mover.velocity.y > 0.5 {
                s.pos_v.z -= 22.0 * m * amp;
                s.rot_v.x -= 35.0 * amp;
            }
            if !s.was_on_ground && mover.on_ground {
                let fall = (-s.last_vy).clamp(0.0, 9.0);
                s.pos_v.z -= fall * 6.0 * m * amp;
                s.rot_v.x += fall * 16.0 * amp;
                s.land_vel -= 28.0 * (fall / 6.0).min(1.5);
            }
            // Crouching and going prone: the gun lags the eye as it drops or
            // rises.
            if let Some(last) = s.last_eye {
                let eye_v = (mover.eye_height - last) / INCH / dt;
                s.pos_v.z -= eye_v * 0.05 * m * amp;
                s.rot_v.x += -eye_v * 0.25 * amp;
            }
        } else {
            s.breath = 0.0;
        }
        s.last_vel = Some(local);
        s.last_eye = Some(mover.eye_height);
        s.was_on_ground = mover.on_ground;
        s.last_vy = mover.velocity.y;

        // Firing: a kick into the same springs, less for a heavier gun.
        let shots = weapon.shots_fired_total;
        if let Some(last) = s.last_shots {
            if shots > last && (sway_on || bob_on) {
                let n = (shots - last).min(3) as f32;
                // Which way it kicks sideways: varied shot to shot.
                s.kick_side = (s.kick_side * 7.31 + 1.13).fract();
                let kick = n * weight * (1.0 - 0.8 * ads) / m.sqrt();
                s.rot_v.x -= 30.0 * kick;
                s.rot_v.y += 12.0 * kick * (s.kick_side * 2.0 - 1.0);
                s.pos_v.x -= 30.0 * kick;
            }
        }
        s.last_shots = Some(shots);

        // The springs: heavier is slower; aiming stiffens them.
        let omega = OMEGA / m.sqrt() * (1.0 + 1.5 * ads);
        let k = omega * omega;
        spring3(&mut s.pos, &mut s.pos_v, pos_target, k, ZETA, dt);
        spring3(&mut s.rot, &mut s.rot_v, rot_target, k, ZETA, dt);
        // Never so far the gun leaves the view.
        s.pos = s.pos.clamp(Vec3::splat(-3.0), Vec3::splat(3.0));
        s.rot = s.rot.clamp(Vec3::splat(-14.0), Vec3::splat(14.0));

        // The camera: small dips at footfalls and landings, a slight roll.
        let (mut cam_up, mut cam_pitch, mut cam_roll) = (0.0, 0.0, 0.0);
        if bob_on {
            let run = (mover.xy_speed_units() / RUN_SPEED).clamp(0.0, 1.5);
            let stance_k = match mover.stance {
                Stance::Stand => if mover.sprinting { 1.5 } else { 1.0 },
                Stance::Crouch => 0.7,
                Stance::Prone => 1.2,
            };
            let walk = if mover.on_ground { run * stance_k } else { 0.0 };
            let step = (mover.bob_cycle * 2.0).floor() as i32;
            if step != s.last_step && mover.on_ground && mover.stance != Stance::Prone && run > 0.15 {
                s.step_vel -= 9.0 * walk;
            }
            s.last_step = step;
            spring(&mut s.step, &mut s.step_vel, 260.0, 0.7, dt);
            spring(&mut s.land, &mut s.land_vel, 110.0, 0.55, dt);
            let keep = 1.0 - 0.7 * ads;
            let dip = s.step + s.land;
            cam_up += crate::units::u(dip * 0.9) * keep;
            cam_pitch += dip * 0.006 * keep;
            cam_roll += (mover.bob_angle().sin() * 0.25 * walk.min(1.2) * keep).to_radians();
        } else {
            s.step = 0.0;
            s.step_vel = 0.0;
            s.land = 0.0;
            s.land_vel = 0.0;
        }
        let (pos, rot) = (s.pos, s.rot);
        feel.cam_up = cam_up;
        feel.cam_pitch = cam_pitch;
        feel.cam_roll = cam_roll;
        feel.gun_angles = rot;
        feel.gun_shift = pos;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn springs_settle() {
        let (mut x, mut v) = (1.0, 0.0);
        for _ in 0..240 {
            spring(&mut x, &mut v, 100.0, 0.7, 1.0 / 60.0);
        }
        assert!(x.abs() < 1e-3 && v.abs() < 1e-2);
        // Under-damped: it passes zero once on the way.
        let (mut x, mut v, mut crossed) = (1.0, 0.0, false);
        for _ in 0..60 {
            spring(&mut x, &mut v, 100.0, 0.55, 1.0 / 60.0);
            crossed |= x < 0.0;
        }
        assert!(crossed);
    }
}
