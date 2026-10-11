//! The menus on a pad. The D-pad or left stick moves focus to the nearest
//! item that way in the top menu (the mouse cursor goes there too, so
//! `menu_input`'s hovering agrees), a newly opened menu focuses the item it
//! was left from, else its first, the right stick turns the gun preview, and a footer shows the
//! buttons. A and B reach `menu_input` as Enter and Escape
//! ([`crate::gamepad`]).

use super::{Frontend, MenuInput, Placement, interactive};
use crate::gamepad::glyphs::Glyph;
use crate::gamepad::prompts::{PROMPT_Z, PadPrompt, glyph, text};
use crate::gamepad::{ActiveDevice, PadFrame};
use crate::state::GameState;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

pub(super) fn build(app: &mut App) {
    app.add_systems(Startup, spawn_footer)
        .add_systems(Update, navigate.before(MenuInput).run_if(resource_exists::<Frontend>))
        .add_systems(Update, show_footer);
}

/// Preview turning: pixels of mouse drag per second at full stick.
const ROTATE_SPEED: f32 = 320.0;

/// A focusable item of the top menu: index, centre and size on screen.
#[derive(Clone, Copy, Debug)]
pub(super) struct Spot {
    pub(super) item: usize,
    centre: Vec2,
}

/// The top menu's items a pad can focus: interactive, shown, not a popup's
/// full-screen backdrop, and on top where they're drawn.
pub(super) fn spots(fe: &Frontend, pl: &Placement) -> Vec<Spot> {
    let Some(om) = fe.stack.last() else { return Vec::new() };
    (0..om.menu.items.len())
        .filter_map(|i| {
            let item = &om.menu.items[i];
            if !interactive(item) || !fe.item_visible(om, i) {
                return None;
            }
            let (pos, size) = pl.rect(&fe.item_rect(item));
            if size.x < 2.0 || size.y < 2.0 || size.x * size.y > pl.w * pl.h * 0.4 {
                return None;
            }
            let centre = pos + size * 0.5;
            let on_top = fe.hit(pl, centre).is_some_and(|(_, j)| j == i);
            on_top.then_some(Spot { item: i, centre })
        })
        .collect()
}

/// The first item: top-most, then left-most.
pub(super) fn first(spots: &[Spot]) -> Option<Spot> {
    spots.iter().copied().min_by(|a, b| (a.centre.y, a.centre.x).partial_cmp(&(b.centre.y, b.centre.x)).unwrap())
}

/// The nearest item from `from` towards `dir` (screen space, y down),
/// favouring ones in line; nothing well off to the side. Up and down wrap
/// around.
pub(super) fn step(spots: &[Spot], from: Spot, dir: IVec2) -> Option<Spot> {
    let d = dir.as_vec2();
    let scored = |ahead: bool| {
        spots
            .iter()
            .filter(|s| s.item != from.item)
            .filter_map(|s| {
                let delta = s.centre - from.centre;
                let along = delta.dot(d);
                let across = (delta - d * along).length();
                let ok = if ahead { along > 2.0 } else { along < -2.0 } && across < along.abs() * 2.0;
                ok.then_some((along + across * 2.5, *s))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|s| s.1)
    };
    scored(true).or_else(|| if dir.y != 0 { scored(false) } else { None })
}

fn navigate(
    time: Res<Time>,
    mut fe: ResMut<Frontend>,
    active: Res<ActiveDevice>,
    frame: Res<PadFrame>,
    state: Res<State<GameState>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut last_top: Local<Option<String>>,
    mut left_from: Local<std::collections::HashMap<String, usize>>,
    mut pending: Local<std::collections::HashSet<String>>,
) {
    let pad = active.pad.is_some();
    if fe.pad != pad {
        fe.pad = pad;
    }
    let Some(top) = fe.stack.last().map(|m| m.name.clone()) else {
        *last_top = None;
        return;
    };
    let opened = last_top.as_deref() != Some(top.as_str());
    *last_top = Some(top.clone());
    if fe.editing.is_some() {
        // Typing on a pad: the D-pad picks letters (`Frontend::pad_edit`); A
        // accepts and B cancels, arriving as Enter and Escape.
        if let (true, Some(dir)) = (pad, frame.menu_dir) {
            fe.pad_edit(dir);
        }
        return;
    }
    if !pad || *state.get() == GameState::Loading {
        return;
    }
    let pl = Placement::new(window.width(), window.height());
    let spots = spots(&fe, &pl);
    let current = fe.focus.as_ref().filter(|(m, _)| *m == top).and_then(|(_, i)| spots.iter().find(|s| s.item == *i).copied());
    if let Some(cur) = current {
        left_from.insert(top.clone(), cur.item);
    }
    let remembered = left_from.get(&top).and_then(|i| spots.iter().find(|s| s.item == *i).copied());
    // Left and right on a setting change it, as on CoD4's consoles.
    if let (Some(cur), Some(dir)) = (current, frame.menu_dir) {
        if dir.y == 0 && dir.x != 0 {
            let item = fe.stack.last().map(|om| om.menu.items[cur.item].clone());
            if let Some(item) = item.filter(|it| Frontend::setting_of(it).is_some() || super::custom_camo::is_row(it)) {
                if fe.setting_step(&item, dir.x.signum()) || fe.camo_row_step(&item, dir.x.signum()) {
                    fe.run("\"play\" \"mouse_click\"", "");
                }
                return;
            }
        }
    }
    // A menu just opened (or not yet laid out the frame it opened: a popup
    // over another isn't on top of the hit test at once) gets its focus as
    // soon as it has items to focus: where it was left, else a popup's "No"
    // (the safe answer), else the first.
    if opened {
        pending.insert(top.clone());
    }
    // (A menu that set its own focus on opening keeps it.)
    let preset = fe.focus.as_ref().and_then(|(m, i)| (*m == top).then_some(*i));
    // (The cursor goes there too, or hovering where it was takes focus.)
    if let Some(i) = preset.filter(|_| pending.contains(&top)) {
        if let Some(s) = spots.iter().find(|s| s.item == i) {
            window.set_cursor_position(Some(s.centre));
            pending.remove(&top);
        }
        return;
    }
    let wants = current.is_none() && pending.contains(&top);
    let safe = || {
        let om = fe.stack.last()?;
        spots.iter().copied().find(|s| {
            let it = &om.menu.items[s.item];
            let label = match it.text_exp.as_slice() {
                [_, iw3::menu::Token::Str(t)] => t.clone(),
                _ => it.text.clone(),
            };
            label.eq_ignore_ascii_case("@MENU_NO")
        })
    };
    if current.is_some() || !spots.is_empty() {
        pending.remove(&top);
    }
    let target = match (current, frame.menu_dir) {
        (None, _) if wants => remembered.or_else(safe).or_else(|| first(&spots)),
        (None, Some(_)) => first(&spots),
        (Some(from), Some(dir)) => step(&spots, from, dir),
        _ => None,
    };
    if let Some(to) = target {
        if let Some(om) = fe.stack.last() {
            let item = &om.menu.items[to.item];
            debug!("pad: focus {top} item {} {:?} {:?}", to.item, item.window.name, item.text);
        }
        window.set_cursor_position(Some(to.centre));
        fe.set_focus(Some((top, to.item)));
    }
    // The right stick turns the preview on top, like a mouse drag.
    if frame.menu_rotate != Vec2::ZERO {
        fe.previews.turn_top(frame.menu_rotate * Vec2::new(1.0, -1.0) * ROTATE_SPEED * time.delta_secs());
    }
}

impl Frontend {
    /// A text field is being typed into.
    pub fn typing(&self) -> bool {
        self.editing.is_some()
    }
}

/// The menus' button footer, and its "rotate" part.
#[derive(Component)]
struct MenuFooter;

#[derive(Component)]
struct RotatePrompt;

/// "Change" (left and right step a setting): the Camo Editor's rows.
#[derive(Component)]
struct ChangePrompt;

/// Spawned once and kept: the menus come and go around it.
fn spawn_footer(mut commands: Commands) {
    let footer = commands
        .spawn((
            MenuFooter,
            Node {
                position_type: PositionType::Absolute,
                right: px(32),
                bottom: px(20),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(22),
                ..default()
            },
            GlobalZIndex(PROMPT_Z),
            Visibility::Hidden,
        ))
        .id();
    commands.spawn((ChangePrompt, PadPrompt::row(vec![glyph(Glyph::DPadLeft), glyph(Glyph::DPadRight), text("Change")], 30.0), ChildOf(footer)));
    commands.spawn((RotatePrompt, PadPrompt::row(vec![glyph(Glyph::RightStick), text("Rotate")], 30.0), ChildOf(footer)));
    commands.spawn((PadPrompt::row(vec![glyph(Glyph::South), text("Select")], 30.0), ChildOf(footer)));
    commands.spawn((PadPrompt::row(vec![glyph(Glyph::East), text("Back")], 30.0), ChildOf(footer)));
}

/// The footer shows with menus up and the pad in use; "rotate" while a gun
/// preview is on show.
fn show_footer(
    fe: Option<Res<Frontend>>,
    active: Res<ActiveDevice>,
    state: Res<State<GameState>>,
    mut footer: Query<&mut Visibility, (With<MenuFooter>, Without<RotatePrompt>, Without<ChangePrompt>)>,
    mut rotate: Query<&mut Visibility, (With<RotatePrompt>, Without<MenuFooter>, Without<ChangePrompt>)>,
    mut change: Query<&mut Visibility, (With<ChangePrompt>, Without<MenuFooter>, Without<RotatePrompt>)>,
) {
    let menus = fe.as_deref().is_some_and(|fe| !fe.stack.is_empty() && !fe.match_only) && *state.get() != GameState::Loading;
    // The new UI's screens draw their own button prompts.
    let own_prompts = fe.as_deref().is_some_and(|fe| super::next::home::on_top(fe) || super::next::cac::on_top(fe) || super::next::picker::on_top(fe) || super::next::camo_edit::on_top(fe) || super::next::color::on_top(fe) || super::next::character::on_top(fe) || super::next::lobby::on_top(fe) || super::next::maps::on_top(fe) || super::next::modes::on_top(fe) || super::next::popup::on_top(fe) || super::next::drops::on_top(fe) || super::next::settings::on_top(fe) || super::next::match_menus::on_top(fe) || super::next::profiles::on_top(fe));
    let menus = menus && !own_prompts;
    let show = |on: bool| if on { Visibility::Inherited } else { Visibility::Hidden };
    for mut v in &mut footer {
        v.set_if_neq(show(menus && active.pad.is_some()));
    }
    let preview = fe.as_deref().is_some_and(|fe| fe.previews.first_rect().is_some());
    for mut v in &mut rotate {
        v.set_if_neq(show(preview));
    }
    let editor = fe.as_deref().is_some_and(|fe| fe.stack.last().is_some_and(|m| m.name == super::custom_camo::EDITOR));
    for mut v in &mut change {
        v.set_if_neq(show(editor));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(item: usize, x: f32, y: f32) -> Spot {
        Spot { item, centre: Vec2::new(x, y) }
    }

    #[test]
    fn steps_to_the_nearest_item_in_line() {
        // A column of buttons and a panel to the right.
        let spots = [spot(0, 100.0, 100.0), spot(1, 100.0, 140.0), spot(2, 100.0, 180.0), spot(3, 400.0, 150.0)];
        assert_eq!(step(&spots, spots[0], IVec2::new(0, 1)).map(|s| s.item), Some(1));
        assert_eq!(step(&spots, spots[1], IVec2::new(0, -1)).map(|s| s.item), Some(0));
        assert_eq!(step(&spots, spots[1], IVec2::new(1, 0)).map(|s| s.item), Some(3));
        // Nothing level with it to the right of the panel.
        assert_eq!(step(&spots, spots[3], IVec2::new(1, 0)).map(|s| s.item), None);
        // Down from the bottom wraps to the top; left from the column stays.
        assert_eq!(step(&spots, spots[2], IVec2::new(0, 1)).map(|s| s.item), Some(0));
        assert_eq!(step(&spots, spots[0], IVec2::new(-1, 0)).map(|s| s.item), None);
        assert_eq!(first(&spots).map(|s| s.item), Some(0));
    }
}
