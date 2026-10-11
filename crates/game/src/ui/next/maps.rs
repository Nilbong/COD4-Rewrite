//! The new map choice for the Private Match lobby: every map as a tile with
//! its loading screen picture and name, in a grid the D-pad moves around.
//! The classic list's own rows (`uiScript lobbyMap N`) are kept and moved.

use super::super::expr::{Env, eval};
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type};
use bevy::prelude::*;
use iw3::menu::{Menu, Window as MenuWindow, flags, item_type};

pub const MENU: &str = "next_maps";

const TOP: f32 = 192.0;
const GRID_TOP: f32 = 256.0;
const BOTTOM: f32 = 960.0;
const LEFT: f32 = t::SAFE_X;
const WIDTH: f32 = 1920.0 - 2.0 * t::SAFE_X;
const GAP: f32 = 16.0;

/// Tile `i` of `n`: six across, rows as many as fit, 16:9 pictures.
fn tile(i: usize, n: usize) -> R {
    let across = if n > 24 { 7 } else { 6 };
    let rows = n.div_ceil(across).max(1);
    let w = (WIDTH - (across - 1) as f32 * GAP) / across as f32;
    let h = (w * 9.0 / 16.0).min((BOTTOM - GRID_TOP - (rows - 1) as f32 * GAP) / rows as f32);
    let (c, r) = (i % across, i / across);
    R::new(LEFT + c as f32 * (w + GAP), GRID_TOP + r as f32 * (h + GAP), w, h)
}

fn map_of(action: &str) -> Option<usize> {
    let rest = action.split("lobbyMap").nth(1)?;
    let digits: String = rest.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The tabs: CoD4's maps, the install's custom maps, Modern Warfare 2's.
const TABS: [&str; 3] = ["Call of Duty 4", "Custom", "Modern Warfare 2"];
const TAB_DVAR: &str = "ui_next_maptab";

fn tab_of(i: usize) -> usize {
    let maps = super::super::lobby::maps();
    if i < super::super::lobby::MAPS.len() {
        0
    } else if maps.get(i).is_some_and(|m| m.1.starts_with("MW2")) {
        2
    } else {
        1
    }
}

/// The tab buttons' places.
fn tab_rect(i: usize) -> R {
    R::new(LEFT + i as f32 * 300.0, TOP - 6.0, 284.0, 44.0)
}

pub(in crate::ui) fn menu(fe: &Frontend, classic: &Menu) -> Option<Menu> {
    let all: Vec<_> = classic.items.iter().filter(|it| it.ty == item_type::BUTTON && map_of(&it.action).is_some()).collect();
    if all.is_empty() {
        return None;
    }
    // The tab shown: the one asked for, else the current map's.
    let current = super::super::lobby::maps().iter().position(|m| m.0 == fe.lobby_view().map.0).unwrap_or(0);
    let tabs: Vec<usize> = (0..TABS.len()).filter(|&t| all.iter().any(|it| tab_of(map_of(&it.action).unwrap_or(0)) == t)).collect();
    let tab = fe.dvar(TAB_DVAR).parse::<usize>().ok().filter(|t| tabs.contains(t)).unwrap_or(tab_of(current));
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = format!("{MENU}|{tab}");
    let rows: Vec<_> = all.into_iter().filter(|it| tab_of(map_of(&it.action).unwrap_or(0)) == tab).collect();
    let n = rows.len();
    for (k, it) in rows.into_iter().enumerate() {
        let i = map_of(&it.action).unwrap_or(0);
        let mut item = it.clone();
        item.window = MenuWindow { rect: virtual_rect(tile(k, n)), dynamic_flags: flags::VISIBLE, name: format!("map_{i}_{k}"), ..MenuWindow::default() };
        item.text_scale = 0.0;
        out.items.push(item);
    }
    // The tabs, when more than one.
    if tabs.len() > 1 {
        for (k, &t) in tabs.iter().enumerate() {
            let mut b = out.items[0].clone();
            b.window = MenuWindow { rect: virtual_rect(tab_rect(k)), dynamic_flags: flags::VISIBLE, name: format!("maptab_{t}"), ..MenuWindow::default() };
            b.text = TABS[t].into();
            b.text_exp = Default::default();
            b.on_focus = "\"play\" \"mouse_over\" ; ".into();
            b.action = format!("\"play\" \"mouse_click\" ; \"setdvar\" \"{TAB_DVAR}\" \"{t}\" ; \"close\" \"self\" ; \"open\" \"private_match_maps\" ; ");
            out.items.push(b);
        }
    }
    let template = out.items[0].clone();
    out.items.push(cac::back_item(&template, &classic.on_esc));
    Some(out)
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name.starts_with(MENU))
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    let Some(tab) = om.menu.window.name.strip_prefix(MENU).and_then(|r| r.strip_prefix('|')).and_then(|t| t.parse::<usize>().ok()) else { return };
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    let b = k.v.bleed();
    k.fill(b, t::hex(0x050607, 1.0));
    header(&mut k, "Private Match", "Choose Map");
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.clone());
    let current = fe.lobby_view().map.0;
    let maps = super::super::lobby::maps();
    // The tabs.
    let tab_items: Vec<_> = om.menu.items.iter().filter(|it| it.window.name.starts_with("maptab_")).collect();
    for (slot, it) in tab_items.iter().enumerate() {
        let Some(tt) = it.window.name.strip_prefix("maptab_").and_then(|s| s.parse::<usize>().ok()) else { continue };
        let r = tab_rect(slot);
        let on = hot.as_deref() == Some(it.window.name.as_str());
        let ty = t::TAB;
        k.text_mid(r.x + 4.0, r.cy(), ty, TABS[tt], if tt == tab || on { t::INK } else { t::INK_DIM }, 0);
        let tw = k.measure(ty, TABS[tt]);
        if tt == tab {
            k.fill(R::new(r.x + 4.0, r.bottom() - 2.0, tw, 3.0), t::ACCENT);
        } else if on {
            k.fill(R::new(r.x + 4.0, r.bottom() - 2.0, tw, 2.0), t::INK_DIM);
        }
    }
    let n = om.menu.items.iter().filter(|it| it.window.name.starts_with("map_")).count();
    for it in om.menu.items.iter().filter(|it| it.window.name.starts_with("map_")) {
        let mut parts = it.window.name.strip_prefix("map_").unwrap_or("").split('_').filter_map(|s| s.parse::<usize>().ok());
        let (Some(i), Some(slot)) = (parts.next(), parts.next()) else { continue };
        let Some(&(id, name)) = maps.get(i) else { continue };
        let r = tile(slot, n);
        let on = hot.as_deref() == Some(it.window.name.as_str());
        k.fill(r, t::RAISED);
        let a = r.w / r.h;
        let vh = ((4.0 / 3.0) / a).min(1.0);
        k.pic_uv(r, &format!("loadscreen_{id}"), Rect::new(0.0, 0.5 - vh * 0.5, 1.0, 0.5 + vh * 0.5), if on { [1.0; 4] } else { [0.62, 0.62, 0.62, 1.0] });
        k.grad_v(R::new(r.x, r.y + r.h * 0.45, r.w, r.h * 0.55), t::hex(0x050607, 1.0), 0.0, 0.92);
        let label = {
            let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, fe).text() };
            let l = fe.assets.localize(&raw);
            if l.trim().is_empty() { name.to_owned() } else { l }
        };
        k.text(r.x + 16.0, r.bottom() - 40.0, Type { size: 26.0, ..t::H2 }, &label, t::INK, 0);
        if id == current {
            k.check(Vec2::new(r.right() - 22.0, r.y + 22.0), 20.0, 2.5, t::ACCENT);
        }
        if on {
            k.frame(r, 2.0, t::INK);
            k.brackets(r.inset(-5.0), 18.0, 3.0, t::ACCENT);
            k.fill(R::new(r.x, r.bottom() - 4.0, r.w, 4.0), t::ACCENT);
        } else {
            k.frame(r, 1.0, t::LINE);
        }
    }
    let back = om.menu.items.iter().any(|it| it.window.name == "cac_back") && hot.as_deref() == Some("cac_back");
    cac::back_button(&mut k, back);
    footer(&mut k, "", &[(Button::Confirm, "Select"), (Button::Back, "Back")]);
}
