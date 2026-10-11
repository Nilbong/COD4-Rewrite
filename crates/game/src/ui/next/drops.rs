//! The new Supply Drops screen: Open Supply Drop and the drop's progress
//! and collection on the left; the drop's three cards on the right, dealt
//! face up one after another when opened (rarity colour, the gun or
//! character in 3D, its name, and a variant's tweaks).
//!
//! The classic screen's Open row (`uiScript supplyOpen`) is kept and moved;
//! the timing of the reveal is the classic screen's (`supply.rs`).

use super::super::expr::Env;
use super::super::supply::{CARD_KEY, FLASH_MS, OPENED_DVAR, REVEAL_FIRST_MS, REVEAL_STEP_MS};
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, MENU_ICONS, R, View, icons};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use crate::supply::{self, Item as DropItem, Rarity};
use bevy::prelude::*;
use iw3::menu::{Menu, Statement, Window as MenuWindow, flags, item_type};

pub const MENU: &str = "next_drops";

const OPEN: R = R::new(96.0, 226.0, 584.0, 64.0);
const CARDS_TOP: f32 = 192.0;
const CARD_H: f32 = 760.0;

fn card_rect(i: usize) -> R {
    let x0 = col_x(5);
    let w = (cols(7) - 2.0 * t::GUTTER) / 3.0;
    R::new(x0 + i as f32 * (w + t::GUTTER), CARDS_TOP, w, CARD_H)
}

fn rgba(c: [f32; 4]) -> t::Rgba {
    c
}

pub(in crate::ui) fn menu(classic: &Menu) -> Option<Menu> {
    let open = classic.items.iter().find(|it| it.ty == item_type::BUTTON && it.action.contains("supplyOpen"))?;
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = MENU.into();
    let mut item = open.clone();
    item.window = MenuWindow { rect: virtual_rect(OPEN), dynamic_flags: flags::VISIBLE, name: "drop_open".into(), ..MenuWindow::default() };
    item.text = "Open Supply Drop".into();
    item.text_exp = Statement::default();
    item.text_scale = 0.0;
    out.items.push(item.clone());
    out.items.push(cac::back_item(&item, &classic.on_esc));
    Some(out)
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

/// A bar with its label and value over it.
fn stat(k: &mut Kit, x: f32, y: f32, w: f32, label: &str, value: &str, f: f32, c: t::Rgba) {
    k.text(x, y, t::MICRO, label, t::INK_DIM, 0);
    k.text(x + w, y - 6.0, Type { size: 24.0, ..t::NUMERAL }, value, t::INK, 2);
    let bar = R::new(x, y + 30.0, w, 6.0);
    k.fill(bar, t::hex(0xffffff, 0.12));
    k.fill(R::new(bar.x, bar.y, bar.w * f.clamp(0.0, 1.0), bar.h), c);
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    let b = k.v.bleed();
    k.fill(b, t::hex(0x050607, 1.0));
    const ASPECT: f32 = 1672.0 / 941.0;
    let (w, h) = if b.w / b.h > ASPECT { (b.w, b.w / ASPECT) } else { (b.h * ASPECT, b.h) };
    k.pic(R::new(b.cx() - w * 0.5, b.cy() - h * 0.5, w, h), "next:bg_barracks", [1.0; 4]);
    k.grad_h(R::new(b.x, b.y, b.w * 0.5, b.h), t::hex(0x050607, 1.0), 0.9, 0.0);
    k.fill(b, t::hex(0x050607, 0.35));
    header(&mut k, "Barracks", "Supply Drops");
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.clone());
    let inv = supply::inventory();

    // Open.
    let x = col_x(0);
    let cw = cols(4);
    section_icon(&mut k, x, 192.0, cw, "Your drops", Some(icons::CRATE));
    let can = inv.unopened > 0;
    let on = hot.as_deref() == Some("drop_open");
    k.slice3(OPEN, "next:panels/btn_chevron", Vec2::new(396.0, 57.0), 30.0, 80.0, [1.0, 1.0, 1.0, if can { 1.0 } else { 0.5 }]);
    if can {
        k.grad_h(R::new(OPEN.x + 4.0, OPEN.y + 4.0, OPEN.w - 8.0, OPEN.h - 8.0), t::ACCENT, if on { 0.75 } else { 0.4 }, 0.0);
        k.fill(R::new(OPEN.x, OPEN.y, t::FOCUS_BAR, OPEN.h), t::ACCENT);
    }
    k.text_mid(OPEN.x + 28.0, OPEN.cy(), Type { size: 32.0, ..t::H1 }, "Open Supply Drop", if can { t::INK } else { t::INK_MUTE }, 0);
    // How many, as text in the button.
    let count = match inv.unopened {
        0 => "None to open".to_owned(),
        1 => "1 ready".to_owned(),
        n => format!("{n} ready"),
    };
    k.text_mid(OPEN.right() - 96.0, OPEN.cy(), t::LABEL, &count, if can { t::INK } else { t::INK_MUTE }, 2);

    // Progress and collection.
    let mut y = OPEN.bottom() + 48.0;
    let interval = supply::drop_interval();
    let left = interval - inv.progress;
    stat(&mut k, x, y, cw, "Next drop (match time)", &supply::format_time(left), inv.progress / interval.max(1.0), t::ACCENT);
    y += 76.0;
    stat(&mut k, x, y, cw, "Variants collected", &format!("{} / {}", inv.owned_count(), supply::catalogue_size()), inv.owned_count() as f32 / supply::catalogue_size().max(1) as f32, t::FRIENDLY);
    y += 76.0;
    let chars = inv.characters_owned().len();
    let all = crate::characters::CHARACTERS.len();
    stat(&mut k, x, y, cw, "Characters collected", &format!("{chars} / {all}"), chars as f32 / all.max(1) as f32, t::FRIENDLY);
    y += 96.0;

    // The odds.
    section(&mut k, x, y, cw, "Drop chances");
    y += 40.0;
    for r in supply::RARITIES {
        let row = R::new(x, y, cw, 44.0);
        k.fill(row, t::ROW);
        k.fill(R::new(row.x, row.y, 4.0, row.h), rgba(r.color()));
        k.text_mid(row.x + 20.0, row.cy(), t::LABEL, r.name(), rgba(r.color()), 0);
        k.text_mid(row.right() - 18.0, row.cy(), Type { size: 24.0, ..t::NUMERAL }, &format!("{:.0}%", r.chance() * 100.0), t::INK, 2);
        y += 50.0;
    }

    // The cards.
    let opened: Option<i64> = fe.dvar(OPENED_DVAR).parse().ok();
    let now = fe.millis();
    for i in 0..supply::DROP_SIZE {
        let r = card_rect(i);
        let item = inv.last_drop.get(i).copied();
        let shown_at = opened.map(|t| t + REVEAL_FIRST_MS + REVEAL_STEP_MS * i as i64);
        match (item, shown_at) {
            (Some((item, new)), Some(at)) if now >= at => card(&mut k, fe, r, item, new, i, now - at),
            (Some((item, new)), None) => card(&mut k, fe, r, item, new, i, FLASH_MS),
            _ => sealed(&mut k, r, now, opened.is_some()),
        }
    }
    if inv.last_drop.is_empty() {
        let hint = if inv.unopened > 0 { "Open one to see inside." } else { "Play matches to earn drops." };
        let cr = card_rect(1);
        k.text(cr.cx(), cr.cy() + 120.0, Type { size: 19.0, ..t::CAPTION }, hint, t::INK_DIM, 1);
    }
    drop(inv);
    cac::back_button(&mut k, hot.as_deref() == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Open"), (Button::Back, "Back")]);
}

/// A card face down: the crate, waiting (pulsing while it's being dealt).
fn sealed(k: &mut Kit, r: R, now: i64, dealing: bool) {
    grid_panel(k, r, 60.0, 0.6);
    let pulse = if dealing { 0.5 + 0.5 * (now as f32 * 0.01).sin() } else { 0.0 };
    let c = Vec2::new(r.cx(), r.cy() - 30.0);
    k.icon(MENU_ICONS, icons::CRATE, R::new(c.x - 70.0, c.y - 60.0, 140.0, 120.0), t::with_alpha(t::REWARD, 0.35 + 0.5 * pulse));
    k.text(r.cx(), c.y + 90.0, t::LABEL, "Supply drop", t::INK_MUTE, 1);
}

fn card(k: &mut Kit, fe: &Frontend, r: R, item: DropItem, new: bool, index: usize, age: i64) {
    let rarity = item.rarity();
    let colour = rgba(rarity.color());
    // Elite cards glow.
    if rarity == Rarity::Elite {
        let g = 0.25 + 0.15 * (fe.millis() as f32 * 0.004).sin();
        k.glow(r.inset(-60.0), t::with_alpha(colour, g));
    }
    grid_panel(k, r, 60.0, 1.0);
    k.fill(R::new(r.x, r.y, r.w, 6.0), colour);
    k.grad_v(R::new(r.x, r.y + 6.0, r.w, 140.0), colour, 0.10, 0.0);
    k.text(r.x + 28.0, r.y + 28.0, t::LABEL, rarity.name(), colour, 0);
    if new {
        k.pic(R::new(r.right() - 108.0, r.y + 22.0, 85.0, 30.0), "next:badge_new", [1.0; 4]);
    }
    let key = CARD_KEY + index as i32;
    match item {
        DropItem::Variant(v) => {
            let picture = fe.table_lookup("mp/statstable.csv", 4, v.weapon, 6);
            k.gun(R::new(r.x + 16.0, r.y + 90.0, r.w - 32.0, 220.0), key, &format!("{}:", v.weapon), v.camo, &picture);
            let mut y = r.y + 340.0;
            k.para(r.x + 28.0, y, r.w - 56.0, Type { size: 40.0, ..t::DISPLAY }, v.name, t::INK, 1.0);
            y += 56.0;
            k.text(r.x + 28.0, y, Type { size: 22.0, ..t::H2 }, &fe.weapon_name(v.weapon), t::INK_DIM, 0);
            k.text(r.x + 28.0, y + 32.0, t::MICRO, v.game().name(), t::INK_MUTE, 0);
            y += 76.0;
            k.hline(r.x + 28.0, r.right() - 28.0, y, 1.0, t::with_alpha(colour, 0.5));
            y += 16.0;
            for (text, up) in v.tweak_texts() {
                k.text(r.x + 28.0, y, Type { size: 21.0, ..t::BODY }, &text, if up { t::ACCENT } else { t::ENEMY }, 0);
                y += 32.0;
            }
            if v.camo > 0 {
                k.text(r.x + 28.0, r.bottom() - 48.0, t::LABEL, &format!("{} camo", fe.camo_name(v.camo)), colour, 0);
            }
        }
        DropItem::Character(c) => {
            let weapon = format!("char:{}", c.id);
            k.gun(R::new(r.x + 16.0, r.y + 70.0, r.w - 32.0, 470.0), key, &weapon, 0, "");
            let y = r.y + 560.0;
            k.para(r.x + 28.0, y, r.w - 56.0, Type { size: 40.0, ..t::DISPLAY }, c.name, t::INK, 1.0);
            k.text(r.x + 28.0, r.bottom() - 80.0, t::MICRO, "Character", t::INK_DIM, 0);
            k.text(r.x + 28.0, r.bottom() - 52.0, t::MICRO, c.game.name(), t::INK_MUTE, 0);
        }
    }
    // Turning over: a flash that fades.
    if age < FLASH_MS {
        let f = 1.0 - age as f32 / FLASH_MS as f32;
        k.fill(r, t::with_alpha(t::INK, 0.85 * f));
    }
}
