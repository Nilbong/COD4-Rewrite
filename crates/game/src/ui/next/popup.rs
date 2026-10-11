//! The new UI's dialogs: CoD4's small centred popups ("Are you sure you
//! want to quit?", leaving a game, resetting settings, errors) shown as a
//! panel over the dimmed screen, the question on top and the choices as
//! rows under it. The popup's own buttons are kept and moved.

use super::super::expr::{Env, eval};
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Statement, Window as MenuWindow, flags, item_type};

pub const PREFIX: &str = "next_popup|";

const W: f32 = 760.0;
const ROW_H: f32 = 52.0;
const PITCH: f32 = 58.0;

fn centred(it: &Item) -> bool {
    it.window.rect.horz_align == 2 && it.window.rect.vert_align == 2
}

fn choice(it: &Item) -> bool {
    it.ty == item_type::BUTTON && !it.action.replace(';', "").trim().is_empty() && it.window.rect.w > 20.0 && it.window.rect.w < 500.0
}

/// Whether `menu` is a small centred popup the dialog stands in for.
pub(in crate::ui) fn wanted(menu: &Menu) -> bool {
    if menu.full_screen || menu.window.name.starts_with("next_") {
        return false;
    }
    let buttons: Vec<&Item> = menu.items.iter().filter(|it| choice(it)).collect();
    if buttons.is_empty() || buttons.len() > 5 {
        return false;
    }
    // Not a screen of its own: no lists, sliders, choices or owner draws.
    let screen = menu.items.iter().any(|it| matches!(it.ty, item_type::LISTBOX | item_type::OWNERDRAW | item_type::SLIDER | item_type::YESNO | item_type::MULTI | item_type::DVARENUM | item_type::BIND));
    // (Nor an info card built from many texts: CoD4's challenge tiers.)
    let texts = menu.items.iter().filter(|it| it.ty == item_type::TEXT && (!it.text.is_empty() || !it.text_exp.is_empty())).count();
    if screen || texts > 4 {
        return false;
    }
    // Centred buttons (most popups), or any placed close together (the
    // restart and confirm popups anchor theirs to a corner).
    if buttons.iter().all(|it| centred(it)) {
        return true;
    }
    let same_anchor = buttons.iter().all(|it| (it.window.rect.horz_align, it.window.rect.vert_align) == (buttons[0].window.rect.horz_align, buttons[0].window.rect.vert_align));
    let (x0, x1) = buttons.iter().fold((f32::MAX, f32::MIN), |(a, b), it| (a.min(it.window.rect.x), b.max(it.window.rect.x + it.window.rect.w)));
    let (y0, y1) = buttons.iter().fold((f32::MAX, f32::MIN), |(a, b), it| (a.min(it.window.rect.y), b.max(it.window.rect.y + it.window.rect.h)));
    same_anchor && x1 - x0 < 480.0 && y1 - y0 < 200.0
}

fn text_of(fe: &Frontend, it: &Item) -> String {
    let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, fe).text() };
    fe.assets.localize(&raw).trim().to_owned()
}

/// Debug aid (`COD4RW_UIDUMP`, once): the popups the dialog doesn't take,
/// and why.
pub(in crate::ui) fn dump_rejected(fe: &Frontend) {
    let mut names: Vec<&String> = fe.assets.menus.keys().collect();
    names.sort();
    for name in names {
        let m = &fe.assets.menus[name];
        if !m.full_screen && wanted(m) && !m.items.iter().filter(|it| choice(it)).all(|it| centred(it)) {
            let labels: Vec<String> = m.items.iter().filter(|it| choice(it)).map(|it| text_of(fe, it)).collect();
            info!("UIDUMP popup+ {name}: {}", labels.join(" | "));
        }
        if m.full_screen || wanted(m) {
            continue;
        }
        let buttons: Vec<&Item> = m.items.iter().filter(|it| choice(it)).collect();
        if buttons.is_empty() {
            continue;
        }
        let labels: Vec<String> = buttons.iter().map(|it| format!("{}[{}/{} {:.0}x{:.0}]", text_of(fe, it), it.window.rect.horz_align, it.window.rect.vert_align, it.window.rect.w, it.window.rect.h)).collect();
        let other: Vec<i32> = m.items.iter().filter(|it| it.ty != item_type::BUTTON && it.ty != item_type::TEXT).map(|it| it.ty).collect();
        info!("UIDUMP popup? {name} ({} buttons, other types {other:?}): {}", buttons.len(), labels.join(" | "));
    }
}

/// The panel for `n` choices and `lines` lines of message.
/// The message's height (none without one).
fn message_h(lines: usize) -> f32 {
    if lines == 0 { 0.0 } else { lines as f32 * 38.0 + 30.0 }
}

fn panel(n: usize, lines: usize) -> R {
    let h = 44.0 + message_h(lines) + n as f32 * PITCH + 30.0;
    R::new(960.0 - W * 0.5, 540.0 - h * 0.5, W, h)
}

fn row_rect(p: R, lines: usize, i: usize) -> R {
    R::new(p.x + 40.0, p.y + 44.0 + message_h(lines) + i as f32 * PITCH, p.w - 80.0, ROW_H)
}

/// The message's lines (wrapped later); here, how many it'll take, roughly.
fn line_count(s: &str) -> usize {
    if s.trim().is_empty() {
        return 0;
    }
    (s.len() / 46 + 1).clamp(1, 4)
}

pub(in crate::ui) fn menu(fe: &Frontend, classic: &Menu) -> Menu {
    // The message: the popup's texts that do nothing.
    let message: Vec<String> = classic
        .items
        .iter()
        .filter(|it| !choice(it) && it.ty != item_type::EDITFIELD && (!it.text.is_empty() || !it.text_exp.is_empty()))
        .map(|it| text_of(fe, it))
        .filter(|s| !s.is_empty())
        .collect();
    // (CoD4 asks "quit?" when leaving a match too.)
    let message = if classic.window.name.eq_ignore_ascii_case("popup_leavegame") { "Leave this match?".to_owned() } else { message.join(" ") };
    let lines = line_count(&message);
    let buttons: Vec<&Item> = classic.items.iter().filter(|it| choice(it)).collect();
    // Text fields (a new profile's name) first, kept by name (`setfocus`).
    let fields: Vec<&Item> = classic.items.iter().filter(|it| it.ty == item_type::EDITFIELD).collect();
    let p = panel(fields.len() + buttons.len(), lines);
    let mut out = Menu { items: Vec::new(), ..classic.clone() };
    out.window.name = format!("{PREFIX}{}", classic.window.name);
    for (i, it) in fields.iter().enumerate() {
        let mut item = (*it).clone();
        item.text = text_of(fe, it).trim_end_matches(':').trim().to_owned();
        item.text_exp = Statement::default();
        item.window = MenuWindow { rect: virtual_rect(row_rect(p, lines, i)), dynamic_flags: flags::VISIBLE, name: it.window.name.clone(), ..MenuWindow::default() };
        item.text_scale = 0.0;
        out.items.push(item);
    }
    for (i, it) in buttons.iter().enumerate() {
        let i = i + fields.len();
        let mut item = (*it).clone();
        item.text = text_of(fe, it);
        item.text_exp = Statement::default();
        item.window = MenuWindow { rect: virtual_rect(row_rect(p, lines, i)), dynamic_flags: flags::VISIBLE, name: format!("popup_{i}"), ..MenuWindow::default() };
        item.text_scale = 0.0;
        out.items.push(item);
    }
    // The message, kept as a text item (painted here, read by the harness).
    let mut msg = Item { text: message, ..Item::default() };
    msg.window.name = "popup_message".into();
    out.items.push(msg);
    out
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name.starts_with(PREFIX))
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if !om.menu.window.name.starts_with(PREFIX) {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    let b = k.v.bleed();
    k.fill(b, t::hex(0x020303, 0.82));
    let message = om.menu.items.iter().find(|it| it.window.name == "popup_message").map(|it| it.text.clone()).unwrap_or_default();
    let fields: Vec<(usize, &Item)> = om.menu.items.iter().enumerate().filter(|(_, it)| it.ty == item_type::EDITFIELD).collect();
    let n = fields.len() + om.menu.items.iter().filter(|it| it.window.name.starts_with("popup_") && it.window.name != "popup_message").count();
    let lines = line_count(&message);
    let p = panel(n, lines);
    grid_panel(&mut k, p, 70.0, 1.0);
    k.fill(R::new(p.x, p.y, p.w, 4.0), t::ACCENT);
    k.para(p.x + 40.0, p.y + 44.0, p.w - 80.0, Type { size: 30.0, ..t::H2 }, &message, t::INK, 1.15);
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.clone());
    // A field: its label, and a box with what's typed (a caret while typing).
    for (row, &(i, it)) in fields.iter().enumerate() {
        let r = row_rect(p, lines, row);
        let typing = fe.editing.as_ref().is_some_and(|(m, f)| *m == om.name && *f == i);
        let on = typing || hot.as_deref() == Some(it.window.name.as_str());
        let lw = k.text_mid(r.x, r.cy(), Type { size: 24.0, ..t::ROW_TEXT }, &it.text, t::INK_DIM, 0);
        let b = R::new(r.x + lw + 20.0, r.y + 4.0, r.right() - r.x - lw - 20.0, r.h - 8.0);
        k.fill(b, t::hex(0x000000, 0.5));
        k.frame(b, 1.0, if on { t::ACCENT } else { t::LINE });
        let mut value = fe.dvar(&it.dvar);
        if typing && (fe.millis() / 400) % 2 == 0 {
            value.push('_');
        }
        k.text_mid(b.x + 14.0, b.cy(), Type { size: 24.0, ..t::ROW_TEXT }, &value, t::INK, 0);
    }
    for (i, it) in om.menu.items.iter().filter(|it| it.window.name.starts_with("popup_") && it.window.name != "popup_message").enumerate() {
        let r = row_rect(p, lines, i + fields.len());
        menu_row(&mut k, r, &it.text, hot.as_deref() == Some(it.window.name.as_str()), None);
    }
    // The prompts, under the panel.
    k.prompts_right(p.right(), p.bottom() + 40.0, &[(Button::Confirm, "Select"), (Button::Back, "Cancel")]);
}
