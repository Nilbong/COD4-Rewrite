//! Kill streaks' models and effects drawn once under the load screen, so
//! the first call-in of a match doesn't hitch.
//!
//! A streak's first appearance built its meshes, materials and textures,
//! uploaded them and compiled their pipelines all at once: the first
//! airstrike of a match cost 270-470 ms frames (Carentan, 2026-10-14). So
//! while [`crate::settle`] holds the load screen, every streak model is
//! spawned well in front of the camera for a few frames and despawned, and
//! the streaks' effects are queued to preload; so are the call-ins'
//! hands, their props and the laptop's screen camera, as the gun camera
//! and the screen draw them. The load screen stays up
//! until that's done ([`holding`]) and the pipelines are compiled. No
//! sounds, no game state: only things to draw.

use super::models::StreakModels;
use super::spawn_static_model;
use crate::content::Content;
use crate::player::MainCamera;
use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Streak models spawned as static models (as the streaks spawn them).
const STATIC: &[&str] = &[
    // Airstrike: the jets and their bomb.
    "vehicle_mig29_desert",
    "projectile_cbu97_clusterbomb",
    // Attack helicopter (and the care package's courier).
    "vehicle_cobra_helicopter_fly",
    "vehicle_mi24p_hind_desert",
    // Call-in props in the hands.
    "com_laptop_2_open",
    "com_cellphone_on",
];

/// Streak effects (each streak's `fx.play` names).
const FX: &[&str] = &[
    "fire/jet_afterburner",
    "smoke/jet_contrail",
    "explosions/clusterbomb",
    "explosions/aerial_explosion",
    "explosions/aerial_explosion_large",
    "explosions/helicopter_explosion_cobra",
    "smoke/smoke_trail_black_heli",
    "smoke/smoke_trail_white_heli",
    "smoke/thin_light_smoke_m",
    "smoke/thin_light_smoke_l",
    "explosions/small_vehicle_explosion",
    "smoke/damaged_vehicle_smoke",
    // The sentry gun's muzzle flash.
    "muzzleflashes/saw_flash_wv",
];

/// Frames the models stay drawn.
const FRAMES: u32 = 8;
/// Give up waiting for the streaks' content after this long (real seconds).
const GIVE_UP: f32 = 3.0;

static HOLDING: AtomicBool = AtomicBool::new(false);

/// The pre-warm isn't done: the load screen stays up.
pub fn holding() -> bool {
    HOLDING.load(Ordering::Relaxed)
}

#[derive(Resource, Default)]
struct Prewarm {
    /// Real time the match was entered.
    since: Option<f32>,
    /// Drawn for this many frames (Some once spawned).
    frames: Option<u32>,
    entities: Vec<Entity>,
    done: bool,
}

pub(super) fn build(app: &mut App) {
    app.init_resource::<Prewarm>()
        .add_systems(OnEnter(crate::state::GameState::InGame), reset)
        .add_systems(Update, (spawn_props, spawn_static, spawn_gun_layer, finish).chain().run_if(crate::state::in_game))
        .add_systems(OnExit(crate::state::GameState::InGame), cleanup);
}

fn reset(mut pw: ResMut<Prewarm>) {
    *pw = Prewarm::default();
    HOLDING.store(true, Ordering::Relaxed);
}

/// Where to put them: well in front of the camera, side by side.
fn spot(camera: &GlobalTransform, i: usize) -> Transform {
    let side = (i as f32 - 4.0) * 6.0;
    Transform::from_translation(camera.translation() + camera.forward() * 30.0 + camera.right() * side)
}

/// Ready to spawn this frame (the load screen up, the streaks' props
/// loaded, a camera), or not yet; gives up after [`GIVE_UP`].
fn due(pw: &mut Prewarm, real: &Time<Real>, ready: bool) -> bool {
    if pw.done || pw.frames.is_some() {
        return false;
    }
    let since = *pw.since.get_or_insert(real.elapsed_secs());
    // (`COD4RW_PERFOLD=prewarm`: none, to compare.)
    if !crate::settle::settling() || real.elapsed_secs() - since > GIVE_UP || crate::perf::old("prewarm") {
        pw.done = true;
        HOLDING.store(false, Ordering::Relaxed);
        return false;
    }
    ready
}

/// The care package's crate, the sentry's gun and tripod, and the courier
/// helicopter as the care package spawns it.
fn spawn_props(
    mut commands: Commands,
    mut pw: ResMut<Prewarm>,
    real: Res<Time<Real>>,
    mut models: StreakModels,
    props: Res<super::models::StreakProps>,
    camera: Query<&GlobalTransform, With<MainCamera>>,
) {
    let Ok(camera) = camera.single() else { return };
    let ready = !props.busy() && models.content().is_some();
    if !due(&mut pw, &real, ready) {
        return;
    }
    let names: [&[&str]; 4] = [
        &[super::carepackage::CRATE_MODEL],
        &[super::sentry::TRIPOD],
        &super::sentry::MODEL,
        &["vehicle_cobra_helicopter_fly", "vehicle_mi24p_hind_desert"],
    ];
    for (i, names) in names.iter().enumerate() {
        let owner = commands.spawn((Name::new("streak prewarm"), spot(camera, i), Visibility::Visible)).id();
        models.spawn(&mut commands, names, owner, true);
        pw.entities.push(owner);
    }
}

/// The static models, and the effects queued to preload.
fn spawn_static(
    mut commands: Commands,
    mut pw: ResMut<Prewarm>,
    mut content: Option<ResMut<Content>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    fx: Option<ResMut<crate::fx::Effects>>,
    camera: Query<&GlobalTransform, With<MainCamera>>,
) {
    // (Right after the props spawned: same frame, same checks.)
    if pw.done || pw.frames.is_some() || pw.entities.is_empty() {
        return;
    }
    let Ok(camera) = camera.single() else { return };
    if let Some(content) = content.as_deref_mut() {
        for (i, name) in STATIC.iter().enumerate() {
            if let Some(e) = spawn_static_model(&mut commands, content, &mut meshes, &mut materials, &mut images, name, spot(camera, 4 + i)) {
                pw.entities.push(e);
            }
        }
    }
    if let Some(mut fx) = fx {
        for name in FX {
            fx.preload_effect(name);
        }
    }
    pw.frames = Some(0);
    info!("kill streaks: drawing {} streak models once under the load screen", pw.entities.len());
}

/// In the hands, as the gun camera draws them (their own pipelines): the
/// call-ins' viewmodels and the phone and laptop they hold, the laptop's
/// screen on its lid; and a camera like the laptop screen's own
/// (`ui::laptop_screen`: 2D, UI into an `Rgba8UnormSrgb` picture) drawing
/// a little UI of the same kinds.
#[allow(clippy::too_many_arguments)]
fn spawn_gun_layer(
    mut commands: Commands,
    mut pw: ResMut<Prewarm>,
    mut content: Option<ResMut<Content>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    gun_camera: Query<Entity, With<crate::player::ViewModelCamera>>,
) {
    // (Once, the frame the others spawn.)
    if pw.done || pw.frames != Some(0) {
        return;
    }
    let layer = bevy::camera::visibility::RenderLayers::layer(crate::splitscreen::viewmodel_layer(0));
    if let (Some(content), Ok(camera)) = (content.as_deref_mut(), gun_camera.single()) {
        let views: Vec<String> = ["radar", "briefcase_bomb", "helicopter"]
            .iter()
            .filter_map(|n| crate::loadout::bot_weapon(content, &format!("{n}:")).map(|d| d.viewmodel.clone()))
            .chain(["com_cellphone_on".to_owned(), "com_laptop_2_open".to_owned()])
            .collect();
        for (i, name) in views.iter().enumerate() {
            let Some(model) = content.model(name, &mut meshes, &mut materials, &mut images, &mut bindposes) else { continue };
            let at = Vec3::new((i as f32 - 2.0) * 0.25, 0.0, -0.8);
            let anchor = commands.spawn((Name::new("streak prewarm (hands)"), Transform::from_translation(at), Visibility::Visible, ChildOf(camera))).id();
            let mut skeleton = crate::models::Skeleton::default();
            crate::models::spawn_model(
                &mut commands,
                &mut skeleton,
                crate::models::SpawnModel { model: &model, owner: anchor, attach_to: None, layers: Some(layer.clone()), shadows: false },
            );
            pw.entities.push(anchor);
        }
        // The laptop's screen quad (unlit, its picture).
        if let Some(image) = super::hands::SCREEN_IMAGE.get() {
            let e = commands
                .spawn((
                    Mesh3d(meshes.add(Rectangle::new(0.2, 0.12))),
                    MeshMaterial3d(materials.add(StandardMaterial { base_color_texture: Some(image.clone()), unlit: true, ..default() })),
                    Transform::from_xyz(0.0, 0.15, -0.8),
                    bevy::light::NotShadowCaster,
                    layer.clone(),
                    ChildOf(camera),
                ))
                .id();
            pw.entities.push(e);
        }
    }
    // The laptop screen's kind of camera and UI.
    let picture = images.add(Image::new_target_texture(64, 40, bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb, None));
    let screen = commands
        .spawn((
            Name::new("streak prewarm (screen camera)"),
            Camera2d,
            Camera { order: -31, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
            bevy::camera::RenderTarget::Image(picture.into()),
        ))
        .id();
    let root = commands.spawn((Node { width: px(64), height: px(40), ..default() }, BackgroundColor(Color::BLACK), UiTargetCamera(screen))).id();
    commands.spawn((ImageNode::default(), Node { width: px(16), height: px(16), border: UiRect::all(px(1.0)), ..default() }, BorderColor::all(Color::WHITE), ChildOf(root)));
    commands.spawn((Text::new("A"), TextFont { font_size: bevy::text::FontSize::Px(14.0), ..default() }, ChildOf(root)));
    commands.spawn((Node { width: px(8), height: px(2), ..default() }, BackgroundColor(Color::WHITE), Outline::new(px(1.0), px(0.0), Color::BLACK), ChildOf(root)));
    pw.entities.extend([screen, root]);
}

/// After [`FRAMES`] frames: gone again.
fn finish(mut commands: Commands, mut pw: ResMut<Prewarm>) {
    if pw.done {
        return;
    }
    let pw = &mut *pw;
    let Some(frames) = pw.frames.as_mut() else { return };
    *frames += 1;
    if *frames >= FRAMES {
        for e in std::mem::take(&mut pw.entities) {
            commands.entity(e).try_despawn();
        }
        pw.done = true;
        HOLDING.store(false, Ordering::Relaxed);
    }
}

/// Leaving the match mid-way: nothing left behind.
fn cleanup(mut commands: Commands, mut pw: ResMut<Prewarm>) {
    for e in std::mem::take(&mut pw.entities) {
        commands.entity(e).try_despawn();
    }
    HOLDING.store(false, Ordering::Relaxed);
}
