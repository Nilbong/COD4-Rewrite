//! The match's menus in the new style: the Escape menu (CoD4's `class`, and
//! Headquarters' `hq_pause` made from it), Change Team (`team_marinesopfor`)
//! and Choose Class (`changeclass`). Each keeps the classic menu's buttons
//! (their variants per team and server, and every script) moved into a
//! column list like the main menu's, over the match dimmed; the right side
//! shows the match (game type, map, your team) or the focused class.

use super::super::expr::{Env, eval};
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, MENU_ICONS, R, View, icons};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Rect as VRect, Statement, Token, flags, item_type, op};

pub const PREFIX: &str = "next_match|";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pause,
    Team,
    Class,
}

fn kind_of(key: &str) -> Option<Kind> {
    match key {
        "class" | super::super::hq::PAUSE_MENU => Some(Kind::Pause),
        "team_marinesopfor" => Some(Kind::Team),
        "changeclass" | "changeclass_marines" | "changeclass_opfor" => Some(Kind::Class),
        _ => None,
    }
}

pub(in crate::ui) fn wanted(key: &str) -> bool {
    kind_of(key).is_some()
}

const TOP: f32 = 176.0;
const ROW_H: f32 = 48.0;
const PITCH: f32 = 50.0;
/// The right side.
const PANEL: R = R::new(826.0, 192.0, 998.0, 760.0);

/// A design rect from an item rect made by [`virtual_rect`].
fn design(r: &VRect) -> R {
    let k = 480.0 / 1080.0;
    R::new(r.x / k + 960.0, r.y / k, r.w / k, r.h / k)
}

/// The classic left column's buttons.
fn left_button(it: &Item) -> bool {
    it.ty == item_type::BUTTON && it.window.rect.horz_align == 2 && it.window.rect.x < -200.0 && !it.window.name.eq_ignore_ascii_case("back")
}

fn label_of(fe: &Frontend, it: &Item) -> String {
    let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, fe).text() };
    fe.assets.localize(&raw).trim().to_owned()
}

/// The class a Choose Class row picks (`assault`, `custom1`).
fn class_of(action: &str) -> Option<String> {
    let at = action.find("scriptMenuResponse")?;
    let arg = action[at..].split('"').nth(2)?;
    let class = arg.split(',').next()?;
    (!class.is_empty() && class != "back").then(|| class.to_owned())
}

/// The screen for `key`, from its classic menu.
pub(in crate::ui) fn menu(fe: &Frontend, classic: &Menu, key: &str) -> Option<Menu> {
    let kind = kind_of(key)?;
    let mut out = Menu { items: Vec::new(), ..classic.clone() };
    out.window.name = format!("{PREFIX}{key}");
    // The rows by the classic heights (a row's variants share one), in
    // sections: Choose Class's by its "Custom Classes" header.
    let custom_y = classic
        .items
        .iter()
        .find(|it| it.ty != item_type::BUTTON && it.text.eq_ignore_ascii_case("@MPUI_CUSTOM_CLASSES") || label_of(fe, it) == "Custom Classes")
        .map(|it| it.window.rect.y);
    // (Only heights with a variant showing now: the team and the server
    // decide, and they don't change while the menu's up.)
    let showing = |it: &Item| it.window.dynamic_flags & flags::VISIBLE != 0 && (it.visible_exp.is_empty() || eval(&it.visible_exp, fe).truthy());
    let mut ys: Vec<f32> = classic.items.iter().filter(|it| left_button(it) && showing(it)).map(|it| it.window.rect.y).collect();
    ys.sort_by(|a, b| a.total_cmp(b));
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    if ys.is_empty() {
        return None;
    }
    let section = |y: f32| usize::from(kind == Kind::Class && custom_y.is_some_and(|c| y > c));
    let mut y = TOP;
    let mut last = None;
    let mut place = Vec::new();
    for &cy in &ys {
        let s = section(cy);
        if last != Some(s) {
            if last.is_some() {
                y += 22.0;
            }
            let mut h = Item::default();
            h.window.name = format!("msec_{s}");
            h.window.rect = virtual_rect(R::new(col_x(0), y, cols(4), 30.0));
            out.items.push(h);
            y += 34.0;
            last = Some(s);
        }
        place.push((cy, R::new(col_x(0), y, cols(4), ROW_H)));
        y += PITCH;
    }
    let mut template = None;
    for it in &classic.items {
        if left_button(it) {
            let Some((_, r)) = place.iter().find(|(y, _)| (*y - it.window.rect.y).abs() < 0.5) else { continue };
            let mut b = it.clone();
            b.text = label_of(fe, it);
            b.text_exp = Statement::default();
            b.text_scale = 0.0;
            b.window.rect = virtual_rect(*r);
            b.window.style = 0;
            b.window.background = None;
            if b.window.name.is_empty() {
                b.window.name = "mrow".into();
            }
            template.get_or_insert_with(|| b.clone());
            out.items.push(b);
            continue;
        }
        // Kept for what they say (drawn here): the team's emblem, the game
        // type and its line, the map's name.
        let r = &it.window.rect;
        let tag = if it.ty != item_type::BUTTON && r.horz_align == 1 && r.w >= 100.0 && (r.w - r.h).abs() < 1.0 && !label_of(fe, it).is_empty() {
            "m_faction"
        } else {
            match it.text_exp.as_slice() {
                [_, Token::Op(op::GAMETYPENAME), ..] => "m_gametype",
                [_, Token::Op(op::GAMETYPEDESCRIPTION), ..] => "m_gamedesc",
                e if e.iter().any(|t| matches!(t, Token::Str(s) if s.contains("mapsTable"))) => "m_map",
                _ => continue,
            }
        };
        let mut k = it.clone();
        if tag == "m_faction" {
            k.text = label_of(fe, it);
        }
        k.window.name = tag.into();
        k.window.rect = VRect { w: 0.0, h: 0.0, ..k.window.rect };
        k.text_scale = 0.0;
        out.items.push(k);
    }
    let template = template?;
    out.items.push(cac::back_item(&template, &classic.on_esc));
    Some(out)
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name.starts_with(PREFIX))
}

/// A kept item's text, if it's showing.
fn kept(fe: &Frontend, om: &OpenMenu, tag: &str) -> Option<String> {
    om.menu.items.iter().enumerate().find(|(i, it)| it.window.name == tag && fe.item_visible(om, *i)).map(|(_, it)| label_of(fe, it)).filter(|s| !s.is_empty())
}

/// The emblem for a team's name (a faction item's material).
fn emblem(fe: &Frontend, om: &OpenMenu, team: &str) -> Option<String> {
    let it = om.menu.items.iter().find(|it| it.window.name == "m_faction" && it.text.eq_ignore_ascii_case(team))?;
    // CoD4's 128 px faction emblems, by the team's name.
    let known = match team.to_ascii_lowercase().as_str() {
        "s.a.s." => Some("faction_128_sas"),
        "marines" => Some("faction_128_usmc"),
        "opfor" => Some("faction_128_arab"),
        "spetsnaz" => Some("faction_128_ussr"),
        _ => None,
    };
    let m = match known {
        Some(m) => m.to_owned(),
        None if it.material_exp.is_empty() => it.window.background.clone()?,
        None => eval(&it.material_exp, fe).text(),
    };
    (!m.is_empty()).then_some(m)
}

fn row_icon(label: &str) -> Option<usize> {
    Some(match label {
        "Choose Class" | "Create a Class" => icons::RIFLE,
        "Change Team" => icons::PEOPLE,
        "Supply Drops" => icons::CRATE,
        "Controls" => icons::PAD,
        "Options" => icons::COG,
        "Auto-Assign" => icons::BOT,
        _ if label.starts_with("Leave") || label.starts_with("End") => icons::HOME,
        _ => return None,
    })
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    let Some(key) = om.menu.window.name.strip_prefix(PREFIX) else { return };
    let Some(kind) = kind_of(key) else { return };
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    // The match, dimmed; darker on the left behind the list.
    let b = k.v.bleed();
    k.fill(b, t::hex(0x050607, 0.8));
    k.grad_h(R::new(b.x, b.y, b.w * 0.55, b.h), t::hex(0x050607, 1.0), 0.75, 0.0);
    k.grad_v(R::new(b.x, b.y, b.w, 200.0), t::hex(0x050607, 1.0), 0.7, 0.0);
    k.grad_v(R::new(b.x, b.bottom() - 220.0, b.w, 220.0), t::hex(0x050607, 1.0), 0.0, 0.8);

    let hq = key == super::super::hq::PAUSE_MENU;
    let game_type = kept(fe, om, "m_gametype").unwrap_or_default();
    let map = kept(fe, om, "m_map").unwrap_or_default();
    let team = kept(fe, om, "m_faction");
    let (crumb, title) = match kind {
        Kind::Pause if hq => ("Headquarters".to_owned(), "Paused".to_owned()),
        Kind::Pause => (if map.is_empty() { game_type.clone() } else { format!("{game_type}  \u{b7}  {map}") }, "Paused".to_owned()),
        Kind::Team => (game_type.clone(), "Choose Team".to_owned()),
        Kind::Class => (team.clone().unwrap_or_else(|| if game_type.is_empty() { "Match".to_owned() } else { game_type.clone() }), "Choose Class".to_owned()),
    };
    header(&mut k, &crumb, &title);

    let focused = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| Some((*i, om.menu.items.get(*i)?)));
    let hot = focused.map(|(i, _)| i);
    for it in om.menu.items.iter().filter(|it| it.window.name.starts_with("msec_")) {
        let r = design(&it.window.rect);
        let title = match (kind, it.window.name.as_str()) {
            (Kind::Class, "msec_0") => "Default classes",
            (Kind::Class, _) => "Custom classes",
            (Kind::Team, _) => "Teams",
            (Kind::Pause, _) if hq => "Headquarters",
            (Kind::Pause, _) => "Match",
        };
        section(&mut k, r.x, r.y, r.w, title);
    }
    // The rows (and which class each picks).
    let mut shown_class = None;
    let mut first_class = None;
    for (i, it) in om.menu.items.iter().enumerate() {
        if it.ty != item_type::BUTTON || it.window.name == "cac_back" || !fe.item_visible(om, i) {
            continue;
        }
        let r = design(&it.window.rect);
        let on = hot == Some(i);
        let class = class_of(&it.action);
        let label = match (&class, kind) {
            (Some(c), Kind::Class) if c.starts_with("custom") => fe.class_loadout(c).map_or_else(|| it.text.clone(), |l| l.name),
            _ => it.text.clone(),
        };
        menu_row(&mut k, r, &label, on, None);
        let right = r.right() - if on { 56.0 } else { 20.0 };
        if kind == Kind::Class {
            if let Some(g) = class.as_deref().and_then(|c| fe.class_loadout(c)).and_then(|l| l.guns.into_iter().next()) {
                k.text_mid(right, r.cy(), Type { size: 18.0, ..t::CAPTION }, &g.name, t::INK_MUTE, 2);
            }
        } else if let Some(icon) = row_icon(&it.text) {
            let c = if on { t::INK } else { t::with_alpha(t::INK_DIM, 0.6) };
            k.icon(MENU_ICONS, icon, R::new(right - 24.0 - if on { 22.0 } else { 0.0 }, r.cy() - 12.0, 24.0, 24.0), c);
        }
        if on && class.is_some() {
            shown_class = class.clone();
        }
        first_class = first_class.or(class);
    }

    match kind {
        Kind::Class => {
            if let Some(class) = remembered(shown_class).or(first_class) {
                let over = if class.starts_with("custom") { "Custom class" } else { "Default class" };
                let key = if class.starts_with("custom") { 9200 } else { 9100 };
                cac::class_panel(&mut k, PANEL, &class, over, key);
            }
        }
        Kind::Team => {
            let focused_team = focused.map(|(_, it)| it.text.clone()).filter(|l| emblem(fe, om, l).is_some());
            match focused_team {
                Some(name) => team_panel(&mut k, fe, om, &name, team.as_deref() == Some(name.as_str())),
                None => match team.as_deref() {
                    Some(name) => team_panel(&mut k, fe, om, name, true),
                    None => match_panel(&mut k, fe, om, &game_type, &map, None),
                },
            }
        }
        Kind::Pause => match_panel(&mut k, fe, om, &game_type, &map, team.as_deref()),
    }

    cac::back_button(&mut k, focused.is_some_and(|(_, it)| it.window.name == "cac_back"));
    let back = if kind == Kind::Pause { "Resume" } else { "Back" };
    footer(&mut k, "", &[(Button::Confirm, "Select"), (Button::Back, back)]);
}

/// The class last focused (it stays shown when the mouse leaves the rows).
fn remembered(now: Option<String>) -> Option<String> {
    static SHOWN: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let mut s = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    if now.is_some() {
        *s = now;
    }
    s.clone()
}

/// The match: game type and its line, the map, the minimap, your team.
fn match_panel(k: &mut Kit, fe: &Frontend, om: &OpenMenu, game_type: &str, map: &str, team: Option<&str>) {
    let p = PANEL;
    grid_panel(k, p, 84.0, 1.0);
    let (x, w) = (p.x + 44.0, p.w - 88.0);
    k.text(x, p.y + 36.0, t::LABEL, "Game type", t::ACCENT, 0);
    k.text(x, p.y + 60.0, Type { size: 52.0, ..t::DISPLAY }, game_type, t::INK, 0);
    let desc = kept(fe, om, "m_gamedesc").unwrap_or_default();
    let mm = 380.0;
    k.para(x, p.y + 136.0, w - mm - 40.0, Type { size: 22.0, ..t::BODY }, &desc, t::INK_DIM, 1.3);
    // The map, north up.
    let map_r = R::new(p.right() - 44.0 - mm, p.y + 44.0, mm, mm);
    let name = fe.dvar("mapname");
    let name = if name.is_empty() { fe.dvar("ui_mapname") } else { name };
    k.fill(map_r, t::hex(0x000000, 0.5));
    if !name.is_empty() {
        k.pic(map_r, &format!("compass_map_{name}"), [1.0; 4]);
    }
    k.frame(map_r, 1.0, t::LINE);
    if !map.is_empty() {
        k.text(map_r.x, map_r.bottom() + 14.0, t::LABEL, map, t::INK_DIM, 0);
    }
    // Your team.
    if let Some(team) = team {
        let y = p.bottom() - 200.0;
        k.slice3(R::new(x, y - 30.0, w, 14.0), "next:panels/divider", Vec2::new(354.0, 28.0), 120.0, 120.0, [1.0; 4]);
        if let Some(m) = emblem(fe, om, team) {
            icon_box(k, R::new(x, y, 140.0, 140.0));
            k.pic(R::new(x + 10.0, y + 10.0, 120.0, 120.0), &m, [1.0; 4]);
        }
        k.text(x + 170.0, y + 30.0, t::LABEL, "Your team", t::INK_DIM, 0);
        k.text(x + 170.0, y + 54.0, Type { size: 44.0, ..t::H1 }, team, t::INK, 0);
    }
}

/// A team: its emblem large, and whether you're on it.
fn team_panel(k: &mut Kit, fe: &Frontend, om: &OpenMenu, team: &str, yours: bool) {
    let p = PANEL;
    grid_panel(k, p, 84.0, 1.0);
    let x = p.x + 44.0;
    k.text(x, p.y + 36.0, t::LABEL, if yours { "Your team" } else { "Join" }, t::ACCENT, 0);
    k.text(x, p.y + 60.0, Type { size: 52.0, ..t::DISPLAY }, team, t::INK, 0);
    if let Some(m) = emblem(fe, om, team) {
        let s = 420.0;
        k.pic(R::new(p.cx() - s * 0.5, p.y + 190.0, s, s), &m, [1.0; 4]);
    }
}
