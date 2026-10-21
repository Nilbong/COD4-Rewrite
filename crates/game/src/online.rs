//! Online private matches: the connection behind the Private Match lobby's
//! "Invite Friends" and "Join with Code".
//!
//! Everyone, host included, connects out to a relay (`crates/multiplayer`),
//! never to each other, so nobody in a lobby learns anyone else's address.
//! The host's relay room has a short code ("K7QM-2X9D") that works only
//! while that lobby exists. What's said in the lobby (who's in it with their
//! combat record names and ranks, the host's settings, the start) is
//! [`LobbyMsg`]s, decided by `ui::lobby`; this module only carries them.
//!
//! The network runs on a thread of its own (a small tokio runtime); the game
//! talks to it through channels and never waits on it.
//!
//! The relay is the official one (`eu`, its certificate built in) unless
//! `COD4RW_RELAY` (`host:port`) names a test relay, trusted through its
//! certificate `COD4RW_RELAY_CERT` (default `%LOCALAPPDATA%\cod4rw\relay.der`).
//! With no relay running at a test address on this PC, a host starts one
//! inside the game (`COD4RW_RELAY=127.0.0.1:28970`; it writes that
//! certificate for the second copy of the game to read);
//! `COD4RW_RELAY_BIND=0.0.0.0:28970` opens it to the local network, for a
//! second PC given the certificate. A relay on a player's PC shows that
//! player's address: online play proper uses the official relay.
//!
//! Matchmaking ([`crate::matchmaking`]) may know several relays, one per
//! region: `COD4RW_RELAYS=eu=203.0.113.5:28970,us=relay.example:28970`
//! (an entry may add `@path\to\relay.der` for its own certificate). Without
//! it there's the one relay above.

use anyhow::{Context, Result, bail};
use bevy::prelude::*;
use cod4rw_multiplayer::client::Session;
use cod4rw_multiplayer::lobby::LobbyMsg;
use cod4rw_multiplayer::matchmaking::Listing;
use cod4rw_multiplayer::protocol::{Control, Hello, InputCommand, LobbyCode, Packet, PeerId, Snapshot};
use cod4rw_multiplayer::{relay::Relay, transport};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::sync::mpsc;

pub use cod4rw_multiplayer::lobby;
pub use cod4rw_multiplayer::protocol::{EVERYONE, HOST};

/// The relay's room needs a map name; the lobby's real settings travel in
/// [`LobbyMsg::Lobby`].
const ROOM_MAP: &str = "mp_lobby";

pub struct OnlinePlugin;

impl Plugin for OnlinePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Online { sim: NetSim::from_env(), ..default() });
    }
}

/// Where the lobby connection is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Off,
    Connecting,
    /// In a lobby: this game's peer ID (0 for the host).
    Connected(PeerId),
    /// It ended or never started: why, for the player.
    Failed(String),
}

/// What the network thread reports.
#[derive(Debug)]
pub enum Event {
    /// In the room; the host gets its code.
    Ready { peer: PeerId, code: Option<LobbyCode>, room: u64 },
    /// The relay chose the match's host ([`Online::elect`]): `Some(p)`, player
    /// `p` took the host's ID and the old host took `p`'s.
    Elected(Option<PeerId>),
    PeerJoined,
    PeerLeft(PeerId),
    Message(PeerId, LobbyMsg),
    /// Host: a player's latest controls (with the last few, in case of loss).
    Inputs(PeerId, Vec<InputCommand>),
    /// Guest: the host's world.
    Snapshot(Snapshot),
    /// The connection ended: why, for the player.
    Closed(String),
}

enum Cmd {
    Send(PeerId, Vec<u8>),
    /// Host: list the lobby publicly, or take it off.
    Publish(Option<Listing>),
    /// Host: let the relay choose the match's host.
    Elect,
    Inputs(Vec<InputCommand>),
    Snapshot(PeerId, Snapshot),
}

struct Link {
    cmds: mpsc::UnboundedSender<Cmd>,
    events: Mutex<mpsc::UnboundedReceiver<Event>>,
}

/// `COD4RW_NETSIM=ms,loss%[,jitter ms]`: a bad connection for testing.
/// Everything this game receives arrives `ms` late (give or take the
/// jitter, for the realtime packets, which may then come out of order),
/// and that share of the realtime packets (controls, snapshots) is lost.
/// Set on both games, a round trip gains twice `ms`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NetSim {
    pub delay: f32,
    pub loss: f32,
    pub jitter: f32,
}

impl NetSim {
    pub fn from_env() -> Option<NetSim> {
        let v = std::env::var("COD4RW_NETSIM").ok()?;
        let mut it = v.split(',').map(|x| x.trim().trim_end_matches('%').parse::<f32>().unwrap_or(0.0));
        let sim = NetSim {
            delay: it.next().unwrap_or(0.0).clamp(0.0, 2000.0) / 1000.0,
            loss: it.next().unwrap_or(0.0).clamp(0.0, 100.0) / 100.0,
            jitter: it.next().unwrap_or(0.0).clamp(0.0, 1000.0) / 1000.0,
        };
        info!("online: simulating a connection: {:.0} ms late, {:.0}% lost, {:.0} ms jitter", sim.delay * 1000.0, sim.loss * 100.0, sim.jitter * 1000.0);
        Some(sim)
    }
}

/// The game's side of the connection.
#[derive(Resource, Default)]
pub struct Online {
    link: Option<Link>,
    /// [`NetSim`], and what it's holding back: (due, event), and the last
    /// reliable one's due time (they stay in order).
    sim: Option<NetSim>,
    held: Vec<(std::time::Instant, Event)>,
    reliable_due: Option<std::time::Instant>,
    pub status: Status,
    /// The host's lobby code, for its friends.
    pub code: Option<LobbyCode>,
    /// The relay this lobby is on, and its room there.
    pub relay: Option<RelayEntry>,
    pub room: Option<u64>,
    /// The match's traffic, for [`crate::netplay`].
    game: Vec<Event>,
}

impl Online {
    /// Open a lobby on the relay (Invite Friends).
    pub fn host(&mut self) {
        self.start(default_relay(), Target::Host);
    }
    /// Open a lobby on a given relay (matchmaking lists it with [`Online::publish`]).
    pub fn host_on(&mut self, relay: RelayEntry) {
        self.start(relay, Target::Host);
    }
    /// Join a friend's lobby by its code.
    pub fn join(&mut self, code: LobbyCode) {
        self.start(default_relay(), Target::Code(code));
    }
    /// Join a public lobby from a relay's list.
    pub fn join_listed(&mut self, relay: RelayEntry, room: u64) {
        self.start(relay, Target::Listed(room));
    }
    /// Host, as the match is about to start: the relay makes the player
    /// nearest it the host ([`Event::Elected`]).
    pub fn elect(&self) {
        if let Some(link) = &self.link {
            let _ = link.cmds.send(Cmd::Elect);
        }
    }
    /// Host: list this lobby publicly (again: at least every few seconds),
    /// or (`None`) take it off the list.
    pub fn publish(&self, listing: Option<Listing>) {
        if let Some(link) = &self.link {
            let _ = link.cmds.send(Cmd::Publish(listing));
        }
    }
    fn start(&mut self, relay: RelayEntry, target: Target) {
        self.leave();
        self.relay = Some(relay.clone());
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let spawned = std::thread::Builder::new().name("online".into()).spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(anyhow::Error::from)
                .and_then(|rt| rt.block_on(run(relay, target, cmd_rx, event_tx.clone())));
            let reason = match result {
                Ok(()) => "Left the lobby.".to_string(),
                Err(e) => {
                    warn!("online: {e:#}");
                    format!("{e}")
                }
            };
            let _ = event_tx.send(Event::Closed(reason));
        });
        if let Err(e) = spawned {
            self.status = Status::Failed(format!("Couldn't start networking: {e}"));
            return;
        }
        self.link = Some(Link { cmds, events: Mutex::new(events) });
        self.status = Status::Connecting;
        self.code = None;
    }
    /// Leave the lobby (dropping the channel ends the network thread).
    pub fn leave(&mut self) {
        self.link = None;
        self.relay = None;
        self.room = None;
        self.held.clear();
        self.reliable_due = None;
        self.code = None;
        self.status = Status::Off;
    }
    /// [`NetSim`]: what's arrived, held back for its delay, minus losses;
    /// whatever's now due comes out.
    fn simulate(&mut self, arrived: Vec<Event>) -> Vec<Event> {
        let Some(sim) = self.sim else { return arrived };
        use rand::Rng;
        let now = std::time::Instant::now();
        let mut rng = rand::rng();
        for e in arrived {
            let realtime = matches!(e, Event::Inputs(..) | Event::Snapshot(_));
            if realtime && rng.random::<f32>() < sim.loss {
                continue;
            }
            let due = if realtime {
                now + std::time::Duration::from_secs_f32((sim.delay + sim.jitter * (rng.random::<f32>() * 2.0 - 1.0)).max(0.0))
            } else {
                // Reliable: late, in order (and a closed link arrives last).
                let due = (now + std::time::Duration::from_secs_f32(sim.delay)).max(self.reliable_due.unwrap_or(now));
                self.reliable_due = Some(due);
                due
            };
            self.held.push((due, e));
        }
        let mut out: Vec<(std::time::Instant, Event)> = Vec::new();
        let mut i = 0;
        while i < self.held.len() {
            if self.held[i].0 <= now {
                out.push(self.held.remove(i));
            } else {
                i += 1;
            }
        }
        out.sort_by_key(|(due, _)| *due);
        out.into_iter().map(|(_, e)| e).collect()
    }

    /// Send a lobby message (players to [`HOST`]; the host to a player or [`EVERYONE`]).
    pub fn send(&self, to: PeerId, msg: &LobbyMsg) {
        let Some(link) = &self.link else { return };
        match msg.encode() {
            Ok(data) => {
                let _ = link.cmds.send(Cmd::Send(to, data));
            }
            Err(e) => warn!("online: not sending {msg:?}: {e}"),
        }
    }
    /// Guest: this frame's controls, with the previous ones for redundancy.
    pub fn send_inputs(&self, commands: Vec<InputCommand>) {
        if let Some(link) = &self.link {
            let _ = link.cmds.send(Cmd::Inputs(commands));
        }
    }
    /// Host: a player's view of the world.
    pub fn send_snapshot(&self, to: PeerId, snapshot: Snapshot) {
        if let Some(link) = &self.link {
            let _ = link.cmds.send(Cmd::Snapshot(to, snapshot));
        }
    }
    /// The match's traffic since last time: controls, snapshots, pawns'
    /// details, classes, and who left.
    pub fn take_game(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.game)
    }
    /// What happened in the lobby since last time; the status follows it.
    /// The match's traffic is kept for [`Online::take_game`].
    pub fn poll(&mut self) -> Vec<Event> {
        let Some(link) = &self.link else { return Vec::new() };
        let mut out = Vec::new();
        let mut arrived = Vec::new();
        if let Ok(mut events) = link.events.lock() {
            while let Ok(e) = events.try_recv() {
                arrived.push(e);
            }
        }
        let arrived = self.simulate(arrived);
        {
            for e in arrived {
                match e {
                    Event::Inputs(..)
                    | Event::Snapshot(_)
                    | Event::Message(_, LobbyMsg::Pawn(_) | LobbyMsg::PawnGone(_) | LobbyMsg::Class(_) | LobbyMsg::MatchOver(_) | LobbyMsg::Kill { .. } | LobbyMsg::Scores { .. } | LobbyMsg::Throw { .. } | LobbyMsg::Launch { .. } | LobbyMsg::Game(_)) => {
                        self.game.push(e)
                    }
                    Event::PeerLeft(p) => {
                        self.game.push(Event::PeerLeft(p));
                        out.push(e);
                    }
                    // In a match it switches the friend's soldier too.
                    Event::Message(p, LobbyMsg::SwitchTeam) => {
                        self.game.push(Event::Message(p, LobbyMsg::SwitchTeam));
                        out.push(e);
                    }
                    e => out.push(e),
                }
            }
        }
        // Nobody's reading it outside a match: don't let it pile up.
        if self.game.len() > 4096 {
            self.game.clear();
        }
        for e in &out {
            match e {
                Event::Ready { peer, code, room } => {
                    self.room = Some(*room);
                    match code {
                        Some(code) => info!("online: hosting a lobby, code {code}"),
                        None => info!("online: joined a lobby as player {peer}"),
                    }
                    self.status = Status::Connected(*peer);
                    self.code = *code;
                }
                Event::Elected(Some(new)) => {
                    if let Status::Connected(me) = self.status {
                        let me = if me == HOST {
                            *new
                        } else if me == *new {
                            HOST
                        } else {
                            me
                        };
                        info!("online: the relay chose a new host; this game is now player {me}");
                        self.status = Status::Connected(me);
                    }
                }
                Event::Closed(reason) => {
                    self.game.clear();
                    self.link = None;
                    self.code = None;
                    self.status = Status::Failed(reason.clone());
                }
                _ => {}
            }
        }
        out
    }
}

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("cod4rw"))
}

/// A relay the game can use: its region's name, where it is, and the
/// certificate it's trusted by.
#[derive(Clone, Debug, PartialEq)]
pub struct RelayEntry {
    /// The region tag lobbies on it are listed with ("eu", "local").
    pub region: String,
    /// `host:port`, resolved when connecting.
    pub address: String,
    pub cert: Option<PathBuf>,
    /// A certificate built into the game (the official relays').
    pub builtin: Option<&'static [u8]>,
}

/// The official relay: Hetzner, Falkenstein (Germany).
const EU_RELAY: &str = "167.233.105.90:28970";
/// Its public certificate (`data/relay/relay.der`; the private key never
/// leaves the server).
const EU_CERT: &[u8] = include_bytes!("../assets/relay/eu.der");

impl RelayEntry {
    pub fn resolve(&self) -> Result<SocketAddr> {
        self.address
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .with_context(|| format!("The relay address \"{}\" isn't valid.", self.address))
    }
    /// Its certificate, read now.
    pub fn trust(&self) -> Result<Vec<u8>> {
        if let Some(path) = &self.cert {
            return std::fs::read(path).with_context(|| format!("Couldn't read the relay certificate {}", path.display()));
        }
        if let Some(der) = self.builtin {
            return Ok(der.to_vec());
        }
        cert_path().and_then(|p| std::fs::read(p).ok()).context("No relay certificate: nobody is hosting through this relay.")
    }
    /// On this PC (or the test network): a game may start it itself.
    pub fn embeddable(&self) -> bool {
        self.resolve().is_ok_and(|a| a.ip().is_loopback()) || std::env::var("COD4RW_RELAY_BIND").is_ok()
    }
}

/// The relays matchmaking may use (`COD4RW_RELAYS`), else the one relay
/// ([`default_relay`]).
pub fn relays() -> Vec<RelayEntry> {
    let list = std::env::var("COD4RW_RELAYS").unwrap_or_default();
    let parsed = parse_relays(&list);
    if parsed.is_empty() { vec![default_relay()] } else { parsed }
}

/// `name=host:port[@cert],...`; an entry without a name is named after its host.
fn parse_relays(list: &str) -> Vec<RelayEntry> {
    list.split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| {
            let (e, cert) = match e.split_once('@') {
                Some((e, c)) => (e, Some(PathBuf::from(c.trim()))),
                None => (e, None),
            };
            let (region, address) = match e.split_once('=') {
                Some((n, a)) => (n.trim(), a.trim()),
                None => (e.split(':').next().unwrap_or(e), e),
            };
            let region: String = region.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(16).collect();
            RelayEntry { region: if region.is_empty() { "relay".into() } else { region }, address: address.into(), cert, builtin: None }
        })
        .collect()
}

/// The relay for Invite Friends and Join with Code: `COD4RW_RELAY` (a test
/// relay, `local`, trusted through `COD4RW_RELAY_CERT`), else the official
/// one, `eu`.
pub fn default_relay() -> RelayEntry {
    match std::env::var("COD4RW_RELAY").ok().filter(|s| !s.trim().is_empty()) {
        Some(address) => RelayEntry { region: "local".into(), address: address.trim().into(), cert: None, builtin: None },
        None => RelayEntry { region: "eu".into(), address: EU_RELAY.into(), cert: None, builtin: Some(EU_CERT) },
    }
}

fn cert_path() -> Option<PathBuf> {
    std::env::var_os("COD4RW_RELAY_CERT").map(PathBuf::from).or_else(|| data_dir().map(|d| d.join("relay.der")))
}

/// The test relay inside this game, started once and kept for as long as
/// the game runs (other copies of the game may be using it).
pub fn ensure_embedded_relay(relay: SocketAddr) -> Result<()> {
    static STARTED: Mutex<bool> = Mutex::new(false);
    let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
    if *started {
        return Ok(());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("test-relay".into()).spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(Err(anyhow::Error::from(e)));
                return;
            }
        };
        rt.block_on(async move {
            match embedded_relay(relay) {
                Ok((endpoint, run)) => {
                    let _ = tx.send(Ok(()));
                    let _ = run.await;
                    drop(endpoint);
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        });
    })?;
    rx.recv().context("The test relay didn't start")??;
    *started = true;
    Ok(())
}

/// A relay inside this game for testing without a server: bound to this PC
/// unless `COD4RW_RELAY_BIND` says otherwise. Its certificate is written
/// where the other copy of the game looks for it.
fn embedded_relay(relay: SocketAddr) -> Result<(cod4rw_multiplayer::Endpoint, impl std::future::Future<Output = Result<()>>)> {
    let bind: SocketAddr = match std::env::var("COD4RW_RELAY_BIND") {
        Ok(b) if !b.trim().is_empty() => b.trim().parse().context("COD4RW_RELAY_BIND isn't an address")?,
        _ => relay,
    };
    let (cert, key) = transport::generate_identity()?;
    if let Some(path) = cert_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, &cert).with_context(|| format!("Couldn't write {}", path.display()))?;
    }
    let endpoint = transport::server_endpoint(bind, cert, key)?;
    info!("online: started a relay on {bind} for testing");
    let run = Relay::new().run(endpoint.clone());
    Ok((endpoint, run))
}

/// What the network thread is to do.
enum Target {
    Host,
    Code(LobbyCode),
    Listed(u64),
}

async fn connect(relay: &RelayEntry, hello: Hello) -> Result<Session> {
    Session::connect(relay.resolve()?, relay.trust()?, hello).await
}

async fn run(relay: RelayEntry, target: Target, mut cmds: mpsc::UnboundedReceiver<Cmd>, events: mpsc::UnboundedSender<Event>) -> Result<()> {
    let joining = !matches!(target, Target::Host);
    let mut session = match target {
        Target::Code(code) => connect(&relay, Hello::JoinCode(code))
            .await
            .map_err(|e| {
                info!("online: join failed: {e:#}");
                anyhow::anyhow!("Couldn't join that lobby. Check the code; the lobby may have closed.")
            })?,
        Target::Listed(room) => connect(&relay, Hello::JoinListed { room })
            .await
            .map_err(|e| {
                info!("online: join failed: {e:#}");
                anyhow::anyhow!("Couldn't join that lobby: it may be full or gone.")
            })?,
        Target::Host => {
            let hello = Hello::Create { map: ROOM_MAP.into() };
            match connect(&relay, hello.clone()).await {
                Ok(s) => s,
                Err(e) if relay.embeddable() => {
                    info!("online: no relay at {} ({e:#}); starting one", relay.address);
                    ensure_embedded_relay(relay.resolve()?)?;
                    connect(&relay, hello).await.context("Couldn't open a lobby")?
                }
                Err(e) => return Err(e.context("Couldn't reach the relay")),
            }
        }
    };
    let _ = events.send(Event::Ready { peer: session.peer, code: session.code, room: session.room });
    enum Next {
        Cmd(Option<Cmd>),
        Control(Option<Control>),
        Packet(Result<Packet>),
    }
    let packets = session.packets();
    loop {
        let next = tokio::select! {
            c = cmds.recv() => Next::Cmd(c),
            c = session.next_control() => Next::Control(c),
            p = packets.next() => Next::Packet(p),
        };
        match next {
            // The game dropped the link: leave.
            Next::Cmd(None) => return Ok(()),
            Next::Cmd(Some(Cmd::Send(to, data))) => session.send_message(to, data).await?,
            Next::Cmd(Some(Cmd::Publish(listing))) => session.publish(listing).await?,
            Next::Cmd(Some(Cmd::Elect)) => session.elect().await?,
            // Realtime traffic is replaceable: a full send buffer drops it.
            Next::Cmd(Some(Cmd::Inputs(commands))) => {
                if let Err(e) = session.send_inputs(commands) {
                    debug!("online: inputs not sent: {e}");
                }
            }
            Next::Cmd(Some(Cmd::Snapshot(to, state))) => {
                if let Err(e) = session.send_snapshot(to, state) {
                    debug!("online: snapshot not sent: {e}");
                }
            }
            Next::Packet(Ok(packet)) => {
                let event = match packet {
                    Packet::RemoteInputs { peer, commands } => Event::Inputs(peer, commands),
                    Packet::Snapshot { state, .. } => Event::Snapshot(state),
                    Packet::Inputs(_) => continue,
                };
                if events.send(event).is_err() {
                    return Ok(());
                }
            }
            Next::Packet(Err(e)) => {
                if session.is_closed() {
                    bail!(if joining { "The host closed the lobby." } else { "Lost the connection to the relay." })
                }
                debug!("online: dropped a packet: {e}");
            }
            Next::Control(None) => {
                bail!(if joining { "The host closed the lobby." } else { "Lost the connection to the relay." })
            }
            Next::Control(Some(c)) => {
                let event = match c {
                    Control::PeerJoined(_) => Event::PeerJoined,
                    Control::PeerLeft(p) => Event::PeerLeft(p),
                    Control::Elected { swap } => Event::Elected(swap),
                    Control::Message { peer, data } => match LobbyMsg::decode(&data) {
                        Ok(m) => Event::Message(peer, m),
                        // Another version of the game: told so plainly, both
                        // ways (a host answers in the joiner's own format,
                        // which every version reads as "different version").
                        Err(_) if data.len() >= 2 && data[0] == b'L' && data[1] != lobby::LOBBY_VERSION => {
                            info!("online: player {peer} is on another version of the game (lobby format {} here, {} there)", lobby::LOBBY_VERSION, data[1]);
                            if session.current_peer() == HOST {
                                let _ = session.send_message(peer, lobby::version_refusal(data[1])).await;
                                continue;
                            }
                            Event::Message(peer, LobbyMsg::Refused(lobby::Refusal::Version))
                        }
                        Err(e) => {
                            warn!("online: bad lobby message from {peer}: {e}");
                            continue;
                        }
                    },
                    // (Only the relay hears these; it never sends them.)
                    Control::Welcome { .. } | Control::Publish(_) | Control::List(_) | Control::QuickMatch(_) | Control::Lobbies { .. } | Control::Elect => continue,
                };
                if events.send(event).is_err() {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_lists_parse() {
        let r = parse_relays(" eu=203.0.113.5:28970, us=relay.example:28970@D:\\keys\\us.der ,10.0.0.2:1 ,");
        assert_eq!(r.len(), 3);
        assert_eq!((r[0].region.as_str(), r[0].address.as_str(), r[0].cert.is_none()), ("eu", "203.0.113.5:28970", true));
        assert_eq!(r[1].cert.as_deref(), Some(std::path::Path::new("D:\\keys\\us.der")));
        assert_eq!((r[2].region.as_str(), r[2].address.as_str()), ("10002", "10.0.0.2:1"));
        assert!(parse_relays("").is_empty());
    }
}
