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

/// The weapon's attachments and camo as chips.
fn gun_chips(fe: &Frontend, gun: &crate::loadout::Gun) -> Vec<(String, bool)> {
    let atts = gun.spec.split_once(':').map_or("", |(_, a)| a);
    let mut out: Vec<(String, bool)> = atts.split('+').filter(|a| !a.is_empty()).map(|a| (attachment_name(fe, a), false)).collect();
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
        let moved: f32 = atts.split('+').filter(|a| !a.is_empty()).map(|a| attribute(fe, weapon, Some(a), col).1 - base).sum();
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

static EDITOR_OPENED: AtomicI64 = AtomicI64::new(i64::MIN);

/// The editor's places.
const PRIMARY: R = R::new(96.0, 196.0, 1170.0, 352.0);
const SECONDARY: R = R::new(96.0, 566.0, 1170.0, 214.0);
const SIDE_X: f32 = 1290.0;
const SIDE_W: f32 = 534.0;
fn side(i: usize) -> R {
    R::new(SIDE_X, 196.0 + i as f32 * 148.0, SIDE_W, 136.0)
}
const RENAME: R = R::new(SIDE_X, 798.0, (SIDE_W - 14.0) * 0.5, 48.0);
const RESET: R = R::new(SIDE_X + (SIDE_W + 14.0) * 0.5, 798.0, (SIDE_W - 14.0) * 0.5, 48.0);

/// Where a classic button goes, by its label.
fn editor_place(label: &str) -> Option<(R, &'static str)> {
    let chip_at = |card: R, from_right: f32| R::new(card.right() - from_right, card.bottom() - 58.0, from_right - 24.0, 38.0);
    Some(match label {
        "Primary Weapon" => (PRIMARY, "cac_primary"),
        "Side Arm" | "Secondary Weapon" => (SECONDARY, "cac_secondary"),
        "Perk 1" => (side(0), "cac_perk1"),
        "Perk 2" => (side(1), "cac_perk2"),
        "Perk 3" => (side(2), "cac_perk3"),
        "Special Grenade" => (side(3), "cac_special"),
        "Primary Variant" => (chip_at(PRIMARY, 200.0), "cac_primary_variant"),
        "Secondary Variant" => (chip_at(SECONDARY, 200.0), "cac_secondary_variant"),
        "Camo" => (chip_at(PRIMARY, 360.0), "cac_camo"),
        "Rename" => (RENAME, "cac_rename"),
        "Reset to Default" => (RESET, "cac_reset"),
        "Back" => (BACK, "cac_back"),
        _ => return None,
    })
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
    if !out.items.iter().any(|it| it.window.name == "cac_back") {
        let template = out.items.first()?.clone();
        out.items.push(back_item(&template, &out.on_esc));
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
    let class = fe.class_loadout(&format!("custom{}", slot + 1));
    let title = class.as_ref().map_or_else(|| format!("Custom Slot {}", slot + 1), |c| c.name.clone());
    header(&mut k, "Loadout  /  Create a Class", &title);
    // A popup over the editor: it stays, a little dimmer.
    let popup = covered(fe, om);
    let hot = if popup { None } else { focused_name(fe, om) };
    let shown = |name: &str| om.menu.items.iter().enumerate().any(|(i, it)| it.window.name == name && fe.item_visible(om, i));

    // Rename.
    if shown("cac_rename") {
        let on = hot == Some("cac_rename");
        let r = RENAME;
        k.fill(r, if on { t::with_alpha(t::ACCENT, 0.35) } else { t::hex(0x000000, 0.45) });
        k.frame(r, 1.5, if on { t::ACCENT } else { t::LINE });
        k.text_mid(r.cx(), r.cy(), Type { size: 20.0, ..t::ROW_TEXT }, "Rename", t::INK, 1);
    }

    k.shift = Vec2::new(0.0, (1.0 - ease) * 16.0);
    // Primary.
    let on = hot == Some("cac_primary");
    card(&mut k, PRIMARY, on);
    let (x, y) = (PRIMARY.x + 40.0, PRIMARY.y + 32.0);
    k.text(x, y, t::LABEL, "Primary weapon", if on { t::ACCENT } else { t::INK_DIM }, 0);
    if let Some(g) = class.as_ref().and_then(|c| c.guns.first()) {
        k.text(x, y + 26.0, Type { size: 52.0, ..t::DISPLAY }, &g.name, t::INK, 0);
        let h = chips(&mut k, x, y + 92.0, x + 470.0, &gun_chips(fe, g));
        stat_bars(&mut k, x, y + 108.0 + h, 420.0, &g.spec);
        gun(&mut k, R::new(PRIMARY.x + 470.0, PRIMARY.y + 12.0, PRIMARY.w - 490.0, 290.0), b + 1, &g.spec, g.camo);
    }
    // Secondary.
    let on = hot == Some("cac_secondary");
    card(&mut k, SECONDARY, on);
    let (x, y) = (SECONDARY.x + 40.0, SECONDARY.y + 30.0);
    k.text(x, y, t::LABEL, "Secondary weapon", if on { t::ACCENT } else { t::INK_DIM }, 0);
    if let Some(g) = class.as_ref().and_then(|c| c.guns.get(1)) {
        k.text(x, y + 26.0, Type { size: 40.0, ..t::H1 }, &g.name, t::INK, 0);
        chips(&mut k, x, y + 80.0, x + 520.0, &gun_chips(fe, g));
        gun(&mut k, R::new(SECONDARY.x + 520.0, SECONDARY.y + 8.0, 420.0, 196.0), b + 3, &g.spec, g.camo);
    }
    // The variant and camo chips inside the weapon cards.
    for (name, label) in [("cac_primary_variant", "Variant"), ("cac_secondary_variant", "Variant"), ("cac_camo", "Camo")] {
        if !shown(name) {
            continue;
        }
        let Some((r, _)) = editor_place(match name {
            "cac_primary_variant" => "Primary Variant",
            "cac_secondary_variant" => "Secondary Variant",
            _ => "Camo",
        }) else {
            continue;
        };
        let on = hot == Some(name);
        k.fill(r, if on { t::with_alpha(t::ACCENT, 0.35) } else { t::hex(0x000000, 0.5) });
        k.frame(r, 1.5, if on { t::ACCENT } else { t::LINE });
        k.text_mid(r.cx(), r.cy(), Type { size: 19.0, ..t::ROW_TEXT }, label, t::INK, 1);
    }

    // Perks and the special grenade.
    let items = class_items(fe, slot);
    for (i, (name, over)) in [("cac_perk1", "Perk 1"), ("cac_perk2", "Perk 2"), ("cac_perk3", "Perk 3"), ("cac_special", "Special grenade")].iter().enumerate() {
        // Not editable now (a weapon perk set by an attachment): shown,
        // locked.
        let editable = shown(name);
        let r = side(i);
        let on = hot == Some(*name);
        card(&mut k, r, on);
        k.text(r.x + 28.0, r.y + 22.0, t::LABEL, over, if on { t::ACCENT } else { t::INK_DIM }, 0);
        if !editable {
            k.lock(Vec2::new(r.right() - 30.0, r.y + 28.0), 18.0, t::INK_MUTE);
        }
        if let Some(reference) = items[i].clone() {
            let (title, picture) = item(fe, &reference);
            k.text(r.x + 28.0, r.y + 46.0, Type { size: 28.0, ..t::H2 }, &title, t::INK, 0);
            let about = description(fe, &reference);
            k.para(r.x + 28.0, r.y + 84.0, r.w - 150.0, Type { size: 17.0, ..t::CAPTION }, &about, t::INK_DIM, 1.2);
            if !picture.is_empty() {
                icon_box(&mut k, R::new(r.right() - 116.0, r.y + 22.0, 96.0, 96.0));
                k.pic(R::new(r.right() - 108.0, r.y + 30.0, 80.0, 80.0), &picture, [1.0; 4]);
            }
        }
    }
    // Reset.
    if shown("cac_reset") {
        let on = hot == Some("cac_reset");
        k.fill(RESET, if on { t::with_alpha(t::ENEMY, 0.25) } else { t::hex(0x000000, 0.45) });
        k.frame(RESET, 1.5, if on { t::ENEMY } else { t::LINE });
        k.text_mid(RESET.cx(), RESET.cy(), Type { size: 21.0, ..t::ROW_TEXT }, "Reset to Default", t::INK, 1);
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
