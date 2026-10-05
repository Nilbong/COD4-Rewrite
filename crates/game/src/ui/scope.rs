//! Sniper scopes, two ways (the `cg_scopestyle` setting, Options > Game):
//!
//! - Classic, CoD4's: aimed all the way in with a scoped gun (CoD4's
//!   `overlayMaterial`, Black Ops' and World at War's `adsOverlayShader`),
//!   the scope's picture fills the middle of the screen with black either
//!   side and the gun isn't drawn, as CoD4's `CG_DrawWeapReticle` does.
//! - Lens, as modern shooters do it: the view stays a rifle's aimed view
//!   ([`LENS_OUTER_FOV`]) and a lens in the middle shows a second camera's
//!   view at the gun's own zoom (`fAdsZoomFov`) under the scope's picture
//!   (its reticle and rim), fading in as the gun comes up. Black Ops' and
//!   World at War's rifles stay drawn below it; CoD4's, whose aimed pose
//!   puts the eyepiece over the whole view (it hides them), go once fully
//!   aimed. The scope camera only renders while the lens shows.
//!
//! The HUD stays on top. Not in Bodycam gunplay or third person, nor under a
//! menu. `COD4RW_SCOPE=classic|lens` picks the style for a run;
//! `COD4RW_SCOPETEST=<dir>` aims in and screenshots the same view through
//! both (`classic.png`, `lens.png`), then exits.

use super::Frontend;
use crate::bodycam::Gunplay;
use crate::combat::Dead;
use crate::player::{LocalPlayer, MainCamera, ViewModelCamera};
use crate::state::{GameState, Setup, in_game};
use crate::weapons::{WeaponSet, WeaponState};
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Hdr, RenderTarget};
use bevy::core_pipeline::Skybox;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::auto_exposure::AutoExposure;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, TextureFormat};
use bevy::shader::ShaderRef;
use bevy::ui_render::prelude::{MaterialNode, UiMaterial, UiMaterialPlugin};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn build(app: &mut App) {
    app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
        std::path::PathBuf::from(file!()).with_file_name("scope_lens.wgsl"),
        std::path::Path::new(LENS_SHADER),
        include_bytes!("scope_lens.wgsl").as_slice(),
    );
    app.add_plugins(UiMaterialPlugin::<ScopeLensMaterial>::default())
        .add_systems(OnEnter(GameState::InGame), spawn.in_set(Setup::Spawn))
        .add_systems(
            Update,
            (spawn_camera, show).chain().after(WeaponSet).run_if(in_game.and_then(resource_exists::<Frontend>)),
        );
    if let Ok(dir) = std::env::var("COD4RW_SCOPETEST") {
        app.insert_resource(ScopeTest(dir.into())).add_systems(
            Update,
            scope_test
                .after(crate::player::InputSet)
                .before(WeaponSet)
                .run_if(in_game.and_then(resource_exists::<Frontend>)),
        );
    }
}

/// The setting's dvar: `classic` (the default) or `lens`.
pub const SCOPE_STYLE_DVAR: &str = "cg_scopestyle";
/// The view around a lens scope, fully aimed (degrees, vertical): about a
/// rifle's aimed zoom.
pub const LENS_OUTER_FOV: f32 = 55.0;

/// Under CoD4's HUD (`draw`'s layers start at 1000).
const SCOPE_Z: i32 = 990;
/// CoD4 shows the scope once the gun is all the way up.
const SHOWN_AT: f32 = 0.999;
/// The lens: its diameter (screen heights), when it starts fading in (aimed
/// fraction) and its camera's picture (pixels square).
const LENS_SIZE: f32 = 0.42;
const LENS_FROM: f32 = 0.6;
const LENS_RESOLUTION: u32 = 1024;
const LENS_SHADER: &str = "cod4rw/scope_lens.wgsl";

static LENS: AtomicBool = AtomicBool::new(false);

/// Lens scopes are the setting (the aimed field of view follows it:
/// [`crate::bodycam::Gunplay::fovs`]).
pub fn lens_scopes() -> bool {
    // Classic in splitscreen: the lens is drawn by one camera, Player 1's.
    LENS.load(Ordering::Relaxed) && !crate::splitscreen::active()
}

/// The scope camera's field of view (radians, vertical) for a gun aimed at
/// `ads_fov` degrees: the lens is [`LENS_SIZE`] of the screen's height, so
/// it sees that much of classic's view, at classic's size on screen (what's
/// under the reticle looks as big through either).
fn lens_fov(ads_fov: f32) -> f32 {
    2.0 * (LENS_SIZE * (ads_fov.max(1.0).to_radians() * 0.5).tan()).atan()
}

/// The camera a lens scope's view comes from.
#[derive(Component)]
pub struct ScopeCamera;

/// See `scope_lens.wgsl`.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct ScopeLensMaterial {
    #[uniform(0)]
    params: Vec4,
    #[texture(1)]
    #[sampler(2)]
    view: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    overlay: Handle<Image>,
}

impl UiMaterial for ScopeLensMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/scope_lens.wgsl".into()
    }
}

/// A local player's classic scope over their view (by their place).
#[derive(Component)]
struct Scope(usize);

#[derive(Component)]
struct ScopePicture(usize);

#[derive(Component)]
struct Lens;

/// The scope camera's picture.
#[derive(Resource)]
struct LensView(Handle<Image>);

/// A new scope camera renders this long first, so its view's shaders are
/// ready before the first lens shows (seconds).
const LENS_WARM_UP: f32 = 1.5;

/// Until when the scope camera warms up.
#[derive(Resource, Default)]
struct LensWarmUp(f32);

fn spawn(mut commands: Commands, mut images: ResMut<Assets<Image>>, mut materials: ResMut<Assets<ScopeLensMaterial>>) {
    for slot in 0..crate::splitscreen::count() {
        let root = commands
            .spawn((
                Scope(slot),
                Node {
                    position_type: PositionType::Absolute,
                    width: percent(100),
                    height: percent(100),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    overflow: Overflow::clip(),
                    ..default()
                },
                GlobalZIndex(SCOPE_Z),
                Visibility::Hidden,
            ))
            .id();
        let bar = || (Node { flex_grow: 1.0, height: percent(100), ..default() }, BackgroundColor(Color::BLACK));
        commands.spawn((bar(), ChildOf(root)));
        commands.spawn((ScopePicture(slot), Node { height: percent(100), flex_shrink: 0.0, ..default() }, ImageNode::default(), ChildOf(root)));
        commands.spawn((bar(), ChildOf(root)));
    }

    let view = images.add(Image::new_target_texture(LENS_RESOLUTION, LENS_RESOLUTION, TextureFormat::Rgba8UnormSrgb, None));
    let size = LENS_SIZE * 100.0;
    commands.spawn((
        Lens,
        Node {
            position_type: PositionType::Absolute,
            left: percent(50),
            top: percent(50),
            width: vh(size),
            height: vh(size),
            margin: UiRect { left: vh(-size / 2.0), top: vh(-size / 2.0), ..default() },
            ..default()
        },
        MaterialNode(materials.add(ScopeLensMaterial { params: Vec4::ZERO, view: view.clone(), overlay: Handle::default() })),
        GlobalZIndex(SCOPE_Z),
        Visibility::Hidden,
    ));
    commands.insert_resource(LensView(view));
}

/// The scope camera, once lens scopes are the setting: on the player's eye
/// like the main camera, seeing what it sees as the frame's finishing camera
/// would (exposure, tonemapping), off until a lens shows.
fn spawn_camera(
    mut commands: Commands,
    time: Res<Time>,
    view: Option<Res<LensView>>,
    existing: Query<(), With<ScopeCamera>>,
    main: Query<(Entity, &Camera, Option<&Skybox>, Option<&DistanceFog>), With<MainCamera>>,
    finishing: Query<&AutoExposure, With<ViewModelCamera>>,
) {
    // Only once lens scopes are the setting: classic needs no camera.
    let (Some(view), true, true) = (view, existing.is_empty(), lens_scopes()) else { return };
    let Ok((main, main_camera, skybox, fog)) = main.single() else { return };
    let mut camera = commands.spawn((
        Name::new("scope camera"),
        ScopeCamera,
        Camera3d::default(),
        Camera { order: -1, is_active: false, clear_color: main_camera.clear_color.clone(), ..default() },
        RenderTarget::Image(view.0.clone().into()),
        Projection::from(PerspectiveProjection { fov: 10f32.to_radians(), near: 0.05, far: 2000.0, ..default() }),
        Hdr,
        Msaa::Off,
        Tonemapping::AgX,
        Transform::default(),
        ChildOf(main),
    ));
    if let Some(s) = skybox {
        camera.insert(s.clone());
    }
    if let Some(f) = fog {
        camera.insert(f.clone());
    }
    if let Ok(exposure) = finishing.single() {
        camera.insert(exposure.clone());
    }
    commands.insert_resource(LensWarmUp(time.elapsed_secs() + LENS_WARM_UP));
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn show(
    mut fe: ResMut<Frontend>,
    mut images: ResMut<Assets<Image>>,
    gunplay: Res<Gunplay>,
    third_person: Option<Res<crate::wardrobe::ThirdPerson>>,
    window: Single<&Window, With<bevy::window::PrimaryWindow>>,
    players: Query<(&crate::splitscreen::LocalSlot, &WeaponState), Without<Dead>>,
    mut scope: Query<(&Scope, &mut Visibility, &mut Node), (Without<Lens>, Without<ScopePicture>)>,
    mut picture: Query<(&ScopePicture, &mut ImageNode, &mut Node), Without<Scope>>,
    mut vm_cameras: Query<(&crate::splitscreen::SlotViewModelCamera, &mut RenderLayers)>,
    (mut lens, mut scope_camera, mut lens_materials): (
        Query<(&mut Visibility, &MaterialNode<ScopeLensMaterial>), (With<Lens>, Without<Scope>)>,
        Query<(&mut Camera, &mut Projection), With<ScopeCamera>>,
        ResMut<Assets<ScopeLensMaterial>>,
    ),
    (time, warm_up): (Res<Time>, Option<Res<LensWarmUp>>),
    mut from_env: Local<bool>,
) {
    if !std::mem::replace(&mut *from_env, true) {
        if let Ok(style) = std::env::var("COD4RW_SCOPE") {
            fe.set_dvar(SCOPE_STYLE_DVAR, &style);
        }
    }
    let lens_style = fe.dvars.get(SCOPE_STYLE_DVAR).is_some_and(|v| v.eq_ignore_ascii_case("lens"));
    LENS.store(lens_style, Ordering::Relaxed);
    // Lens scopes are Player 1's alone, and not in splitscreen.
    let lens_style = lens_scopes();
    let count = crate::splitscreen::count();
    let picture_of = |fe: &mut Frontend, images: &mut Assets<Image>, material: &str| fe.assets.material(material, images);
    let mut lens_on = None;
    for slot in 0..count {
        let viewing = !gunplay.is_bodycam() && !third_person.as_ref().is_some_and(|t| t.on(slot)) && fe.stack.is_empty();
        let held = players.iter().find(|p| p.0.0 == slot).map(|p| p.1).filter(|_| viewing);
        let scoped = held.and_then(|w| Some((w.ads, w.def.ads_fov, w.def.ads_overlay.clone()?)));
        let cod4_gun = held.is_some_and(|w| !crate::waw::is_waw(&w.def.name) && !crate::bo1::is_bo1(&w.def.name));

        // Classic: the scope's picture over the player's view.
        let classic = scoped.as_ref().filter(|(ads, ..)| !lens_style && *ads >= SHOWN_AT);
        let image = classic.and_then(|(_, _, o)| Some((picture_of(&mut fe, &mut images, &o.material)?, o)));
        if let Some((image, o)) = &image {
            for (_, mut node, mut layout) in picture.iter_mut().filter(|p| p.0.0 == slot) {
                if node.image != image.handle {
                    node.image = image.handle.clone();
                }
                // Sized in CoD's 640x480 virtual screen, by its height.
                let height = percent(o.height / 4.8);
                let ratio = Some(o.width / o.height.max(1.0));
                if layout.height != height || layout.aspect_ratio != ratio {
                    layout.height = height;
                    layout.aspect_ratio = ratio;
                }
            }
        }
        let on = image.is_some();
        for (_, mut v, mut node) in scope.iter_mut().filter(|s| s.0.0 == slot) {
            v.set_if_neq(if on { Visibility::Inherited } else { Visibility::Hidden });
            if count > 1 {
                let (at, size) = crate::splitscreen::logical_rect(slot, count, &window);
                let (left, top, width, height) = (px(at.x), px(at.y), px(size.x), px(size.y));
                if node.left != left || node.top != top || node.width != width || node.height != height {
                    (node.left, node.top, node.width, node.height) = (left, top, width, height);
                }
            }
        }
        // The gun goes while looking through the classic scope, and CoD4's
        // through a lens once fully aimed.
        let lens_hides = lens_style && cod4_gun && scoped.as_ref().is_some_and(|(ads, ..)| *ads >= SHOWN_AT);
        let layers = if on || lens_hides { RenderLayers::none() } else { RenderLayers::layer(crate::splitscreen::viewmodel_layer(slot)) };
        for (_, mut l) in vm_cameras.iter_mut().filter(|c| c.0.0 == slot) {
            l.set_if_neq(layers.clone());
        }
        if slot == 0 {
            lens_on = scoped.filter(|(ads, ..)| lens_style && *ads > LENS_FROM);
        }
    }

    // Lens: the scope camera's view at the gun's zoom, fading in.
    let lens_on = lens_on.as_ref();
    let fade = lens_on.map_or(0.0, |(ads, ..)| ((ads - LENS_FROM) / (1.0 - LENS_FROM)).clamp(0.0, 1.0));
    let warming = warm_up.is_some_and(|w| time.elapsed_secs() < w.0);
    for (mut camera, mut projection) in &mut scope_camera {
        let active = lens_on.is_some() || warming;
        if camera.is_active != active {
            camera.is_active = active;
        }
        if let (Some((_, fov, _)), Projection::Perspective(p)) = (lens_on, projection.as_mut()) {
            let fov = lens_fov(*fov);
            if p.fov != fov {
                p.fov = fov;
            }
        }
    }
    let overlay = lens_on.and_then(|(_, _, o)| picture_of(&mut fe, &mut images, &o.material));
    for (mut v, node) in &mut lens {
        v.set_if_neq(if lens_on.is_some() { Visibility::Inherited } else { Visibility::Hidden });
        let Some(m) = lens_on.and_then(|_| lens_materials.get_mut(&node.0)) else { continue };
        let mut m = m;
        let params = Vec4::new(overlay.is_some() as u8 as f32, fade, 0.0, 0.0);
        if m.params != params {
            m.params = params;
        }
        if let Some(o) = overlay.as_ref().filter(|o| m.overlay != o.handle) {
            m.overlay = o.handle.clone();
        }
    }
}

#[derive(Resource)]
struct ScopeTest(std::path::PathBuf);

/// From 6 s: aim, held still (the gun's first raise done); classic at 9 s,
/// lens at 11 s (after its camera's warm-up), then exit.
fn scope_test(
    mut commands: Commands,
    time: Res<Time>,
    test: Res<ScopeTest>,
    mut fe: ResMut<Frontend>,
    mut player: Query<(&mut crate::weapons::WeaponInput, &mut Transform, &mut crate::movement::Mover), With<LocalPlayer>>,
    mut held: Local<Option<Vec3>>,
    mut step: Local<u8>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let t = time.elapsed_secs();
    let Ok((mut input, mut feet, mut mover)) = player.single_mut() else { return };
    if t < 6.0 {
        return;
    }
    let at = *held.get_or_insert(feet.translation);
    feet.translation = at;
    mover.velocity = Vec3::ZERO;
    input.ads = true;
    let mut shot = |name: &str| {
        std::fs::create_dir_all(&test.0).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.0.join(name)));
    };
    match *step {
        0 => {
            fe.set_dvar(SCOPE_STYLE_DVAR, "classic");
            *step = 1;
        }
        1 if t >= 9.0 => {
            shot("classic.png");
            *step = 2;
        }
        // A screenshot is of the frame being drawn: switch a little later.
        2 if t >= 9.3 => {
            fe.set_dvar(SCOPE_STYLE_DVAR, "lens");
            *step = 3;
        }
        3 if t >= 11.0 => {
            shot("lens.png");
            *step = 4;
        }
        4 if t >= 11.5 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Something filling a fraction of the screen's height in classic fills
    /// the same number of pixels through the lens.
    #[test]
    fn lens_magnifies_like_classic() {
        let ads = 15.0f32;
        let classic_px_per_tan = 1.0 / (2.0 * (ads.to_radians() * 0.5).tan());
        let lens_px_per_tan = LENS_SIZE / (2.0 * (lens_fov(ads) * 0.5).tan());
        assert!((classic_px_per_tan - lens_px_per_tan).abs() < 1e-4);
        assert!(lens_fov(ads) < ads.to_radians());
    }
}
