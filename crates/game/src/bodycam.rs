//! Bodycam-style gunplay, toggled against CoD4's with B (or `--bodycam`).
//!
//! Modelled on the game *Bodycam*:
//! * a helmet-mounted camera: a wide lens with barrel distortion, vignette,
//!   chromatic aberration, motion blur, grain and a timestamp overlay. It
//!   rides on the head, so it bobs, nods and jolts with footsteps, landings
//!   and shots;
//! * a weapon that moves freely of the camera: the mouse steers the weapon
//!   (the player's [`ViewAngles`]) and the head only follows it, lagging
//!   behind and keeping it inside a deadzone, so the weapon swings around the
//!   frame. The weapon also has its own inertia: it sways with breathing and
//!   footsteps, kicks back and climbs under recoil and tucks away from walls.
//!   Shots leave along the barrel and there is no crosshair;
//! * heavier movement (slower, slow to build up and to stop) and leaning.
//!
//! The viewmodel is placed relative to the camera from the weapon's own pose
//! in the world, so its sights always show where the bullets will go.

use crate::combat::Dead;
use crate::movement::{Landed, MoveTuning, MovementSet, Mover, RUN_SPEED, Stance, ViewAngles};
use crate::player::{LocalPlayer, MainCamera, ViewModelCamera, hip_fov, look_scale};
use crate::units::u;
use crate::viewmodel::ViewModelRoot;
use crate::weapons::{FreeAim, ShotFired, WeaponDef, WeaponSet, WeaponState};
use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::post_process::effect_stack::{ChromaticAberration, LensDistortion, Vignette};
use bevy::post_process::motion_blur::MotionBlur;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::{ColorGrading, ColorGradingGlobal, ColorGradingSection};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use rand::Rng;

pub struct BodycamPlugin;

impl Plugin for BodycamPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Gunplay>()
            .add_systems(OnEnter(crate::state::GameState::InGame), spawn_overlay.in_set(crate::state::Setup::Spawn))
            .add_systems(
                Update,
                (
                    (toggle_gunplay, apply_gunplay.run_if(resource_changed::<Gunplay>)).chain(),
                    update_handling.after(MovementSet).before(WeaponSet),
                    update_overlay,
                )
                    .run_if(crate::state::in_game),
            )
            .add_systems(
                PostUpdate,
                pose_view
                    .after(crate::player::follow_camera)
                    .after(crate::viewmodel::weapon_angles)
                    .before(TransformSystems::Propagate)
                    .run_if(crate::state::in_game),
            );
    }
}

/// Which game's gunplay the local player uses.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gunplay {
    #[default]
    Cod4,
    Bodycam,
}

impl Gunplay {
    pub fn is_bodycam(self) -> bool {
        self == Gunplay::Bodycam
    }

    /// (hip, fully aimed) vertical field of view in degrees.
    pub fn fovs(self, def: &WeaponDef) -> (f32, f32) {
        match self {
            // A lens scope keeps the view around it a rifle's aimed view
            // (`ui::scope`); the lens shows the gun's own zoom.
            Gunplay::Cod4 if def.ads_overlay.is_some() && crate::ui::lens_scopes() => (hip_fov(), crate::ui::LENS_OUTER_FOV),
            // A 3D scope (an ACOG too): likewise, the eyepiece magnifying.
            Gunplay::Cod4 if crate::ui::scope_3d() && crate::ui::scope_magnifies(def) => (hip_fov(), crate::ui::LENS_OUTER_FOV.max(def.ads_fov)),
            Gunplay::Cod4 => (hip_fov(), def.ads_fov),
            // A wide body-worn lens that barely zooms when aiming.
            Gunplay::Bodycam => (BODYCAM_FOV, BODYCAM_FOV - 5.0),
        }
    }
}

const BODYCAM_FOV: f32 = 80.0;

/// Heavier movement: a slower top speed that takes longer to reach and to
/// shed (CoD4: accelerate 9, friction 5.5).
const MOVEMENT: MoveTuning = MoveTuning { speed_scale: 0.8, accelerate: 5.0, friction: 4.0, sprint_speed_scale: 1.6 };

// --- weapon handling
/// Weapon spring (stiffness, damping) at the hip and fully aimed.
const AIM_SPRING_HIP: (f32, f32) = (120.0, 15.0);
const AIM_SPRING_ADS: (f32, f32) = (220.0, 24.0);
/// The weapon's inertia against the hands steering it (1/s), hip and aimed.
const TURN_LAG_HIP: f32 = 1.5;
const TURN_LAG_ADS: f32 = 0.8;
/// Turning cants the weapon into the turn (1/s).
const TURN_CANT: f32 = 2.5;
/// Furthest the weapon can trail the view: (yaw, pitch, roll) in degrees.
const MAX_AIM_OFFSET: Vec3 = Vec3::new(8.0, 6.0, 12.0);
/// Muzzle impulse per shot in degrees/s: (yaw jitter, climb, roll jitter).
const RECOIL_IMPULSE: Vec3 = Vec3::new(22.0, 38.0, 45.0);
/// Kickback impulse per shot (m/s, towards the shoulder).
const KICKBACK: f32 = 0.9;
const PUSH_SPRING: (f32, f32) = (350.0, 26.0);
/// Share of the weapon's own view kick that moves the view.
const VIEW_KICK: f32 = 1.5;
/// Mechanical spread (degrees) at the hip and aimed; the rest of hip-fire
/// inaccuracy comes from where the weapon is actually pointing.
const SPREAD_HIP: f32 = 0.5;
const SPREAD_ADS: f32 = 0.1;
/// Eye to muzzle, for tucking the weapon away from walls.
const REACH: f32 = u(32.0);
/// Tucked further than this, the weapon can't fire or aim.
const TUCK_BLOCKS: f32 = 0.55;

// --- camera
/// Where the camera sits on the helmet: right, up and forward of the eye
/// (a little left, so the gun at the right shoulder shows to the right).
const HELMET_CAM: Vec3 = Vec3::new(-u(1.5), u(3.0), -u(0.5));
/// How quickly the head turns after the weapon (1/s), hip and aimed. Aiming
/// down sights brings the camera onto the sights.
const HEAD_FOLLOW_HIP: f32 = 1.6;
const HEAD_FOLLOW_ADS: f32 = 6.0;
/// Furthest the weapon can point from the camera's centre (yaw, pitch
/// degrees) before the head is dragged along.
const DEADZONE_HIP: Vec2 = Vec2::new(19.0, 12.0);
const DEADZONE_ADS: Vec2 = Vec2::new(3.0, 2.0);
/// Aiming, the head looks this far (yaw left, pitch up, degrees) off the
/// weapon's aim: the camera is on the helmet, not behind the sights, so the
/// raised gun sits low and right in frame.
const ADS_HEAD_OFFSET: Vec2 = Vec2::new(4.0, 2.0);
/// The head follows faster while moving (per unit of speed, 1/s), so a
/// running gun stays in frame.
const HEAD_FOLLOW_MOVING: f32 = 2.5;
const SHAKE_SPRING: (f32, f32) = (160.0, 22.0);
const SHAKE_POS_SPRING: (f32, f32) = (200.0, 26.0);
/// Camera roll at full lean, degrees.
const LEAN_ROLL: f32 = 12.0;

/// A damped spring on three axes, at rest at zero.
#[derive(Clone, Copy, Default, Debug)]
struct Spring {
    x: Vec3,
    v: Vec3,
}

impl Spring {
    fn step(&mut self, (k, c): (f32, f32), dt: f32) {
        let n = (dt * 240.0).ceil().max(1.0);
        let h = dt / n;
        for _ in 0..n as u32 {
            self.v += (-self.x * k - self.v * c) * h;
            self.x += self.v * h;
        }
    }
}

/// Local player's weapon and camera dynamics while Bodycam gunplay is on.
/// Rotations are (yaw, pitch, roll) in radians, relative to the view (the
/// weapon's aim) or, for the camera, to the head.
#[derive(Component, Default)]
pub struct Handling {
    /// Where the head looks (yaw, pitch); it trails the weapon.
    head: Option<Vec2>,
    aim: Spring,
    /// Weapon translation in view space (kickback).
    push: Spring,
    shake: Spring,
    shake_pos: Spring,
    /// 0..1, how far the weapon is pulled in from a wall.
    tuck: f32,
    step: i32,
    time: f32,
    /// Weapon pose relative to the (leaning) eye and the view rotation.
    gun_pos: Vec3,
    gun_rot: Quat,
    /// Camera motion relative to the helmet mount and the head rotation.
    cam_pos: Vec3,
    cam_rot: Quat,
}

fn toggle_gunplay(keys: Res<ButtonInput<KeyCode>>, mut gunplay: ResMut<Gunplay>, mut overlay: ResMut<Overlay>, time: Res<Time>) {
    // Splitscreen plays CoD4's gunplay ([`crate::splitscreen`]).
    if crate::splitscreen::active() {
        if gunplay.is_bodycam() {
            *gunplay = Gunplay::Cod4;
        }
        return;
    }
    // Debug: `COD4RW_BODYCAM_FLIP=<seconds>` presses B at that many seconds
    // in, and again twice that, for testing the switch both ways.
    let flip = std::env::var("COD4RW_BODYCAM_FLIP").ok().and_then(|v| v.parse::<f32>().ok()).is_some_and(|at| {
        let (t, dt) = (time.elapsed_secs(), time.delta_secs());
        [at, at * 2.0].iter().any(|&m| t >= m && t - dt < m)
    });
    if keys.just_pressed(KeyCode::KeyB) || flip {
        *gunplay = if gunplay.is_bodycam() { Gunplay::Cod4 } else { Gunplay::Bodycam };
        overlay.toast_until = time.elapsed_secs() + 2.0;
    }
}

/// Swap the movement tuning and camera effects for the current gunplay.
fn apply_gunplay(
    mut commands: Commands,
    gunplay: Res<Gunplay>,
    mut tuning: ResMut<MoveTuning>,
    main: Single<Entity, With<MainCamera>>,
    vm: Single<Entity, With<ViewModelCamera>>,
) {
    info!("gunplay: {:?}", *gunplay);
    if !gunplay.is_bodycam() {
        *tuning = MoveTuning::COD4;
        // The motion-vector prepass stays once added: removing it mid-game
        // left Bevy's background motion-vector pipeline cached for the old
        // attachments, and the next frame failed validation (a crash on
        // leaving bodycam).
        commands.entity(*main).remove::<MotionBlur>().insert(ColorGrading::default());
        commands
            .entity(*vm)
            .remove::<(LensDistortion, Vignette, ChromaticAberration)>()
            .insert(ColorGrading::default());
        return;
    }
    *tuning = MOVEMENT;
    // Slightly flat, desaturated video.
    let grading = ColorGrading {
        global: ColorGradingGlobal { post_saturation: 0.85, ..default() },
        midtones: ColorGradingSection { contrast: 1.12, ..default() },
        ..default()
    };
    // Motion blur goes on the world camera only, so the weapon stays sharp.
    commands.entity(*main).insert((MotionBlur { shutter_angle: 0.6, samples: 2 }, grading.clone()));
    // The viewmodel camera draws last into the shared target, so the lens
    // effects on it apply to the whole picture.
    commands.entity(*vm).insert((
        LensDistortion { intensity: 0.3, scale: 1.15, ..default() },
        Vignette { intensity: 0.7, radius: 0.85, smoothness: 4.0, ..default() },
        ChromaticAberration { intensity: 0.012, ..default() },
        grading,
    ));
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_handling(
    mut commands: Commands,
    time: Res<Time>,
    gunplay: Res<Gunplay>,
    tuning: Res<MoveTuning>,
    spatial: SpatialQuery,
    motion: Res<AccumulatedMouseMotion>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    mut shots: MessageReader<ShotFired>,
    mut landed: MessageReader<Landed>,
    mut player: Query<
        (Entity, &Transform, &Mover, &ViewAngles, &WeaponState, Option<&mut Handling>, Option<&mut FreeAim>, Has<Dead>),
        With<LocalPlayer>,
    >,
) {
    let Ok((me, tf, mover, view, weapon, handling, free_aim, dead)) = player.single_mut() else { return };
    let fired = shots.read().filter(|s| s.shooter == me).count();
    let fall = landed.read().filter(|l| l.entity == me).map(|l| l.fall_height).fold(0.0f32, f32::max);
    if !gunplay.is_bodycam() {
        if handling.is_some() {
            commands.entity(me).remove::<(Handling, FreeAim)>();
        }
        return;
    }
    let Some(mut h) = handling else {
        commands.entity(me).insert(Handling::default());
        return;
    };
    if dead {
        *h = Handling::default();
        return;
    }
    let dt = time.delta_secs().min(0.05);
    if dt <= 0.0 {
        return;
    }
    let mut rng = rand::rng();
    let deg = |v: Vec3| v * std::f32::consts::PI / 180.0;
    h.time += dt;
    let t = h.time;
    let ads = weapon.ads;
    let steady = match mover.stance {
        Stance::Stand => 1.0,
        Stance::Crouch => 0.75,
        Stance::Prone => 0.45,
    };

    // The weapon resists being swung around, and cants into the turn.
    let grabbed = cursor.grab_mode != CursorGrabMode::None;
    let look = if grabbed { -motion.delta * look_scale(*gunplay, weapon) } else { Vec2::ZERO };
    let lag = TURN_LAG_HIP + (TURN_LAG_ADS - TURN_LAG_HIP) * ads;
    h.aim.v -= Vec3::new(look.x, look.y, -look.x * TURN_CANT / lag) * lag;

    // Recoil: the muzzle jumps up and sideways, the weapon kicks back into
    // the shoulder and the camera jolts.
    let recoil_scale = (1.0 - 0.4 * ads) * (0.4 + 0.6 * steady);
    for _ in 0..fired {
        let jitter = |rng: &mut rand::rngs::ThreadRng| rng.random_range(-1.0f32..1.0);
        let impulse = Vec3::new(jitter(&mut rng), 0.7 + 0.3 * rng.random::<f32>(), jitter(&mut rng));
        h.aim.v += deg(impulse * RECOIL_IMPULSE) * recoil_scale;
        h.push.v.z += KICKBACK * recoil_scale;
        h.shake.v += deg(Vec3::new(jitter(&mut rng) * 8.0, 20.0, jitter(&mut rng) * 25.0)) * recoil_scale;
        h.shake_pos.v.z += 0.15 * recoil_scale;
    }

    // Footsteps (two per bob cycle) and landings jolt the head, and the
    // weapon less so.
    let full_speed = RUN_SPEED * tuning.speed_scale;
    let speed = (mover.horizontal_speed() / full_speed).min(1.6);
    let step = (mover.bob_cycle * 2.0).floor() as i32;
    if step != h.step && mover.on_ground && speed > 0.1 {
        let side = if step % 2 == 0 { 1.0 } else { -1.0 };
        let s = speed * steady * (1.0 - 0.5 * ads) * if mover.sprinting { 1.6 } else { 1.0 };
        h.shake.v += deg(Vec3::new(side * 2.5, -7.0, side * 5.0)) * s;
        h.shake_pos.v.y -= 0.08 * s;
        h.aim.v += deg(Vec3::new(side * 1.5, -2.0, side * 1.5)) * s.min(1.0);
    }
    h.step = step;
    if fall > 0.0 {
        let f = (fall / 60.0).clamp(0.3, 2.0);
        h.shake.v.y -= 40f32.to_radians() * f;
        h.shake_pos.v.y -= 0.6 * f;
        h.aim.v.y -= 30f32.to_radians() * f;
    }

    let aim_spring = (
        AIM_SPRING_HIP.0 + (AIM_SPRING_ADS.0 - AIM_SPRING_HIP.0) * ads,
        AIM_SPRING_HIP.1 + (AIM_SPRING_ADS.1 - AIM_SPRING_HIP.1) * ads,
    );
    h.aim.step(aim_spring, dt);
    // Moving, the weapon strays less (it stays low in frame).
    let max = deg(MAX_AIM_OFFSET) * (1.0 - 0.4 * speed.min(1.0));
    h.aim.x = h.aim.x.clamp(-max, max);
    h.push.step(PUSH_SPRING, dt);
    h.shake.step(SHAKE_SPRING, dt);
    h.shake_pos.step(SHAKE_POS_SPRING, dt);

    // Continuous motion: walking sway, strafe cant, breathing.
    let phase = mover.bob_cycle * std::f32::consts::TAU;
    let view_rot = view.rotation();
    let right = Quat::from_rotation_y(view.yaw) * Vec3::X;
    let strafe = (mover.velocity.dot(right) / full_speed).clamp(-1.0, 1.0);
    // A steady carry when running: the bob grows with speed only so far.
    let walk = speed.min(1.0) * (1.0 - 0.65 * ads) * if mover.sprinting { 1.2 } else { 1.0 };
    let walk_rot = deg(Vec3::new(phase.sin() * 1.2, (2.0 * phase).sin() * 0.7, 0.0)) * walk
        + deg(Vec3::Z * -strafe * 3.0) * (1.0 - 0.65 * ads);
    let walk_pos = Vec3::new(phase.sin() * u(0.4), -(1.0 - (2.0 * phase).cos()) * u(0.25), 0.0) * walk;
    let breath = deg(Vec3::new(
        (t * 0.9).sin() * 0.3 + (t * 0.53 + 1.0).sin() * 0.2,
        (t * 1.4).sin() * 0.3 + (t * 0.71).sin() * 0.15,
        0.0,
    )) * steady
        * (1.0 - 0.5 * ads);

    // Pull the weapon in when the muzzle would go through a wall.
    let eye = mover.eye(tf.translation) + mover.lean_offset(view.yaw);
    let lean_roll = -mover.lean * LEAN_ROLL.to_radians();
    let aim_rot = euler(h.aim.x + walk_rot + breath);
    let muzzle = view_rot * aim_rot * Vec3::NEG_Z;
    let target = Dir3::new(muzzle)
        .ok()
        .and_then(|d| spatial.cast_ray(eye, d, REACH, true, &crate::collision::sight_filter()))
        .map_or(0.0, |hit| ((REACH - hit.distance) / (REACH - u(10.0))).clamp(0.0, 1.0));
    h.tuck += (target - h.tuck) * (1.0 - (-12.0 * dt).exp());
    let tuck_rot = deg(Vec3::new(10.0, 35.0, 25.0)) * h.tuck;
    let tuck_pos = Vec3::new(0.0, -u(2.0), u(7.0)) * h.tuck;

    // Sprinting: CoD4's sprint animation drops the gun well down, out of a
    // helmet camera's frame: lifted back, so it's carried in the lower
    // right.
    let carry = if mover.sprinting && mover.on_ground { (1.0 - ads) * speed.min(1.0) } else { 0.0 };
    let carry_rot = deg(Vec3::new(-4.0, 6.0, 6.0)) * carry;
    let carry_pos = Vec3::new(u(1.0), u(3.0), -u(1.0)) * carry;
    h.gun_rot = euler(h.aim.x + walk_rot + breath + tuck_rot + carry_rot + Vec3::Z * lean_roll);
    h.gun_pos = h.push.x + walk_pos + tuck_pos + carry_pos;

    // The camera rides on the head: a bob stronger than CoD's with a nod and
    // sway per step, a slow drift, a roll when strafing, and the shake springs.
    let (bob_side, bob_up) = mover.view_bob();
    // The head moves on its own (a person looking about, not a camera bolted
    // to the gun): a slow look-around, a stronger nod and sway per step, and
    // a glance the way you strafe, all fading out while aiming.
    let loose = 1.0 - 0.85 * ads;
    let drift = deg(Vec3::new(
        (t * 0.31).sin() * 0.45 + (t * 0.17 + 1.3).sin() * 0.3,
        (t * 0.47 + 2.0).sin() * 0.25 + (t * 0.23).sin() * 0.15,
        (t * 0.29).sin() * 0.2,
    )) * loose;
    let cam_walk = deg(Vec3::new(phase.sin() * 0.5, (2.0 * phase).sin() * 0.6, phase.sin() * 0.7 - strafe * 1.0)) * speed.min(1.2) * loose;
    let glance = deg(Vec3::X * -strafe * 3.0) * loose;
    h.cam_rot = euler(h.shake.x + drift + cam_walk + glance + Vec3::Z * lean_roll);
    h.cam_pos = h.shake_pos.x + Vec3::new(bob_side, bob_up, 0.0) * 1.3 * (1.0 - 0.6 * ads);

    let aim = FreeAim {
        origin: eye + view_rot * h.gun_pos,
        rotation: view_rot * h.gun_rot,
        spread: SPREAD_HIP + (SPREAD_ADS - SPREAD_HIP) * ads,
        view_kick: VIEW_KICK,
        blocked: h.tuck > TUCK_BLOCKS,
    };
    match free_aim {
        Some(mut a) => *a = aim,
        None => {
            commands.entity(me).insert(aim);
        }
    }
}

/// (yaw, pitch, roll) radians to a view-space rotation.
fn euler(v: Vec3) -> Quat {
    Quat::from_euler(EulerRot::YXZ, v.x, v.y, v.z)
}

/// Turn the head toward where the weapon points (`aim`, yaw/pitch): smoothly,
/// but never letting the weapon leave the deadzone.
fn follow_head(head: Vec2, aim: Vec2, ads: f32, dt: f32) -> Vec2 {
    follow_head_moving(head, aim, ads, 0.0, dt)
}

/// [`follow_head`], moving at `speed` (0..1+ of full speed).
fn follow_head_moving(head: Vec2, aim: Vec2, ads: f32, speed: f32, dt: f32) -> Vec2 {
    use std::f32::consts::{PI, TAU};
    let rate = HEAD_FOLLOW_HIP + (HEAD_FOLLOW_ADS - HEAD_FOLLOW_HIP) * ads + HEAD_FOLLOW_MOVING * speed.min(1.5);
    let aim = aim + ADS_HEAD_OFFSET * (PI / 180.0) * ads;
    let zone = (DEADZONE_HIP + (DEADZONE_ADS - DEADZONE_HIP) * ads) * (PI / 180.0);
    let off = |head: Vec2| Vec2::new((aim.x - head.x + PI).rem_euclid(TAU) - PI, aim.y - head.y);
    let head = head + off(head) * (1.0 - (-rate * dt).exp());
    let d = off(head);
    let over = (d / zone).length();
    if over > 1.0 { aim - d / over } else { head }
}

/// Place the camera and the viewmodel from the handling state, using this
/// frame's final view angles (after recoil).
#[allow(clippy::type_complexity)]
pub(crate) fn pose_view(
    time: Res<Time>,
    gunplay: Res<Gunplay>,
    mut player: Query<
        (&Transform, &Mover, &ViewAngles, &WeaponState, Option<&mut Handling>, Has<Dead>),
        With<LocalPlayer>,
    >,
    mut camera: Single<&mut Transform, (With<MainCamera>, Without<LocalPlayer>, Without<ViewModelRoot>)>,
    mut vm: Query<&mut Transform, (With<ViewModelRoot>, Without<LocalPlayer>, Without<MainCamera>)>,
) {
    let Ok(mut vm_tf) = vm.single_mut() else { return };
    let rest = crate::viewmodel::rest_transform();
    let Ok((tf, mover, view, weapon, handling, dead)) = player.single_mut() else { return };
    let Some(mut h) = handling.filter(|_| gunplay.is_bodycam() && !dead) else {
        // CoD4's own weapon sway ([`crate::viewmodel::weapon_angles`]) sets
        // the rotation and offset.
        return;
    };
    let aim = Vec2::new(view.yaw, view.pitch);
    let speed = (mover.horizontal_speed() / RUN_SPEED).min(1.6);
    let head = follow_head_moving(h.head.unwrap_or(aim), aim, weapon.ads, speed, time.delta_secs());
    h.head = Some(head);
    let head_rot = euler(head.extend(0.0));
    let eye = mover.eye(tf.translation) + mover.lean_offset(view.yaw);
    let view_rot = view.rotation();
    let cam_pos = eye + head_rot * (HELMET_CAM + h.cam_pos);
    let cam_rot = head_rot * h.cam_rot;
    camera.translation = cam_pos;
    camera.rotation = cam_rot;
    // The viewmodel hangs off the camera; undo the camera's pose so the
    // weapon keeps its own in the world.
    let inv = cam_rot.inverse();
    vm_tf.translation = inv * (eye + view_rot * h.gun_pos - cam_pos);
    vm_tf.rotation = inv * view_rot * h.gun_rot * rest.rotation;
}

// --- overlay: film grain, timestamp and the mode toast

#[derive(Resource, Default)]
struct Overlay {
    toast_until: f32,
}

#[derive(Component)]
struct Grain;
#[derive(Component)]
struct Timestamp;
#[derive(Component)]
struct Toast;

const GRAIN_TILE: u32 = 256;

fn spawn_overlay(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.init_resource::<Overlay>();
    let mut rng = rand::rng();
    let n = GRAIN_TILE as usize;
    let mut data = Vec::with_capacity(n * n * 4);
    for _ in 0..n * n {
        let v: u8 = rng.random();
        data.extend_from_slice(&[v, v, v, 12]);
    }
    let image = Image::new(
        Extent3d { width: GRAIN_TILE, height: GRAIN_TILE, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    commands.spawn((
        Grain,
        ImageNode {
            image: images.add(image),
            image_mode: NodeImageMode::Tiled { tile_x: true, tile_y: true, stretch_value: 1.0 },
            ..default()
        },
        Node { position_type: PositionType::Absolute, ..default() },
        GlobalZIndex(-1),
        Visibility::Hidden,
    ));
    let text = |s: &str, size: f32| {
        (Text::new(s), TextFont { font_size: FontSize::Px(size), ..default() }, TextColor(Color::WHITE), TextShadow::default())
    };
    commands.spawn((
        Timestamp,
        text("", 18.0),
        TextLayout::justify(Justify::Right),
        Node { position_type: PositionType::Absolute, right: px(24), top: px(16), ..default() },
        Visibility::Hidden,
    ));
    commands.spawn((
        Toast,
        text("", 22.0),
        TextLayout::justify(Justify::Center),
        Node {
            position_type: PositionType::Absolute,
            bottom: px(170),
            width: percent(100),
            justify_content: JustifyContent::Center,
            ..default()
        },
        Visibility::Hidden,
    ));
}

#[allow(clippy::type_complexity)]
fn update_overlay(
    time: Res<Time>,
    gunplay: Res<Gunplay>,
    overlay: Res<Overlay>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut grain: Single<(&mut Node, &mut Visibility), (With<Grain>, Without<Timestamp>, Without<Toast>)>,
    mut stamp: Single<(&mut Text, &mut Visibility), (With<Timestamp>, Without<Grain>, Without<Toast>)>,
    mut toast: Single<(&mut Text, &mut Visibility), (With<Toast>, Without<Grain>, Without<Timestamp>)>,
) {
    let on = gunplay.is_bodycam();
    let vis = if on { Visibility::Inherited } else { Visibility::Hidden };
    let (node, grain_vis) = &mut *grain;
    **grain_vis = vis;
    *stamp.1 = vis;
    let showing_toast = time.elapsed_secs() < overlay.toast_until;
    *toast.1 = if showing_toast { Visibility::Inherited } else { Visibility::Hidden };
    if showing_toast {
        toast.0.0 = if on { "BODYCAM GUNPLAY" } else { "COD4 GUNPLAY" }.to_owned();
    }
    if !on {
        return;
    }
    // Jump the grain tile around every frame so it reads as moving noise.
    let mut rng = rand::rng();
    let tile = GRAIN_TILE as f32;
    node.left = px(-rng.random_range(0.0..tile));
    node.top = px(-rng.random_range(0.0..tile));
    node.width = px(window.width() + tile);
    node.height = px(window.height() + tile);

    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let s = secs % 86_400;
    stamp.0.0 = format!("{y:04}-{m:02}-{d:02} T{:02}:{:02}:{:02}Z\nBODY CAM  X81", s / 3600, s / 60 % 60, s % 60);
}

/// Days since 1970-01-01 to a (year, month, day) date (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_728), (2026, 10, 2));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn head_trails_the_weapon() {
        let zone = DEADZONE_HIP.x.to_radians();
        // A flick far past the deadzone drags the head to its edge.
        let head = follow_head(Vec2::ZERO, Vec2::new(1.0, 0.0), 0.0, 1.0 / 60.0);
        assert!((1.0 - head.x - zone).abs() < 1e-4, "{head}");
        // Then it catches up (in about three seconds: the head is loose).
        let mut head = head;
        for _ in 0..180 {
            head = follow_head(head, Vec2::new(1.0, 0.0), 0.0, 1.0 / 60.0);
        }
        assert!((head.x - 1.0).abs() < 0.01, "{head}");
        // Across the yaw wrap it takes the short way round.
        let head = follow_head(Vec2::new(3.1, 0.0), Vec2::new(-3.1, 0.0), 0.0, 1.0 / 60.0);
        assert!(head.x > 3.1 || head.x < -3.0, "{head}");
    }

    #[test]
    fn spring_settles() {
        let mut s = Spring { x: Vec3::ZERO, v: Vec3::new(1.0, -2.0, 0.5) };
        for _ in 0..120 {
            s.step(AIM_SPRING_HIP, 1.0 / 60.0);
        }
        assert!(s.x.length() < 1e-3 && s.v.length() < 1e-2, "{s:?}");
    }

    #[test]
    fn bodycam_movement_still_reaches_top_speed() {
        // Quake-style acceleration only reaches the wish speed if it beats friction.
        assert!(MOVEMENT.accelerate >= MOVEMENT.friction);
    }
}
