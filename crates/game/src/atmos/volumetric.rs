//! Sun shafts: Bevy's volumetric fog pass (`VolumetricFog` on the world
//! cameras, `VolumetricLight` on the map's sun) with our own shader in
//! place of Bevy's (`volumetric_fog.wgsl`, swapped in as the SSAO shader
//! is in `crate::player`).
//!
//! The air is the map's: its fog colour tints the shafts and its fog
//! density sets how thick the air is (Crash dusty, Bog brown, Overgrown
//! misty), thinning with height above the map's floor. The shafts only
//! reach as far as the sun's shadow cascades; past that `crate::fog`'s
//! distance fog carries on. The pass adds the sun's light scattered in
//! the air and takes away very little, so it doesn't fog the view twice.
//!
//! One fog volume follows player 1's camera. Not in splitscreen: Bevy's
//! pass draws over the whole target, not a camera's viewport.

use super::setting;
use crate::player::MainCamera;
use bevy::light::{FogVolume, VolumetricFog, VolumetricLight};
use bevy::prelude::*;

pub struct VolumetricPlugin;

impl Plugin for VolumetricPlugin {
    fn build(&self, app: &mut App) {
        let registry = app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>();
        registry.insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("volumetric_fog.wgsl"),
            std::path::Path::new("bevy_pbr/volumetric_fog/volumetric_fog.wgsl"),
            include_bytes!("volumetric_fog.wgsl").as_slice(),
        );
        app.add_systems(Update, (apply, tune).chain().run_if(crate::state::in_game))
            .add_systems(PostUpdate, follow.after(TransformSystems::Propagate).run_if(crate::state::in_game));
        // Its GPU time in `crate::perf`'s log ("volumetric_fog").
        if std::env::var("COD4RW_PERF").is_ok() {
            use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
            use bevy::core_pipeline::core_3d::main_transparent_pass_3d;
            if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
                render_app.add_systems(
                    Core3d,
                    (
                        // (Bevy's `volumetric_fog` runs between these sets.)
                        span_begin.in_set(Core3dSystems::MainPass).after(main_transparent_pass_3d),
                        span_end.in_set(Core3dSystems::EarlyPostProcess),
                    ),
                );
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Level {
    Off,
    Low,
    High,
}

impl Level {
    pub(super) fn of(settings: &crate::settings::Settings) -> Level {
        match setting(settings, "r_volumetric", "COD4RW_VOLUMETRIC").as_str() {
            "low" => Level::Low,
            "high" => Level::High,
            _ => Level::Off,
        }
    }

    /// Raymarch steps per pixel (packed toward the eye): High's only.
    fn steps(self) -> u32 {
        match self {
            Level::Off => 0,
            Level::Low => 0,
            Level::High => 32,
        }
    }
}

/// The air's one fog volume.
#[derive(Component)]
struct Air;

/// How far the shafts reach (metres): within the sun's shadow cascades.
const REACH: f32 = 50.0;

/// Full daylight for the shafts (lux), whatever share of the sun is live.
const DAYLIGHT: f32 = 12_000.0;

#[allow(clippy::type_complexity)]
fn apply(
    mut commands: Commands,
    settings: Res<crate::settings::Settings>,
    fog: Option<Res<crate::fog::MapFog>>,
    map: Option<Res<crate::world::MapInfo>>,
    cameras: Query<(Entity, Has<VolumetricFog>), With<crate::splitscreen::SlotCamera>>,
    suns: Query<(Entity, &DirectionalLight, Has<VolumetricLight>), Without<crate::model_lighting::ViewModelSun>>,
    air: Query<Entity, With<Air>>,
    mut last: Local<Option<(Level, bool)>>,
) {
    // The raymarched shafts are High's (Low is god rays alone, `super::godrays`).
    let level = match Level::of(&settings) {
        Level::High if !crate::splitscreen::active() => Level::High,
        _ => Level::Off,
    };
    let shadows = suns.iter().any(|(_, l, _)| l.shadow_maps_enabled);
    // The live sun's illuminance (lux): the shafts are lit as by full daylight.
    let illuminance = suns.iter().find(|(_, l, _)| l.shadow_maps_enabled).map_or(DAYLIGHT, |(_, l, _)| l.illuminance);
    let level = if shadows { level } else { Level::Off };
    let new_camera = cameras.iter().any(|(_, has)| has != (level != Level::Off));
    let new_sun = suns.iter().any(|(_, _, has)| has != (level != Level::Off));
    let fog_changed = fog.as_ref().is_some_and(|f| f.is_changed());
    if *last == Some((level, shadows)) && !new_camera && !new_sun && !fog_changed && !air.is_empty() == (level != Level::Off) {
        return;
    }
    *last = Some((level, shadows));
    info!("volumetric lighting: {level:?} (live sun {illuminance:.0} lux)");
    for (e, _) in &cameras {
        match level {
            Level::Off => {
                commands.entity(e).remove::<VolumetricFog>();
            }
            _ => {
                commands.entity(e).insert(VolumetricFog {
                    // (Our shader adds no ambient light: the distance fog has it.)
                    ambient_intensity: 0.0,
                    // Our shader's dither: 0, the same each frame (no TAA to
                    // smooth a moving one).
                    jitter: 0.0,
                    step_count: level.steps(),
                    ..default()
                });
            }
        }
    }
    for (e, _, _) in &suns {
        match level {
            Level::Off => {
                commands.entity(e).remove::<VolumetricLight>();
            }
            _ => {
                commands.entity(e).insert(VolumetricLight);
            }
        }
    }
    for e in &air {
        commands.entity(e).despawn();
    }
    if level == Level::Off {
        return;
    }
    // The map's air: its fog's colour, and thickness from its density
    // (Crash's 3500-unit halfway is ~0.008/m): the shafts are a few times
    // that, so one through a window shows (see the shader's levelling off).
    let (tint, fog_density) = super::map_fog(fog.as_deref()).map_or((Vec3::ONE, 0.006), |(c, d)| (c / c.max_element().max(1e-3), d));
    let density = (fog_density * 6.0).clamp(0.025, 0.06);
    // Half way to white: the shafts keep the sun's colour, tinted.
    let tint = (tint + Vec3::ONE) * 0.5;
    // The floor the air thins up from: the spawns' average height.
    let ground = map.as_ref().filter(|m| !m.spawns.is_empty()).map_or(0.0, |m| {
        m.spawns.iter().map(|s| s.pos.y).sum::<f32>() / m.spawns.len() as f32
    });
    commands.spawn((
        Name::new("air"),
        Air,
        FogVolume {
            fog_color: Color::linear_rgb(tint.x, tint.y, tint.z),
            density_factor: density,
            // Extinction, as a share of `density_factor`: about the map
            // fog's own (a tenth of the view hidden at the shafts' reach),
            // so the shafts add light without fogging the view twice.
            absorption: (fog_density * 0.3 / density).min(1.0),
            scattering: 1.0,
            // Brightest looking toward the sun.
            scattering_asymmetry: 0.4,
            light_intensity: intensity() * (DAYLIGHT / illuminance.max(1.0)).clamp(1.0, 20.0), // (`tune` keeps it current)
            // Our shader's own: x how far the shafts reach, y how fast the
            // air thins with height (per metre), z the floor's height.
            density_texture_offset: Vec3::new(REACH, 0.04, ground),
            ..default()
        },
        Transform::from_scale(Vec3::splat(REACH * 2.5)),
    ));
}

/// Debug aid: `COD4RW_VOLUMETRIC_GAIN` scales the shafts' light.
fn intensity() -> f32 {
    std::env::var("COD4RW_VOLUMETRIC_GAIN").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0)
}

/// The air stays round player 1's eye (the pass draws it as a full-screen
/// quad while the eye is inside it).
/// (After transforms propagate, so it sets both.)
fn follow(camera: Query<&GlobalTransform, (With<MainCamera>, Without<Air>)>, mut air: Query<(&mut Transform, &mut GlobalTransform), With<Air>>) {
    let Ok(eye) = camera.single() else { return };
    for (mut t, mut g) in &mut air {
        t.translation = eye.translation();
        *g = GlobalTransform::from(*t);
    }
}

/// (Both on one thread, as the recorder pairs spans by thread.)
fn span_begin(_main: bevy::ecs::system::NonSendMarker, _view: bevy::render::renderer::ViewQuery<&VolumetricFog>, mut ctx: bevy::render::renderer::RenderContext) {
    use bevy::render::diagnostic::RecordDiagnostics;
    let diagnostics = ctx.diagnostic_recorder();
    diagnostics.as_deref().begin_time_span(ctx.command_encoder(), "volumetric_fog".into());
}

fn span_end(_main: bevy::ecs::system::NonSendMarker, _view: bevy::render::renderer::ViewQuery<&VolumetricFog>, mut ctx: bevy::render::renderer::RenderContext) {
    use bevy::render::diagnostic::RecordDiagnostics;
    let diagnostics = ctx.diagnostic_recorder();
    diagnostics.as_deref().end_time_span(ctx.command_encoder());
}

/// The shafts' light each frame. They're lit as by full daylight from the
/// map's own sun (CoD4 baked most of it into the lightmaps), but never
/// lifted past that sun: the showcase's moon, a storm or a night sun keep
/// their weakness (the moon lifted to daylight made white walls of haze).
fn tune(
    tod: Res<super::climate::TimeOfDay>,
    weather: Res<super::climate::Weather>,
    suns: Query<&DirectionalLight, Without<crate::model_lighting::ViewModelSun>>,
    mut air: Query<&mut FogVolume, With<Air>>,
) {
    let Some(now) = suns.iter().find(|l| l.shadow_maps_enabled).map(|l| l.illuminance) else { return };
    let reference = tod.map.filter(|_| tod.enabled).map_or(now, |m| m.illuminance);
    let mut want = intensity() * (DAYLIGHT / reference.max(1.0)).clamp(1.0, 20.0);
    if tod.enabled {
        // (The live light already carries the moon's and storm's dimming;
        // a storm's air is also full of rain, not dust in sunbeams.)
        let storm = if weather.enabled { (weather.rain / 0.8).clamp(0.0, 1.0) } else { 0.0 };
        want *= 1.0 - 0.7 * storm;
    }
    for mut a in &mut air {
        if (a.light_intensity - want).abs() > 1e-3 * want.max(1e-3) {
            a.light_intensity = want;
        }
    }
}
