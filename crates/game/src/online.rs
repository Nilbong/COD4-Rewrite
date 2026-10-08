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
//! The relay is `COD4RW_RELAY` (`host:port`, default this PC's port 28970)
//! trusted through its certificate `COD4RW_RELAY_CERT` (default
//! `%LOCALAPPDATA%\cod4rw\relay.der`). With no relay running on this PC, a
//! host starts one inside the game for testing on one machine (it writes
//! that certificate for the second copy of the game to read);
//! `COD4RW_RELAY_BIND=0.0.0.0:28970` opens it to the local network, for a
//! second PC given the certificate. Online play proper needs a relay on a
//! server of its own: a relay on a player's PC shows that player's address.

use anyhow::{Context, Result, bail};
use bevy::prelude::*;
use cod4rw_multiplayer::client::Session;
use cod4rw_multiplayer::lobby::LobbyMsg;
use cod4rw_multiplayer::protocol::{Control, Hello, InputCommand, LobbyCode, Packet, PeerId, Snapshot};
use cod4rw_multiplayer::{relay::Relay, transport};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::sync::mpsc;

pub use cod4rw_multiplayer::lobby;
pub use cod4rw_multiplayer::protocol::{EVERYONE, HOST};

const DEFAULT_RELAY: &str = "127.0.0.1:28970";
/// The relay's room needs a map name; the lobby's real settings travel in
/// [`LobbyMsg::Lobby`].
const ROOM_MAP: &str = "mp_lobby";

pub struct OnlinePlugin;

impl Plugin for OnlinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Online>();
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
    Ready { peer: PeerId, code: Option<LobbyCode> },
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
    Inputs(Vec<InputCommand>),
    Snapshot(PeerId, Snapshot),
}

struct Link {
    cmds: mpsc::UnboundedSender<Cmd>,
    events: Mutex<mpsc::UnboundedReceiver<Event>>,
}

/// The game's side of the connection.
#[derive(Resource, Default)]
pub struct Online {
    link: Option<Link>,
    pub status: Status,
    /// The host's lobby code, for its friends.
    pub code: Option<LobbyCode>,
    /// The match's traffic, for [`crate::netplay`].
    game: Vec<Event>,
}

impl Online {
    /// Open a lobby on the relay (Invite Friends).
    pub fn host(&mut self) {
        self.start(None);
    }
    /// Join a friend's lobby by its code.
    pub fn join(&mut self, code: LobbyCode) {
        self.start(Some(code));
    }
    fn start(&mut self, code: Option<LobbyCode>) {
        self.leave();
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let spawned = std::thread::Builder::new().name("online".into()).spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(anyhow::Error::from)
                .and_then(|rt| rt.block_on(run(code, cmd_rx, event_tx.clone())));
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
        self.code = None;
        self.status = Status::Off;
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
        if let Ok(mut events) = link.events.lock() {
            while let Ok(e) = events.try_recv() {
                match e {
                    Event::Inputs(..)
                    | Event::Snapshot(_)
                    | Event::Message(_, LobbyMsg::Pawn(_) | LobbyMsg::PawnGone(_) | LobbyMsg::Class(_) | LobbyMsg::MatchOver(_) | LobbyMsg::Kill { .. } | LobbyMsg::Scores { .. } | LobbyMsg::Throw { .. } | LobbyMsg::Launch { .. }) => {
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
                Event::Ready { peer, code } => {
                    match code {
                        Some(code) => info!("online: hosting a lobby, code {code}"),
                        None => info!("online: joined a lobby as player {peer}"),
                    }
                    self.status = Status::Connected(*peer);
                    self.code = *code;
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

fn relay_address() -> Result<SocketAddr> {
    let name = std::env::var("COD4RW_RELAY").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_RELAY.into());
    name.to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .with_context(|| format!("The relay address \"{name}\" isn't valid."))
}

fn cert_path() -> Option<PathBuf> {
    std::env::var_os("COD4RW_RELAY_CERT").map(PathBuf::from).or_else(|| data_dir().map(|d| d.join("relay.der")))
}

/// A relay inside this game for testing without a server: bound to this PC
/// unless `COD4RW_RELAY_BIND` says otherwise. Its certificate is written
/// where the other copy of the game looks for it.
fn embedded_relay(relay: SocketAddr) -> Result<cod4rw_multiplayer::Endpoint> {
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
    tokio::spawn(Relay::new().run(endpoint.clone()));
    Ok(endpoint)
}

async fn connect(relay: SocketAddr, hello: Hello) -> Result<Session> {
    let cert = cert_path().and_then(|p| std::fs::read(p).ok()).context("No relay certificate: nobody is hosting through this relay.")?;
    Session::connect(relay, cert, hello).await
}

async fn run(code: Option<LobbyCode>, mut cmds: mpsc::UnboundedReceiver<Cmd>, events: mpsc::UnboundedSender<Event>) -> Result<()> {
    let relay = relay_address()?;
    let mut _embedded = None;
    let mut session = match code {
        Some(code) => connect(relay, Hello::JoinCode(code))
            .await
            .map_err(|e| {
                info!("online: join failed: {e:#}");
                anyhow::anyhow!("Couldn't join that lobby. Check the code; the lobby may have closed.")
            })?,
        None => {
            let hello = Hello::Create { map: ROOM_MAP.into() };
            match connect(relay, hello.clone()).await {
                Ok(s) => s,
                Err(e) if relay.ip().is_loopback() || std::env::var("COD4RW_RELAY_BIND").is_ok() => {
                    info!("online: no relay at {relay} ({e:#}); starting one");
                    _embedded = Some(embedded_relay(relay)?);
                    connect(relay, hello).await.context("Couldn't open a lobby")?
                }
                Err(e) => return Err(e.context("Couldn't reach the relay")),
            }
        }
    };
    let _ = events.send(Event::Ready { peer: session.peer, code: session.code });
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
                    bail!(if code.is_some() { "The host closed the lobby." } else { "Lost the connection to the relay." })
                }
                debug!("online: dropped a packet: {e}");
            }
            Next::Control(None) => {
                bail!(if code.is_some() { "The host closed the lobby." } else { "Lost the connection to the relay." })
            }
            Next::Control(Some(c)) => {
                let event = match c {
                    Control::PeerJoined(_) => Event::PeerJoined,
                    Control::PeerLeft(p) => Event::PeerLeft(p),
                    Control::Message { peer, data } => match LobbyMsg::decode(&data) {
                        Ok(m) => Event::Message(peer, m),
                        Err(e) => {
                            warn!("online: bad lobby message from {peer}: {e}");
                            continue;
                        }
                    },
                    Control::Welcome { .. } => continue,
                };
                if events.send(event).is_err() {
                    return Ok(());
                }
            }
        }
    }
}
