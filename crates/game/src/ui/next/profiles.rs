//! Select Profile in the new style (CoD4's `player_profile`): the profiles
//! as a column list (the one in use ticked), New and Delete under them,
//! and the focused profile's name and rank on the right. Picking a profile
//! uses it ([`super::super::profiles`]); Delete asks first (CoD4's own
//! popup) and deletes the profile last focused.

use super::super::expr::Env;
use super::super::profiles::{self, Profile, SELECTED_DVAR};
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, MENU_ICONS, R, View, icons};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Window as MenuWindow, flags, item_type};

pub const MENU: &str = "next_profiles";
const TOP: f32 = 176.0;
const PITCH: f32 = 50.0;
const PANEL: R = R::new(826.0, 192.0, 998.0, 560.0);
/// The most profiles listed.
const MAX: usize = 10;

fn button(r: R, name: String, text: &str, action: String, on_focus: String) -> Item {
    Item {
        ty: item_type::BUTTON,
        window: MenuWindow { rect: virtual_rect(r), dynamic_flags: flags::VISIBLE, name, ..MenuWindow::default() },
        text: text.into(),
        action,
        on_focus,
        text_scale: 0.0,
        ..Item::default()
    }
}

/// The rows' places: profiles, then New and Delete.
fn layout(n: usize) -> (Vec<R>, f32, R, R) {
    let row = |y: f32| R::new(col_x(0), y, cols(4), 48.0);
    let mut y = TOP + 34.0;
    let rows = (0..n).map(|_| {
        let r = row(y);
        y += PITCH;
        r
    });
    let rows: Vec<R> = rows.collect();
    let manage = y + 22.0;
    (rows, manage, row(manage + 34.0), row(manage + 34.0 + PITCH))
}

pub(in crate::ui) fn menu(classic: &Menu) -> Menu {
    let list: Vec<Profile> = profiles::list().into_iter().take(MAX).collect();
    let mut m = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    m.window.name = MENU.into();
    let (rows, _, new, delete) = layout(list.len());
    for (p, r) in list.iter().zip(rows) {
        m.items.push(button(
            r,
            format!("profile_{}", p.id),
            &p.name,
            format!("\"play\" \"mouse_click\" ; \"setdvar\" \"ui_playerProfileAlreadyChosen\" 1 ; \"uiScript\" \"loadProfile\" \"{}\" ; \"close\" \"player_profile\" ; ", p.id),
            format!("\"play\" \"mouse_over\" ; \"uiScript\" \"selectProfile\" \"{}\" ; ", p.id),
        ));
    }
    let over = "\"play\" \"mouse_over\" ; ".to_owned();
    m.items.push(button(new, "profile_new".into(), "New Profile", "\"play\" \"mouse_click\" ; \"open\" \"profile_create_popmenu\" ; ".into(), over.clone()));
    if list.len() > 1 {
        m.items.push(button(delete, "profile_delete".into(), "Delete Profile", "\"play\" \"mouse_click\" ; \"open\" \"profile_del_sure_popmenu\" ; ".into(), over));
    }
    let template = m.items[0].clone();
    let back = "\"play\" \"mouse_click\" ; \"setdvar\" \"ui_playerProfileAlreadyChosen\" 1 ; \"close\" \"player_profile\" ; ";
    m.on_esc = back.into();
    m.items.push(cac::back_item(&template, back));
    m
}

/// Focus the profile in use.
pub(in crate::ui) fn first_focus(menu: &Menu) -> Option<usize> {
    (menu.window.name == MENU).then(|| menu.items.iter().position(|it| it.window.name == format!("profile_{}", profiles::active()))).flatten()
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    backdrop(&mut k, 0.5);
    header(&mut k, "System", "Select Profile");
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.as_str());
    let list: Vec<Profile> = profiles::list().into_iter().take(MAX).collect();
    let active = profiles::active();
    let (rows, manage, new, delete) = layout(list.len());
    section_icon(&mut k, col_x(0), TOP, cols(4), "Profiles", Some(icons::STAR));
    for (p, r) in list.iter().zip(rows) {
        let name = format!("profile_{}", p.id);
        let on = hot == Some(name.as_str());
        menu_row(&mut k, r, &p.name, on, None);
        let level = super::super::combat_record::rank_at(fe, p.xp).level;
        let right = r.right() - if on { 56.0 } else { 20.0 };
        let w = k.text_mid(right, r.cy(), Type { size: 18.0, ..t::CAPTION }, &format!("LV {level}"), t::INK_MUTE, 2);
        if p.id == active {
            k.check(Vec2::new(right - w - 24.0, r.cy()), 16.0, 2.5, t::ACCENT);
        }
    }
    section(&mut k, col_x(0), manage, cols(4), "Manage");
    menu_row(&mut k, new, "New Profile", hot == Some("profile_new"), None);
    k.icon(MENU_ICONS, icons::PEOPLE, R::new(new.right() - if hot == Some("profile_new") { 78.0 } else { 46.0 }, new.cy() - 12.0, 24.0, 24.0), t::with_alpha(t::INK_DIM, 0.6));
    if om.menu.items.iter().any(|it| it.window.name == "profile_delete") {
        menu_row(&mut k, delete, "Delete Profile", hot == Some("profile_delete"), None);
    }

    // The profile focused (else the one last picked).
    let selected = fe.dvar(SELECTED_DVAR);
    let shown = list.iter().find(|p| hot == Some(format!("profile_{}", p.id).as_str())).or_else(|| list.iter().find(|p| p.id == selected)).or(list.first());
    if let Some(p) = shown {
        grid_panel(&mut k, PANEL, 84.0, 1.0);
        let (x, w) = (PANEL.x + 44.0, PANEL.w - 88.0);
        let rank = super::super::combat_record::rank_at(fe, p.xp);
        k.text(x, PANEL.y + 36.0, t::LABEL, if p.id == active { "In use" } else { "Profile" }, t::ACCENT, 0);
        k.text(x, PANEL.y + 60.0, Type { size: 56.0, ..t::DISPLAY }, &p.name, t::INK, 0);
        k.slice3(R::new(x, PANEL.y + 136.0, w, 14.0), "next:panels/divider", Vec2::new(354.0, 28.0), 120.0, 120.0, [1.0; 4]);
        if !rank.icon.is_empty() {
            icon_box(&mut k, R::new(x, PANEL.y + 176.0, 120.0, 120.0));
            k.pic(R::new(x + 14.0, PANEL.y + 190.0, 92.0, 92.0), &rank.icon, [1.0; 4]);
        }
        k.text(x + 150.0, PANEL.y + 190.0, t::LABEL, &format!("Level {}", rank.level), t::INK_DIM, 0);
        k.text(x + 150.0, PANEL.y + 214.0, Type { size: 40.0, ..t::H1 }, &rank.name, t::INK, 0);
        let f = rank.next.map_or(1.0, |n| (rank.xp - rank.min) as f32 / (n - rank.min).max(1) as f32);
        let bar = R::new(x + 150.0, PANEL.y + 276.0, w - 150.0, 6.0);
        k.fill(bar, t::hex(0xffffff, 0.12));
        k.fill(R::new(bar.x, bar.y, bar.w * f.clamp(0.0, 1.0), bar.h), t::ACCENT);
        k.text(bar.right(), bar.bottom() + 12.0, t::MICRO, &format!("{} XP", rank.xp), t::INK_DIM, 2);
        let line = match hot {
            Some("profile_delete") => format!("Delete {}? It's kept in the deleted folder, not erased.", p.name),
            Some("profile_new") => "Start a new profile from rank one, with CoD4's unlocks and your settings.".to_owned(),
            _ if p.id == active => "Your progress, classes and settings are saved here.".to_owned(),
            _ => "Select to play as this profile: its rank, classes and settings.".to_owned(),
        };
        k.para(x, PANEL.bottom() - 90.0, w, Type { size: 20.0, ..t::BODY }, &line, t::INK_DIM, 1.3);
    }
    cac::back_button(&mut k, hot == Some("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Select"), (Button::Back, "Back")]);
}
