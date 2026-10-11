//! The new Create a Class pickers: every popup opened from the new class
//! editor (the CoD4 / Black Ops / World at War choice, weapon classes,
//! weapons, attachments, camos, perks, grenades) shown full screen as a
//! column list, with a preview of the focused choice on the right.
//!
//! Like the other new screens, the popup's own buttons stay: CoD4's popups
//! draw each row as a pair (the primary and the secondary weapon's, or the
//! unlocked and locked look) at one spot, so the buttons are grouped by
//! their spot and each group moved to a row of the column. The popup's text
//! on the right (the focused weapon's name and description) is kept to
//! read for the preview. Only the drawing is this file's.

use super::super::expr::{Env, eval};
use super::super::{Frontend, OpenMenu, Op, Placement, weapon_picture_stat};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Statement, Window as MenuWindow, flags, item_type};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

/// The window name of a picker: this, then the weapon stat its preview
/// shows (CoD4's popup picture's, or the class weapon's).
pub const PREFIX: &str = "next_pick|";

/// Whether `key` is a popup of the class editor's to show as a picker.
pub(in crate::ui) fn wanted(fe: &Frontend, key: &str) -> bool {
    let from_editor = fe.stack.iter().any(|m| m.menu.window.name == cac::EDITOR);
    let popup = key.contains("popup_cac_")
        || key.contains("attachment_popup")
        || key.starts_with("t5|")
        || key.starts_with("t4|")
        || key.starts_with("iw4|")
        || key == super::super::reticle_menu::MENU
        || key == super::super::custom_camo::PICKER;
    from_editor && popup
}

/// Per picker (by its window name): its title, and the popup's info texts.
struct Info {
    title: String,
    texts: Vec<Item>,
    /// The popup's pictures on the right (the focused perk's icon).
    pictures: Vec<Item>,
}

fn infos() -> &'static Mutex<HashMap<String, Info>> {
    static I: std::sync::OnceLock<Mutex<HashMap<String, Info>>> = std::sync::OnceLock::new();
    I.get_or_init(Default::default)
}

static OPENED: AtomicI64 = AtomicI64::new(i64::MIN);

const TOP: f32 = 226.0;

/// The column's rows for `n` choices.
fn row_rect(i: usize, n: usize) -> R {
    let pitch = (734.0 / n.max(1) as f32).min(54.0);
    R::new(col_x(0), TOP + i as f32 * pitch, cols(4), (pitch - 4.0).min(50.0))
}

fn label_of(fe: &Frontend, it: &Item) -> String {
    let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, fe).text() };
    fe.assets.localize(&raw).trim().to_owned()
}

/// The value a row's focus script sets `dvar` to (`set ui_primary_highlighted
/// ak47`, `"setdvar" "ui_attachment_highlighted" "acog"`).
fn focus_value(it: &Item, dvar: &str) -> Option<String> {
    let script = format!("{} {}", it.on_focus, it.mouse_enter);
    let at = script.find(dvar)? + dvar.len();
    let v: String = script[at..].trim_start_matches(|c: char| c == '"' || c.is_whitespace()).chars().take_while(|c| !matches!(c, '"' | ';' | ' ')).collect();
    (!v.is_empty()).then_some(v)
}

/// A title for a popup without its own.
fn title_for(key: &str) -> &'static str {
    if key == super::super::reticle_menu::MENU {
        "Reticle"
    } else if key == super::super::custom_camo::PICKER {
        "Custom Camos"
    } else if key.contains("|game|") {
        "Choose a game"
    } else if key.contains("attachment") {
        "Attachments"
    } else if key.contains("camo") {
        "Camo"
    } else if key.contains("perk1") {
        "Perk 1"
    } else if key.contains("perk2") {
        "Perk 2"
    } else if key.contains("perk3") {
        "Perk 3"
    } else if key.contains("perk") {
        "Perk"
    } else if key.contains("extra") {
        "Special Grenade"
    } else if key.ends_with("_primary") || key.ends_with("_primary2") || key.contains("|primary") {
        "Weapon Class"
    } else if key.contains("secondary") {
        "Secondary Weapon"
    } else {
        "Choose"
    }
}

/// The picker built from a popup.
pub(in crate::ui) fn menu(fe: &Frontend, classic: &Menu, key: &str) -> Option<Menu> {
    let spot = |it: &Item| ((it.window.rect.x * 2.0).round() as i32, (it.window.rect.y * 2.0).round() as i32);
    // (Not a screen's own Back button: the picker has its own.)
    let choice = |it: &Item| (it.ty == item_type::BUTTON || it.ty == item_type::MULTI) && !it.window.name.eq_ignore_ascii_case("back") && it.window.rect.w < 500.0 && it.window.rect.h < 100.0 && !label_of(fe, it).is_empty();
    // The rows' spots, top to bottom.
    let mut spots: Vec<(i32, i32)> = classic.items.iter().filter(|it| choice(it)).map(spot).collect();
    spots.sort_by_key(|&(x, y)| (y, x));
    spots.dedup();
    if spots.is_empty() {
        return None;
    }
    let first_y = spots.first().map_or(0.0, |s| s.1 as f32 * 0.5);
    let preview = classic.items.iter().find_map(|it| weapon_picture_stat(&it.material_exp));
    let editing_secondary = fe.editing_secondary();
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = format!("{PREFIX}{}|{key}", preview.map_or(String::from("-"), |k| k.to_string()));
    let mut title = None;
    let mut texts = Vec::new();
    let mut pictures = Vec::new();
    for it in &classic.items {
        let r = it.window.rect;
        let at = spots.iter().position(|&s| s == spot(it));
        if let Some(row) = at.filter(|_| choice(it) || (it.ty != item_type::BUTTON && !label_of(fe, it).is_empty())) {
            let mut item = it.clone();
            let name = if it.ty == item_type::BUTTON || it.ty == item_type::MULTI { format!("pick_{row}") } else { format!("pick_label_{row}") };
            let label = label_of(fe, it);
            item.window = MenuWindow { rect: virtual_rect(row_rect(row, spots.len())), dynamic_flags: flags::VISIBLE, name, ..MenuWindow::default() };
            item.window.dynamic_flags |= it.window.dynamic_flags & flags::VISIBLE;
            item.text = label;
            item.text_exp = Statement::default();
            item.text_scale = 0.0;
            out.items.push(item);
        } else if it.ty != item_type::BUTTON && it.ty != item_type::MULTI && (!it.text.is_empty() || !it.text_exp.is_empty()) {
            if r.y < first_y - 1.0 && r.x < 200.0 {
                // The popup's heading, just above its rows.
                title = Some(label_of(fe, it)).filter(|s| !s.is_empty()).or(title);
            } else if r.x >= 200.0 {
                texts.push(it.clone());
            }
        } else if it.ty != item_type::BUTTON && !it.material_exp.is_empty() && r.x >= 200.0 && weapon_picture_stat(&it.material_exp).is_none() {
            pictures.push(it.clone());
        }
    }
    // A back button, for the mouse.
    if let Some(template) = out.items.iter().find(|it| it.ty == item_type::BUTTON).cloned() {
        out.items.push(cac::back_item(&template, &classic.on_esc));
    }
    let title = if key.contains("camo") || key.contains("perk") || key.contains("extra") { None } else { title };
    let title = title.unwrap_or_else(|| title_for(key).to_owned());
    let title = if editing_secondary && title == "Weapon Class" { "Secondary Weapon".to_owned() } else { title };
    if let Ok(mut m) = infos().lock() {
        m.insert(out.window.name.clone(), Info { title, texts, pictures });
    }
    Some(out)
}

pub(in crate::ui) fn opened(fe: &Frontend) {
    OPENED.store(fe.millis(), Ordering::Relaxed);
}

/// Whether a picker is up with nothing over it.
pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    let Some(at) = fe.stack.iter().rposition(|m| m.menu.window.name.starts_with(PREFIX)) else { return false };
    !fe.stack[at + 1..].iter().any(|m| m.menu.visible_exp.is_empty() || eval(&m.menu.visible_exp, fe).truthy())
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    let Some(rest) = om.menu.window.name.strip_prefix(PREFIX) else { return };
    let preview_key = rest.split('|').next().and_then(|k| k.parse::<i32>().ok());
    let (title, texts, pictures) =
        infos().lock().ok().and_then(|m| m.get(&om.menu.window.name).map(|i| (i.title.clone(), i.texts.clone(), i.pictures.clone()))).unwrap_or_default();
    // Perks and grenades: their picture, not a gun.
    let perks = rest.contains("perk") || rest.contains("extra");
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    cac::loadout_backdrop(&mut k);
    let since = (fe.millis() - OPENED.load(Ordering::Relaxed)).max(0) as f32 / 1000.0;
    let ease = t::ease_out(since / t::MEDIUM);
    k.alpha = ease;
    // Where we are: the class being edited.
    let class = fe.stack.iter().rev().find(|m| m.menu.window.name == cac::EDITOR).map(|m| m.name.clone());
    let slot = class.as_deref().and_then(cac::slot_of);
    let class_name = slot.and_then(|s| fe.class_loadout(&format!("custom{}", s + 1))).map_or_else(|| String::from("Create a Class"), |c| c.name);
    header(&mut k, &format!("Create a Class  /  {class_name}"), &title);

    let camos = rest.contains("camo");
    let key = preview_key.or_else(|| slot.map(|s| 200 + 10 * s as i32 + if fe.editing_secondary() { 3 } else { 1 }));
    // The rows.
    // The class stat the perk picker sets (perk 1..3, special grenade).
    let perk_stat = slot.and_then(|s| {
        let n = if rest.contains("perk1") { 5 } else if rest.contains("perk2") { 6 } else if rest.contains("perk3") { 7 } else if rest.contains("extra") { 8 } else { return None };
        Some(200 + 10 * s as i32 + n)
    });
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).map(|(_, i)| *i);
    let hot_name = hot.and_then(|i| om.menu.items.get(i)).map(|it| it.window.name.clone());
    let rows = om.menu.items.iter().filter_map(|it| it.window.name.strip_prefix("pick_")?.parse::<usize>().ok()).max().map_or(0, |m| m + 1);
    let labels = om.menu.items.iter().filter_map(|it| it.window.name.strip_prefix("pick_label_")?.parse::<usize>().ok()).max().map_or(0, |m| m + 1);
    let n = rows.max(labels);
    k.shift = Vec2::new((1.0 - ease) * -20.0, 0.0);
    let icon = if perks { super::kit::icons::STAR } else if camos { super::kit::icons::CRATE } else { super::kit::icons::RIFLE };
    section_icon(&mut k, col_x(0), 192.0, cols(4), "Choose", Some(icon));
    for row in 0..n {
        let r = row_rect(row, n);
        let button = om.menu.items.iter().enumerate().find(|(i, it)| it.window.name == format!("pick_{row}") && fe.item_visible(om, *i));
        match button {
            Some((i, it)) => {
                let focused = hot == Some(i);
                // A setting stepped left and right: its value, with arrows.
                if let iw3::menu::ItemData::Multi(m) = &it.data {
                    let at = super::super::multi_index(m, &fe.dvar(&it.dvar));
                    let value = m.labels.get(at).map(|l| fe.assets.localize(l)).unwrap_or_default();
                    super::shell::row(&mut k, r, it.text.trim_end_matches(':'), Some(&value), focused);
                    continue;
                }
                if it.text.eq_ignore_ascii_case("Accept") {
                    action_button(&mut k, r, &it.text, focused);
                    continue;
                }
                menu_row(&mut k, r, &it.text, focused, None);
                let right = r.right() - if focused { 52.0 } else { 20.0 };
                // A camo: its swatch, ticked when on the gun; the camo
                // editor's and reticle's rows their icons.
                if let Some(c) = focus_value(it, "ui_camo_highlighted") {
                    let sw = R::new(right - 44.0, r.cy() - 20.0, 40.0, 40.0);
                    match c.as_str() {
                        "camo_custom" => k.icon("next:icons_menu", 7, sw, t::INK_DIM),
                        "camo_reticle" => k.icon("next:icons_menu", 1, sw, t::INK_DIM),
                        _ => {
                            let swatch = fe.table_lookup("mp/attachmenttable.csv", 4, &c, 6);
                            if !swatch.is_empty() && swatch != "weapon_missing_image" {
                                k.pic(sw, &swatch, [1.0; 4]);
                                k.frame(sw, 1.0, t::LINE);
                            } else {
                                k.frame(sw, 1.0, t::LINE);
                                k.line(Vec2::new(sw.x + 6.0, sw.bottom() - 6.0), Vec2::new(sw.right() - 6.0, sw.y + 6.0), 1.5, t::INK_MUTE);
                            }
                            let id = fe.table_lookup("mp/attachmenttable.csv", 4, &c, 11).parse::<i32>().ok();
                            let on = key.is_some_and(|k| id == Some(fe.class_camo(k)));
                            if on {
                                k.check(Vec2::new(sw.x - 26.0, r.cy()), 16.0, 2.5, t::ACCENT);
                            }
                        }
                    }
                }
                // A perk or grenade: its icon, ticked when the class has it.
                if let Some(perk) = focus_value(it, "ui_perk_highlighted").or_else(|| focus_value(it, "ui_sgrenade_highlighted")) {
                    let picture = fe.table_lookup("mp/statstable.csv", 4, &perk, 6);
                    let ib = R::new(right - 46.0, r.cy() - 21.0, 42.0, 42.0);
                    if !picture.is_empty() {
                        k.pic(ib, &picture, [1.0, 1.0, 1.0, if focused { 1.0 } else { 0.8 }]);
                    }
                    let index = |col| fe.table_lookup("mp/statstable.csv", 4, &perk, col).parse::<i32>().ok();
                    let held = perk_stat.map(|st| fe.stat(st));
                    if held.is_some() && (held == index(1) || held == index(0)) {
                        k.check(Vec2::new(ib.x - 24.0, r.cy()), 18.0, 2.5, t::ACCENT);
                    }
                }
                // A weapon: its picture, and NEW when just unlocked.
                if let Some(weapon) = focus_value(it, "ui_primary_highlighted") {
                    let picture = fe.table_lookup("mp/statstable.csv", 4, &weapon, 6);
                    if !picture.is_empty() {
                        k.pic(R::new(right - 112.0, r.cy() - 22.0, 112.0, 44.0), &picture, [1.0, 1.0, 1.0, if focused { 1.0 } else { 0.75 }]);
                    }
                    let stat = fe.table_lookup("mp/statstable.csv", 4, &weapon, 1).parse::<i32>().unwrap_or(0);
                    if stat > 0 && fe.stat(stat) & 65536 != 0 {
                        let tw = k.measure(Type { size: 28.0, ..t::ROW_TEXT }, &it.text);
                        k.pic(R::new(r.x + 24.0 + tw + 12.0, r.cy() - 13.0, 74.0, 26.0), "next:badge_new", [1.0; 4]);
                    }
                }
                // An attachment: its box, ticked when on.
                if let Some((stat, name)) = om.rows.get(i).and_then(|r| r.as_ref()).filter(|(_, n)| n != "none") {
                    let on = super::super::attachments::has(super::super::attachments::set_of(fe, stat - 1), name);
                    let icon = fe.table_lookup("mp/attachmenttable.csv", 4, name, 6);
                    if !icon.is_empty() && icon != "weapon_missing_image" {
                        k.pic(R::new(r.right() - 138.0, r.cy() - 20.0, 40.0, 40.0), &icon, [1.0, 1.0, 1.0, if focused { 1.0 } else { 0.75 }]);
                    }
                    let b = R::new(r.right() - 82.0, r.cy() - 13.0, 26.0, 26.0);
                    k.fill(b, if on { t::ACCENT } else { t::hex(0x000000, 0.4) });
                    k.frame(b, 1.5, if on { t::ACCENT } else { t::LINE_STRONG });
                    if on {
                        k.check(Vec2::new(b.cx(), b.cy()), 14.0, 2.5, t::INK_ON_ACCENT);
                    }
                }
            }
            None => {
                // Locked: its label, dimmed, with a padlock.
                let label = om.menu.items.iter().enumerate().find(|(i, it)| it.window.name == format!("pick_label_{row}") && fe.item_visible(om, *i));
                if let Some((_, it)) = label {
                    k.hline(r.x + 24.0, r.right(), r.bottom(), 1.0, t::HAIRLINE);
                    k.text_mid(r.x + 24.0, r.cy(), Type { size: 28.0, ..t::ROW_TEXT }, &it.text, t::INK_MUTE, 0);
                    k.lock(Vec2::new(r.right() - 26.0, r.cy()), 18.0, t::INK_MUTE);
                    // What unlocks it (the row's hidden button names the weapon).
                    let weapon = om.menu.items.iter().filter(|b| b.window.name == format!("pick_{row}")).find_map(|b| focus_value(b, "ui_primary_highlighted").or_else(|| focus_value(b, "ui_perk_highlighted")));
                    let unlock = weapon.as_ref().map(|w| fe.table_lookup("mp/statstable.csv", 4, w, 10)).filter(|u| !u.is_empty());
                    // (Black Ops' and World at War's guns: their level.)
                    let level = || weapon.as_ref().and_then(|w| super::super::progression::other_levels(&fe.assets).get(w).copied()).map(|l| format!("Level {l}"));
                    if let Some(s) = unlock.map(|u| fe.assets.localize(&format!("@{u}"))).or_else(level) {
                        k.text_mid(r.right() - 52.0, r.cy(), Type { size: 17.0, ..t::CAPTION }, &s, t::INK_MUTE, 2);
                    }
                }
            }
        }
    }

    // The preview: the focused (or current) gun, its name, what the popup
    // says about it, its attributes.
    k.shift = Vec2::new((1.0 - ease) * 20.0, 0.0);
    // Sized to what it holds: a gun, its name, stats and a line or two; or
    // a perk's picture, name and description.
    let perks_early = rest.contains("perk") || rest.contains("extra");
    let tall = om.name == super::super::reticle_menu::MENU;
    let p = R::new(col_x(5), 192.0, cols(7), if perks_early { 430.0 } else if tall { 780.0 } else { 600.0 });
    grid_panel(&mut k, p, 84.0, 1.0);
    let (x, w) = (p.x + 44.0, p.w - 88.0);
    let reticle = om.name == super::super::reticle_menu::MENU;
    let ccamo = om.name == super::super::custom_camo::PICKER;
    let gun = key.filter(|_| !perks && !reticle && !ccamo).and_then(|key| Some((key, fe.gun_for(key)?)));
    // The reticle: the lens, its name and where it shows.
    if reticle {
        let (lens, name, note) = fe.reticle_preview();
        let code = lens.rsplit('_').next().and_then(|c| c.parse::<u16>().ok()).unwrap_or(0);
        super::sight::draw(&mut k, R::new(p.x + 12.0, p.y + 12.0, p.w - 24.0, p.h - 190.0), code);
        grid_frame(&mut k, p);
        k.text(x, p.bottom() - 136.0, Type { size: 36.0, ..t::H1 }, &name, t::INK, 0);
        k.text(x, p.bottom() - 84.0, Type { size: 20.0, ..t::BODY }, note, t::INK_DIM, 0);
    }
    // A custom camo slot: the class gun wearing it.
    if ccamo {
        let (spec, preview, camo, title, lines, note) = fe.custom_camo_preview();
        let weapon = spec.split_once(':').map_or(spec.as_str(), |(w, _)| w);
        let picture = fe.table_lookup("mp/statstable.csv", 4, weapon, 6);
        k.gun(R::new(x + 40.0, p.y + 24.0, w - 80.0, 300.0), preview, &spec, camo, &picture);
        k.text(x, p.y + 330.0, Type { size: 48.0, ..t::DISPLAY }, &title, t::INK, 0);
        let mut y = p.y + 404.0;
        if let Some((swatch, a, b)) = lines {
            let sw = R::new(x, y, 64.0, 64.0);
            k.pic(sw, &swatch, [1.0; 4]);
            k.frame(sw, 1.0, t::LINE);
            k.text(x + 84.0, y + 2.0, Type { size: 26.0, ..t::H2 }, &a, t::INK, 0);
            k.text(x + 84.0, y + 38.0, Type { size: 19.0, ..t::CAPTION }, &b, t::INK_DIM, 0);
            y += 84.0;
        }
        k.text(x, y, Type { size: 20.0, ..t::BODY }, note, t::INK_DIM, 0);
        k.text(x, y + 30.0, Type { size: 20.0, ..t::BODY }, "Custom camos work on every weapon.", t::INK_MUTE, 0);
    }
    let mut text_top = p.y + 476.0;
    let mut text_x = x;
    if perks {
        let picture = pictures.iter().filter(|it| it.visible_exp.is_empty() || eval(&it.visible_exp, fe).truthy()).map(|it| eval(&it.material_exp, fe).text())
            // An item's picture (not a backdrop or a line).
            .find(|m| !m.is_empty() && !fe.table_lookup("mp/statstable.csv", 6, m, 6).is_empty());
        if let Some(m) = picture {
            icon_box(&mut k, R::new(x, p.y + 36.0, 196.0, 196.0));
            k.pic(R::new(x + 8.0, p.y + 44.0, 180.0, 180.0), &m, [1.0; 4]);
        }
        text_top = p.y + 252.0;
    }
    let mut said: Vec<String> = Vec::new();
    if let Some((key, (spec, camo))) = &gun {
        let weapon = spec.split_once(':').map_or(spec.as_str(), |(w, _)| w);
        let (picture, name) = (fe.table_lookup("mp/statstable.csv", 4, weapon, 6), fe.table_lookup("mp/statstable.csv", 4, weapon, 3));
        let gun_h = if camos { 300.0 } else { 290.0 };
        k.gun(R::new(x + 40.0, p.y + 24.0, w - 80.0, gun_h), *key, spec, *camo, &picture);
        let name = if name.is_empty() { weapon.to_uppercase() } else { fe.assets.localize(&format!("@{name}")) };
        k.text(x, p.y + 330.0, Type { size: 48.0, ..t::DISPLAY }, &name, t::INK, 0);
        said.push(name.to_lowercase());
        // Against what the class has now: the weapon list compares the
        // hovered gun, the attachment list the hovered attachment.
        let current = slot.map(|s| 200 + 10 * s as i32 + if fe.editing_secondary() { 3 } else { 1 }).and_then(|k| fe.gun_for(k)).map(|(s, _)| s);
        let base = if preview_key == Some(super::super::HIGHLIGHTED_WEAPON) {
            current
        } else if rest.contains("attachment") {
            let set = super::super::attachments::set_of(fe, *key);
            Some(format!("{weapon}:{}", super::super::attachments::names_of(set, &weapon).join("+")))
        } else {
            None
        };
        if !camos {
            cac::stat_bars_vs(&mut k, x + w * 0.52, p.y + 336.0, w * 0.48, spec, base.as_deref());
        }
        // The camo under the cursor: its swatch, name and what it is.
        if camos {
            let c = fe.dvar("ui_camo_highlighted");
            let swatch = fe.table_lookup("mp/attachmenttable.csv", 4, &c, 6);
            if !swatch.is_empty() && swatch != "weapon_missing_image" {
                let name = fe.assets.localize(&format!("@{}", fe.table_lookup("mp/attachmenttable.csv", 4, &c, 3)));
                let sw = R::new(x, p.y + 404.0, 64.0, 64.0);
                k.pic(sw, &swatch, [1.0; 4]);
                k.frame(sw, 1.0, t::LINE);
                k.text(x + 84.0, p.y + 408.0, Type { size: 28.0, ..t::H2 }, &name, t::INK, 0);
                said.push(name.to_lowercase());
                text_top = p.y + 444.0;
                text_x = x + 84.0;
            }
        }
        // The attachment under the cursor: what it is.
        if rest.contains("attachment") {
            let a = fe.dvar("ui_attachment_highlighted");
            if !a.is_empty() && a != "none" {
                let name = fe.assets.localize(&format!("@{}", fe.table_lookup("mp/attachmenttable.csv", 4, &a, 3)));
                let about = fe.assets.localize(&format!("@{}", fe.table_lookup("mp/attachmenttable.csv", 4, &a, 7)));
                let icon = fe.table_lookup("mp/attachmenttable.csv", 4, &a, 6);
                k.pic(R::new(x, p.y + 400.0, 56.0, 56.0), &icon, [1.0; 4]);
                k.text(x + 72.0, p.y + 404.0, Type { size: 26.0, ..t::H2 }, &name, t::INK, 0);
                k.para(x + 72.0, p.y + 436.0, w * 0.5 - 90.0, Type { size: 18.0, ..t::CAPTION }, &about, t::INK_DIM, 1.25);
                said.push(name.to_lowercase());
                said.push(about.to_lowercase());
                text_top = p.y + 520.0;
            }
        }
    }
    // The popup's own words (skipping a name already shown).
    let mut y = text_top;
    // (Indented beside a camo's swatch: narrower.)
    let (x, w) = (text_x, w - (text_x - x));
    let mut first = perks;
    for it in &texts {
        if !it.visible_exp.is_empty() && !eval(&it.visible_exp, fe).truthy() {
            continue;
        }
        let s = label_of(fe, it);
        if s.len() < 3 || said.contains(&s.to_lowercase()) {
            continue;
        }
        said.push(s.to_lowercase());
        // A perk's name first, big.
        if std::mem::replace(&mut first, false) {
            k.text(x, y, Type { size: 48.0, ..t::DISPLAY }, &s, t::INK, 0);
            y += 72.0;
            continue;
        }
        y += k.para(x, y, w, Type { size: 22.0, ..t::BODY }, &s, t::INK_DIM, 1.3) + 10.0;
        if y > p.bottom() - 60.0 {
            break;
        }
    }
    k.shift = Vec2::ZERO;
    cac::back_button(&mut k, hot_name.as_deref() == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Select"), (Button::Back, "Back")]);
}
