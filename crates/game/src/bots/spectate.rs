//! Spectating the bots (`--spectate`): fly freely over the map, or ride along
//! with a bot in first or third person. You aren't in the match while
//! spectating; a bot takes your slot.
//!
//! Free camera: mouse to look, WASD to fly, Space/Ctrl up and down, Shift
//! for speed, the mouse wheel to change speed. Click to follow a bot
//! (left/right: next/previous), V for first/third person, F to fly free
//! again from where you are.
//!
//! `--spectate <bot name or role>` starts out following that bot. With
//! `COD4RW_SPECTATE_SHOTS=<dir>` as well, frames of its fights are saved
//! (every 0.15 s, 3 fights) and the game exits; adding
//! `COD4RW_SPECTATE_EVERY=<seconds>` saves 32 frames that far apart instead;
//! `COD4RW_SPECTATE_THIRD=1` starts in third person.

use super::{Bot, Mode};
use crate::combat::{Dead, Health, Pawn, Team};
use crate::movement::{Mover, ViewAngles};
use crate::player::MainCamera;
use crate::units::u;
use avian3d::prelude::*;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

/// The `--spectate` command-line argument (`Some("")` for the free camera),
/// set before the plugins build.
#[derive(Resource, Default)]
pub struct SpectateArg(pub Option<String>);

/// Present while spectating.
#[derive(Resource)]
pub struct Spectate {
    /// The bot to find and follow at the start (a name or a role), if any.
    start_with: Option<String>,
    following: Option<Entity>,
    third_person: bool,
    /// The free camera.
    pos: Option<Vec3>,
    yaw: f32,
    pitch: f32,
    speed: f32,
    /// The followed bot's body, hidden in first person.
    hidden_body: Option<Entity>,
}

/// Mouse look speed, radians per count.
const LOOK: f32 = 0.0022;
/// Free camera speed, metres per second (Shift: x3).
const FLY_SPEED: f32 = 8.0;

pub fn setup(app: &mut App) {
    let arg = app.world().get_resource::<SpectateArg>().and_then(|a| a.0.clone());
    // Photo mode ([`crate::photo`]) always flies free.
    let photo = crate::photo::active().then(String::new);
    let Some(name) = arg.or_else(|| std::env::var("COD4RW_SPECTATE").ok()).or(photo) else { return };
    let start_with = Some(name).filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case("free"));
    app.insert_resource(Spectate {
        start_with,
        following: None,
        third_person: std::env::var_os("COD4RW_SPECTATE_THIRD").is_some(),
        pos: None,
        yaw: 0.0,
        pitch: -0.5,
        speed: FLY_SPEED,
        hidden_body: None,
    })
    .add_systems(OnEnter(crate::state::GameState::InGame), spawn_overlay)
    .add_systems(Update, (controls, overlay).chain().run_if(crate::state::in_game))
    .add_systems(
        PostUpdate,
        (place_camera, fight_shots)
            .chain()
            .after(crate::player::follow_camera)
            .before(bevy::transform::TransformSystems::Propagate)
            .run_if(crate::state::in_game),
    );
}

type BotItem<'a> = (Entity, &'a Bot, &'a Pawn, &'a Transform, &'a Mover, &'a ViewAngles, &'a Health, Has<Dead>);

#[allow(clippy::too_many_arguments)]
fn controls(
    mut spec: ResMut<Spectate>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    time: Res<Time>,
    map: Option<Res<crate::world::MapInfo>>,
    bots: Query<BotItem>,
    camera: Single<&Transform, With<MainCamera>>,
    mut was_grabbed: Local<bool>,
) {
    let spec = &mut *spec;
    let grabbed = cursor.grab_mode != CursorGrabMode::None;
    // Clicks count once the mouse is captured (the first one captures it).
    let was = std::mem::replace(&mut *was_grabbed, grabbed);
    let clicked = |b: MouseButton| was && grabbed && mouse.just_pressed(b);

    // Start: above the middle of the map, looking down, or on the bot asked for.
    if spec.pos.is_none() {
        let spawns = map.as_ref().map(|m| m.spawns.iter().map(|s| s.pos).collect::<Vec<_>>()).unwrap_or_default();
        let centre = spawns.iter().copied().sum::<Vec3>() / spawns.len().max(1) as f32;
        spec.pos = Some(centre + Vec3::Y * u(700.0) + Vec3::Z * u(900.0));
        spec.pitch = -35f32.to_radians();
    }
    if let Some(name) = spec.start_with.clone() {
        let name = name.to_ascii_lowercase();
        let found = bots
            .iter()
            .find(|b| b.2.name.eq_ignore_ascii_case(&name))
            .or_else(|| bots.iter().find(|b| format!("{:?}", b.1.role).eq_ignore_ascii_case(&name)))
            .or_else(|| bots.iter().next());
        if let Some(b) = found {
            spec.following = Some(b.0);
            spec.start_with = None;
        }
    }

    // Next/previous bot, in a stable order: team, then name.
    let mut order: Vec<(Team, &str, Entity)> = bots.iter().map(|b| (b.2.team, b.2.name.as_str(), b.0)).collect();
    order.sort_by(|a, b| (a.0 as u8, a.1).cmp(&(b.0 as u8, b.1)));
    let step = if clicked(MouseButton::Left) {
        1
    } else if clicked(MouseButton::Right) {
        -1
    } else {
        0
    };
    if step != 0 && !order.is_empty() {
        let at = spec.following.and_then(|f| order.iter().position(|o| o.2 == f));
        let next = match at {
            Some(i) => (i as i32 + step).rem_euclid(order.len() as i32) as usize,
            None if step > 0 => 0,
            None => order.len() - 1,
        };
        spec.following = Some(order[next].2);
    }
    if keys.just_pressed(KeyCode::KeyV) {
        spec.third_person = !spec.third_person;
    }
    if keys.just_pressed(KeyCode::KeyF) && spec.following.is_some() {
        // Fly on from the current view.
        spec.following = None;
        spec.pos = Some(camera.translation);
        let (yaw, pitch, _) = camera.rotation.to_euler(EulerRot::YXZ);
        spec.yaw = yaw;
        spec.pitch = pitch;
    }
    if spec.following.is_some_and(|f| !bots.contains(f)) {
        spec.following = None;
    }

    // Free flight.
    if spec.following.is_none() {
        if grabbed {
            spec.yaw -= motion.delta.x * LOOK;
            spec.pitch = (spec.pitch - motion.delta.y * LOOK).clamp(-89f32.to_radians(), 89f32.to_radians());
        }
        if scroll.delta.y != 0.0 {
            spec.speed = (spec.speed * 1.25f32.powf(scroll.delta.y.signum())).clamp(1.0, 80.0);
        }
        let rot = Quat::from_euler(EulerRot::YXZ, spec.yaw, spec.pitch, 0.0);
        let key = |k: KeyCode| keys.pressed(k) as i32 as f32;
        let mut dir = rot * Vec3::NEG_Z * (key(KeyCode::KeyW) - key(KeyCode::KeyS))
            + rot * Vec3::X * (key(KeyCode::KeyD) - key(KeyCode::KeyA));
        dir.y += key(KeyCode::Space) - key(KeyCode::ControlLeft).max(key(KeyCode::KeyC));
        let fast = if keys.pressed(KeyCode::ShiftLeft) { 3.0 } else { 1.0 };
        if let Some(pos) = spec.pos.as_mut() {
            *pos += dir.normalize_or_zero() * spec.speed * fast * time.delta_secs();
        }
    }
}

#[allow(clippy::type_complexity)]
fn place_camera(
    mut spec: ResMut<Spectate>,
    spatial: SpatialQuery,
    bots: Query<(&Transform, &Mover, &ViewAngles, Has<Dead>, Option<&crate::thirdperson::Body>), With<Bot>>,
    mut camera: Single<&mut Transform, (With<MainCamera>, Without<Bot>)>,
    mut visibility: Query<&mut Visibility, Without<Bot>>,
    viewmodel: Query<Entity, With<crate::viewmodel::ViewModelRoot>>,
) {
    for e in &viewmodel {
        if let Ok(mut v) = visibility.get_mut(e) {
            *v = Visibility::Hidden;
        }
    }
    let followed = spec.following.and_then(|f| bots.get(f).ok());
    // Only a first-person view hides the followed bot's body.
    let hide = followed.filter(|_| !spec.third_person).and_then(|b| b.4.map(|body| body.0));
    if spec.hidden_body != hide {
        for (e, v) in [(spec.hidden_body, Visibility::Inherited), (hide, Visibility::Hidden)] {
            if let Some(mut vis) = e.and_then(|e| visibility.get_mut(e).ok()) {
                *vis = v;
            }
        }
        spec.hidden_body = hide;
    }

    let Some((tf, mover, view, dead, _)) = followed else {
        let Some(pos) = spec.pos else { return };
        camera.translation = pos;
        camera.rotation = Quat::from_euler(EulerRot::YXZ, spec.yaw, spec.pitch, 0.0);
        return;
    };
    let eye = if dead { tf.translation + Vec3::Y * u(8.0) } else { mover.eye(tf.translation) };
    let rot = Quat::from_euler(EulerRot::YXZ, view.yaw, view.pitch, 0.0);
    camera.rotation = rot;
    camera.translation = if spec.third_person {
        // Over the shoulder, pulled in where a wall is behind.
        let want = rot * Vec3::new(u(16.0), u(10.0), u(80.0));
        let reach = Dir3::new(want)
            .ok()
            .and_then(|d| spatial.cast_ray(eye, d, want.length(), true, &crate::collision::sight_filter()))
            .map_or(want.length(), |hit| (hit.distance - u(6.0)).max(0.0));
        eye + want.normalize_or_zero() * reach
    } else {
        eye
    };
}

/// With `COD4RW_SPECTATE_SHOTS=<dir>`: frames of the followed bot's fights.
fn fight_shots(
    mut commands: Commands,
    time: Res<Time>,
    spec: Res<Spectate>,
    bots: Query<(&Bot, Has<Dead>)>,
    mut shots: Local<(u32, u32, f32, bool)>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(dir) = std::env::var("COD4RW_SPECTATE_SHOTS") else { return };
    let Some((bot, dead)) = spec.following.and_then(|f| bots.get(f).ok()) else { return };
    let now = time.elapsed_secs();
    // `COD4RW_SPECTATE_EVERY=<seconds>`: a frame that often instead (32 of them).
    if let Some(every) = std::env::var("COD4RW_SPECTATE_EVERY").ok().and_then(|v| v.parse::<f32>().ok()) {
        let (taken, _, next, _) = &mut *shots;
        if now >= *next && now > 5.0 {
            *next = now + every;
            let path = std::path::PathBuf::from(&dir).join(format!("every_{:02}.png", *taken));
            std::fs::create_dir_all(&dir).ok();
            commands
                .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
                .observe(bevy::render::view::screenshot::save_to_disk(path));
            *taken += 1;
            if *taken >= 32 {
                exit.write(AppExit::Success);
            }
        }
        return;
    }
    let (fight, frame, next, in_fight) = &mut *shots;
    let fighting = !dead && bot.mode == Mode::Engage && bot.engagement.as_ref().is_some_and(|e| now >= e.react_at - 0.3);
    if fighting && !*in_fight {
        *in_fight = true;
        *frame = 0;
    }
    if *in_fight && now >= *next {
        *next = now + 0.15;
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join(format!("fight{}_{:02}.png", *fight, *frame));
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(path));
        *frame += 1;
        if *frame >= 16 {
            *in_fight = false;
            *fight += 1;
            *next = now + 4.0;
            if *fight >= 3 {
                exit.write(AppExit::Success);
            }
        }
    }
}

#[derive(Component)]
struct Overlay;

/// Score, time and kill feed (the player HUD shows them only to players).
#[derive(Component)]
struct MatchOverlay;

fn spawn_overlay(mut commands: Commands) {
    let text = |size: f32| {
        (Text::new(""), TextFont { font_size: FontSize::Px(size), ..default() }, TextColor(Color::WHITE), TextShadow::default())
    };
    commands.spawn((
        Overlay,
        text(18.0),
        Node { position_type: PositionType::Absolute, left: px(16), bottom: px(14), ..default() },
    ));
    commands.spawn((
        MatchOverlay,
        text(20.0),
        Node { position_type: PositionType::Absolute, left: px(16), top: px(12), ..default() },
    ));
}

#[allow(clippy::too_many_arguments)]
fn overlay(
    spec: Res<Spectate>,
    time: Res<Time>,
    state: Option<Res<crate::tdm::MatchState>>,
    feed: Res<crate::combat::KillFeed>,
    bots: Query<BotItem>,
    mut text: Single<&mut Text, (With<Overlay>, Without<MatchOverlay>)>,
    mut match_text: Single<&mut Text, (With<MatchOverlay>, Without<Overlay>)>,
    mut player_hints: Query<&mut Visibility, With<crate::hud::HintText>>,
) {
    let now = time.elapsed_secs();
    // Photo mode: nothing over the view.
    if crate::photo::active() {
        for mut v in &mut player_hints {
            v.set_if_neq(Visibility::Hidden);
        }
        if !text.0.is_empty() {
            text.0.clear();
        }
        if !match_text.0.is_empty() {
            match_text.0.clear();
        }
        return;
    }
    let mut lines = Vec::new();
    if let Some(state) = &state {
        let left = state.time_left(now);
        // Round modes (S&D, Sabotage) have no match clock: none shown.
        let clock = if left.is_finite() { format!("    {}:{:02}", (left / 60.0) as u32, (left % 60.0) as u32) } else { String::new() };
        lines.push(format!(
            "{} {}   {} {}   (to {}){clock}",
            Team::Allies.name(),
            state.score(Team::Allies),
            Team::Axis.name(),
            state.score(Team::Axis),
            state.score_limit,
        ));
    }
    lines.extend(feed.entries.iter().rev().filter(|e| now - e.time < 6.0).take(5).map(|e| e.text.clone()));
    let scores = lines.join("
");
    if match_text.0 != scores {
        match_text.0 = scores;
    }

    // The player controls don't apply.
    for mut v in &mut player_hints {
        *v = Visibility::Hidden;
    }
    let line = match spec.following.and_then(|f| bots.get(f).ok()) {
        Some((_, bot, pawn, _, _, _, health, dead)) => format!(
            "Following {} ({:?}, {:?}): {}\n\
             Click / right-click: next / previous bot   V: {}   F: free camera",
            pawn.name,
            pawn.team,
            bot.role,
            if dead { "dead".to_string() } else { format!("{:?}, {:.0} hp", bot.mode, health.current) },
            if spec.third_person { "first person" } else { "third person" },
        ),
        None => format!(
            "Free camera ({:.0} m/s)\n\
             WASD fly   Space / Ctrl up / down   Shift faster   Wheel speed   Click: follow a bot",
            spec.speed
        ),
    };
    if text.0 != line {
        text.0 = line;
    }
}
