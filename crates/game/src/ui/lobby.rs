//! Private Match: the lobby that replaces Start New Server on the main menu.
//!
//! The left column sets the match up (map, game type, time and score
//! limits, bot difficulty, which side you're on, Hardcore, splitscreen) and
//! starts it, with the chosen map's picture and name under it. On the right
//! are the two teams as in a console lobby, with "+ Add Bot" under each;
//! clicking a bot takes it out. With splitscreen on, the local players are
//! listed with their devices ("Player 2 - DualSense Edge"); clicking one
//! moves them to the next device. Built from Create a Class's frame and
//! rows, like the supply drop screens, so the menu interpreter lays it out
//! and runs it. Under the teams, Invite Friends and Join with Code take the
//! lobby online ([`online`]).

#[path = "lobby_online.rs"]
mod online;
pub(super) use online::online_lobby;
use online::{CODE_DVAR, OnlineLobby};

use super::attachments::{highlight_rows, retarget_highlight};
use super::draw::{self, Placement};
use super::expr::Env;
use super::{Frontend, Op, OpenMenu};
use crate::combat::Team;
use crate::modes::GameMode;
use crate::splitscreen::{Device, LocalPlayers, MAX_PLAYERS};
use crate::tdm::{MatchConfig, bot_name};
use iw3::menu::{EditField, Item, ItemData, Menu, Rect as VRect, Statement, Token, flags, item_type, op};
use std::sync::Arc;

pub(super) const LOBBY_MENU: &str = "private_match";
const MAPS_MENU: &str = "private_match_maps";
/// The map under the mouse on the map list.
const HOVER_DVAR: &str = "ui_pm_hover";

/// The maps that load, with their names in CoD4.
pub const MAPS: [(&str, &str); 21] = [
    ("mp_convoy", "Ambush"),
    ("mp_backlot", "Backlot"),
    ("mp_bloc", "Bloc"),
    ("mp_bog", "Bog"),
    ("mp_broadcast", "Broadcast"),
    ("mp_carentan", "Chinatown"),
    ("mp_countdown", "Countdown"),
    ("mp_crash", "Crash"),
    ("mp_creek", "Creek"),
    ("mp_crossfire", "Crossfire"),
    ("mp_citystreets", "District"),
    ("mp_farm", "Downpour"),
    ("mp_killhouse", "Killhouse"),
    ("mp_overgrown", "Overgrown"),
    ("mp_pipeline", "Pipeline"),
    ("mp_shipment", "Shipment"),
    ("mp_showdown", "Showdown"),
    ("mp_strike", "Strike"),
    ("mp_vacant", "Vacant"),
    ("mp_cargoship", "Wet Work"),
    ("mp_crash_snow", "Winter Crash"),
];
/// Minutes.
const TIMES: [u32; 6] = [5, 10, 15, 20, 30, 60];
const DIFFICULTY: [(&str, f32); 4] = [("Recruit", 0.25), ("Regular", 0.5), ("Hardened", 0.7), ("Veteran", 0.9)];
/// Players a side, you included.
const TEAM_SIZE: usize = 9;

const SIDES: [Team; 2] = [Team::Allies, Team::Axis];
const TEAM_NAMES: [&str; 2] = ["MARINES", "OPFOR"];
const TEAM_ICONS: [&str; 2] = ["faction_128_usmc", "faction_128_arab"];

const GREY: [f32; 4] = [0.69, 0.69, 0.69, 1.0];
const GOLD: [f32; 4] = [1.0, 0.85, 0.45, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Team panels: two columns from the right edge (right-aligned units).
const PANEL_X: [f32; 2] = [-420.0, -212.0];
const PANEL_W: f32 = 196.0;
const PANEL_TOP: f32 = 34.0;
const ROW_H: f32 = 20.0;
/// A splitscreen player's switch-team arrow, at the end of their row,
/// pointing at the other team's panel.
const SWITCH_W: f32 = 24.0;
const SWITCH_ARROW: [&str; 2] = [">>", "<<"];
/// The online box, under the teams.
const ONLINE_TOP: f32 = PANEL_TOP + TEAM_SIZE as f32 * ROW_H + 96.0;

/// The lobby's settings.
pub(super) struct Lobby {
    map: usize,
    mode: usize,
    time: usize,
    score: usize,
    difficulty: usize,
    hardcore: bool,
    /// Local players (1: no splitscreen), and each one's device and team.
    players: usize,
    devices: [Device; MAX_PLAYERS],
    teams: [Team; MAX_PLAYERS],
    /// Bot names per side: Allies, Axis.
    bots: [Vec<String>; 2],
    /// Start Match was pressed: the next match is this lobby's.
    starting: bool,
    /// Friends in the lobby, through the relay.
    online: OnlineLobby,
}

impl Default for Lobby {
    fn default() -> Self {
        Lobby {
            map: MAPS.iter().position(|m| m.0 == "mp_killhouse").unwrap_or(0),
            mode: 0,
            time: 1,
            score: 2,
            difficulty: 1,
            hardcore: false,
            players: 1,
            devices: [Device::KeyboardMouse, Device::NextPad, Device::NextPad, Device::NextPad],
            teams: [Team::Allies; MAX_PLAYERS],
            bots: [Vec::new(), Vec::new()],
            starting: false,
            online: OnlineLobby::default(),
        }
    }
}

fn side(team: Team) -> usize {
    (team == Team::Axis) as usize
}

impl Lobby {
    fn mode(&self) -> GameMode {
        GameMode::ALL[self.mode]
    }

    /// Kills to win: a team's, or in free-for-all a player's.
    fn score_limit(&self) -> u32 {
        let (limits, default) = self.mode().score_limits();
        limits.get(self.score).or(limits.get(default)).copied().unwrap_or(crate::tdm::SCORE_LIMIT)
    }

    pub(super) fn map_id(&self) -> &'static str {
        MAPS[self.map].0
    }

    pub(super) fn config(&self) -> MatchConfig {
        MatchConfig {
            // An online guest's bots are the host's: seen, not played.
            bots: if self.online.guest() { [Vec::new(), Vec::new()] } else { self.bots.clone() },
            player_team: self.teams[0],
            time_limit: TIMES[self.time] as f32 * 60.0,
            score_limit: self.score_limit(),
            bot_skill: DIFFICULTY[self.difficulty].1,
            mode: self.mode(),
            hardcore: self.hardcore,
        }
    }

    /// The match to start, once, after Start Match: its settings and its
    /// local players.
    pub(super) fn take_start(&mut self) -> Option<(MatchConfig, LocalPlayers)> {
        std::mem::take(&mut self.starting).then(|| (self.config(), self.local_players()))
    }

    pub(super) fn local_players(&self) -> LocalPlayers {
        if self.players < 2 {
            return LocalPlayers::default();
        }
        LocalPlayers { devices: self.devices[..self.players].to_vec(), teams: self.teams[..self.players].to_vec() }
    }

    /// The local players on `s`.
    fn locals_on(&self, s: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.players).filter(move |&i| side(self.teams[i]) == s)
    }

    /// Players on `s` (you, any splitscreen players and friends included).
    fn count(&self, s: usize) -> usize {
        self.bots[s].len() + self.locals_on(s).count() + self.online.others_on(s).count()
    }

    /// Splitscreen: one more local player, back to one after four (or with
    /// both sides full). A new player joins Player 1's side if there's room,
    /// and takes a device nobody has.
    fn next_players(&mut self, devices: &[(Device, String)]) {
        let first = side(self.teams[0]);
        let room = [first, 1 - first].into_iter().find(|&s| self.count(s) < TEAM_SIZE);
        match room.filter(|_| self.players < MAX_PLAYERS) {
            Some(s) => {
                self.teams[self.players] = SIDES[s];
                self.players += 1;
            }
            None => self.players = 1,
        }
        for i in 1..self.players {
            let taken = |d: Device, lobby: &Lobby| (0..lobby.players).any(|j| j != i && lobby.devices[j] == d);
            if self.devices[i] == Device::NextPad || taken(self.devices[i], self) {
                self.devices[i] = devices.iter().map(|(d, _)| *d).find(|d| !taken(*d, self)).unwrap_or(Device::NextPad);
            }
        }
    }

    /// A player's next device (swapping with whoever has it), then "Any
    /// Controller" (the next one connected), then round again.
    fn next_device(&mut self, i: usize, devices: &[(Device, String)]) {
        if i >= self.players {
            return;
        }
        let mut list: Vec<Device> = devices.iter().map(|(d, _)| *d).collect();
        list.push(Device::NextPad);
        let at = list.iter().position(|d| *d == self.devices[i]).map_or(0, |p| p + 1) % list.len();
        let next = list[at];
        if next != Device::NextPad {
            if let Some(j) = (0..self.players).find(|&j| j != i && self.devices[j] == next) {
                self.devices[j] = self.devices[i];
            }
        }
        self.devices[i] = next;
    }

    fn add_bot(&mut self, s: usize) {
        if self.count(s) >= TEAM_SIZE {
            return;
        }
        let taken: Vec<&String> = self.bots.iter().flatten().collect();
        let name = (0..).map(bot_name).find(|n| !taken.contains(&n)).unwrap_or_else(|| "Bot".into());
        self.bots[s].push(name);
    }

    /// Everyone to the other side from Player 1 (just you, without
    /// splitscreen), if there's room.
    fn switch_team(&mut self) {
        let other = 1 - side(self.teams[0]);
        if self.bots[other].len() + self.players <= TEAM_SIZE {
            self.teams = [SIDES[other]; MAX_PLAYERS];
        }
    }

    /// One local player to the other side, if there's room.
    fn switch_player(&mut self, i: usize) {
        if i >= self.players {
            return;
        }
        let other = 1 - side(self.teams[i]);
        if self.count(other) < TEAM_SIZE {
            self.teams[i] = SIDES[other];
        }
    }
}

// ---------------------------------------------------------------- menus

fn str_exp(s: &str) -> Statement {
    vec![Token::Op(op::LEFTPAREN), Token::Str(s.into())]
}

fn dvar_exp(name: &str) -> Statement {
    vec![Token::Op(op::LEFTPAREN), Token::Op(op::DVARSTRING), Token::Str(name.into()), Token::Op(op::RIGHTPAREN)]
}

fn script(parts: &[&str]) -> String {
    let mut s = String::from("\"play\" \"mouse_click\" ; ");
    for p in parts {
        s += p;
        s += " ; ";
    }
    s
}

fn ui_script(args: &str) -> String {
    let quoted: Vec<String> = args.split(' ').map(|a| format!("\"{a}\"")).collect();
    format!("\"uiScript\" {}", quoted.join(" "))
}

/// Left-column items of Create a Class (its rows).
fn left_column(it: &Item) -> bool {
    it.window.rect.horz_align == 1 && it.window.rect.x < 230.0
}

/// Copies of a row moved to `y` as highlight row `row`, its button showing
/// `label` and doing `action` (like `supply::place_row`).
fn place_row(template: &[Item], y: f32, row: i32, label: Statement, action: &str) -> Vec<Item> {
    template
        .iter()
        .cloned()
        .map(|mut it| {
            it.window.rect.y = y;
            it.window.name.clear();
            retarget_highlight(&mut it.visible_exp, row);
            if it.ty == item_type::BUTTON {
                it.text_exp = label.clone();
                it.action = action.into();
                let key = "\"ui_highlight\" ";
                if let Some(at) = it.on_focus.find(key) {
                    let start = at + key.len();
                    let end = it.on_focus[start..].find(|c: char| !c.is_ascii_digit()).map_or(it.on_focus.len(), |e| start + e);
                    it.on_focus = format!("{}{row}{}", &it.on_focus[..start], &it.on_focus[end..]);
                }
                it.dvar_flags = 0;
            }
            it
        })
        .collect()
}

/// A text button on the right, left-aligned text.
fn side_button(x: f32, y: f32, w: f32, label: &str, action: &str, color: [f32; 4]) -> Item {
    let mut it = Item::default();
    it.ty = item_type::BUTTON;
    it.window.rect = VRect { x, y, w, h: ROW_H - 2.0, horz_align: 3, vert_align: 1 };
    it.window.fore_color = color;
    it.window.dynamic_flags = flags::VISIBLE;
    it.font_enum = 1;
    it.text_scale = 0.3;
    it.text_align_mode = 8;
    it.text_align_x = 8.0;
    it.text_style = 6;
    it.text_exp = str_exp(label);
    it.action = action.into();
    it.on_focus = "\"play\" \"mouse_over\" ; ".into();
    it
}

/// "Start New Server" on the main menu becomes "Private Match".
fn with_private_match(menu: &Menu) -> Option<Menu> {
    let i = menu.items.iter().position(|it| it.ty == item_type::BUTTON && it.action.contains("\"createserver\""))?;
    let mut out = menu.clone();
    let it = &mut out.items[i];
    it.text_exp = str_exp("Private Match");
    it.action = format!("\"play\" \"mouse_click\" ; \"open\" \"{LOBBY_MENU}\" ; ");
    // Headquarters ([`crate::hq`]) above Join Game, a copy of this row.
    let y = it.window.rect.y;
    let template: Vec<Item> = out.items.iter().filter(|it| left_column(it) && (it.window.rect.y - y).abs() < 0.5).cloned().collect();
    let row = out.items.iter().flat_map(|it| highlight_rows(&it.visible_exp)).max().unwrap_or(0) + 1;
    out.items.extend(place_row(&template, y - 2.0 * HQ_PITCH, row, str_exp("Headquarters"), &script(&[&ui_script("startHeadquarters")])));
    Some(out)
}

/// The main menu's rows' spacing.
const HQ_PITCH: f32 = 24.0;

impl Frontend {
    /// The lobby's menus, and the main menu pointing at it.
    pub(super) fn lobby_menu(&mut self, key: &str, menu: Option<Arc<Menu>>) -> Option<Arc<Menu>> {
        match key {
            LOBBY_MENU => {
                self.sync_lobby();
                self.lobby_screen().map(Arc::new)
            }
            MAPS_MENU => {
                self.set_dvar(HOVER_DVAR, &self.lobby.map.to_string());
                self.maps_screen().map(Arc::new)
            }
            "main_text" => {
                let menu = menu?;
                Some(with_private_match(&menu).map_or(menu, Arc::new))
            }
            _ => menu,
        }
    }

    /// The row labels (dvars) and `ui_mapname` from the lobby's settings.
    fn sync_lobby(&mut self) {
        let l = &self.lobby;
        let labels = [
            ("ui_pm_map", format!("Map: {}", MAPS[l.map].1)),
            ("ui_pm_mode", format!("Game Type: {}", l.mode().name())),
            ("ui_pm_time", format!("Time Limit: {} Minutes", TIMES[l.time])),
            ("ui_pm_score", format!("Score Limit: {}", l.score_limit())),
            ("ui_pm_difficulty", format!("Bot Difficulty: {}", DIFFICULTY[l.difficulty].0)),
            ("ui_pm_team", format!("{}Join {}", if l.players > 1 { "All " } else { "" }, if l.teams[0] == Team::Allies { "OpFor" } else { "Marines" })),
            ("ui_pm_hardcore", format!("Hardcore: {}", if l.hardcore { "On" } else { "Off" })),
            ("ui_pm_split", if l.players > 1 { format!("Splitscreen: {} Players", l.players) } else { "Splitscreen: Off".into() }),
            ("ui_mapname", MAPS[l.map].0.to_string()),
        ];
        for (k, v) in labels {
            self.set_dvar(k, &v);
        }
    }

    /// `uiScript lobby...`: `lobbyMap <i>`, `lobbyNext <mode|time|score|difficulty|hardcore|splitscreen>`,
    /// `lobbyTeam`, `lobbyAdd <side>`, `lobbyRemove <side> <i>`, `lobbyDevice <player>`, `lobbySwitch <player>`,
    /// `lobbyStart`.
    pub(super) fn lobby_script(&mut self, args: &[String]) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        let n = |i: usize| a(i).parse::<usize>().unwrap_or(0);
        let mut roster_changed = false;
        let cmd = a(0).to_ascii_lowercase();
        if matches!(cmd.as_str(), "lobbyinvite" | "lobbyjoincode" | "lobbyleave") {
            self.online_script(&cmd);
            self.rebuild_lobby();
            return;
        }
        // In a friend's lobby, the host sets everything; you can only ask
        // to switch teams.
        if self.lobby.online.guest() {
            if cmd == "lobbyteam" {
                self.ask_switch();
            }
            return;
        }
        let online = self.lobby.online.active();
        let l = &mut self.lobby;
        match cmd.as_str() {
            "lobbymap" => l.map = n(1).min(MAPS.len() - 1),
            "lobbynext" => match a(1) {
                "mode" => {
                    // Each game type starts on its own default limit. Online,
                    // only the modes friends can play yet (no objectives).
                    l.mode = (l.mode + 1) % GameMode::ALL.len();
                    while online && !matches!(l.mode(), GameMode::Tdm | GameMode::Ffa) {
                        l.mode = (l.mode + 1) % GameMode::ALL.len();
                    }
                    l.score = l.mode().score_limits().1;
                    let minutes = l.mode().default_time_limit();
                    l.time = TIMES.iter().position(|&t| t == minutes).unwrap_or(l.time);
                }
                "time" => l.time = (l.time + 1) % TIMES.len(),
                "score" => l.score = (l.score + 1) % l.mode().score_limits().0.len(),
                "difficulty" => l.difficulty = (l.difficulty + 1) % DIFFICULTY.len(),
                "hardcore" => l.hardcore = !l.hardcore,
                "splitscreen" if !online => {
                    l.next_players(&self.devices);
                    roster_changed = true;
                }
                _ => {}
            },
            "lobbydevice" => {
                l.next_device(n(1), &self.devices);
                roster_changed = true;
            }
            "lobbyteam" => {
                l.switch_team();
                roster_changed = true;
            }
            "lobbyswitch" => {
                l.switch_player(n(1));
                roster_changed = true;
            }
            "lobbyadd" => {
                l.add_bot(n(1).min(1));
                roster_changed = true;
            }
            "lobbyremove" => {
                let s = n(1).min(1);
                if n(2) < l.bots[s].len() {
                    l.bots[s].remove(n(2));
                }
                roster_changed = true;
            }
            "lobbystart" if !l.online.connecting => {
                l.starting = true;
                l.online.start = l.online.host();
                self.start = Some(l.map_id().to_string());
            }
            _ => {}
        }
        self.sync_lobby();
        // The team lists are rows of their own: rebuild the screen.
        if roster_changed {
            self.rebuild_lobby();
        }
    }

    /// In a friend's online lobby.
    pub(super) fn lobby_guest(&self) -> bool {
        self.lobby.online.guest()
    }

    /// The lobby screen again, if it's up, for a changed roster.
    pub(super) fn rebuild_lobby(&mut self) {
        self.sync_lobby();
        if self.stack.iter().any(|m| m.name == LOBBY_MENU) {
            let top = self.stack.last().is_some_and(|m| m.name == LOBBY_MENU);
            self.close(LOBBY_MENU);
            self.open(LOBBY_MENU);
            // Kept under whatever was open over it (the map list).
            if !top && let Some(i) = self.stack.iter().position(|m| m.name == LOBBY_MENU) {
                let lobby = self.stack.remove(i);
                let at = self.stack.len().saturating_sub(1);
                self.stack.insert(at, lobby);
            }
        }
    }

    fn lobby_screen(&self) -> Option<Menu> {
        let (mut m, template) = self.frame(LOBBY_MENU, "PRIVATE MATCH")?;
        let rows: [(f32, &str, String); 9] = [
            (34.0, "ui_pm_map", script(&[&format!("\"open\" \"{MAPS_MENU}\"")])),
            (58.0, "ui_pm_mode", script(&[&ui_script("lobbyNext mode")])),
            (82.0, "ui_pm_time", script(&[&ui_script("lobbyNext time")])),
            (106.0, "ui_pm_score", script(&[&ui_script("lobbyNext score")])),
            (130.0, "ui_pm_difficulty", script(&[&ui_script("lobbyNext difficulty")])),
            (154.0, "ui_pm_team", script(&[&ui_script("lobbyTeam")])),
            (178.0, "ui_pm_hardcore", script(&[&ui_script("lobbyNext hardcore")])),
            (202.0, "ui_pm_split", script(&[&ui_script("lobbyNext splitscreen")])),
            (236.0, "", script(&[&ui_script("lobbyStart")])),
        ];
        let guest = self.lobby.online.guest();
        for (i, (y, dvar, action)) in rows.iter().enumerate() {
            let label = match (dvar.is_empty(), guest) {
                (true, false) => str_exp("Start Match"),
                (true, true) => str_exp("Waiting for the host"),
                _ => dvar_exp(dvar),
            };
            // The map list is the host's alone.
            let action = if guest && i == 0 { String::new() } else { action.clone() };
            m.items.extend(place_row(&template, *y, i as i32 + 1, label, &action));
        }
        // The teams: splitscreen players are buttons (click to change their
        // device) with an arrow to the other team, bots are buttons too
        // (click to take one out), then "+ Add Bot".
        let split = self.lobby.players > 1;
        for s in 0..2 {
            let x = PANEL_X[s];
            let mut locals = 0;
            for i in self.lobby.locals_on(s) {
                if split {
                    let y = PANEL_TOP + 26.0 + ROW_H * locals as f32;
                    let label = format!("Player {} - {}", i + 1, self.device_name(self.lobby.devices[i]));
                    let action = script(&[&ui_script(&format!("lobbyDevice {i}"))]);
                    m.items.push(side_button(x + 6.0, y, PANEL_W - 6.0 - SWITCH_W, &label, &action, GOLD));
                    let action = script(&[&ui_script(&format!("lobbySwitch {i}"))]);
                    let mut switch = side_button(x + PANEL_W - SWITCH_W, y, SWITCH_W - 2.0, SWITCH_ARROW[s], &action, GOLD);
                    switch.text_align_mode = 9;
                    switch.text_align_x = 0.0;
                    m.items.push(switch);
                }
                locals += 1;
            }
            let others = self.lobby.online.others_on(s).count();
            let mut y = PANEL_TOP + 26.0 + ROW_H * (locals + others) as f32;
            for (i, name) in self.lobby.bots[s].iter().enumerate() {
                let action = if guest { String::new() } else { script(&[&ui_script(&format!("lobbyRemove {s} {i}"))]) };
                m.items.push(side_button(x, y, PANEL_W, name, &action, GREY));
                y += ROW_H;
            }
            if !guest && self.lobby.count(s) < TEAM_SIZE {
                let action = script(&[&ui_script(&format!("lobbyAdd {s}"))]);
                m.items.push(side_button(x, y + 4.0, PANEL_W, "+ Add Bot", &action, GOLD));
            }
        }
        // Online: Invite Friends, or a code to join a friend's lobby; once
        // in one, leaving it.
        let o = &self.lobby.online;
        let x = PANEL_X[0];
        if o.active() {
            let label = if o.host() { "Close Lobby" } else { "Leave Lobby" };
            let action = script(&[&ui_script("lobbyLeave")]);
            m.items.push(side_button(PANEL_X[1] + PANEL_W - 120.0, ONLINE_TOP + 30.0, 120.0, label, &action, GOLD));
        } else {
            let action = script(&[&ui_script("lobbyInvite")]);
            m.items.push(side_button(x, ONLINE_TOP, PANEL_W, "Invite Friends", &action, GOLD));
            let mut field = side_button(PANEL_X[1], ONLINE_TOP, PANEL_W - 64.0, "", "", WHITE);
            field.ty = item_type::EDITFIELD;
            field.window.name = CODE_DVAR.into();
            field.dvar = CODE_DVAR.into();
            field.text_exp.clear();
            field.data = ItemData::EditField(EditField { max_chars: 9, max_paint_chars: 9, ..EditField::default() });
            field.on_accept = script(&[&ui_script("lobbyJoinCode")]);
            field.window.style = 1;
            field.window.back_color = [0.1, 0.1, 0.1, 0.45];
            field.window.border = 1;
            field.window.border_size = 0.5;
            field.window.border_color = [0.9, 0.9, 0.95, 0.3];
            m.items.push(field);
            let action = script(&[&ui_script("lobbyJoinCode")]);
            let mut join = side_button(PANEL_X[1] + PANEL_W - 60.0, ONLINE_TOP, 60.0, "Join", &action, GOLD);
            join.text_align_mode = 9;
            join.text_align_x = 0.0;
            m.items.push(join);
        }
        Some(m)
    }

    fn device_name(&self, d: Device) -> String {
        match d {
            Device::NextPad => "Any Controller".into(),
            d => self.devices.iter().find(|(x, _)| *x == d).map_or_else(|| "Disconnected".into(), |(_, n)| n.clone()),
        }
    }

    /// The devices changed (one connected or went): the lobby's names.
    pub(super) fn devices_changed(&mut self, devices: Vec<(Device, String)>) {
        self.devices = devices;
        if self.lobby.players > 1 && self.stack.iter().any(|m| m.name == LOBBY_MENU) {
            self.close(LOBBY_MENU);
            self.open(LOBBY_MENU);
        }
    }

    fn maps_screen(&self) -> Option<Menu> {
        let (mut m, template) = self.frame(MAPS_MENU, "CHOOSE MAP")?;
        for (i, (_, name)) in MAPS.iter().enumerate() {
            let action = script(&[&ui_script(&format!("lobbyMap {i}")), "\"close\" \"self\""]);
            for mut it in place_row(&template, 34.0 + i as f32 * 19.0, i as i32 + 1, str_exp(name), &action) {
                if it.ty == item_type::BUTTON {
                    it.text_scale = 0.3;
                    it.on_focus += &format!(" ; \"setdvar\" \"{HOVER_DVAR}\" \"{i}\" ; ");
                }
                it.window.rect.h = it.window.rect.h.min(19.0);
                m.items.push(it);
            }
        }
        Some(m)
    }

    /// Create a Class's frame with its two columns emptied, titled `title`,
    /// and its row for templates.
    fn frame(&self, name: &str, title: &str) -> Option<(Menu, Vec<Item>)> {
        let cac = self.assets.menu("menu_cac_assault")?;
        let mut m = Menu {
            window: cac.window.clone(),
            font: cac.font.clone(),
            full_screen: true,
            focus_color: cac.focus_color,
            disable_color: [0.5, 0.5, 0.5, 0.6],
            on_esc: "\"play\" \"mouse_click\" ; \"close\" \"self\" ; ".into(),
            ..Menu::default()
        };
        m.window.name = name.into();
        for it in cac.items.iter().filter(|it| !matches!(it.window.rect.horz_align, 1 | 3)) {
            let mut it = it.clone();
            if it.text_exp.iter().any(|t| matches!(t, Token::Str(s) if s.eq_ignore_ascii_case("@MPUI_CREATE_A_CLASS_CAP"))) {
                it.text_exp = str_exp(title);
            }
            m.items.push(it);
        }
        let template: Vec<Item> =
            cac.items.iter().filter(|it| left_column(it) && (it.window.rect.y - 34.0).abs() < 0.5).cloned().collect();
        let _ = highlight_rows;
        Some((m, template))
    }

    /// Drawing for the lobby: the map's picture and name, and the team
    /// panels; on the map list, the picture of the map under the mouse.
    pub(super) fn paint_lobby(&self, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
        let mut p = Paint { fe: self, pl, ops, horz_align: 1 };
        if om.name == MAPS_MENU {
            let i = self.dvar(HOVER_DVAR).parse::<usize>().unwrap_or(self.lobby.map).min(MAPS.len() - 1);
            p.horz_align = 3;
            p.map_card(-420.0, 34.0, 404.0, MAPS[i]);
            return;
        }
        if om.name != LOBBY_MENU {
            return;
        }
        let l = &self.lobby;
        // The map, under the settings.
        p.map_card(6.0, 264.0, 212.0, MAPS[l.map]);

        // The right side's focused button (a pad's, or the mouse's): a bar
        // like the left column's highlight, not just its text turning white.
        if let Some((_, i)) = self.focus.as_ref().filter(|(m, _)| m == LOBBY_MENU)
            && let Some(it) = om.menu.items.get(*i).filter(|it| it.window.rect.horz_align == 3)
        {
            let r = &it.window.rect;
            p.horz_align = 3;
            p.fill(r.x, r.y, r.w, r.h, [1.0, 0.85, 0.45, 0.16]);
            p.fill(r.x, r.y, 3.0, r.h, [0.85, 0.82, 0.45, 0.95]);
            p.horz_align = 1;
        }

        // The teams.
        p.horz_align = 3;
        for s in 0..2 {
            let x = PANEL_X[s];
            let rows = TEAM_SIZE as f32 * ROW_H + 34.0;
            p.fill(x, PANEL_TOP, PANEL_W, rows, [0.0, 0.0, 0.0, 0.45]);
            p.fill(x, PANEL_TOP, PANEL_W, 22.0, [1.0, 1.0, 1.0, 0.08]);
            p.pic(x + 4.0, PANEL_TOP + 3.0, 16.0, 16.0, TEAM_ICONS[s], WHITE);
            p.text(x + 26.0, PANEL_TOP + 16.0, 0, 0.3, TEAM_NAMES[s], WHITE);
            p.text(x + PANEL_W - 8.0, PANEL_TOP + 16.0, 2, 0.27, &format!("{}/{TEAM_SIZE}", l.count(s)), GREY);
            if l.players > 1 {
                // The splitscreen players' rows (their buttons draw the names).
                for row in 0..l.locals_on(s).count() {
                    p.fill(x + 2.0, PANEL_TOP + 26.0 + ROW_H * row as f32, PANEL_W - 4.0, ROW_H - 2.0, [1.0, 0.85, 0.45, 0.12]);
                }
            } else if side(l.teams[0]) == s {
                let y = PANEL_TOP + 26.0;
                p.fill(x + 2.0, y, PANEL_W - 4.0, ROW_H - 2.0, [1.0, 0.85, 0.45, 0.12]);
                let (rank, prestige) = self.my_rank();
                let w = p.member(x, y, rank, prestige, &self.profile_name(), GOLD);
                p.text(x + 32.0 + w, y + 13.0, 0, 0.24, "(you)", GREY);
            }
            // Friends, after this game's players.
            let mut row = l.locals_on(s).count();
            for m in l.online.others_on(s) {
                let y = PANEL_TOP + 26.0 + ROW_H * row as f32;
                p.fill(x + 2.0, y, PANEL_W - 4.0, ROW_H - 2.0, [1.0, 1.0, 1.0, 0.06]);
                let w = p.member(x, y, m.profile.rank as i32, m.profile.prestige as i32, &m.profile.name, WHITE);
                if m.peer == crate::online::HOST {
                    p.text(x + 32.0 + w, y + 13.0, 0, 0.24, "(host)", GREY);
                }
                row += 1;
            }
        }
        let o = &l.online;
        let hint = if o.guest() {
            "The host picks the settings. Join Team asks to switch sides"
        } else if l.players > 1 {
            "Click a player to change their device, an arrow to switch their team"
        } else {
            "Click a bot to remove it"
        };
        p.text(-212.0 + PANEL_W, PANEL_TOP + TEAM_SIZE as f32 * ROW_H + 52.0, 2, 0.24, hint, GREY);

        // Online.
        let x = PANEL_X[0];
        let w = PANEL_X[1] + PANEL_W - x;
        p.fill(x, ONLINE_TOP - 26.0, w, 82.0, [0.0, 0.0, 0.0, 0.45]);
        p.text(x + 8.0, ONLINE_TOP - 9.0, 0, 0.27, "PLAY WITH FRIENDS", WHITE);
        if let Some(code) = &o.code {
            p.text(x + 8.0, ONLINE_TOP + 16.0, 0, 0.27, "Lobby code:", GREY);
            p.text(x + 100.0, ONLINE_TOP + 18.0, 0, 0.42, code, GOLD);
            p.text(x + 8.0, ONLINE_TOP + 44.0, 0, 0.22, "Give it to friends. It only works for this lobby.", GREY);
        } else if o.guest() {
            let host = o.members.iter().find(|m| m.peer == crate::online::HOST).map_or("the host", |m| m.profile.name.as_str());
            p.text(x + 8.0, ONLINE_TOP + 16.0, 0, 0.27, &format!("In {host}'s lobby"), WHITE);
        } else if !o.active() && self.dvar(CODE_DVAR).is_empty() && self.editing.is_none() {
            p.text(PANEL_X[1] + 8.0, ONLINE_TOP + 13.0, 0, 0.27, "Enter a code", GREY);
        }
        if !o.note.is_empty() {
            p.text(x + 8.0, ONLINE_TOP + 44.0 + 4.0 * o.code.is_some() as u8 as f32, 0, 0.22, &o.note, GOLD);
        }
    }

    /// This player's rank and prestige (as the HUD has them).
    fn my_rank(&self) -> (i32, i32) {
        let stat = |name: &str| self.table_lookup("mp/playerStatsTable.csv", 1, name, 0).parse::<i32>().ok().map_or(0, |i| self.stat(i));
        (stat("rank").max(0), stat("plevel").max(0))
    }
}

/// Drawing helpers in menu units.
struct Paint<'a> {
    fe: &'a Frontend,
    pl: &'a Placement,
    ops: &'a mut Vec<Op>,
    horz_align: u8,
}

impl Paint<'_> {
    fn r(&self, x: f32, y: f32, w: f32, h: f32) -> (bevy::math::Vec2, bevy::math::Vec2) {
        self.pl.rect(&VRect { x, y, w, h, horz_align: self.horz_align, vert_align: 1 })
    }

    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let (pos, size) = self.r(x, y, w, h);
        self.ops.push(Op::Fill { pos, size, color });
    }

    fn pic(&mut self, x: f32, y: f32, w: f32, h: f32, material: &str, color: [f32; 4]) {
        let (pos, size) = self.r(x, y, w, h);
        self.ops.push(Op::Pic { pos, size, material: material.into(), color });
    }

    /// One line with its baseline at `y`, `align` 0 left, 1 centred, 2
    /// right of `x`. Returns its width.
    fn text(&mut self, x: f32, y: f32, align: u8, scale: f32, text: &str, color: [f32; 4]) -> f32 {
        let height = scale * 48.0 * self.pl.scale;
        let font = self.fe.font_for(1, height);
        let f = &self.fe.assets.fonts[font];
        let k = height / f.pixel_height as f32;
        let w = draw::text_width(f, text, k);
        let (pos, _) = self.r(x, y, 0.0, 0.0);
        let x = pos.x - [0.0, w * 0.5, w][align.min(2) as usize];
        self.ops.push(Op::Text { text: text.into(), x, y: pos.y, font, k, color, shadow: self.pl.scale });
        w / self.pl.scale
    }

    /// A lobby row's player: rank icon, level and name. Returns the name's
    /// width.
    fn member(&mut self, x: f32, y: f32, rank: i32, prestige: i32, name: &str, color: [f32; 4]) -> f32 {
        let icon = super::hud::rank_icon(self.fe, rank, prestige);
        self.pic(x + 6.0, y + 1.0, 16.0, 16.0, &icon, WHITE);
        self.text(x + PANEL_W - 8.0, y + 13.0, 2, 0.24, &(rank + 1).to_string(), GREY);
        self.text(x + 26.0, y + 13.0, 0, 0.3, name, color)
    }

    /// A map's loading screen picture (16:9) with its name under it.
    fn map_card(&mut self, x: f32, y: f32, w: f32, (id, name): (&str, &str)) {
        let h = w * 9.0 / 16.0;
        self.fill(x - 2.0, y - 2.0, w + 4.0, h + 26.0, [0.0, 0.0, 0.0, 0.5]);
        self.pic(x, y, w, h, &format!("loadscreen_{id}"), WHITE);
        self.text(x + 4.0, y + h + 18.0, 0, 0.36, name, WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pads(n: usize) -> Vec<(Device, String)> {
        (0..n).map(|i| (Device::Pad(bevy::prelude::Entity::from_raw_u32(i as u32 + 1).unwrap()), format!("Pad {i}"))).collect()
    }

    #[test]
    fn splitscreen_players_join_player_ones_side_and_switch_alone() {
        let mut l = Lobby::default();
        let devices = pads(3);
        for _ in 0..3 {
            l.next_players(&devices);
        }
        assert_eq!(l.players, 4);
        assert_eq!(l.count(0), 4);
        // Player 3 goes over; the others stay.
        l.switch_player(2);
        assert_eq!(&l.teams[..4], &[Team::Allies, Team::Allies, Team::Axis, Team::Allies]);
        assert_eq!((l.count(0), l.count(1)), (3, 1));
        let locals = l.local_players();
        assert_eq!(locals.teams, vec![Team::Allies, Team::Allies, Team::Axis, Team::Allies]);
        // "All Join OpFor" takes everyone.
        l.switch_team();
        assert_eq!(l.count(1), 4);
        assert_eq!(l.config().player_team, Team::Axis);
    }

    #[test]
    fn a_full_side_takes_no_more() {
        let mut l = Lobby::default();
        for _ in 0..TEAM_SIZE {
            l.add_bot(1);
        }
        l.next_players(&pads(1));
        assert_eq!(l.players, 2);
        l.switch_player(1);
        assert_eq!(l.teams[1], Team::Allies, "OpFor is full");
        // With Marines full too, splitscreen goes back to one player.
        for _ in 0..TEAM_SIZE {
            l.add_bot(0);
        }
        l.next_players(&pads(1));
        assert_eq!(l.players, 1);
    }
}
