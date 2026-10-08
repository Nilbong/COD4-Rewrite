//! 3D scopes (`cg_scopestyle` 3D, `COD4RW_SCOPE=3d`): aiming a magnifying
//! optic (a sniper scope, an ACOG), the scope's own eyepiece on the gun
//! shows a second camera's magnified view, the gun and the scope's tube
//! drawn round it and the rest of the view at a rifle's aimed zoom
//! ([`super::LENS_OUTER_FOV`]), as in modern shooters.
//!
//! The eyepiece is the gun model's own lens surface ([`ScopeLens`], tagged
//! by [`crate::gunmodel`]); its material (`scope3d.wgsl`) looks up the
//! scope camera's picture by the eye's direction, magnified so what's
//! under the reticle is as big as through CoD4's own scope, draws the
//! reticle on the scope's axis (a duplex for a rifle scope, the ACOG's red
//! chevron) and darkens towards the edge of the eye's reach, more the
//! further the scope's axis is off the eye's (coming up into the aim, or
//! swaying): the eye relief.
//!
//! The scope camera renders only while aiming in, at a reduced size, in
//! the scene's own light (linear HDR, no exposure or tonemapping of its
//! own), so the frame's finishing camera exposes and tonemaps it with
//! everything else; it has none of the main camera's extra passes.

use crate::gunmodel::ScopeLens;
use crate::player::{LocalPlayer, MainCamera};
use crate::weapons::WeaponState;
use bevy::camera::{Hdr, RenderTarget};
use bevy::core_pipeline::Skybox;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, TextureFormat};
use bevy::shader::ShaderRef;

const SHADER: &str = "cod4rw/scope3d.wgsl";
/// The scope camera's picture (pixels square).
const RESOLUTION: u32 = 768;
/// The eyepiece's reach: the tangent of its half angle as the eye sees it,
/// as a share of the aimed view's (about the lens's share of the screen).
const LENS_SHARE: f32 = 0.62;
/// The scope camera sees this much more than the lens shows (for sway).
const MARGIN: f32 = 1.4;
/// The view shows in the lens from this far into the aim.
const FROM: f32 = 0.5;

pub(super) fn build(app: &mut App) {
    app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
        std::path::PathBuf::from(file!()).with_file_name("scope3d.wgsl"),
        std::path::Path::new(SHADER),
        include_bytes!("scope3d.wgsl").as_slice(),
    );
    app.add_plugins(MaterialPlugin::<ScopeGlass>::default())
        .add_systems(
            Update,
            (spawn_camera, glaze, drive, blur).chain().after(crate::weapons::WeaponSet).run_if(crate::state::in_game),
        )
        .add_systems(
            PostUpdate,
            eye_relief
                .after(crate::viewmodel::weapon_angles)
                .before(bevy::transform::TransformSystems::Propagate)
                .run_if(crate::state::in_game),
        );
}

/// 3D scopes are the style.
pub fn scope_3d() -> bool {
    super::scope::style_3d() && !crate::splitscreen::active()
}

/// Eye relief: how much further from the eye a 3D scope sits fully aimed
/// than CoD4's aimed pose puts it (inches), and how far that pose puts its
/// eyepiece (inches): rifle scopes, then ACOGs. CoD4's pose fills the screen
/// with the scope's tube (made for its full-screen overlay).
const RELIEF: [f32; 2] = [1.5, 2.0];
const EYEPIECE: [f32; 2] = [4.5, 6.0];

/// The optic's kind: 0 a rifle scope, 1 an ACOG.
fn optic(def: &crate::weapons::WeaponDef) -> usize {
    if def.ads_overlay.is_some() { 0 } else { 1 }
}

/// How far a 3D scope is pushed out from CoD4's aimed pose now (inches),
/// and how much smaller the eyepiece looks for it (its share of its size).
fn relief(w: &WeaponState) -> (f32, f32) {
    let k = optic(w.def);
    let env = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<f32>().ok());
    let (relief, eyepiece) = (env("COD4RW_SCOPE_RELIEF").unwrap_or(RELIEF[k]), env("COD4RW_SCOPE_EYEPIECE").unwrap_or(EYEPIECE[k]));
    let a = w.ads.clamp(0.0, 1.0);
    let out = relief * a * a * (3.0 - 2.0 * a);
    (out, eyepiece / (eyepiece + out))
}

/// A gun aimed through a 3D scope: one with a scope picture (CoD4's
/// sniper rifles) or an ACOG.
pub fn magnifies(def: &crate::weapons::WeaponDef) -> bool {
    def.ads_overlay.is_some() || def.name.contains("acog")
}

/// The eyepiece's glass.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct ScopeGlass {
    /// x: how far the view has come in (0..1), y: reticle (1 duplex, 2
    /// ACOG chevron), z: image scale (texture widths per unit of the eye's
    /// direction off the view's middle), w: the eyepiece's reach (tangent).
    #[uniform(0)]
    params: Vec4,
    /// xyz: the scope's axis (the gun's forward, world space). Not the
    /// lens's normals: the eyepiece is curved.
    #[uniform(3)]
    axis: Vec4,
    #[texture(1)]
    #[sampler(2)]
    view: Handle<Image>,
}

impl Material for ScopeGlass {
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/scope3d.wgsl".into()
    }

    fn enable_shadows() -> bool {
        false
    }
}

#[derive(Component)]
struct ScopeCamera3d;

/// The scope camera's picture and the glass made with it.
#[derive(Resource)]
struct Glass {
    material: Handle<ScopeGlass>,
    /// The camera renders until then, so its shaders are ready before the
    /// first aim (the first frames otherwise came out garbled).
    warm_until: f32,
}

/// How long a new scope camera warms up (seconds).
const WARM_UP: f32 = 1.5;

fn spawn_camera(
    mut commands: Commands,
    time: Res<Time>,
    glass: Option<Res<Glass>>,
    main: Query<(Entity, &Camera, Option<&Skybox>, Option<&DistanceFog>), With<MainCamera>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ScopeGlass>>,
) {
    if glass.is_some() || !scope_3d() {
        return;
    }
    let Ok((main, main_camera, skybox, fog)) = main.single() else { return };
    let image = images.add(Image::new_target_texture(RESOLUTION, RESOLUTION, TextureFormat::Rgba16Float, None));
    let mut camera = commands.spawn((
        Name::new("scope camera (3D)"),
        ScopeCamera3d,
        Camera3d::default(),
        Camera { order: -2, is_active: false, clear_color: main_camera.clear_color.clone(), ..default() },
        RenderTarget::Image(image.clone().into()),
        Projection::from(PerspectiveProjection { fov: 10f32.to_radians(), near: 0.05, far: 2000.0, ..default() }),
        Hdr,
        Msaa::Off,
        // The frame's finishing camera exposes and tonemaps it.
        Tonemapping::None,
        Transform::default(),
        ChildOf(main),
    ));
    if let Some(s) = skybox {
        camera.insert(s.clone());
    }
    if let Some(f) = fog {
        camera.insert(f.clone());
    }
    let material = materials.add(ScopeGlass { params: Vec4::ZERO, axis: Vec4::NEG_Z, view: image.clone() });
    commands.insert_resource(Glass { material, warm_until: time.elapsed_secs() + WARM_UP });
}

/// The eyepieces' own material, kept to put back.
#[derive(Component)]
struct Unglazed(Handle<StandardMaterial>);

/// Eyepieces take the glass in the 3D style, and their own back otherwise.
fn glaze(
    mut commands: Commands,
    glass: Option<Res<Glass>>,
    plain: Query<(Entity, &MeshMaterial3d<StandardMaterial>), (With<ScopeLens>, Without<Unglazed>)>,
    glazed: Query<(Entity, &Unglazed)>,
) {
    let on = scope_3d();
    if let (true, Some(glass)) = (on, glass.as_ref()) {
        for (e, m) in &plain {
            commands.entity(e).insert((Unglazed(m.0.clone()), MeshMaterial3d(glass.material.clone()))).remove::<MeshMaterial3d<StandardMaterial>>();
        }
    } else if !on {
        for (e, u) in &glazed {
            commands.entity(e).insert(MeshMaterial3d(u.0.clone())).remove::<(Unglazed, MeshMaterial3d<ScopeGlass>)>();
        }
    }
}

/// The gun pushed out along the view while aimed through a 3D scope, on
/// top of its pose (`crate::viewmodel::weapon_angles` sets it afresh each
/// frame).
fn eye_relief(player: Query<&WeaponState, With<LocalPlayer>>, mut gun: Query<&mut Transform, With<crate::viewmodel::ViewModelRoot>>) {
    let Some(w) = player.single().ok().filter(|w| scope_3d() && magnifies(w.def)) else { return };
    let (out, _) = relief(w);
    if out > 0.0 {
        for mut tf in &mut gun {
            // The camera looks down -Z.
            tf.translation.z -= crate::units::u(out);
        }
    }
}

/// While aimed through a 3D scope, the world outside it slightly out of
/// focus (the viewmodel and the lens's view, drawn by other cameras, stay
/// sharp): Bevy's depth of field on the world camera, focused right at the
/// eye so everything blurs alike, a few pixels at most. On for a moment at
/// the start too, so its shaders are ready before the first aim.
fn blur(
    mut commands: Commands,
    time: Res<Time>,
    glass: Option<Res<Glass>>,
    player: Query<&WeaponState, With<LocalPlayer>>,
    mut main: Query<(Entity, Option<&mut bevy::post_process::dof::DepthOfField>), With<MainCamera>>,
) {
    let Ok((e, dof)) = main.single_mut() else { return };
    let aimed = player.single().ok().filter(|w| scope_3d() && magnifies(w.def)).map_or(0.0, |w| w.ads.clamp(0.0, 1.0));
    let warming = glass.is_some_and(|g| time.elapsed_secs() < g.warm_until);
    // (`COD4RW_SCOPE_BLUR=<pixels>` for test runs.)
    let most = std::env::var("COD4RW_SCOPE_BLUR").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(BLUR_PIXELS);
    let size = most * aimed * aimed;
    match (dof, size > 0.05 || warming) {
        (Some(mut d), true) => {
            if (d.max_circle_of_confusion_diameter - size).abs() > 0.05 {
                d.max_circle_of_confusion_diameter = size;
            }
        }
        (None, true) => {
            commands.entity(e).insert(bevy::post_process::dof::DepthOfField {
                mode: bevy::post_process::dof::DepthOfFieldMode::Gaussian,
                focal_distance: 0.05,
                aperture_f_stops: 1.0,
                max_circle_of_confusion_diameter: size,
                max_depth: 1000.0,
                ..default()
            });
        }
        (Some(_), false) => {
            commands.entity(e).remove::<bevy::post_process::dof::DepthOfField>();
        }
        (None, false) => {}
    }
}

/// The world's blur fully aimed (pixels across).
const BLUR_PIXELS: f32 = 3.0;

fn drive(
    time: Res<Time>,
    glass: Option<Res<Glass>>,
    gun: Query<&GlobalTransform, With<crate::viewmodel::ViewModelRoot>>,
    player: Query<&WeaponState, With<LocalPlayer>>,
    mut camera: Query<(&mut Camera, &mut Projection), With<ScopeCamera3d>>,
    mut materials: ResMut<Assets<ScopeGlass>>,
) {
    let Some(glass) = glass else { return };
    let aimed = player.single().ok().filter(|w| scope_3d() && magnifies(w.def) && w.ads > FROM);
    let fade = aimed.map_or(0.0, |w| ((w.ads - FROM) / (1.0 - FROM)).clamp(0.0, 1.0));
    // The lens's reach (tangent), the gun's zoom over the aimed view round
    // it, and the scope camera's field to cover it with a margin.
    let outer = (super::LENS_OUTER_FOV.to_radians() * 0.5).tan();
    // (Smaller, the scope pushed out: `eye_relief`.)
    let reach = LENS_SHARE * outer * aimed.map_or(1.0, |w| relief(w).1);
    let (zoom, field) = aimed.map_or((1.0, 0.2), |w| {
        let ads = (w.def.ads_fov.max(1.0).to_radians() * 0.5).tan();
        let zoom = ads / outer;
        (zoom, reach * zoom * MARGIN)
    });
    for (mut cam, mut projection) in &mut camera {
        let active = aimed.is_some() || time.elapsed_secs() < glass.warm_until;
        if cam.is_active != active {
            cam.is_active = active;
        }
        if let Projection::Perspective(p) = projection.as_mut() {
            let fov = 2.0 * field.atan();
            if active && (p.fov - fov).abs() > 1e-5 {
                p.fov = fov;
            }
        }
    }
    let reticle = aimed.map_or(1.0, |w| if w.def.ads_overlay.is_some() { 1.0 } else { 2.0 });
    // An eye direction's tangent `d` off the middle shows the world at
    // `d * zoom`, which is `d * zoom / (2 * field)` of the picture's width.
    let params = Vec4::new(fade, reticle, zoom / (2.0 * field), reach);
    // The gun's forward: CoD models face +X.
    let axis = gun.iter().next().map_or(Vec4::NEG_Z, |g| (g.rotation() * Vec3::X).normalize_or(Vec3::NEG_Z).extend(0.0));
    if materials.get(&glass.material).is_some_and(|m| m.params != params || m.axis != axis)
        && let Some(mut m) = materials.get_mut(&glass.material)
    {
        m.params = params;
        m.axis = axis;
    }
}
