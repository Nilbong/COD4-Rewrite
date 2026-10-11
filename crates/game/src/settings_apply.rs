//! Putting the settings ([`crate::settings`]) into effect, as they change:
//! the window and frame rate, the camera's anti-aliasing, glow, ambient
//! occlusion and brightness, the sun's shadows, the models' shadows and
//! draw distance, effects' density, the volumes; and the values other
//! systems ask for every frame (field of view, mouse, HUD switches), kept
//! here where any of them can read them.

use crate::settings::Settings;
use bevy::anti_alias::fxaa::Fxaa;
use bevy::anti_alias::smaa::{Smaa, SmaaPreset};
use bevy::camera::visibility::VisibilityRange;
use bevy::light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap, NotShadowCaster};
use bevy::pbr::ScreenSpaceAmbientOcclusion;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::render::view::ColorGrading;
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, VideoModeSelection, WindowMode};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub struct SettingsApplyPlugin;

impl Plugin for SettingsApplyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (keep_values, window, camera, sun, models, brightness, mute_unfocused))
            .add_systems(Last, frame_limit);
    }
}

/// A float kept for anyone to read.
struct Value(AtomicU32);

impl Value {
    const fn new(v: f32) -> Value {
        Value(AtomicU32::new(v.to_bits()))
    }
    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

static HIP_FOV: Value = Value::new(65.0);
static MOUSE: Value = Value::new(1.0);
static ADS_MOUSE: Value = Value::new(1.0);
static INVERT: AtomicBool = AtomicBool::new(false);
static BLOOM: AtomicBool = AtomicBool::new(true);
static MODERN_HUD: AtomicBool = AtomicBool::new(true);
static FPS_LIMIT: Value = Value::new(0.0);
static EFFECTS_VOLUME: Value = Value::new(1.0);
static MUSIC_VOLUME: Value = Value::new(1.0);
static VOICE_VOLUME: Value = Value::new(1.0);
static UI_VOLUME: Value = Value::new(1.0);
static MASTER_VOLUME: Value = Value::new(0.9);
static FX_DENSITY: Value = Value::new(1.0);
static CORPSES: Value = Value::new(8.0);
static OVERLAY: Value = Value::new(0.0);
/// The HUD's switches, in [`Hud`] order.
static HUD: [AtomicBool; 8] = [const { AtomicBool::new(true) }; 8];

/// The HUD's switches.
#[derive(Clone, Copy, Debug)]
pub enum Hud {
    Crosshair,
    HitMarkers,
    KillFeed,
    MinimapRotates,
    DamageDirection,
    ScorePopups,
    HitMarkerSound,
    MapEffects,
}

const HUD_DVARS: [(Hud, &str); 8] = [
    (Hud::Crosshair, "cg_crosshair"),
    (Hud::HitMarkers, "cg_hitmarkers"),
    (Hud::KillFeed, "cg_killfeed"),
    (Hud::MinimapRotates, "cg_minimap_rotate"),
    (Hud::DamageDirection, "cg_damage_direction"),
    (Hud::ScorePopups, "cg_xp_popups"),
    (Hud::HitMarkerSound, "cg_hitmarker_sound"),
    (Hud::MapEffects, "fx_mapfx"),
];

/// A HUD switch (and the map effects').
pub fn hud(which: Hud) -> bool {
    HUD[which as usize].load(Ordering::Relaxed)
}

/// The hip field of view (degrees, vertical as CoD4's `cg_fov`).
pub fn hip_fov() -> f32 {
    HIP_FOV.get()
}

/// The mouse's scale on CoD4's base turn (sensitivity 5 is 1), at an aim
/// fraction `ads` (the aiming sensitivity eased in).
pub fn mouse_scale(ads: f32) -> f32 {
    MOUSE.get() * (1.0 + (ADS_MOUSE.get() - 1.0) * ads.clamp(0.0, 1.0))
}

pub fn invert_mouse() -> bool {
    INVERT.load(Ordering::Relaxed)
}

/// The mix's headroom: every sound plays at this share, so that sounds
/// stacking up (the player's gunshot and its layers over everything else,
/// a sound panned hard into one ear) stay under full scale instead of
/// clipping into a crackle. Nothing limits the sum of the sounds after
/// they're mixed. `audio.headroom` in `tuning.txt` adjusts it.
const HEADROOM: f32 = 0.6;

/// A sound's volume scale by what it is (and the master).
pub fn volume(category: Sound) -> f32 {
    crate::audio::audible()
        * crate::tune::get("audio.headroom", HEADROOM).clamp(0.05, 1.0)
        * MASTER_VOLUME.get()
        * match category {
            Sound::Effects => EFFECTS_VOLUME.get(),
            Sound::Music => MUSIC_VOLUME.get(),
            Sound::Voice => VOICE_VOLUME.get(),
            Sound::Menu => UI_VOLUME.get(),
        }
}

/// What a sound is, for its volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Effects,
    Music,
    Voice,
    Menu,
}

/// How many particles effects make: 1 all, less on lower settings.
pub fn fx_density() -> f32 {
    FX_DENSITY.get()
}

/// How many corpses stay.
pub fn corpses() -> usize {
    CORPSES.get() as usize
}

/// The performance overlay: 0 off, 1 FPS, 2 with the frame time.
/// The settings' HUD Style: Modern unless Classic.
pub fn modern_hud() -> bool {
    MODERN_HUD.load(Ordering::Relaxed)
}

/// Whether the frame glows: Bevy's bloom, or a map's own glow
/// (`crate::vision`).
pub fn bloom() -> bool {
    BLOOM.load(Ordering::Relaxed)
}

pub fn overlay() -> u8 {
    OVERLAY.get() as u8
}

/// Copy the values others read every frame.
fn keep_values(settings: Res<Settings>) {
    if !settings.is_changed() {
        return;
    }
    let s = &*settings;
    HIP_FOV.set(s.num("cg_fov").clamp(50.0, 120.0));
    MOUSE.set((s.num("sensitivity") / 5.0).clamp(0.02, 10.0));
    ADS_MOUSE.set(s.num("cg_ads_sens").clamp(0.1, 4.0));
    INVERT.store(s.on("m_invert"), Ordering::Relaxed);
    BLOOM.store(s.on("r_bloom"), Ordering::Relaxed);
    RICH.store(rich_tonemap(s), Ordering::Relaxed);
    MODERN_HUD.store(s.text("cg_hudstyle") != "classic", Ordering::Relaxed);
    FPS_LIMIT.set(s.num("com_maxfps").max(0.0));
    MASTER_VOLUME.set(s.num("snd_volume") / 100.0);
    EFFECTS_VOLUME.set(s.num("snd_effects") / 100.0);
    MUSIC_VOLUME.set(s.num("snd_music") / 100.0);
    VOICE_VOLUME.set(s.num("snd_voice") / 100.0);
    UI_VOLUME.set(s.num("snd_ui") / 100.0);
    FX_DENSITY.set(match s.text("fx_density") {
        "low" => 0.35,
        "medium" => 0.65,
        _ => 1.0,
    });
    CORPSES.set(s.num("r_corpses").max(1.0));
    OVERLAY.set(s.num("cg_drawfps"));
    for (which, dvar) in HUD_DVARS {
        HUD[which as usize].store(s.on(dvar), Ordering::Relaxed);
    }
    crate::ragdoll::set_enabled(s.on("ragdoll_enable"));
    crate::first_person::apply(s);
    crate::lightmaps::set_rebaked(s.text("r_lightmaps") == "rebaked");
    if let Ok(mut k) = crate::hq::CALL_KEY.lock() {
        *k = s.bindings().key_name(crate::bindings::Action::Frag);
    }
    if let Ok(mut k) = crate::splitscreen::USE_KEY.lock() {
        *k = s.bindings().key_name(crate::bindings::Action::Use);
    }
    crate::clutter::set_enabled(s.on("phys_clutter"));
    crate::textures::set_anisotropy(match s.text("r_texfilter") {
        "bilinear" => 0,
        "trilinear" => 1,
        v => v.parse().unwrap_or(16),
    });
    if let Some(mut pad) = PAD.lock().ok() {
        *pad = Some(PadValues {
            sensitivity: s.num("pad_sens"),
            ads: s.num("pad_ads_sens"),
            deadzones: (s.num("pad_deadzone_left"), s.num("pad_deadzone_right")),
            invert: s.on("pad_invert"),
            aim_assist: s.on("pad_aim_assist"),
            rumble: s.on("pad_rumble"),
            tactical: s.text("pad_layout") == "tactical",
            glyphs: s.text("pad_glyphs").to_owned(),
        });
    }
}

/// The controller's settings, for [`crate::gamepad`] to take.
#[derive(Clone, Debug)]
pub struct PadValues {
    pub sensitivity: f32,
    pub ads: f32,
    pub deadzones: (f32, f32),
    pub invert: bool,
    pub aim_assist: bool,
    pub rumble: bool,
    pub tactical: bool,
    pub glyphs: String,
}

pub static PAD: std::sync::Mutex<Option<PadValues>> = std::sync::Mutex::new(None);

/// A background sim's window is its own (small, out of the way).
fn sim() -> bool {
    std::env::var_os("COD4RW_SIM").is_some() || std::env::var_os("COD4RW_RES").is_some()
}

/// Display mode, resolution and vsync.
fn window(
    settings: Res<Settings>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    monitors: Query<(), With<bevy::window::Monitor>>,
    mut placed_on: Local<Option<String>>,
) {
    if !settings.is_changed() || sim() {
        return;
    }
    // The settings' Monitor: the current one, the primary, or one by number
    // (if there's that many).
    let chosen = settings.text("r_monitor").to_owned();
    let monitor = match chosen.as_str() {
        "primary" => MonitorSelection::Primary,
        n => match n.parse::<usize>() {
            Ok(i) if i < monitors.iter().count() => MonitorSelection::Index(i),
            _ => MonitorSelection::Current,
        },
    };
    let size = settings.text("r_resolution").split_once('x').and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)));
    let mode = match settings.text("r_displaymode") {
        "borderless" => WindowMode::BorderlessFullscreen(monitor),
        "fullscreen" => WindowMode::Fullscreen(
            monitor,
            match size {
                Some((w, h)) => VideoModeSelection::Specific(bevy::window::VideoMode { physical_size: UVec2::new(w, h), bit_depth: 32, refresh_rate_millihertz: 0 }),
                None => VideoModeSelection::Current,
            },
        ),
        _ => WindowMode::Windowed,
    };
    if window.mode != mode {
        window.mode = mode;
    }
    // A window moves to the chosen screen when the choice changes (not with
    // every other setting, nor at start with "current").
    if placed_on.as_deref() != Some(chosen.as_str()) {
        let first = placed_on.is_none();
        *placed_on = Some(chosen.clone());
        if mode == WindowMode::Windowed && !(first && chosen == "current") && monitor != MonitorSelection::Current {
            window.position = WindowPosition::Centered(monitor);
        }
    }
    // Debug runs' own window size (`COD4RW_RES`) and vsync off
    // (`COD4RW_PERF=novsync`) win over the saved settings.
    let debug_size = std::env::var_os("COD4RW_RES").is_some();
    let debug_novsync = std::env::var("COD4RW_PERF").is_ok_and(|p| p.split(',').any(|t| t.trim() == "novsync"));
    if let (Some((w, h)), WindowMode::Windowed, false) = (size, mode, debug_size) {
        if window.resolution.physical_width() != w || window.resolution.physical_height() != h {
            window.resolution.set_physical_resolution(w, h);
        }
    }
    let present = if settings.on("r_vsync") && !debug_novsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
    if window.present_mode != present && !debug_novsync {
        window.present_mode = present;
    }
}

/// The frame rate limit: sleep the rest of the frame's time.
fn frame_limit(mut last: Local<Option<std::time::Instant>>) {
    let limit = FPS_LIMIT.get();
    let now = std::time::Instant::now();
    if limit >= 1.0 && !sim() {
        if let Some(prev) = *last {
            let frame = std::time::Duration::from_secs_f64(1.0 / limit as f64);
            let spent = now - prev;
            if spent < frame {
                std::thread::sleep(frame - spent);
            }
        }
    }
    *last = Some(std::time::Instant::now());
}

/// Anti-aliasing and glow on the cameras that finish the frame (the
/// viewmodel's), ambient occlusion on the world's; anew for new cameras.
/// Only what's different is changed: any setting changing (a volume
/// slider dragged) re-inserted them all, each a pipeline or texture
/// rebuild, every frame of the drag.
#[allow(clippy::type_complexity)]
fn camera(
    mut commands: Commands,
    settings: Res<Settings>,
    finishing: Query<(Entity, Ref<crate::player::ViewModelCamera>, Has<Bloom>, Option<&Smaa>, Has<Fxaa>, Option<&Tonemapping>)>,
    world: Query<(Entity, Ref<crate::player::MainCamera>, Has<ScreenSpaceAmbientOcclusion>)>,
) {
    for (e, marker, bloom, smaa, fxaa, tonemapping) in &finishing {
        if !(settings.is_changed() || marker.is_added()) {
            continue;
        }
        let mut c = commands.entity(e);
        let smaa_preset = smaa.map(|s| s.preset);
        match settings.text("r_aa") {
            "off" => {
                if smaa.is_some() || fxaa {
                    c.remove::<(Smaa, Fxaa)>();
                }
            }
            "fxaa" => {
                if smaa.is_some() || !fxaa {
                    c.remove::<Smaa>().insert(Fxaa::default());
                }
            }
            "smaa" => {
                if fxaa || !matches!(smaa_preset, Some(SmaaPreset::Medium)) {
                    c.remove::<Fxaa>().insert(Smaa { preset: SmaaPreset::Medium });
                }
            }
            _ => {
                if fxaa || !matches!(smaa_preset, Some(SmaaPreset::High)) {
                    c.remove::<Fxaa>().insert(Smaa { preset: SmaaPreset::High });
                }
            }
        }
        // (Rich is graded on top: `crate::vision`.)
        if tonemapping != Some(&Tonemapping::AgX) {
            c.insert(Tonemapping::AgX);
        }
        // A map's own glow (its vision file) takes Bevy's off again
        // (`crate::vision`).
        match (settings.on("r_bloom"), bloom) {
            (false, true) => {
                c.remove::<Bloom>();
            }
            (true, false) => {
                c.insert(Bloom::NATURAL);
            }
            _ => {}
        }
    }
    // Ambient occlusion: not in splitscreen, nor with ray traced lighting.
    let rt = crate::ui::lighting() != crate::rtgi::Lighting::Baked;
    for (e, marker, ssao) in &world {
        if !(settings.is_changed() || marker.is_added()) || crate::splitscreen::active() {
            continue;
        }
        match (settings.on("r_ssao") && !rt, ssao) {
            (false, true) => {
                commands.entity(e).remove::<ScreenSpaceAmbientOcclusion>();
            }
            (true, false) => {
                commands.entity(e).insert(crate::player::ssao());
            }
            _ => {}
        }
    }
}

/// The sun's shadows: how far, how many cascades and how sharp.
fn sun(
    mut commands: Commands,
    settings: Res<Settings>,
    // (The viewmodels' suns follow the map's: `crate::model_lighting`.)
    mut suns: Query<(Entity, &mut DirectionalLight), Without<crate::model_lighting::ViewModelSun>>,
    mut map_size: ResMut<DirectionalLightShadowMap>,
    mut applied: Local<Option<(usize, f32)>>,
) {
    // Splitscreen keeps its lighter shadows (`crate::splitscreen`).
    if crate::splitscreen::active() {
        return;
    }
    let (on, cascades, distance, size) = match settings.text("r_shadows") {
        // Black ops' measured trade-offs (2026-10-06).
        "off" => (false, 1, 30.0, 1024),
        "low" => (true, 1, 30.0, 1024),
        "medium" => (true, 2, 60.0, 1024),
        // Ultra at 2048: 4096 cost ~7 ms on open maps (Bloc) for no visible
        // difference (perf, 2026-10-08); 3 cascades over 120 m look the same
        // as 4 (A/B on four maps) and save ~1 ms.
        "ultra" => (true, 3, 120.0, 2048),
        _ => (true, 2, 80.0, 2048),
    };
    if settings.is_changed() && map_size.size != size {
        map_size.size = size;
    }
    // The cascades anew only when they change (or for a new sun).
    let new_cascades = *applied != Some((cascades, distance));
    let mut any = false;
    for (e, mut light) in &mut suns {
        let added = light.is_added();
        if !(settings.is_changed() || added) {
            continue;
        }
        any = true;
        if light.shadow_maps_enabled != on {
            light.shadow_maps_enabled = on;
        }
        if new_cascades || added {
            let config = CascadeShadowConfigBuilder { num_cascades: cascades, maximum_distance: distance, first_cascade_far_bound: 12f32.min(distance), ..default() };
            commands.entity(e).insert(config.build());
        }
    }
    if any {
        *applied = Some((cascades, distance));
    }
}

/// The static models' (batches') shadows and draw distance: now and then,
/// as they may still be arriving.
#[derive(Component)]
struct BaseRange(VisibilityRange);

#[allow(clippy::type_complexity)]
fn models(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    names: Query<(Entity, &Name)>,
    children: Query<&Children>,
    meshes: Query<(Has<NotShadowCaster>, Option<&VisibilityRange>, Option<&BaseRange>, Has<crate::world::ShadowViaProxy>), With<Mesh3d>>,
    mut next: Local<f32>,
) {
    let now = time.elapsed_secs();
    if now < *next && !settings.is_changed() {
        return;
    }
    *next = now + 1.0;
    let Some((root, _)) = names.iter().find(|(_, n)| n.as_str() == "static models") else { return };
    let shadows = settings.on("r_propshadows") && !crate::splitscreen::active();
    let reach = match settings.text("r_drawdistance") {
        "near" => 0.5,
        "medium" => 0.75,
        _ => 1.0,
    };
    for model in children.iter_descendants(root) {
        let Ok((no_shadow, range, base, via_proxy)) = meshes.get(model) else { continue };
        // Opaque batches shadow through the merged proxy, which this switches.
        match (shadows, no_shadow) {
            _ if via_proxy => {}
            (true, true) => {
                commands.entity(model).remove::<NotShadowCaster>();
            }
            (false, false) => {
                commands.entity(model).insert(NotShadowCaster);
            }
            _ => {}
        }
        // CoD4's own cull distance, drawn nearer.
        if let Some(range) = range {
            let base = base.map_or_else(|| range.clone(), |b| b.0.clone());
            let mut scaled = base.clone();
            scaled.end_margin = (base.end_margin.start * reach)..(base.end_margin.end * reach);
            if *range != scaled {
                commands.entity(model).insert((scaled, BaseRange(base)));
            }
        }
    }
}

/// Brightness: the frame's exposure, on every camera that grades colour.
fn brightness(settings: Res<Settings>, mut grading: Query<&mut ColorGrading>) {
    // (Plus Headquarters' outdoor darkening, `crate::hq`.)
    // (And the showcase's per-map offset, `crate::atmos::climate`.)
    let exposure = settings.num("r_gamma").clamp(0.3, 3.0).log2() + crate::hq::exposure_offset() + crate::atmos::climate::exposure_offset();
    for mut g in &mut grading {
        if (g.global.exposure - exposure).abs() > 1e-4 {
            g.global.exposure = exposure;
        }
    }
}

/// The look (`r_tonemap`): Natural, or Rich (`aces`, its old name), which
/// is Natural with more contrast and colour (`crate::vision`).
/// `COD4RW_TONEMAP=agx|aces` sets it for debug runs.
fn rich_tonemap(settings: &Settings) -> bool {
    std::env::var("COD4RW_TONEMAP").ok().as_deref().unwrap_or(settings.text("r_tonemap")) == "aces"
}

static RICH: AtomicBool = AtomicBool::new(false);

/// The Rich look is on.
pub fn rich() -> bool {
    RICH.load(Ordering::Relaxed)
}

/// Silence when the window isn't in front (if asked), else the master
/// volume's.
fn mute_unfocused(settings: Res<Settings>, window: Single<&Window, With<PrimaryWindow>>, mut was: Local<Option<bool>>) {
    let quiet = settings.on("snd_mute_unfocused") && !window.focused;
    if *was != Some(quiet) {
        *was = Some(quiet);
        MASTER_VOLUME.set(if quiet { 0.0 } else { settings.num("snd_volume") / 100.0 });
    }
    if settings.is_changed() && !quiet {
        MASTER_VOLUME.set(settings.num("snd_volume") / 100.0);
    }
}
