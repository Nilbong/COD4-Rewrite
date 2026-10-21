//! The new Create a Class: a class select screen (CoD4's `pc_cac_popup`)
//! listing the custom classes beside everything about the one focused, and
//! the class editor (CoD4's `menu_cac_<class>`) laid out as cards.
//!
//! Like the main menu ([`super::home`]), both keep the classic menus' own
//! buttons (as the supply drops, Black Ops and World at War dress them),
//! moved to the new places with their text cleared, so every action and the
//! weapon, perk, camo and attachment popups behind them stay the classic
//! menu code's. Only the drawing is this file's.

use super::super::expr::{Env, eval};
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Statement, Window as MenuWindow, flags, item_type};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

/// The menus that stand in for `pc_cac_popup` and `menu_cac_<class>`.
pub const SELECT: &str = "next_cac_select";
pub const EDITOR: &str = "next_cac_edit";

/// The custom classes' menus, slot by slot.
const CLASSES: [&str; 5] = ["assault", "specops", "heavygunner", "demolitions", "sniper"];

/// The custom class slot (0..5) of a class menu's key (`menu_cac_<class>`).
pub(super) fn slot_of(key: &str) -> Option<usize> {
    CLASSES.iter().position(|c| key == format!("menu_cac_{c}"))
}

/// A custom class's first stat (200, 210, ...).
fn base(slot: usize) -> i32 {
    200 + 10 * slot as i32
}

// --- Shared drawing -------------------------------------------------------

/// The loadout backdrop.
pub(super) fn loadout_backdrop(k: &mut Kit) {
    let b = k.v.bleed();
    k.fill(b, t::hex(0x050607, 1.0));
    const ASPECT: f32 = 1672.0 / 941.0;
    let (w, h) = if b.w / b.h > ASPECT { (b.w, b.w / ASPECT) } else { (b.h * ASPECT, b.h) };
    k.pic(R::new(b.cx() - w * 0.5, b.cy() - h * 0.5, w, h), "next:bg_loadout", [1.0; 4]);
    k.grad_h(R::new(b.x, b.y, b.w * 0.5, b.h), t::hex(0x050607, 1.0), 0.8, 0.0);
    k.grad_v(R::new(b.x, b.bottom() - 260.0, b.w, 260.0), t::hex(0x050607, 1.0), 0.0, 0.85);
    k.grad_v(R::new(b.x, b.y, b.w, 200.0), t::hex(0x050607, 1.0), 0.7, 0.0);
}

/// A statstable item's name and picture.
fn item(fe: &Frontend, reference: &str) -> (String, String) {
    let name = fe.table_lookup("mp/statstable.csv", 4, reference, 3);
    let name = if name.is_empty() { reference.to_owned() } else { fe.assets.localize(&format!("@{name}")) };
    (name, fe.table_lookup("mp/statstable.csv", 4, reference, 6))
}

/// A statstable item's description.
fn description(fe: &Frontend, reference: &str) -> String {
    let d = fe.table_lookup("mp/statstable.csv", 4, reference, 7);
    if d.is_empty() { String::new() } else { fe.assets.localize(&format!("@{d}")) }
}

fn attachment_name(fe: &Frontend, a: &str) -> String {
    let name = fe.table_lookup("mp/attachmenttable.csv", 4, a, 3);
    if name.is_empty() {
        match a {
            "reflex" => "Red Dot Sight".into(),
            "silencer" => "Silencer".into(),
            "acog" => "ACOG Scope".into(),
            "grip" => "Grip".into(),
            "gl" => "Grenade Launcher".into(),
            _ => a.to_owned(),
        }
    } else {
        fe.assets.localize(&format!("@{name}"))
    }
}

fn camo_name(camo: usize) -> Option<&'static str> {
    Some(match camo & 0xffff {
        0 => return None,
        1 => "Desert",
        2 => "Woodland",
        3 => "Digital",
        4 => "Red Tiger",
        5 => "Blue Tiger",
        6 => "Gold",
        200 => "Platinum",
        201 => "Diamond",
        300.. => "Custom Camo",
        _ => "Camo",
    })
}

/// The class's perk 1..3 and special grenade (statstable references),
/// straight from its stats (perk 1 may be a weapon perk such as the grenade
/// launcher).
fn class_items(fe: &Frontend, slot: usize) -> [Option<String>; 4] {
    let b = base(slot);
    let at = |k: i32| {
        let r = fe.table_lookup("mp/statstable.csv", 0, &fe.stat(b + k).to_string(), 4);
        (!r.is_empty() && r != "specialty_null" && r != "none").then_some(r)
    };
    [at(5), at(6), at(7), at(8)]
}

/// A chip: a small labelled box; returns its width.
fn chip(k: &mut Kit, x: f32, y: f32, s: &str, accent: bool) -> f32 {
    let ty = Type { size: 18.0, ..t::CAPTION };
    let w = k.measure(ty, s) + 24.0;
    let r = R::new(x, y, w, 32.0);
    k.fill(r, t::hex(0x000000, 0.45));
    k.frame(r, 1.0, if accent { t::ACCENT } else { t::LINE });
    k.text_mid(r.x + 12.0, r.cy(), ty, s, t::INK, 0);
    w
}

/// Chips in a row from `x`, wrapping at `right`; returns the height used.
fn chips(k: &mut Kit, x: f32, y: f32, right: f32, list: &[(String, bool)]) -> f32 {
    let mut at = x;
    let mut row = 0.0;
    for (s, accent) in list {
        let w = k.measure(Type { size: 18.0, ..t::CAPTION }, s) + 24.0;
        if at + w > right && at > x {
            at = x;
            row += 40.0;
        }
        chip(k, at, y + row, s, *accent);
        at += w + 8.0;
    }
    if list.is_empty() { 0.0 } else { row + 32.0 }
}

/// The weapon's attachments, IR laser and camo as chips.
fn gun_chips(fe: &Frontend, gun: &crate::loadout::Gun) -> Vec<(String, bool)> {
    let atts = gun.spec.split_once(':').map_or("", |(_, a)| a);
    let mut out: Vec<(String, bool)> = atts.split('+').filter(|a| !a.is_empty() && *a != crate::laser::ATTACHMENT).map(|a| (attachment_name(fe, a), false)).collect();
    if crate::laser::fitted(&gun.spec) {
        out.push(("IR Laser".into(), false));
    }
    if let Some(c) = camo_name(gun.camo) {
        out.push((c.into(), true));
    }
    out
}

/// One of CoD4's weapon attributes (`mp/attributesTable.csv` column 2..6:
/// accuracy, damage, range, fire rate, mobility), 0..1, for the weapon
/// alone and with its first attachment.
fn attribute(fe: &Frontend, weapon: &str, attachment: Option<&str>, col: i32) -> (f32, f32) {
    let get = |key: &str| fe.table_lookup("mp/attributestable.csv", 1, key, col).trim().parse::<f32>().ok().map(|v| v / 100.0);
    let alone = get(weapon).unwrap_or(0.0);
    // (Some attachments' rows are all zero: the weapon's own then.)
    let with = attachment.and_then(|a| get(&format!("{weapon}_{a}"))).filter(|v| *v > 0.0).unwrap_or(alone);
    (alone, with)
}

/// The five attribute bars for a gun spec.
pub(super) fn stat_bars(k: &mut Kit, x: f32, y: f32, w: f32, spec: &str) {
    stat_bars_vs(k, x, y, w, spec, None);
}

/// A gun spec's five attributes, 0..1: the weapon's, moved by each of its
/// attachments' rows (the rows that have values).
fn spec_values(fe: &Frontend, spec: &str) -> [f32; 5] {
    let (weapon, atts) = spec.split_once(':').unwrap_or((spec, ""));
    std::array::from_fn(|i| {
        let col = 2 + i as i32;
        let base = attribute(fe, weapon, None, col).0;
        // (The IR laser changes none of them.)
        let moved: f32 = atts.split('+').filter(|a| !a.is_empty() && *a != crate::laser::ATTACHMENT).map(|a| attribute(fe, weapon, Some(a), col).1 - base).sum();
        (base + moved).clamp(0.0, 1.0)
    })
}

/// The bars for `spec`, against `base` when given: green where it gains,
/// red where it loses.
pub(super) fn stat_bars_vs(k: &mut Kit, x: f32, y: f32, w: f32, spec: &str, base: Option<&str>) {
    let now = spec_values(k.fe, spec);
    let was = base.map_or(now, |b| spec_values(k.fe, b));
    for (i, label) in ["Accuracy", "Damage", "Range", "Fire rate", "Mobility"].iter().enumerate() {
        let yy = y + i as f32 * 26.0;
        k.text(x, yy, t::MICRO, label, t::INK_DIM, 0);
        let bar = R::new(x + 130.0, yy + 4.0, w - 130.0, 6.0);
        k.fill(bar, t::hex(0xffffff, 0.12));
        let (n, o) = (now[i], was[i]);
        k.fill(R::new(bar.x, bar.y, bar.w * n.min(o), bar.h), t::INK);
        if n > o + 0.001 {
            k.fill(R::new(bar.x + bar.w * o, bar.y, bar.w * (n - o), bar.h), t::ACCENT);
        } else if o > n + 0.001 {
            k.fill(R::new(bar.x + bar.w * n, bar.y, bar.w * (o - n), bar.h), t::ENEMY);
        }
        for n in 1..10 {
            k.vline(bar.x + bar.w * n as f32 / 10.0, bar.y, bar.bottom(), 2.0, t::hex(0x0b0e0f, 1.0));
        }
    }
}

/// A card: the pack's grid panel, its corner brackets lit when focused.
fn card(k: &mut Kit, r: R, focused: bool) {
    grid_panel(k, r, 52.0, if focused { 1.0 } else { 0.6 });
    if focused {
        k.brackets(r.inset(-6.0), 22.0, 3.0, t::ACCENT);
        k.fill(R::new(r.x + 12.0, r.bottom() - 4.0, r.w - 24.0, 3.0), t::ACCENT);
    }
}

/// A gun's 3D preview (its picture meanwhile).
fn gun(k: &mut Kit, r: R, key: i32, spec: &str, camo: usize) {
    let weapon = spec.split_once(':').map_or(spec, |(w, _)| w);
    let (_, picture) = item(k.fe, weapon);
    let (shown, camo) = k.fe.gun_for(key).unwrap_or((spec.to_owned(), camo));
    k.gun(r, key, &shown, camo, &picture);
}

/// The fade in.
fn ease(fe: &Frontend, at: &AtomicI64) -> f32 {
    let since = (fe.millis() - at.load(Ordering::Relaxed)).max(0) as f32 / 1000.0;
    t::ease_out(since / t::SLOW)
}

/// Whether a menu above `om` is showing (a popup): the screen dims a
/// little under it.
fn covered(fe: &Frontend, om: &OpenMenu) -> bool {
    fe.stack
        .iter()
        .skip_while(|m| !std::ptr::eq(*m, om))
        .skip(1)
        .any(|m| m.menu.visible_exp.is_empty() || eval(&m.menu.visible_exp, fe).truthy())
}

/// The label a classic button shows.
fn label_of(fe: &Frontend, it: &Item) -> String {
    let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, fe).text() };
    fe.assets.localize(&raw)
}

/// A classic button moved to `r`, its text kept (the shot harness finds it)
/// but not drawn.
fn place(mut item: Item, r: R, name: String, label: String) -> Item {
    item.window = MenuWindow { rect: virtual_rect(r), dynamic_flags: flags::VISIBLE, name, ..MenuWindow::default() };
    item.text = label;
    item.text_exp = Statement::default();
    item.text_scale = 0.0;
    item
}

/// A back button at the footer's left, for the mouse.
const BACK: R = R::new(t::SAFE_X, t::FOOTER_Y - 2.0, 150.0, 44.0);

pub(in crate::ui) fn back_button(k: &mut Kit, hot: bool) {
    let r = BACK;
    if hot {
        k.grad_h(r, t::ACCENT, 0.4, 0.0);
    }
    k.chevron(Vec2::new(r.x + 16.0, r.cy()), 16.0, 2, 2.5, if hot { t::INK } else { t::INK_DIM });
    k.text_mid(r.x + 34.0, r.cy(), Type { size: 24.0, ..t::ROW_TEXT }, "Back", if hot { t::INK } else { t::INK_DIM }, 0);
}

/// The back item: a copy of a button whose action is the menu's Esc.
pub(in crate::ui) fn back_item(template: &Item, on_esc: &str) -> Item {
    let mut it = template.clone();
    it.action = on_esc.to_owned();
    it.on_focus = String::from("\"play\" \"mouse_over\" ; ");
    it.mouse_enter = String::new();
    it.visible_exp = Statement::default();
    place(it, BACK, "cac_back".into(), "Back".into())
}

/// The focused item's name in `om`, if it's this menu's.
fn focused_name<'a>(fe: &Frontend, om: &'a OpenMenu) -> Option<&'a str> {
    fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.as_str())
}

// --- Class select -----------------------------------------------------------

static SELECT_OPENED: AtomicI64 = AtomicI64::new(i64::MIN);
static SELECT_SHOWN: AtomicUsize = AtomicUsize::new(0);

const LIST_TOP: f32 = 226.0;
const LIST_PITCH: f32 = 62.0;

fn select_row(slot: usize) -> R {
    R::new(col_x(0), LIST_TOP + slot as f32 * LIST_PITCH, cols(4), 56.0)
}

/// The class select screen built from `pc_cac_popup`.
pub(in crate::ui) fn select_menu(fe: &Frontend, classic: &Menu) -> Option<Menu> {
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = SELECT.into();
    let mut template = None;
    for it in classic.items.iter().filter(|it| it.ty == item_type::BUTTON) {
        let action = it.action.to_ascii_lowercase();
        let Some(slot) = CLASSES.iter().position(|c| action.contains(&format!("menu_cac_{c}\""))) else { continue };
        template.get_or_insert_with(|| it.clone());
        out.items.push(place(it.clone(), select_row(slot), format!("cac_slot_{slot}"), label_of(fe, it)));
    }
    let template = template?;
    out.items.push(back_item(&template, &classic.on_esc));
    Some(out)
}

pub(in crate::ui) fn select_opened(fe: &Frontend) {
    SELECT_OPENED.store(fe.millis(), Ordering::Relaxed);
}

pub(in crate::ui) fn paint_select(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != SELECT {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    loadout_backdrop(&mut k);
    let ease = ease(fe, &SELECT_OPENED);
    k.alpha = ease;
    header(&mut k, "Loadout", "Create a Class");

    let hot = focused_name(fe, om);
    let slot_of = |name: &str| name.strip_prefix("cac_slot_").and_then(|n| n.parse::<usize>().ok());
    if let Some(s) = hot.and_then(slot_of).or_else(|| super::mock_focus().and_then(|f| f.parse::<usize>().ok()).map(|n| n.saturating_sub(1))) {
        SELECT_SHOWN.store(s, Ordering::Relaxed);
    }
    let shown = SELECT_SHOWN.load(Ordering::Relaxed).min(4);

    k.shift = Vec2::new((1.0 - ease) * -24.0, 0.0);
    section_icon(&mut k, col_x(0), 192.0, cols(4), "Custom classes", Some(super::kit::icons::RIFLE));
    for (i, it) in om.menu.items.iter().enumerate() {
        let Some(slot) = slot_of(&it.window.name) else { continue };
        if !fe.item_visible(om, i) {
            continue;
        }
        let r = select_row(slot);
        let loadout = fe.class_loadout(&format!("custom{}", slot + 1));
        let name = loadout.as_ref().map_or_else(|| it.text.clone(), |l| l.name.clone());
        menu_row(&mut k, r, &name, shown == slot, None);
        if let Some(g) = loadout.as_ref().and_then(|l| l.guns.first()) {
            let right = r.right() - if shown == slot { 56.0 } else { 20.0 };
            k.text_mid(right, r.cy(), Type { size: 18.0, ..t::CAPTION }, &g.name, t::INK_MUTE, 2);
        }
    }

    // Everything about the class.
    k.shift = Vec2::new((1.0 - ease) * 24.0, 0.0);
    let p = R::new(col_x(5), 192.0, cols(7), 780.0);
    grid_panel(&mut k, p, 84.0, 1.0);
    if let Some(class) = fe.class_loadout(&format!("custom{}", shown + 1)) {
        let (x, w) = (p.x + 44.0, p.w - 88.0);
        k.text(x, p.y + 36.0, t::LABEL, &format!("Custom class {}", shown + 1), t::ACCENT, 0);
        k.text(x, p.y + 60.0, Type { size: 52.0, ..t::DISPLAY }, &class.name, t::INK, 0);
        k.slice3(R::new(x, p.y + 128.0, w, 14.0), "next:panels/divider", Vec2::new(354.0, 28.0), 120.0, 120.0, [1.0; 4]);

        // The primary: picture right, name, attachments, stats left.
        let b = base(shown);
        let mut y = p.y + 160.0;
        if let Some(g) = class.guns.first() {
            k.text(x, y, t::LABEL, "Primary", t::INK_DIM, 0);
            k.text(x, y + 22.0, Type { size: 40.0, ..t::H1 }, &g.name, t::INK, 0);
            let h = chips(&mut k, x, y + 72.0, x + w * 0.46, &gun_chips(fe, g));
            stat_bars(&mut k, x, y + 84.0 + h, w * 0.42, &g.spec);
            gun(&mut k, R::new(x + w * 0.42, y - 20.0, w * 0.58, 260.0), b + 1, &g.spec, g.camo);
            y += 250.0 + h.max(32.0) - 32.0;
        }
        // The secondary.
        if let Some(g) = class.guns.get(1) {
            k.text(x, y, t::LABEL, "Secondary", t::INK_DIM, 0);
            k.text(x, y + 22.0, Type { size: 32.0, ..t::H1 }, &g.name, t::INK, 0);
            chips(&mut k, x, y + 64.0, x + w * 0.5, &gun_chips(fe, g));
            gun(&mut k, R::new(x + w * 0.5, y - 20.0, w * 0.46, 160.0), b + 3, &g.spec, g.camo);
            y += 130.0;
        }
        // Perks and the special grenade.
        let list: Vec<(String, String)> = class_items(fe, shown)
            .into_iter()
            .enumerate()
            .filter_map(|(i, r)| Some((if i < 3 { format!("Perk {}", i + 1) } else { "Special grenade".into() }, r?)))
            .collect();
        let n = list.len().max(1) as f32;
        let cw = (w - 16.0 * (n - 1.0)) / n;
        let y = y.max(p.bottom() - 190.0);
        for (i, (over, reference)) in list.iter().enumerate() {
            let cx = x + i as f32 * (cw + 16.0);
            let (name, picture) = item(fe, reference);
            k.text(cx, y, t::MICRO, over, t::INK_DIM, 0);
            if !picture.is_empty() {
                icon_box(&mut k, R::new(cx, y + 20.0, 84.0, 84.0));
                k.pic(R::new(cx + 6.0, y + 26.0, 72.0, 72.0), &picture, [1.0; 4]);
            }
            k.para(cx, y + 108.0, cw, Type { size: 20.0, ..t::H2 }, &name, t::INK, 1.1);
        }
    }
    k.shift = Vec2::ZERO;
    back_button(&mut k, hot == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Edit Class"), (Button::Back, "Back")]);
}

// --- Class editor -----------------------------------------------------------

/// Each custom class gun's IR laser (1 on): this plus the gun's stat
/// (`200 + 10 * class + 1` primary, `+ 3` secondary).
pub(in crate::ui) const LASER_STATS: i32 = 5300;

impl Frontend {
    /// `uiScript cacLaser <1|3>`: the open class's primary's (or
    /// secondary's) IR laser, on or off. `uiScript cacLaserReset`: both off
    /// (Reset to Default).
    pub(in crate::ui) fn laser_script(&mut self, args: &[String]) -> bool {
        let Some(first) = args.first().map(|a| a.to_ascii_lowercase()) else { return false };
        if first != "caclaser" && first != "caclaserreset" {
            return false;
        }
        // (The class editor open, classic or new.)
        let Some(slot) = self.stack.iter().rev().find_map(|m| CLASSES.iter().position(|c| m.name == format!("menu_cac_{c}"))) else { return true };
        if first == "caclaserreset" {
            for k in [1, 3] {
                self.stats.set(LASER_STATS + base(slot) + k, 0);
            }
        } else {
            let k: i32 = args.get(1).and_then(|s| s.parse().ok()).filter(|k| matches!(k, 1 | 3)).unwrap_or(1);
            let stat = LASER_STATS + base(slot) + k;
            let on = self.stats.get(stat) == 0;
            self.stats.set(stat, on as i32);
        }
        self.stats.save_if_changed();
        true
    }
}

static EDITOR_OPENED: AtomicI64 = AtomicI64::new(i64::MIN);
/// The editor row last focused (its details stay shown).
static EDITOR_SHOWN: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The editor's rows, in sections: (section, item name, the classic
/// button's label, the row's label).
const EDITOR_ROWS: [(&str, &str, &str, &str); 14] = [
    ("Weapons", "cac_primary", "Primary Weapon", "Primary"),
    ("Weapons", "cac_primary_variant", "Primary Variant", "Variant"),
    ("Weapons", "cac_camo", "Camo", "Camo"),
    ("Weapons", "cac_primary_laser", "", "IR Laser"),
    ("Weapons", "cac_secondary", "Secondary Weapon", "Secondary"),
    ("Weapons", "cac_secondary_variant", "Secondary Variant", "Variant"),
    ("Weapons", "cac_secondary_laser", "", "IR Laser"),
    ("Perks", "cac_perk1", "Perk 1", "Perk 1"),
    ("Perks", "cac_perk2", "Perk 2", "Perk 2"),
    ("Perks", "cac_perk3", "Perk 3", "Perk 3"),
    ("Perks", "cac_special", "Special Grenade", "Special Grenade"),
    ("Kill Streaks", "cac_streaks", "", "Kill Streaks"),
    ("Class", "cac_rename", "Rename", "Rename"),
    ("Class", "cac_reset", "Reset to Default", "Reset to Default"),
];

const EDITOR_TOP: f32 = 176.0;
const EDITOR_PITCH: f32 = 42.0;
/// The details, right of the rows.
const DETAILS: R = R::new(826.0, 192.0, 998.0, 780.0);

/// Section headers (y, title) and each row's rect, by row.
fn editor_layout() -> (Vec<(f32, &'static str)>, Vec<R>) {
    let (mut heads, mut rows) = (Vec::new(), Vec::new());
    let mut y = EDITOR_TOP;
    let mut last = "";
    for (section, ..) in EDITOR_ROWS {
        if section != last {
            if !last.is_empty() {
                y += 18.0;
            }
            heads.push((y, section));
            y += 32.0;
            last = section;
        }
        rows.push(R::new(col_x(0), y, cols(4), EDITOR_PITCH - 4.0));
        y += EDITOR_PITCH;
    }
    (heads, rows)
}

/// Where a classic button goes, by its label.
fn editor_place(label: &str) -> Option<(R, &'static str)> {
    if label == "Back" {
        return Some((BACK, "cac_back"));
    }
    let (_, rows) = editor_layout();
    let i = EDITOR_ROWS.iter().position(|(_, _, l, _)| !l.is_empty() && (*l == label || (label == "Side Arm" && *l == "Secondary Weapon")))?;
    Some((rows[i], EDITOR_ROWS[i].1))
}

/// The class editor built from `menu_cac_<class>`.
pub(in crate::ui) fn editor_menu(fe: &Frontend, classic: &Menu) -> Option<Menu> {
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = EDITOR.into();
    // Back to the class select when it was open (closed by the editor's
    // own onOpen).
    let from_select = fe.stack.iter().any(|m| m.menu.window.name == SELECT);
    let reopen = if from_select { " \"open\" \"pc_cac_popup\" ; " } else { "" };
    out.on_esc = format!("{}{reopen}", classic.on_esc);
    for it in classic.items.iter().filter(|it| it.ty == item_type::BUTTON) {
        let label = label_of(fe, it);
        let Some((r, name)) = editor_place(label.trim()) else {
            debug!("new class editor: no place for {label:?}");
            continue;
        };
        let mut item = place(it.clone(), r, name.into(), label.trim().into());
        if name == "cac_back" {
            item.action = format!("{}{reopen}", item.action);
        }
        out.items.push(item);
    }
    let template = out.items.first()?.clone();
    if !out.items.iter().any(|it| it.window.name == "cac_back") {
        out.items.push(back_item(&template, &out.on_esc));
    }
    let (_, rows) = editor_layout();
    // The IR laser rows: on or off for this class's gun.
    for (name, k) in [("cac_primary_laser", 1), ("cac_secondary_laser", 3)] {
        if let Some(i) = EDITOR_ROWS.iter().position(|r| r.1 == name) {
            let mut it = place(template.clone(), rows[i], name.into(), "IR Laser".into());
            it.action = format!("\"play\" \"mouse_click\" ; \"uiScript\" \"cacLaser\" \"{k}\" ; ");
            it.on_focus = "\"play\" \"mouse_over\" ; ".into();
            it.mouse_enter = String::new();
            it.visible_exp = Statement::default();
            out.items.push(it);
        }
    }
    // Kill streaks: the player's three (`super::streaks`).
    if let Some(i) = EDITOR_ROWS.iter().position(|r| r.1 == "cac_streaks") {
        let mut it = place(template, rows[i], "cac_streaks".into(), "Kill Streaks".into());
        it.action = format!("\"play\" \"mouse_click\" ; \"open\" \"{}\" ; ", super::streaks::MENU);
        it.on_focus = "\"play\" \"mouse_over\" ; ".into();
        it.mouse_enter = String::new();
        it.visible_exp = Statement::default();
        out.items.push(it);
    }
    Some(out)
}

pub(in crate::ui) fn editor_opened(fe: &Frontend) {
    EDITOR_OPENED.store(fe.millis(), Ordering::Relaxed);
}

pub(in crate::ui) fn paint_editor(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != EDITOR {
        return;
    }
    let Some(slot) = CLASSES.iter().position(|c| om.name == format!("menu_cac_{c}")) else { return };
    let b = base(slot);
    // Back at the class select, this class stays shown.
    SELECT_SHOWN.store(slot, Ordering::Relaxed);
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    loadout_backdrop(&mut k);
    let ease = ease(fe, &EDITOR_OPENED);
    k.alpha = ease;
    let class_key = format!("custom{}", slot + 1);
    let class = fe.class_loadout(&class_key);
    let title = class.as_ref().map_or_else(|| format!("Custom Slot {}", slot + 1), |c| c.name.clone());
    header(&mut k, "Loadout  /  Create a Class", &title);
    // A popup over the editor: it stays, a little dimmer.
    let popup = covered(fe, om);
    let hot = if popup { None } else { focused_name(fe, om) };
    let shown = |name: &str| om.menu.items.iter().enumerate().any(|(i, it)| it.window.name == name && fe.item_visible(om, i));
    let items = class_items(fe, slot);
    let primary = class.as_ref().and_then(|c| c.guns.first());
    let secondary = class.as_ref().and_then(|c| c.guns.get(1));

    // The rows.
    k.shift = Vec2::new((1.0 - ease) * -20.0, 0.0);
    let (heads, rects) = editor_layout();
    for (y, title) in heads {
        section(&mut k, col_x(0), y, cols(4), title);
    }
    let regular = Type { cut: super::font::Cut::Regular, size: 19.0, ..t::ROW_TEXT };
    for (i, r) in rects.iter().enumerate() {
        let (_, name, _, label) = EDITOR_ROWS[i];
        let on = hot == Some(name);
        let available = shown(name);
        let ty = Type { size: 24.0, ..t::ROW_TEXT };
        if on {
            k.slice3(R::new(r.x - 4.0, r.y - 3.0, r.w + 8.0, r.h + 6.0), "next:panels/row_focus", Vec2::new(450.0, 88.0), 70.0, 80.0, [1.0; 4]);
            k.grad_h(R::new(r.x + 8.0, r.y + 4.0, r.w * 0.7, r.h - 8.0), t::ACCENT, 0.22, 0.0);
        } else {
            k.hline(r.x + 24.0, r.right(), r.bottom(), 1.0, t::HAIRLINE);
        }
        let ink = if !available { t::INK_MUTE } else if on { t::INK } else { t::INK_DIM };
        // (Variant and camo rows sit under their gun.)
        let indent = if matches!(name, "cac_primary_variant" | "cac_camo" | "cac_primary_laser" | "cac_secondary_variant" | "cac_secondary_laser") { 44.0 } else { 24.0 };
        k.text_mid(r.x + indent, r.cy(), ty, label, ink, 0);
        // What's chosen, on the right.
        let right = r.right() - 16.0;
        let perk = |n: usize| items[n].clone().map(|reference| item(fe, &reference));
        let value: Option<(String, Option<String>)> = match name {
            "cac_primary" => primary.map(|g| (g.name.clone(), None)),
            "cac_secondary" => secondary.map(|g| (g.name.clone(), None)),
            "cac_primary_variant" => primary.map(|g| (if g.variant.is_some() { "Equipped".into() } else { "None".into() }, None)),
            "cac_secondary_variant" => secondary.map(|g| (if g.variant.is_some() { "Equipped".into() } else { "None".into() }, None)),
            "cac_camo" => primary.map(|g| (camo_name(g.camo).unwrap_or("None").into(), None)),
            "cac_primary_laser" => primary.map(|g| ((if crate::laser::fitted(&g.spec) { "On" } else { "Off" }).into(), None)),
            "cac_secondary_laser" => secondary.map(|g| ((if crate::laser::fitted(&g.spec) { "On" } else { "Off" }).into(), None)),
            "cac_perk1" => perk(0).map(|(n, p)| (n, Some(p))),
            "cac_perk2" => perk(1).map(|(n, p)| (n, Some(p))),
            "cac_perk3" => perk(2).map(|(n, p)| (n, Some(p))),
            "cac_special" => perk(3).map(|(n, p)| (n, Some(p))),
            _ => None,
        };
        if name == "cac_streaks" {
            for (n, h) in super::streaks::chosen(fe).iter().rev().enumerate() {
                k.pic(R::new(right - 34.0 - n as f32 * 40.0, r.cy() - 16.0, 32.0, 32.0), h.hud_icon(), [1.0, 1.0, 1.0, if on { 1.0 } else { 0.8 }]);
            }
        } else if let Some((text, icon)) = value {
            let mut x = right;
            if let Some(p) = icon.filter(|p| !p.is_empty()) {
                k.pic(R::new(x - 30.0, r.cy() - 15.0, 30.0, 30.0), &p, [1.0, 1.0, 1.0, if on { 1.0 } else { 0.8 }]);
                x -= 40.0;
            }
            k.text_mid(x, r.cy(), regular, &text, if on { t::INK } else { t::INK_MUTE }, 2);
        } else if !available && !matches!(name, "cac_rename" | "cac_reset") {
            k.lock(Vec2::new(right - 10.0, r.cy()), 16.0, t::INK_MUTE);
        }
    }

    // The details of the row focused (or last focused).
    k.shift = Vec2::new((1.0 - ease) * 20.0, 0.0);
    let showing = {
        let mut last = EDITOR_SHOWN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = hot.filter(|h| h.starts_with("cac_") && *h != "cac_back") {
            *last = h.to_owned();
        }
        last.clone()
    };
    let p = DETAILS;
    let (x, w) = (p.x + 44.0, p.w - 88.0);
    match showing.as_str() {
        "cac_primary" | "cac_primary_variant" | "cac_camo" | "cac_primary_laser" | "cac_secondary" | "cac_secondary_variant" | "cac_secondary_laser" => {
            let second = showing.starts_with("cac_secondary");
            let (gun_key, over) = if second { (b + 3, "Secondary weapon") } else { (b + 1, "Primary weapon") };
            grid_panel(&mut k, p, 84.0, 1.0);
            if let Some(g) = if second { secondary } else { primary } {
                k.text(x, p.y + 36.0, t::LABEL, over, t::ACCENT, 0);
                k.text(x, p.y + 60.0, Type { size: 52.0, ..t::DISPLAY }, &g.name, t::INK, 0);
                // The gun itself, large: the 3D preview, not its small picture.
                gun(&mut k, R::new(p.x + 40.0, p.y + 130.0, p.w - 80.0, 380.0), gun_key, &g.spec, g.camo);
                let h = chips(&mut k, x, p.y + 540.0, x + w, &gun_chips(fe, g));
                stat_bars(&mut k, x, p.y + 560.0 + h, w * 0.6, &g.spec);
                if showing.ends_with("_laser") {
                    let note = "Seen only through night vision, where aiming cants the gun to the laser. No stat changes.";
                    k.text(x, p.y + 708.0 + h, Type { size: 19.0, ..t::CAPTION }, note, t::INK_DIM, 0);
                }
            }
        }
        "cac_perk1" | "cac_perk2" | "cac_perk3" | "cac_special" => {
            let names = ["cac_perk1", "cac_perk2", "cac_perk3", "cac_special"];
            let n = names.iter().position(|s| *s == showing).unwrap_or(0);
            grid_panel(&mut k, R::new(p.x, p.y, p.w, 420.0), 84.0, 1.0);
            let over = ["Perk 1", "Perk 2", "Perk 3", "Special grenade"][n];
            k.text(x, p.y + 36.0, t::LABEL, over, t::ACCENT, 0);
            if let Some(reference) = items[n].clone() {
                let (title, picture) = item(fe, &reference);
                k.text(x, p.y + 60.0, Type { size: 52.0, ..t::DISPLAY }, &title, t::INK, 0);
                if !picture.is_empty() {
                    icon_box(&mut k, R::new(x, p.y + 150.0, 180.0, 180.0));
                    k.pic(R::new(x + 18.0, p.y + 168.0, 144.0, 144.0), &picture, [1.0; 4]);
                }
                k.para(x + 210.0, p.y + 160.0, w - 210.0, Type { size: 22.0, ..t::BODY }, &description(fe, &reference), t::INK_DIM, 1.3);
            }
            if !shown(names[n]) {
                k.text(x, p.y + 360.0, Type { size: 19.0, ..t::CAPTION }, "Set by the primary weapon's attachment.", t::INK_MUTE, 0);
            }
        }
        "cac_streaks" => {
            grid_panel(&mut k, R::new(p.x, p.y, p.w, 420.0), 84.0, 1.0);
            k.text(x, p.y + 36.0, t::LABEL, "Kill streaks", t::ACCENT, 0);
            k.text(x, p.y + 60.0, Type { size: 40.0, ..t::H1 }, "Yours, in every class", t::INK, 0);
            let picked = super::streaks::chosen(fe);
            let cw = (w - 32.0) / super::streaks::MAX as f32;
            for (i, h) in picked.iter().enumerate() {
                let cx = x + i as f32 * (cw + 16.0);
                icon_box(&mut k, R::new(cx, p.y + 130.0, 150.0, 150.0));
                k.pic(R::new(cx + 15.0, p.y + 145.0, 120.0, 120.0), h.hud_icon(), [1.0; 4]);
                k.text(cx, p.y + 296.0, Type { size: 26.0, ..t::H2 }, h.name(), t::INK, 0);
                k.text(cx, p.y + 330.0, t::MICRO, &format!("{} kills", h.kills()), t::INK_DIM, 0);
            }
        }
        _ => class_panel(&mut k, p, &class_key, "Custom class", b),
    }
    k.shift = Vec2::ZERO;
    if popup {
        let b = k.v.bleed();
        k.fill(b, t::hex(0x050607, 0.45));
    }
    back_button(&mut k, hot == Some("cac_back"));
    if !popup {
        footer(&mut k, "", &[(Button::Confirm, "Change"), (Button::Back, "Back")]);
    }
}

/// Whether one of these screens is up with nothing over it (they draw
/// their own button prompts then).
pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    let Some(top) = fe.stack.iter().rposition(|m| m.menu.window.name == SELECT || m.menu.window.name == EDITOR) else { return false };
    !fe.stack[top + 1..].iter().any(|m| m.menu.visible_exp.is_empty() || eval(&m.menu.visible_exp, fe).truthy())
}

// --- A class's loadout, for the match's Choose Class ----------------------------

/// Everything about class `class` (`assault`, `custom2`) in a panel over `p`:
/// its name, both guns with their attachments and stats, perks and
/// grenade. `over` is the line above the name; `key` the guns' preview
/// keys (pictures in a match).
pub(super) fn class_panel(k: &mut Kit, p: R, class: &str, over: &str, key: i32) {
    let fe = k.fe;
    grid_panel(k, p, 84.0, 1.0);
    let Some(l) = fe.class_loadout(class) else { return };
    let (x, w) = (p.x + 44.0, p.w - 88.0);
    k.text(x, p.y + 36.0, t::LABEL, over, t::ACCENT, 0);
    k.text(x, p.y + 60.0, Type { size: 52.0, ..t::DISPLAY }, &l.name, t::INK, 0);
    k.slice3(R::new(x, p.y + 128.0, w, 14.0), "next:panels/divider", Vec2::new(354.0, 28.0), 120.0, 120.0, [1.0; 4]);
    let picture = |spec: &str| item(fe, spec.split_once(':').map_or(spec, |(w, _)| w)).1;
    let mut y = p.y + 160.0;
    if let Some(g) = l.guns.first() {
        k.text(x, y, t::LABEL, "Primary", t::INK_DIM, 0);
        k.text(x, y + 22.0, Type { size: 40.0, ..t::H1 }, &g.name, t::INK, 0);
        let h = chips(k, x, y + 72.0, x + w * 0.46, &gun_chips(fe, g));
        stat_bars(k, x, y + 84.0 + h, w * 0.42, &g.spec);
        k.gun(R::new(x + w * 0.46, y + 10.0, w * 0.54, w * 0.27), key + 1, &g.spec, g.camo, &picture(&g.spec));
        y += 250.0 + h.max(32.0) - 32.0;
    }
    if let Some(g) = l.guns.get(1) {
        k.text(x, y, t::LABEL, "Secondary", t::INK_DIM, 0);
        k.text(x, y + 22.0, Type { size: 32.0, ..t::H1 }, &g.name, t::INK, 0);
        chips(k, x, y + 64.0, x + w * 0.5, &gun_chips(fe, g));
        k.gun(R::new(x + w * 0.62, y, w * 0.3, w * 0.15), key + 3, &g.spec, g.camo, &picture(&g.spec));
        y += 130.0;
    }
    // Perks, then the special grenade.
    let mut list: Vec<(String, String)> = l.perks.iter().enumerate().map(|(i, r)| (format!("Perk {}", i + 1), r.clone())).collect();
    if let Some(s) = &l.special {
        list.push(("Special grenade".into(), s.trim_end_matches("_mp").to_owned()));
    }
    let n = list.len().max(1) as f32;
    let cw = (w - 16.0 * (n - 1.0)) / n;
    let y = y.max(p.bottom() - 190.0);
    for (i, (over, reference)) in list.iter().enumerate() {
        let cx = x + i as f32 * (cw + 16.0);
        let (name, picture) = item(fe, reference);
        k.text(cx, y, t::MICRO, over, t::INK_DIM, 0);
        if !picture.is_empty() {
            icon_box(k, R::new(cx, y + 20.0, 84.0, 84.0));
            k.pic(R::new(cx + 6.0, y + 26.0, 72.0, 72.0), &picture, [1.0; 4]);
        }
        k.para(cx, y + 108.0, cw, Type { size: 20.0, ..t::H2 }, &name, t::INK, 1.1);
    }
}
