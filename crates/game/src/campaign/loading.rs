//! A mission loading as CoD4 loads one: the level's briefing movie
//! (`video/cin_levels.txt`: `<map>_load`, `credits_load` for ac130 with
//! the credits dvar) full screen, with the `briefing` menu's thin loading
//! bar under it, while the level's zone is read on a thread. Once it's in,
//! the PC game's `pregame` menu takes over: the movie plays on (no bar, no
//! prompt), a click anywhere or Escape (`playerstart`) goes into the level,
//! and so does the movie's end. The briefings have no subtitles on PC.

use super::Campaign;
use crate::state::GameState;
use bevy::prelude::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

pub(super) fn build(app: &mut App) {
    app.add_systems(OnEnter(GameState::Loading), begin)
        .add_systems(Update, update.run_if(in_state(GameState::Loading)))
        .add_systems(OnExit(GameState::Loading), |mut commands: Commands| {
            HOLD.store(false, Ordering::Relaxed);
            commands.queue(|w: &mut World| crate::cinematic::stop(w));
        });
}

/// The level's zone, parsing: (map, thread).
static ZONE: Mutex<Option<(String, JoinHandle<anyhow::Result<iw3::zone::Zone>>)>> = Mutex::new(None);
/// The loading screen waits (the movie and the zone).
static HOLD: AtomicBool = AtomicBool::new(false);

/// Should the loading screen wait (a mission's briefing is on)?
pub fn hold_loading() -> bool {
    HOLD.load(Ordering::Relaxed)
}

/// The level's zone if it was read during the briefing ([`crate::world`]'s
/// load takes it).
pub fn take_zone(map: &str) -> Option<iw3::zone::Zone> {
    let mut slot = ZONE.lock().ok()?;
    if slot.as_ref().is_none_or(|(m, _)| m != map) {
        return None;
    }
    let (_, handle) = slot.take()?;
    match handle.join() {
        Ok(Ok(z)) => Some(z),
        Ok(Err(e)) => {
            warn!("campaign: reading the level ahead failed: {e:#}");
            None
        }
        Err(_) => None,
    }
}

fn zone_ready() -> bool {
    ZONE.lock().ok().is_some_and(|s| s.as_ref().is_some_and(|(_, h)| h.is_finished()))
}

#[derive(Component)]
struct Bar;
#[derive(Component)]
struct BarFrame;

#[derive(Resource)]
struct LoadingSince(f32);

fn begin(mut commands: Commands, time: Res<Time<Real>>, campaign: Option<Res<Campaign>>) {
    let Some(campaign) = campaign else { return };
    let map = campaign.mission.map.to_owned();
    HOLD.store(true, Ordering::Relaxed);
    commands.insert_resource(LoadingSince(time.elapsed_secs()));
    // The movie (`cin_levels.txt`'s rule).
    let movie = if map == "ac130" && std::env::var("COD4RW_CREDITS_LOAD").is_ok() { "credits_load".to_owned() } else { format!("{map}_load") };
    commands.queue(crate::cinematic::Play { name: movie, skippable: false });
    // The zone, meanwhile.
    let name = map.clone();
    let handle = std::thread::Builder::new()
        .name(format!("read {map}"))
        .spawn(move || {
            let install = iw3::Install::locate()?;
            let data = iw3::fastfile::load(&install.zone_path(&name))?;
            iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())
        })
        .ok();
    if let (Some(h), Ok(mut slot)) = (handle, ZONE.lock()) {
        *slot = Some((map, h));
    }
}

/// The bar fills as the level reads; once in, Use (or the movie's end)
/// goes on.
#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    time: Res<Time<Real>>,
    since: Option<Res<LoadingSince>>,
    cin: Option<Res<crate::cinematic::Cinematic>>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    mut bar: Query<&mut Node, With<Bar>>,
    mut frames: Query<&mut Visibility, With<BarFrame>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut made: Local<Option<Entity>>,
) {
    if !hold_loading() {
        *made = None;
        return;
    }
    let ready = zone_ready();
    // No movie (missing, or failed): wait for the zone only.
    let Some(cin) = cin else {
        if ready {
            HOLD.store(false, Ordering::Relaxed);
        }
        return;
    };
    if *made != Some(cin.root) {
        *made = Some(cin.root);
        // `briefing`: a 256x2 bar at (192, 430) of 640x480, its backing
        // (style 1) and the fill (style 6).
        let frame = commands
            .spawn((
                BarFrame,
                Node { position_type: PositionType::Absolute, left: percent(30.0), top: percent(89.6), width: percent(40.0), height: px(2.0), ..default() },
                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.15)),
                ChildOf(cin.root),
            ))
            .id();
        commands.spawn((Bar, Node { width: percent(0.0), height: percent(100.0), ..default() }, BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.8)), ChildOf(frame)));
    }
    // About how long the level takes to read; full once it has.
    let t = since.map_or(0.0, |s| time.elapsed_secs() - s.0);
    let fill = if ready { 100.0 } else { (t / 6.0).min(0.95) * 100.0 };
    for mut n in &mut bar {
        n.width = percent(fill);
    }
    // `pregame` has no bar.
    for mut v in &mut frames {
        v.set_if_neq(if ready { Visibility::Hidden } else { Visibility::Inherited });
    }
    // `pregame`: its full-screen button (a click, or Enter on it) and
    // Escape run `playerstart`; a pad's A or Start does the menus' select.
    let pressed = keys.any_just_pressed([KeyCode::Enter, KeyCode::Escape])
        || mouse.just_pressed(MouseButton::Left)
        || pads.iter().any(|p| p.just_pressed(GamepadButton::South) || p.just_pressed(GamepadButton::Start));
    if ready && (pressed || cin.finished) {
        info!("campaign: into {} (movie at frame {:?} of {})", cin.name, cin.shown, cin.frame_count);
        HOLD.store(false, Ordering::Relaxed);
    }
}
