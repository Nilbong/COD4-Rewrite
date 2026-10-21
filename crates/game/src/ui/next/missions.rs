//! The campaign's Mission Select in the new UI's look.
//!
//! - **Acts** (CoD4's `sp:levels`): the act buttons down the left column
//!   (Prologue to Epilogue, the locked ones hidden by the classic rules).
//! - **Missions** (`sp:popmenu_act_*`): the act's missions as a grid of
//!   tiles, each with the mission's own picture (`levelshot_<map>`)
//!   and line (`desc_<map>`); missions not reached yet are shown locked.
//!
//! The classic buttons are kept and moved: their actions (`ui_load_level`
//! then the difficulty popup, opening an act) and visibility rules are the
//! classic menus'.

use super::super::expr::{Env, eval};
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Statement, Window as MenuWindow, flags, item_type};

pub const ACTS: &str = "next_acts";
pub const MISSIONS: &str = "next_missions";

/// The classic act list, and an act's mission list (`sp:popmenu_act_*`).
pub(in crate::ui) fn wanted(key: &str) -> bool {
    key == "sp:levels" || key.starts_with("sp:popmenu_act_")
}

/// An act's name by its popup.
fn act_name(key: &str) -> &'static str {
    match key.trim_start_matches("sp:popmenu_act_") {
        "pre" => "Prologue",
        "one" => "Act I",
        "two" => "Act II",
        "three" => "Act III",
        "ep" => "Epilogue",
        _ => "Missions",
    }
}

fn label_of(fe: &Frontend, it: &Item) -> String {
    let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, fe).text() };
    fe.assets.localize(&raw).trim().to_owned()
}

/// A mission button's map (`"ui_load_level" "<map>"` in its action).
fn map_of(it: &Item) -> Option<&str> {
    it.action.split("\"ui_load_level\"").nth(1).and_then(|r| r.split('"').nth(1))
}

// --- Acts --------------------------------------------------------------------

const ACT_TOP: f32 = 226.0;
const ACT_PITCH: f32 = 56.0;

fn act_rect(i: usize) -> R {
    R::new(col_x(0), ACT_TOP + i as f32 * ACT_PITCH, cols(4), 50.0)
}

/// The new screens built from the classic menus.
pub(in crate::ui) fn menu(fe: &Frontend, classic: &Menu, key: &str) -> Option<Menu> {
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    let buttons: Vec<&Item> = classic.items.iter().filter(|it| it.ty == item_type::BUTTON).collect();
    if key == "sp:levels" {
        out.window.name = ACTS.into();
        let mut n = 0;
        for it in buttons.iter().filter(|it| it.action.contains("popmenu_act_")) {
            let mut item = (*it).clone();
            let label = label_of(fe, it);
            item.window = MenuWindow { rect: virtual_rect(act_rect(n)), dynamic_flags: it.window.dynamic_flags, name: format!("act_{n}"), ..MenuWindow::default() };
            item.text = label;
            item.text_exp = Statement::default();
            item.text_scale = 0.0;
            out.items.push(item);
            n += 1;
        }
    } else {
        out.window.name = format!("{MISSIONS}|{key}");
        let missions: Vec<&&Item> = buttons.iter().filter(|it| map_of(it).is_some()).collect();
        let n = missions.len();
        for (i, it) in missions.into_iter().enumerate() {
            let mut item = (**it).clone();
            item.window = MenuWindow { rect: virtual_rect(tile_rect(i, n)), dynamic_flags: it.window.dynamic_flags, name: format!("mission_{i}"), ..MenuWindow::default() };
            item.text = label_of(fe, it);
            item.text_exp = Statement::default();
            item.text_scale = 0.0;
            out.items.push(item);
        }
    }
    let template = out.items.first()?.clone();
    let mut back = cac::back_item(&template, &classic.on_esc);
    // (The act list's Back goes to the campaign's menu, as its classic one.)
    if let Some(b) = buttons.iter().find(|it| label_of(fe, it).eq_ignore_ascii_case("back")) {
        back.action = b.action.clone();
    }
    out.items.push(back);
    Some(out)
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == ACTS || m.menu.window.name.starts_with(MISSIONS))
}

fn focused<'a>(fe: &Frontend, om: &'a OpenMenu) -> Option<&'a str> {
    fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.as_str())
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    let name = om.menu.window.name.as_str();
    if name == ACTS {
        paint_acts(fe, om, pl, ops);
    } else if let Some(key) = name.strip_prefix(MISSIONS).and_then(|k| k.strip_prefix('|')) {
        paint_missions(fe, om, pl, ops, key);
    }
}

fn paint_acts(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    // (Under an act's missions: only the backdrop.)
    let covered = fe.stack.last().is_some_and(|m| !std::ptr::eq(m, om));
    backdrop(&mut k, 0.05);
    if covered {
        return;
    }
    header(&mut k, "Campaign", "Mission Select");
    let hot = focused(fe, om);
    section(&mut k, col_x(0), 192.0, cols(4), "Acts");
    for (i, it) in om.menu.items.iter().enumerate() {
        let Some(n) = it.window.name.strip_prefix("act_").and_then(|n| n.parse::<usize>().ok()) else { continue };
        let r = act_rect(n);
        if fe.item_visible(om, i) {
            menu_row(&mut k, r, &it.text, hot == Some(it.window.name.as_str()), None);
        } else {
            k.hline(r.x + 24.0, r.right(), r.bottom(), 1.0, t::HAIRLINE);
            k.text_mid(r.x + 24.0, r.cy(), Type { size: 26.0, ..t::ROW_TEXT }, &it.text, t::INK_MUTE, 0);
            k.lock(Vec2::new(r.right() - 30.0, r.cy()), 16.0, t::INK_MUTE);
        }
    }
    let art = R::new(col_x(5), 192.0, cols(7), 620.0);
    k.fill(art, t::RAISED);
    k.pic_uv(art, "next:preview_campaign", crop(art, 2.0), [1.0; 4]);
    k.grad_v(R::new(art.x, art.y + art.h * 0.3, art.w, art.h * 0.7), t::hex(0x050607, 1.0), 0.0, 0.95);
    k.slice9(art.inset(-10.0), "next:panels/frame_big", Vec2::new(569.0, 256.0), 84.0, 1.0, [1.0, 1.0, 1.0, 0.95]);
    k.text(art.x + 36.0, art.bottom() - 150.0, t::LABEL, "Call of Duty 4", t::ACCENT, 0);
    k.text(art.x + 36.0, art.bottom() - 122.0, Type { size: 64.0, ..t::DISPLAY }, "Choose an act", t::INK, 0);
    k.para(art.x + 36.0, art.bottom() - 50.0, art.w - 72.0, t::BODY, "Acts open as the campaign reaches them.", t::INK_DIM, 1.3);
    cac::back_button(&mut k, hot == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Select"), (Button::Back, "Back")]);
}

// --- Missions ----------------------------------------------------------------

const GRID_TOP: f32 = 226.0;
const GRID_BOTTOM: f32 = 960.0;

/// Tile `i` of `n`: four across (three for three or fewer rows' worth).
fn tile_rect(i: usize, n: usize) -> R {
    let across = if n <= 3 { n.max(1) } else { 4 };
    let rows = n.div_ceil(across).max(1);
    let gap = 24.0;
    let w = (cols(12) - gap * (across - 1) as f32) / across as f32;
    let h = ((GRID_BOTTOM - GRID_TOP - gap * (rows - 1) as f32) / rows as f32).min(w * 0.75);
    R::new(col_x(0) + (i % across) as f32 * (w + gap), GRID_TOP + (i / across) as f32 * (h + gap), w, h)
}

fn paint_missions(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>, key: &str) {
    let Some(classic) = fe.assets.menu(key) else { return };
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    backdrop(&mut k, 0.05);
    header(&mut k, "Campaign  /  Mission Select", act_name(key));
    let hot = focused(fe, om);
    let tiles: Vec<(usize, &Item)> = om.menu.items.iter().enumerate().filter(|(_, it)| it.window.name.starts_with("mission_")).collect();
    let n = tiles.len();
    for (j, (i, it)) in tiles.into_iter().enumerate() {
        let r = tile_rect(j, n);
        let open = fe.item_visible(om, i);
        let on = open && hot == Some(it.window.name.as_str());
        let map = map_of(it).unwrap_or_default();
        // The mission's own picture (its classic image item's).
        // (The classic image items draw `levelshot_<map>` by owner draw.)
        let picture = classic.items.iter().find(|c| c.window.name == format!("image_{map}")).and_then(|c| c.window.background.clone()).or_else(|| Some(format!("levelshot_{map}")));
        let desc = classic.items.iter().find(|c| c.window.name == format!("desc_{map}")).map(|c| label_of(fe, c)).unwrap_or_default();
        let pic = R::new(r.x, r.y, r.w, r.h - 70.0);
        k.fill(r, t::hex(0x0b0d0e, 0.92));
        if let Some(p) = picture {
            k.pic_uv(pic, &p, crop(pic, 1.0), if open { [1.0; 4] } else { [0.35, 0.35, 0.35, 1.0] });
        }
        k.grad_v(R::new(pic.x, pic.bottom() - 60.0, pic.w, 60.0), t::hex(0x050607, 1.0), 0.0, 0.8);
        k.frame(r, if on { 2.0 } else { 1.0 }, if on { t::ACCENT } else { t::LINE });
        if on {
            k.brackets(r.inset(-6.0), 22.0, 3.0, t::ACCENT);
        }
        k.text(r.x + 18.0, pic.bottom() + 10.0, Type { size: 26.0, ..t::H2 }, &it.text, if open { t::INK } else { t::INK_MUTE }, 0);
        if open {
            k.text(r.x + 18.0, pic.bottom() + 42.0, Type { size: 17.0, ..t::CAPTION }, &desc, t::INK_DIM, 0);
        } else {
            k.lock(Vec2::new(pic.cx(), pic.cy()), 28.0, t::INK_DIM);
            k.text(r.x + 18.0, pic.bottom() + 42.0, Type { size: 17.0, ..t::CAPTION }, "Not reached yet", t::INK_MUTE, 0);
        }
    }
    cac::back_button(&mut k, hot == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Play"), (Button::Back, "Back")]);
}
