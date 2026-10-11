//! Bug reports from inside the game: press F10 and the game takes a
//! screenshot, pauses, and asks what's wrong. Enter saves the screenshot and
//! a text file (the description plus the map, the time, where you and the
//! camera are, and what every bot is doing) to `bugreports/`; Esc throws it
//! away. Either way the game carries on.

use crate::combat::{Dead, Health, Pawn};
use crate::movement::{Mover, ViewAngles};
use crate::player::{LocalPlayer, MainCamera};
use crate::units;
use bevy::asset::RenderAssetUsages;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use std::path::PathBuf;

pub struct BugReportPlugin;

impl Plugin for BugReportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BugReport>()
            .add_systems(
                PreUpdate,
                swallow_game_input.after(bevy::input::InputSystems).run_if(|r: Res<BugReport>| r.stage != Stage::Idle),
            )
            .add_systems(Update, (start, show_prompt, type_text, toast).chain().run_if(crate::state::in_game));
        if std::env::var_os("COD4RW_BUGTEST").is_some() {
            app.add_systems(PreUpdate, test_driver.before(bevy::input::InputSystems).run_if(crate::state::in_game));
        }
    }
}

/// Debug aid: with `COD4RW_BUGTEST=<text>`, file a bug report with that text
/// through the real keyboard path (F10, the text, Enter) a few seconds in,
/// then exit.
fn test_driver(
    mut commands: Commands,
    real: Res<Time<Real>>,
    window: Single<Entity, With<PrimaryWindow>>,
    mut keys: MessageWriter<KeyboardInput>,
    mut step: Local<usize>,
    mut start: Local<Option<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = real.elapsed_secs();
    let t0 = *start.get_or_insert(now);
    let text = std::env::var("COD4RW_BUGTEST").unwrap_or_default();
    let press = |key_code: KeyCode, logical_key: Key, text: Option<&str>| KeyboardInput {
        key_code,
        logical_key,
        state: ButtonState::Pressed,
        text: text.map(Into::into),
        repeat: false,
        window: *window,
    };
    let release = |key_code: KeyCode, logical_key: Key| KeyboardInput {
        key_code,
        logical_key,
        state: ButtonState::Released,
        text: None,
        repeat: false,
        window: *window,
    };
    match *step {
        0 if now - t0 > 8.0 => {
            keys.write(press(KeyCode::F10, Key::F10, None));
            *step += 1;
        }
        1 => {
            keys.write(release(KeyCode::F10, Key::F10));
            *step += 1;
        }
        2 if now - t0 > 9.5 => {
            for c in text.chars() {
                let s = c.to_string();
                keys.write(press(KeyCode::KeyA, Key::Character(s.as_str().into()), Some(&s)));
            }
            *step += 1;
        }
        3 if now - t0 > 10.2 => {
            // A picture of the prompt itself, when asked for.
            if let Ok(path) = std::env::var("COD4RW_BUGTEST_SHOT") {
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
            }
            *step += 1;
        }
        4 if now - t0 > 10.8 => {
            keys.write(press(KeyCode::Enter, Key::Enter, None));
            *step += 1;
        }
        5 if now - t0 > 13.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

#[derive(Default, PartialEq, Eq, Clone, Copy, Debug)]
enum Stage {
    #[default]
    Idle,
    /// Screenshot asked for; waiting for it before showing the prompt (so
    /// the prompt isn't in it).
    Capturing,
    Typing,
}

#[derive(Resource, Default)]
pub struct BugReport {
    stage: Stage,
    /// Where this report goes, without extension.
    base: PathBuf,
    text: String,
    /// The screenshot, for the prompt's thumbnail.
    shot: Option<Handle<Image>>,
    /// Facts about the moment, written with the description.
    context: String,
    /// What to show briefly after saving, and until when.
    toast: Option<(String, f32)>,
}

#[derive(Component)]
struct Prompt;

#[derive(Component)]
struct PromptText;

#[derive(Component)]
struct Toast;

/// While the prompt is up the game ignores the keyboard and mouse (typing
/// "c" mustn't crouch, Esc mustn't open the menu); the prompt reads typed
/// text separately.
fn swallow_game_input(mut keys: ResMut<ButtonInput<KeyCode>>, mut mouse: ResMut<ButtonInput<MouseButton>>) {
    keys.reset_all();
    mouse.reset_all();
}

#[allow(clippy::too_many_arguments)]
fn start(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut report: ResMut<BugReport>,
    mut time: ResMut<Time<Virtual>>,
    map: Option<Res<crate::world::MapName>>,
    state: Option<Res<crate::tdm::MatchState>>,
    player: Query<(&Pawn, &Transform, &Mover, &ViewAngles, &Health, Has<Dead>), With<LocalPlayer>>,
    camera: Single<&GlobalTransform, With<MainCamera>>,
    bots: Query<(&crate::bots::Bot, &Pawn, &Transform, &Health, Has<Dead>)>,
    spectating: Option<Res<crate::bots::Spectate>>,
) {
    if report.stage != Stage::Idle || !keys.just_pressed(KeyCode::F10) {
        return;
    }
    let map = map.map_or("map".to_string(), |m| m.0.clone());
    let stamp = timestamp();
    let base = PathBuf::from("bugreports").join(format!("{}_{map}", stamp.replace([':', ' '], "-")));
    std::fs::create_dir_all("bugreports").ok();

    // What was going on, for whoever reads the report.
    let mut c = format!("Map: {map}\nGame time: {:.1} s\n", time.elapsed_secs());
    // Which game this is (its file, when it was built) and the game type:
    // what a report was made with.
    if let Ok(exe) = std::env::current_exe() {
        let built = std::fs::metadata(&exe).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        c += &format!("Game: {} (file modified at unix time {built}, version {})\nGame type: {}\n", exe.display(), env!("CARGO_PKG_VERSION"), crate::modes::current().name());
    }
    if let Some(s) = state.as_deref() {
        let left = s.time_left(time.elapsed_secs());
        c += &format!(
            "Score: {} {} - {} {} ({}:{:02} left)\n",
            crate::combat::Team::Allies.name(),
            s.score(crate::combat::Team::Allies),
            crate::combat::Team::Axis.name(),
            s.score(crate::combat::Team::Axis),
            (left / 60.0) as u32,
            (left % 60.0) as u32
        );
    }
    let cod = |p: Vec3| units::to_cod(p).map(|v| v.round() as i32);
    match player.single() {
        Ok((pawn, tf, mover, view, health, dead)) => {
            c += &format!(
                "You ({:?}): at {:?}, view yaw {:.0} pitch {:.0} (CoD degrees), {:?}, {:.0} hp{}\n",
                pawn.team,
                cod(tf.translation),
                view.yaw.to_degrees() + 90.0,
                -view.pitch.to_degrees(),
                mover.stance,
                health.current,
                if dead { ", dead" } else { "" }
            );
        }
        Err(_) => c += &format!("Spectating{}\n", if spectating.is_some() { "" } else { " (no player)" }),
    }
    let (yaw, pitch, _) = camera.rotation().to_euler(EulerRot::YXZ);
    c += &format!(
        "Camera: at {:?}, yaw {:.0} pitch {:.0} (CoD degrees)\nBots:\n",
        cod(camera.translation()),
        yaw.to_degrees() + 90.0,
        -pitch.to_degrees()
    );
    let mut lines: Vec<String> = bots
        .iter()
        .map(|(bot, pawn, tf, health, dead)| {
            format!(
                "  {:<12} {:?} at {:?}, {:.0} hp: {}",
                pawn.name,
                pawn.team,
                cod(tf.translation),
                health.current,
                if dead { "dead".to_string() } else { bot.summary() }
            )
        })
        .collect();
    lines.sort();
    c += &lines.join("\n");

    report.stage = Stage::Capturing;
    report.base = base.clone();
    report.text.clear();
    report.context = c;
    report.shot = None;
    time.pause();
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(base.with_extension("png")))
        .observe(|captured: On<ScreenshotCaptured>, mut report: ResMut<BugReport>, mut images: ResMut<Assets<Image>>| {
            // A copy for the prompt, fully opaque (the alpha of an HDR
            // frame holds brightness, not coverage).
            let mut image = captured.image.clone();
            let opaque_rgba8 = matches!(
                image.texture_descriptor.format,
                TextureFormat::Rgba8UnormSrgb | TextureFormat::Rgba8Unorm | TextureFormat::Bgra8UnormSrgb | TextureFormat::Bgra8Unorm
            );
            if let (true, Some(data)) = (opaque_rgba8, image.data.as_mut()) {
                for px in data.chunks_exact_mut(4) {
                    px[3] = 255;
                }
                image.asset_usage = RenderAssetUsages::RENDER_WORLD;
                report.shot = Some(images.add(image));
            }
            report.stage = Stage::Typing;
        });
}

fn show_prompt(
    mut commands: Commands,
    report: Res<BugReport>,
    prompt: Query<Entity, With<Prompt>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if report.stage != Stage::Typing || !prompt.is_empty() {
        return;
    }
    cursor.grab_mode = CursorGrabMode::None;
    cursor.visible = true;
    let text = |s: &str, size: f32, color: Color| {
        (Text::new(s), TextFont { font_size: FontSize::Px(size), ..default() }, TextColor(color))
    };
    let root = commands
        .spawn((
            Prompt,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            // Above the HUD (1000-1002) and menus.
            GlobalZIndex(5000),
        ))
        .id();
    let panel = commands
        .spawn((
            Node {
                width: px(760),
                flex_direction: FlexDirection::Column,
                row_gap: px(10),
                padding: UiRect::all(px(16)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.08, 0.09, 0.1)),
            ChildOf(root),
        ))
        .id();
    commands.spawn((text("Bug report: what's wrong in this picture?", 22.0, Color::WHITE), ChildOf(panel)));
    if let Some(shot) = &report.shot {
        commands.spawn((
            ImageNode::new(shot.clone()),
            Node { width: percent(100), aspect_ratio: Some(16.0 / 9.0), ..default() },
            ChildOf(panel),
        ));
    }
    commands.spawn((
        PromptText,
        text("_", 18.0, Color::srgb(0.95, 0.9, 0.6)),
        Node { min_height: px(60), padding: UiRect::all(px(8)), ..default() },
        BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.06)),
        ChildOf(panel),
    ));
    commands.spawn((text("Enter: save    Esc: discard    (the game is paused)", 15.0, Color::srgb(0.7, 0.7, 0.7)), ChildOf(panel)));
}

#[allow(clippy::too_many_arguments)]
fn type_text(
    mut commands: Commands,
    mut events: MessageReader<KeyboardInput>,
    mut report: ResMut<BugReport>,
    mut time: ResMut<Time<Virtual>>,
    real: Res<Time<Real>>,
    prompt: Query<Entity, With<Prompt>>,
    mut shown: Query<&mut Text, With<PromptText>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if report.stage != Stage::Typing {
        events.clear();
        return;
    }
    let mut done = None;
    for e in events.read() {
        if e.state != ButtonState::Pressed {
            continue;
        }
        match &e.logical_key {
            Key::Enter => done = Some(true),
            Key::Escape => done = Some(false),
            Key::Backspace => {
                report.text.pop();
            }
            _ => {
                if let Some(t) = &e.text {
                    report.text.extend(t.chars().filter(|c| !c.is_control()));
                }
            }
        }
    }
    for mut t in &mut shown {
        t.0 = format!("{}_", report.text);
    }
    let Some(save) = done else { return };
    let png = report.base.with_extension("png");
    let txt = report.base.with_extension("txt");
    let message = if save {
        let body = format!(
            "Bug report {} UTC\n\nWhat's wrong:\n{}\n\nScreenshot: {}\n\n{}\n",
            timestamp(),
            if report.text.trim().is_empty() { "(no description)" } else { report.text.trim() },
            png.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()),
            report.context
        );
        match std::fs::write(&txt, body) {
            Ok(()) => format!("Bug report saved: {}", txt.display()),
            Err(e) => format!("Couldn't save the bug report: {e}"),
        }
    } else {
        std::fs::remove_file(&png).ok();
        "Bug report discarded".to_string()
    };
    info!("{message}");
    report.toast = Some((message, real.elapsed_secs() + 3.0));
    report.stage = Stage::Idle;
    report.shot = None;
    for e in &prompt {
        commands.entity(e).despawn();
    }
    time.unpause();
    cursor.grab_mode = CursorGrabMode::Locked;
    cursor.visible = false;
}

/// A line confirming what happened, for a few seconds.
fn toast(mut commands: Commands, mut report: ResMut<BugReport>, real: Res<Time<Real>>, shown: Query<Entity, With<Toast>>) {
    let Some((message, until)) = report.toast.clone() else { return };
    if real.elapsed_secs() > until {
        report.toast = None;
        for e in &shown {
            commands.entity(e).despawn();
        }
    } else if shown.is_empty() {
        commands.spawn((
            Toast,
            Text::new(message),
            TextFont { font_size: FontSize::Px(16.0), ..default() },
            TextColor(Color::srgb(0.6, 1.0, 0.6)),
            TextShadow::default(),
            Node { position_type: PositionType::Absolute, right: px(16), top: px(48), ..default() },
        ));
    }
}

/// The current date and time in UTC, `YYYY-MM-DD HH:MM:SS`.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn timestamp_has_the_shape_of_a_date() {
        let t = super::timestamp();
        assert_eq!(t.len(), 19);
        assert!(t.starts_with("20"));
    }
}
