//! Console-style matchmaking: Find Match and the lobby browser.
//!
//! Find Match asks every relay ([`crate::online::relays`]) for the best open
//! lobbies of the playlist under a ping cap that loosens the longer it
//! looks ([`ping_cap`]), joins the best one (fullest, then a lobby still
//! waiting, then the nearest), and tries the next if that fails. With none
//! to join it hosts a public lobby on the nearest relay; the lobby counts
//! down and fills its empty slots with bots at the start (the lobby side,
//! `ui/lobby_public.rs`). Later searchers find it; a human who joins takes
//! a bot's slot, in the lobby or (once netplay takes [`JoinInProgress`])
//! during the match.
//!
//! The relays are asked on a thread of their own ([`Lookout`]): one
//! browsing connection each, kept while it's needed. A relay's answer
//! carries each lobby's ping, the relay's round trips to its host and to
//! us added, never anyone's address.

use crate::modes::GameMode;
use crate::online::lobby::Member;
use crate::online::{Online, RelayEntry, Status};
use bevy::prelude::*;
use cod4rw_multiplayer::client::Browser;
use cod4rw_multiplayer::matchmaking::{Filter, LobbyInfo, sort_for_quick_match};
use std::sync::Mutex;
use tokio::sync::mpsc;

pub struct MatchmakingPlugin;

impl Plugin for MatchmakingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Matchmaker>().add_message::<JoinInProgress>().add_systems(Update, drive);
    }
}

/// Host, in a match: a human from matchmaking is joining. Netplay spawns
/// their soldier on `member.team`, in place of the bot `replaces_bot` (gone
/// from the match), and their game loads the match as a guest. Only sent
/// while [`AcceptsJoinInProgress`] exists; without it the host turns
/// latecomers away and lists its lobby closed while playing.
#[derive(Message, Clone, Debug)]
pub struct JoinInProgress {
    pub member: Member,
    pub replaces_bot: Option<String>,
}

/// Inserted by netplay once it handles [`JoinInProgress`].
#[derive(Resource, Default)]
pub struct AcceptsJoinInProgress;

/// What Find Match looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Playlist {
    Tdm,
}

impl Playlist {
    pub fn mode(self) -> GameMode {
        match self {
            Playlist::Tdm => GameMode::Tdm,
        }
    }
    pub fn name(self) -> &'static str {
        self.mode().name()
    }
    /// Players a side (humans and bots): 6v6, as on console.
    pub fn team_size(self) -> usize {
        6
    }
}

/// A game type's id in listings (its CoD4 gametype: `war`, `dm`...).
pub fn mode_id(mode: GameMode) -> &'static str {
    mode.gametype()
}

/// The ping cap after searching `secs`: 80 ms at first, loosening to 150 ms
/// by 20 s.
pub fn ping_cap(secs: f32) -> u16 {
    const START: f32 = 80.0;
    const END: f32 = 150.0;
    let t = ((secs - 5.0) / 15.0).clamp(0.0, 1.0);
    (START + (END - START) * t).round() as u16
}

/// Searching longer than this with nothing to join: host instead (env
/// `COD4RW_MM_SEARCH`, seconds).
fn search_time() -> f32 {
    env_secs("COD4RW_MM_SEARCH", 25.0)
}
const ASK_EVERY: f32 = 2.0;
/// The browser list refreshes this often while it's up.
const LIST_EVERY: f32 = 5.0;
/// Joined but sent away this soon (full, started): try another.
const REFUSAL_WINDOW: f32 = 6.0;

pub fn env_secs(key: &str, default: f32) -> f32 {
    std::env::var(key).ok().and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(default)
}

/// A lobby and the relay it's on (an index into [`Matchmaker::relays`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub relay: usize,
    pub info: LobbyInfo,
}

/// A relay as last seen: its round trip, or why it couldn't be asked.
#[derive(Clone, Debug, PartialEq)]
pub struct RelayStatus {
    pub entry: RelayEntry,
    pub rtt: Option<u16>,
    pub error: Option<String>,
}

/// Quick match across relays: the best lobby not tried yet.
pub fn choose(found: &[Found], tried: &[(usize, u64)]) -> Option<Found> {
    let mut open: Vec<Found> = found.iter().filter(|f| f.info.open() && !tried.contains(&(f.relay, f.info.room))).cloned().collect();
    let mut infos: Vec<LobbyInfo> = open.iter().map(|f| f.info.clone()).collect();
    sort_for_quick_match(&mut infos);
    let best = infos.first()?;
    let at = open.iter().position(|f| f.info.room == best.room && f.info == *best)?;
    Some(open.swap_remove(at))
}

/// Where Find Match is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    Searching { playlist: Playlist, since: f32, next_ask: f32, tried: Vec<(usize, u64)> },
    Joining { playlist: Option<Playlist>, relay: usize, room: u64, since: f32, tried: Vec<(usize, u64)> },
    /// In someone else's public lobby.
    Joined { playlist: Option<Playlist>, relay: usize, room: u64, since: f32, tried: Vec<(usize, u64)> },
    /// Hosting a public lobby of our own.
    Hosting { playlist: Playlist, relay: usize },
}

#[derive(Resource, Default)]
pub struct Matchmaker {
    lookout: Option<Lookout>,
    pub phase: Phase,
    pub relays: Vec<RelayStatus>,
    /// The browser's list, best first, and when it came.
    pub lobbies: Vec<Found>,
    pub listed_at: Option<f32>,
    /// The browser's filter.
    pub filter: Filter,
    /// The browser screen is up: keep its list fresh.
    pub browsing: bool,
    /// What's happening, for the player.
    pub status: String,
    /// In someone else's public lobby: its listing (its countdown).
    pub joined: Option<LobbyInfo>,
    in_flight: bool,
    next_list: f32,
    refresh: bool,
    now: f32,
}

impl Matchmaker {
    /// Find Match.
    pub fn find(&mut self, playlist: Playlist) {
        if matches!(self.phase, Phase::Idle) {
            self.phase = Phase::Searching { playlist, since: self.now, next_ask: self.now, tried: Vec::new() };
            self.status = "Searching for a match...".into();
        }
    }
    /// Stop searching (not once in a lobby: leaving the lobby does that).
    pub fn cancel(&mut self) {
        if matches!(self.phase, Phase::Searching { .. }) {
            self.phase = Phase::Idle;
            self.status.clear();
        }
    }
    pub fn searching(&self) -> bool {
        matches!(self.phase, Phase::Searching { .. } | Phase::Joining { .. })
    }
    /// The browser's Refresh.
    pub fn refresh(&mut self) {
        self.refresh = true;
    }
    /// Join a lobby from the browser.
    pub fn join(&mut self, online: &mut Online, found: &Found) {
        let Some(relay) = self.relays.get(found.relay) else { return };
        online.join_listed(relay.entry.clone(), found.info.room);
        self.phase = Phase::Joining { playlist: None, relay: found.relay, room: found.info.room, since: self.now, tried: Vec::new() };
        self.status = "Joining...".into();
    }
    /// Hosting a public lobby: the playlist and the relay's region tag.
    pub fn hosting(&self) -> Option<(Playlist, String)> {
        match &self.phase {
            Phase::Hosting { playlist, relay } => Some((*playlist, self.relays.get(*relay).map_or("local".into(), |r| r.entry.region.clone()))),
            _ => None,
        }
    }
    /// In a public lobby, hosting or not.
    pub fn public(&self) -> bool {
        matches!(self.phase, Phase::Joined { .. } | Phase::Hosting { .. })
    }
    /// The relay's host election moved the host role to (`host`) or from
    /// this game: hosting a public lobby becomes being in one, or back.
    pub fn role_changed(&mut self, host: bool, room: Option<u64>) {
        match (self.phase.clone(), host) {
            (Phase::Joined { playlist, relay, .. }, true) => {
                self.phase = Phase::Hosting { playlist: playlist.unwrap_or(Playlist::Tdm), relay };
            }
            (Phase::Hosting { relay, .. }, false) => {
                self.phase = Phase::Joined { playlist: None, relay, room: room.unwrap_or(0), since: self.now - REFUSAL_WINDOW, tried: Vec::new() };
            }
            _ => {}
        }
    }
    /// The lobby was left: matchmaking is done with it.
    pub fn left(&mut self) {
        if !matches!(self.phase, Phase::Idle | Phase::Searching { .. }) {
            self.phase = Phase::Idle;
            self.joined = None;
        }
    }
    /// Seconds searched so far.
    pub fn searched(&self) -> f32 {
        match &self.phase {
            Phase::Searching { since, .. } => self.now - since,
            _ => 0.0,
        }
    }

    fn ask(&mut self, quick: bool, filter: Filter) {
        let lookout = self.lookout.get_or_insert_with(Lookout::start);
        if lookout.cmds.send(Ask { quick, filter }).is_ok() {
            self.in_flight = true;
        } else {
            self.lookout = None;
        }
    }

    fn host(&mut self, online: &mut Online, playlist: Playlist) {
        // The nearest relay that answered (else the first).
        let best = self.relays.iter().enumerate().filter_map(|(i, r)| r.rtt.map(|t| (t, i))).min().map_or(0, |(_, i)| i);
        let entry = self.relays.get(best).map(|r| r.entry.clone()).unwrap_or_else(crate::online::default_relay);
        if self.relays.is_empty() {
            self.relays.push(RelayStatus { entry: entry.clone(), rtt: None, error: None });
        }
        info!("matchmaking: hosting a {} lobby on {}", playlist.name(), entry.region);
        online.host_on(entry);
        self.phase = Phase::Hosting { playlist, relay: best };
        self.status = "No match found: hosting one.".into();
    }

    /// A relay's answer.
    fn answered(&mut self, answer: Answer, online: &mut Online) {
        self.in_flight = false;
        self.relays = answer.relays;
        let now = self.now;
        if !answer.quick {
            if let Phase::Joined { relay, room, .. } = &self.phase {
                self.joined = answer.lobbies.iter().find(|f| f.relay == *relay && f.info.room == *room).map(|f| f.info.clone());
            }
            self.lobbies = answer.lobbies;
            self.listed_at = Some(now);
            return;
        }
        let Phase::Searching { playlist, since, tried, .. } = &self.phase else { return };
        let (playlist, since, tried) = (*playlist, *since, tried.clone());
        let none_anywhere = answer.total == 0 && self.relays.iter().any(|r| r.rtt.is_some());
        let unreachable = self.relays.iter().all(|r| r.rtt.is_none());
        if let Some(best) = choose(&answer.lobbies, &tried) {
            info!(
                "matchmaking: joining a {} lobby on {} ({} players, {} ms)",
                best.info.listing.map,
                self.relays.get(best.relay).map_or("?", |r| r.entry.region.as_str()),
                best.info.humans,
                best.info.ping
            );
            let relay = self.relays[best.relay].entry.clone();
            online.join_listed(relay, best.info.room);
            self.phase = Phase::Joining { playlist: Some(playlist), relay: best.relay, room: best.info.room, since: now, tried };
            self.status = "Match found. Joining...".into();
        } else if none_anywhere || now - since >= search_time() || (unreachable && now - since >= ASK_EVERY * 2.0) {
            self.host(online, playlist);
        } else {
            self.status = if answer.total > 0 {
                format!("Searching for a match... ({} lobbies further than {} ms)", answer.total, ping_cap(now - since))
            } else {
                "Searching for a match...".into()
            };
        }
    }

    fn step(&mut self, online: &mut Online) {
        let now = self.now;
        if let Some(lookout) = &self.lookout {
            let answers: Vec<Answer> = lookout.answers.lock().map(|mut rx| std::iter::from_fn(|| rx.try_recv().ok()).collect()).unwrap_or_default();
            for a in answers {
                self.answered(a, online);
            }
        }
        match self.phase.clone() {
            Phase::Idle => {}
            Phase::Searching { playlist, since, next_ask, tried } => {
                if !self.in_flight && now >= next_ask {
                    let filter = Filter { mode: Some(mode_id(playlist.mode()).into()), map: None, open_only: true, max_ping: ping_cap(now - since) };
                    self.ask(true, filter);
                    self.phase = Phase::Searching { playlist, since, next_ask: now + ASK_EVERY, tried };
                }
            }
            Phase::Joining { playlist, relay, room, since, mut tried } => match &online.status {
                Status::Connected(_) => {
                    self.phase = Phase::Joined { playlist, relay, room, since, tried };
                    self.status.clear();
                }
                Status::Connecting => {}
                other => {
                    let why = if let Status::Failed(w) = other { w.clone() } else { String::new() };
                    tried.push((relay, room));
                    self.retry_or_stop(playlist, tried, why);
                }
            },
            Phase::Joined { playlist, relay, room, since, mut tried } => {
                if !matches!(online.status, Status::Connected(_)) {
                    // Sent away right after joining (full, started): the next one.
                    if now - since < REFUSAL_WINDOW && playlist.is_some() {
                        tried.push((relay, room));
                        self.retry_or_stop(playlist, tried, String::new());
                    } else {
                        self.left();
                    }
                }
            }
            Phase::Hosting { .. } => {
                if matches!(online.status, Status::Off | Status::Failed(_)) {
                    self.left();
                }
            }
        }
        // The browser's list (and a guest's own lobby's countdown).
        let watching = self.browsing || matches!(self.phase, Phase::Joined { .. });
        if watching && !self.in_flight && !self.searching() && (self.refresh || now >= self.next_list) {
            self.refresh = false;
            self.next_list = now + if self.browsing { LIST_EVERY } else { ASK_EVERY };
            let filter = if self.browsing { self.filter.clone() } else { Filter::default() };
            self.ask(false, filter);
        }
        // Nothing to ask: let the relays go.
        if !watching && !self.searching() && !self.in_flight {
            self.lookout = None;
        }
    }

    fn retry_or_stop(&mut self, playlist: Option<Playlist>, tried: Vec<(usize, u64)>, why: String) {
        match playlist {
            Some(playlist) => {
                // Searching again, the ping cap part loosened already.
                self.phase = Phase::Searching { playlist, since: self.now - 10.0, next_ask: self.now, tried };
                self.status = "Searching for a match...".into();
            }
            None => {
                self.phase = Phase::Idle;
                self.status = if why.is_empty() { "Couldn't join that lobby.".into() } else { why };
            }
        }
    }
}

fn drive(mut mm: ResMut<Matchmaker>, mut online: ResMut<Online>, time: Res<Time<Real>>) {
    mm.now = time.elapsed_secs();
    mm.step(&mut online);
}

// ------------------------------------------------------------ the lookout

struct Ask {
    quick: bool,
    filter: Filter,
}

struct Answer {
    quick: bool,
    relays: Vec<RelayStatus>,
    lobbies: Vec<Found>,
    /// Quick match: the open lobbies at any ping, on all relays.
    total: usize,
}

/// The browsing thread: one connection per relay.
struct Lookout {
    cmds: mpsc::UnboundedSender<Ask>,
    answers: Mutex<mpsc::UnboundedReceiver<Answer>>,
}

impl Lookout {
    fn start() -> Lookout {
        let (cmds, mut rx) = mpsc::unbounded_channel::<Ask>();
        let (tx, answers) = mpsc::unbounded_channel();
        let spawned = std::thread::Builder::new().name("matchmaking".into()).spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            rt.block_on(async move {
                let relays = crate::online::relays();
                let mut browsers: Vec<Option<Browser>> = relays.iter().map(|_| None).collect();
                while let Some(ask) = rx.recv().await {
                    let mut status = Vec::new();
                    let mut lobbies = Vec::new();
                    let mut total = 0;
                    for (i, entry) in relays.iter().enumerate() {
                        let result = ask_relay(entry, &mut browsers[i], &ask).await;
                        match result {
                            Ok((n, found, rtt)) => {
                                total += n as usize;
                                lobbies.extend(found.into_iter().map(|info| Found { relay: i, info }));
                                status.push(RelayStatus { entry: entry.clone(), rtt: Some(rtt), error: None });
                            }
                            Err(e) => {
                                debug!("matchmaking: relay {} ({}): {e:#}", entry.region, entry.address);
                                browsers[i] = None;
                                status.push(RelayStatus { entry: entry.clone(), rtt: None, error: Some(format!("{e}")) });
                            }
                        }
                    }
                    if !ask.quick {
                        lobbies.sort_by_key(|f: &Found| (!f.info.open(), std::cmp::Reverse(f.info.humans), f.info.ping));
                    }
                    if tx.send(Answer { quick: ask.quick, relays: status, lobbies, total }).is_err() {
                        break;
                    }
                }
            });
        });
        if let Err(e) = spawned {
            warn!("matchmaking: couldn't start its thread: {e}");
        }
        Lookout { cmds, answers: Mutex::new(answers) }
    }
}

/// One relay's answer: (total, lobbies, round trip ms).
async fn ask_relay(entry: &RelayEntry, browser: &mut Option<Browser>, ask: &Ask) -> anyhow::Result<(u16, Vec<LobbyInfo>, u16)> {
    if browser.as_ref().is_none_or(|b| b.is_closed()) {
        *browser = Some(open_browser(entry).await?);
    }
    let b = browser.as_mut().expect("just opened");
    let (total, lobbies) = if ask.quick { b.quick_match(ask.filter.clone()).await? } else { b.list(ask.filter.clone()).await? };
    let rtt = b.rtt().as_millis().min(u16::MAX as u128) as u16;
    Ok((total, lobbies, rtt))
}

async fn open_browser(entry: &RelayEntry) -> anyhow::Result<Browser> {
    let address = entry.resolve()?;
    // A relay on this PC that nobody runs yet (its port is free): this game
    // starts the test relay, as hosting would.
    if entry.embeddable() && std::net::UdpSocket::bind(address).is_ok() {
        info!("matchmaking: no relay at {}; starting one", entry.address);
        if let Err(e) = crate::online::ensure_embedded_relay(address) {
            debug!("matchmaking: test relay: {e:#}");
        }
    }
    let browser = Browser::connect(address, entry.trust()?).await?;
    Ok(browser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cod4rw_multiplayer::matchmaking::{Listing, LobbyState};

    fn found(relay: usize, room: u64, humans: u8, ping: u16) -> Found {
        let listing = Listing {
            mode: "war".into(),
            map: "mp_crash".into(),
            bots: 0,
            max: 12,
            state: LobbyState::Lobby { countdown: 20 },
            joinable: true,
            region: "local".into(),
        };
        Found { relay, info: LobbyInfo { room, listing, humans, ping } }
    }

    #[test]
    fn ping_cap_loosens() {
        assert_eq!(ping_cap(0.0), 80);
        assert_eq!(ping_cap(5.0), 80);
        assert_eq!(ping_cap(12.5), 115);
        assert_eq!(ping_cap(20.0), 150);
        assert_eq!(ping_cap(90.0), 150);
    }

    #[test]
    fn chooses_the_fullest_untried_lobby_across_relays() {
        let all = [found(0, 1, 2, 30), found(1, 2, 5, 70), found(0, 3, 5, 40)];
        assert_eq!(choose(&all, &[]).map(|f| (f.relay, f.info.room)), Some((0, 3)));
        assert_eq!(choose(&all, &[(0, 3)]).map(|f| (f.relay, f.info.room)), Some((1, 2)));
        let mut full = found(0, 9, 12, 5);
        full.info.listing.max = 12;
        assert!(choose(&[full], &[]).is_none());
    }
}
