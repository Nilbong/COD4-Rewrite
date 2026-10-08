//! How first person feels, beyond CoD4: the player's own body under the
//! view ([`body`]), the gun lit by the world around it, the gun lagging as
//! the view turns, and a richer bob. Each has a switch in Options > Game
//! (and a `COD4RW_FP_*` override for test runs), so it can be compared
//! with CoD4's.

use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub mod body;
pub mod feel;

pub struct FirstPersonPlugin;

impl Plugin for FirstPersonPlugin {
    fn build(&self, app: &mut App) {
        body::build(app);
        feel::build(app);
        if std::env::var_os("COD4RW_FP_PITCH").is_some() || std::env::var_os("COD4RW_FP_TURN").is_some() {
            app.add_systems(Update, test_view.before(crate::movement::MovementSet));
        }
        if std::env::var_os("COD4RW_FP_MOVE").is_some() {
            app.add_systems(PreUpdate, test_move.after(bevy::input::InputSystems));
        }
        if std::env::var_os("COD4RW_FP_STRIP").is_some() {
            app.add_systems(Last, test_strip);
        }
    }
}

/// Test runs: hold the view's pitch (`COD4RW_FP_PITCH`, degrees, down
/// negative) and turn it steadily (`COD4RW_FP_TURN`, degrees a second; a
/// leading `~` swings it back and forth over a second each way).
fn test_view(time: Res<Time>, mut players: Query<&mut crate::movement::ViewAngles, With<crate::splitscreen::LocalSlot>>) {
    let num = |k: &str| std::env::var(k).ok().and_then(|v| v.trim_start_matches('~').parse::<f32>().ok());
    let swing = std::env::var("COD4RW_FP_TURN").is_ok_and(|v| v.starts_with('~'));
    for mut v in &mut players {
        if let Some(p) = num("COD4RW_FP_PITCH") {
            v.pitch = p.to_radians();
        }
        if let Some(t) = num("COD4RW_FP_TURN") {
            let dir = if swing && (time.elapsed_secs() as u32) % 2 == 1 { -1.0 } else { 1.0 };
            v.yaw += (t * dir).to_radians() * time.delta_secs();
        }
    }
}

static FULL_BODY: AtomicBool = AtomicBool::new(true);
static WORLD_LIGHTING: AtomicBool = AtomicBool::new(true);
static SWAY: AtomicBool = AtomicBool::new(true);
static RICH_BOB: AtomicBool = AtomicBool::new(true);

/// A switch, unless a test run overrides it (`COD4RW_FP_<NAME>=0|1`).
fn switch(flag: &AtomicBool, name: &str) -> bool {
    match std::env::var(format!("COD4RW_FP_{name}")).as_deref() {
        Ok("0") => false,
        Ok(_) => true,
        Err(_) => flag.load(Ordering::Relaxed),
    }
}

/// The player's own body seen when looking down.
pub fn full_body() -> bool {
    switch(&FULL_BODY, "BODY")
}

/// The viewmodel lit by the world at the eye (else CoD4's: the light grid
/// and an unshadowed sun).
pub fn world_lighting() -> bool {
    switch(&WORLD_LIGHTING, "LIGHTING")
}

/// The gun lagging behind the view as it turns.
pub fn sway() -> bool {
    switch(&SWAY, "SWAY")
}

/// The richer view and gun bob (else CoD4's).
pub fn rich_bob() -> bool {
    switch(&RICH_BOB, "BOB")
}

/// From the settings ([`crate::settings_apply`]).
pub fn apply(s: &crate::settings::Settings) {
    FULL_BODY.store(s.on("cg_fullbody"), Ordering::Relaxed);
    WORLD_LIGHTING.store(s.text("cg_vmlighting") != "classic", Ordering::Relaxed);
    SWAY.store(s.on("cg_weaponsway"), Ordering::Relaxed);
    RICH_BOB.store(s.text("cg_bobstyle") != "classic", Ordering::Relaxed);
}

/// Test runs: keep moving (`COD4RW_FP_MOVE=walk|sprint|crouch|course`, `+jump`
/// to jump every two seconds), from two seconds into play; or `third`:
/// stand in third person.
/// When a local player was first in play (seconds), for the test aids.
static IN_PLAY_AT: std::sync::Mutex<Option<f32>> = std::sync::Mutex::new(None);

fn in_play(time: &Time, players: bool) -> Option<f32> {
    let mut at = IN_PLAY_AT.lock().ok()?;
    if at.is_none() && players {
        *at = Some(time.elapsed_secs());
    }
    at.map(|a| time.elapsed_secs() - a)
}

fn test_move(
    time: Res<Time>,
    players: Query<(), With<crate::splitscreen::LocalSlot>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut cursor: Single<&mut bevy::window::CursorOptions, With<bevy::window::PrimaryWindow>>,
) {
    let how = std::env::var("COD4RW_FP_MOVE").unwrap_or_default();
    let Some(t) = in_play(&time, !players.is_empty()) else { return };
    if t < 2.0 {
        return;
    }
    // Play only takes input with the mouse captured.
    if cursor.grab_mode == bevy::window::CursorGrabMode::None {
        cursor.grab_mode = bevy::window::CursorGrabMode::Locked;
    }
    // `course`: sprint forward (2.6-3.6 s), stop, strafe right (4.3-5.3 s),
    // then jump and land: the gun's weight in one strip.
    if how.contains("course") {
        let (fwd, right, sprint, jump) = (t >= 2.6 && t < 3.6, t >= 4.3 && t < 5.3, t >= 2.6 && t < 3.6, t >= 5.3 && t < 5.35);
        for (key, on) in [(KeyCode::KeyW, fwd), (KeyCode::KeyD, right), (KeyCode::ShiftLeft, sprint), (KeyCode::Space, jump)] {
            if on {
                keys.press(key);
            } else {
                keys.release(key);
            }
        }
        return;
    }
    // `third`: switch to third person once, standing still.
    if how.contains("third") {
        if t < 2.05 {
            keys.press(KeyCode::F5);
        } else {
            keys.release(KeyCode::F5);
        }
        return;
    }
    // `ads`: aim in from 2.5 s into play.
    let ads_at = std::env::var("COD4RW_FP_ADS_AT").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(2.5);
    if how.contains("ads") && t >= ads_at {
        mouse.press(MouseButton::Right);
    }
    // `crouch`, `prone`: down first; `still`: then stand there.
    if how.contains("crouch") && t < 2.1 {
        keys.press(KeyCode::KeyC);
    } else {
        keys.release(KeyCode::KeyC);
    }
    if how.contains("prone") && t < 2.1 {
        keys.press(KeyCode::ControlLeft);
    } else {
        keys.release(KeyCode::ControlLeft);
    }
    if how.contains("still") || t < 2.6 {
        return;
    }
    keys.press(KeyCode::KeyW);
    if how.contains("sprint") {
        keys.press(KeyCode::ShiftLeft);
    }
    if how.contains("jump") && (t % 2.0) < 0.05 {
        keys.press(KeyCode::Space);
    } else {
        keys.release(KeyCode::Space);
    }
}

/// Test runs: `COD4RW_FP_STRIP=<dir>[,start,count,every]` saves `count`
/// screenshots `every` seconds apart from `start` seconds into play
/// (defaults 3 s, 24, 0.05).
fn test_strip(
    mut commands: Commands,
    time: Res<Time>,
    players: Query<(), With<crate::splitscreen::LocalSlot>>,
    mut taken: Local<usize>,
    mut next: Local<f32>,
) {
    let spec = std::env::var("COD4RW_FP_STRIP").unwrap_or_default();
    let mut parts = spec.split(',');
    let dir = std::path::PathBuf::from(parts.next().unwrap_or("."));
    let num = |p: Option<&str>, d: f32| p.and_then(|v| v.parse().ok()).unwrap_or(d);
    let (start, count, every) = (num(parts.next(), 3.0), num(parts.next(), 24.0) as usize, num(parts.next(), 0.05));
    let Some(t) = in_play(&time, !players.is_empty()) else { return };
    if *taken >= count || t < start.max(*next) {
        return;
    }
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(format!("f{:02}.png", *taken));
    commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(path));
    *taken += 1;
    *next = t + every;
}

