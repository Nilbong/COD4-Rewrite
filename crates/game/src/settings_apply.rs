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
static FILM_FX: AtomicBool = AtomicBool::new(true);
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
static HUD: [AtomicBool; 9] = [const { AtomicBool::new(true) }; 9];

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
    XpBar,
}

const HUD_DVARS: [(Hud, &str); 9] = [
    (Hud::Crosshair, "cg_crosshair"),
    (Hud::HitMarkers, "cg_hitmarkers"),
    (Hud::KillFeed, "cg_killfeed"),
    (Hud::MinimapRotates, "cg_minimap_rotate"),
    (Hud::DamageDirection, "cg_damage_direction"),
    (Hud::ScorePopups, "cg_xp_popups"),
    (Hud::HitMarkerSound, "cg_hitmarker_sound"),
    (Hud::MapEffects, "fx_mapfx"),
    (Hud::XpBar, "cg_xpbar"),
];

/// A HUD switch (and the map effects').
pub fn hud(which: Hud) -> bool {
    HUD[which as usize].load(Ordering::Relaxed)
}

/// A campaign level script's `cg_fov` (degrees, as f32 bits; 0 for none):
/// over the player's own while that level runs (the AC-130's zoom).
pub static SCRIPT_FOV: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static ADS_AFFECTED: AtomicBool = AtomicBool::new(true);
static AUTO_LEAN: AtomicBool = AtomicBool::new(true);
/// Weapon Position: 0 far (CoD4's), 1 standard, 2 close.
static GUN_POSITION: AtomicU32 = AtomicU32::new(0);
/// Auto lean's sensitivity: 0 low, 1 high.
static AUTO_LEAN_HIGH: AtomicBool = AtomicBool::new(false);

/// CoD4's own hip field of view, which the weapons' ADS zooms are made for.
const COD4_FOV: f32 = 65.0;

/// A weapon's aimed field of view (degrees) for its `adsZoomFov`: with ADS
/// Field of View "Affected" (MW2019's), zoomed by the same amount from the
/// player's own field of view as from CoD4's 65, so a wide view stays wide;
/// "Independent", the weapon's own.
pub fn ads_fov(weapon_ads_fov: f32) -> f32 {
    if !ADS_AFFECTED.load(Ordering::Relaxed) || weapon_ads_fov <= 0.0 {
        return weapon_ads_fov;
    }
    let half = |d: f32| (d.to_radians() * 0.5).tan();
    let zoom = half(weapon_ads_fov) / half(COD4_FOV);
    (2.0 * (half(hip_fov()) * zoom).atan()).to_degrees()
}

/// Test aid: ADS Field of View affected or not.
pub fn set_ads_affected(on: bool) {
    ADS_AFFECTED.store(on, Ordering::Relaxed);
}

/// The Weapon Position setting's shift of the gun at the hip (CoD inches:
/// forward, left, up): back towards the eye and in towards the middle,
/// live tuned `gunpos.<standard|close>_<back|in|up>`.
pub fn gun_position(now: f32) -> bevy::math::Vec3 {
    // (Test aid: `COD4RW_GUNPOS=far|standard|close`, or `cycle`: each in
    // turn, 1.5 s apiece from 5 s, as `COD4RW_SHOT_VIEWS` takes its shots.)
    let which = match std::env::var("COD4RW_GUNPOS").as_deref() {
        Ok("cycle") => (((now - 5.0) / 1.5).floor().max(0.0) as u32) % 3,
        Ok("close") => 2,
        Ok("standard") => 1,
        Ok("far") => 0,
        _ => GUN_POSITION.load(Ordering::Relaxed),
    };
    let (name, back, inward, up) = match which {
        2 => ("close", 4.0, 1.4, 0.8),
        1 => ("standard", 2.0, 0.7, 0.4),
        _ => return bevy::math::Vec3::ZERO,
    };
    let k = |what: &str, d: f32| crate::tune::get(&format!("gunpos.{name}_{what}"), d);
    bevy::math::Vec3::new(-k("back", back), k("in", inward), k("up", up))
}

/// Auto lean set to High (Low: only hugging the edge, after a moment).
pub fn auto_lean_high() -> bool {
    AUTO_LEAN_HIGH.load(Ordering::Relaxed)
}

/// Whether aiming beside a wall's edge leans out past it.
pub fn auto_lean() -> bool {
    match std::env::var("COD4RW_AUTOLEAN").as_deref() {
        Ok("0") => false,
        Ok(_) => true,
        Err(_) => AUTO_LEAN.load(Ordering::Relaxed),
    }
}

/// The hip field of view (degrees, vertical as CoD4's `cg_fov`).
pub fn hip_fov() -> f32 {
    let script = f32::from_bits(SCRIPT_FOV.load(Ordering::Relaxed));
    if script > 0.0 && script.is_finite() { script.clamp(10.0, 120.0) } else { HIP_FOV.get() }
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

/// Film grain, vignette and lens dirt (`r_filmfx`, `crate::vision`).
pub fn film_fx() -> bool {
    FILM_FX.load(Ordering::Relaxed)
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
    ADS_AFFECTED.store(s.text("cg_adsfov") != "independent", Ordering::Relaxed);
    AUTO_LEAN.store(s.text("cg_autolean") != "0", Ordering::Relaxed);
    GUN_POSITION.store(match s.text("cg_gunposition") { "close" => 2, "standard" => 1, _ => 0 }, Ordering::Relaxed);
    AUTO_LEAN_HIGH.store(s.text("cg_autolean") == "1", Ordering::Relaxed);
    MOUSE.set((s.num("sensitivity") / 5.0).clamp(0.02, 10.0));
    ADS_MOUSE.set(s.num("cg_ads_sens").clamp(0.1, 4.0));
    INVERT.store(s.on("m_invert"), Ordering::Relaxed);
    BLOOM.store(s.on("r_bloom"), Ordering::Relaxed);
    FILM_FX.store(s.on("r_filmfx"), Ordering::Relaxed);
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
    crate::lightmaps::set_live(s.text("r_lightmaps") == "live");
    if let Ok(mut k) = crate::hq::CALL_KEY.lock() {
        *k = s.bindings().key_name(crate::bindings::Action::Frag);
    }
    if let Ok(mut k) = crate::splitscreen::USE_KEY.lock() {
        *k = s.bindings().key_name(crate::bindings::Action::Use);
    }
    crate::clutter::set_enabled(s.on("phys_clutter"));
    crate::gamepad::playstation::set_enabled(s.on("pad_ps_features"));
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
        // (Not a test window placed with `COD4RW_WINPOS`.)
        let placed_by_test = std::env::var_os("COD4RW_WINPOS").is_some();
        if mode == WindowMode::Windowed && !(first && chosen == "current") && monitor != MonitorSelection::Current && !placed_by_test {
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
    present_overrides(&mut window);
}

/// Test overrides for presentation (normal play and sims alike):
/// `COD4RW_FRAMELATENCY=n` frames the GPU may queue (wgpu's default 2),
/// `COD4RW_PRESENT=immediate|mailbox|fifo` the presentation mode.
pub fn present_overrides(window: &mut Window) {
    let latency = std::env::var("COD4RW_FRAMELATENCY").ok().and_then(|v| v.parse::<u32>().ok()).and_then(std::num::NonZero::new);
    if latency.is_some() && window.desired_maximum_frame_latency != latency {
        window.desired_maximum_frame_latency = latency;
    }
    let forced = match std::env::var("COD4RW_PRESENT").as_deref() {
        Ok("immediate") => Some(PresentMode::Immediate),
        Ok("mailbox") => Some(PresentMode::Mailbox),
        Ok("fifo") => Some(PresentMode::Fifo),
        _ => None,
    };
    if let Some(mode) = forced.filter(|m| window.present_mode != *m) {
        window.present_mode = mode;
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
        // (Debug A/B: `COD4RW_TONEMAPPER=tony|aces|agx`.)
        let want = match std::env::var("COD4RW_TONEMAPPER").as_deref() {
            Ok("tony") => Tonemapping::TonyMcMapface,
            Ok("aces") => Tonemapping::AcesFitted,
            _ => Tonemapping::AgX,
        };
        if tonemapping != Some(&want) {
            c.insert(want);
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
    // (The viewmodels' suns follow the map's: `crate::model_lighting`. Their
    // fill lights, lightning and the menus' gun preview lights never cast
    // shadows: given the sun's, the six fills alone added 12 shadow views
    // for the gun camera, 2048 square each (Shipment, 2026-10-14).)
    mut suns: Query<(Entity, &mut DirectionalLight, Option<&Name>), (Without<crate::model_lighting::ViewModelSun>, Without<crate::model_lighting::VmFill>)>,
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
        // difference (perf, 2026-10-08); 3 cascades over 120 m looked the same
        // as 4 (A/B on four maps) and saved ~1 ms, and 2 the same as 3
        // (2026-10-14, Creek and Carentan; GPU -0.7 ms on Creek, -0.3 on
        // Bloc). Killhouse's reports (2026-10-20: far shadows in blocks,
        // one 2048 map over 12-120 m): a third cascade fixed them but cost
        // 1.2-1.3 ms (its shadow view, mostly the render thread), so two
        // over a shorter reach instead, the far one 10-90 m (a third denser,
        // free). Live knobs `shadows.ultra_cascades`, `shadows.ultra_distance`
        // and `shadows.first`.
        "ultra" => (
            true,
            crate::tune::get("shadows.ultra_cascades", 2.0).round().clamp(1.0, 4.0) as usize,
            crate::tune::get("shadows.ultra_distance", 90.0).clamp(20.0, 300.0),
            2048,
        ),
        _ => (true, 2, 80.0, 2048),
    };
    if settings.is_changed() && map_size.size != size {
        map_size.size = size;
    }
    // The cascades anew only when they change (or for a new sun).
    let new_cascades = *applied != Some((cascades, distance));
    let mut any = false;
    for (e, mut light, name) in &mut suns {
        if name.is_some_and(|n| matches!(n.as_str(), "lightning" | "gun preview light")) {
            continue;
        }
        let added = light.is_added();
        if !(settings.is_changed() || added) {
            continue;
        }
        any = true;
        if light.shadow_maps_enabled != on {
            light.shadow_maps_enabled = on;
        }
        if new_cascades || added {
            let config = CascadeShadowConfigBuilder { num_cascades: cascades, maximum_distance: distance, first_cascade_far_bound: crate::tune::get("shadows.first", if settings.text("r_shadows") == "ultra" { 10.0 } else { 12.0 }).clamp(2.0, distance), ..default() };
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
fn brightness(settings: Res<Settings>, mut grading: Query<&mut ColorGrading>, mut graded: Local<bool>) {
    // (Plus Headquarters' outdoor darkening, `crate::hq`.)
    // (And the showcase's per-map offset, `crate::atmos::climate`.)
    let exposure = settings.num("r_gamma").clamp(0.3, 3.0).log2() + crate::hq::exposure_offset() + crate::atmos::climate::exposure_offset();
    // (A map look's white balance, `warmth.<map>`: Shipment's golden hour.)
    let warmth = crate::atmos::climate::look_knob("warmth", |l| l.warmth, 0.0);
    let tint = crate::atmos::climate::look_knob("tint", |l| l.tint, 0.0);
    // The Map Look Editor's contrast and saturation (`contrast.<map>`,
    // `saturation.<map>`): only while set (Bodycam has its own), back to
    // neutral once unset.
    let contrast = crate::atmos::climate::look_knob("contrast", |_| -1.0, -1.0);
    let saturation = crate::atmos::climate::look_knob("saturation", |_| -1.0, -1.0);
    let set = contrast >= 0.0 || saturation >= 0.0;
    for mut g in &mut grading {
        if (g.global.exposure - exposure).abs() > 1e-4 {
            g.global.exposure = exposure;
        }
        if (g.global.temperature - warmth).abs() > 1e-4 {
            g.global.temperature = warmth;
        }
        if (g.global.tint - tint).abs() > 1e-4 {
            g.global.tint = tint;
        }
        if set || *graded {
            let c = if contrast >= 0.0 { contrast.clamp(0.2, 3.0) } else { 1.0 };
            let s = if saturation >= 0.0 { saturation.clamp(0.0, 3.0) } else { 1.0 };
            if g.midtones.contrast != c {
                g.midtones.contrast = c;
            }
            if g.global.post_saturation != s {
                g.global.post_saturation = s;
            }
        }
    }
    *graded = set;
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
