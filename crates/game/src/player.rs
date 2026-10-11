//! The local player: keyboard/mouse input, camera and viewmodel.

use crate::bodycam::Gunplay;
use crate::combat::Dead;
use crate::movement::{MoveInput, Mover, Stance, ViewAngles};
use crate::units::u;
use crate::weapons::{WeaponInput, WeaponState};
use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::anti_alias::smaa::{Smaa, SmaaPreset};
use bevy::camera::Hdr;
use bevy::light::Skybox;
use bevy::math::cubic_splines::LinearSpline;
use bevy::post_process::auto_exposure::{AutoExposure, AutoExposureCompensationCurve, AutoExposurePlugin};
use bevy::post_process::bloom::Bloom;
use bevy::pbr::ScreenSpaceAmbientOcclusion;
use bevy::prelude::*;
use crate::splitscreen::{LocalSlot, PlayerInput};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutoExposurePlugin);
        override_ssao_shader(app);
        app.add_systems(OnEnter(crate::state::GameState::InGame), spawn_camera.in_set(crate::state::Setup::Spawn))
            .add_systems(
                Update,
                (
                    grab_cursor.run_if(crate::ui::no_ingame_menu),
                    (mouse_look, keyboard_input).in_set(InputSet).before(crate::movement::MovementSet),
                )
                    .run_if(crate::state::in_game),
            )
            .add_systems(
                PostUpdate,
                (place_views, follow_camera).chain().before(TransformSystems::Propagate).run_if(crate::state::in_game),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct InputSet;

/// Swap Bevy's SSAO shader for our tuned copy (stronger, wider occlusion).
/// Must run after `DefaultPlugins` registered the original.
fn override_ssao_shader(app: &mut App) {
    let registry = app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>();
    registry.insert_asset(
        std::path::PathBuf::from(file!()).with_file_name("shaders/ssao.wgsl"),
        std::path::Path::new("bevy_pbr/ssao/ssao.wgsl"),
        include_bytes!("shaders/ssao.wgsl").as_slice(),
    );
}

pub const VIEWMODEL_LAYER: usize = 1;
/// The hip field of view: the settings' (CoD4's 65 by default).
pub fn hip_fov() -> f32 {
    crate::settings_apply::hip_fov()
}
/// Radians per mouse count, roughly CoD's default sensitivity.
const SENSITIVITY: f32 = 0.0022;

#[derive(Component)]
pub struct LocalPlayer;

#[derive(Component)]
pub struct MainCamera;

#[derive(Component)]
pub struct ViewModelCamera;

/// Stance toggles (crouch/prone are toggles in CoD4 by default), each
/// local player's.
#[derive(Resource, Default)]
struct StanceToggle([Stance; crate::splitscreen::MAX_PLAYERS]);

/// Auto-exposure: the average scene luminance is steered toward
/// 2^EXPOSURE_TARGET_EV (about 18% grey), within +-EXPOSURE_RANGE_EV stops.
const EXPOSURE_TARGET_EV: f32 = -2.5;
const EXPOSURE_RANGE_EV: f32 = 3.0;

/// The world camera's ambient occlusion. Objects are taken to be 3 m
/// thick rather than Bevy's 0.25 m, so a car or crate darkens the ground
/// under it (a thin shell lets the ground "see" past it): cars standing in
/// shade, which CoD4 never baked into the lightmaps, sit on the ground.
pub fn ssao() -> bevy::pbr::ScreenSpaceAmbientOcclusion {
    // (`ssao.thickness` in tuning.txt, read as the camera spawns.)
    bevy::pbr::ScreenSpaceAmbientOcclusion { constant_object_thickness: crate::tune::get("ssao.thickness", 3.0), ..default() }
}

/// Sky luminance in cd/m^2 for a fully white sky texel, tuned to sit
/// alongside sunlit surfaces: at 3000 (and 1500, with the exposure set by the
/// darker part of the view) overcast skies (Bloc, Vacant) clipped
/// to white.
const SKY_BRIGHTNESS: f32 = 800.0;

pub fn spawn_camera(
    mut commands: Commands,
    mut curves: ResMut<Assets<AutoExposureCompensationCurve>>,
    sky: Option<Res<crate::world::MapSky>>,
) {
    commands.init_resource::<StanceToggle>();
    let flat = LinearSpline::new([Vec2::new(-16.0, EXPOSURE_TARGET_EV), Vec2::new(16.0, EXPOSURE_TARGET_EV)]);
    let compensation_curve =
        curves.add(AutoExposureCompensationCurve::from_curve(flat).expect("flat exposure compensation curve"));
    let count = crate::splitscreen::count();
    for slot in 0..count {
        spawn_view(&mut commands, slot, count, compensation_curve.clone(), sky.as_deref());
    }
    // Splitscreen: the menus and HUDs over the whole window, blended onto
    // the players' views.
    if count > 1 {
        commands.spawn((
            crate::splitscreen::OverlayCamera,
            Camera2d,
            Camera {
                order: 100,
                clear_color: ClearColorConfig::Custom(Color::NONE),
                output_mode: bevy::camera::CameraOutputMode::Write {
                    blend_state: Some(bevy::render::render_resource::BlendState::ALPHA_BLENDING),
                    clear_color: ClearColorConfig::None,
                },
                ..default()
            },
            bevy::ui::IsDefaultUiCamera,
        ));
    }
}

/// A local player's cameras: the world's, and the viewmodel's drawn over
/// it. Player 1's are the [`MainCamera`] and [`ViewModelCamera`].
fn spawn_view(commands: &mut Commands, slot: usize, count: usize, compensation_curve: Handle<AutoExposureCompensationCurve>, sky: Option<&crate::world::MapSky>) {
    use crate::splitscreen::{SlotCamera, SlotViewModelCamera, viewmodel_layer};
    let split = count > 1;
    let world = commands
        .spawn((
            SlotCamera(slot),
            Camera3d::default(),
            Camera {
                order: 2 * slot as isize,
                clear_color: ClearColorConfig::Custom(Color::srgb(0.55, 0.65, 0.78)),
                // The viewmodel camera draws over this image and writes the
                // finished frame out; this one's own copy out was wasted.
                output_mode: bevy::camera::CameraOutputMode::Skip,
                ..default()
            },
            Projection::from(PerspectiveProjection { fov: hip_fov().to_radians(), near: 0.05, far: 2000.0, ..default() }),
            // HDR, left un-tonemapped: the viewmodel camera draws over this
            // image and then exposes, blooms and tonemaps the whole frame once.
            Hdr,
            Tonemapping::None,
            Msaa::Off,
            // Only the effects' few muzzle-flash lights are clustered: one
            // cluster for them all (the default froxel grid cost the render
            // thread over a millisecond a frame).
            bevy::light::cluster::ClusterConfig::Single,
            DistanceFog {
                color: Color::srgba(0.6, 0.68, 0.78, 1.0),
                falloff: FogFalloff::Linear { start: 120.0, end: 600.0 },
                ..default()
            },
            // CoD skies are looked up with CoD axes (x forward, y left, z up);
            // this maps Bevy's (x, y, z) to CoD's (x, -z, y).
            Skybox {
                image: sky.map(|s| s.0.clone()),
                brightness: SKY_BRIGHTNESS,
                rotation: Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            },
            Transform::default(),
        ))
        .id();
    if slot == 0 {
        commands.entity(world).insert(MainCamera);
    }
    if !split {
        // Horizon-based ambient occlusion (Bevy's GTAO). It needs MSAA
        // off; SMAA on the viewmodel camera anti-aliases instead. Not in
        // splitscreen: Bevy's samples the whole window's depth, not the
        // camera's viewport, so a player's occlusion came out broken.
        commands.entity(world).insert(ScreenSpaceAmbientOcclusion::default());
    }
    if split {
        // The other players' bodies (each player's own is only its shadow,
        // [`crate::wardrobe`]).
        commands.entity(world).insert(crate::splitscreen::world_layers(slot, count, false));
    }
    // The viewmodel is drawn by its own camera so it never clips into walls.
    let vm = commands
        .spawn((
            SlotViewModelCamera(slot),
            Camera3d::default(),
            Camera { order: 2 * slot as isize + 1, clear_color: ClearColorConfig::None, ..default() },
            Projection::from(PerspectiveProjection { fov: hip_fov().to_radians(), near: 0.01, far: 10.0, ..default() }),
            RenderLayers::layer(viewmodel_layer(slot)),
            // Both cameras draw into one image, so their MSAA and HDR
            // must match. This camera renders last, so its post-processing
            // (exposure, bloom, tonemapping, SMAA) covers the whole frame.
            Hdr,
            Msaa::Off,
            // Only the effects' few muzzle-flash lights are clustered: one
            // cluster for them all (the default froxel grid cost the render
            // thread over a millisecond a frame).
            bevy::light::cluster::ClusterConfig::Single,
            AutoExposure {
                range: -EXPOSURE_RANGE_EV..=EXPOSURE_RANGE_EV,
                // The brightest 15% left out: snow and overcast skies set it
                // otherwise, leaving shade black (Bloc); leaving out more
                // lifted the whole frame and flattened Crash's depth
                // against CoD4's.
                filter: 0.05..=0.85,
                compensation_curve,
                ..default()
            },
            Bloom::NATURAL,
            Tonemapping::AgX,
            Smaa { preset: SmaaPreset::High },
            ChildOf(world),
        ))
        .id();
    if slot == 0 {
        commands.entity(vm).insert(ViewModelCamera);
    }
    // The sun for the viewmodel's layer (see `model_lighting`).
    commands.spawn((
        crate::model_lighting::ViewModelSun::new(slot),
        DirectionalLight { illuminance: 0.0, shadow_maps_enabled: false, ..default() },
        Transform::default(),
        RenderLayers::layer(viewmodel_layer(slot)),
    ));
}

/// Each local player's cameras draw their part of the window.
fn place_views(
    window: Single<&Window, With<PrimaryWindow>>,
    mut cameras: Query<(&mut Camera, Option<&crate::splitscreen::SlotCamera>, Option<&crate::splitscreen::SlotViewModelCamera>)>,
) {
    let count = crate::splitscreen::count();
    if count < 2 {
        return;
    }
    let size = window.physical_size();
    for (mut camera, world, vm) in &mut cameras {
        let Some(slot) = world.map(|w| w.0).or(vm.map(|v| v.0)) else { continue };
        let wanted = crate::splitscreen::viewport(slot, count, size);
        let now = camera.viewport.as_ref().map(|v| (v.physical_position, v.physical_size));
        if now != wanted.as_ref().map(|v| (v.physical_position, v.physical_size)) {
            camera.viewport = wanted;
        }
    }
}

fn grab_cursor(
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if mouse.just_pressed(MouseButton::Left) && cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// Vertical field of view in degrees for the weapon's current ADS fraction.
pub fn view_fov(gunplay: Gunplay, weapon: &WeaponState) -> f32 {
    let (hip, ads) = gunplay.fovs(weapon.def);
    hip + (ads - hip) * weapon.ads
}

/// Radians of view rotation per mouse count. Sensitivity scales with zoom
/// like CoD's ADS sensitivity.
pub fn look_scale(gunplay: Gunplay, weapon: &WeaponState) -> f32 {
    // Through a lens scope, by the lens's zoom rather than the view's.
    let fov = if weapon.def.ads_overlay.is_some() && crate::ui::lens_scopes() && !gunplay.is_bodycam() {
        hip_fov() + (weapon.def.ads_fov - hip_fov()) * weapon.ads
    } else {
        view_fov(gunplay, weapon)
    };
    // The settings' sensitivity (and aiming sensitivity) on top.
    SENSITIVITY * fov / gunplay.fovs(weapon.def).0 * crate::settings_apply::mouse_scale(weapon.ads)
}

fn mouse_look(
    gunplay: Res<Gunplay>,
    mut players: Query<(&PlayerInput, &mut ViewAngles, &WeaponState), (With<LocalSlot>, Without<Dead>)>,
) {
    for (input, mut view, weapon) in &mut players {
        if !input.live || input.look == Vec2::ZERO {
            continue;
        }
        let scale = look_scale(*gunplay, weapon);
        let invert = if crate::settings_apply::invert_mouse() { -1.0 } else { 1.0 };
        view.yaw -= input.look.x * scale;
        view.pitch = (view.pitch - input.look.y * scale * invert).clamp(-85f32.to_radians(), 85f32.to_radians());
    }
}

fn keyboard_input(
    gunplay: Res<Gunplay>,
    mut toggles: ResMut<StanceToggle>,
    mut players: Query<(&LocalSlot, &PlayerInput, &mut MoveInput, &mut WeaponInput, &WeaponState, &Mover)>,
) {
    for (slot, input, mut mv, mut wi, weapon, mover) in &mut players {
        let toggle = &mut toggles.0[slot.0.min(crate::splitscreen::MAX_PLAYERS - 1)];
        if !input.live {
            *mv = MoveInput { stance: mv.stance, ..default() };
            *wi = WeaponInput::default();
            continue;
        }
        let (keys, mouse, pad) = (&input.keys, &input.mouse, &input.pad);
        let axis = |pos: KeyCode, neg: KeyCode| keys.pressed(pos) as i32 as f32 - keys.pressed(neg) as i32 as f32;

        // C toggles crouch, Ctrl toggles prone, jump stands back up.
        if keys.just_pressed(KeyCode::KeyC) {
            *toggle = if *toggle == Stance::Crouch { Stance::Stand } else { Stance::Crouch };
        }
        if keys.just_pressed(KeyCode::ControlLeft) {
            *toggle = if *toggle == Stance::Prone { Stance::Stand } else { Stance::Prone };
        }
        let mut jump = keys.pressed(KeyCode::Space);
        if keys.just_pressed(KeyCode::Space) && *toggle != Stance::Stand {
            *toggle = if *toggle == Stance::Prone { Stance::Crouch } else { Stance::Stand };
            jump = false;
        }
        // The pad's left stick adds to the keys (its buttons press them).
        let forward = (axis(KeyCode::KeyW, KeyCode::KeyS) + pad.movement.y).clamp(-1.0, 1.0);
        let right = (axis(KeyCode::KeyD, KeyCode::KeyA) + pad.movement.x).clamp(-1.0, 1.0);
        let sprint = keys.pressed(KeyCode::ShiftLeft) || pad.sprint;
        if sprint && mover.stance != Stance::Stand && forward > 0.5 {
            // Sprinting stands you up, like CoD4.
            *toggle = Stance::Stand;
        }

        *mv = MoveInput {
            forward,
            right,
            jump,
            sprint,
            stance: *toggle,
            speed_scale: weapon.speed_scale(),
            // Q/E lean in Bodycam gunplay.
            lean: if gunplay.is_bodycam() { axis(KeyCode::KeyE, KeyCode::KeyQ) } else { 0.0 },
        };
        *wi = WeaponInput {
            fire: mouse.pressed(MouseButton::Left),
            ads: mouse.pressed(MouseButton::Right),
            reload: keys.just_pressed(KeyCode::KeyR),
            inspect: keys.just_pressed(KeyCode::KeyI),
        };
    }
}

/// CoD4's camera, each local player's; [`crate::bodycam`] re-poses it
/// afterwards in Bodycam gunplay.
#[allow(clippy::type_complexity)]
/// Out for the round (Search and Destroy, a held HQ): the teammate each
/// local player watches, by slot, and their name for the HUD.
static WATCHING: std::sync::Mutex<[Option<String>; crate::splitscreen::MAX_PLAYERS]> =
    std::sync::Mutex::new([const { None }; crate::splitscreen::MAX_PLAYERS]);

/// Who a local player out for the round is watching.
pub fn watching(slot: usize) -> Option<String> {
    WATCHING.lock().ok().and_then(|w| w.get(slot).cloned().flatten())
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn follow_camera(
    players: Query<
        (
            (&LocalSlot, &crate::combat::Pawn, &crate::splitscreen::PlayerInput),
            &Transform,
            &Mover,
            &ViewAngles,
            &WeaponState,
            Option<&Dead>,
            Has<crate::loadout::AwaitingClass>,
            Option<&crate::first_person::feel::Feel>,
        ),
        Without<crate::splitscreen::SlotCamera>,
    >,
    teammates: Query<(Entity, &crate::combat::Pawn, &Transform, &Mover, &ViewAngles), (Without<Dead>, Without<crate::splitscreen::SlotCamera>)>,
    mut watched: Local<[Option<Entity>; crate::splitscreen::MAX_PLAYERS]>,
    mut cameras: Query<(&crate::splitscreen::SlotCamera, &mut Transform, &mut Projection), Without<LocalSlot>>,
    mut vm_cameras: Query<(&crate::splitscreen::SlotViewModelCamera, &mut Projection), (Without<crate::splitscreen::SlotCamera>, Without<LocalSlot>)>,
    gunplay: Res<Gunplay>,
    time: Res<Time>,
) {
    for ((slot, me, input), tf, mover, view, weapon, dead_state, picking_class, feel) in &players {
        let Some((_, mut cam_tf, mut proj)) = cameras.iter_mut().find(|c| c.0.0 == slot.0) else { continue };
        // Out until the next round: watch a teammate (fire for the next), as
        // CoD4's spectating does.
        let out = dead_state.is_some_and(|d| d.respawn_at.is_infinite()) && !picking_class && !crate::combat::free_for_all()
            // (An online guest's dead wait on the host instead.)
            && crate::netplay::authority();
        let s = slot.0.min(crate::splitscreen::MAX_PLAYERS - 1);
        if out {
            let mut team: Vec<_> = teammates.iter().filter(|t| t.1.team == me.team && t.1.id != me.id).collect();
            team.sort_by_key(|t| t.1.id);
            let at = watched[s].and_then(|w| team.iter().position(|t| t.0 == w));
            let next = input.live && input.mouse.just_pressed(MouseButton::Left);
            let pick = match at {
                Some(i) if next => Some((i + 1) % team.len()),
                Some(i) => Some(i),
                None if !team.is_empty() => Some(0),
                None => None,
            };
            if let Some(&(e, p, t_tf, t_mover, t_view)) = pick.and_then(|i| team.get(i)) {
                watched[s] = Some(e);
                if let Ok(mut w) = WATCHING.lock() {
                    w[s] = Some(p.name.clone());
                }
                cam_tf.translation = t_mover.eye(t_tf.translation);
                cam_tf.rotation = Quat::from_euler(EulerRot::YXZ, t_view.yaw, t_view.pitch, 0.0);
                continue;
            }
        }
        if watched[s].take().is_some()
            && let Ok(mut w) = WATCHING.lock()
        {
            w[s] = None;
        }
        // Waiting to spawn with a class: a level view from the spawn.
        let dead = dead_state.is_some() && !picking_class;
        let mut eye = if dead { tf.translation + Vec3::Y * u(8.0) } else { mover.eye(tf.translation) };
        // View angle bob while aiming down sights (`BG_CalculateViewAngles`),
        // in CoD degrees: pitch down, yaw left.
        let (mut bob_pitch, mut bob_yaw) = (0.0, 0.0);
        if !dead {
            // View bob (`BG_GetPlayerViewOrigin`).
            let (side, up) = mover.view_bob();
            let right = Quat::from_rotation_y(view.yaw) * Vec3::X;
            eye += right * side + Vec3::Y * up;
            // Never bob below 8 units above the feet.
            eye.y = eye.y.max(tf.translation.y + u(8.0));

            let scale = weapon.ads * weapon.def.ads_view_bob_mult;
            if scale != 0.0 {
                let (cycle, speed) = (mover.bob_angle(), mover.xy_speed_units());
                bob_pitch = -mover.vertical_bob(cycle, speed, 45.0) * scale;
                bob_yaw = -mover.horizontal_bob(cycle, speed, 45.0) * scale;
            }
        }
        // The richer bob's dips and roll ([`crate::first_person::feel`]).
        let (feel_up, feel_pitch, feel_roll) = feel.filter(|_| !dead).map_or((0.0, 0.0, 0.0), |f| (f.cam_up, f.cam_pitch, f.cam_roll));
        cam_tf.translation = eye + Vec3::Y * feel_up;
        let roll = if dead { 0.6 } else { feel_roll };
        cam_tf.rotation =
            Quat::from_euler(EulerRot::YXZ, view.yaw + bob_yaw.to_radians(), view.pitch - bob_pitch.to_radians() + feel_pitch, roll);
        if let Projection::Perspective(p) = proj.as_mut() {
            let target = view_fov(*gunplay, weapon).to_radians();
            p.fov += (target - p.fov) * (1.0 - (-20.0 * time.delta_secs()).exp());
            let fov = p.fov;
            if let Some((_, mut vp)) = vm_cameras.iter_mut().find(|c| c.0.0 == slot.0) {
                if let Projection::Perspective(vp) = vp.as_mut() {
                    vp.fov = fov;
                }
            }
        }
    }
}
