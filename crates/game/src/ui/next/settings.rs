//! Options and Controls in the new style: one screen per settings page
//! ([`crate::settings::PAGES`]), each standing in for the CoD4 menu that
//! page took over. The pages are a column list on the left, as the main
//! menu's, and the page's settings rows on the right: choices step with
//! chevrons, sliders have a bar, binds show their keys. A line about the
//! focused setting sits under them. LB / RB (Q / E) go through the pages.
//!
//! The rows are the settings' own items ([`super::super::settings_menu`]),
//! so stepping, sliders, binding and the quality preset work as they do in
//! the classic pages.

use super::super::expr::Env;
use super::super::{Frontend, MenuInput, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, HUD_ICONS, Kit, MENU_ICONS, R, View, icons};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use crate::gamepad::ActiveDevice;
use crate::settings::{self, Kind, Page, Setting};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Window as MenuWindow, flags, item_type};
use std::sync::atomic::{AtomicBool, Ordering};

pub const PREFIX: &str = "next_settings|";

/// The list on the left: its groups and their pages (by CoD4 menu).
const GROUPS: [(&str, &[&str]); 2] = [
    ("Options", &["options_graphics", "options_graphics_texture", "options_sound", "options_game"]),
    ("Controls", &["options_look", "options_move", "options_shoot", "options_misc", "controls_multi"]),
];

const NAV_TOP: f32 = 176.0;
const NAV_H: f32 = 48.0;
const NAV_PITCH: f32 = 50.0;

/// The settings list: its x and width, the rows' top and bottom.
const LIST_X: f32 = 826.0;
const LIST_W: f32 = 998.0;
const ROWS_TOP: f32 = 212.0;
const ROWS_BOTTOM: f32 = 860.0;
/// The line about the focused row.
const DESC: R = R::new(LIST_X, 878.0, LIST_W, 72.0);
/// A row's value part: this far in from its right, and wide.
const VALUE_IN: f32 = 430.0;
const VALUE_W: f32 = 406.0;
/// A slider's bar in the value part: from its left, and short of its right
/// (where the number goes).
const BAR_FROM: f32 = 12.0;
const BAR_SHORT: f32 = 104.0;

/// Next page from LB / RB: focus its first row rather than the list.
static TO_ROWS: AtomicBool = AtomicBool::new(false);

fn page(menu: &str) -> Option<&'static Page> {
    settings::PAGES.iter().find(|p| p.menu == menu)
}

/// The pages in list order.
fn pages() -> impl Iterator<Item = &'static str> {
    GROUPS.iter().flat_map(|(_, list)| list.iter().copied()).filter(|m| page(m).is_some())
}

/// Whether this CoD4 menu is a settings page the screen stands in for.
pub(in crate::ui) fn wanted(key: &str) -> bool {
    page(key).is_some()
}

/// The Options and Controls lists open their first page.
pub(in crate::ui) fn redirect(key: &str) -> Option<&'static str> {
    match key {
        "main_options" => Some(GROUPS[0].1[0]),
        "main_controls" => Some(GROUPS[1].1[0]),
        _ => None,
    }
}

fn group_of(menu: &str) -> usize {
    GROUPS.iter().position(|(_, list)| list.contains(&menu)).unwrap_or(0)
}

/// A page's rows (test features' only in builds that have them).
fn rows_of(p: &Page) -> Vec<Setting> {
    p.rows.iter().filter(|s| cfg!(feature = "raytracing") || s.dvar != "r_lighting").copied().collect()
}

/// The list's section headers (y, title) and rows (page menu or "reset",
/// rect).
fn nav_layout(group: usize) -> (Vec<(f32, &'static str)>, Vec<(&'static str, R)>) {
    let (x, w) = (col_x(0), cols(4));
    let mut heads = Vec::new();
    let mut rows = Vec::new();
    let mut y = NAV_TOP;
    for (title, list) in GROUPS {
        heads.push((y, title));
        y += 34.0;
        for m in list.iter().copied().filter(|m| page(m).is_some()) {
            rows.push((m, R::new(x, y, w, NAV_H)));
            y += NAV_PITCH;
        }
        y += 22.0;
    }
    heads.push((y, "Defaults"));
    y += 34.0;
    rows.push((if group == 0 { "reset_options" } else { "reset_controls" }, R::new(x, y, w, NAV_H)));
    (heads, rows)
}

/// Row `i` of `n`.
fn row_rect(i: usize, n: usize) -> R {
    let pitch = ((ROWS_BOTTOM - ROWS_TOP) / n.max(1) as f32).min(54.0);
    R::new(LIST_X, ROWS_TOP + i as f32 * pitch, LIST_W, pitch - 6.0)
}

fn value_part(r: R) -> R {
    R::new(r.right() - VALUE_IN, r.y, VALUE_W, r.h)
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

/// The screen for page `key`, from its classic menu (kept for its scripts).
pub(in crate::ui) fn menu(classic: &Menu, key: &str) -> Menu {
    let Some(p) = page(key) else { return classic.clone() };
    let mut m = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    m.window.name = format!("{PREFIX}{key}");
    m.on_esc = "\"close\" \"self\" ; \"settingsBack\" ; ".into();
    let group = group_of(key);
    let (_, nav) = nav_layout(group);
    for (target, r) in nav {
        let item = match target {
            "reset_options" | "reset_controls" => {
                let which = target.trim_start_matches("reset_");
                button(r, format!("nav_{target}"), "Reset to Defaults", format!("\"play\" \"mouse_click\" ; \"settingsReset\" \"{which}\" ; "))
            }
            _ if target == key => button(r, format!("nav_{target}"), page(target).map_or("", |p| p.title), "\"play\" \"mouse_click\" ; ".into()),
            _ => button(
                r,
                format!("nav_{target}"),
                page(target).map_or("", |p| p.title),
                format!("\"play\" \"mouse_click\" ; \"close\" \"self\" ; \"open\" \"{target}\" ; "),
            ),
        };
        m.items.push(item);
    }
    let rows = rows_of(p);
    for (i, s) in rows.iter().enumerate() {
        let mut it = Item::default();
        super::super::settings_menu::value_item(&mut it, s, 100 + i as i32);
        it.window = MenuWindow { rect: virtual_rect(row_rect(i, rows.len())), dynamic_flags: flags::VISIBLE, name: format!("set_{i}"), ..MenuWindow::default() };
        // The label stays (the shot harness finds rows by it), undrawn.
        it.text = s.label.into();
        it.text_scale = 0.0;
        m.items.push(it);
    }
    let template = m.items[0].clone();
    m.items.push(cac::back_item(&template, &m.on_esc));
    m
}

/// The item a page opens focused on: its row in the list, or its first
/// setting after LB / RB.
pub(in crate::ui) fn first_focus(menu: &Menu) -> Option<usize> {
    let key = menu.window.name.strip_prefix(PREFIX)?;
    let want = if TO_ROWS.swap(false, Ordering::Relaxed) { "set_0".to_owned() } else { format!("nav_{key}") };
    menu.items.iter().position(|it| it.window.name == want)
}

/// A slider row's bar, as fractions of the row's width; `None` for the
/// classic pages' items.
pub(in crate::ui) fn slider_span(item: &Item) -> Option<(f32, f32)> {
    item.window.name.starts_with("set_").then(|| {
        let x0 = LIST_W - VALUE_IN + BAR_FROM;
        let x1 = LIST_W - VALUE_IN + VALUE_W - BAR_SHORT;
        (x0 / LIST_W, x1 / LIST_W)
    })
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name.starts_with(PREFIX))
}

pub(in crate::ui) fn build(app: &mut App) {
    app.add_systems(Update, page_keys.before(MenuInput).run_if(resource_exists::<Frontend>));
}

/// LB / RB (Q / E): the previous or next page.
fn page_keys(mut fe: ResMut<Frontend>, keys: Res<ButtonInput<KeyCode>>, active: Res<ActiveDevice>, pads: Query<&Gamepad>) {
    let Some(top) = fe.stack.last() else { return };
    let Some(key) = top.menu.window.name.strip_prefix(PREFIX).map(str::to_owned) else { return };
    if fe.capturing.is_some() || fe.editing.is_some() {
        return;
    }
    let pad = active.entity.and_then(|e| pads.get(e).ok());
    let pressed = |k: KeyCode, b: GamepadButton| keys.just_pressed(k) || pad.is_some_and(|p| p.just_pressed(b));
    let dir = if pressed(KeyCode::KeyQ, GamepadButton::LeftTrigger) {
        -1
    } else if pressed(KeyCode::KeyE, GamepadButton::RightTrigger) {
        1
    } else {
        return;
    };
    let all: Vec<&str> = pages().collect();
    let Some(at) = all.iter().position(|m| *m == key) else { return };
    let next = all[(at as i32 + dir).rem_euclid(all.len() as i32) as usize];
    TO_ROWS.store(true, Ordering::Relaxed);
    let owner = top.name.clone();
    fe.run(&format!("\"play\" \"mouse_click\" ; \"close\" \"{owner}\" ; \"open\" \"{next}\" ; "), &owner);
}

/// A page's icon in the list (sheet, index).
fn page_icon(menu: &str) -> Option<(&'static str, usize)> {
    Some(match menu {
        "options_graphics" => (MENU_ICONS, icons::COG),
        "options_graphics_texture" => (MENU_ICONS, icons::STAR),
        "options_sound" => (HUD_ICONS, 14),
        "options_game" => (MENU_ICONS, icons::HELMET),
        "options_look" => (MENU_ICONS, icons::CROSSHAIR),
        "options_move" => (MENU_ICONS, icons::KEYBOARD),
        "options_shoot" => (MENU_ICONS, icons::RIFLE),
        "options_misc" => (MENU_ICONS, icons::LINK),
        "controls_multi" => (MENU_ICONS, icons::PAD),
        _ => return None,
    })
}

/// What a page is for (the line under the rows while its list row is
/// focused).
fn page_about(menu: &str) -> &'static str {
    match menu {
        "options_graphics" => "Display, frame rate, field of view and how the picture is lit and graded.",
        "options_graphics_texture" => "Shadows, anti-aliasing, effects and the detail drawn.",
        "options_sound" => "Volumes for everything you hear.",
        "options_game" => "The HUD, the scope, your gun's feel and the menus' style.",
        "options_look" => "Mouse sensitivity and leaning.",
        "options_move" => "Movement keys, and how crouch, prone and sprint behave.",
        "options_shoot" => "Firing, aiming, weapons, grenades and kill streaks.",
        "options_misc" => "Use, night vision, the scoreboard and third person.",
        "controls_multi" => "Stick speeds, deadzones, aim assist, vibration and button icons.",
        "reset_options" => "Puts every Options setting back to its default.",
        "reset_controls" => "Puts every key and Controls setting back to its default.",
        _ => "",
    }
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    let Some(key) = om.menu.window.name.strip_prefix(PREFIX) else { return };
    let Some(p) = page(key) else { return };
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    backdrop(&mut k, 0.5);
    header(&mut k, GROUPS[group_of(key)].0, p.title);
    let focused = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i));
    let hot = focused.map(|it| it.window.name.as_str());

    // The pages.
    let (heads, nav) = nav_layout(group_of(key));
    for (y, title) in heads {
        section(&mut k, col_x(0), y, cols(4), title);
    }
    for (target, r) in nav {
        let name = format!("nav_{target}");
        let on = hot == Some(name.as_str());
        let label = page(target).map_or("Reset to Defaults", |p| p.title);
        menu_row(&mut k, r, label, on, None);
        if target == key && !on {
            k.fill(R::new(r.x, r.y + 8.0, t::FOCUS_BAR, r.h - 16.0), t::ACCENT);
            k.text_mid(r.x + 24.0, r.cy(), Type { size: 28.0, ..t::ROW_TEXT }, label, t::ACCENT, 0);
        }
        if let Some((sheet, i)) = page_icon(target) {
            let x = r.right() - if on { 78.0 } else { 46.0 };
            let c = if on { t::INK } else if target == key { t::ACCENT } else { t::with_alpha(t::INK_DIM, 0.6) };
            k.icon(sheet, i, R::new(x, r.cy() - 12.0, 24.0, 24.0), c);
        }
    }

    // The page's settings.
    grid_panel(&mut k, R::new(LIST_X - 12.0, ROWS_TOP - 14.0, LIST_W + 24.0, DESC.bottom() - ROWS_TOP + 24.0), 70.0, 1.0);
    section_icon(&mut k, LIST_X, NAV_TOP, LIST_W, p.title, page_icon(key).filter(|(s, _)| *s == MENU_ICONS).map(|(_, i)| i));
    for it in om.menu.items.iter().filter(|it| it.window.name.starts_with("set_")) {
        let Some(s) = Frontend::setting_of(it) else { continue };
        let i: usize = it.window.name["set_".len()..].parse().unwrap_or(0);
        let n = om.menu.items.iter().filter(|it| it.window.name.starts_with("set_")).count();
        setting_row(&mut k, fe, it, s, row_rect(i, n), hot == Some(it.window.name.as_str()));
    }

    // About the focused row.
    let about = match focused {
        Some(it) if it.window.name.starts_with("set_") => fe.setting_desc(),
        Some(it) => page_about(it.window.name.trim_start_matches("nav_")).to_owned(),
        None => page_about(key).to_owned(),
    };
    if !about.is_empty() {
        k.hline(DESC.x + 24.0, DESC.right() - 24.0, DESC.y - 4.0, 1.0, t::LINE);
        k.para(DESC.x + 24.0, DESC.y + 14.0, DESC.w - 48.0, Type { size: 20.0, ..t::BODY }, &about, t::INK_DIM, 1.3);
    }

    cac::back_button(&mut k, hot == Some("cac_back"));
    let on_row = focused.is_some_and(|it| it.window.name.starts_with("set_"));
    let binding = focused.and_then(Frontend::setting_of).is_some_and(|s| matches!(s.kind, Kind::Bind(_)));
    let mut prompts = vec![(Button::PrevTab, ""), (Button::NextTab, "Page")];
    if fe.capturing.is_some() {
        prompts = vec![(Button::Back, "Cancel")];
    } else {
        prompts.push((Button::Confirm, if binding { "Bind" } else if on_row { "Change" } else { "Select" }));
        prompts.push((Button::Back, "Back"));
    }
    footer(&mut k, "", &prompts);
}

/// A setting's row: its label, and its value as a choice, a bar or keys.
fn setting_row(k: &mut Kit, fe: &Frontend, it: &Item, s: &Setting, r: R, hot: bool) {
    if hot {
        k.grad_h(r, t::ACCENT, 0.2, 0.0);
        k.fill(R::new(r.x, r.y, t::FOCUS_BAR, r.h), t::ACCENT);
    } else {
        k.hline(r.x + 24.0, r.right(), r.bottom() + 3.0, 1.0, t::HAIRLINE);
    }
    let ink = if hot { t::INK } else { t::INK_DIM };
    let size = (r.h * 0.5).clamp(20.0, 26.0);
    k.text_mid(r.x + 24.0, r.cy(), Type { size, ..t::ROW_TEXT }, s.label, ink, 0);
    let v = value_part(r);
    let value = fe.dvar(&it.dvar);
    let regular = Type { cut: super::font::Cut::Regular, size, ..t::ROW_TEXT };
    match s.kind {
        Kind::Toggle | Kind::Choice(_) => {
            let list: Vec<(&str, &str)> = match s.kind {
                Kind::Choice(list) => list.to_vec(),
                _ => vec![("Off", "0"), ("On", "1")],
            };
            let at = list.iter().position(|(_, v)| v.eq_ignore_ascii_case(value.trim())).unwrap_or(0);
            let arrows = if hot { t::ACCENT } else { t::with_alpha(t::INK_DIM, 0.45) };
            if list.len() > 1 {
                k.chevron(Vec2::new(v.x + 14.0, r.cy()), 16.0, 2, 2.0, arrows);
                k.chevron(Vec2::new(v.right() - 14.0, r.cy()), 16.0, 0, 2.0, arrows);
            }
            k.text_mid(v.cx(), r.cy() - 2.0, regular, list[at].0, ink, 1);
            // Where the value sits among the choices.
            let n = list.len();
            if (2..=8).contains(&n) {
                let (pw, gap) = (22.0, 4.0);
                let x0 = v.cx() - (n as f32 * pw + (n - 1) as f32 * gap) * 0.5;
                for i in 0..n {
                    let c = if i == at { if hot { t::ACCENT } else { t::ACCENT_DIM } } else { t::LINE };
                    k.fill(R::new(x0 + i as f32 * (pw + gap), r.bottom() - 6.0, pw, 2.0), c);
                }
            }
        }
        Kind::Slider { min, max, .. } => {
            let x: f32 = value.trim().parse().unwrap_or(min);
            let f = ((x - min) / (max - min).max(1e-6)).clamp(0.0, 1.0);
            let (x0, x1) = (v.x + BAR_FROM, v.right() - BAR_SHORT);
            k.fill(R::new(x0, r.cy() - 2.0, x1 - x0, 4.0), t::LINE);
            k.fill(R::new(x0, r.cy() - 2.0, (x1 - x0) * f, 4.0), if hot { t::ACCENT } else { t::ACCENT_DIM });
            k.fill(R::new(x0 + (x1 - x0) * f - 3.0, r.cy() - 11.0, 6.0, 22.0), ink);
            let shown = fe.setting_value(it).unwrap_or_default();
            k.text_mid(v.right() - 12.0, r.cy(), regular, &shown, ink, 2);
        }
        Kind::Bind(action) => {
            let listening = fe.capturing == Some(action);
            let keys = fe.setting_value(it).unwrap_or_default();
            let box_r = R::new(v.x + 12.0, r.y + 6.0, v.w - 24.0, r.h - 12.0);
            if listening {
                let pulse = 0.55 + 0.45 * (k.now * 6.0).sin().abs();
                k.fill(box_r, t::with_alpha(t::ACCENT, 0.12 * pulse));
                k.frame(box_r, 1.0, t::with_alpha(t::ACCENT, pulse));
                k.text_mid(box_r.cx(), r.cy(), regular, "Press a key...", t::ACCENT, 1);
            } else {
                k.frame(box_r, 1.0, if hot { t::LINE_STRONG } else { t::HAIRLINE });
                let shown = if keys.trim().is_empty() { "Unbound" } else { keys.as_str() };
                k.text_mid(box_r.cx(), r.cy(), regular, shown, if keys.trim().is_empty() { t::INK_MUTE } else { ink }, 1);
            }
        }
    }
}
