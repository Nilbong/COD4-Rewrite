//! The new Private Match lobby: the match settings in a column on the left
//! with Start Match and the online buttons under them; on the right the
//! map's banner over the two teams' rosters (you, splitscreen players,
//! friends and bots, their open slots and "+ Add Bot").
//!
//! The classic lobby's own buttons (`lobby.rs`: settings rows, team rows,
//! the code field) are kept and moved, so every setting, bot, splitscreen
//! device and online action stays the classic code's; the drawing is this
//! file's, from [`Frontend::lobby_view`].

use super::super::expr::{Env, eval};
use super::super::lobby::{LobbyRow, lobby_slot};
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, MENU_ICONS, R, View, icons};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Window as MenuWindow, flags, item_type};
use std::sync::atomic::{AtomicI64, Ordering};

pub const MENU: &str = "next_lobby";
const CODE_DVAR: &str = "ui_pm_code";

static OPENED: AtomicI64 = AtomicI64::new(i64::MIN);

// The left column.
const SET_TOP: f32 = 226.0;
const SET_H: f32 = 44.0;
const SET_PITCH: f32 = 48.0;

fn setting_rect(i: usize) -> R {
    R::new(col_x(0), SET_TOP + i as f32 * SET_PITCH, cols(4), SET_H)
}
fn start_rect(n: usize) -> R {
    R::new(col_x(0), SET_TOP + n as f32 * SET_PITCH + 14.0, cols(4), 60.0)
}
fn online_top(n: usize) -> f32 {
    start_rect(n).bottom() + 40.0
}

// The right side.
const BANNER: R = R::new(col_x5(), 192.0, cols7(), 196.0);
const fn col_x5() -> f32 {
    96.0 + 5.0 * (122.0 + 24.0)
}
const fn cols7() -> f32 {
    7.0 * 122.0 + 6.0 * 24.0
}
const TEAM_TOP: f32 = 414.0;
const TEAM_ROW: f32 = 40.0;
const TEAM_PITCH: f32 = 43.0;

fn team_x(s: usize) -> f32 {
    BANNER.x + s as f32 * (team_w() + t::GUTTER)
}
fn team_w() -> f32 {
    (BANNER.w - t::GUTTER) * 0.5
}
fn slot_rect(s: usize, row: usize) -> R {
    R::new(team_x(s), TEAM_TOP + 52.0 + row as f32 * TEAM_PITCH, team_w(), TEAM_ROW)
}
/// Join Team, in the header of the side to join.
fn join_rect(s: usize) -> R {
    R::new(team_x(s) + team_w() - 230.0, TEAM_TOP + 2.0, 160.0, 36.0)
}

/// A splitscreen player's team switch, at the end of their row.
fn switch_rect(s: usize, row: usize) -> R {
    let r = slot_rect(s, row);
    R::new(r.right() - 48.0, r.y, 48.0, r.h)
}

/// Players 2 to 4's profile button, left of their team switch.
fn profile_rect(s: usize, row: usize) -> R {
    let r = switch_rect(s, row);
    R::new(r.x - 54.0, r.y, 48.0, r.h)
}

fn place(item: &Item, r: R, name: String) -> Item {
    let mut it = item.clone();
    it.window = MenuWindow { rect: virtual_rect(r), dynamic_flags: flags::VISIBLE, name, ..MenuWindow::default() };
    it.text_scale = 0.0;
    it.window.style = 0;
    it.window.border = 0;
    it
}

/// The new lobby built from the classic one.
pub(in crate::ui) fn menu(fe: &Frontend, classic: &Menu) -> Option<Menu> {
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = MENU.into();
    // The settings rows (left column), then Start.
    let left: Vec<&Item> =
        classic.items.iter().filter(|it| it.ty == item_type::BUTTON && it.window.rect.horz_align == 1 && it.window.rect.y >= 30.0 && it.window.rect.y < 260.0).collect();
    // (Join Team goes to the teams; Hardcore to the game type screen.)
    let settings: Vec<&&Item> = left
        .iter()
        .filter(|it| !it.action.contains("lobbyStart") && !it.action.contains("lobbyTeam") && !it.action.contains("hardcore"))
        .collect();
    let n = settings.len();
    for (i, it) in settings.iter().enumerate() {
        let mut item = place(it, setting_rect(i), format!("lobby_set_{i}"));
        if it.action.contains("mode") && it.action.contains("lobbyNext") {
            item.action = format!("\"play\" \"mouse_click\" ; \"open\" \"{}\" ; ", super::modes::MENU);
        }
        out.items.push(item);
    }
    if let Some(join) = left.iter().find(|it| it.action.contains("lobbyTeam")) {
        let other = 1 - fe.lobby_view().my_side;
        out.items.push(place(join, join_rect(other), "lobby_join_team".into()));
    }
    if let Some(start) = left.iter().find(|it| it.action.contains("lobbyStart")) {
        out.items.push(place(start, start_rect(n), "lobby_start".into()));
    } else if let Some(start) = left.iter().find(|it| it.action.is_empty() && it.window.rect.y > 230.0) {
        out.items.push(place(start, start_rect(n), "lobby_start".into()));
    }
    // The right side: team rows and the online buttons.
    let top = online_top(n);
    let half = (cols(4) - 12.0) * 0.5;
    for it in classic.items.iter().filter(|it| it.window.rect.horz_align == 3) {
        let r = it.window.rect;
        let a = &it.action;
        let item = if a.contains("lobbyInvite") {
            place(it, R::new(col_x(0), top + 34.0, cols(4), 48.0), "lobby_invite".into())
        } else if a.contains("lobbyLeave") {
            place(it, R::new(col_x(0), top + 104.0, cols(4), 48.0), "lobby_leave".into())
        } else if it.window.name == CODE_DVAR {
            place(it, R::new(col_x(0), top + 96.0, cols(4) - half * 0.5 - 12.0, 48.0), CODE_DVAR.into())
        } else if a.contains("lobbyJoinCode") {
            place(it, R::new(col_x(0) + cols(4) - half * 0.5, top + 96.0, half * 0.5, 48.0), "lobby_join".into())
        } else if let Some((s, row)) = lobby_slot(r.x, r.y) {
            if a.contains("lobbySwitch") {
                place(it, switch_rect(s, row), format!("lobby_switch_{s}_{row}"))
            } else if a.contains("lobbyProfile") {
                place(it, profile_rect(s, row), format!("lobby_profile_{s}_{row}"))
            } else {
                let mut sr = slot_rect(s, row);
                if a.contains("lobbyDevice") {
                    sr.w -= if a.contains("lobbyDevice 0") { 54.0 } else { 108.0 };
                }
                let kind = if a.contains("lobbyAdd") {
                    "add"
                } else if a.contains("lobbyRemove") {
                    "bot"
                } else {
                    "player"
                };
                place(it, sr, format!("lobby_{kind}_{s}_{row}"))
            }
        } else {
            continue;
        };
        out.items.push(item);
    }
    let template = out.items.first()?.clone();
    out.items.push(cac::back_item(&template, &classic.on_esc));
    Some(out)
}

pub(in crate::ui) fn opened(fe: &Frontend) {
    OPENED.store(fe.millis(), Ordering::Relaxed);
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

fn text_of(fe: &Frontend, it: &Item) -> String {
    let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, fe).text() };
    fe.assets.localize(&raw)
}

/// A member's rank badge and level at the row's left; returns where the
/// name starts.
fn rank_badge(k: &mut Kit, r: R, rank: i32, prestige: i32) -> f32 {
    let icon = super::super::hud::rank_icon(k.fe, rank, prestige);
    k.pic(R::new(r.x + 12.0, r.cy() - 13.0, 26.0, 26.0), &icon, [1.0; 4]);
    k.text_mid(r.x + 46.0, r.cy(), t::MICRO, &(rank + 1).to_string(), t::INK_DIM, 0);
    r.x + 82.0
}

/// A small tag at the row's right; returns its left.
fn tag(k: &mut Kit, right: f32, cy: f32, s: &str, accent: bool) -> f32 {
    let w = k.measure(t::MICRO, s) + 16.0;
    let r = R::new(right - w, cy - 11.0, w, 22.0);
    k.frame(r, 1.0, if accent { t::ACCENT } else { t::LINE });
    k.text_mid(r.cx(), r.cy(), t::MICRO, s, if accent { t::ACCENT } else { t::INK_DIM }, 1);
    r.x
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    // The lobby's backdrop.
    let b = k.v.bleed();
    k.fill(b, t::hex(0x050607, 1.0));
    const ASPECT: f32 = 1672.0 / 941.0;
    let (w, h) = if b.w / b.h > ASPECT { (b.w, b.w / ASPECT) } else { (b.h * ASPECT, b.h) };
    k.pic(R::new(b.cx() - w * 0.5, b.cy() - h * 0.5, w, h), "next:bg_lobby", [1.0; 4]);
    k.grad_h(R::new(b.x, b.y, b.w * 0.5, b.h), t::hex(0x050607, 1.0), 0.85, 0.0);
    k.grad_v(R::new(b.x, b.bottom() - 260.0, b.w, 260.0), t::hex(0x050607, 1.0), 0.0, 0.85);
    k.grad_v(R::new(b.x, b.y, b.w, 200.0), t::hex(0x050607, 1.0), 0.7, 0.0);
    let since = (fe.millis() - OPENED.load(Ordering::Relaxed)).max(0) as f32 / 1000.0;
    let ease = t::ease_out(since / t::MEDIUM);
    k.alpha = ease;
    header(&mut k, "Play", "Private Match");
    let view = fe.lobby_view();
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.clone());
    let is_hot = |name: &str| hot.as_deref() == Some(name);
    let item = |name: &str| om.menu.items.iter().enumerate().find(|(i, it)| it.window.name == name && fe.item_visible(om, *i)).map(|(_, it)| it);

    // The settings.
    k.shift = Vec2::new((1.0 - ease) * -20.0, 0.0);
    section_icon(&mut k, col_x(0), 192.0, cols(4), "Match settings", Some(icons::COG));
    let mut n = 0;
    while let Some(it) = item(&format!("lobby_set_{n}")) {
        let name = format!("lobby_set_{n}");
        let s = text_of(fe, it);
        match s.split_once(": ") {
            Some((label, value)) if label == "Game Type" && view.hardcore => {
                row(&mut k, setting_rect(n), label, Some(&format!("{value} (Hardcore)")), is_hot(&name))
            }
            Some((label, value)) => row(&mut k, setting_rect(n), label, Some(value), is_hot(&name)),
            None => menu_row(&mut k, setting_rect(n), &s, is_hot(&name), None),
        }
        n += 1;
    }
    if let Some(it) = item("lobby_start") {
        let r = start_rect(n);
        let label = text_of(fe, it);
        let on = is_hot("lobby_start");
        k.slice3(r, "next:panels/btn_chevron", Vec2::new(396.0, 57.0), 30.0, 80.0, [1.0; 4]);
        if !view.guest {
            k.grad_h(R::new(r.x + 4.0, r.y + 4.0, r.w - 8.0, r.h - 8.0), t::ACCENT, if on { 0.75 } else { 0.4 }, 0.0);
            k.fill(R::new(r.x, r.y, t::FOCUS_BAR, r.h), t::ACCENT);
        }
        k.text_mid(r.x + 28.0, r.cy(), Type { size: 32.0, ..t::H1 }, &label, if view.guest { t::INK_DIM } else { t::INK }, 0);
    }

    // Online.
    let top = online_top(n);
    section_icon(&mut k, col_x(0), top, cols(4), "Play with friends", Some(icons::PEOPLE));
    if let Some(code) = &view.code {
        k.text(col_x(0), top + 40.0, t::MICRO, "Lobby code", t::INK_DIM, 0);
        k.text(col_x(0) + 130.0, top + 30.0, Type { size: 36.0, tracking: 0.12, ..t::NUMERAL }, code, t::ACCENT, 0);
        k.text(col_x(0), top + 74.0, Type { size: 17.0, ..t::CAPTION }, "Give it to friends. It only works for this lobby.", t::INK_MUTE, 0);
    } else if view.guest {
        let host = view.host_name.as_deref().unwrap_or("the host");
        k.text(col_x(0), top + 40.0, Type { size: 22.0, ..t::BODY }, &format!("In {host}'s lobby"), t::INK, 0);
    }
    if let Some(it) = item("lobby_invite") {
        let r = virtual_back(it);
        menu_row(&mut k, r, "Invite Friends", is_hot("lobby_invite"), None);
        k.icon(MENU_ICONS, icons::LINK, R::new(r.right() - if is_hot("lobby_invite") { 76.0 } else { 46.0 }, r.cy() - 12.0, 24.0, 24.0), t::INK_DIM);
    }
    if let Some(it) = item(CODE_DVAR) {
        let r = virtual_back(it);
        let editing = fe.editing.as_ref().is_some_and(|(m, i)| *m == om.name && om.menu.items.get(*i).is_some_and(|x| x.window.name == CODE_DVAR));
        k.fill(r, t::hex(0x000000, 0.5));
        k.frame(r, 1.5, if editing || is_hot(CODE_DVAR) { t::ACCENT } else { t::LINE });
        let mut code = fe.dvar(CODE_DVAR);
        if code.is_empty() && !editing {
            k.text_mid(r.x + 18.0, r.cy(), Type { size: 22.0, ..t::BODY }, "Enter a friend's code", t::INK_MUTE, 0);
        } else {
            if editing && (fe.millis() / 500) % 2 == 0 {
                code.push('_');
            }
            k.text_mid(r.x + 18.0, r.cy(), Type { size: 26.0, tracking: 0.12, ..t::NUMERAL }, &code, t::INK, 0);
        }
    }
    if let Some(it) = item("lobby_join") {
        let r = virtual_back(it);
        action_button(&mut k, r, "Join", is_hot("lobby_join"));
    }
    if let Some(it) = item("lobby_leave") {
        let r = virtual_back(it);
        menu_row(&mut k, r, &text_of(fe, it), is_hot("lobby_leave"), None);
    }
    if !view.note.is_empty() {
        k.text(col_x(0), top + 164.0, Type { size: 18.0, ..t::CAPTION }, &view.note, t::REWARD, 0);
    }

    // The map.
    k.shift = Vec2::new((1.0 - ease) * 20.0, 0.0);
    let (map_id, map_name) = view.map;
    k.fill(BANNER, t::RAISED);
    let a = BANNER.w / BANNER.h;
    let vh = (4.0 / 3.0) / a;
    k.pic_uv(BANNER, &format!("loadscreen_{map_id}"), Rect::new(0.0, 0.5 - vh * 0.5, 1.0, 0.5 + vh * 0.5), [1.0; 4]);
    k.grad_h(BANNER, t::hex(0x050607, 1.0), 0.95, 0.15);
    grid_frame(&mut k, BANNER.inset(-6.0));
    let summary: Vec<String> = (1..4)
        .filter_map(|i| item(&format!("lobby_set_{i}")).map(|it| text_of(fe, it)))
        .map(|s| s.split_once(": ").map_or(s.clone(), |(_, v)| v.to_owned()))
        .collect();
    k.text(BANNER.x + 36.0, BANNER.y + 34.0, t::LABEL, &summary.join("  \u{b7}  "), t::ACCENT, 0);
    k.text(BANNER.x + 36.0, BANNER.y + 62.0, Type { size: 76.0, ..t::DISPLAY }, map_name, t::INK, 0);
    if !view.guest {
        k.text(BANNER.x + 36.0, BANNER.bottom() - 44.0, Type { size: 18.0, ..t::CAPTION }, "Select Map to change it.", t::INK_DIM, 0);
    }

    // The teams, on a dark panel.
    let panel = R::new(BANNER.x - 16.0, TEAM_TOP - 16.0, BANNER.w + 32.0, slot_rect(0, view.team_size).y + 34.0 - (TEAM_TOP - 16.0));
    k.fill(panel, t::hex(0x050706, 0.97));
    k.frame(panel, 1.0, t::HAIRLINE);
    for s in 0..2 {
        let x = team_x(s);
        let colour = if s == 0 { t::FRIENDLY } else { t::ENEMY };
        let rows = &view.teams[s];
        k.fill(R::new(x, TEAM_TOP, 4.0, 40.0), colour);
        k.pic(R::new(x + 16.0, TEAM_TOP + 2.0, 36.0, 36.0), view.team_icons[s], [1.0; 4]);
        let name = view.team_names[s];
        let name = format!("{}{}", &name[..1], name[1..].to_lowercase());
        k.text_mid(x + 62.0, TEAM_TOP + 20.0, t::H1, if s == 1 { "OpFor" } else { &name }, t::INK, 0);
        k.text_mid(x + team_w(), TEAM_TOP + 20.0, t::H2, &format!("{}/{}", rows.len(), view.team_size), t::INK_DIM, 2);
        // Join this side.
        if let Some(it) = item("lobby_join_team").filter(|_| s != view.my_side) {
            let r = join_rect(s);
            let on = is_hot("lobby_join_team");
            k.fill(r, if on { t::with_alpha(t::ACCENT, 0.35) } else { t::hex(0x000000, 0.45) });
            k.frame(r, 1.5, if on { t::ACCENT } else { t::LINE });
            let label = text_of(fe, it);
            let label = if label.to_lowercase().contains("all") { "All Join" } else { "Join" };
            k.text_mid(r.cx(), r.cy(), Type { size: 20.0, ..t::ROW_TEXT }, label, t::INK, 1);
        }
        let add = om.menu.items.iter().find_map(|it| it.window.name.strip_prefix(&format!("lobby_add_{s}_")).and_then(|r| r.parse::<usize>().ok()));
        for slot in 0..view.team_size {
            let r = slot_rect(s, slot);
            match rows.get(slot) {
                Some(LobbyRow::You { name, rank, prestige }) => {
                    k.fill(r, t::with_alpha(colour, 0.12));
                    k.fill(R::new(r.x, r.y, 3.0, r.h), colour);
                    let nx = rank_badge(&mut k, r, *rank, *prestige);
                    k.text_mid(nx, r.cy(), Type { size: 22.0, ..t::ROW_TEXT }, name, t::INK, 0);
                    tag(&mut k, r.right() - 12.0, r.cy(), "You", true);
                }
                Some(LobbyRow::Friend { name, rank, prestige, host }) => {
                    k.fill(r, t::ROW);
                    let nx = rank_badge(&mut k, r, *rank, *prestige);
                    k.text_mid(nx, r.cy(), Type { size: 22.0, ..t::ROW_TEXT }, name, t::INK, 0);
                    if *host {
                        k.icon(MENU_ICONS, icons::CROWN, R::new(r.right() - 40.0, r.cy() - 12.0, 24.0, 24.0), t::REWARD);
                    }
                }
                Some(LobbyRow::Local { player, device, profile }) => {
                    let name = format!("lobby_player_{s}_{slot}");
                    let on = is_hot(&name);
                    let pr = R::new(r.x, r.y, r.w - if *player > 0 { 108.0 } else { 54.0 }, r.h);
                    if on {
                        k.grad_h(pr, t::ACCENT, 0.3, 0.05);
                    } else {
                        k.fill(pr, t::with_alpha(colour, 0.1));
                    }
                    k.fill(R::new(r.x, r.y, 3.0, r.h), colour);
                    let pad = !device.to_lowercase().contains("keyboard");
                    k.icon(MENU_ICONS, if pad { icons::PAD } else { icons::KEYBOARD }, R::new(r.x + 14.0, r.cy() - 12.0, 26.0, 24.0), t::INK);
                    // Player 1 is the profile in use; the others a profile or a guest.
                    let who = match (player, profile) {
                        (0, _) => fe.profile_name(),
                        (_, Some(p)) => p.clone(),
                        (n, None) => format!("Player {} (Guest)", n + 1),
                    };
                    let w = k.text_mid(r.x + 54.0, r.cy(), Type { size: 22.0, ..t::ROW_TEXT }, &who, t::INK, 0);
                    k.text_mid(r.x + 64.0 + w, r.cy(), Type { size: 18.0, ..t::CAPTION }, device, t::INK_DIM, 0);
                    if *player > 0 {
                        let pb = profile_rect(s, slot);
                        let on = is_hot(&format!("lobby_profile_{s}_{slot}"));
                        k.fill(pb, if on { t::with_alpha(t::ACCENT, 0.4) } else { t::hex(0x000000, 0.45) });
                        k.frame(pb, 1.0, if on { t::ACCENT } else { t::LINE });
                        k.icon(MENU_ICONS, icons::STAR, R::new(pb.cx() - 11.0, pb.cy() - 11.0, 22.0, 22.0), if profile.is_some() { t::ACCENT } else { t::INK_DIM });
                    }
                    let sw = switch_rect(s, slot);
                    let on = is_hot(&format!("lobby_switch_{s}_{slot}"));
                    k.fill(sw, if on { t::with_alpha(t::ACCENT, 0.4) } else { t::hex(0x000000, 0.45) });
                    k.frame(sw, 1.0, if on { t::ACCENT } else { t::LINE });
                    k.chevron(Vec2::new(sw.cx(), sw.cy()), 18.0, if s == 0 { 0 } else { 2 }, 2.5, t::INK);
                }
                Some(LobbyRow::Bot { name }) => {
                    let on = is_hot(&format!("lobby_bot_{s}_{slot}"));
                    if on {
                        k.grad_h(r, t::ENEMY, 0.25, 0.03);
                    } else {
                        k.fill(r, t::ROW);
                    }
                    k.icon(MENU_ICONS, icons::BOT, R::new(r.x + 14.0, r.cy() - 11.0, 24.0, 22.0), t::INK_DIM);
                    k.text_mid(r.x + 54.0, r.cy(), Type { size: 22.0, ..t::ROW_TEXT }, name, t::INK_DIM, 0);
                    if on {
                        k.text_mid(r.right() - 14.0, r.cy(), t::MICRO, "Remove", t::ENEMY, 2);
                    } else {
                        tag(&mut k, r.right() - 12.0, r.cy(), "Bot", false);
                    }
                }
                None if add == Some(slot) => {
                    let on = is_hot(&format!("lobby_add_{s}_{slot}"));
                    if on {
                        k.grad_h(r, t::ACCENT, 0.3, 0.05);
                    }
                    k.frame(r, 1.0, if on { t::ACCENT } else { t::with_alpha(t::ACCENT, 0.5) });
                    k.plus(Vec2::new(r.x + 26.0, r.cy()), 14.0, 2.5, t::ACCENT);
                    k.text_mid(r.x + 54.0, r.cy(), Type { size: 22.0, ..t::ROW_TEXT }, "Add Bot", if on { t::INK } else { t::INK_DIM }, 0);
                }
                None => {
                    k.frame(r, 1.0, t::hex(0xffffff, 0.05));
                    k.text_mid(r.x + 54.0, r.cy(), Type { size: 19.0, ..t::ROW_TEXT }, "Open", t::INK_MUTE, 0);
                }
            }
        }
    }
    let hint = if view.guest {
        "The host picks the settings."
    } else if view.split {
        "Select a player to change their controller, the arrow to switch their team."
    } else {
        "Select a bot to remove it."
    };
    k.text(BANNER.x, slot_rect(0, view.team_size).y + 2.0, Type { size: 17.0, ..t::CAPTION }, hint, t::INK_MUTE, 0);
    k.shift = Vec2::ZERO;
    cac::back_button(&mut k, is_hot("cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Select"), (Button::Back, "Leave")]);
}

/// An item's place in design units (its rect was set from one).
fn virtual_back(it: &Item) -> R {
    let r = it.window.rect;
    let k = 1080.0 / 480.0;
    R::new(r.x * k + 960.0, r.y * k, r.w * k, r.h * k)
}
