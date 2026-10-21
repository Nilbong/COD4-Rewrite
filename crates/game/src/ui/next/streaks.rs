//! Create a Class's kill streaks: choose up to three of the game's
//! ([`crate::killstreaks::Hardpoint::ALL`]), listed by the kills they take.
//! The choice is the player's (MW2's way, not per class), saved with the
//! profile ([`crate::killstreaks::choice`], which the game reads).

use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use crate::killstreaks::{Hardpoint, choice};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Window as MenuWindow, flags, item_type};

pub const MENU: &str = "next_streaks";
/// At most this many.
pub const MAX: usize = choice::MAX;

const TOP: f32 = 226.0;
const PITCH: f32 = 62.0;
const PANEL: R = R::new(826.0, 192.0, 998.0, 560.0);

/// The streaks chosen (the profile's, `crate::killstreaks::choice`), by kills.
pub(in crate::ui) fn chosen(fe: &Frontend) -> Vec<Hardpoint> {
    choice::chosen(|i| fe.stats.get(i))
}

/// The streaks, by kills.
fn all() -> Vec<Hardpoint> {
    let mut list = Hardpoint::ALL.to_vec();
    list.sort_by_key(|h| h.kills());
    list
}

fn row(i: usize) -> R {
    R::new(col_x(0), TOP + i as f32 * PITCH, cols(4), 56.0)
}

pub(in crate::ui) fn menu(fe: &Frontend) -> Menu {
    let mut m = Menu::default();
    m.window.name = MENU.into();
    m.full_screen = true;
    m.on_esc = "\"play\" \"mouse_click\" ; \"close\" \"self\" ; ".into();
    for (i, h) in all().into_iter().enumerate() {
        m.items.push(Item {
            ty: item_type::BUTTON,
            window: MenuWindow { rect: virtual_rect(row(i)), dynamic_flags: flags::VISIBLE, name: format!("streak_{}", h.id()), ..MenuWindow::default() },
            text: h.name().into(),
            action: format!("\"play\" \"mouse_click\" ; \"uiScript\" \"streakToggle\" \"{}\" ; ", h.id()),
            on_focus: "\"play\" \"mouse_over\" ; ".into(),
            text_scale: 0.0,
            ..Item::default()
        });
    }
    let _ = fe;
    let template = m.items[0].clone();
    m.items.push(cac::back_item(&template, &m.on_esc));
    m
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

impl Frontend {
    /// `uiScript streakToggle <id>`: in or out of the three (when there's
    /// room); kept by kills.
    pub(in crate::ui) fn streak_script(&mut self, args: &[String]) -> bool {
        if !args.first().is_some_and(|a| a.eq_ignore_ascii_case("streaktoggle")) {
            return false;
        }
        let Some(h) = args.get(1).and_then(|s| Hardpoint::from_id(s)) else { return true };
        let mut list = chosen(self);
        if let Some(at) = list.iter().position(|x| *x == h) {
            list.remove(at);
        } else {
            // Full: the last one makes way.
            if list.len() >= MAX {
                list.pop();
            }
            list.push(h);
        }
        let stats = &mut self.stats;
        choice::set(&mut |i, v| stats.set(i, v), &list);
        self.stats.save_if_changed();
        true
    }
}

/// What a streak does.
fn about(h: Hardpoint) -> &'static str {
    match h {
        Hardpoint::Uav => "Shows enemies on the minimap for your team.",
        Hardpoint::CarePackage => "A crate dropped by helicopter: a random kill streak or ammo.",
        Hardpoint::Airstrike => "Three jets bomb where you mark on the map.",
        Hardpoint::Sentry => "A sentry gun you place, that fires at enemies it sees.",
        Hardpoint::Helicopter => "An attack helicopter that hunts enemies for a while.",
    }
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    cac::loadout_backdrop(&mut k);
    header(&mut k, "Loadout  /  Create a Class", "Kill Streaks");
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.clone());
    let picked = chosen(fe);
    section(&mut k, col_x(0), 192.0, cols(4), &format!("Choose up to {MAX}  \u{b7}  {} chosen", picked.len()));
    let mut shown = None;
    for (i, h) in all().into_iter().enumerate() {
        let r = row(i);
        let on = hot.as_deref() == Some(format!("streak_{}", h.id()).as_str());
        if on {
            shown = Some(h);
        }
        let mine = picked.contains(&h);
        menu_row(&mut k, r, h.name(), on, None);
        // Its icon left of the name? Right: kills, icon, tick.
        let right = r.right() - if on { 56.0 } else { 20.0 };
        k.text_mid(right, r.cy(), Type { size: 18.0, ..t::CAPTION }, &format!("{} kills", h.kills()), t::INK_DIM, 2);
        k.pic(R::new(right - 130.0, r.cy() - 20.0, 40.0, 40.0), h.hud_icon(), [1.0, 1.0, 1.0, if mine || on { 1.0 } else { 0.5 }]);
        if mine {
            k.check(Vec2::new(right - 156.0, r.cy()), 18.0, 2.5, t::ACCENT);
        }
    }
    // The one under the cursor, else the chosen three in order.
    grid_panel(&mut k, PANEL, 84.0, 1.0);
    let (x, w) = (PANEL.x + 44.0, PANEL.w - 88.0);
    match shown {
        Some(h) => {
            k.text(x, PANEL.y + 36.0, t::LABEL, &format!("{} kills", h.kills()), t::ACCENT, 0);
            k.text(x, PANEL.y + 60.0, Type { size: 56.0, ..t::DISPLAY }, h.name(), t::INK, 0);
            k.pic(R::new(PANEL.right() - 44.0 - 220.0, PANEL.y + 40.0, 220.0, 220.0), h.hud_icon(), [1.0; 4]);
            k.para(x, PANEL.y + 150.0, w - 280.0, Type { size: 22.0, ..t::BODY }, about(h), t::INK_DIM, 1.3);
            let note = if picked.contains(&h) { "Chosen. Select to take it out." } else if picked.len() < MAX { "Select to add it." } else { "Select to swap it for your last one." };
            k.text(x, PANEL.bottom() - 60.0, Type { size: 19.0, ..t::CAPTION }, note, t::INK_MUTE, 0);
        }
        None => {
            k.text(x, PANEL.y + 36.0, t::LABEL, "Your kill streaks", t::ACCENT, 0);
            let cw = (w - 32.0) / MAX as f32;
            for (i, h) in picked.iter().enumerate() {
                let cx = x + i as f32 * (cw + 16.0);
                icon_box(&mut k, R::new(cx, PANEL.y + 90.0, cw, cw));
                k.pic(R::new(cx + 24.0, PANEL.y + 114.0, cw - 48.0, cw - 48.0), h.hud_icon(), [1.0; 4]);
                k.text(cx, PANEL.y + 110.0 + cw, Type { size: 26.0, ..t::H2 }, h.name(), t::INK, 0);
                k.text(cx, PANEL.y + 146.0 + cw, t::MICRO, &format!("{} kills", h.kills()), t::INK_DIM, 0);
            }
        }
    }
    cac::back_button(&mut k, hot.as_deref() == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Choose"), (Button::Back, "Done")]);
}
