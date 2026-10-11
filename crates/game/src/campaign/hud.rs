//! The mission's own HUD: the objective, its marker in the world, the
//! mission's title as it starts, and the mission complete / failed screen.

use super::{Campaign, Outcome};
use crate::player::LocalPlayer;
use bevy::prelude::*;

/// Seconds after the end before a key leaves.
pub const END_DELAY: f32 = 2.5;
/// How long the mission's title shows at the start.
const TITLE_TIME: f32 = 6.0;
/// How long a script message stays up.
const MESSAGE_TIME: f32 = 5.0;

pub(super) fn build(app: &mut App) {
    app.add_systems(OnEnter(crate::state::GameState::InGame), setup.in_set(crate::state::Setup::Spawn))
        .add_systems(Update, (objective_text, marker, title, end_screen).run_if(super::active));
}

#[derive(Component)]
struct ObjectiveText;
#[derive(Component)]
struct Marker;
#[derive(Component)]
struct Title;
#[derive(Component)]
struct EndScreen;
#[derive(Component)]
struct EndHeading;
#[derive(Component)]
struct EndBody;

const GOLD: Color = Color::srgb(1.0, 0.85, 0.45);

fn text(s: &str, size: f32, color: Color) -> (Text, TextFont, TextColor, TextShadow) {
    (Text::new(s), TextFont { font_size: FontSize::Px(size), ..default() }, TextColor(color), TextShadow::default())
}

fn setup(mut commands: Commands, campaign: Option<Res<Campaign>>) {
    let Some(campaign) = campaign else { return };
    let root = commands
        .spawn(Node { width: percent(100), height: percent(100), position_type: PositionType::Absolute, ..default() })
        .id();
    commands.spawn((
        ObjectiveText,
        text("", 17.0, Color::WHITE),
        Node { position_type: PositionType::Absolute, left: px(28), top: percent(30.0), max_width: px(420), ..default() },
        ChildOf(root),
    ));
    commands.spawn((
        Marker,
        text("", 15.0, GOLD),
        TextLayout::justify(Justify::Center),
        Visibility::Hidden,
        Node { position_type: PositionType::Absolute, ..default() },
        ChildOf(root),
    ));
    commands.spawn((
        Title,
        text(&campaign.mission.title.to_uppercase(), 34.0, Color::WHITE),
        TextLayout::justify(Justify::Center),
        Node { position_type: PositionType::Absolute, top: percent(30), width: percent(100), justify_content: JustifyContent::Center, ..default() },
        ChildOf(root),
    ));
    let screen = commands
        .spawn((
            EndScreen,
            Visibility::Hidden,
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.0)),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: px(18),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    commands.spawn((EndHeading, text("", 46.0, GOLD), TextLayout::justify(Justify::Center), ChildOf(screen)));
    commands.spawn((EndBody, text("", 19.0, Color::WHITE), TextLayout::justify(Justify::Center), ChildOf(screen)));
}

/// A localized string from the level's zones, or the English given.
pub fn localize(content: &crate::content::Content, key: &str, english: &str) -> String {
    if key.is_empty() {
        return english.to_owned();
    }
    content
        .generic(iw3::zone::AssetType::LocalizeEntry, key)
        .and_then(|(_, n)| n.string("value").filter(|s| !s.is_empty()).map(str::to_owned))
        .unwrap_or_else(|| english.to_owned())
}

fn objective_text(time: Res<Time>, campaign: Res<Campaign>, content: Option<Res<crate::content::Content>>, mut q: Single<&mut Text, With<ObjectiveText>>) {
    let Some(content) = content else { return };
    let mut wanted = match campaign.current() {
        Some((key, english, _)) if campaign.ended.is_none() => {
            format!("OBJECTIVE\n{}", localize(&content, key, if english.is_empty() { key } else { english }))
        }
        _ => String::new(),
    };
    // The script's messages (`iprintln`), for a few seconds each; and the
    // use trigger's hint.
    let now = time.elapsed_secs();
    for (m, at) in campaign.messages.iter().filter(|(_, at)| now - at < MESSAGE_TIME) {
        let _ = at;
        wanted.push_str(&format!("\n\n{}", localize(&content, m, m)));
    }
    if let Some(h) = &campaign.use_hint {
        wanted.push_str(&format!("\n\nPress F: {}", localize(&content, h, h)));
    }
    if q.0 != wanted {
        q.0 = wanted;
    }
}

/// The objective's marker where it stands, with how far it is.
fn marker(
    campaign: Res<Campaign>,
    cameras: Query<(&Camera, &GlobalTransform, &crate::splitscreen::SlotCamera)>,
    player: Query<&Transform, With<LocalPlayer>>,
    mut q: Single<(&mut Text, &mut Node, &mut Visibility, &ComputedNode), With<Marker>>,
) {
    let (text, node, vis, computed) = &mut *q;
    let shown = (|| {
        let at = crate::units::pos(campaign.marker()?);
        let (camera, view, _) = cameras.iter().find(|c| c.2.0 == 0)?;
        let px = camera.world_to_viewport(view, at).ok()?;
        let me = player.single().ok()?.translation;
        let metres = me.distance(at) / crate::units::u(1.0) * 0.0254;
        Some((px, metres))
    })();
    let Some((px, metres)) = shown else {
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    let size = computed.size() * computed.inverse_scale_factor();
    node.left = Val::Px(px.x - size.x * 0.5);
    node.top = Val::Px(px.y - size.y * 0.5);
    let wanted = format!("[ + ]\n{metres:.0} m");
    if text.0 != wanted {
        text.0 = wanted;
    }
    vis.set_if_neq(Visibility::Inherited);
}

fn title(time: Res<Time>, campaign: Res<Campaign>, mut q: Single<(&mut TextColor, &mut Visibility), With<Title>>) {
    let Some(t0) = campaign.started else { return };
    let t = time.elapsed_secs() - t0;
    let over = if campaign.ended.is_some() { 0.0 } else { 1.0 };
    let alpha = (t / 1.0).min(1.0).min((TITLE_TIME - t) / 1.5).clamp(0.0, 1.0) * over;
    let (color, vis) = &mut *q;
    color.0 = Color::srgba(1.0, 1.0, 1.0, alpha);
    vis.set_if_neq(if alpha > 0.0 { Visibility::Inherited } else { Visibility::Hidden });
}

#[allow(clippy::type_complexity)]
fn end_screen(
    time: Res<Time>,
    campaign: Res<Campaign>,
    content: Option<Res<crate::content::Content>>,
    mut screen: Single<(&mut Visibility, &mut BackgroundColor), With<EndScreen>>,
    mut heading: Single<&mut Text, (With<EndHeading>, Without<EndBody>)>,
    mut body: Single<&mut Text, (With<EndBody>, Without<EndHeading>)>,
) {
    let (vis, bg) = &mut *screen;
    let Some((outcome, at)) = campaign.ended else {
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    let t = time.elapsed_secs() - at;
    vis.set_if_neq(Visibility::Inherited);
    bg.0 = Color::srgba(0.0, 0.0, 0.0, (t / 1.5).min(1.0) * 0.72);
    let m = campaign.mission;
    let (h, line) = match outcome {
        Outcome::Complete => ("MISSION COMPLETE", m.title.to_owned()),
        Outcome::Failed => ("MISSION FAILED", content.as_ref().map_or(m.failed.1.to_owned(), |c| localize(c, m.failed.0, m.failed.1))),
    };
    let secs = (at - campaign.started.unwrap_or(at)).max(0.0) as u32;
    let mut b = format!("{line}\n\nTime  {}:{:02}      Kills  {}", secs / 60, secs % 60, campaign.kills);
    if t > END_DELAY {
        b.push_str("\n\n\nPress ENTER to continue");
    }
    if heading.0 != h {
        heading.0 = h.to_owned();
    }
    if body.0 != b {
        body.0 = b;
    }
}
