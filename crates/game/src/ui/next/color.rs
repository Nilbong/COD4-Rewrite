//! The Camo Editor's colour picker: a hue bar and a square of shades (left
//! to right greyer to purer, top to bottom lighter to darker), each a grid of
//! cells so the D-pad moves around them like any menu; a click or A picks.
//! The gun behind updates as colours are picked; Done keeps it, Back puts
//! the colour back.

use super::super::expr::Env;
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Window as MenuWindow, flags, item_type};

pub const MENU: &str = "next_camo_color";

/// Hue steps (degrees) and the square's steps (percent).
const HUE_STEP: i32 = 10;
const SV_STEP: i32 = 10;
const HUES: i32 = 360 / HUE_STEP;
const SVS: i32 = 100 / SV_STEP + 1;

/// The square, the hue bar, the swatch and the buttons.
const SQUARE: R = R::new(96.0, 226.0, 600.0, 600.0);
const HUE: R = R::new(760.0, 226.0, 64.0, 600.0);
const DONE: R = R::new(890.0, 772.0, 934.0, 54.0);

fn sv_cell(si: i32, vi: i32) -> R {
    let (w, h) = (SQUARE.w / SVS as f32, SQUARE.h / SVS as f32);
    // Value from the top (100%) down.
    R::new(SQUARE.x + si as f32 * w, SQUARE.y + (SVS - 1 - vi) as f32 * h, w, h)
}

fn hue_cell(i: i32) -> R {
    let h = HUE.h / HUES as f32;
    R::new(HUE.x, HUE.y + i as f32 * h, HUE.w, h)
}

fn button(r: R, name: String, action: String) -> Item {
    Item {
        ty: item_type::BUTTON,
        window: MenuWindow { rect: virtual_rect(r), dynamic_flags: flags::VISIBLE, name, ..MenuWindow::default() },
        action,
        text_scale: 0.0,
        ..Item::default()
    }
}

/// The picker's menu: a button per cell, Done, and Back.
pub(in crate::ui) fn menu(_fe: &Frontend) -> Menu {
    let mut m = Menu::default();
    m.window.name = MENU.into();
    m.full_screen = true;
    m.on_esc = "\"play\" \"mouse_click\" ; \"uiScript\" ccamoColorCancel ;".into();
    for si in 0..SVS {
        for vi in 0..SVS {
            let (s, v) = (si * SV_STEP, vi * SV_STEP);
            m.items.push(button(sv_cell(si, vi), format!("sv_{s}_{v}"), format!("\"play\" \"mouse_click\" ; \"uiScript\" ccamoColorSV {s} {v} ;")));
        }
    }
    for i in 0..HUES {
        let h = i * HUE_STEP;
        m.items.push(button(hue_cell(i), format!("hue_{h}"), format!("\"play\" \"mouse_click\" ; \"uiScript\" ccamoColorHue {h} ;")));
    }
    let mut done = button(DONE, "color_done".into(), "\"play\" \"mouse_click\" ; \"uiScript\" ccamoColorDone ;".into());
    done.text = "Done".into();
    m.items.push(done);
    let template = m.items[0].clone();
    m.items.push(cac::back_item(&template, &m.on_esc));
    m
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

fn rgba(c: [u8; 3]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    cac::loadout_backdrop(&mut k);
    let (row, h, s, v, colour) = fe.colour_pick();
    header(&mut k, "Custom Camos  /  Camo Editor", &format!("Colour {row}"));
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.clone());
    let from_hsv = super::super::custom_camo::from_hsv;

    // The square, in this hue.
    section(&mut k, SQUARE.x, 192.0, SQUARE.w, "Shade");
    for si in 0..SVS {
        for vi in 0..SVS {
            let c = from_hsv(h, si as f32 / (SVS - 1) as f32, vi as f32 / (SVS - 1) as f32);
            k.fill(sv_cell(si, vi), rgba(c));
        }
    }
    k.frame(SQUARE.inset(-2.0), 2.0, t::LINE);
    // The hue bar.
    k.text(HUE.cx(), 192.0, t::LABEL, "Hue", t::INK_DIM, 1);
    for i in 0..HUES {
        k.fill(hue_cell(i), rgba(from_hsv((i * HUE_STEP) as f32, 1.0, 1.0)));
    }
    k.frame(HUE.inset(-2.0), 2.0, t::LINE);

    // The picked cells (white ring) and the one under the cursor or pad.
    let picked_sv = sv_cell((s * 100.0 / SV_STEP as f32).round() as i32, (v * 100.0 / SV_STEP as f32).round() as i32);
    let picked_h = hue_cell((h / HUE_STEP as f32).round() as i32 % HUES);
    for r in [picked_sv, picked_h] {
        k.frame(r.inset(-3.0), 3.0, t::hex(0x000000, 1.0));
        k.frame(r.inset(-1.0), 2.0, t::INK);
    }
    if let Some(name) = &hot {
        let r = if let Some(rest) = name.strip_prefix("sv_") {
            let mut p = rest.split('_').filter_map(|n| n.parse::<i32>().ok());
            Some(sv_cell(p.next().unwrap_or(0) / SV_STEP, p.next().unwrap_or(0) / SV_STEP))
        } else {
            name.strip_prefix("hue_").and_then(|n| n.parse::<i32>().ok()).map(|h| hue_cell(h / HUE_STEP))
        };
        if let Some(r) = r {
            k.brackets(r.inset(-5.0), 10.0, 3.0, t::ACCENT);
        }
    }

    // The colour, the gun wearing it, Done.
    let (gun, key, camo, ..) = fe.custom_camo_editor_view();
    let p = R::new(890.0, 226.0, 934.0, 520.0);
    grid_panel(&mut k, p, 70.0, 1.0);
    let weapon = gun.split_once(':').map_or(gun.as_str(), |(w, _)| w);
    let picture = fe.table_lookup("mp/statstable.csv", 4, weapon, 6);
    k.gun(R::new(p.x + 40.0, p.y + 24.0, p.w - 80.0, 300.0), key, &gun, camo, &picture);
    let sw = R::new(p.x + 40.0, p.y + 350.0, 120.0, 120.0);
    icon_box(&mut k, sw);
    k.fill(sw.inset(10.0), rgba(colour));
    k.text(sw.right() + 24.0, sw.y + 6.0, Type { size: 36.0, ..t::H1 }, &format!("Colour {row}"), t::INK, 0);
    k.text(sw.right() + 24.0, sw.y + 52.0, Type { size: 22.0, ..t::BODY }, &format!("#{:02X}{:02X}{:02X}", colour[0], colour[1], colour[2]), t::INK_DIM, 0);
    k.text(sw.right() + 24.0, sw.y + 84.0, Type { size: 18.0, ..t::CAPTION }, &format!("Hue {h:.0}\u{b0}  \u{b7}  Shade {:.0}%  \u{b7}  Light {:.0}%", s * 100.0, v * 100.0), t::INK_MUTE, 0);
    action_button(&mut k, DONE, "Done", hot.as_deref() == Some("color_done"));
    cac::back_button(&mut k, hot.as_deref() == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Pick"), (Button::Back, "Cancel")]);
}
