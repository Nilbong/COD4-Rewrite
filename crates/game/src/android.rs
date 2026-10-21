//! Android (the `android/` build): the entry point, where the player's game
//! files are, and the device's built-in pad.
//!
//! Files: the app's external files folder
//! (`/storage/emulated/0/Android/data/<package>/files/`) holds the games,
//! `cod4/` (with `main/` and `zone/`) and optionally `bo1/`, `mw2/`, `waw/`,
//! plus `env.txt`, `KEY=value` lines set as environment variables before the
//! game starts (the `COD4RW_*` debug aids). Everything the game saves under
//! `%LOCALAPPDATA%` on Windows goes to the app's internal files folder.
//!
//! Pad: stock winit turns a pad's stick motion into touches and its buttons
//! into unidentified keys. The patched winit (android/patches/winit) hands
//! every input event to [`hook`] first; pad events become a Bevy gamepad
//! here, fed as `gamepad/sony.rs` feeds its HID pads, so the rest of the game
//! treats it like any pad.

use bevy::input::gamepad::{
    GamepadConnection, GamepadConnectionEvent, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
};
use bevy::prelude::*;

mod etc2;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use winit::platform::android::activity::{AndroidApp, WindowManagerFlags};
use winit::platform::android::activity::input::{Axis, InputEvent, KeyAction, Keycode};

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    let missing = setup_env(&app);
    let _ = winit::platform::android::GAMEPAD_HOOK.set(hook);
    start_watchdog(app.clone());
    // A game: the screen stays on while it's in front (pad input doesn't
    // count as touching the screen).
    app.set_window_flags(WindowManagerFlags::KEEP_SCREEN_ON, WindowManagerFlags::empty());
    let _ = bevy::android::ANDROID_APP.set(app);
    match missing {
        Some(dir) => missing_files(&dir),
        None => {
            let _ = crate::main();
        }
    }
}

/// The app's external files folder (`.../Android/data/<package>/files`).
static FILES_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

pub fn files_dir() -> Option<&'static Path> {
    FILES_DIR.get().map(|p| p.as_path())
}

/// Without CoD4's files there's no game: a screen saying where they go.
fn missing_files(dir: &Path) {
    let text = format!(
        "Call of Duty 4's files are missing.\n\nCopy the main and zone folders of your Call of Duty 4 install into\n\n{}\n\n\
         (over USB: Internal storage > Android > data > com.cod4rewrite.game > files > cod4),\nthen start the game again.\n\n\
         A copy in Download/Call of Duty 4 works too, once the app has All files access\n\
         (Settings > Apps > CoD4 Rewrite > Permissions).",
        dir.display()
    );
    App::new()
        .add_plugins(DefaultPlugins.build().disable::<bevy::gilrs::GilrsPlugin>())
        .add_systems(Startup, move |mut commands: Commands| {
            commands.spawn(Camera2d);
            commands.spawn((
                Text::new(text.clone()),
                TextFont { font_size: bevy::text::FontSize::Px(30.0), ..default() },
                Node { margin: UiRect::all(Val::Px(60.0)), ..default() },
            ));
        })
        .run();
}

/// The folders and variables the game reads, before any thread starts; the
/// CoD4 folder, if its files are missing.
fn setup_env(app: &AndroidApp) -> Option<std::path::PathBuf> {
    let set = |k: &str, v: &std::ffi::OsStr| {
        // SAFETY: the only thread so far.
        unsafe { std::env::set_var(k, v) }
    };
    if let Some(internal) = app.internal_data_path() {
        let _ = std::fs::create_dir_all(&internal);
        set("LOCALAPPDATA", internal.as_os_str());
        // Files the game keeps beside itself (tuning.txt, profiles).
        let _ = std::env::set_current_dir(&internal);
    }
    let Some(external) = app.external_data_path() else {
        log::error!("android: no external files folder");
        return None;
    };
    let _ = FILES_DIR.set(external.clone());
    if let Ok(text) = std::fs::read_to_string(external.join("env.txt")) {
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            if let Some((k, v)) = line.split_once('=') {
                set(k.trim(), std::ffi::OsStr::new(v.trim()));
            }
        }
    }
    let cod4 = external.join("cod4");
    let _ = std::fs::create_dir_all(&cod4);
    // A copy of the PC install elsewhere on shared storage (readable with
    // "All files access", MANAGE_EXTERNAL_STORAGE).
    if std::env::var_os("COD4_PATH").is_none() && !has_cod4(&cod4) {
        let shared = Path::new("/storage/emulated/0");
        let found = ["Download/Call of Duty 4", "Call of Duty 4", "Games/Call of Duty 4", "Download/cod4", "cod4"]
            .into_iter()
            .map(|d| shared.join(d))
            .find(|d| has_cod4(d));
        if let Some(dir) = found {
            log::info!("android: Call of Duty 4 at {}", dir.display());
            set("COD4_PATH", dir.as_os_str());
        } else {
            log::warn!("android: no Call of Duty 4 in shared storage (or no All files access to read it)");
        }
    }
    for (var, dir) in [("COD4_PATH", "cod4"), ("BLACKOPS_PATH", "bo1"), ("MW2_PATH", "mw2"), ("WAW_PATH", "waw")] {
        let path = external.join(dir);
        if std::env::var_os(var).is_none() && (var == "COD4_PATH" || path.is_dir()) {
            set(var, path.as_os_str());
        }
    }
    let cod4 = std::env::var_os("COD4_PATH").map_or(cod4, std::path::PathBuf::from);
    if has_cod4(&cod4) {
        return None;
    }
    log::error!("android: Call of Duty 4's files are missing: copy its main and zone folders into {}", cod4.display());
    Some(cod4)
}

/// `GAME_ARGS` (env.txt): the command line's arguments, e.g.
/// `--map mp_crossfire --bots 3`.
pub fn args() -> Vec<String> {
    std::env::var("GAME_ARGS").map(|a| a.split_whitespace().map(String::from).collect()).unwrap_or_default()
}

fn has_cod4(dir: &Path) -> bool {
    dir.join("main").is_dir() && dir.join("zone").is_dir()
}

/// The Android pieces of the app: the pad, and no gilrs (it has no Android
/// backend, and would be asked to rumble a pad it doesn't own).
pub fn plugins(plugins: bevy::app::PluginGroupBuilder) -> bevy::app::PluginGroupBuilder {
    // Android can't grab or move a cursor: Bevy says so every frame the game
    // asks, as an error.
    let filter = format!("{},bevy_winit::system=off,bevy_winit::winit_windows=off", bevy::log::DEFAULT_FILTER);
    plugins
        .disable::<bevy::gilrs::GilrsPlugin>()
        .set(bevy::log::LogPlugin { custom_layer: crate::log_file_layer, filter, ..default() })
        .add(AndroidPlugin)
}

struct AndroidPlugin;

impl Plugin for AndroidPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pads>().add_systems(First, (feed, beat)).add_systems(Last, vitals);
        if std::env::var_os("VM_DEBUG").is_some() {
            app.add_systems(Update, vm_debug.run_if(crate::state::in_game));
        }
    }
}

// ---- vitals ----

/// Every 10 s: frames a second (average and the slowest frame), memory and
/// the battery's temperature, in the log (`android: fps ...`), menus included.
fn vitals(time: Res<Time<Real>>, mut frames: Local<Vec<f32>>, mut since: Local<f32>) {
    let dt = time.delta_secs();
    frames.push(dt * 1000.0);
    *since += dt;
    if *since < 10.0 {
        return;
    }
    let n = frames.len() as f32;
    let worst = frames.iter().copied().fold(0.0, f32::max);
    let mut sorted = frames.clone();
    sorted.sort_by(f32::total_cmp);
    let p99 = sorted[((n * 0.99) as usize).min(sorted.len() - 1)];
    // Resident memory (pages) and the battery's temperature (tenths of a degree).
    let rss_mb = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<f32>().ok())
        .map_or(0.0, |pages| pages * 4096.0 / 1e6);
    let temp = std::fs::read_to_string("/sys/class/power_supply/battery/temp")
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map_or(0.0, |t| t / 10.0);
    info!("android: fps {:.1} (1% slowest {p99:.1} ms, worst {worst:.1} ms), {rss_mb:.0} MB, battery {temp:.1} C", n / *since);
    frames.clear();
    *since = 0.0;
}

// ---- viewmodel debugging ----

/// `VM_DEBUG=1` (env.txt): every 10 s, the viewmodel camera and the meshes on
/// its layer, in the log; `VM_DEBUG=world` also puts those meshes on the world
/// camera's layer (drawn there: the camera's at fault, not the materials).
#[allow(clippy::type_complexity)]
fn vm_debug(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut next: Local<f32>,
    cams: Query<(&Camera, Option<&bevy::camera::visibility::RenderLayers>, Has<bevy::camera::Hdr>, Option<&Msaa>, &GlobalTransform, &Projection), With<crate::player::ViewModelCamera>>,
    world_cam: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    roots: Query<(&Transform, &GlobalTransform, &crate::models::Skeleton, &Visibility), With<crate::viewmodel::ViewModelRoot>>,
    joints: Query<&GlobalTransform>,
    meshes: Query<(
        Entity,
        Option<&Name>,
        &bevy::camera::visibility::RenderLayers,
        &ViewVisibility,
        &InheritedVisibility,
        Option<&bevy::camera::primitives::Aabb>,
        &GlobalTransform,
        Has<MeshMaterial3d<StandardMaterial>>,
        Has<MeshMaterial3d<crate::gunmodel::CamoMaterial>>,
    ), With<Mesh3d>>,
) {
    let world = std::env::var("VM_DEBUG").is_ok_and(|v| v == "world");
    let layer = crate::player::VIEWMODEL_LAYER;
    if world {
        for (e, _, layers, ..) in &meshes {
            if layers.intersects(&bevy::camera::visibility::RenderLayers::layer(layer)) && !layers.intersects(&bevy::camera::visibility::RenderLayers::layer(0)) {
                commands.entity(e).insert(layers.clone().with(0));
            }
        }
    }
    if time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 10.0;
    for (cam, layers, hdr, msaa, at, proj) in &cams {
        let near = match proj {
            Projection::Perspective(p) => format!("fov {:.1} near {} far {}", p.fov.to_degrees(), p.near, p.far),
            _ => "other".into(),
        };
        info!(
            "vm: camera active {} order {} layers {:?} hdr {hdr} msaa {msaa:?} clear {:?} output {:?} viewport {:?} at {} {near}",
            cam.is_active, cam.order, layers, cam.clear_color, cam.output_mode, cam.viewport, at.translation()
        );
    }
    if let Ok(w) = world_cam.single() {
        info!("vm: world camera at {}", w.translation());
        // The gun's root and joints in the camera's frame (x right, y up,
        // -z ahead): a gun in view sits around (0.1, -0.1, -0.3).
        let to_cam = w.affine().inverse();
        for (local, at, skeleton, vis) in &roots {
            info!(
                "vm: root local {:?} rot {:?} scale {:?} vis {vis:?}, in camera {} ({} joints)",
                local.translation,
                local.rotation,
                local.scale,
                to_cam.transform_point3(at.translation()),
                skeleton.joints.len()
            );
            for j in skeleton.joints.iter().filter(|j| ["tag_torso", "j_shoulder_le", "j_wrist_le", "j_wrist_ri", "tag_weapon", "j_gun", "tag_flash", "tag_clip"].contains(&j.name.as_str())) {
                if let Ok(g) = joints.get(j.entity) {
                    info!("vm:   joint {} in camera {} scale {}", j.name, to_cam.transform_point3(g.translation()), g.scale());
                }
            }
        }
    }
    let mut n = 0;
    for (e, name, layers, view, inherited, aabb, at, standard, camo) in &meshes {
        if !layers.intersects(&bevy::camera::visibility::RenderLayers::layer(layer)) {
            continue;
        }
        n += 1;
        if n <= 12 {
            info!(
                "vm: mesh {e} {:?} layers {layers:?} visible {} inherited {} aabb {:?} at {} scale {} standard {standard} camo {camo}",
                name.map(|n| n.as_str()),
                view.get(),
                inherited.get(),
                aabb.map(|a| (a.center, a.half_extents)),
                at.translation(),
                at.scale()
            );
        }
    }
    info!("vm: {n} meshes on the viewmodel layer");
}

// ---- input during long frames ----

/// When the main loop last ran a frame (ms since the watchdog started).
static LAST_FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn now_ms() -> u64 {
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

fn beat() {
    LAST_FRAME.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
}

/// NativeActivity hands input to the main loop's thread, and Android calls
/// the app "not responding" (and offers to close it) when an event waits 5 s:
/// loading a map, or a first frame's shaders, take longer. While the main
/// loop is stalled for over a second, this thread takes the events instead:
/// pad input goes to [`hook`] (and on to the game), the rest is dropped.
fn start_watchdog(app: AndroidApp) {
    now_ms();
    std::thread::Builder::new()
        .name("input watchdog".into())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let last = LAST_FRAME.load(std::sync::atomic::Ordering::Relaxed);
                if last == 0 || now_ms() - last < 1000 {
                    continue;
                }
                // Busy (the main loop is reading them) is fine: it's running.
                if let Ok(mut events) = app.input_events_iter() {
                    while events.next(|event| {
                        hook(event);
                        winit::platform::android::activity::InputStatus::Handled
                    }) {}
                }
            }
        })
        .expect("input watchdog thread");
}

// ---- pad ----

enum PadEvent {
    Button(i32, GamepadButton, f32),
    Axis(i32, GamepadAxis, f32),
}

/// What [`hook`] has seen since the last frame, and each pad's last values
/// (to send only changes).
#[derive(Default)]
struct Seen {
    events: Vec<PadEvent>,
    axes: HashMap<(i32, GamepadAxis), f32>,
    buttons: HashMap<(i32, GamepadButton), f32>,
    /// Pads whose triggers are analog axes (their L2/R2 keys are ignored).
    analog_triggers: Vec<i32>,
}

static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

/// Android's `SOURCE_GAMEPAD` bit.
const SOURCE_GAMEPAD: u32 = 0x400;

/// Takes pad input (on the event loop's thread).
fn hook(event: &InputEvent<'_>) -> bool {
    let mut guard = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = guard.get_or_insert_with(Seen::default);
    match event {
        InputEvent::KeyEvent(key) => {
            let source: u32 = key.source().into();
            let pad = source & SOURCE_GAMEPAD != 0 || source & 0x10 != 0;
            let Some(button) = pad_button(key.key_code(), pad) else {
                return false;
            };
            let id = key.device_id();
            if matches!(button, GamepadButton::LeftTrigger2 | GamepadButton::RightTrigger2) && seen.analog_triggers.contains(&id) {
                return true;
            }
            let value = match key.action() {
                KeyAction::Down => 1.0,
                _ => 0.0,
            };
            seen.button(id, button, value);
            true
        }
        InputEvent::MotionEvent(motion) => {
            if !motion.source().is_joystick_class() {
                return false;
            }
            let id = motion.device_id();
            let p = motion.pointer_at_index(0);
            let a = |axis| p.axis_value(axis);
            seen.axis(id, GamepadAxis::LeftStickX, a(Axis::X));
            seen.axis(id, GamepadAxis::LeftStickY, -a(Axis::Y));
            seen.axis(id, GamepadAxis::RightStickX, a(Axis::Z));
            seen.axis(id, GamepadAxis::RightStickY, -a(Axis::Rz));
            let (lt, rt) = (a(Axis::Ltrigger).max(a(Axis::Brake)), a(Axis::Rtrigger).max(a(Axis::Gas)));
            if (lt > 0.0 || rt > 0.0) && !seen.analog_triggers.contains(&id) {
                seen.analog_triggers.push(id);
            }
            if seen.analog_triggers.contains(&id) {
                seen.button(id, GamepadButton::LeftTrigger2, lt);
                seen.button(id, GamepadButton::RightTrigger2, rt);
            }
            // The d-pad as a hat (Xbox-style pads).
            let (hx, hy) = (a(Axis::HatX), a(Axis::HatY));
            seen.button(id, GamepadButton::DPadLeft, f32::from(hx < -0.5));
            seen.button(id, GamepadButton::DPadRight, f32::from(hx > 0.5));
            seen.button(id, GamepadButton::DPadUp, f32::from(hy < -0.5));
            seen.button(id, GamepadButton::DPadDown, f32::from(hy > 0.5));
            true
        }
        _ => false,
    }
}

impl Seen {
    fn button(&mut self, id: i32, button: GamepadButton, value: f32) {
        let last = self.buttons.insert((id, button), value);
        if last != Some(value) && (last.is_some() || value != 0.0) {
            self.events.push(PadEvent::Button(id, button, value));
        }
    }

    fn axis(&mut self, id: i32, axis: GamepadAxis, value: f32) {
        let last = self.axes.insert((id, axis), value);
        if last != Some(value) && (last.is_some() || value != 0.0) {
            self.events.push(PadEvent::Axis(id, axis, value));
        }
    }
}

/// Android's pad keys as Bevy's buttons (the d-pad's only from a pad).
fn pad_button(key: Keycode, pad: bool) -> Option<GamepadButton> {
    Some(match key {
        Keycode::ButtonA => GamepadButton::South,
        Keycode::ButtonB => GamepadButton::East,
        Keycode::ButtonX => GamepadButton::West,
        Keycode::ButtonY => GamepadButton::North,
        Keycode::ButtonL1 => GamepadButton::LeftTrigger,
        Keycode::ButtonR1 => GamepadButton::RightTrigger,
        Keycode::ButtonL2 => GamepadButton::LeftTrigger2,
        Keycode::ButtonR2 => GamepadButton::RightTrigger2,
        Keycode::ButtonThumbl => GamepadButton::LeftThumb,
        Keycode::ButtonThumbr => GamepadButton::RightThumb,
        Keycode::ButtonStart => GamepadButton::Start,
        Keycode::ButtonSelect => GamepadButton::Select,
        Keycode::ButtonMode => GamepadButton::Mode,
        Keycode::DpadUp if pad => GamepadButton::DPadUp,
        Keycode::DpadDown if pad => GamepadButton::DPadDown,
        Keycode::DpadLeft if pad => GamepadButton::DPadLeft,
        Keycode::DpadRight if pad => GamepadButton::DPadRight,
        _ => return None,
    })
}

/// A pad met through [`hook`].
#[derive(Component)]
pub(crate) struct AndroidPad;

/// Android device id → the pad's entity.
#[derive(Resource, Default)]
struct Pads {
    entities: HashMap<i32, Entity>,
    /// Input for pads Bevy meets this frame, sent the next.
    held: Vec<PadEvent>,
}

fn feed(mut commands: Commands, mut pads: ResMut<Pads>, mut connect: MessageWriter<GamepadConnectionEvent>, mut raw: MessageWriter<RawGamepadEvent>) {
    let mut events = std::mem::take(&mut pads.held);
    if let Some(seen) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        events.append(&mut seen.events);
    }
    let mut new = Vec::new();
    for event in events {
        let id = match event {
            PadEvent::Button(id, ..) | PadEvent::Axis(id, ..) => id,
        };
        let entity = match pads.entities.get(&id) {
            Some(&e) if !new.contains(&id) => e,
            Some(_) => {
                pads.held.push(event);
                continue;
            }
            None => {
                let entity = commands.spawn(AndroidPad).id();
                info!("gamepad: Android pad (device {id})");
                connect.write(GamepadConnectionEvent::new(
                    entity,
                    GamepadConnection::Connected { name: format!("Android pad {id}"), vendor_id: None, product_id: None },
                ));
                pads.entities.insert(id, entity);
                new.push(id);
                pads.held.push(event);
                continue;
            }
        };
        raw.write(match event {
            PadEvent::Button(_, b, v) => RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(entity, b, v)),
            PadEvent::Axis(_, a, v) => RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(entity, a, v)),
        });
    }
}

// ---- textures ----

/// The largest texture side on Android (`TEXTURE_MAX_SIZE` in env.txt):
/// bigger compressed textures drop their top mip levels. Less memory (they're
/// decoded to RGBA8, below) and a faster load.
fn texture_max() -> u32 {
    static MAX: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| std::env::var("TEXTURE_MAX_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(1024))
}

/// `iwi` without the mip levels above [`texture_max`], if it has any.
pub(crate) fn shrink(iwi: &iw3::iwi::Iwi) -> Option<iw3::iwi::Iwi> {
    let max = texture_max();
    let mut skip = 0;
    while (iwi.width >> skip).max(iwi.height >> skip) > max && skip + 1 < iwi.levels.len() {
        skip += 1;
    }
    // Block-compressed levels keep sides of at least 4.
    if skip == 0 || (iwi.width >> skip) % 4 != 0 || (iwi.height >> skip) % 4 != 0 {
        return None;
    }
    Some(iw3::iwi::Iwi {
        format: iwi.format,
        flags: iwi.flags,
        width: iwi.width >> skip,
        height: iwi.height >> skip,
        depth: iwi.depth,
        levels: iwi.levels[skip..].to_vec(),
    })
}

/// Opaque textures as ETC2 ([`etc2`]), a quarter of RGBA8's memory:
/// Killhouse on the Odin 2 went from 30 to 37 fps (the world pass 24.9 to
/// 17.4 ms), for 3 s more load. `TEXTURE_FORMAT=rgba8` in env.txt: off.
fn etc2_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("TEXTURE_FORMAT").is_ok_and(|v| v.eq_ignore_ascii_case("rgba8")))
}

/// A DXT iwi's levels (cube maps: each level's six faces, mip-major) in a
/// format Android GPUs sample (they have no BC formats): RGBA8, or ETC2 for
/// opaque textures when [`etc2_on`].
pub(crate) fn transcode(iwi: &iw3::iwi::Iwi, srgb: bool) -> Option<(bevy::render::render_resource::TextureFormat, Vec<u8>)> {
    use bevy::render::render_resource::TextureFormat;
    use rayon::prelude::*;
    let faces = if iwi.is_cube() { 6 } else { 1 };
    // Each level and face on its own core (map loads wait on these).
    let parts: Vec<(usize, usize, Vec<[u8; 4]>)> =
        (0..iwi.levels.len() * faces).into_par_iter().map(|i| crate::bake::dxt::decode(iwi, i / faces, i % faces)).collect::<Option<_>>()?;
    if etc2_on() && parts.iter().all(|(_, _, px)| px.iter().all(|p| p[3] == 255)) {
        let blocks: Vec<Vec<u8>> = parts.par_iter().map(|(w, h, px)| etc2::encode(px.as_flattened(), *w, *h)).collect();
        let format = if srgb { TextureFormat::Etc2Rgb8UnormSrgb } else { TextureFormat::Etc2Rgb8Unorm };
        return Some((format, blocks.concat()));
    }
    let format = if srgb { TextureFormat::Rgba8UnormSrgb } else { TextureFormat::Rgba8Unorm };
    Some((format, parts.into_iter().flat_map(|(_, _, px)| px).flatten().collect()))
}

// ---- settings ----

/// A setting's default on Android: `SETTING_<dvar>` from env.txt (to test
/// one, e.g. `SETTING_r_renderscale=100`), else [`MOBILE_DEFAULTS`]'s.
/// (Settings the player changed are saved and win over both.)
pub(crate) fn mobile_default(dvar: &str) -> Option<String> {
    std::env::var(format!("SETTING_{dvar}"))
        .ok()
        .or_else(|| MOBILE_DEFAULTS.iter().find(|(d, _)| *d == dvar).map(|(_, v)| (*v).to_owned()))
}

/// The settings' defaults on Android (a phone-class GPU at 1080p): about the
/// Low preset, with medium sun shadows, the world at 65% scale, and the
/// costly extras (3D scopes, full body, volumetrics, dynamic sky, rebaked or
/// ray-traced lighting) off. The FPS overlay is on while this is a test build.
pub(crate) const MOBILE_DEFAULTS: &[(&str, &str)] = &[
    ("r_preset", "custom"),
    ("r_renderscale", "65"),
    ("r_shadows", "medium"),
    ("r_propshadows", "0"),
    ("r_ssao", "0"),
    ("r_aa", "fxaa"),
    ("r_bloom", "0"),
    ("r_volumetric", "off"),
    ("r_sky", "classic"),
    ("r_lighting", "baked"),
    ("r_lightmaps", "original"),
    ("r_texfilter", "trilinear"),
    ("r_drawdistance", "medium"),
    ("fx_density", "low"),
    ("r_corpses", "4"),
    ("cg_scopestyle", "classic"),
    ("cg_fullbody", "0"),
    ("cg_drawfps", "2"),
];
