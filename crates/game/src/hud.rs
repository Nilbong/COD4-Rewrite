//! The controls hint. The HUD, the Tab scoreboard included, is CoD4's own,
//! drawn by [`crate::ui`].

use crate::player::LocalPlayer;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_hint_font)
            .add_systems(OnEnter(crate::state::GameState::InGame), setup_hud.in_set(crate::state::Setup::Spawn))
            .add_systems(Update, (update_hint, update_mantle_hint, update_cover_hint).run_if(crate::state::in_game));
    }
}

#[derive(Component)]
pub struct HintText;

/// IW3's mantle hint, while there's something to climb.
#[derive(Component)]
struct MantleHint;

/// 3rd Person TDM: cover within reach ([`crate::cover`]).
#[derive(Component)]
struct CoverHint;

/// The menus' typeface (Bahnschrift) for the hints, made before the match
/// starts; Bevy's own font where the PC lacks it.
fn load_hint_font(mut fonts: ResMut<Assets<Font>>) {
    if HINT_FONT.get().is_none()
        && let Some(bytes) = crate::ui::next::font::face_bytes()
    {
        HINT_FONT.set(fonts.add(Font::from_bytes(bytes.to_vec()))).ok();
        debug!("hud: hints in Bahnschrift");
    }
}

static HINT_FONT: std::sync::OnceLock<Handle<Font>> = std::sync::OnceLock::new();

fn text(s: &str, size: f32) -> (Text, TextFont, TextColor, TextShadow) {
    (
        Text::new(s),
        match HINT_FONT.get() {
            Some(font) => TextFont { font: bevy::text::FontSource::Handle(font.clone()), font_size: FontSize::Px(size), ..default() },
            None => TextFont { font_size: FontSize::Px(size), ..default() },
        },
        TextColor(Color::WHITE),
        TextShadow::default(),
    )
}

fn setup_hud(mut commands: Commands) {
    // Full-screen root.
    let root = commands
        .spawn(Node { width: percent(100), height: percent(100), position_type: PositionType::Absolute, ..default() })
        .id();
    commands.spawn((
        HintText,
        text(MATCH_HINT, 16.0),
        TextLayout::justify(Justify::Center),
        Node { position_type: PositionType::Absolute, bottom: px(60), width: percent(100), justify_content: JustifyContent::Center, ..default() },
        ChildOf(root),
    ));
    commands.spawn((
        MantleHint,
        text("Press SPACE to mantle", 20.0),
        TextLayout::justify(Justify::Center),
        Visibility::Hidden,
        Node { position_type: PositionType::Absolute, top: percent(58), width: percent(100), justify_content: JustifyContent::Center, ..default() },
        ChildOf(root),
    ));
    commands.spawn((
        CoverHint,
        text("Press X to take cover", 20.0),
        TextLayout::justify(Justify::Center),
        Visibility::Hidden,
        Node { position_type: PositionType::Absolute, top: percent(63), width: percent(100), justify_content: JustifyContent::Center, ..default() },
        ChildOf(root),
    ));
}

/// "Press X to take cover", like the mantle hint, with a keyboard.
fn update_cover_hint(pad: Res<crate::gamepad::ActiveDevice>, mut hint: Single<&mut Visibility, With<CoverHint>>) {
    let show = crate::cover::available() && pad.pad.is_none() && !crate::splitscreen::active();
    hint.set_if_neq(if show { Visibility::Inherited } else { Visibility::Hidden });
}

fn update_mantle_hint(
    player: Query<&crate::movement::Mover, With<LocalPlayer>>,
    pad: Res<crate::gamepad::ActiveDevice>,
    mut hint: Single<&mut Visibility, With<MantleHint>>,
) {
    // With a controller in use, `gamepad::prompts` shows its own.
    let show = player.single().is_ok_and(|m| m.mantle_hint) && pad.pad.is_none() && !crate::splitscreen::active();
    hint.set_if_neq(if show { Visibility::Inherited } else { Visibility::Hidden });
}

/// The controls hint shows while the mouse is free of the game and no menu
/// is up, and no controller is in use (`gamepad::prompts` has its own),
/// nor while the mouse picks an airstrike's spot.
fn update_hint(
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    menus: Option<Res<crate::ui::Frontend>>,
    pad: Res<crate::gamepad::ActiveDevice>,
    player: Query<(), With<LocalPlayer>>,
    selecting: Option<Res<crate::killstreaks::airstrike::Selecting>>,
    mut hint: Single<(&mut Visibility, &mut Text), With<HintText>>,
) {
    if player.is_empty() {
        return;
    }
    let free = cursor.grab_mode == CursorGrabMode::None
        && !crate::ui::menu_open(menus.as_deref())
        && pad.pad.is_none()
        && selecting.is_none()
        && !crate::splitscreen::active();
    let (visibility, text) = &mut *hint;
    // Headquarters says what to do where it matters (its prompts).
    let free = free && !crate::hq::active();
    visibility.set_if_neq(if free { Visibility::Inherited } else { Visibility::Hidden });
    // Headquarters has nothing to fight with.
    let wanted = if crate::hq::active() { HQ_HINT } else { MATCH_HINT };
    if text.0 != wanted {
        text.0 = wanted.to_owned();
    }
}

const MATCH_HINT: &str = "Click to play  |  WASD move, Shift sprint, Space jump, C crouch, Ctrl prone
LMB fire, RMB aim, R reload, Tab scores, Esc release mouse
B switch Bodycam / CoD4 gunplay, Q/E lean (Bodycam)";
const HQ_HINT: &str = "Click to play  |  WASD move, Shift sprint, Space jump, C crouch, Ctrl prone  |  Esc menu";
