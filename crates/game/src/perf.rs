//! Debug aid: with `COD4RW_PERF` set, log frame times in a match every five
//! seconds (average, median, 95th percentile and worst, in ms, real time)
//! with the entity count and the GPU's time in each render pass, and the
//! whole run's on exit. The first ten seconds of a match (loading,
//! settling) are left out.
//!
//! `COD4RW_PERF` can also name things to leave out, comma-separated, to
//! measure what they cost: `novsync`, `noshadows` (the sun's), `nossao`,
//! `noviewmodel` (its camera, which also finishes the frame), `vmpost`
//! (no viewmodel camera, its post-processing on the world one),
//! `offscreen` (drawn into an image: no presenting, no outside cap), `nofx`
//! (effects aren't drawn, though still run), `cascades2` (the sun's shadows
//! in two cascades), `nopropshadows` (static models cast none), `noprops`
//! (static models hidden), `nohud` (the HUD and in-game menus aren't
//! painted) and `ads` (the player holds aim, to measure scopes:
//! `COD4RW_SCOPE`, `COD4RW_LOADOUT`). The log also counts what's redone
//! each frame: UI nodes changed, and images, materials and meshes added or
//! modified (each a GPU upload).
//!
//! `COD4RW_SHADOWTEST=<dir>` screenshots the same three views under each of
//! [`SHADOW_TRIALS`]' sun shadow settings (`<trial>_<view>.png`), then exits.

use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static NO_HUD: AtomicBool = AtomicBool::new(false);

/// `COD4RW_PERF` is set: systems may time themselves.
pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("COD4RW_PERF").is_some())
}

/// A system timing itself under `COD4RW_PERF`: hold one for the system's
/// run (`let _t = perf::Probe::start("name");`); every 300 runs the average
/// is logged.
pub struct Probe(Option<(&'static str, std::time::Instant)>);

impl Probe {
    pub fn start(name: &'static str) -> Probe {
        Probe(on().then(|| (name, std::time::Instant::now())))
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let Some((name, t0)) = self.0 else { return };
        static TIMES: std::sync::Mutex<Option<std::collections::HashMap<&'static str, (f64, u32)>>> = std::sync::Mutex::new(None);
        let Ok(mut times) = TIMES.lock() else { return };
        let e = times.get_or_insert_with(Default::default).entry(name).or_default();
        e.0 += t0.elapsed().as_secs_f64();
        e.1 += 1;
        if e.1 >= 300 {
            info!("perf: {name} {:.3} ms a run", e.0 * 1000.0 / e.1 as f64);
            *e = (0.0, 0);
        }
    }
}

/// `COD4RW_PERF=nohud`: the HUD and in-game menus go unpainted.
pub fn no_hud() -> bool {
    NO_HUD.load(Ordering::Relaxed)
}

pub struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        if let Ok(dir) = std::env::var("COD4RW_SHADOWTEST") {
            app.insert_resource(ShadowTest(dir.into())).add_systems(Update, shadow_test.run_if(crate::state::in_game));
        }
        if let Ok(options) = std::env::var("COD4RW_PERF") {
            let off = |name: &str| options.split(',').any(|o| o.trim() == name);
            let without = Without {
                vsync: off("novsync"),
                shadows: off("noshadows"),
                ssao: off("nossao"),
                viewmodel: off("noviewmodel") || off("vmpost"),
                vm_post: off("vmpost"),
                offscreen: off("offscreen"),
                fx: off("nofx"),
                cascades2: off("cascades2"),
                prop_shadows: off("nopropshadows"),
                props: off("noprops"),
                ads: off("ads"),
            };
            NO_HUD.store(off("nohud"), Ordering::Relaxed);
            // Bevy updates an unfocused window (a test run's) at 60 Hz,
            // which capped every timing run: always run flat out.
            app.insert_resource(bevy::winit::WinitSettings::continuous());
            info!("perf: leaving out {without:?}");
            // CPU time per frame on the main and render threads (frame
            // times alone hide it under vsync).
            // Exclusive systems, so each mark runs on the schedule's own
            // thread (its CPU cycles are measured too).
            app.init_schedule(FrameStart).add_systems(FrameStart, |_: &mut World| busy_mark(&MAIN_BUSY, true));
            app.init_schedule(FrameEnd).add_systems(FrameEnd, |_: &mut World| busy_mark(&MAIN_BUSY, false));
            let mut order = app.world_mut().resource_mut::<bevy::app::MainScheduleOrder>();
            order.insert_before(First, FrameStart);
            order.insert_after(Last, FrameEnd);
            if let Some(r) = app.get_sub_app_mut(bevy::render::RenderApp) {
                use bevy::render::{Render, RenderSystems};
                r.add_systems(Render, (|_: &mut World| busy_mark(&RENDER_BUSY, true)).in_set(RenderSystems::ExtractCommands));
                r.add_systems(Render, (|_: &mut World| busy_mark(&RENDER_BUSY, false)).in_set(RenderSystems::PostCleanup));
                r.add_systems(Render, draw_census.in_set(RenderSystems::Render));
            }
            app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin)
                .init_resource::<FrameTimes>()
                .insert_resource(without)
                .init_resource::<Redone>()
                .add_systems(Last, (leave_out, count_redone, log_frames, census).chain())
                .add_systems(
                    Update,
                    hold_aim.run_if(|w: Res<Without>| w.ads).after(crate::player::InputSet).before(crate::weapons::WeaponSet),
                );
        }
    }
}

/// What to leave out, to measure it.
#[derive(Resource, Debug, Clone, Copy)]
struct Without {
    vsync: bool,
    shadows: bool,
    ssao: bool,
    viewmodel: bool,
    /// `vmpost`: no viewmodel camera, its post-processing moved onto the
    /// world camera (what drawing the gun in the world camera would cost).
    vm_post: bool,
    /// `offscreen`: the cameras (and HUD) draw into an image the window's
    /// size, so nothing is presented and no outside frame cap applies: the
    /// frame times are the game's own.
    offscreen: bool,
    fx: bool,
    cascades2: bool,
    prop_shadows: bool,
    props: bool,
    ads: bool,
}

#[allow(clippy::type_complexity)]
fn leave_out(
    mut commands: Commands,
    without: Res<Without>,
    mut window: Single<&mut Window, With<bevy::window::PrimaryWindow>>,
    mut suns: Query<&mut DirectionalLight>,
    ssao: Query<Entity, With<bevy::pbr::ScreenSpaceAmbientOcclusion>>,
    mut viewmodel: Query<&mut Camera, With<crate::player::ViewModelCamera>>,
    mut main_camera: Query<&mut Camera, (With<crate::player::MainCamera>, bevy::ecs::query::Without<crate::player::ViewModelCamera>)>,
    mut fx: Query<&mut Visibility, With<crate::fx::FxBatch>>,
    (main_entity, vm_post, vm_entity, mut images, mut offscreen_done): (
        Query<Entity, With<crate::player::MainCamera>>,
        Query<&bevy::post_process::auto_exposure::AutoExposure, With<crate::player::ViewModelCamera>>,
        Query<Entity, With<crate::player::ViewModelCamera>>,
        ResMut<Assets<bevy::image::Image>>,
        Local<bool>,
    ),
    mut map_size: ResMut<bevy::light::DirectionalLightShadowMap>,
    (cascades, names, children, mut done): (
        Query<(Entity, &bevy::light::CascadeShadowConfig), With<DirectionalLight>>,
        Query<(Entity, &Name)>,
        Query<&Children>,
        Local<bool>,
    ),
) {
    // `COD4RW_SHADOWTRY=<cascades>,<metres>,<map size>`: the sun's shadows
    // so, whatever the settings say (to price each part).
    if let Some((n, far, size)) = shadow_try() {
        if map_size.size != size {
            map_size.size = size;
        }
        for (e, c) in &cascades {
            if c.bounds.len() != n || c.bounds.last().is_some_and(|b| (b - far).abs() > 0.5) {
                let config = bevy::light::CascadeShadowConfigBuilder {
                    num_cascades: n,
                    maximum_distance: far,
                    first_cascade_far_bound: 12f32.min(far),
                    ..default()
                };
                commands.entity(e).insert(config.build());
            }
        }
    }
    if without.cascades2 {
        for (e, c) in &cascades {
            if c.bounds.len() != 2 {
                let config = bevy::light::CascadeShadowConfigBuilder {
                    num_cascades: 2,
                    maximum_distance: 120.0,
                    first_cascade_far_bound: 12.0,
                    ..default()
                };
                commands.entity(e).insert(config.build());
            }
        }
    }
    // Every mesh under "static models" (once they're there).
    if without.props && !*done {
        if let Some((root, _)) = names.iter().find(|(_, n)| n.as_str() == "static models") {
            *done = true;
            commands.entity(root).insert(Visibility::Hidden);
            info!("perf: static models hidden");
        }
    }
    if without.prop_shadows && !*done {
        if let Some((root, _)) = names.iter().find(|(_, n)| n.as_str() == "static models") {
            *done = true;
            let mut count = 0;
            for model in children.iter_descendants(root) {
                commands.entity(model).insert(bevy::light::NotShadowCaster);
                count += 1;
            }
            info!("perf: {count} static model entities cast no shadows");
        }
    }
    // `COD4RW_PRESENT=immediate|mailbox` asks for that presentation mode
    // outright (AutoNoVsync falls back to one of them, or to FIFO).
    let unsynced = match std::env::var("COD4RW_PRESENT").as_deref() {
        Ok("immediate") => bevy::window::PresentMode::Immediate,
        Ok("mailbox") => bevy::window::PresentMode::Mailbox,
        _ => bevy::window::PresentMode::AutoNoVsync,
    };
    if without.vsync && window.present_mode != unsynced {
        window.present_mode = unsynced;
    }
    if without.shadows {
        for mut sun in &mut suns {
            if sun.shadow_maps_enabled {
                sun.shadow_maps_enabled = false;
            }
        }
    }
    if without.ssao {
        for e in &ssao {
            commands.entity(e).remove::<bevy::pbr::ScreenSpaceAmbientOcclusion>();
        }
    }
    if without.viewmodel {
        for mut camera in &mut viewmodel {
            camera.is_active = false;
        }
        // The world camera writes the frame out itself then.
        for mut camera in &mut main_camera {
            if matches!(camera.output_mode, bevy::camera::CameraOutputMode::Skip) {
                camera.output_mode = bevy::camera::CameraOutputMode::default();
                if without.vm_post {
                    let e = main_entity.single().ok();
                    if let (Some(e), Some(curve)) = (e, vm_post.iter().next()) {
                        commands.entity(e).insert((
                            curve.clone(),
                            bevy::post_process::bloom::Bloom::NATURAL,
                            bevy::core_pipeline::tonemapping::Tonemapping::AgX,
                            bevy::anti_alias::smaa::Smaa { preset: bevy::anti_alias::smaa::SmaaPreset::High },
                        ));
                    }
                }
            }
        }
    }
    if without.offscreen && !*offscreen_done {
        if let (Ok(world), Ok(vm)) = (main_entity.single(), vm_entity.single()) {
            *offscreen_done = true;
            let (w, h) = (window.resolution.physical_width(), window.resolution.physical_height());
            let target = images.add(bevy::image::Image::new_target_texture(
                w,
                h,
                <bevy::render::render_resource::TextureFormat as bevy::image::BevyDefault>::bevy_default(),
                None,
            ));
            for e in [world, vm] {
                commands.entity(e).insert(bevy::camera::RenderTarget::from(target.clone()));
            }
            commands.entity(vm).insert(bevy::ui::IsDefaultUiCamera);
            info!("perf: drawing offscreen at {w}x{h}");
        }
    }
    if without.fx {
        for mut v in &mut fx {
            v.set_if_neq(Visibility::Hidden);
        }
    }
}

/// Sun shadow settings to compare: name, cascades, far distance and first
/// cascade's far bound (metres).
const SHADOW_TRIALS: [(&str, usize, f32, f32); 3] = [("now", 4, 120.0, 12.0), ("c2_120", 2, 120.0, 12.0), ("c2_60", 2, 60.0, 12.0)];

#[derive(Resource)]
struct ShadowTest(std::path::PathBuf);

/// From 6 s, for each trial: its settings, then three views a third of a
/// turn apart, looking a little down (aimed 0.3 s before each shot), the
/// player held where it spawned.
fn shadow_test(
    mut commands: Commands,
    time: Res<Time>,
    test: Res<ShadowTest>,
    suns: Query<Entity, With<DirectionalLight>>,
    mut player: Query<
        (&mut crate::movement::ViewAngles, &mut Transform, &mut crate::movement::Mover),
        With<crate::player::LocalPlayer>,
    >,
    mut step: Local<usize>,
    mut start: Local<Option<(f32, Vec3)>>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    const PER_TRIAL: f32 = 3.0;
    let t = time.elapsed_secs() - 6.0;
    let Ok((mut view, mut feet, mut mover)) = player.single_mut() else { return };
    // Held where it spawned, so every trial sees the same views.
    let (yaw0, at) = *start.get_or_insert((view.yaw, feet.translation));
    feet.translation = at;
    mover.velocity = Vec3::ZERO;
    if t < 0.0 {
        return;
    }
    let trial = (t / PER_TRIAL) as usize;
    if trial >= SHADOW_TRIALS.len() {
        exit.write(AppExit::Success);
        return;
    }
    let local = t - trial as f32 * PER_TRIAL;
    // Steps per trial: the settings at 0, shots at 1.0, 1.6 and 2.2 s.
    let shot_at = |k: usize| 1.0 + k as f32 * 0.6;
    if let Some(k) = (0..3).find(|&k| local >= shot_at(k) - 0.3 && local < shot_at(k) + 0.3) {
        view.yaw = yaw0 + (k as f32 * 120.0).to_radians();
        view.pitch = -5f32.to_radians();
    }
    let (want_trial, part) = (*step / 4, *step % 4);
    let due = if part == 0 { 0.0 } else { shot_at(part - 1) };
    if want_trial != trial || local < due {
        return;
    }
    *step += 1;
    let (name, cascades, far, first) = SHADOW_TRIALS[trial];
    if part == 0 {
        let config = bevy::light::CascadeShadowConfigBuilder {
            num_cascades: cascades,
            maximum_distance: far,
            first_cascade_far_bound: first,
            ..default()
        };
        for sun in &suns {
            commands.entity(sun).insert(config.build());
        }
    } else {
        std::fs::create_dir_all(&test.0).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.0.join(format!("{name}_{part}.png"))));
    }
}

/// `ads`: the local player keeps aiming.
fn hold_aim(mut player: Query<&mut crate::weapons::WeaponInput, With<crate::player::LocalPlayer>>) {
    for mut input in &mut player {
        input.ads = true;
    }
}

/// What's redone each frame, summed over the log's window.
#[derive(Resource, Default, Debug)]
struct Redone {
    frames: u32,
    nodes: u32,
    images: u32,
    materials: u32,
    meshes: u32,
}

#[allow(clippy::type_complexity)]
fn count_redone(
    mut redone: ResMut<Redone>,
    nodes: Query<(), Or<(Changed<Node>, Changed<ImageNode>, Changed<Text>, Changed<UiTransform>)>>,
    mut images: MessageReader<AssetEvent<Image>>,
    mut materials: MessageReader<AssetEvent<StandardMaterial>>,
    mut meshes: MessageReader<AssetEvent<Mesh>>,
) {
    redone.frames += 1;
    redone.nodes += nodes.iter().count() as u32;
    redone.images += images.read().filter(|e| matches!(e, AssetEvent::Added { .. } | AssetEvent::Modified { .. })).count() as u32;
    redone.materials += materials.read().filter(|e| matches!(e, AssetEvent::Added { .. } | AssetEvent::Modified { .. })).count() as u32;
    redone.meshes += meshes.read().filter(|e| matches!(e, AssetEvent::Added { .. } | AssetEvent::Modified { .. })).count() as u32;
}

/// Settling time after a match starts.
const WARMUP: f32 = 10.0;
const EVERY: f32 = 5.0;

#[derive(Resource, Default)]
struct FrameTimes {
    started: Option<f32>,
    window: Vec<f32>,
    all: Vec<f32>,
    next: f32,
}

/// Average, median, 95th percentile and worst of `ms`.
fn stats(ms: &[f32]) -> (f32, f32, f32, f32) {
    let mut sorted = ms.to_vec();
    sorted.sort_by(f32::total_cmp);
    let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
    (sorted.iter().sum::<f32>() / sorted.len() as f32, at(0.5), at(0.95), sorted[sorted.len() - 1])
}

fn log_frames(
    time: Res<Time<Real>>,
    state: Option<Res<State<crate::state::GameState>>>,
    mut frames: ResMut<FrameTimes>,
    entities: Query<()>,
    pawns: Query<(), With<crate::combat::Pawn>>,
    kinds: (
        Query<(), With<Mesh3d>>,
        Query<(), With<Node>>,
        Query<(), With<avian3d::prelude::Collider>>,
        Query<(), With<AudioPlayer>>,
        Query<(), With<bevy::light::NotShadowCaster>>,
    ),
    mut exit: MessageReader<AppExit>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    mut redone: ResMut<Redone>,
) {
    let now = time.elapsed_secs();
    let in_game = state.is_some_and(|s| *s.get() == crate::state::GameState::InGame);
    let started = match (in_game, frames.started) {
        (false, _) => return,
        (true, None) => *frames.started.insert(now),
        (true, Some(t)) => t,
    };
    if now - started >= WARMUP {
        let ms = time.delta_secs() * 1000.0;
        if ms > 100.0 {
            info!("perf: {ms:.0} ms frame at {:.1} s into the match", now - started);
        }
        frames.window.push(ms);
        frames.all.push(ms);
    }
    if now >= frames.next && !frames.window.is_empty() {
        frames.next = now + EVERY;
        let (avg, median, p95, worst) = stats(&frames.window);
        let (meshes, nodes, colliders, sounds, unshadowed) = kinds;
        info!(
            "perf: frame {avg:.2} ms avg ({:.0} fps), median {median:.2}, p95 {p95:.2}, worst {worst:.2}; {} entities ({} meshes, {} not casting shadows, {} UI nodes, {} colliders, {} sounds), {} pawns",
            1000.0 / avg,
            entities.iter().count(),
            meshes.iter().count(),
            unshadowed.iter().count(),
            nodes.iter().count(),
            colliders.iter().count(),
            sounds.iter().count(),
            pawns.iter().count()
        );
        frames.window.clear();
        let n = redone.frames.max(1) as f32;
        info!(
            "perf: per frame {:.0} UI nodes changed, {:.1} images, {:.1} materials, {:.1} meshes added or modified",
            redone.nodes as f32 / n,
            redone.images as f32 / n,
            redone.materials as f32 / n,
            redone.meshes as f32 / n
        );
        *redone = Redone::default();
        let frames_since = MAIN_BUSY.frames.load(Ordering::Relaxed).max(1);
        let (main_cycles, render_cycles) = (MAIN_BUSY.take_cycles(), RENDER_BUSY.take_cycles());
        let (main, render) = (MAIN_BUSY.take(), RENDER_BUSY.take());
        let cycles = process_cycles();
        let used = cycles.saturating_sub(LAST_CYCLES.swap(cycles, Ordering::Relaxed)) as f64 / frames_since as f64 / 1e6;
        info!("perf: cpu {main:.2} ms main update, {render:.2} ms render schedule (waits included), {used:.1} Mcycles all threads (per frame)");
        // Busy cycles of the two threads themselves (waits left out): what
        // bounds the frame rate once nothing caps it.
        info!("perf: thread Mcycles per frame: main {main_cycles:.1}, render {render_cycles:.1}");
        // The GPU's time per pass (top level and their parts), longest first.
        let mut gpu: Vec<(String, f64)> = diagnostics
            .iter()
            .filter(|d| d.path().as_str().starts_with("render/") && d.path().as_str().ends_with("/elapsed_gpu"))
            .filter_map(|d| Some((d.path().as_str().trim_start_matches("render/").trim_end_matches("/elapsed_gpu").to_owned(), d.average()?)))
            .collect();
        gpu.sort_by(|a, b| b.1.total_cmp(&a.1));
        let list: Vec<String> = gpu.iter().take(14).map(|(p, ms)| format!("{p} {ms:.2}")).collect();
        info!("perf: gpu ms: {}", list.join(", "));
    }
    if exit.read().next().is_some() && !frames.all.is_empty() {
        let (avg, median, p95, worst) = stats(&frames.all);
        let mut sorted = frames.all.clone();
        sorted.sort_by(f32::total_cmp);
        let p99 = sorted[(sorted.len() - 1) * 99 / 100];
        info!("perf: run {avg:.2} ms avg ({:.0} fps), median {median:.2}, p95 {p95:.2}, p99 {p99:.2}, worst {worst:.2} over {} frames", 1000.0 / avg, frames.all.len());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stats_of_frames() {
        let (avg, median, p95, worst) = super::stats(&[10.0, 20.0, 30.0, 40.0, 100.0]);
        assert_eq!((avg, median, worst), (40.0, 30.0, 100.0));
        assert_eq!(p95, 40.0);
    }
}

/// 15 s into a match and every 20 s after: mesh entities counted by their top-level
/// owner's name (digits dropped), skinned ones apart, to see what a map's
/// draws are.
fn census(
    time: Res<Time<Real>>,
    meshes: Query<(Entity, Has<bevy::mesh::skinning::SkinnedMesh>, &InheritedVisibility), With<Mesh3d>>,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    (standard, world, standard_assets, world_assets): (
        Query<&MeshMaterial3d<StandardMaterial>>,
        Query<&MeshMaterial3d<crate::world::WorldMaterial>>,
        Res<Assets<StandardMaterial>>,
        Res<Assets<crate::world::WorldMaterial>>,
    ),
    mut done: Local<bool>,
    mut since: Local<Option<f32>>,
) {
    // Again every 20 s, to see what builds up.
    let t = time.elapsed_secs();
    if meshes.is_empty() || t - *since.get_or_insert(t) < if *done { 20.0 } else { 15.0 } {
        return;
    }
    *done = true;
    *since = Some(t);
    // Blended (sorted, a draw each) and visible ones too.
    let blended = |e: Entity| {
        let mode = standard
            .get(e)
            .ok()
            .and_then(|m| standard_assets.get(&m.0))
            .map(|m| m.alpha_mode)
            .or_else(|| world.get(e).ok().and_then(|m| world_assets.get(&m.0)).map(|m| m.base.alpha_mode));
        mode.is_some_and(|m| !matches!(m, AlphaMode::Opaque | AlphaMode::Mask(_)))
    };
    let mut counts: std::collections::BTreeMap<String, (usize, usize, usize)> = Default::default();
    for (e, skinned, visible) in &meshes {
        let mut top = e;
        while let Ok(p) = parents.get(top) {
            top = p.parent();
        }
        let name = names.get(top).map_or("(unnamed)".to_string(), |n| n.as_str().chars().filter(|c| !c.is_ascii_digit()).collect());
        let c = counts.entry(name).or_default();
        c.0 += 1;
        c.1 += skinned as usize;
        c.2 += (visible.get() && blended(e)) as usize;
    }
    let mut list: Vec<_> = counts.into_iter().collect();
    list.sort_by_key(|(_, (n, _, _))| std::cmp::Reverse(*n));
    for (name, (n, skinned, blended)) in list.iter().take(16) {
        info!("perf: meshes under {name:?}: {n} ({skinned} skinned, {blended} blended and visible)");
    }
    let mut by_blend: Vec<_> = list.iter().filter(|(_, c)| c.2 > 0).map(|(name, c)| format!("{name} {}", c.2)).collect();
    by_blend.truncate(12);
    info!("perf: blended meshes by owner: {}", by_blend.join(", "));
}


#[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct FrameStart;
#[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct FrameEnd;

/// A thread's busy time: when its frame began (µs since `EPOCH`, 0 none),
/// and the total and count of frames since last read.
struct Busy {
    since: AtomicU64,
    total_us: AtomicU64,
    frames: AtomicU64,
    /// The thread's cycle count when its frame began, and the total since
    /// last read, and frames that went into it.
    cycles_since: AtomicU64,
    cycles_total: AtomicU64,
    cycle_frames: AtomicU64,
}
static MAIN_BUSY: Busy = Busy::new();
static RENDER_BUSY: Busy = Busy::new();
static EPOCH: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);

impl Busy {
    const fn new() -> Busy {
        Busy {
            since: AtomicU64::new(0),
            total_us: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            cycles_since: AtomicU64::new(0),
            cycles_total: AtomicU64::new(0),
            cycle_frames: AtomicU64::new(0),
        }
    }
    /// Average million cycles a frame since the last read.
    fn take_cycles(&self) -> f64 {
        let total = self.cycles_total.swap(0, Ordering::Relaxed);
        let frames = self.cycle_frames.swap(0, Ordering::Relaxed).max(1);
        total as f64 / frames as f64 / 1e6
    }
    /// Average ms a frame since the last read.
    fn take(&self) -> f64 {
        let total = self.total_us.swap(0, Ordering::Relaxed);
        let frames = self.frames.swap(0, Ordering::Relaxed).max(1);
        total as f64 / frames as f64 / 1000.0
    }
}

fn busy_mark(busy: &Busy, start: bool) {
    let now = EPOCH.elapsed().as_micros() as u64 + 1;
    let cycles = thread_cycles() + 1;
    if start {
        busy.since.store(now, Ordering::Relaxed);
        busy.cycles_since.store(cycles, Ordering::Relaxed);
    } else {
        let cycles_since = busy.cycles_since.swap(0, Ordering::Relaxed);
        if cycles_since > 0 && cycles >= cycles_since {
            busy.cycles_total.fetch_add(cycles - cycles_since, Ordering::Relaxed);
            busy.cycle_frames.fetch_add(1, Ordering::Relaxed);
        }
        let since = busy.since.swap(0, Ordering::Relaxed);
        if since > 0 {
            busy.total_us.fetch_add(now - since, Ordering::Relaxed);
            busy.frames.fetch_add(1, Ordering::Relaxed);
        }
    }
}

static LAST_CYCLES: AtomicU64 = AtomicU64::new(0);

/// CPU cycles the calling thread has used (0 off Windows).
fn thread_cycles() -> u64 {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
        }
        let mut cycles = 0;
        unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
        cycles
    }
    #[cfg(not(windows))]
    0
}

/// CPU cycles all of the process's threads have used (0 off Windows).
fn process_cycles() -> u64 {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn QueryProcessCycleTime(process: isize, cycles: *mut u64) -> i32;
        }
        let mut cycles = 0;
        unsafe { QueryProcessCycleTime(GetCurrentProcess(), &mut cycles) };
        cycles
    }
    #[cfg(not(windows))]
    0
}

/// Every five seconds, from the render world: each phase's views and what
/// they'd draw: multi-draw batch sets (one draw call each), batchable bins,
/// unbatchable meshes and sorted items.
fn draw_census(
    opaque: Res<bevy::render::render_phase::ViewBinnedRenderPhases<bevy::core_pipeline::core_3d::Opaque3d>>,
    alpha: Res<bevy::render::render_phase::ViewBinnedRenderPhases<bevy::core_pipeline::core_3d::AlphaMask3d>>,
    shadow: Res<bevy::render::render_phase::ViewBinnedRenderPhases<bevy::pbr::Shadow>>,
    pre: Res<bevy::render::render_phase::ViewBinnedRenderPhases<bevy::core_pipeline::prepass::Opaque3dPrepass>>,
    pre_alpha: Res<bevy::render::render_phase::ViewBinnedRenderPhases<bevy::core_pipeline::prepass::AlphaMask3dPrepass>>,
    transparent: Res<bevy::render::render_phase::ViewSortedRenderPhases<bevy::core_pipeline::core_3d::Transparent3d>>,
    views: Query<&bevy::render::view::ExtractedView>,
    mut next: Local<Option<std::time::Instant>>,
) {
    use bevy::render::render_phase::{BinnedPhaseItem, ViewBinnedRenderPhases};
    let now = std::time::Instant::now();
    if next.is_some_and(|t| now < t) {
        return;
    }
    *next = Some(now + std::time::Duration::from_secs(5));
    fn binned<B: BinnedPhaseItem>(name: &str, phases: &ViewBinnedRenderPhases<B>) -> String {
        let parts: Vec<String> = phases
            .0
            .values()
            .map(|p| {
                let unbatchable: usize = p.unbatchable_meshes.values().map(|u| u.entities.len()).sum();
                format!("{}/{}/{}", p.multidrawable_meshes.len(), p.batchable_meshes.len(), unbatchable)
            })
            .collect();
        format!("{name} [{}]", parts.join(" "))
    }
    let sorted: Vec<String> = transparent.0.values().map(|p| p.items.len().to_string()).collect();
    // The biggest view's opaque sets: distinct pipelines, material bind
    // groups and mesh slabs (what splits them).
    if let Some(p) = opaque.0.values().max_by_key(|p| p.multidrawable_meshes.len()) {
        let keys: Vec<_> = p.multidrawable_meshes.keys().collect();
        let distinct = |f: &dyn Fn(&bevy::core_pipeline::core_3d::Opaque3dBatchSetKey) -> String| {
            keys.iter().map(|k| f(k)).collect::<std::collections::HashSet<_>>().len()
        };
        info!(
            "perf: opaque sets split by {} pipelines, {} material bind groups, {} slabs, {} lightmap slabs",
            distinct(&|k| format!("{:?}", k.pipeline)),
            distinct(&|k| format!("{:?}", k.material_bind_group_index)),
            distinct(&|k| format!("{:?}", k.slabs)),
            distinct(&|k| format!("{:?}", k.lightmap_slab)),
        );
    }
    info!(
        "perf: draws (multidraw sets/bins/unbatchable per view), {} views: {}, {}, {}, {}, {}, transparent [{}]",
        views.iter().count(),
        binned("opaque", &opaque),
        binned("alpha", &alpha),
        binned("shadow", &shadow),
        binned("prepass", &pre),
        binned("prepass alpha", &pre_alpha),
        sorted.join(" ")
    );
}

fn shadow_try() -> Option<(usize, f32, usize)> {
    static TRY: std::sync::OnceLock<Option<(usize, f32, usize)>> = std::sync::OnceLock::new();
    *TRY.get_or_init(|| {
        let v = std::env::var("COD4RW_SHADOWTRY").ok()?;
        let p: Vec<&str> = v.split(',').collect();
        Some((p.first()?.trim().parse().ok()?, p.get(1)?.trim().parse().ok()?, p.get(2)?.trim().parse().ok()?))
    })
}
