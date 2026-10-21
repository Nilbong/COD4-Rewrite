//! In-match text chat (Modern Warfare's): a few lines at the middle of the
//! screen's left edge, newest at the bottom, each fading a few seconds after
//! it came in: an optional `[DEAD]` / `[TEAM]` tag, the sender's name in
//! their side's colour (friendly blue, enemy red; plain in free-for-all),
//! then the text, in the menus' typeface.
//!
//! Anything can speak by writing a [`ChatMessage`]; the bots' banter
//! ([`crate::bots::chat::ChatMessage`]) shows here too.
//! The player types with T (everyone) or Y (their team): an input line
//! opens under the chat, Enter sends, Esc cancels; the game's controls are
//! held while typing. Team messages reach only that team.
//!
//! Not yet: online play doesn't relay chat between players (a later
//! `LobbyMsg`); each copy of the game shows its own.
//!
//! Debug aid: `COD4RW_CHATTEST=1` has a few bots speak (both sides, a dead
//! one, team chat) from 4 s in, and opens the input line with a line typed.

use crate::combat::{Dead, Pawn, Team};
use crate::player::LocalPlayer;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// A line of chat.
#[derive(Message, Clone, Debug)]
pub struct ChatMessage {
    /// The pawn speaking (`None`: the game itself).
    pub sender: Option<Entity>,
    /// The name shown (clan tag and all).
    pub name: String,
    /// Their side, for the name's colour (`None`: free-for-all, or the game).
    pub team: Option<Team>,
    /// Said to their team only.
    pub team_only: bool,
    /// Said while dead (`[DEAD]`).
    pub dead: bool,
    pub text: String,
}

/// How many lines show, how long each stays, and its fade at the end (s).
const LINES: usize = 6;
const SHOW: f32 = 8.0;
const FADE: f32 = 1.5;
/// The longest line typed.
const MAX_CHARS: usize = 120;

static TYPING: AtomicBool = AtomicBool::new(false);
/// Lines the player just sent as the bots' chat message too (sender, text):
/// that copy is skipped once when it comes round (online, another player
/// of the same name is still shown).
static SENT: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// The player is typing a chat line (the game's controls are held).
pub fn typing() -> bool {
    TYPING.load(Ordering::Relaxed)
}

pub(super) fn build(app: &mut App) {
    app.add_message::<ChatMessage>()
        .init_resource::<ChatLog>()
        .init_resource::<ChatInput>()
        .add_systems(OnEnter(crate::state::GameState::InGame), (reset, spawn))
        .add_systems(PreUpdate, swallow_game_input.after(bevy::input::InputSystems).run_if(|| typing()))
        .add_systems(Update, (collect, type_line, draw).chain().run_if(crate::state::in_game));
    if std::env::var_os("COD4RW_CHATTEST").is_some() {
        app.add_systems(Update, test.before(collect).run_if(crate::state::in_game));
    }
}

/// `COD4RW_CHATTEST`: bots speak, then the input line opens.
fn test(time: Res<Time>, mut step: Local<usize>, mut out: MessageWriter<ChatMessage>, mut input: ResMut<ChatInput>, pawns: Query<(Entity, &Pawn), Without<LocalPlayer>>, me: Query<&Pawn, With<LocalPlayer>>) {
    let Ok(mine) = me.single().map(|p| p.team) else { return };
    let t = time.elapsed_secs();
    let said = [("gg ez", false, false, true), ("anyone got eyes on B?", true, false, false), ("nice shot", false, false, false), ("lag", false, true, true), ("pushing long", true, false, false)];
    if *step < said.len() && t > 4.0 + *step as f32 * 0.6 {
        let (text, team_only, dead, enemy) = said[*step];
        if let Some((e, p)) = pawns.iter().find(|(_, p)| (p.team != mine) == enemy) {
            out.write(ChatMessage { sender: Some(e), name: p.name.clone(), team: Some(p.team), team_only, dead, text: text.into() });
        }
        *step += 1;
    } else if *step == said.len() && t > 8.0 {
        input.0 = Some((false, "on my way".into()));
        TYPING.store(true, Ordering::Relaxed);
        *step += 1;
    }
}

#[derive(Clone)]
struct Line {
    msg: ChatMessage,
    at: f32,
}

/// What's been said this match (as player 1 sees it).
#[derive(Resource, Default)]
struct ChatLog(Vec<Line>);

/// The line being typed: to the team only, and its text.
#[derive(Resource, Default)]
struct ChatInput(Option<(bool, String)>);

#[derive(Component)]
struct ChatRoot;

/// Row `n` (0 the oldest shown): its tag, name and text.
#[derive(Component)]
struct Row(usize, u8);

#[derive(Component)]
struct InputRow;

#[derive(Component)]
struct InputText;

fn reset(mut log: ResMut<ChatLog>, mut input: ResMut<ChatInput>) {
    log.0.clear();
    input.0 = None;
    TYPING.store(false, Ordering::Relaxed);
    SENT.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

fn rgba(c: super::next::theme::Rgba, a: f32) -> Color {
    Color::srgba(c[0], c[1], c[2], c[3] * a)
}

/// The menus' typeface (Bahnschrift), made once; Bevy's own font where the
/// PC lacks it.
fn ui_font(fonts: &mut Assets<Font>) -> bevy::text::FontSource {
    static FONT: std::sync::OnceLock<Handle<Font>> = std::sync::OnceLock::new();
    if let Some(h) = FONT.get() {
        return bevy::text::FontSource::Handle(h.clone());
    }
    match super::next::font::face_bytes() {
        Some(bytes) => bevy::text::FontSource::Handle(FONT.get_or_init(|| fonts.add(Font::from_bytes(bytes.to_vec()))).clone()),
        None => bevy::text::FontSource::default(),
    }
}

fn spawn(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    use super::next::theme as t;
    let font = ui_font(&mut fonts);
    let text = |size: f32, c: Color| (Text::new(""), TextFont { font: font.clone(), font_size: FontSize::Px(size), ..default() }, TextColor(c), TextShadow::default());
    let root = commands
        .spawn((
            ChatRoot,
            Node { position_type: PositionType::Absolute, left: px(16), top: percent(42), width: px(560), flex_direction: FlexDirection::Column, row_gap: px(2), ..default() },
            DespawnOnExit(crate::state::GameState::InGame),
        ))
        .id();
    for n in 0..LINES {
        let row = commands.spawn((Node { column_gap: px(6), ..default() }, ChildOf(root))).id();
        commands.spawn((Row(n, 0), text(15.0, rgba(t::INK_DIM, 1.0)), ChildOf(row)));
        commands.spawn((Row(n, 1), text(16.0, rgba(t::INK, 1.0)), ChildOf(row)));
        commands.spawn((Row(n, 2), text(16.0, rgba(t::INK, 1.0)), Node { flex_shrink: 1.0, ..default() }, ChildOf(row)));
    }
    let input = commands
        .spawn((
            InputRow,
            Visibility::Hidden,
            Node { margin: UiRect::top(px(6)), padding: UiRect::axes(px(10), px(5)), border: UiRect { left: px(3), ..default() }, ..default() },
            BackgroundColor(rgba(t::hex(0x0b0e0f, 0.8), 1.0)),
            BorderColor::all(rgba(t::ACCENT, 1.0)),
            ChildOf(root),
        ))
        .id();
    commands.spawn((InputText, text(16.0, rgba(t::INK, 1.0)), ChildOf(input)));
}

/// New lines, as player 1 sees them (another team's team chat isn't).
fn collect(
    time: Res<Time>,
    mut msgs: MessageReader<ChatMessage>,
    mut bots: MessageReader<crate::bots::chat::ChatMessage>,
    mut log: ResMut<ChatLog>,
    me: Query<&Pawn, With<LocalPlayer>>,
) {
    let mine = me.single().ok().map(|p| p.team);
    // (The bots' lines as ours: their team for the colour, none in
    // free-for-all. The player's own, also written as one, is shown from
    // its own copy.)
    let ffa = crate::combat::free_for_all();
    let mut sent = SENT.lock().unwrap_or_else(|e| e.into_inner());
    let from_bots: Vec<ChatMessage> = bots
        .read()
        .filter(|b| match sent.iter().position(|(n, t)| *n == b.sender && *t == b.text) {
            Some(i) => {
                sent.remove(i);
                false
            }
            None => true,
        })
        .map(|b| ChatMessage { sender: None, name: b.sender.clone(), team: (!ffa).then_some(b.team), team_only: b.team_only, dead: false, text: b.text.clone() })
        .collect();
    for m in msgs.read().chain(from_bots.iter()) {
        if m.team_only && m.team.is_some() && m.team != mine {
            continue;
        }
        log.0.push(Line { msg: m.clone(), at: time.elapsed_secs() });
    }
    let len = log.0.len();
    if len > 50 {
        log.0.drain(..len - 50);
    }
}

/// T or Y opens the input line; typing, Enter sends, Esc cancels.
#[allow(clippy::too_many_arguments)]
fn type_line(
    keys: Res<ButtonInput<KeyCode>>,
    mut events: MessageReader<KeyboardInput>,
    mut input: ResMut<ChatInput>,
    fe: Option<Res<super::Frontend>>,
    me: Query<(Entity, &Pawn, Has<Dead>), With<LocalPlayer>>,
    mut out: MessageWriter<ChatMessage>,
    mut relay: MessageWriter<crate::bots::chat::ChatMessage>,
) {
    let Some((team_only, text)) = input.0.as_mut() else {
        events.clear();
        // (Not over a menu, the bug report or the map editor, nor in the
        // campaign.)
        let free = fe.as_ref().is_none_or(|fe| fe.stack.is_empty()) && !crate::campaign::active() && !crate::flycam::active();
        if free && (keys.just_pressed(KeyCode::KeyT) || keys.just_pressed(KeyCode::KeyY)) {
            input.0 = Some((keys.just_pressed(KeyCode::KeyY) && !crate::combat::free_for_all(), String::new()));
            TYPING.store(true, Ordering::Relaxed);
        }
        return;
    };
    let mut done = None;
    for e in events.read() {
        if e.state != ButtonState::Pressed {
            continue;
        }
        match &e.logical_key {
            Key::Enter => done = Some(true),
            Key::Escape => done = Some(false),
            Key::Backspace => {
                text.pop();
            }
            _ => {
                if let Some(t) = &e.text {
                    for c in t.chars().filter(|c| !c.is_control()) {
                        if text.chars().count() < MAX_CHARS {
                            text.push(c);
                        }
                    }
                }
            }
        }
    }
    let Some(send) = done else { return };
    let (team_only, text) = (*team_only, text.trim().to_owned());
    input.0 = None;
    TYPING.store(false, Ordering::Relaxed);
    if !send || text.is_empty() {
        return;
    }
    let Ok((e, pawn, dead)) = me.single() else { return };
    let ffa = crate::combat::free_for_all();
    // (Also as the bots' chat line: online play relays that one to the
    // other players, and the bots hear it. The box shows this copy, with
    // its [DEAD] tag, and skips that one.)
    SENT.lock().unwrap_or_else(|e| e.into_inner()).push((pawn.name.clone(), text.clone()));
    relay.write(crate::bots::chat::ChatMessage { sender: pawn.name.clone(), team: pawn.team, text: text.clone(), team_only });
    out.write(ChatMessage { sender: Some(e), name: pawn.name.clone(), team: (!ffa).then_some(pawn.team), team_only, dead, text });
}

/// The game's controls held while typing (the keys still reach the line).
fn swallow_game_input(mut keys: ResMut<ButtonInput<KeyCode>>, mut mouse: ResMut<ButtonInput<MouseButton>>) {
    keys.reset_all();
    mouse.reset_all();
}

/// The rows: the newest lines still showing (all the last ones while
/// typing), fading out at the end.
#[allow(clippy::type_complexity)]
fn draw(
    time: Res<Time>,
    log: Res<ChatLog>,
    input: Res<ChatInput>,
    me: Query<&Pawn, With<LocalPlayer>>,
    mut rows: Query<(&Row, &mut Text, &mut TextColor), Without<InputText>>,
    mut input_row: Query<&mut Visibility, With<InputRow>>,
    mut input_text: Query<&mut Text, (With<InputText>, Without<Row>)>,
) {
    use super::next::theme as t;
    let now = time.elapsed_secs();
    let typing = input.0.is_some();
    let mine = me.single().ok().map(|p| p.team);
    let shown: Vec<&Line> = log.0.iter().rev().filter(|l| typing || now - l.at < SHOW).take(LINES).collect::<Vec<_>>().into_iter().rev().collect();
    // (Bottom-aligned: the newest on the last row.)
    let skip = LINES - shown.len();
    for (row, mut text, mut colour) in &mut rows {
        let line = row.0.checked_sub(skip).and_then(|i| shown.get(i));
        let Some(line) = line else {
            if !text.0.is_empty() {
                text.0.clear();
            }
            continue;
        };
        let alpha = if typing { 1.0 } else { ((SHOW - (now - line.at)) / FADE).clamp(0.0, 1.0) };
        let m = &line.msg;
        let (want, c) = match row.1 {
            0 => {
                let mut tag = String::new();
                if m.dead {
                    tag.push_str("[DEAD]");
                }
                if m.team_only {
                    tag.push_str(if tag.is_empty() { "[TEAM]" } else { " [TEAM]" });
                }
                (tag, rgba(t::INK_DIM, alpha))
            }
            1 => {
                let side = match (m.team, mine) {
                    (Some(a), Some(b)) if a == b => t::FRIENDLY,
                    (Some(_), _) => t::ENEMY,
                    _ => t::REWARD,
                };
                (format!("{}:", m.name), rgba(side, alpha))
            }
            _ => (m.text.clone(), rgba(t::INK, alpha)),
        };
        if text.0 != want {
            text.0 = want;
        }
        if colour.0 != c {
            colour.0 = c;
        }
    }
    for mut v in &mut input_row {
        v.set_if_neq(if typing { Visibility::Inherited } else { Visibility::Hidden });
    }
    if let Some((team_only, line)) = &input.0 {
        let want = format!("{}  {line}_", if *team_only { "[TEAM]" } else { "[ALL]" });
        for mut text in &mut input_text {
            if text.0 != want {
                text.0 = want.clone();
            }
        }
    }
}
