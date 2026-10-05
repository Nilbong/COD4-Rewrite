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
        // A pad can't type: A or B leaves the field, as a click elsewhere does.
        if frame.menu_done {
            fe.editing = None;
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
    let target = match (current, frame.menu_dir) {
        (None, _) if opened => remembered.or_else(|| first(&spots)),
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
    mut footer: Query<&mut Visibility, (With<MenuFooter>, Without<RotatePrompt>)>,
    mut rotate: Query<&mut Visibility, (With<RotatePrompt>, Without<MenuFooter>)>,
) {
    let menus = fe.as_deref().is_some_and(|fe| !fe.stack.is_empty() && !fe.match_only) && *state.get() != GameState::Loading;
    let show = |on: bool| if on { Visibility::Inherited } else { Visibility::Hidden };
    for mut v in &mut footer {
        v.set_if_neq(show(menus && active.pad.is_some()));
    }
    let preview = fe.as_deref().is_some_and(|fe| fe.previews.first_rect().is_some());
    for mut v in &mut rotate {
        v.set_if_neq(show(preview));
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
