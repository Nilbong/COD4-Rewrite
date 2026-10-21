//! A free-flying camera for screenshots and editors (the Map Look Editor's
//! camera mode, a map editor's): player 1's world camera leaves the player
//! and flies, the HUD and viewmodel hidden, the world still running. The
//! player stands where they were and gets the camera back on exit.
//!
//! Use: [`Flycam::enter`] / [`Flycam::exit`] (or [`Flycam::toggle`]) on the
//! resource, [`active`] anywhere. F8 toggles it in a match.
//!
//! Keys: WASD, Space/E up, Ctrl/Q down, mouse to look, wheel the speed,
//! Shift faster. Pad: left stick moves, right stick looks, RT up, LT down,
//! LB/RB the speed, L3 faster.

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct FlycamPlugin;

impl Plugin for FlycamPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Flycam>()
            .add_systems(OnExit(crate::state::GameState::InGame), |mut f: ResMut<Flycam>| f.exit())
            .add_systems(Update, (keys, hide_viewmodel, hide_ui).chain().run_if(crate::state::in_game))
            .add_systems(
                PostUpdate,
                fly.after(crate::player::follow_camera).before(bevy::transform::TransformSystems::Propagate).run_if(crate::state::in_game),
            );
    }
}

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The fly camera has player 1's view (their input doesn't reach the game,
/// the HUD isn't drawn).
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// The fly camera's state.
#[derive(Resource)]
pub struct Flycam {
    /// Wanted on (applied next frame).
    want: bool,
    on: bool,
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    /// Metres a second.
    pub speed: f32,
    /// Editors' way (`crate::map_editor`): the mouse pointer stays free for
    /// clicking, and the camera only looks, moves and changes speed while
    /// the right button is held (letters free for the editor's keys).
    pub hold_to_fly: bool,
    /// Where to start flying, if not from the camera ([`Flycam::enter_at`]).
    at: Option<(Vec3, f32, f32)>,
}

impl Default for Flycam {
    fn default() -> Self {
        Flycam { want: false, on: false, pos: Vec3::ZERO, yaw: 0.0, pitch: 0.0, speed: 6.0, hold_to_fly: false, at: None }
    }
}

impl Flycam {
    /// Flies from where the camera is now.
    pub fn enter(&mut self) {
        self.want = true;
    }

    /// Back to the player.
    pub fn exit(&mut self) {
        self.want = false;
        self.hold_to_fly = false;
        ACTIVE.store(false, Ordering::Relaxed);
    }

    /// Flies from a given place: Bevy position, yaw and pitch (radians).
    pub fn enter_at(&mut self, pos: Vec3, yaw: f32, pitch: f32) {
        self.want = true;
        self.at = Some((pos, yaw, pitch));
    }

    pub fn toggle(&mut self) {
        if self.want { self.exit() } else { self.enter() }
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Where the camera is (Bevy space) while flying.
    pub fn position(&self) -> Vec3 {
        self.pos
    }
}

fn keys(keys: Res<ButtonInput<KeyCode>>, mut f: ResMut<Flycam>, time: Res<Time<Real>>, mut debug: Local<u32>) {
    if keys.just_pressed(KeyCode::F8) && !f.hold_to_fly && crate::dev::allow("the fly camera (F8)") {
        f.toggle();
    }
    // Debug (screenshots): `COD4RW_FLYCAM=x,y,z,yaw,pitch` (CoD units and
    // degrees, pitch up positive) flies from there a few seconds in.
    if *debug != crate::atmos::climate::match_id() && time.elapsed_secs() > 3.0 {
        *debug = crate::atmos::climate::match_id();
        if let Some(v) = std::env::var("COD4RW_FLYCAM").ok().map(|s| s.split(',').filter_map(|x| x.trim().parse::<f32>().ok()).collect::<Vec<_>>()) {
            if let [x, y, z, yaw, pitch] = v[..] {
                f.enter_at(crate::units::pos([x, y, z]), crate::units::yaw_from_cod_degrees(yaw), pitch.to_radians());
            }
        }
    }
}

/// A UI root that stays up while flying (an editor's panel); every other
/// root (the HUD, the hints, the minimap) is hidden meanwhile.
#[derive(Component)]
pub struct KeepUi;

/// The viewmodel out of sight while flying (back when done). (Its camera
/// draws nothing meanwhile too: `ui::scope`, which owns its layers.)
fn hide_viewmodel(f: Res<Flycam>, mut roots: Query<&mut Visibility, With<crate::viewmodel::ViewModelRoot>>, mut was: Local<bool>) {
    if f.on == *was && !f.on {
        return;
    }
    for mut v in &mut roots {
        let want = if f.on { Visibility::Hidden } else { Visibility::Inherited };
        if *v != want {
            *v = want;
        }
    }
    *was = f.on;
}

/// The UI hidden while flying, but for [`KeepUi`] roots; shown again after.
fn hide_ui(
    f: Res<Flycam>,
    mut roots: Query<(Entity, &mut Visibility), (With<Node>, Without<ChildOf>, Without<KeepUi>)>,
    mut hidden: Local<Vec<(Entity, Visibility)>>,
) {
    if f.on {
        for (e, mut v) in &mut roots {
            if *v != Visibility::Hidden {
                hidden.push((e, *v));
                *v = Visibility::Hidden;
            }
        }
    } else if !hidden.is_empty() {
        for (e, old) in hidden.drain(..) {
            if let Ok((_, mut v)) = roots.get_mut(e) {
                *v = old;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fly(
    time: Res<Time<Real>>,
    mut f: ResMut<Flycam>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    pads: Query<&Gamepad>,
    mut camera: Query<&mut Transform, With<crate::player::MainCamera>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(mut tf) = camera.single_mut() else { return };
    // Entering: from the camera's pose now; the mouse is the camera's.
    if f.want && !f.on {
        f.on = true;
        match f.at.take() {
            Some((pos, yaw, pitch)) => (f.pos, f.yaw, f.pitch) = (pos, yaw, pitch),
            None => {
                f.pos = tf.translation;
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                (f.yaw, f.pitch) = (yaw, pitch);
            }
        }
        if !f.hold_to_fly {
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
        ACTIVE.store(true, Ordering::Relaxed);
        info!("flycam: on at {:?}", f.pos);
    }
    if !f.want && f.on {
        f.on = false;
        ACTIVE.store(false, Ordering::Relaxed);
        info!("flycam: off");
    }
    if !f.on {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    // Hold-to-fly: the pointer's locked only while the right button is.
    let flying = !f.hold_to_fly || buttons.pressed(MouseButton::Right);
    if f.hold_to_fly {
        let mode = if flying { CursorGrabMode::Locked } else { CursorGrabMode::None };
        if cursor.grab_mode != mode {
            cursor.grab_mode = mode;
            cursor.visible = !flying;
        }
    }
    let pad = pads.iter().next();
    let axis = |a: GamepadAxis| pad.and_then(|p| p.get(a)).filter(|v| v.abs() > 0.15).unwrap_or(0.0);
    let held = |b: GamepadButton| pad.is_some_and(|p| p.pressed(b));
    let pressed = |b: GamepadButton| pad.is_some_and(|p| p.just_pressed(b));
    // Look: the mouse while it's the game's, and the right stick.
    if cursor.grab_mode != CursorGrabMode::None && flying {
        f.yaw -= motion.delta.x * 0.0022;
        f.pitch -= motion.delta.y * 0.0022;
    }
    f.yaw -= axis(GamepadAxis::RightStickX) * 2.2 * dt;
    f.pitch += axis(GamepadAxis::RightStickY) * 1.6 * dt;
    f.pitch = f.pitch.clamp(-1.55, 1.55);
    // Speed: the wheel (and LB/RB), Shift (L3) for a burst.
    let notches = if flying { scroll.delta.y } else { 0.0 } + pressed(GamepadButton::RightTrigger) as i32 as f32 - pressed(GamepadButton::LeftTrigger) as i32 as f32;
    if notches != 0.0 {
        f.speed = (f.speed * 1.25f32.powf(notches)).clamp(0.5, 200.0);
    }
    let boost = if keys.pressed(KeyCode::ShiftLeft) || held(GamepadButton::LeftThumb) { 4.0 } else { 1.0 };
    let key = |k: KeyCode| (flying && keys.pressed(k)) as i32 as f32;
    let rot = Quat::from_euler(EulerRot::YXZ, f.yaw, f.pitch, 0.0);
    let forward = rot * Vec3::NEG_Z;
    let right = rot * Vec3::X;
    let up = (key(KeyCode::Space) + key(KeyCode::KeyE) - key(KeyCode::ControlLeft) - key(KeyCode::KeyQ))
        + pad.and_then(|p| p.get(GamepadButton::RightTrigger2)).unwrap_or(0.0)
        - pad.and_then(|p| p.get(GamepadButton::LeftTrigger2)).unwrap_or(0.0);
    let ahead = key(KeyCode::KeyW) - key(KeyCode::KeyS) + axis(GamepadAxis::LeftStickY);
    let side = key(KeyCode::KeyD) - key(KeyCode::KeyA) + axis(GamepadAxis::LeftStickX);
    let step = (forward * ahead + right * side + Vec3::Y * up).clamp_length_max(1.0) * f.speed * boost * dt;
    f.pos += step;
    tf.translation = f.pos;
    tf.rotation = rot;
}
