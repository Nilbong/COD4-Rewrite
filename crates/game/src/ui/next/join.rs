//! Server Browser: the relay's public lobbies, and Find Match.
//!
//! The main menu's Server Browser comes here. On the left, Find Match (the
//! playlist: Team Deathmatch) with what the search is doing, and the
//! browser's filters and Refresh; on the right, every public lobby on the
//! relays (map, game type, players and bots, ping, where it is), a click
//! joining one. The work is [`crate::matchmaking`]'s: this screen sends it
//! requests (`uiScript mm...`) and shows what [`sync`] last copied over.

use super::super::expr::Env;
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, HUD_ICONS, Kit, MENU_ICONS, R, View, icons};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use crate::matchmaking::{Found, Matchmaker, Phase, Playlist, ping_cap};
use crate::modes::GameMode;
use crate::online::Online;
use bevy::prelude::*;
use cod4rw_multiplayer::matchmaking::LobbyState;
use iw3::menu::{Item, Menu, Window as MenuWindow, flags, item_type};
use std::sync::Mutex;

pub const MENU: &str = "next_join";

/// Rows the list shows.
const ROWS: usize = 12;
const LEFT_TOP: f32 = 226.0;
const PITCH: f32 = 54.0;
const LIST: R = R::new(826.0, 192.0, 998.0, 780.0);
const LIST_ROW: f32 = 54.0;
const LIST_TOP: f32 = 192.0 + 70.0;

/// The filters' choices.
const MODES: [Option<GameMode>; 4] = [None, Some(GameMode::Tdm), Some(GameMode::Ffa), Some(GameMode::Tdm3)];
const PINGS: [u16; 4] = [0, 80, 150, 250];

/// A request from the screen, carried out by [`sync`].
enum Request {
    Find,
    Cancel,
    Join(usize),
    Refresh,
    NextMode,
    ToggleFull,
    NextPing,
}

/// What the screen shows, copied from the matchmaker by [`sync`].
#[derive(Default, Clone)]
struct Shown {
    rows: Vec<Found>,
    status: String,
    searching: bool,
    /// Seconds searched, and the ping cap now.
    searched: f32,
    cap: u16,
    /// Relays: "local 2 ms".
    relays: String,
    listed: bool,
    in_lobby: bool,
    mode: usize,
    hide_full: bool,
    ping: usize,
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);
static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());

fn shown() -> Shown {
    SHOWN.lock().ok().and_then(|s| s.clone()).unwrap_or_default()
}

fn left(i: usize) -> R {
    R::new(col_x(0), LEFT_TOP + i as f32 * PITCH, cols(4), 50.0)
}
/// Find Match: the big one.
fn find_rect() -> R {
    R::new(col_x(0), LEFT_TOP, cols(4), 64.0)
}
fn filter_rect(i: usize) -> R {
    left(4 + i)
}
fn list_row(i: usize) -> R {
    R::new(LIST.x + 24.0, LIST_TOP + 38.0 + i as f32 * LIST_ROW, LIST.w - 48.0, LIST_ROW - 4.0)
}

/// A button: `text` is its label for the shot harness (`#Find Match`),
/// not drawn (this file draws it).
fn button(r: R, name: String, text: &str, action: String) -> Item {
    Item {
        text: text.into(),
        ty: item_type::BUTTON,
        window: MenuWindow { rect: virtual_rect(r), dynamic_flags: flags::VISIBLE, name, ..MenuWindow::default() },
        action,
        on_focus: "\"play\" \"mouse_over\" ; ".into(),
        text_scale: 0.0,
        ..Item::default()
    }
}

fn action(cmd: &str) -> String {
    format!("\"play\" \"mouse_click\" ; \"uiScript\" \"{cmd}\" ; ")
}

pub(in crate::ui) fn menu(_fe: &Frontend) -> Menu {
    let s = shown();
    let mut m = Menu::default();
    m.window.name = MENU.into();
    m.full_screen = true;
    m.on_esc = "\"play\" \"mouse_click\" ; \"uiScript\" \"mmCancel\" ; \"close\" \"self\" ; ".into();
    m.items.push(button(find_rect(), "mm_find".into(), "Find Match", action("mmFind")));
    let filters = [("mm_mode", "Game Type", "mmMode"), ("mm_full", "Hide Full", "mmFull"), ("mm_ping", "Max Ping", "mmPing"), ("mm_refresh", "Refresh", "mmRefresh")];
    for (i, (name, label, cmd)) in filters.into_iter().enumerate() {
        m.items.push(button(filter_rect(i), name.into(), label, action(cmd)));
    }
    for i in 0..s.rows.len().min(ROWS) {
        m.items.push(button(list_row(i), format!("mm_row_{i}"), &format!("Lobby {}", i + 1), format!("\"play\" \"mouse_click\" ; \"uiScript\" \"mmJoin\" \"{i}\" ; ")));
    }
    let template = m.items[0].clone();
    m.items.push(cac::back_item(&template, &m.on_esc));
    m
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

/// `uiScript mm...`: `mmFind` (or Cancel while searching), `mmCancel`,
/// `mmJoin <row>`, `mmRefresh`, `mmMode`, `mmFull`, `mmPing`.
pub(in crate::ui) fn script(args: &[String]) {
    let a = |i: usize| args.get(i).map_or("", String::as_str);
    let req = match a(0).to_ascii_lowercase().as_str() {
        "mmfind" => {
            if shown().searching {
                Request::Cancel
            } else {
                Request::Find
            }
        }
        "mmcancel" => Request::Cancel,
        "mmjoin" => Request::Join(a(1).parse().unwrap_or(usize::MAX)),
        "mmrefresh" => Request::Refresh,
        "mmmode" => Request::NextMode,
        "mmfull" => Request::ToggleFull,
        "mmping" => Request::NextPing,
        _ => return,
    };
    if let Ok(mut r) = REQUESTS.lock() {
        r.push(req);
    }
}

/// Carry out the screen's requests and copy what it shows.
pub(in crate::ui) fn sync(mut fe: ResMut<Frontend>, mut mm: ResMut<Matchmaker>, mut online: ResMut<Online>, mut last_rows: Local<usize>) {
    let open = fe.stack.iter().any(|m| m.menu.window.name == MENU);
    mm.browsing = open;
    let mut s = SHOWN.lock().ok().and_then(|s| s.clone()).unwrap_or_default();
    let in_lobby = !matches!(online.status, crate::online::Status::Off | crate::online::Status::Failed(_));
    for r in REQUESTS.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default() {
        match r {
            Request::Find if !in_lobby => mm.find(Playlist::Tdm),
            Request::Find => {}
            Request::Cancel => mm.cancel(),
            Request::Join(i) => {
                if let Some(found) = s.rows.get(i).filter(|f| f.info.open()).cloned()
                    && !in_lobby
                    && !mm.searching()
                {
                    mm.join(&mut online, &found);
                }
            }
            Request::Refresh => mm.refresh(),
            Request::NextMode => {
                s.mode = (s.mode + 1) % MODES.len();
                mm.filter.mode = MODES[s.mode].map(|m| crate::matchmaking::mode_id(m).to_string());
                mm.refresh();
            }
            Request::ToggleFull => {
                s.hide_full = !s.hide_full;
                mm.filter.open_only = s.hide_full;
                mm.refresh();
            }
            Request::NextPing => {
                s.ping = (s.ping + 1) % PINGS.len();
                mm.filter.max_ping = PINGS[s.ping];
                mm.refresh();
            }
        }
    }
    s.rows = mm.lobbies.clone();
    // Debug aid: `COD4RW_MOCKLOBBIES` fills an empty list with made-up
    // lobbies (for screenshots without a relay).
    if s.rows.is_empty() && std::env::var_os("COD4RW_MOCKLOBBIES").is_some() {
        s.rows = mock_lobbies();
        s.listed = true;
    }
    s.status = mm.status.clone();
    s.searching = mm.searching();
    s.searched = mm.searched();
    s.cap = ping_cap(s.searched);
    s.listed |= mm.listed_at.is_some();
    s.in_lobby = in_lobby || matches!(mm.phase, Phase::Joined { .. } | Phase::Hosting { .. });
    s.relays = mm
        .relays
        .iter()
        .map(|r| match r.rtt {
            Some(ms) => format!("{} {ms} ms", r.entry.region),
            None => format!("{} unreachable", r.entry.region),
        })
        .collect::<Vec<_>>()
        .join("   ");
    let rows = s.rows.len().min(ROWS);
    if let Ok(mut shown) = SHOWN.lock() {
        *shown = Some(s);
    }
    // A row more or less: the screen's buttons again (only on top: the
    // lobby may have opened over it).
    if on_top(&fe) && rows != *last_rows {
        fe.close(MENU);
        fe.open(MENU);
        *last_rows = rows;
    }
}

fn mock_lobbies() -> Vec<Found> {
    use cod4rw_multiplayer::matchmaking::{LobbyInfo, Listing};
    let lobby = |map: &str, mode: &str, humans: u8, bots: u8, ping: u16, state: LobbyState, region: &str| Found {
        relay: 0,
        info: LobbyInfo {
            room: humans as u64 * 7 + ping as u64,
            humans,
            ping,
            listing: Listing { mode: mode.into(), map: map.into(), bots, max: 12, state, joinable: humans < 12, region: region.into() },
        },
    };
    vec![
        lobby("mp_crash", "war", 5, 4, 42, LobbyState::InMatch { time_left: 412 }, "eu"),
        lobby("mp_backlot", "dm", 3, 0, 96, LobbyState::Lobby { countdown: 0 }, "eu"),
        lobby("mp_strike", "war", 12, 0, 61, LobbyState::InMatch { time_left: 95 }, "na"),
        lobby("mp_crossfire", "war3p", 2, 6, 180, LobbyState::Lobby { countdown: 8 }, "na"),
        lobby("mp_killhouse", "war", 8, 2, 23, LobbyState::Ending, "local"),
    ]
}

fn mode_name(id: &str) -> &'static str {
    GameMode::ALL.into_iter().find(|m| m.gametype() == id).map_or("?", |m| m.name())
}

fn map_name(id: &str) -> String {
    super::super::lobby::maps().iter().find(|(m, _)| *m == id).map_or_else(|| id.trim_start_matches("mp_").to_owned(), |(_, n)| (*n).to_owned())
}

fn state_text(s: LobbyState) -> String {
    match s {
        LobbyState::Lobby { countdown: 0 } => "In lobby".into(),
        LobbyState::Lobby { countdown } => format!("Starting in {countdown}"),
        LobbyState::InMatch { time_left } => format!("In match {}:{:02}", time_left / 60, time_left % 60),
        LobbyState::Ending => "Ending".into(),
    }
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let s = shown();
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    backdrop(&mut k, 0.55);
    header(&mut k, "Play", "Server Browser");
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i)).map(|it| it.window.name.clone());
    let is_hot = |n: &str| hot.as_deref() == Some(n);

    // Find Match.
    section_icon(&mut k, col_x(0), 192.0, cols(4), "Find match", Some(icons::CROSSHAIR));
    let r = find_rect();
    let label = if s.searching { "Cancel Search".to_string() } else { format!("Find Match: {}", Playlist::Tdm.name()) };
    action_button(&mut k, r, &label, is_hot("mm_find"));
    if !s.searching && !s.in_lobby {
        k.icon(HUD_ICONS, 10, R::new(r.right() - 48.0, r.cy() - 14.0, 28.0, 28.0), t::INK_DIM);
    }
    let mut y = r.bottom() + 22.0;
    if s.searching {
        // A turning arc while it looks.
        let a = fe.millis() as f32 / 1000.0 * 4.0;
        let c = Vec2::new(col_x(0) + 12.0, y + 9.0);
        for i in 0..6 {
            let ang = a + i as f32 * 0.35;
            k.disc(c + Vec2::new(ang.cos(), ang.sin()) * 9.0, 2.0 + i as f32 * 0.25, t::with_alpha(t::ACCENT, 0.3 + i as f32 * 0.12));
        }
        k.text(col_x(0) + 34.0, y, Type { size: 19.0, ..t::CAPTION }, &s.status, t::INK, 0);
        y += 28.0;
        k.text(col_x(0) + 34.0, y, Type { size: 17.0, ..t::CAPTION }, &format!("{:.0} s   ping up to {} ms", s.searched, s.cap), t::INK_DIM, 0);
    } else if !s.status.is_empty() {
        k.para(col_x(0), y, cols(4), Type { size: 19.0, ..t::CAPTION }, &s.status, t::REWARD, 1.3);
    }

    // The browser's filters.
    section_icon(&mut k, col_x(0), filter_rect(0).y - 34.0, cols(4), "Filters", Some(icons::COG));
    let mode = MODES[s.mode].map_or("All", |m| m.name());
    let ping = if PINGS[s.ping] == 0 { "Any".to_string() } else { format!("{} ms", PINGS[s.ping]) };
    row(&mut k, filter_rect(0), "Game Type", Some(mode), is_hot("mm_mode"));
    row(&mut k, filter_rect(1), "Hide Full", Some(if s.hide_full { "On" } else { "Off" }), is_hot("mm_full"));
    row(&mut k, filter_rect(2), "Max Ping", Some(&ping), is_hot("mm_ping"));
    menu_row(&mut k, filter_rect(3), "Refresh", is_hot("mm_refresh"), None);
    k.icon(MENU_ICONS, icons::LINK, R::new(filter_rect(3).right() - if is_hot("mm_refresh") { 78.0 } else { 46.0 }, filter_rect(3).cy() - 12.0, 24.0, 24.0), t::with_alpha(t::INK_DIM, 0.6));
    if !s.relays.is_empty() {
        k.text(col_x(0), filter_rect(3).bottom() + 18.0, Type { size: 16.0, ..t::CAPTION }, &format!("Relays: {}", s.relays), t::INK_MUTE, 0);
    }

    // The list.
    grid_panel(&mut k, LIST, 84.0, 0.6);
    k.icon(MENU_ICONS, icons::CROSSHAIR, R::new(LIST.x + 24.0, LIST.y + 26.0, 22.0, 22.0), t::ACCENT);
    k.text(LIST.x + 58.0, LIST.y + 26.0, t::LABEL, "Public lobbies", t::INK_DIM, 0);
    let count = if s.rows.is_empty() { String::new() } else { format!("{} found", s.rows.len()) };
    k.text(LIST.right() - 24.0, LIST.y + 26.0, t::LABEL, &count, t::INK_MUTE, 2);
    // Columns: map (with its picture), game type, players, ping, state.
    let cols_x = [0.0, 330.0, 520.0, 680.0];
    let head = ["Map", "Game type", "Players", "Ping"];
    for (x, h) in cols_x.iter().zip(head) {
        k.text(LIST.x + 48.0 + x, LIST_TOP + 6.0, t::MICRO, h, t::INK_MUTE, 0);
    }
    k.text(LIST.right() - 40.0, LIST_TOP + 6.0, t::MICRO, "Status", t::INK_MUTE, 2);
    if s.rows.is_empty() {
        let note = if !s.listed { "Looking for lobbies..." } else { "No public lobbies right now. Find Match hosts one." };
        k.text(LIST.x + 48.0, LIST_TOP + 60.0, Type { size: 20.0, ..t::BODY }, note, t::INK_DIM, 0);
    }
    for (i, f) in s.rows.iter().take(ROWS).enumerate() {
        let r = list_row(i);
        let on = is_hot(&format!("mm_row_{i}"));
        let open = f.info.open();
        if on {
            k.grad_h(r, t::ACCENT, 0.22, 0.05);
            k.fill(R::new(r.x, r.y, t::FOCUS_BAR, r.h), t::ACCENT);
        } else {
            k.fill(r, t::hex(0x000000, 0.25));
            k.hline(r.x, r.right(), r.bottom(), 1.0, t::HAIRLINE);
        }
        let ink = if !open { t::INK_MUTE } else if on { t::INK } else { t::INK_DIM };
        let ty = Type { size: 22.0, ..t::ROW_TEXT };
        let l = &f.info.listing;
        // The map: its load screen, cropped to the row.
        let pic = R::new(r.x + 12.0, r.y + 5.0, 76.0, r.h - 10.0);
        let vh = ((4.0 / 3.0) / (pic.w / pic.h)).min(1.0);
        k.fill(pic, t::RAISED);
        let tint = if open { 1.0 } else { 0.45 };
        k.pic_uv(pic, &format!("loadscreen_{}", l.map), Rect::new(0.0, 0.5 - vh * 0.5, 1.0, 0.5 + vh * 0.5), [tint, tint, tint, 1.0]);
        k.frame(pic, 1.0, if on { t::ACCENT } else { t::LINE });
        k.text_mid(pic.right() + 14.0, r.cy(), ty, &map_name(&l.map), ink, 0);
        k.text_mid(r.x + 24.0 + cols_x[1], r.cy(), ty, mode_name(&l.mode), ink, 0);
        // Players: the count, a bar filling up, bots after.
        let px = r.x + 24.0 + cols_x[2];
        k.text_mid(px, r.cy() - 6.0, Type { size: 20.0, ..t::ROW_TEXT }, &format!("{}/{}", f.info.humans, l.max), ink, 0);
        let bar = R::new(px, r.cy() + 10.0, 110.0, 4.0);
        k.fill(bar, t::hex(0xffffff, 0.12));
        let full = f.info.humans as f32 / l.max.max(1) as f32;
        k.fill(R::new(bar.x, bar.y, bar.w * full.clamp(0.0, 1.0), bar.h), if open { t::ACCENT } else { t::INK_MUTE });
        if l.bots > 0 {
            k.text_mid(px + 70.0, r.cy() - 6.0, Type { size: 15.0, ..t::CAPTION }, &format!("+{} bots", l.bots), t::INK_MUTE, 0);
        }
        let ping_ink = match f.info.ping {
            0..=80 => t::ACCENT,
            81..=150 => t::WARN,
            _ => t::ENEMY,
        };
        let cx = r.x + 24.0 + cols_x[3];
        // Signal bars by ping.
        let bars = match f.info.ping { 0..=80 => 3, 81..=150 => 2, _ => 1 };
        for b in 0..3 {
            let h = 6.0 + b as f32 * 5.0;
            k.fill(R::new(cx + b as f32 * 7.0, r.cy() + 8.0 - h, 5.0, h), if b < bars && open { ping_ink } else { t::hex(0xffffff, 0.15) });
        }
        k.text_mid(cx + 30.0, r.cy(), Type { size: 20.0, ..t::ROW_TEXT }, &format!("{} ms", f.info.ping), if open { ink } else { t::INK_MUTE }, 0);
        // Status, and where it is.
        let state = state_text(l.state);
        let why = if open { "" } else if l.joinable { "  \u{b7}  FULL" } else { "  \u{b7}  CLOSED" };
        let right = r.right() - 16.0;
        if !open {
            k.lock(Vec2::new(right - 8.0, r.cy()), 16.0, t::INK_MUTE);
        }
        let sx = right - if open { 0.0 } else { 26.0 };
        k.text_mid(sx, r.cy() - 8.0, Type { size: 16.0, ..t::CAPTION }, &state, if open { t::INK_DIM } else { t::INK_MUTE }, 2);
        k.text_mid(sx, r.cy() + 11.0, t::MICRO, &format!("{}{why}", l.region.to_uppercase()), t::INK_MUTE, 2);
    }

    cac::back_button(&mut k, is_hot("cac_back"));
    let joining = hot.as_deref().is_some_and(|h| h.starts_with("mm_row_"));
    footer(&mut k, "", &[(Button::Confirm, if joining { "Join" } else { "Select" }), (Button::Back, "Back")]);
}
