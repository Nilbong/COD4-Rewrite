//! Create a Class's weapon variant popup (`supply_variants`, built in
//! [`super::super::supply`]) as a full-screen picker: the standard gun and
//! each variant collected for it down the left column, in their rarity's
//! colour, the equipped one ticked; the gun and what the variant under the
//! cursor changes on the right. The classic rows are kept (their actions
//! equip, their focus sets the highlighted variant the class preview reads).

use super::super::expr::Env;
use super::super::supply::SLOT_DVAR;
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Statement, Window as MenuWindow, flags, item_type};

pub const MENU: &str = "next_variants";

const TOP: f32 = 226.0;
const PANEL: R = R::new(826.0, 192.0, 998.0, 780.0);

/// The column's rows for `n` choices.
fn row_rect(i: usize, n: usize) -> R {
    let pitch = (734.0 / n.max(1) as f32).min(54.0);
    R::new(col_x(0), TOP + i as f32 * pitch, cols(4), (pitch - 4.0).min(50.0))
}

/// A row's variant id (`none`: the standard gun), from its action.
fn row_id(it: &Item) -> Option<&str> {
    it.action.split("\"supplyEquip\"").nth(1).and_then(|r| r.split('"').nth(3))
}

/// The new screen built from the classic popup.
pub(in crate::ui) fn menu(classic: &Menu) -> Option<Menu> {
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = MENU.into();
    let rows: Vec<&Item> = classic.items.iter().filter(|it| it.ty == item_type::BUTTON && row_id(it).is_some()).collect();
    let n = rows.len();
    for (i, it) in rows.into_iter().enumerate() {
        let mut it = it.clone();
        it.window = MenuWindow { rect: virtual_rect(row_rect(i, n)), dynamic_flags: flags::VISIBLE, name: format!("variant_{i}"), ..MenuWindow::default() };
        it.text_scale = 0.0;
        it.visible_exp = Statement::default();
        out.items.push(it);
    }
    let template = out.items.first()?.clone();
    out.items.push(cac::back_item(&template, &classic.on_esc));
    Some(out)
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let Ok(stat) = fe.dvar(SLOT_DVAR).parse::<i32>() else { return };
    let weapon = fe.table_lookup("mp/statstable.csv", 0, &fe.stat(stat).to_string(), 4);
    let gun = fe.weapon_name(&weapon);
    // (The inventory's lock let go at once: the gun preview takes it too.)
    let (equipped, none_owned) = {
        let inv = crate::supply::inventory();
        (inv.equipped(stat, &weapon).map_or_else(|| "none".to_owned(), |v| v.id.clone()), inv.owned_of(&weapon).is_empty())
    };
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    cac::loadout_backdrop(&mut k);
    header(&mut k, "Loadout  /  Create a Class", if stat % 10 == 1 { "Primary Variant" } else { "Secondary Variant" });
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.clone());

    // The rows: the variant's name in its rarity's colour, its rarity on
    // the right, a tick on the one equipped.
    section(&mut k, col_x(0), 192.0, cols(4), &format!("{gun} variants"));
    let mut shown = None;
    let rows: Vec<&Item> = om.menu.items.iter().filter(|it| it.window.name.starts_with("variant_")).collect();
    for (i, it) in rows.iter().enumerate() {
        let Some(id) = row_id(it) else { continue };
        let r = row_rect(i, rows.len());
        let on = hot.as_deref() == Some(it.window.name.as_str());
        if on {
            shown = Some(id.to_owned());
        }
        let v = crate::supply::variant(id);
        menu_row(&mut k, r, v.map_or("Standard Issue", |v| v.name), on, None);
        let right = r.right() - if on { 56.0 } else { 20.0 };
        let (tag, colour) = v.map_or(("Standard", t::INK_DIM), |v| (v.rarity.name(), v.rarity.color()));
        let w = k.text_mid(right, r.cy(), Type { size: 18.0, ..t::CAPTION }, tag, colour, 2);
        if id == equipped {
            k.check(Vec2::new(right - w - 24.0, r.cy()), 18.0, 2.5, t::ACCENT);
        }
    }

    // The one under the cursor (else the one equipped): the gun, and what
    // the variant changes.
    let id = shown.unwrap_or(equipped.clone());
    let v = crate::supply::variant(&id);
    grid_panel(&mut k, PANEL, 84.0, 1.0);
    let (x, w) = (PANEL.x + 44.0, PANEL.w - 88.0);
    k.text(x, PANEL.y + 36.0, t::LABEL, &gun, t::ACCENT, 0);
    k.text(x, PANEL.y + 60.0, Type { size: 52.0, ..t::DISPLAY }, v.map_or("Standard Issue", |v| v.name), v.map_or(t::INK, |v| v.rarity.color()), 0);
    let camo = v.map_or(0, |v| v.camo);
    k.gun(R::new(PANEL.x + 40.0, PANEL.y + 130.0, PANEL.w - 80.0, 360.0), stat, &format!("{weapon}:"), camo, "");
    let mut y = PANEL.y + 520.0;
    match v {
        Some(v) => {
            k.text(x, y, Type { size: 20.0, ..t::CAPTION }, &format!("{}  \u{b7}  {}", v.rarity.name(), v.game().name()), t::INK_DIM, 0);
            y += 40.0;
            for (text, up) in v.tweak_texts() {
                k.text(x, y, Type { size: 22.0, ..t::BODY }, &text, if up { t::ACCENT } else { t::ENEMY }, 0);
                y += 32.0;
            }
            if v.camo > 0 {
                y += 8.0;
                k.text(x, y, Type { size: 20.0, ..t::CAPTION }, &format!("Signature camo: {}", fe.camo_name(v.camo)), t::REWARD, 0);
            }
        }
        None => {
            k.para(x, y, w, Type { size: 22.0, ..t::BODY }, &format!("The {gun} as issued."), t::INK_DIM, 1.3);
            if none_owned {
                k.para(x, y + 44.0, w, Type { size: 20.0, ..t::CAPTION }, &format!("No {gun} variants yet. Supply drops from the main menu hold them."), t::INK_MUTE, 1.3);
            }
        }
    }
    let note = if id == equipped { "Equipped." } else { "Select to equip." };
    k.text(x, PANEL.bottom() - 60.0, Type { size: 19.0, ..t::CAPTION }, note, t::INK_MUTE, 0);
    cac::back_button(&mut k, hot.as_deref() == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Equip"), (Button::Back, "Back")]);
}
