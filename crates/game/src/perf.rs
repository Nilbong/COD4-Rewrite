//! Debug aid: with `COD4RW_PERF` set, log frame times in a match every five
//! seconds (average, median, 95th percentile and worst, in ms, real time)
//! with the entity count and the GPU's time in each render pass, and the
//! whole run's on exit. The first ten seconds of a match (loading,
//! settling) are left out.
//!
//! `COD4RW_PERF` can also name things to leave out, comma-separated, to
//! measure what they cost: `novsync`, `noshadows` (the sun's), `nossao`,
//! `noviewmodel` (its camera, which also finishes the frame), `nofx`
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
use std::sync::atomic::{AtomicBool, Ordering};

static NO_HUD: AtomicBool = AtomicBool::new(false);

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
                viewmodel: off("noviewmodel"),
                fx: off("nofx"),
                cascades2: off("cascades2"),
                prop_shadows: off("nopropshadows"),
                props: off("noprops"),
                ads: off("ads"),
            };
            NO_HUD.store(off("nohud"), Ordering::Relaxed);
            info!("perf: leaving out {without:?}");
            app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin)
                .init_resource::<FrameTimes>()
                .insert_resource(without)
                .init_resource::<Redone>()
                .add_systems(Last, (leave_out, count_redone, log_frames).chain())
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
    mut fx: Query<&mut Visibility, With<crate::fx::FxBatch>>,
    (cascades, names, children, mut done): (
        Query<(Entity, &bevy::light::CascadeShadowConfig), With<DirectionalLight>>,
        Query<(Entity, &Name)>,
        Query<&Children>,
        Local<bool>,
    ),
) {
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
        info!("perf: run {avg:.2} ms avg ({:.0} fps), median {median:.2}, p95 {p95:.2}, worst {worst:.2} over {} frames", 1000.0 / avg, frames.all.len());
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
