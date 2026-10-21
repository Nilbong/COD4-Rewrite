//! The map's load screen stays up until the match has settled.
//!
//! Entering the match does most of its loading in its first frames (the
//! world's meshes and materials, characters, the bots' nav graph and map
//! analysis, the HUD's menus), each frame of it up to seconds long. A
//! player shown the game then sees it frozen. So from entering the match the
//! game's clock is held (`Time<Virtual>` paused: the match clock, spawn
//! grace, bots, movement and netplay all run on it) and the load screen is
//! drawn over everything (`crate::ui`), until [`CALM_FRAMES`] frames in a row
//! come in under [`CALM_MS`] with no render pipelines left compiling, or
//! [`MOST`] of smooth running has gone by. Locally, in splitscreen
//! and online alike (each machine holds its own clock while it loads).

use crate::state::{GameState, Setup};
use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub struct SettlePlugin;

impl Plugin for SettlePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Settle>()
            .add_systems(OnEnter(GameState::InGame), start.after(Setup::Spawn))
            .add_systems(Last, watch.run_if(crate::state::in_game))
            .add_systems(OnExit(GameState::InGame), stop);
        if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
            render.add_systems(bevy::render::Render, count_compiling.in_set(bevy::render::RenderSystems::Cleanup));
        }
        if let Ok(dir) = std::env::var("COD4RW_SETTLESHOT") {
            app.insert_resource(ShotDir(dir.into())).add_systems(Last, shoot.run_if(crate::state::in_game));
        }
    }
}

/// Debug aid: with `COD4RW_SETTLESHOT=<dir>`, a screenshot of every frame
/// from entering the match until five after it's shown
/// (`<n>_<ms>ms_<settling|shown>.png`), to check none shows it frozen.
#[derive(Resource)]
struct ShotDir(std::path::PathBuf);

fn shoot(mut commands: Commands, dir: Res<ShotDir>, real: Res<Time<Real>>, mut n: Local<u32>, mut after: Local<u32>) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    if *after >= 5 {
        return;
    }
    if !settling() {
        *after += 1;
    }
    std::fs::create_dir_all(&dir.0).ok();
    let name = format!("{:03}_{:.0}ms_{}.png", *n, real.delta_secs() * 1000.0, if settling() { "settling" } else { "shown" });
    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.0.join(name)));
    *n += 1;
}

/// A frame this quick counts as settled (ms).
const CALM_MS: f32 = 50.0;
/// Calm frames in a row needed (the load comes in bursts a few quick frames
/// apart: the world, then the characters, then the bots' maps).
const CALM_FRAMES: u32 = 10;
/// The longest the load screen stays (seconds), not counting slow frames
/// (loading): if the game never runs smoothly, it's shown anyway.
const MOST: f32 = 5.0;

static SETTLING: AtomicBool = AtomicBool::new(false);
/// Render pipelines queued or compiling, from the render world.
static COMPILING: AtomicUsize = AtomicUsize::new(0);
/// Render frames since a pipeline was last first needed.
static STABLE: AtomicUsize = AtomicUsize::new(0);
/// The load screen waits for this many frames with no new pipeline: a few
/// things (the HUD's first menus, a view's first frame) ask for theirs a
/// frame or two after the last batch compiled.
const STABLE_FRAMES: usize = 30;

fn count_compiling(cache: Res<bevy::render::render_resource::PipelineCache>, mut total: Local<usize>) {
    use bevy::render::render_resource::CachedPipelineState;
    let n = cache.pipelines().filter(|p| matches!(p.state, CachedPipelineState::Queued | CachedPipelineState::Creating(_))).count();
    COMPILING.store(n, Ordering::Relaxed);
    // `COD4RW_PERF`: pipelines first needed mid-match (each a compile, a
    // hitch with a cold shader cache), as they come.
    let all = cache.pipelines().count();
    if all > *total && *total > 0 && !settling() && crate::perf::on() {
        use bevy::render::render_resource::PipelineDescriptor;
        let labels: Vec<String> = cache
            .pipelines()
            .skip(*total)
            .map(|p| match &p.descriptor {
                PipelineDescriptor::RenderPipelineDescriptor(d) => format!("{:?}", d.label),
                PipelineDescriptor::ComputePipelineDescriptor(d) => format!("compute {:?}", d.label),
            })
            .collect();
        info!("perf: {} new render pipelines mid-match ({all} in all): {}", all - *total, labels.join(", "));
    }
    if all > *total {
        STABLE.store(0, Ordering::Relaxed);
    } else {
        STABLE.fetch_add(1, Ordering::Relaxed);
    }
    *total = all;
}
/// How far settling has got, 0..1 (an `f32`'s bits), for the load screen's bar.
static PROGRESS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// How far the match is from being shown, 0..1: calm frames so far, or the
/// time waited against [`MOST`], whichever is further.
pub fn progress() -> f32 {
    f32::from_bits(PROGRESS.load(Ordering::Relaxed))
}

/// The match is still settling: the load screen shows, the clock is held.
pub fn settling() -> bool {
    SETTLING.load(Ordering::Relaxed)
}

#[derive(Resource, Default)]
struct Settle {
    /// Time waited outside slow frames (seconds), and calm frames in a row.
    waited: f32,
    calm: u32,
}

fn start(real: Res<Time<Real>>, mut virt: ResMut<Time<Virtual>>, mut settle: ResMut<Settle>) {
    let _ = real;
    *settle = Settle::default();
    PROGRESS.store(0f32.to_bits(), Ordering::Relaxed);
    SETTLING.store(true, Ordering::Relaxed);
    virt.pause();
}

fn watch(real: Res<Time<Real>>, mut virt: ResMut<Time<Virtual>>, mut settle: ResMut<Settle>, fe: Option<Res<crate::ui::Frontend>>) {
    if !settling() {
        return;
    }
    let ms = real.delta_secs() * 1000.0;
    // (Calm counts from when the HUD's UI is there: the settings it
    // brings change the cameras a few frames later.)
    let ui_up = fe.is_some() || crate::perf::old("settle");
    settle.calm = if ms < CALM_MS && ui_up { settle.calm + 1 } else { 0 };
    // A slow frame is loading: only up to a calm frame's worth counts.
    settle.waited += real.delta_secs().min(CALM_MS / 1000.0);
    let waited = settle.waited;
    let p = (settle.calm as f32 / CALM_FRAMES as f32).max(waited / MOST).clamp(0.0, 1.0);
    PROGRESS.store(p.to_bits(), Ordering::Relaxed);
    // Not while pipelines are still compiling: a view whose pipeline is
    // needed then (the 3D scope's camera, Bevy's blit for it) holds its
    // frame until the whole queue ahead of it is done, seconds at the start
    // of a match. Better under the load screen than in the player's face.
    let unsteady = STABLE.load(Ordering::Relaxed) < STABLE_FRAMES && !crate::perf::old("settle");
    let compiling = COMPILING.load(Ordering::Relaxed) + unsteady as usize;
    // Nor while kill streaks' models are being drawn once under it
    // (`killstreaks::prewarm`).
    // Nor before the HUD's own UI has loaded (a match started straight
    // from the command line gets it a moment later): its settings (bloom,
    // volumetric light) and HUD only come with it, and every pipeline
    // would be made again just after.
    let warming = crate::killstreaks::prewarm::holding() || !ui_up;
    if settle.calm >= CALM_FRAMES && compiling == 0 && !warming || waited >= MOST {
        SETTLING.store(false, Ordering::Relaxed);
        virt.unpause();
        info!(
            "settle: match shown after {waited:.2} s{}",
            if waited >= MOST { format!(" (the most; {compiling} pipelines still compiling)") } else { String::new() }
        );
    }
}

fn stop(mut virt: ResMut<Time<Virtual>>) {
    if settling() {
        SETTLING.store(false, Ordering::Relaxed);
        virt.unpause();
    }
}
