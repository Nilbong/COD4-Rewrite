//! The Private Match's game type screen: every game type as a tile (its
//! icon, name and a line about it), and Hardcore as a switch under them.
//! Picking a tile sets it (`uiScript lobbyMode N`) and goes back.

use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, HUD_ICONS, Kit, MENU_ICONS, R, View, icons};
use super::shell::*;
use super::theme::{self as t, Type};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Window as MenuWindow, flags, item_type};

pub const MENU: &str = "next_modes";

const TOP: f32 = 226.0;
const PITCH: f32 = 54.0;

/// Row `i` of the list.
fn tile(i: usize) -> R {
    R::new(super::theme::col_x(0), TOP + i as f32 * PITCH, super::theme::cols(4), 50.0)
}

fn hardcore_rect(n: usize) -> R {
    let r = tile(n);
    R::new(r.x, r.y + 64.0, r.w, r.h)
}

/// The preview panel.
const PANEL: R = R::new(826.0, 192.0, 998.0, 440.0);

/// A mode's icon (sheet, index) and its line.
fn about(name: &str) -> ((&'static str, usize), &'static str) {
    match name {
        "Team Deathmatch" => ((HUD_ICONS, 10), "Two teams. The first to the score limit wins."),
        "Free-for-all" => ((MENU_ICONS, icons::CROSSHAIR), "Every player for themselves."),
        "Domination" => ((HUD_ICONS, 12), "Capture and hold three flags."),
        "Search and Destroy" => ((HUD_ICONS, 13), "Plant the bomb or defend the sites. One life a round."),
        "Headquarters" => ((HUD_ICONS, 15), "Take the headquarters and hold it."),
        "Sabotage" => ((HUD_ICONS, 13), "One bomb. Carry it to the enemy's target."),
        _ => ((MENU_ICONS, icons::HELMET), "Team Deathmatch seen from behind your soldier."),
    }
}

fn button(r: R, name: String, text: &str, action: String) -> Item {
    Item {
        ty: item_type::BUTTON,
        window: MenuWindow { rect: virtual_rect(r), dynamic_flags: flags::VISIBLE, name, ..MenuWindow::default() },
        text: text.into(),
        action,
        on_focus: "\"play\" \"mouse_over\" ; ".into(),
        text_scale: 0.0,
        ..Item::default()
    }
}

pub(in crate::ui) fn menu(fe: &Frontend) -> Menu {
    let view = fe.lobby_view();
    let mut m = Menu::default();
    m.window.name = MENU.into();
    m.full_screen = true;
    m.on_esc = "\"play\" \"mouse_click\" ; \"close\" \"self\" ; ".into();
    for (k, &(i, name, ok)) in view.modes.iter().enumerate() {
        let action = if ok { format!("\"play\" \"mouse_click\" ; \"uiScript\" \"lobbyMode\" \"{i}\" ; \"close\" \"self\" ; ") } else { String::new() };
        m.items.push(button(tile(k), format!("mode_{i}"), name, action));
    }
    if !view.guest {
        m.items.push(button(hardcore_rect(view.modes.len()), "mode_hardcore".into(), "Hardcore", "\"play\" \"mouse_click\" ; \"uiScript\" \"lobbyNext\" \"hardcore\" ; ".into()));
    }
    let template = m.items[0].clone();
    m.items.push(cac::back_item(&template, &m.on_esc));
    m
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    let b = k.v.bleed();
    k.fill(b, t::hex(0x050607, 1.0));
    header(&mut k, "Private Match", "Game Type");
    let view = fe.lobby_view();
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.clone());
    section_icon(&mut k, super::theme::col_x(0), 192.0, super::theme::cols(4), "Game type", Some(icons::CROSSHAIR));
    let mut shown = view.modes.iter().find(|m| m.0 == view.mode).copied();
    for (n, &(i, name, ok)) in view.modes.iter().enumerate() {
        let r = tile(n);
        let on = hot.as_deref() == Some(format!("mode_{i}").as_str());
        if on {
            shown = Some((i, name, ok));
        }
        if ok {
            menu_row(&mut k, r, name, on, None);
        } else {
            k.hline(r.x + 24.0, r.right(), r.bottom(), 1.0, t::HAIRLINE);
            k.text_mid(r.x + 24.0, r.cy(), Type { size: 28.0, ..t::ROW_TEXT }, name, t::INK_MUTE, 0);
            k.lock(Vec2::new(r.right() - 26.0, r.cy()), 18.0, t::INK_MUTE);
        }
        let ((sheet, icon), _) = about(name);
        let right = r.right() - if on { 52.0 } else { 20.0 };
        if ok {
            k.icon(sheet, icon, R::new(right - 26.0, r.cy() - 13.0, 26.0, 26.0), if on { t::INK } else { t::with_alpha(t::INK_DIM, 0.7) });
        }
        if i == view.mode {
            k.check(Vec2::new(right - 50.0, r.cy()), 18.0, 2.5, t::ACCENT);
        }
    }
    // Hardcore: a switch.
    if !view.guest {
        let r = hardcore_rect(view.modes.len());
        section(&mut k, r.x, r.y - 34.0, r.w, "Options");
        let on = hot.as_deref() == Some("mode_hardcore");
        row(&mut k, r, "Hardcore", Some(if view.hardcore { "On" } else { "Off" }), on);
    }
    // The mode under the cursor (or set): what it is.
    if let Some((_, name, ok)) = shown {
        grid_panel(&mut k, PANEL, 84.0, 1.0);
        let ((sheet, icon), line) = about(name);
        k.icon(sheet, icon, R::new(PANEL.x + 48.0, PANEL.y + 48.0, 150.0, 150.0), if ok { t::ACCENT } else { t::INK_MUTE });
        k.text(PANEL.x + 48.0, PANEL.y + 236.0, Type { size: 56.0, ..t::DISPLAY }, name, t::INK, 0);
        k.para(PANEL.x + 48.0, PANEL.y + 312.0, PANEL.w - 96.0, Type { size: 22.0, ..t::BODY }, line, t::INK_DIM, 1.3);
        let note = if !ok { "Not available online yet." } else if view.hardcore { "Hardcore: a minimal HUD and less health." } else { "" };
        if !note.is_empty() {
            k.text(PANEL.x + 48.0, PANEL.bottom() - 52.0, Type { size: 19.0, ..t::CAPTION }, note, if ok { t::REWARD } else { t::INK_MUTE }, 0);
        }
    }
    cac::back_button(&mut k, hot.as_deref() == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Select"), (Button::Back, "Back")]);
}
