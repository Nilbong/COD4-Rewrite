//! The server browser: CoD4's Join Game menu (`pc_join_unranked`) over
//! [`crate::net::Browser`]. CoD4 fills the menu from code, so this does
//! too: its list box (`FEEDER_SERVERS`), the source, game type and refresh
//! time owner draws, and its `uiScript`s (refresh, sort, favourites, join).
//! Joining waits for multiplayer: Join Server says so.

use super::draw::{self, Placement};
use super::expr::Env;
use super::{Frontend, Op};
use crate::net::{self, SortKey, Source};
use bevy::prelude::*;
use iw3::menu::{Item, ItemData};
use std::time::{Duration, Instant};

/// CoD4's `FEEDER_SERVERS`: the list box showing servers.
pub(super) const FEEDER_SERVERS: f32 = 2.0;
/// Owner draws: `UI_NETSOURCE`, `UI_SERVERREFRESHDATE`, `UI_JOINGAMETYPE`.
const NETSOURCE: i32 = 220;
const REFRESH_DATE: i32 = 247;
const JOIN_GAMETYPE: i32 = 253;
/// The game types the filter cycles through (`None` is all of them).
const GAMETYPES: [Option<&str>; 7] = [None, Some("war"), Some("dm"), Some("dom"), Some("sd"), Some("koth"), Some("sab")];
/// Two clicks on a row this close together join it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// What the menu asks of the browser (done by [`sync_servers`]).
pub(super) enum Command {
    Refresh(Source),
    Requery,
    Sort(SortKey),
    Filter(Option<String>),
    AddFavorite(String),
    RemoveFavorite(String),
}

/// A server as the list shows it.
#[derive(Clone, Debug, Default)]
pub(super) struct Row {
    pub addr: String,
    pub name: String,
    pub map: String,
    pub players: String,
    pub gametype: String,
    pub ping: String,
    pub password: bool,
}

/// The menu's side of the browser.
#[derive(Default)]
pub(super) struct ServerList {
    pub rows: Vec<Row>,
    pub selected: Option<usize>,
    pub scroll: usize,
    /// Seconds since the list was asked for.
    pub age: Option<u64>,
    pub status: Option<String>,
    pub commands: Vec<Command>,
    pub last_click: Option<(usize, Instant)>,
    pub source: Source,
    /// Index into [`GAMETYPES`].
    pub gametype: usize,
    /// The window's size, for working out which row a click is on.
    pub window: Vec2,
}

impl Frontend {
    fn loc(&self, key: &str, fallback: &str) -> String {
        let s = self.assets.localize(key);
        if s.eq_ignore_ascii_case(key.trim_start_matches('@')) { fallback.to_owned() } else { s }
    }

    /// The owner draw flags that show items only for favourites
    /// (`UI_SHOW_FAVORITESERVERS`, 4) or only for other sources
    /// (`UI_SHOW_NOTFAVORITESERVERS`, 4096): Del. Favorite and Add to
    /// Favorites.
    pub(super) fn browser_shows(&self, flags: u32) -> bool {
        let favorites = self.servers.source == Source::Favorites;
        !(flags & 4 != 0 && !favorites || flags & 4096 != 0 && favorites)
    }

    /// An owner draw's text, if it's one of the browser's.
    pub(super) fn browser_owner_draw(&self, id: i32) -> Option<String> {
        match id {
            NETSOURCE => {
                let source = match self.servers.source {
                    Source::Local => self.loc("@MENU_LOCAL", "Local"),
                    Source::Internet => self.loc("@MENU_INTERNET", "Internet"),
                    Source::Favorites => self.loc("@MENU_FAVORITES", "Favorites"),
                };
                Some(format!("{} {source}", self.loc("@MENU_SOURCE", "Source:")))
            }
            JOIN_GAMETYPE => Some(match GAMETYPES[self.servers.gametype] {
                None => self.loc("@MENU_ALL", "All"),
                Some(g) => gametype_name(g, false),
            }),
            REFRESH_DATE => Some(match (&self.servers.status, self.servers.age) {
                (Some(s), _) => s.clone(),
                (None, Some(a)) => format!("{} {a}s ago", self.loc("@MENU_REFRESH_TIME", "Refresh Time:")),
                (None, None) => String::new(),
            }),
            _ => None,
        }
    }

    /// A click on one of the browser's owner draws: the next source or
    /// game type.
    pub(super) fn browser_owner_draw_click(&mut self, id: i32) {
        match id {
            NETSOURCE => {
                let next = match self.servers.source {
                    Source::Local => Source::Internet,
                    Source::Internet => Source::Favorites,
                    Source::Favorites => Source::Local,
                };
                self.set_source(next);
            }
            JOIN_GAMETYPE => {
                self.servers.gametype = (self.servers.gametype + 1) % GAMETYPES.len();
                let g = GAMETYPES[self.servers.gametype].map(str::to_owned);
                self.servers.commands.push(Command::Filter(g));
            }
            _ => {}
        }
    }

    fn set_source(&mut self, source: Source) {
        self.servers.source = source;
        let n = match source {
            Source::Local => 0,
            Source::Internet => 1,
            Source::Favorites => 2,
        };
        self.set_dvar("ui_netSource", &n.to_string());
        self.servers.selected = None;
        self.servers.scroll = 0;
        self.servers.commands.push(Command::Refresh(source));
    }

    /// The browser's `uiScript`s; false for anything else.
    pub(super) fn browser_script(&mut self, args: &[String]) -> bool {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        let selected = self.servers.selected.and_then(|i| self.servers.rows.get(i)).map(|r| r.addr.clone());
        match a(0).to_ascii_lowercase().as_str() {
            // Opening the menu, and Refresh List.
            "updatefilter" | "refreshservers" => {
                let source = Source::from_dvar(super::num(&self.dvar("ui_netSource")));
                self.set_source(source);
            }
            // Quick Refresh: the ones listed.
            "refreshfilter" => self.servers.commands.push(Command::Requery),
            "serversort" => self.servers.commands.push(Command::Sort(SortKey::from_column(super::num(a(1))))),
            "joinserver" => self.join(),
            "addfavorite" => {
                if let Some(addr) = selected {
                    self.servers.commands.push(Command::AddFavorite(addr));
                }
            }
            "deletefavorite" => {
                if let Some(addr) = selected {
                    self.servers.commands.push(Command::RemoveFavorite(addr));
                }
            }
            "createfavorite" => {
                let addr = self.dvar("ui_favoriteAddress");
                self.servers.commands.push(Command::AddFavorite(addr));
            }
            "closejoin" | "serverstatus" | "stopserverrefresh" => {}
            _ => return false,
        }
        true
    }

    /// Join Server: nothing to join with yet.
    fn join(&mut self) {
        let Some(row) = self.servers.selected.and_then(|i| self.servers.rows.get(i)).cloned() else { return };
        info!("ui: join {} ({}) asked for; there's no multiplayer yet", row.name, row.addr);
        self.set_dvar("com_errorMessage", &format!("Can't join {} yet: multiplayer is still to come.", row.name));
        self.open("error_popmenu");
    }

    /// A click in the server list: pick the row (twice quickly: join it).
    pub(super) fn browser_click(&mut self, item: &Item, cursor: Option<Vec2>) {
        let (Some(p), ItemData::ListBox(list)) = (cursor, &item.data) else { return };
        let pl = Placement::new(self.servers.window.x, self.servers.window.y);
        let r = self.item_rect(item);
        let (pos, _) = pl.rect(&r);
        let row_h = list.element_height * pl.sy(r.vert_align);
        if row_h <= 0.0 {
            return;
        }
        let row = self.servers.scroll + ((p.y - pos.y) / row_h).floor().max(0.0) as usize;
        if row >= self.servers.rows.len() {
            return;
        }
        let double = self.servers.last_click.is_some_and(|(r, at)| r == row && at.elapsed() < DOUBLE_CLICK);
        self.servers.selected = Some(row);
        self.servers.last_click = Some((row, Instant::now()));
        if double {
            self.join();
        }
    }

    /// The mouse wheel over the list.
    pub(super) fn browser_scroll(&mut self, item: &Item, lines: i32) {
        let ItemData::ListBox(list) = &item.data else { return };
        let visible = (self.item_rect(item).h / list.element_height.max(1.0)) as usize;
        let max = self.servers.rows.len().saturating_sub(visible.max(1));
        self.servers.scroll = (self.servers.scroll as i32 - lines).clamp(0, max as i32) as usize;
    }

    /// The list: a row per server under the menu's column headers.
    pub(super) fn paint_servers(&self, item: &Item, pl: &Placement, ops: &mut Vec<Op>) {
        let ItemData::ListBox(list) = &item.data else { return };
        let r = self.item_rect(item);
        let (pos, size) = pl.rect(&r);
        let (sx, sy) = (pl.sx(r.horz_align), pl.sy(r.vert_align));
        let row_h = list.element_height * sy;
        let height = item.text_scale.max(0.25) * 48.0 * sy;
        let font = self.font_for(item.font_enum, height);
        let f = &self.assets.fonts[font];
        let k = height / f.pixel_height as f32;
        let visible = (size.y / row_h.max(1.0)) as usize;
        let white = [1.0, 1.0, 1.0, 0.9];
        if self.servers.rows.is_empty() {
            let note = match self.servers.source {
                Source::Favorites => "No favourites yet: New Favorite adds one.",
                _ if self.servers.age.is_some_and(|a| a < 3) => "Searching...",
                Source::Local => "No games on this network.",
                Source::Internet => "No games listed by the master server.",
            };
            let w = draw::text_width(f, note, k);
            ops.push(Op::Text { text: note.into(), x: pos.x + (size.x - w) * 0.5, y: pos.y + row_h * 2.0, font, k, color: white, shadow: 0.0 });
            return;
        }
        for (n, row) in self.servers.rows.iter().enumerate().skip(self.servers.scroll).take(visible) {
            let top = pos.y + (n - self.servers.scroll) as f32 * row_h;
            if self.servers.selected == Some(n) {
                ops.push(Op::Fill { pos: Vec2::new(pos.x, top), size: Vec2::new(size.x, row_h), color: [0.1, 0.2, 0.37, 0.8] });
            }
            let cells: [(usize, &str); 6] = [
                (0, if row.password { "*" } else { "" }),
                (2, &row.name),
                (3, &row.map),
                (4, &row.players),
                (5, &row.gametype),
                (10, &row.ping),
            ];
            for (c, text) in cells {
                let Some(col) = list.columns.get(c) else { continue };
                let max = col.max_chars.max(1) as usize;
                let text: String = text.chars().take(max).collect();
                let x = pos.x + col.pos as f32 * sx + 2.0 * sx;
                ops.push(Op::Text { text, x, y: top + row_h * 0.5 + height * 0.35, font, k, color: white, shadow: 0.0 });
            }
        }
    }
}

/// A game type's name in the list (`war` → "Team Deathmatch").
fn gametype_name(g: &str, hardcore: bool) -> String {
    let name = crate::modes::GameMode::ALL.into_iter().find(|m| m.gametype() == g).map_or(g, |m| m.name());
    if hardcore { format!("HC {name}") } else { name.to_owned() }
}

/// A map's name (`mp_crash` → "Crash").
fn map_name(map: &str) -> String {
    super::lobby::MAPS.iter().find(|(m, _)| *m == map).map_or_else(|| map.trim_start_matches("mp_").to_owned(), |(_, n)| (*n).to_owned())
}

/// Carry out the menu's requests and show what the browser has found.
pub(super) fn sync_servers(mut fe: ResMut<Frontend>, mut browser: ResMut<net::Browser>, window: Option<Single<&Window>>) {
    if let Some(w) = window {
        fe.servers.window = Vec2::new(w.width(), w.height());
    }
    for c in std::mem::take(&mut fe.servers.commands) {
        match c {
            Command::Refresh(s) => browser.refresh(s),
            Command::Requery => browser.requery(),
            Command::Sort(k) => browser.sort_by(k),
            Command::Filter(g) => browser.gametype = g,
            Command::AddFavorite(a) => browser.add_favorite(&a),
            Command::RemoveFavorite(a) => {
                browser.remove_favorite(&a);
                fe.servers.selected = None;
            }
        }
    }
    if !fe.menu_open("pc_join_unranked") && browser.refreshed.is_none() {
        return;
    }
    fe.servers.rows = browser
        .shown()
        .into_iter()
        .map(|s| Row {
            addr: s.addr.to_string(),
            name: s.name.clone(),
            map: map_name(&s.map),
            players: if s.max_players > 0 { format!("{}/{}", s.players, s.max_players) } else { String::new() },
            gametype: if s.gametype.is_empty() { String::new() } else { gametype_name(&s.gametype, s.hardcore) },
            ping: s.ping.map_or_else(|| "---".into(), |p| p.to_string()),
            password: s.password,
        })
        .collect();
    fe.servers.age = browser.refreshed.and_then(|t| t.elapsed().ok()).map(|d| d.as_secs());
    fe.servers.status = browser.status.clone();
    if fe.servers.selected.is_some_and(|i| i >= fe.servers.rows.len()) {
        fe.servers.selected = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_for_the_list() {
        assert_eq!(map_name("mp_crash"), "Crash");
        assert_eq!(map_name("mp_unknown"), "unknown");
        assert_eq!(gametype_name("war", false), "Team Deathmatch");
        assert!(gametype_name("sd", true).starts_with("HC "));
    }
}
