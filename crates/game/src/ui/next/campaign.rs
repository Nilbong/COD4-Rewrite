//! The campaign's main menu (CoD4's single-player `sp:main_text`) in the new
//! UI's look: a column of sections and rows beside a preview of the focused
//! row, as the multiplayer main menu ([`super::home`]).
//!
//! Its rows are the classic menu's own buttons, found by their labels and
//! moved to the column's places with their text cleared: every action and
//! visibility rule (Resume Game only with a save, Arcade Mode once
//! unlocked, the two New Game buttons) stays the classic menu's.

use super::super::expr::{Env, eval};
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Menu, Statement, Window as MenuWindow, flags, item_type};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

/// The menu that replaces `sp:main_text`.
pub const MENU: &str = "sp_modern";
/// The classic menu it stands in for.
pub const CLASSIC: &str = "sp:main_text";

/// The sections and their rows (labels as the classic buttons have them).
const SECTIONS: [(&str, &[&str]); 3] = [
    ("Campaign", &["Resume Game", "New Game", "Mission Select", "Arcade Mode", "Credits"]),
    ("Settings", &["Select Profile", "Options", "Controls"]),
    ("Leave", &["Multiplayer", "Quit"]),
];

fn rows() -> impl Iterator<Item = (usize, &'static str)> {
    SECTIONS.iter().enumerate().flat_map(|(s, (_, rows))| rows.iter().map(move |r| (s, *r)))
}

const ROW_H: f32 = 48.0;
const ROW_PITCH: f32 = 50.0;
const TOP: f32 = 176.0;

/// The column's layout for the rows present (by index in [`rows`]).
fn layout(present: &[usize]) -> (Vec<(usize, f32)>, Vec<(usize, R)>) {
    let (x, w) = (col_x(0), cols(4));
    let (mut headers, mut out) = (Vec::new(), Vec::new());
    let (mut y, mut i) = (TOP, 0);
    for (s, (_, list)) in SECTIONS.iter().enumerate() {
        let here: Vec<usize> = (i..i + list.len()).filter(|k| present.contains(k)).collect();
        i += list.len();
        if here.is_empty() {
            continue;
        }
        headers.push((s, y));
        y += 34.0;
        for k in here {
            out.push((k, R::new(x, y, w, ROW_H)));
            y += ROW_PITCH;
        }
        y += 22.0;
    }
    (headers, out)
}

fn item_name(k: usize) -> String {
    format!("sp_modern_{k}")
}

/// The rows present now: those with a classic button showing (a hidden
/// Resume Game or Arcade Mode takes no place).
fn showing(fe: &Frontend, om: &OpenMenu) -> Vec<usize> {
    let all: Vec<&str> = rows().map(|(_, r)| r).collect();
    (0..all.len()).filter(|&k| om.menu.items.iter().enumerate().any(|(n, it)| it.window.name == item_name(k) && fe.item_visible(om, n))).collect()
}

/// The new menu built from the classic one; `None` if none of its buttons
/// are found.
pub(in crate::ui) fn menu(fe: &Frontend, classic: &Menu) -> Option<Menu> {
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = MENU.into();
    let all: Vec<&str> = rows().map(|(_, r)| r).collect();
    let mut found = Vec::new();
    for it in classic.items.iter().filter(|it| it.ty == item_type::BUTTON) {
        let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, fe).text() };
        let label = fe.assets.localize(&raw);
        let Some(k) = all.iter().position(|r| r.eq_ignore_ascii_case(label.trim())) else {
            debug!("campaign menu: no row for {label:?}");
            continue;
        };
        // Showing now by the classic rules (Resume Game with a save, Arcade
        // Mode unlocked): only those take a place in the column.
        let shows = it.window.dynamic_flags & flags::VISIBLE != 0 && (it.visible_exp.is_empty() || eval(&it.visible_exp, fe).truthy()) && fe.dvar_test(it, 4 | 8);
        found.push((k, shows, it.clone()));
    }
    if found.is_empty() {
        return None;
    }
    let mut present: Vec<usize> = found.iter().filter(|f| f.1).map(|f| f.0).collect();
    present.dedup();
    let (_, rects) = layout(&present);
    for (k, _, mut item) in found {
        // (A row hidden now keeps its own rules; it sits on its row's place.)
        let r = rects.iter().find(|(i, _)| *i == k).map_or(R::new(col_x(0), TOP, cols(4), ROW_H), |(_, r)| *r);
        item.window = MenuWindow { rect: virtual_rect(r), dynamic_flags: item.window.dynamic_flags, name: item_name(k), ..MenuWindow::default() };
        item.text = all[k].into();
        item.text_exp = Statement::default();
        item.text_scale = 0.0;
        out.items.push(item);
    }
    Some(out)
}

/// A menu item's rect back in design units (the inverse of [`virtual_rect`]).
fn design_rect(v: &iw3::menu::Rect) -> R {
    let k = 1080.0 / 480.0;
    R::new(v.x * k + 960.0, v.y * k, v.w * k, v.h * k)
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    let Some(at) = fe.stack.iter().position(|m| m.menu.window.name == MENU) else { return false };
    !fe.stack[at + 1..].iter().any(|m| m.menu.visible_exp.is_empty() || eval(&m.menu.visible_exp, fe).truthy())
}

static OPENED_AT: AtomicI64 = AtomicI64::new(i64::MIN);
static SHOWN: AtomicUsize = AtomicUsize::new(usize::MAX);

pub(in crate::ui) fn opened(fe: &Frontend) {
    OPENED_AT.store(fe.millis(), Ordering::Relaxed);
}

/// A row's preview: over-line and description.
fn preview(row: &str) -> (&'static str, &'static str) {
    match row {
        "Resume Game" => ("Continue", "Pick up from your last checkpoint."),
        "New Game" => ("Story", "Call of Duty 4's campaign from the start: F.N.G., then Crew Expendable."),
        "Mission Select" => ("Story", "Play any mission you've reached, act by act."),
        "Arcade Mode" => ("Score attack", "The whole campaign for points, against the clock, with limited lives."),
        "Credits" => ("Story", "The people who made Call of Duty 4."),
        "Select Profile" => ("Profile", "Switch between player profiles."),
        "Options" => ("System", "Graphics, sound and game settings."),
        "Controls" => ("System", "Keyboard, mouse and controller bindings."),
        "Multiplayer" => ("Leave", "Back to multiplayer."),
        _ => ("Leave", "Leave the game."),
    }
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    let since = (fe.millis() - OPENED_AT.load(Ordering::Relaxed)).max(0) as f32 / 1000.0;
    let ease = t::ease_out(since / t::SLOW);
    let covered = fe.stack.iter().skip_while(|m| !std::ptr::eq(*m, om)).skip(1).any(|m| m.menu.visible_exp.is_empty() || eval(&m.menu.visible_exp, fe).truthy());
    backdrop(&mut k, 0.05);
    if covered {
        return;
    }
    k.alpha = ease;
    k.pic(R::new(t::SAFE_X, 40.0, 300.0, 300.0 * 383.0 / 1384.0), "next:logo", [1.0; 4]);
    k.text(t::SAFE_X + 320.0, 74.0, t::LABEL, "Campaign", t::ACCENT, 0);

    let all: Vec<&str> = rows().map(|(_, r)| r).collect();
    let present = showing(fe, om);
    let focused_item = fe.focus.as_ref().filter(|(m, _)| *m == om.name).map(|(_, i)| *i);
    let focused = present.iter().copied().find(|&k| om.menu.items.get(focused_item.unwrap_or(usize::MAX)).is_some_and(|it| it.window.name == item_name(k)));
    let forced = super::mock_focus().and_then(|f| all.iter().position(|r| *r == f));
    if let Some(f) = forced.or(focused) {
        SHOWN.store(f, Ordering::Relaxed);
    }
    let shown = Some(SHOWN.load(Ordering::Relaxed)).filter(|s| present.contains(s)).or(present.first().copied());

    k.shift = Vec2::new((1.0 - ease) * -24.0, 0.0);
    // Each row where its item is (placed when the menu opened), each
    // section's header over its first row.
    let rects: Vec<(usize, R)> = present
        .iter()
        .filter_map(|&k| om.menu.items.iter().find(|it| it.window.name == item_name(k)).map(|it| (k, design_rect(&it.window.rect))))
        .collect();
    let mut last = usize::MAX;
    for &(i, r) in &rects {
        let s = rows().nth(i).map_or(0, |(s, _)| s);
        if s != last {
            section(&mut k, col_x(0), r.y - 34.0, cols(4), SECTIONS[s].0);
            last = s;
        }
    }
    for (i, r) in rects {
        let hot = forced.or(focused) == Some(i);
        menu_row(&mut k, r, all[i], hot, None);
        use super::kit::icons::*;
        let icon = match all[i] {
            "Resume Game" | "New Game" | "Mission Select" | "Arcade Mode" => Some(CROSSHAIR),
            "Credits" => Some(STAR),
            "Select Profile" => Some(STAR),
            "Options" => Some(COG),
            "Controls" => Some(PAD),
            "Multiplayer" => Some(PEOPLE),
            _ => None,
        };
        if let Some(ic) = icon {
            let x = r.right() - if hot { 78.0 } else { 46.0 };
            k.icon(super::kit::MENU_ICONS, ic, R::new(x, r.cy() - 12.0, 24.0, 24.0), if hot { t::INK } else { t::with_alpha(t::INK_DIM, 0.6) });
        }
    }

    k.shift = Vec2::new((1.0 - ease) * 24.0, 0.0);
    if let Some(s) = shown {
        let label = all[s];
        let (over, about) = preview(label);
        let art = R::new(col_x(5), TOP, cols(7), 620.0);
        k.fill(art, t::RAISED);
        k.pic_uv(art, "next:preview_campaign", crop(art, 2.0), [1.0; 4]);
        k.grad_v(R::new(art.x, art.y + art.h * 0.3, art.w, art.h * 0.7), t::hex(0x050607, 1.0), 0.0, 0.95);
        k.grad_h(R::new(art.x, art.y, art.w * 0.6, art.h), t::hex(0x050607, 1.0), 0.5, 0.0);
        k.slice9(art.inset(-10.0), "next:panels/frame_big", Vec2::new(569.0, 256.0), 84.0, 1.0, [1.0, 1.0, 1.0, 0.95]);
        k.text(art.x + 36.0, art.bottom() - 178.0, t::LABEL, over, t::ACCENT, 0);
        k.text(art.x + 36.0, art.bottom() - 150.0, Type { size: 72.0, ..t::DISPLAY }, label, t::INK, 0);
        k.para(art.x + 36.0, art.bottom() - 64.0, art.w - 72.0, t::BODY, about, t::INK_DIM, 1.3);
    }
    k.shift = Vec2::ZERO;
    footer(&mut k, "", &[(Button::Confirm, "Select")]);
}
