//! Button prompts for the pad in use: rows of glyphs and words, rebuilt
//! when the pad changes between Xbox and PlayStation. In a match they
//! replace [`crate::hud`]'s keyboard controls hint and mantle prompt while
//! the pad is in use.

use super::ActiveDevice;
use super::glyphs::{Glyph, Glyphs};
use crate::movement::Mover;
use crate::player::LocalPlayer;
use crate::state::{GameState, Setup};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

pub struct PromptsPlugin;

impl Plugin for PromptsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::InGame), spawn_match_prompts.in_set(Setup::Spawn))
            .add_systems(Update, show_match_prompts.run_if(crate::state::in_game))
            .add_systems(PostUpdate, build_prompts);
    }
}

/// Above CoD4's HUD (`ui::draw`'s layers).
pub const PROMPT_Z: i32 = 1100;

#[derive(Clone, Debug)]
pub enum Part {
    Glyph(Glyph),
    Text(String),
}

pub fn glyph(g: Glyph) -> Part {
    Part::Glyph(g)
}

pub fn text(s: &str) -> Part {
    Part::Text(s.to_owned())
}

/// A row of glyphs and words, `height` pixels tall.
#[derive(Component, Clone, Debug)]
pub struct PadPrompt {
    pub parts: Vec<Part>,
    pub height: f32,
}

impl PadPrompt {
    /// The prompt with its row layout.
    pub fn row(parts: Vec<Part>, height: f32) -> (PadPrompt, Node) {
        (
            PadPrompt { parts, height },
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                column_gap: px((height * 0.22).round()),
                ..default()
            },
        )
    }
}

/// The pad a prompt was built for.
#[derive(Component)]
struct BuiltFor(super::PadKind);

/// (Re)build prompts that are new, changed, or were built for the other pad.
fn build_prompts(
    mut commands: Commands,
    glyphs: Option<Res<Glyphs>>,
    active: Res<ActiveDevice>,
    prompts: Query<(Entity, Ref<PadPrompt>, Option<&BuiltFor>)>,
) {
    let (Some(glyphs), Some(kind)) = (glyphs, active.pad) else { return };
    for (e, prompt, built) in &prompts {
        if built.is_some_and(|b| b.0 == kind) && !prompt.is_changed() {
            continue;
        }
        commands.entity(e).despawn_children().insert(BuiltFor(kind));
        for part in &prompt.parts {
            match part {
                Part::Glyph(g) => glyphs.spawn(&mut commands, e, kind, *g, prompt.height),
                Part::Text(s) => {
                    commands.spawn((
                        Text::new(s.clone()),
                        TextFont { font_size: FontSize::Px((prompt.height * 0.6).round()), ..default() },
                        TextColor(Color::WHITE),
                        TextShadow::default(),
                        ChildOf(e),
                    ));
                }
            }
        }
    }
}

/// The pad's controls hint, shown where [`crate::hud`]'s keyboard one is.
#[derive(Component)]
struct PadHint;

/// "Press A to mantle".
#[derive(Component)]
struct PadMantleHint;

/// "Press B to take cover" (3rd Person TDM, [`crate::cover`]).
#[derive(Component)]
struct PadCoverHint;

fn spawn_match_prompts(mut commands: Commands) {
    use Glyph::*;
    let hint = commands
        .spawn((
            PadHint,
            Node {
                position_type: PositionType::Absolute,
                bottom: px(52),
                width: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(4),
                ..default()
            },
            GlobalZIndex(PROMPT_Z),
            Visibility::Hidden,
        ))
        .id();
    let rows = [
        vec![
            glyph(South),
            text("Play   |  "),
            glyph(LeftStick),
            text("Move "),
            glyph(RightStick),
            text("Look "),
            glyph(LeftStickClick),
            text("Sprint "),
            glyph(RightStickClick),
            text("Knife "),
            glyph(South),
            text("Jump "),
            glyph(East),
            text("Crouch (hold: prone)"),
        ],
        vec![
            glyph(RightTrigger),
            text("Fire "),
            glyph(LeftTrigger),
            text("Aim "),
            glyph(West),
            text("Reload "),
            glyph(North),
            text("Switch weapon "),
            glyph(RightBumper),
            text("Frag "),
            glyph(LeftBumper),
            text("Special grenade "),
            glyph(DPadRight),
            text("Kill streak"),
        ],
        vec![
            glyph(Select),
            text("Scores "),
            glyph(Start),
            text("Menu "),
            glyph(DPadDown),
            text("Bodycam / CoD4 gunplay "),
            glyph(LeftTrigger),
            text("+"),
            glyph(LeftStickClick),
            glyph(RightStickClick),
            text("Lean (Bodycam) "),
            glyph(DPadUp),
            text("Night vision "),
            glyph(Select),
            text("+"),
            glyph(RightStickClick),
            text("Third person"),
        ],
    ];
    for row in rows {
        commands.spawn((PadPrompt::row(row, 26.0), ChildOf(hint)));
    }
    commands.spawn((
        PadMantleHint,
        PadPrompt { parts: vec![text("Press"), glyph(South), text("to mantle")], height: 32.0 },
        Node {
            position_type: PositionType::Absolute,
            top: percent(58),
            width: percent(100),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            column_gap: px(7),
            ..default()
        },
        GlobalZIndex(PROMPT_Z),
        Visibility::Hidden,
    ));
    commands.spawn((
        PadCoverHint,
        PadPrompt { parts: vec![text("Press"), glyph(East), text("to take cover")], height: 32.0 },
        Node {
            position_type: PositionType::Absolute,
            top: percent(63),
            width: percent(100),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            column_gap: px(7),
            ..default()
        },
        GlobalZIndex(PROMPT_Z),
        Visibility::Hidden,
    ));
}

/// The controls hint shows while the game doesn't have the mouse and no
/// menu is up (as the keyboard one does); the mantle prompt while there's
/// something to climb. Both only while the pad is in use.
fn show_match_prompts(
    active: Res<ActiveDevice>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    menus: Option<Res<crate::ui::Frontend>>,
    player: Query<&Mover, With<LocalPlayer>>,
    mut hint: Query<&mut Visibility, (With<PadHint>, Without<PadMantleHint>, Without<PadCoverHint>)>,
    mut mantle: Query<&mut Visibility, (With<PadMantleHint>, Without<PadHint>, Without<PadCoverHint>)>,
    mut cover: Query<&mut Visibility, (With<PadCoverHint>, Without<PadHint>, Without<PadMantleHint>)>,
) {
    // Not in splitscreen: they'd cover everyone's views.
    let pad = active.pad.is_some() && !crate::splitscreen::active();
    let me = player.single().ok();
    let free = cursor.grab_mode == CursorGrabMode::None && !crate::ui::menu_open(menus.as_deref());
    let show = |on: bool| if on { Visibility::Inherited } else { Visibility::Hidden };
    for mut v in &mut hint {
        // (Headquarters says what to do where it matters: its prompts.)
        v.set_if_neq(show(pad && me.is_some() && free && !crate::hq::active()));
    }
    for mut v in &mut mantle {
        v.set_if_neq(show(pad && me.is_some_and(|m| m.mantle_hint)));
    }
    for mut v in &mut cover {
        v.set_if_neq(show(pad && crate::cover::available()));
    }
}
