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
                    grab_cursor.run_if(crate::ui::no_ingame_menu).run_if(crate::look_editor::closed),
                    (mouse_look, keyboard_input).in_set(InputSet).before(crate::movement::MovementSet),
                )
                    .run_if(crate::state::in_game),
            )
            .add_systems(
                PostUpdate,
                (place_views, follow_camera).chain().before(TransformSystems::Propagate).run_if(crate::state::in_game),
            );
        if std::env::var_os("COD4RW_LEANTEST").is_some() {
            app.add_systems(PreUpdate, lean_test.after(bevy::input::InputSystems).run_if(crate::state::in_game));
        }
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
    // (`ssao.thickness` and `ssao.quality` in tuning.txt, read as the camera
    // spawns: quality 0 low, 1 medium, 2 high (Bevy's default), 3 ultra.)
    use bevy::pbr::ScreenSpaceAmbientOcclusionQualityLevel as Q;
    let quality_level = match crate::tune::get("ssao.quality", 2.0).round() as i32 {
        0 => Q::Low,
        1 => Q::Medium,
        3 => Q::Ultra,
        _ => Q::High,
    };
    bevy::pbr::ScreenSpaceAmbientOcclusion { constant_object_thickness: crate::tune::get("ssao.thickness", 3.0), quality_level, ..default() }
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
    // Sloped (knob `exp.slope`, 1; 0 flat at the target): dark views are
    // let stay darker than mid grey, so a dim room with a sunlit patch
    // reads as dim (a flat curve pushed every room to the same grey).
    let k = crate::tune::get("exp.slope", 1.0);
    let curve = LinearSpline::new([
        // (Not past dim rooms: by night, below -6, back to half a stop
        // under, so a moonlit street reads (Wet Work's night went black).)
        Vec2::new(-16.0, EXPOSURE_TARGET_EV - 0.5 * k),
        Vec2::new(-9.0, EXPOSURE_TARGET_EV - 0.5 * k),
        Vec2::new(-6.0, EXPOSURE_TARGET_EV - 1.5 * k),
        Vec2::new(-4.5, EXPOSURE_TARGET_EV - 1.5 * k),
        Vec2::new(-2.5, EXPOSURE_TARGET_EV),
        // (Bright views held at the target: brightening them burnt out
        // Crossfire's sunlit streets with the whole sun live.)
        Vec2::new(4.0, EXPOSURE_TARGET_EV),
        Vec2::new(16.0, EXPOSURE_TARGET_EV),
    ]);
    let compensation_curve = curves.add(AutoExposureCompensationCurve::from_curve(curve).expect("exposure compensation curve"));
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
                // (Knobs `exp.min`, `exp.max`: a wider range lets dark rooms
                // and sunlit streets each keep their own level.)
                range: crate::tune::get("exp.min", -4.5)..=crate::tune::get("exp.max", EXPOSURE_RANGE_EV),
                // The brightest 15% left out: snow and overcast skies set it
                // otherwise, leaving shade black (Bloc); leaving out more
                // lifted the whole frame and flattened Crash's depth
                // against CoD4's.
                filter: 0.05..=crate::tune::get("exp.filter_hi", 0.85),
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

/// Auto lean: how far to lean (-1 left .. 1 right, 0 none) for an eye
/// looking along `yaw`, given the last frame's. A wall's edge ahead to one
/// side (short rays ahead from across the eye meet it on that side and not
/// the other): lean out the open side only as far as puts the eye (and the
/// gun's barrel with it, aimed) just past the edge, a small margin clear,
/// no further; none if it's clear already. At most CoD4's lean; the
/// lean's own collision keeps it out of walls. Kept a little longer once
/// leaning, so it doesn't flicker.
fn auto_lean(spatial: &avian3d::prelude::SpatialQuery, eye: Vec3, yaw: f32, last: f32) -> f32 {
    let filter = crate::collision::movement_filter();
    let forward = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
    let right = Quat::from_rotation_y(yaw) * Vec3::X;
    let knob = |n: &str, d: f32| crate::tune::get(&format!("autolean.{n}"), d);
    // (High sensitivity: from a little further off.)
    let high = crate::settings_apply::auto_lean_high();
    let keep = if last != 0.0 { 1.3 } else { 1.0 };
    let reach = u(if high { knob("reach_high", 32.0) } else { knob("reach", 22.0) }) * keep;
    let blocked = |side: f32| Dir3::new(forward).ok().is_some_and(|d| spatial.cast_ray(eye + right * side, d, reach, true, &filter).is_some());
    let shoulder = u(knob("shoulder", 10.0));
    // Which side the wall is: blocked from that shoulder, open from the other.
    let wall = match (blocked(-shoulder), blocked(shoulder)) {
        (true, false) => -1.0,
        (false, true) => 1.0,
        _ => return 0.0,
    };
    // Where its edge is: across from the open side, the first blocked ray
    // (signed, towards the wall).
    let reach_out = crate::movement::LEAN_DISTANCE;
    let step = u(1.0);
    let mut edge = None;
    // (From the open shoulder, which the test above found clear.)
    let mut o = -shoulder;
    while o <= shoulder {
        if blocked(o * wall) {
            edge = Some(o);
            break;
        }
        o += step;
    }
    let Some(edge) = edge else { return 0.0 };
    // The eye a margin clear of it, towards the open side.
    let margin = u(knob("margin", 3.0));
    let need = (margin - edge).max(0.0);
    // (A small lean isn't worth it: the deadband.)
    if need <= u(if high { knob("deadband_high", 2.0) } else { knob("deadband", 5.0) }) {
        return 0.0;
    }
    (-wall * (need / reach_out)).clamp(-1.0, 1.0) * knob("amount", 1.0)
}

/// Test aid ([`crate::first_person::live_hands`]' A/B): the step smoothing
/// off.
pub static ZSMOOTH_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Vertical field of view in degrees for the weapon's current ADS fraction.
pub fn view_fov(gunplay: Gunplay, weapon: &WeaponState) -> f32 {
    let (hip, ads) = gunplay.fovs(weapon.def);
    // The laser aim zooms a little ([`crate::laser`]).
    hip + (ads - hip) * weapon.ads.max(weapon.laser_aim * crate::laser::zoom_share())
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
    mut players: Query<(&PlayerInput, &mut ViewAngles, &WeaponState, Has<LocalPlayer>), (With<LocalSlot>, Without<Dead>)>,
) {
    for (input, mut view, weapon, me) in &mut players {
        // The airstrike's laptop has the mouse (`ui::laptop_screen`).
        if !input.live || input.look == Vec2::ZERO || (me && crate::killstreaks::hands::laptop_open()) {
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
    spatial: avian3d::prelude::SpatialQuery,
    mut auto_leaning: Local<[f32; crate::splitscreen::MAX_PLAYERS]>,
    // (Since when each player's lean has been wanted: it waits a moment.)
    mut lean_wanted_since: Local<[Option<f32>; crate::splitscreen::MAX_PLAYERS]>,
    time: Res<Time>,
    mut players: Query<(&LocalSlot, &PlayerInput, &mut MoveInput, &mut WeaponInput, &WeaponState, &Mover, &Transform, &ViewAngles)>,
    net: Option<Res<crate::netplay::NetStart>>,
) {
    // (Not online yet: a lean isn't in the inputs or the snapshots, so host
    // and guest would put the eye, and the shots, in different places.)
    let online = net.is_some();
    for (slot, input, mut mv, mut wi, weapon, mover, tf, view) in &mut players {
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
            // (No manual lean: it's automatic, below.)
            lean: 0.0,
        };
        // Aiming beside a wall's edge: leaning out past it on its own.
        let auto = &mut auto_leaning[slot.0.min(crate::splitscreen::MAX_PLAYERS - 1)];
        let aiming = mouse.pressed(MouseButton::Right) && !mover.sprinting && mover.stance != Stance::Prone;
        let want = if aiming && mv.lean == 0.0 && !online && crate::settings_apply::auto_lean() {
            auto_lean(&spatial, mover.eye(tf.translation), view.yaw, *auto)
        } else {
            0.0
        };
        // Leaning starts only once it's been wanted a moment (aiming past
        // the edge, not sweeping by it); it stops at once.
        let since = &mut lean_wanted_since[slot.0.min(crate::splitscreen::MAX_PLAYERS - 1)];
        let now = time.elapsed_secs();
        *since = if want == 0.0 { None } else { Some(since.unwrap_or(now)) };
        let hold = crate::tune::get(if crate::settings_apply::auto_lean_high() { "autolean.hold_high" } else { "autolean.hold" }, if crate::settings_apply::auto_lean_high() { 0.15 } else { 0.35 });
        *auto = if *auto != 0.0 || since.is_some_and(|s| now - s >= hold) { want } else { 0.0 };
        if mv.lean == 0.0 {
            mv.lean = *auto;
        }
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
    mut step_smooth: Local<[(Option<f32>, f32); crate::splitscreen::MAX_PLAYERS]>,
) {
    let time_delta = time.delta_secs();
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
        // A step up or down: the eye eases over it (`cam.zsmooth_time`, 0
        // for none).
        {
            let i = slot.0.min(crate::splitscreen::MAX_PLAYERS - 1);
            let (last, offset) = &mut step_smooth[i];
            let change = last.map_or(0.0, |l| mover.steps_total - l);
            *last = Some(mover.steps_total);
            let time = if ZSMOOTH_OFF.load(std::sync::atomic::Ordering::Relaxed) { 0.0 } else { crate::tune::get("cam.zsmooth_time", 0.1) };
            if dead || time <= 0.0 {
                *offset = 0.0;
            } else {
                if change.abs() >= u(1.0) && change.abs() <= u(16.0) {
                    *offset -= change;
                }
                *offset = offset.clamp(-u(16.0), u(16.0));
                let ease = (offset.abs() / time.max(0.01)).max(u(40.0)) * time_delta;
                *offset = if *offset > 0.0 { (*offset - ease).max(0.0) } else { (*offset + ease).min(0.0) };
                eye.y += *offset;
                if std::env::var_os("COD4RW_STEPLOG").is_some() && (change != 0.0 || offset.abs() > 0.0 || !mover.on_ground) {
                    info!(
                        "steplog: feet {:.1} eye {:.1} change {:.1} offset {:.1} ground {} vy {:.0}",
                        tf.translation.y / u(1.0),
                        eye.y / u(1.0),
                        change / u(1.0),
                        *offset / u(1.0),
                        mover.on_ground,
                        mover.velocity.y / u(1.0)
                    );
                }
            }
        }
        // Leaning (CoD4's `AddLeanToPosition`): the eye out to the side and
        // the view rolled with it (Bodycam gunplay leans its own way).
        let lean_roll = if dead || gunplay.is_bodycam() {
            0.0
        } else {
            eye += mover.lean_offset(view.yaw);
            -mover.lean * crate::tune::get("lean.roll", 10.0).to_radians()
        };
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
        let roll = if dead { 0.6 } else { feel_roll + lean_roll };
        cam_tf.rotation =
            Quat::from_euler(EulerRot::YXZ, view.yaw + bob_yaw.to_radians(), view.pitch - bob_pitch.to_radians() + feel_pitch, roll);
        if let Projection::Perspective(p) = proj.as_mut() {
            // A heavy shot kicks the view in for a moment ([`crate::first_person::feel`]).
            let punch = feel.filter(|_| !dead).map_or(0.0, |f| f.fov_punch).to_radians();
            let target = view_fov(*gunplay, weapon).to_radians() + punch;
            p.fov += (target - p.fov) * (1.0 - (-20.0 * time.delta_secs()).exp());
            // The gun drawn at the weapon's own ADS zoom when aimed (CoD4
            // draws it at `fAdsZoomFov`: its aimed pose is made for it), not a
            // wider aimed view's (ADS Field of View "Affected", a high field
            // of view), which shrank it and set the sights back from the eye.
            // A 3D scope's lens is fitted to the world's own view.
            // (A 3D or lens scope: the world's, which the lens is fitted to.)
            let three_d = (crate::ui::scope_3d() && crate::ui::scope_magnifies(weapon.def)) || (weapon.def.ads_overlay.is_some() && crate::ui::lens_scopes());
            let fov = if gunplay.is_bodycam() || three_d || weapon.def.ads_fov <= 0.0 {
                p.fov
            } else {
                let hip = gunplay.fovs(weapon.def).0;
                let aimed = hip + (weapon.def.ads_fov - hip) * weapon.ads.clamp(0.0, 1.0);
                // (Easing from the world's at the hip, as the world's eases.)
                p.fov - punch + (aimed.to_radians() - view_fov(*gunplay, weapon).to_radians())
            };
            if let Some((_, mut vp)) = vm_cameras.iter_mut().find(|c| c.0.0 == slot.0) {
                if let Projection::Perspective(vp) = vp.as_mut() {
                    vp.fov = fov;
                }
            }

        }
    }
}

/// Test aid: `COD4RW_LEANTEST=<dir>`. At 5 s the player is put beside a
/// wall's edge found near its spawn (where [`auto_lean`] would lean),
/// looking past it; aims (`auto_lean.png` at 7 s), lets go (`upright.png`
/// at 8.5 s); then at a field of view of 90 aims with ADS Field of View
/// Affected (`ads_affected.png`) and Independent (`ads_independent.png`),
/// and at the hip (`hip_90.png`); then exits.
pub(crate) fn lean_test(
    mut commands: Commands,
    time: Res<Time>,
    spatial: avian3d::prelude::SpatialQuery,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut player: Query<(&mut Transform, &mut ViewAngles, &Mover), With<LocalPlayer>>,
    names: Query<(Option<&Name>, Option<&crate::combat::Hitbox>, Option<&ChildOf>)>,
    mut step: Local<usize>,
    mut start: Local<Option<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(dir) = std::env::var("COD4RW_LEANTEST") else { return };
    let Ok((mut tf, mut view, mover)) = player.single_mut() else { return };
    let t = time.elapsed_secs() - *start.get_or_insert(time.elapsed_secs());
    if cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
    }
    let aim = (5.5..7.5).contains(&t) || (9.0..13.0).contains(&t);
    if aim { mouse.press(MouseButton::Right) } else { mouse.release(MouseButton::Right) }
    let shot = |commands: &mut Commands, name: &str| {
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        info!("lean test: {}", path.display());
        commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(path));
    };
    const STEPS: [(f32, &str); 8] = [(5.0, "place"), (7.0, "auto_lean"), (8.5, "upright"), (9.0, "fov90"), (10.5, "ads_affected"), (11.0, "independent"), (12.5, "ads_independent"), (13.6, "hip_90")];
    let Some(&(at, name)) = STEPS.get(*step) else {
        if t > 14.5 {
            crate::settings_apply::SCRIPT_FOV.store(0, std::sync::atomic::Ordering::Relaxed);
            exit.write(AppExit::Success);
        }
        return;
    };
    if t < at {
        return;
    }
    *step += 1;
    match name {
        "place" if std::env::var("COD4RW_SHOTTRACE").is_ok() || std::env::var("COD4RW_LEANTEST_SPOT").is_ok() => {
            let nums = |k: &str| std::env::var(k).ok().map(|v| v.split(',').filter_map(|x| x.trim().parse::<f32>().ok()).collect::<Vec<f32>>());
            // `COD4RW_SHOTTRACE=x,y,z,yaw,pitch` (CoD units and degrees, the
            // eye): everything a bullet from there meets.
            if let Some(v) = nums("COD4RW_SHOTTRACE").filter(|v| v.len() == 5) {
                let eye = crate::units::pos([v[0], v[1], v[2]]);
                let dir = ViewAngles { yaw: crate::units::yaw_from_cod_degrees(v[3]), pitch: -v[4].to_radians() }.rotation() * Vec3::NEG_Z;
                if let Ok(d) = Dir3::new(dir) {
                    let mut hits = spatial.ray_hits(eye, d, u(3000.0), 12, true, &crate::collision::bullet_filter());
                    hits.sort_by(|a, b| a.distance.total_cmp(&b.distance));
                    for h in hits {
                        let (name, hitbox, parent) = names.get(h.entity).unwrap_or((None, None, None));
                        let parent_name = parent.and_then(|p| names.get(p.parent()).ok()).and_then(|n| n.0.map(|n| n.to_string()));
                        info!(
                            "shot trace: {:.0} units: {:?} {:?} (parent {:?}){} at CoD {:?}",
                            h.distance / u(1.0),
                            h.entity,
                            name.map(|n| n.to_string()),
                            parent_name,
                            if hitbox.is_some() { " a hitbox" } else { "" },
                            crate::units::to_cod(eye + dir * h.distance).map(f32::round)
                        );
                    }
                }
            }
            // `COD4RW_LEANTEST_SPOT=x,y,z,yaw` (CoD): the lean tried there.
            if let Some(v) = nums("COD4RW_LEANTEST_SPOT").filter(|v| v.len() == 4) {
                tf.translation = crate::units::pos([v[0], v[1], v[2]]) + Vec3::Y * u(1.0);
                view.yaw = crate::units::yaw_from_cod_degrees(v[3]);
                view.pitch = 0.0;
            }
        }
        "place" => {
            // The nearest spot (on the floor, a grid round the spawn) and
            // facing where it would lean.
            let feet = tf.translation;
            let down = Dir3::new(Vec3::NEG_Y).unwrap();
            let height = mover.eye(feet) - feet;
            let mut best: Option<(f32, Vec3, f32)> = None;
            for i in -20i32..=20 {
                for j in -20i32..=20 {
                    let at = feet + Vec3::new(i as f32, 0.0, j as f32) * u(24.0);
                    let Some(hit) = spatial.cast_ray(at + Vec3::Y * u(40.0), down, u(120.0), true, &crate::collision::movement_filter()) else { continue };
                    let floor = at + Vec3::Y * (u(40.0) - hit.distance);
                    for k in 0..24 {
                        let yaw = k as f32 * std::f32::consts::TAU / 24.0;
                        if auto_lean(&spatial, floor + height, yaw, 0.0) != 0.0 {
                            let d = floor.distance(feet);
                            if best.is_none_or(|b| d < b.0) {
                                best = Some((d, floor, yaw));
                            }
                        }
                    }
                }
            }
            match best {
                Some((_, floor, yaw)) => {
                    tf.translation = floor + Vec3::Y * u(1.0);
                    view.yaw = yaw;
                    view.pitch = 0.0;
                    info!("lean test: a wall's edge at CoD {:?}, yaw {:.0}", crate::units::to_cod(floor).map(f32::round), yaw.to_degrees());
                }
                None => warn!("lean test: no wall's edge near the spawn"),
            }
        }
        "fov90" => {
            crate::settings_apply::SCRIPT_FOV.store(90f32.to_bits(), std::sync::atomic::Ordering::Relaxed);
            crate::settings_apply::set_ads_affected(true);
            view.yaw += std::f32::consts::PI;
        }
        "independent" => crate::settings_apply::set_ads_affected(false),
        "auto_lean" => {
            info!("lean test: leaning {:.2}", mover.lean);
            shot(&mut commands, name);
        }
        other => shot(&mut commands, other),
    }
}
