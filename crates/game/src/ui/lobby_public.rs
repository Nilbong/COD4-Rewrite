//! Public matches: the lobby Find Match lands in.
//!
//! It is the Private Match lobby ([`super`]) run by matchmaking
//! ([`crate::matchmaking`]) instead of by its host: the playlist's game
//! type on a map from the rotation, the settings locked. The host's lobby
//! counts down (`COD4RW_MM_COUNTDOWN`, 30 s) and, at the start, fills each
//! side's empty slots with bots. The host lists it on its relay, refreshing
//! the listing every few seconds: the map, players and bots, and the
//! countdown or the time left. A human who joins takes a slot a bot would
//! have had; in a match, a bot's place ([`JoinInProgress`]) once netplay
//! takes them. After a match the lobby counts down again on the next map.

use super::*;
use crate::matchmaking::{JoinInProgress, Matchmaker, Playlist, env_secs, mode_id};
use crate::online::lobby::{LobbyMsg, Member};
use crate::online::Online;
use bevy::log::info;
use cod4rw_multiplayer::matchmaking::{Listing, LobbyState};

/// The rotation: CoD4's maps (most of them, not the tiny ones).
const ROTATION: [&str; 18] = [
    "mp_convoy",
    "mp_backlot",
    "mp_bloc",
    "mp_bog",
    "mp_broadcast",
    "mp_carentan",
    "mp_countdown",
    "mp_crash",
    "mp_creek",
    "mp_crossfire",
    "mp_citystreets",
    "mp_farm",
    "mp_overgrown",
    "mp_pipeline",
    "mp_showdown",
    "mp_strike",
    "mp_vacant",
    "mp_cargoship",
];
/// The start button's label in a public lobby: the countdown.
pub(in crate::ui) const START_DVAR: &str = "ui_mm_start";
/// The listing is sent again this often (and whenever it changes).
const PUBLISH_EVERY: f32 = 3.0;

/// A public lobby's own state.
#[derive(Default)]
pub(in crate::ui) struct PublicLobby {
    /// Hosting: the playlist, and when the countdown ends.
    pub(in crate::ui) playlist: Option<Playlist>,
    starts_at: Option<f32>,
    next_publish: f32,
    sent: Option<Listing>,
    /// The lobby screen has been up (closing it then leaves the lobby).
    shown: bool,
    in_match: bool,
}

/// What the lobby needs to know this frame.
pub(in crate::ui) struct Now {
    pub(in crate::ui) real: f32,
    pub(in crate::ui) in_frontend: bool,
    /// In a match: seconds left, and whether it's over.
    pub(in crate::ui) clock: Option<(f32, bool)>,
}

fn countdown() -> f32 {
    env_secs("COD4RW_MM_COUNTDOWN", 30.0)
}

/// The next map: `COD4RW_MM_MAP`, or one from the rotation (not `last`).
fn next_map(last: Option<usize>) -> usize {
    let all = maps();
    if let Ok(m) = std::env::var("COD4RW_MM_MAP")
        && let Some(i) = all.iter().position(|(id, _)| *id == m.trim())
    {
        return i;
    }
    let choices: Vec<usize> = ROTATION.iter().filter_map(|id| all.iter().position(|(m, _)| m == id)).filter(|i| Some(*i) != last).collect();
    if choices.is_empty() {
        return last.unwrap_or(0);
    }
    choices[rand::random_range(0..choices.len())]
}

impl Lobby {
    /// Players a side in a public lobby.
    pub(in crate::ui) fn public_team_size(&self) -> Option<usize> {
        self.online.public.as_ref().map(|p| p.playlist.unwrap_or(Playlist::Tdm).team_size())
    }

    /// A new round of the host's public lobby: its game type, the next map,
    /// no bots yet, a fresh countdown.
    fn public_round(&mut self, playlist: Playlist, now: f32) {
        let mode = GameMode::ALL.iter().position(|m| *m == playlist.mode()).unwrap_or(0);
        self.mode = mode;
        self.score = self.mode().score_limits().1;
        let minutes = self.mode().default_time_limit();
        self.time = TIMES.iter().position(|&t| t == minutes).unwrap_or(self.time);
        self.difficulty = 1;
        self.hardcore = false;
        self.players = 1;
        self.bots = [Vec::new(), Vec::new()];
        let last = Some(self.map);
        self.map = next_map(last);
        if let Some(p) = &mut self.online.public {
            p.starts_at = Some(now + countdown());
            p.in_match = false;
        }
    }

    /// The bots that fill each side's empty slots at the start.
    fn fill_with_bots(&mut self, team_size: usize) {
        for s in 0..2 {
            while self.count(s) < team_size {
                let before = self.bots[s].len();
                self.add_bot(s);
                if self.bots[s].len() == before {
                    break;
                }
            }
        }
    }

    /// Humans on side `s`: this game's players and friends.
    pub(in crate::ui) fn humans_on(&self, s: usize) -> usize {
        self.locals_on(s).count() + self.online.others_on(s).count()
    }

    /// Players a side in this online lobby.
    pub(in crate::ui) fn online_team_size(&self) -> usize {
        self.public_team_size().unwrap_or(TEAM_SIZE)
    }

    /// May a human on side `from` move to the other side? A public lobby
    /// keeps the sides' humans within one of each other; a full side moves
    /// one of its bots over to make room. Moves the bot if so.
    pub(in crate::ui) fn online_switch(&mut self, from: usize) -> bool {
        let to = 1 - from;
        let (here, there) = (self.humans_on(from), self.humans_on(to));
        if self.online.public.is_some() && (there + 1).abs_diff(here.saturating_sub(1)) > 1 {
            return false;
        }
        if self.count(to) >= self.online_team_size() {
            let Some(bot) = self.bots[to].pop() else { return false };
            self.bots[from].push(bot);
        }
        true
    }

    /// A human joining the host's lobby, before or during its match: the
    /// side with fewer humans (1v1, 2v2, 3v2...) and, when that side is
    /// full, the bot whose place they take (gone from the lobby). `None`:
    /// no room.
    pub(in crate::ui) fn seat(&mut self, in_match: bool) -> Option<(u8, Option<String>)> {
        let size = self.online_team_size();
        if self.online.members.len() >= MAX_ONLINE {
            return None;
        }
        // The side with fewer humans (then fewer players).
        let s = (0..2).min_by_key(|&s| (self.humans_on(s), self.count(s)))?;
        if self.humans_on(s) >= size {
            return None;
        }
        if !in_match {
            return (self.count(s) < size || !self.bots[s].is_empty()).then(|| {
                // A bot the host added gives up its slot.
                if self.count(s) >= size {
                    self.bots[s].pop();
                }
                (s as u8, None)
            });
        }
        if self.count(s) < size {
            return Some((s as u8, None));
        }
        let bot = self.bots[s].pop()?;
        Some((s as u8, Some(bot)))
    }

    fn listing(&self, region: String, now: &Now) -> Option<Listing> {
        let p = self.online.public.as_ref()?;
        let playlist = p.playlist?;
        let size = playlist.team_size();
        let bots = (self.bots[0].len() + self.bots[1].len()) as u8;
        let (state, joinable) = match now.clock {
            Some((_, true)) => (LobbyState::Ending, false),
            Some((left, false)) => (LobbyState::InMatch { time_left: left.ceil() as u16 }, self.online.accepts_jip),
            None => {
                let countdown = p.starts_at.map_or(0.0, |t| (t - now.real).max(0.0));
                (LobbyState::Lobby { countdown: countdown.ceil() as u16 }, !self.starting)
            }
        };
        Some(Listing {
            mode: mode_id(self.mode()).into(),
            map: self.map_id().into(),
            bots,
            max: (2 * size).min(cod4rw_multiplayer::protocol::MAX_PLAYERS) as u8,
            state,
            joinable,
            region,
        })
    }
}

impl Frontend {
    /// The public lobby's frame: its setup when it opens, the countdown,
    /// the start, the listing, the guest's view of the countdown, and
    /// leaving when its screen is closed. Returns whether the lobby screen
    /// needs building again.
    pub(in crate::ui) fn public_lobby(&mut self, online: &mut Online, mm: &mut Matchmaker, now: Now) -> bool {
        let mut rebuild = false;
        let o = &self.lobby.online;
        // Into the lobby matchmaking found or opened.
        if o.public.is_none() && o.me.is_some() && mm.public() {
            let hosting = mm.hosting().map(|(p, _)| p).filter(|_| o.host());
            self.lobby.online.public = Some(PublicLobby { playlist: hosting, ..PublicLobby::default() });
            if let Some(playlist) = hosting {
                self.lobby.public_round(playlist, now.real);
                // The host's own row, on a side.
                self.lobby.teams = [SIDES[0]; MAX_PLAYERS];
            }
            self.sync_lobby();
            if !self.menu_open(LOBBY_MENU) {
                self.open(LOBBY_MENU);
            }
            rebuild = true;
        }
        if self.lobby.online.public.is_none() {
            return rebuild;
        }
        if !self.lobby.online.active() {
            self.lobby.online.public = None;
            mm.left();
            return true;
        }
        let in_match = !now.in_frontend;
        let open = self.menu_open(LOBBY_MENU);
        let l = &mut self.lobby;
        let p = l.online.public.as_mut().expect("checked");
        if in_match {
            p.in_match = true;
            p.shown = false;
        } else if std::mem::take(&mut p.in_match) {
            // Back from a match: the next round.
            if let Some(playlist) = p.playlist {
                l.public_round(playlist, now.real);
                l.starting = false;
            }
            self.sync_lobby();
            if !open {
                self.open(LOBBY_MENU);
            }
            return true;
        } else if open {
            p.shown = true;
        } else if p.shown && !l.starting {
            // The lobby's screen was closed: leave it.
            l.online.request = Some(super::Request::Leave);
            l.online.public = None;
            mm.left();
            return true;
        }
        let l = &mut self.lobby;
        let p = l.online.public.as_ref().expect("checked");
        if l.online.host() {
            let playlist = p.playlist.unwrap_or(Playlist::Tdm);
            // The countdown, then the start with bots in the empty slots.
            let left = p.starts_at.map(|t| t - now.real);
            if !in_match && !l.starting && l.online.electing.is_none() && left.is_some_and(|t| t <= 0.0) {
                l.fill_with_bots(playlist.team_size());
                info!("matchmaking: the countdown's over: starting {} with {} bots", l.map_id(), l.bots[0].len() + l.bots[1].len());
                self.online_start();
                rebuild = true;
            }
            let l = &mut self.lobby;
            l.online.note = match left {
                Some(t) if !in_match && !l.starting => format!("Public match. Starting in {}", t.max(0.0).ceil() as u32),
                _ => "Public match.".into(),
            };
            let label = match left {
                Some(t) if !l.starting => format!("Starting in {}", t.max(0.0).ceil() as u32),
                _ => "Starting".into(),
            };
            self.set_dvar(START_DVAR, &label);
            let l = &mut self.lobby;
            // The listing: on change, and every few seconds anyway.
            let region = mm.hosting().map_or_else(|| "local".into(), |(_, r)| r);
            let listing = l.listing(region, &now);
            let p = l.online.public.as_mut().expect("checked");
            // (The clock ticking alone waits for the next refresh.)
            let same_but_clock = match (&listing, &p.sent) {
                (Some(a), Some(b)) => Listing { state: b.state, ..a.clone() } == *b && std::mem::discriminant(&a.state) == std::mem::discriminant(&b.state),
                (None, None) => true,
                _ => false,
            };
            if now.real >= p.next_publish || !same_but_clock {
                p.next_publish = now.real + PUBLISH_EVERY;
                p.sent = listing.clone();
                online.publish(listing);
            }
        } else {
            // A guest: the host's countdown, as its listing has it.
            let (note, label) = match mm.joined.as_ref().map(|i| i.listing.state) {
                Some(LobbyState::Lobby { countdown }) if countdown > 0 => (format!("Public match. Starting in {countdown}"), format!("Starting in {countdown}")),
                Some(LobbyState::InMatch { .. }) => ("Public match. In progress.".into(), "In progress".into()),
                _ => ("Public match. Waiting for the host...".into(), "Waiting for the host".into()),
            };
            l.online.note = note;
            self.set_dvar(START_DVAR, &label);
        }
        rebuild
    }

    /// The host hears a player joining: a seat for them (maybe a bot's), or
    /// why not. Returns the refusal.
    pub(in crate::ui) fn public_join(&mut self, from: PeerId, profile: crate::online::lobby::Profile, in_frontend: bool, online: &Online) -> Option<Refusal> {
        let l = &mut self.lobby;
        if !in_frontend && !l.online.accepts_jip {
            return Some(Refusal::Started);
        }
        let Some((team, bot)) = l.seat(!in_frontend) else { return Some(Refusal::Full) };
        let member = Member { peer: from, profile, team };
        l.online.members.push(member.clone());
        if !in_frontend {
            info!("matchmaking: {} joins the match in place of {:?}", member.profile.name, bot);
            l.online.jip.push(JoinInProgress { member, replaces_bot: bot });
            // Their game loads the match as it is now.
            online.send(from, &LobbyMsg::Lobby { settings: l.settings(), members: l.online.members.clone(), bots: l.bots.clone() });
            online.send(from, &LobbyMsg::Start);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::online::lobby::Profile;

    fn friend(peer: PeerId, team: u8) -> Member {
        Member { peer, profile: Profile { name: format!("f{peer}"), rank: 0, prestige: 0 }, team }
    }

    fn public_lobby() -> Lobby {
        let mut l = Lobby::default();
        l.online.me = Some(crate::online::HOST);
        l.online.public = Some(PublicLobby { playlist: Some(Playlist::Tdm), ..PublicLobby::default() });
        l.online.members.push(friend(crate::online::HOST, 0));
        l
    }

    #[test]
    fn bots_fill_the_empty_slots_and_humans_take_them() {
        let mut l = public_lobby();
        l.fill_with_bots(6);
        assert_eq!((l.bots[0].len(), l.bots[1].len()), (5, 6));
        // In a match, a joiner takes a bot's place on the side short of humans.
        let (team, bot) = l.seat(true).unwrap();
        assert_eq!(team, 1);
        assert!(bot.is_some());
        assert_eq!(l.bots[1].len(), 5);
        l.online.members.push(friend(1, team));
        let (team, _) = l.seat(true).unwrap();
        assert_eq!(team, 0, "then the other side");
    }

    #[test]
    fn joiners_balance_the_humans_and_bots_make_way() {
        // A private lobby whose host filled both sides with bots.
        let mut l = Lobby::default();
        l.online.me = Some(crate::online::HOST);
        l.online.members.push(friend(crate::online::HOST, 0));
        for s in 0..2 {
            while l.count(s) < TEAM_SIZE {
                l.add_bot(s);
            }
        }
        let (team, bot) = l.seat(false).unwrap();
        assert_eq!((team, bot), (1, None), "1v1: the other side, a bot gave up its slot");
        assert_eq!(l.bots[1].len(), TEAM_SIZE - 1);
        l.online.members.push(friend(1, team));
        let (team, _) = l.seat(false).unwrap();
        assert_eq!(team, 0, "then 2v1");
        l.online.members.push(friend(2, team));
        assert_eq!((l.humans_on(0), l.humans_on(1)), (2, 1));
        assert_eq!(l.count(0) + l.count(1), 2 * TEAM_SIZE);
    }

    #[test]
    fn join_team_moves_a_bot_and_keeps_public_sides_even() {
        // Private: the host can join a side full of bots; a bot swaps over.
        let mut l = Lobby::default();
        l.online.me = Some(crate::online::HOST);
        l.online.members.push(friend(crate::online::HOST, 0));
        while l.count(1) < TEAM_SIZE {
            l.add_bot(1);
        }
        assert!(l.online_switch(0));
        assert_eq!((l.bots[0].len(), l.bots[1].len()), (1, TEAM_SIZE - 1));
        // Public: 1v1 may not become 0v2, but 2v1 may become 1v2.
        let mut p = public_lobby();
        p.online.members.push(friend(1, 1));
        assert!(!p.online_switch(0));
        p.online.members.push(friend(2, 0));
        assert!(p.online_switch(0));
    }

    #[test]
    fn a_public_lobby_is_full_at_its_team_size() {
        let mut l = public_lobby();
        for peer in 1..12 {
            let (team, bot) = l.seat(false).expect("room");
            assert!(bot.is_none());
            l.online.members.push(friend(peer, team));
        }
        assert!(l.seat(false).is_none());
        assert_eq!(l.count(0) + l.count(1), 12);
    }
}
