//! Private Match online: Invite Friends and Join with Code.
//!
//! The host's lobby is the one that counts: its settings, its bots and who's
//! on which team. Whenever any of it changes, the host sends the whole
//! lobby to everyone ([`LobbyMsg::Lobby`]); a friend's lobby screen shows
//! that, with the settings locked. Friends tell the host who they are when
//! they join (their combat record name and rank) and may ask to switch
//! teams. Start Match is the host's: everyone's game loads the match with
//! the host's settings.

use super::*;
use crate::online::lobby::{LobbyMsg, MAX_RANK, MAX_PRESTIGE, Member, Profile, Refusal, Settings, clean_name};
use crate::online::{EVERYONE, Event, HOST, Online, Status};
use crate::state::GameState;
use bevy::prelude::{Commands, Res, ResMut, State};
use cod4rw_multiplayer::protocol::{LobbyCode, MAX_PLAYERS as MAX_ONLINE, PeerId};

/// The Join with Code text field's dvar.
pub(super) const CODE_DVAR: &str = "ui_pm_code";
/// This build: friends must be on the same one.
const BUILD: &str = env!("CARGO_PKG_VERSION");

pub(super) enum Request {
    Host,
    Join(LobbyCode),
    Leave,
}

/// The lobby's online side.
#[derive(Default)]
pub(super) struct OnlineLobby {
    /// For [`Online`], from the menu's buttons.
    pub(super) request: Option<Request>,
    /// Everyone in the online lobby, this game included.
    pub(super) members: Vec<Member>,
    /// This game's ID in the lobby, once in one.
    pub(super) me: Option<PeerId>,
    pub(super) connecting: bool,
    /// The host's code, to show.
    pub(super) code: Option<String>,
    /// For the player: why it ended, or what's happening.
    pub(super) note: String,
    /// A friend asked to switch teams.
    switch: bool,
    /// The host pressed Start Match: tell everyone.
    pub(super) start: bool,
    /// The host's last lobby sent, to send again only when it changes.
    sent: Vec<u8>,
    /// The match is starting online: for [`crate::netplay`].
    begin: bool,
}

impl OnlineLobby {
    pub(super) fn active(&self) -> bool {
        self.me.is_some() || self.connecting
    }
    pub(super) fn host(&self) -> bool {
        self.me == Some(HOST)
    }
    /// In someone else's lobby: its settings aren't ours to change.
    pub(super) fn guest(&self) -> bool {
        self.me.is_some_and(|me| me != HOST)
    }
    /// The other players on side `s`.
    pub(super) fn others_on(&self, s: usize) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(move |m| m.team as usize == s && Some(m.peer) != self.me)
    }
    fn reset(&mut self, note: impl Into<String>) {
        *self = OnlineLobby { note: note.into(), ..OnlineLobby::default() };
    }
}

impl Lobby {
    fn settings(&self) -> Settings {
        Settings {
            map: self.map_id().into(),
            mode: self.mode as u8,
            time: self.time as u8,
            score: self.score as u8,
            difficulty: self.difficulty as u8,
            hardcore: self.hardcore,
        }
    }

    /// The host's settings, where this build has them.
    fn apply(&mut self, s: &Settings) {
        if let Some(map) = maps().iter().position(|m| m.0 == s.map) {
            self.map = map;
        }
        if (s.mode as usize) < GameMode::ALL.len() {
            self.mode = s.mode as usize;
        }
        if (s.time as usize) < TIMES.len() {
            self.time = s.time as usize;
        }
        if (s.score as usize) < self.mode().score_limits().0.len() {
            self.score = s.score as usize;
        }
        if (s.difficulty as usize) < DIFFICULTY.len() {
            self.difficulty = s.difficulty as usize;
        }
        self.hardcore = s.hardcore;
    }

    /// A side for someone joining: the one with fewer players, if either has room.
    fn open_side(&self) -> Option<u8> {
        let counts = [self.count(0), self.count(1)];
        let s = (counts[1] < counts[0]) as usize;
        (counts[s] < TEAM_SIZE).then_some(s as u8)
    }
}

impl Frontend {
    /// This player as friends see them: the combat record's name and rank.
    fn online_profile(&self) -> Profile {
        let stat = |name: &str| {
            self.table_lookup("mp/playerStatsTable.csv", 1, name, 0).parse::<i32>().ok().map_or(0, |i| self.stat(i))
        };
        Profile {
            name: clean_name(&self.profile_name()),
            rank: stat("rank").clamp(0, MAX_RANK as i32) as u8,
            prestige: stat("plevel").clamp(0, MAX_PRESTIGE as i32) as u8,
        }
    }

    /// `lobbyInvite`, `lobbyJoinCode`, `lobbyLeave`.
    pub(super) fn online_script(&mut self, cmd: &str) {
        let o = &mut self.lobby.online;
        match cmd {
            "lobbyinvite" if !o.active() => {
                o.request = Some(Request::Host);
                o.note = "Opening the lobby...".into();
            }
            "lobbyjoincode" if !o.active() => match LobbyCode::parse(&self.dvar(CODE_DVAR)) {
                Ok(code) => {
                    let o = &mut self.lobby.online;
                    o.request = Some(Request::Join(code));
                    o.note = "Joining...".into();
                }
                Err(_) => self.lobby.online.note = "A lobby code is 8 letters and numbers, like K7QM-2X9D.".into(),
            },
            "lobbyleave" => {
                o.request = Some(Request::Leave);
            }
            _ => {}
        }
    }

    /// A friend's Join Team: ask the host.
    pub(super) fn ask_switch(&mut self) {
        self.lobby.online.switch = true;
    }

    /// One thing from the network. Returns whether the lobby screen needs
    /// building again.
    fn online_event(&mut self, event: Event, online: &Online, in_frontend: bool) -> bool {
        match event {
            Event::Ready { peer, code } => {
                let profile = self.online_profile();
                let l = &mut self.lobby;
                l.online.connecting = false;
                l.online.me = Some(peer);
                l.online.note.clear();
                // Online lobbies are one player per game.
                l.players = 1;
                if peer == HOST {
                    // Objective modes aren't online yet.
                    if !matches!(l.mode(), GameMode::Tdm | GameMode::Ffa | GameMode::Tdm3) {
                        l.mode = 0;
                        l.score = l.mode().score_limits().1;
                    }
                    l.online.code = code.map(|c| c.to_string());
                    l.online.members = vec![Member { peer, profile, team: side(l.teams[0]) as u8 }];
                } else {
                    l.online.note = "Joined. Waiting for the host...".into();
                    online.send(HOST, &LobbyMsg::Join { build: BUILD.into(), profile });
                }
                true
            }
            Event::PeerJoined | Event::Inputs(..) | Event::Snapshot(_) => false,
            Event::PeerLeft(peer) => {
                let o = &mut self.lobby.online;
                let before = o.members.len();
                o.members.retain(|m| m.peer != peer);
                o.members.len() != before
            }
            Event::Message(from, msg) => self.online_message(from, msg, online, in_frontend),
            Event::Closed(reason) => {
                self.lobby.online.reset(reason);
                true
            }
        }
    }

    fn online_message(&mut self, from: PeerId, msg: LobbyMsg, online: &Online, in_frontend: bool) -> bool {
        let l = &mut self.lobby;
        if l.online.host() {
            match msg {
                LobbyMsg::Join { build, profile } => {
                    if l.online.members.iter().any(|m| m.peer == from) {
                        return false;
                    }
                    let refusal = if build != BUILD {
                        Some(Refusal::Version)
                    } else if !in_frontend {
                        Some(Refusal::Started)
                    } else if l.online.members.len() >= MAX_ONLINE {
                        Some(Refusal::Full)
                    } else {
                        None
                    };
                    match refusal.map(Err).unwrap_or_else(|| l.open_side().ok_or(Refusal::Full)) {
                        Ok(team) => {
                            l.online.members.push(Member { peer: from, profile, team });
                            true
                        }
                        Err(refusal) => {
                            online.send(from, &LobbyMsg::Refused(refusal));
                            false
                        }
                    }
                }
                LobbyMsg::SwitchTeam => {
                    let Some(i) = l.online.members.iter().position(|m| m.peer == from) else { return false };
                    let other = 1 - l.online.members[i].team as usize;
                    if l.count(other) >= TEAM_SIZE {
                        return false;
                    }
                    l.online.members[i].team = other as u8;
                    true
                }
                _ => false,
            }
        } else if from == HOST {
            match msg {
                LobbyMsg::Lobby { settings, members, bots } => {
                    l.apply(&settings);
                    l.bots = bots;
                    if let Some(me) = members.iter().find(|m| Some(m.peer) == l.online.me) {
                        l.teams[0] = if me.team == 0 { Team::Allies } else { Team::Axis };
                    }
                    l.online.members = members;
                    l.online.note.clear();
                    true
                }
                LobbyMsg::Start if in_frontend => {
                    l.starting = true;
                    l.online.begin = true;
                    self.start = Some(l.map_id().to_string());
                    false
                }
                LobbyMsg::Refused(r) => {
                    let why = match r {
                        Refusal::Full => "That lobby is full.",
                        Refusal::Version => "That lobby is on a different version of the game.",
                        Refusal::Started => "That lobby's match has already started.",
                    };
                    l.online.reset(why);
                    l.online.request = Some(Request::Leave);
                    true
                }
                _ => false,
            }
        } else {
            false
        }
    }
}

/// The menus' requests to [`Online`], what it reports back into the lobby,
/// and the host's lobby to everyone when it changes.
pub(in crate::ui) fn online_lobby(
    mut commands: Commands,
    mut fe: ResMut<Frontend>,
    mut online: ResMut<Online>,
    state: Res<State<GameState>>,
) {
    let fe = &mut *fe;
    match fe.lobby.online.request.take() {
        Some(Request::Host) => online.host(),
        Some(Request::Join(code)) => online.join(code),
        Some(Request::Leave) => {
            let note = std::mem::take(&mut fe.lobby.online.note);
            online.leave();
            fe.lobby.online.reset(note);
            fe.rebuild_lobby();
        }
        None => {}
    }
    let in_frontend = *state.get() == GameState::Frontend;
    let mut rebuild = false;
    for event in online.poll() {
        rebuild |= fe.online_event(event, &online, in_frontend);
    }
    let o = &mut fe.lobby.online;
    let connecting = online.status == Status::Connecting;
    if o.connecting != connecting {
        o.connecting = connecting;
        rebuild = true;
    }
    if o.guest() && std::mem::take(&mut o.switch) {
        online.send(HOST, &LobbyMsg::SwitchTeam);
    }
    if o.host() {
        let profile = fe.online_profile();
        let l = &mut fe.lobby;
        if let Some(me) = l.online.members.iter_mut().find(|m| m.peer == HOST) {
            me.profile = profile;
            me.team = side(l.teams[0]) as u8;
        }
        let msg = LobbyMsg::Lobby { settings: l.settings(), members: l.online.members.clone(), bots: l.bots.clone() };
        if let Ok(data) = msg.encode()
            && data != l.online.sent
        {
            online.send(EVERYONE, &msg);
            l.online.sent = data;
        }
        if std::mem::take(&mut l.online.start) {
            online.send(EVERYONE, &LobbyMsg::Start);
            l.online.begin = true;
        }
    }
    let o = &mut fe.lobby.online;
    if std::mem::take(&mut o.begin)
        && let Some(me) = o.me
    {
        commands.insert_resource(crate::netplay::NetStart { me, members: o.members.clone() });
    }
    if rebuild {
        fe.rebuild_lobby();
    }
}
