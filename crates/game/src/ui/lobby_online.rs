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
use crate::matchmaking::{AcceptsJoinInProgress, JoinInProgress, Matchmaker};
use crate::netplay::mixed;

/// Public matches (Find Match, the lobby browser).
#[path = "lobby_public.rs"]
mod public;
pub(super) use public::START_DVAR;
use public::PublicLobby;

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
    /// Start Match pressed with friends here: ask the relay for the best
    /// host first ([`Online::elect`]), and since when we've waited.
    elect: bool,
    pub(super) electing: Option<std::time::Instant>,
    /// The election moved the host role to or from this game.
    role_changed: bool,
    /// A public lobby (matchmaking's), not a private one.
    pub(super) public: Option<PublicLobby>,
    /// Netplay takes players in a match ([`AcceptsJoinInProgress`]).
    accepts_jip: bool,
    /// Players joining the match, for netplay.
    jip: Vec<JoinInProgress>,
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
        crate::netplay::layout_sync::clear();
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

    /// The host's settings, where this build has them. False if this game
    /// can't load the lobby's map (another game's, or a custom map it
    /// hasn't got): it can't play there.
    fn apply(&mut self, s: &Settings) -> bool {
        let Some(map) = maps().iter().position(|m| m.0 == s.map) else { return false };
        self.map = map;
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
        true
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
        // The games installed, and the ones the custom classes' guns are
        // from ([`crate::netplay::mixed`]: who can host, who sees what).
        let guns: Vec<String> = (1..=5).filter_map(|n| self.class_loadout(&format!("custom{n}"))).flat_map(|c| c.guns.into_iter().map(|g| g.spec)).collect();
        Profile {
            name: clean_name(&self.profile_name()),
            rank: stat("rank").clamp(0, MAX_RANK as i32) as u8,
            prestige: stat("plevel").clamp(0, MAX_PRESTIGE as i32) as u8,
            games: mixed::installed(),
            uses: mixed::guns_use(guns.iter().map(String::as_str)) & mixed::installed(),
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

    /// Start Match (or a public lobby's countdown ending). With friends
    /// here the relay first chooses the host: the one nearest it, whose
    /// game runs the match ([`Frontend::elected`] starts it).
    pub(super) fn online_start(&mut self) {
        let l = &mut self.lobby;
        if l.online.electing.is_some() || l.starting {
            return;
        }
        // Only among players who could all run every gun in play: else the
        // host stays (and a class it can't run takes CoD4 stand-ins).
        let used = l.online.members.iter().fold(0, |b, m| b | m.profile.uses);
        let all_could_host = l.online.members.iter().all(|m| mixed::can_host(m.profile.games, used));
        if l.online.host() && l.online.members.len() > 1 && !all_could_host {
            bevy::log::info!("online: not everyone has the games in play ({}); this game stays the host", mixed::tags(used).join(", "));
        }
        if l.online.host() && l.online.members.len() > 1 && all_could_host {
            l.online.elect = true;
            l.online.electing = Some(std::time::Instant::now());
            return;
        }
        l.starting = true;
        l.online.start = l.online.host();
        self.start = Some(l.map_id().to_string());
    }

    /// The relay chose the host: `swap`, the player who took the host's ID
    /// (the old host takes theirs). Whoever is the host now starts.
    fn elected(&mut self, swap: Option<PeerId>) -> bool {
        let l = &mut self.lobby;
        l.online.electing = None;
        if let Some(new) = swap {
            let flip = |p: PeerId| if p == HOST { new } else if p == new { HOST } else { p };
            for m in &mut l.online.members {
                m.peer = flip(m.peer);
            }
            l.online.me = l.online.me.map(flip);
            l.online.role_changed = true;
            l.online.sent.clear();
            let host = l.online.host();
            if let Some(p) = &mut l.online.public {
                p.playlist = host.then(|| p.playlist.unwrap_or(crate::matchmaking::Playlist::Tdm));
            }
            bevy::log::info!("online: {} the match's host (the nearest the relay)", if host { "this game is now" } else { "a friend is now" });
        }
        if l.online.host() && !l.starting {
            l.starting = true;
            l.online.start = true;
            self.start = Some(l.map_id().to_string());
        }
        true
    }

    /// A friend's Join Team: ask the host.
    pub(super) fn ask_switch(&mut self) {
        self.lobby.online.switch = true;
    }

    /// One thing from the network. Returns whether the lobby screen needs
    /// building again.
    fn online_event(&mut self, event: Event, online: &Online, in_frontend: bool) -> bool {
        match event {
            Event::Ready { peer, code, .. } => {
                let profile = self.online_profile();
                let l = &mut self.lobby;
                l.online.connecting = false;
                l.online.me = Some(peer);
                l.online.note.clear();
                // Online lobbies are one player per game.
                l.players = 1;
                if peer == HOST {
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
                let l = &mut self.lobby;
                let Some(at) = l.online.members.iter().position(|m| m.peer == peer) else { return false };
                let gone = l.online.members.remove(at);
                // Mid-match a bot takes their place (netplay spawns it).
                if !in_frontend && l.online.host() {
                    l.add_bot(gone.team as usize);
                }
                true
            }
            Event::Message(from, msg) => self.online_message(from, msg, online, in_frontend),
            Event::Elected(swap) => self.elected(swap),
            Event::Closed(reason) => {
                self.lobby.online.reset(reason);
                true
            }
        }
    }

    fn online_message(&mut self, from: PeerId, msg: LobbyMsg, online: &Online, in_frontend: bool) -> bool {
        // A player who can't load the lobby's map can't come in.
        if self.lobby.online.host()
            && let LobbyMsg::Join { profile, .. } = &msg
            && !self.lobby.online.members.iter().any(|m| m.peer == from)
            && let mixed::MapNeed::Game(g) = mixed::map_need(self.lobby.map_id())
            && profile.games & mixed::bit(g) == 0
        {
            bevy::log::info!("online: {} can't load {} (no {}): refused", profile.name, self.lobby.map_id(), g.title());
            online.send(from, &LobbyMsg::Refused(Refusal::Map));
            return false;
        }
        // The host seats joiners on the side short of humans (in a match,
        // in place of a bot).
        if self.lobby.online.host()
            && let LobbyMsg::Join { build, profile } = &msg
            && *build == BUILD
            && !self.lobby.online.members.iter().any(|m| m.peer == from)
        {
            if let Some(refusal) = self.public_join(from, profile.clone(), in_frontend, online) {
                online.send(from, &LobbyMsg::Refused(refusal));
                return false;
            }
            return true;
        }
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
                    let here = l.online.members[i].team as usize;
                    if !l.online_switch(here) {
                        return false;
                    }
                    l.online.members[i].team = 1 - here as u8;
                    true
                }
                _ => false,
            }
        } else if from == HOST {
            match msg {
                LobbyMsg::Lobby { settings, members, bots } => {
                    if !l.apply(&settings) {
                        let why = match mixed::map_need(&settings.map) {
                            mixed::MapNeed::Game(g) => format!("That lobby is on a map from {}, which isn't installed.", g.title()),
                            mixed::MapNeed::Custom => format!("That lobby is on a custom map you haven't got ({}).", settings.map),
                        };
                        bevy::log::info!("online: leaving: {why}");
                        l.online.reset(why);
                        l.online.request = Some(Request::Leave);
                        return true;
                    }
                    l.bots = bots;
                    if let Some(me) = members.iter().find(|m| Some(m.peer) == l.online.me) {
                        l.teams[0] = if me.team == 0 { Team::Allies } else { Team::Axis };
                    }
                    l.online.members = members;
                    l.online.note.clear();
                    true
                }
                // The host's map editor layout, before its Start.
                LobbyMsg::Layout { map, part, parts, data } => {
                    crate::netplay::layout_sync::receive(map, part, parts, data);
                    false
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
                        Refusal::Version => "The host is on a different version of the game. Update to play together.",
                        Refusal::Started => "That lobby's match has already started.",
                        Refusal::Map => "That lobby's map is from a game you haven't got installed.",
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
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn online_lobby(
    mut commands: Commands,
    mut fe: ResMut<Frontend>,
    mut online: ResMut<Online>,
    state: Res<State<GameState>>,
    mut mm: ResMut<Matchmaker>,
    accepts: Option<Res<AcceptsJoinInProgress>>,
    real: Res<bevy::time::Time<bevy::time::Real>>,
    time: Res<bevy::time::Time>,
    match_state: Option<Res<crate::tdm::MatchState>>,
    mut joining: bevy::prelude::MessageWriter<JoinInProgress>,
) {
    let fe = &mut *fe;
    fe.lobby.online.accepts_jip = accepts.is_some();
    match fe.lobby.online.request.take() {
        Some(Request::Host) => online.host(),
        Some(Request::Join(code)) => online.join(code),
        Some(Request::Leave) => {
            let note = std::mem::take(&mut fe.lobby.online.note);
            online.leave();
            // Our own map editor layouts again.
            crate::netplay::layout_sync::clear();
            mm.left();
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
    let clock = match_state.filter(|_| !in_frontend).map(|m| (m.time_left(time.elapsed_secs()), m.ended.is_some()));
    rebuild |= fe.public_lobby(&mut online, &mut mm, public::Now { real: real.elapsed_secs(), in_frontend, clock });
    for j in fe.lobby.online.jip.drain(..) {
        joining.write(j);
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
        // (After the lobby: the new host must have it before it's chosen.)
        if std::mem::take(&mut l.online.elect) {
            online.elect();
        }
        if std::mem::take(&mut l.online.start) {
            // Everyone plays this game's map editor layout: sent first.
            for part in crate::netplay::layout_sync::messages(l.map_id()) {
                online.send(EVERYONE, &part);
            }
            online.send(EVERYONE, &LobbyMsg::Start);
            l.online.begin = true;
        }
    }
    // No answer from the relay: start as we are.
    if fe.lobby.online.electing.is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(5)) {
        bevy::log::warn!("online: no host election from the relay; starting as host");
        rebuild |= fe.elected(None);
    }
    if std::mem::take(&mut fe.lobby.online.role_changed) {
        mm.role_changed(fe.lobby.online.host(), online.room);
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
