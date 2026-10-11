//! The new Character screen: the characters collected in a column, grouped
//! by game, each with its rarity's colour and a tick on the one worn; on the
//! right the one under the cursor (or worn) standing in 3D, with its name,
//! rarity and game, and the collection's progress.
//!
//! The rows are the classic screen's own (`uiScript supplyWear`, the
//! "More" page row), moved; the drawing is this file's.

use super::super::expr::Env;
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View, icons};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use crate::characters::{self, Character};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Statement, Window as MenuWindow, flags, item_type};
use std::sync::atomic::{AtomicI64, Ordering};

pub const MENU: &str = "next_character";
/// The classic screen's (`supply.rs`) highlight dvar and figure preview.
const HIGHLIGHT: &str = "ui_character_highlighted";
const FIGURE_KEY: i32 = 1010;

static OPENED: AtomicI64 = AtomicI64::new(i64::MIN);

const TOP: f32 = 192.0;
const ROW_H: f32 = 44.0;
const PITCH: f32 = 48.0;

/// The character a row wears.
fn wears(it: &Item) -> Option<&'static Character> {
    let id = it.action.split("\"supplyWear\"").nth(1)?.split('"').nth(1)?;
    characters::character(id)
}

/// The column: a header (game) or a row (index into `rows`) at each y.
enum Line {
    Header(f32),
    Row(usize, R),
}

fn layout(rows: &[Option<&'static Character>]) -> Vec<Line> {
    let mut out = vec![Line::Header(TOP)];
    let mut y = TOP + 34.0;
    for (i, c) in rows.iter().enumerate() {
        if c.is_none() {
            y += 12.0;
        }
        out.push(Line::Row(i, R::new(col_x(0), y, cols(4), ROW_H)));
        y += PITCH;
    }
    out
}

/// The classic screen's buttons, in order (characters, then "More").
fn buttons(menu: &Menu) -> Vec<&Item> {
    menu.items.iter().filter(|it| it.ty == item_type::BUTTON && (wears(it).is_some() || it.action.contains("supplyCharPage"))).collect()
}

pub(in crate::ui) fn menu(classic: &Menu) -> Option<Menu> {
    let list = buttons(classic);
    if list.is_empty() {
        return None;
    }
    let rows: Vec<Option<&'static Character>> = list.iter().map(|it| wears(it)).collect();
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = MENU.into();
    for line in layout(&rows) {
        let Line::Row(i, r) = line else { continue };
        let mut item = list[i].clone();
        item.window = MenuWindow { rect: virtual_rect(r), dynamic_flags: flags::VISIBLE, name: format!("char_{i}"), ..MenuWindow::default() };
        item.text = rows[i].map_or_else(|| "More".to_owned(), |c| c.name.to_owned());
        item.text_exp = Statement::default();
        item.text_scale = 0.0;
        out.items.push(item);
    }
    let template = out.items[0].clone();
    out.items.push(cac::back_item(&template, &classic.on_esc));
    Some(out)
}

pub(in crate::ui) fn opened(fe: &Frontend) {
    OPENED.store(fe.millis(), Ordering::Relaxed);
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    cac::loadout_backdrop(&mut k);
    let since = (fe.millis() - OPENED.load(Ordering::Relaxed)).max(0) as f32 / 1000.0;
    let ease = t::ease_out(since / t::MEDIUM);
    k.alpha = ease;
    header(&mut k, "Loadout", "Character");
    let inv = crate::supply::inventory();
    let worn = inv.wearing();
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.clone());

    // The column.
    let items: Vec<&Item> = om.menu.items.iter().filter(|it| it.window.name.starts_with("char_")).collect();
    let rows: Vec<Option<&'static Character>> = items.iter().map(|it| wears(it)).collect();
    k.shift = Vec2::new((1.0 - ease) * -20.0, 0.0);
    for line in layout(&rows) {
        match line {
            Line::Header(y) => section_icon(&mut k, col_x(0), y, cols(4), "Collected", Some(icons::HELMET)),
            Line::Row(i, r) => {
                let focused = hot.as_deref() == Some(items[i].window.name.as_str());
                match rows[i] {
                    Some(c) => {
                        menu_row(&mut k, r, c.name, focused, None);
                        // Its rarity: a bar of its colour on the left, its
                        // name on the right.
                        k.fill(R::new(r.x, r.y + 8.0, 4.0, r.h - 16.0), c.rarity.color());
                        let right = r.right() - if focused { 52.0 } else { 20.0 };
                        let w = k.text_mid(right, r.cy(), t::MICRO, c.rarity.name(), t::with_alpha(c.rarity.color(), 0.9), 2);
                        let w = w + 14.0 + k.text_mid(right - w - 14.0, r.cy(), t::MICRO, c.game.short(), t::INK_MUTE, 2);
                        if c.id == worn.id {
                            k.check(Vec2::new(right - w - 26.0, r.cy()), 18.0, 2.5, t::ACCENT);
                        }
                    }
                    None => action_button(&mut k, r, &items[i].text, focused),
                }
            }
        }
    }

    // The one under the cursor (or worn), standing.
    k.shift = Vec2::new((1.0 - ease) * 20.0, 0.0);
    let shown = characters::character(&fe.dvar(HIGHLIGHT)).filter(|c| inv.owns_character(c.id)).unwrap_or(worn);
    let p = R::new(col_x(5), TOP, cols(7), 780.0);
    grid_panel(&mut k, p, 84.0, 1.0);
    // A glow of its rarity behind it.
    k.glow(R::new(p.cx() - 300.0, p.y + 40.0, 600.0, 560.0), t::with_alpha(shown.rarity.color(), 0.06));
    k.gun(R::new(p.x + 120.0, p.y + 24.0, p.w - 240.0, 560.0), FIGURE_KEY, &format!("char:{}", shown.id), 0, "");
    let x = p.x + 44.0;
    let y = p.bottom() - 170.0;
    let tw = k.text(x, y, t::LABEL, shown.rarity.name(), shown.rarity.color(), 0);
    k.text(x + tw + 18.0, y, t::LABEL, shown.game.name(), t::INK_DIM, 0);
    k.text(x, y + 26.0, Type { size: 56.0, ..t::DISPLAY }, shown.name, t::INK, 0);
    let state = if shown.id == worn.id { "Worn in matches" } else { "Select to wear" };
    k.text(x, y + 96.0, Type { size: 20.0, ..t::BODY }, state, if shown.id == worn.id { t::ACCENT } else { t::INK_DIM }, 0);
    // The collection.
    let (have, all) = (inv.characters_owned().len(), characters::CHARACTERS.len());
    let cx = p.right() - 344.0;
    k.text(cx, y + 60.0, t::MICRO, "Collected", t::INK_DIM, 0);
    k.text(p.right() - 44.0, y + 52.0, Type { size: 26.0, ..t::NUMERAL }, &format!("{have} / {all}"), t::INK, 2);
    let bar = R::new(cx, y + 96.0, 300.0, 6.0);
    k.fill(bar, t::hex(0xffffff, 0.12));
    k.fill(R::new(bar.x, bar.y, bar.w * have as f32 / all.max(1) as f32, bar.h), t::ACCENT);
    k.shift = Vec2::ZERO;
    cac::back_button(&mut k, hot.as_deref() == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Wear"), (Button::Back, "Back")]);
}
