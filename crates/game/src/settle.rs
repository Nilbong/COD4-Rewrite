//! The map's load screen stays up until the match has settled.
//!
//! Entering the match does most of its loading in its first frames (the
//! world's meshes and materials, characters, the bots' nav graph and map
//! analysis, the HUD's menus), each frame of it up to seconds long. A
//! player shown the game then sees it frozen. So from entering the match the
//! game's clock is held (`Time<Virtual>` paused: the match clock, spawn
//! grace, bots, movement and netplay all run on it) and the load screen is
//! drawn over everything (`crate::ui`), until [`CALM_FRAMES`] frames in a row
//! come in under [`CALM_MS`], or [`MOST`] of smooth running has gone by. Locally, in splitscreen
//! and online alike (each machine holds its own clock while it loads).

use crate::state::{GameState, Setup};
use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct SettlePlugin;

impl Plugin for SettlePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Settle>()
            .add_systems(OnEnter(GameState::InGame), start.after(Setup::Spawn))
            .add_systems(Last, watch.run_if(crate::state::in_game))
            .add_systems(OnExit(GameState::InGame), stop);
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
    SETTLING.store(true, Ordering::Relaxed);
    virt.pause();
}

fn watch(real: Res<Time<Real>>, mut virt: ResMut<Time<Virtual>>, mut settle: ResMut<Settle>) {
    if !settling() {
        return;
    }
    let ms = real.delta_secs() * 1000.0;
    settle.calm = if ms < CALM_MS { settle.calm + 1 } else { 0 };
    // A slow frame is loading: only up to a calm frame's worth counts.
    settle.waited += real.delta_secs().min(CALM_MS / 1000.0);
    let waited = settle.waited;
    if settle.calm >= CALM_FRAMES || waited >= MOST {
        SETTLING.store(false, Ordering::Relaxed);
        virt.unpause();
        info!("settle: match shown after {waited:.2} s{}", if waited >= MOST { " (the most)" } else { "" });
    }
}

fn stop(mut virt: ResMut<Time<Virtual>>) {
    if settling() {
        SETTLING.store(false, Ordering::Relaxed);
        virt.unpause();
    }
}
