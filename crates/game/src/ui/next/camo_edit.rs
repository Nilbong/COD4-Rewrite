//! The new Camo Editor: the custom camo editor's settings as rows in three
//! groups on the left, and on the right the class gun wearing the draft over
//! the palette the colour rows take their colours from.
//!
//! As everywhere in the new UI, the editor's own rows, name field, buttons
//! and palette swatches are kept and moved; the drawing is this file's.

use super::super::expr::Env;
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Item, ItemData, Menu, Statement, Window as MenuWindow, flags, item_type};
use std::sync::atomic::{AtomicI64, Ordering};

pub const MENU: &str = "next_camo_edit";

static OPENED: AtomicI64 = AtomicI64::new(i64::MIN);

/// The groups, by the rows' labels (before the colon).
const GROUPS: [(&str, &[&str]); 3] = [
    ("Pattern", &["Pattern", "Colours", "Scale", "Rotation", "Finish"]),
    ("Colours", &["Colour 1", "Colour 2", "Colour 3", "Colour 4"]),
    ("Name", &["Name"]),
];
const BUTTONS: [&str; 2] = ["Save Camo", "Cancel"];

const ROW_H: f32 = 44.0;
const PITCH: f32 = 48.0;

/// Where each row goes: (label, rect), and the groups' header heights.
fn layout() -> (Vec<(&'static str, R)>, Vec<(&'static str, f32)>) {
    let (x, w) = (col_x(0), cols(4));
    let mut y = 192.0;
    let mut rows = Vec::new();
    let mut heads = Vec::new();
    for (name, labels) in GROUPS {
        heads.push((name, y));
        y += 32.0;
        for l in labels {
            rows.push((*l, R::new(x, y, w, ROW_H)));
            y += PITCH;
        }
        y += 14.0;
    }
    y += 6.0;
    for b in BUTTONS {
        rows.push((b, R::new(x, y, w, ROW_H)));
        y += PITCH;
    }
    (rows, heads)
}

/// The preview panel.
const PANEL: R = R::new(904.0, 192.0, 920.0, 500.0);

fn label_of(it: &Item) -> String {
    it.text.trim().trim_end_matches(':').to_owned()
}

/// The new editor built from `cod4rw_camo_editor`.
pub(in crate::ui) fn menu(classic: &Menu) -> Option<Menu> {
    let (rows, _) = layout();
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = MENU.into();
    let mut template = None;
    for it in &classic.items {
        let place = |item: &Item, r: R, name: String| {
            let mut item = item.clone();
            item.window = MenuWindow { rect: virtual_rect(r), dynamic_flags: flags::VISIBLE, name, ..MenuWindow::default() };
            item.text_scale = 0.0;
            item.text_exp = Statement::default();
            item
        };
        // (The palette's swatches: the colour picker instead.)
        if it.action.contains("ccamoSwatch") {
            continue;
        }
        let label = label_of(it);
        if let Some((_, r)) = rows.iter().find(|(l, _)| l.eq_ignore_ascii_case(&label)) {
            template.get_or_insert_with(|| it.clone());
            let mut item = place(it, *r, format!("camo_row_{label}"));
            // A colour row opens the colour picker.
            if let Some(n) = label.strip_prefix("Colour ").and_then(|n| n.parse::<usize>().ok()) {
                item.ty = item_type::BUTTON;
                item.text = label.clone();
                item.data = ItemData::default();
                item.action = format!("\"play\" \"mouse_click\" ; \"uiScript\" ccamoColor {n} ;");
            }
            out.items.push(item);
        }
    }
    let template = template?;
    out.items.push(cac::back_item(&template, &classic.on_esc));
    Some(out)
}

pub(in crate::ui) fn opened(fe: &Frontend) {
    OPENED.store(fe.millis(), Ordering::Relaxed);
}

fn rgba(c: [u8; 3]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    cac::loadout_backdrop(&mut k);
    let since = (fe.millis() - OPENED.load(Ordering::Relaxed)).max(0) as f32 / 1000.0;
    k.alpha = t::ease_out(since / t::MEDIUM);
    header(&mut k, "Create a Class  /  Custom Camos", "Camo Editor");
    let (gun, key, camo, swatch, name, active, _, colours) = fe.custom_camo_editor_view();
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).map(|(_, i)| *i);
    let hot_name = hot.and_then(|i| om.menu.items.get(i)).map(|it| it.window.name.clone());
    let editing = fe.editing.as_ref().filter(|(m, _)| *m == om.name).map(|(_, i)| *i);

    // The rows.
    let (_, heads) = layout();
    for (name, y) in heads {
        section(&mut k, col_x(0), y, cols(4), name);
    }
    for (i, it) in om.menu.items.iter().enumerate() {
        let Some(label) = it.window.name.strip_prefix("camo_row_") else { continue };
        if !fe.item_visible(om, i) {
            continue;
        }
        let r = super::kit::R::new(0.0, 0.0, 0.0, 0.0);
        let r = layout().0.into_iter().find(|(l, _)| l.eq_ignore_ascii_case(label)).map_or(r, |(_, r)| r);
        let focused = hot == Some(i);
        match &it.data {
            ItemData::Multi(m) => {
                let at = super::super::multi_index(m, &fe.dvar(&it.dvar));
                let value = m.labels.get(at).map(|l| fe.assets.localize(l)).unwrap_or_default();
                row(&mut k, r, label, Some(&value), focused);
                // A colour row: its colour, and which one the palette sets.
                if let Some(n) = label.strip_prefix("Colour ").and_then(|n| n.parse::<usize>().ok()) {
                    let lw = k.measure(t::ROW_TEXT, label);
                    let chip = R::new(r.x + 24.0 + lw + 16.0, r.cy() - 12.0, 24.0, 24.0);
                    if let Some(c) = colours.get(n - 1) {
                        k.fill(chip, rgba(*c));
                        k.frame(chip, 1.5, t::LINE_STRONG);
                    }
                    if n == active {
                        k.text_mid(chip.right() + 12.0, r.cy(), t::MICRO, "Palette", t::ACCENT, 0);
                    }
                }
            }
            _ if it.ty == item_type::EDITFIELD => {
                let mut value = fe.dvar(&it.dvar);
                if editing == Some(i) && (fe.millis() / 500) % 2 == 0 {
                    value.push('_');
                }
                row(&mut k, r, label, Some(&value), focused || editing == Some(i));
            }
            _ if label.starts_with("Colour ") => {
                menu_row(&mut k, r, label, focused, None);
                let n = label.strip_prefix("Colour ").and_then(|n| n.parse::<usize>().ok()).unwrap_or(1);
                if let Some(c) = colours.get(n - 1) {
                    let chip = R::new(r.right() - if focused { 150.0 } else { 120.0 }, r.cy() - 14.0, 96.0, 28.0);
                    k.fill(chip, rgba(*c));
                    k.frame(chip, 1.5, t::LINE_STRONG);
                }
            }
            _ => {
                if label == "Save Camo" {
                    action_button(&mut k, r, label, focused);
                } else {
                    menu_row(&mut k, r, label, focused, None);
                }
            }
        }
    }

    // The preview: the gun in the draft, its name, then the palette.
    grid_panel(&mut k, PANEL, 84.0, 1.0);
    let (x, w) = (PANEL.x + 44.0, PANEL.w - 88.0);
    let weapon = gun.split_once(':').map_or(gun.as_str(), |(w, _)| w);
    let picture = fe.table_lookup("mp/statstable.csv", 4, weapon, 6);
    k.gun(R::new(x + 20.0, PANEL.y + 30.0, w - 40.0, 330.0), key, &gun, camo, &picture);
    let sw = R::new(x, PANEL.y + 384.0, 72.0, 72.0);
    k.pic(sw.inset(6.0), &swatch, [1.0; 4]);
    icon_box(&mut k, sw);
    k.text(x + 92.0, PANEL.y + 386.0, Type { size: 40.0, ..t::DISPLAY }, &name, t::INK, 0);
    k.text(x + 92.0, PANEL.y + 434.0, Type { size: 19.0, ..t::CAPTION }, "Select a colour row to pick its colour.", t::INK_DIM, 0);
    cac::back_button(&mut k, hot_name.as_deref() == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Select"), (Button::Back, "Cancel")]);
}
