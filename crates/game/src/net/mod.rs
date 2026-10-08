//! Finding games: groundwork for multiplayer. There's no netcode to join a
//! game with yet, but games already announce themselves and the browser
//! already finds them, the way CoD4 does it (the `cod4rw-net` crate's
//! protocol):
//!
//! - a match being played can answer `getinfo` queries on UDP port 28960
//!   (the next free one up to 28963) with its name, map, mode and players,
//!   and heartbeat a master server ([`Host`]). Off unless
//!   `COD4RW_ADVERTISE=1`: with nothing to join yet, it would only make
//!   Windows ask to let the game on the network every match;
//! - the server browser ([`Browser`], CoD4's Join Game menu) asks the local
//!   network (a broadcast), a master server (`getservers`) or the player's
//!   favourites, and lists whoever answers, with their ping.
//!
//! The master server is `cod4rw-net`'s `master` (`cargo run -p cod4rw-net
//! --bin master`). Its address is `net_master`
//! (`COD4RW_MASTER` overrides it); favourites are kept in
//! `%LOCALAPPDATA%\cod4rw\favorites.txt` (not by debug runs).

pub use cod4rw_net as protocol;

use protocol::{Info, MIN_QUERY, PORTS, RateLimit, packet, padded, parse, parse_info, parse_servers, unpad};
use bevy::prelude::*;
use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Browser>()
            .init_resource::<Host>()
            .add_systems(Update, poll_browser)
            .add_systems(Update, host.run_if(crate::state::in_game))
            .add_systems(OnExit(crate::state::GameState::InGame), |mut host: ResMut<Host>| host.close());
    }
}

/// The master server when nothing's set: one running on this machine
/// (`cargo run -p cod4rw-net --bin master`).
const DEFAULT_MASTER: &str = "127.0.0.1:20810";
/// A master is told a game is up this often (seconds), as Quake 3 does.
const HEARTBEAT: f32 = 300.0;
/// A query unanswered for this long is given up on.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The master server's address (`host:port`).
pub fn master() -> String {
    std::env::var("COD4RW_MASTER").ok().filter(|m| !m.is_empty()).unwrap_or_else(|| DEFAULT_MASTER.into())
}

/// Where to look.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Source {
    /// The local network (CoD4's "Local").
    #[default]
    Local,
    Internet,
    Favorites,
}

impl Source {
    /// From CoD4's `ui_netSource` (0 local, 1 internet, 2 favourites).
    pub fn from_dvar(v: i32) -> Source {
        match v {
            1 => Source::Internet,
            2 => Source::Favorites,
            _ => Source::Local,
        }
    }
}

/// A game that answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub addr: SocketAddr,
    pub name: String,
    pub map: String,
    pub gametype: String,
    pub players: u32,
    pub max_players: u32,
    pub hardcore: bool,
    pub password: bool,
    /// Round trip, milliseconds; `None` for a favourite that didn't answer.
    pub ping: Option<u32>,
    pub version: String,
}

impl Server {
    fn from_info(addr: SocketAddr, info: &Info, ping: u32) -> Server {
        let get = |k: &str| info.get(k).cloned().unwrap_or_default();
        let num = |k: &str| info.get(k).and_then(|v| v.parse().ok()).unwrap_or(0);
        Server {
            addr,
            name: get("hostname"),
            map: get("mapname"),
            gametype: get("gametype"),
            players: num("clients"),
            max_players: num("sv_maxclients"),
            hardcore: num("hc") != 0,
            password: num("pswrd") != 0,
            ping: Some(ping),
            version: get("version"),
        }
    }
}

/// How to order the list: CoD4's `ServerSort` columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Password,
    Name,
    Map,
    Players,
    Type,
    Ping,
}

impl SortKey {
    /// From `ServerSort`'s column number (the menu's header order).
    pub fn from_column(c: i32) -> SortKey {
        match c {
            0 => SortKey::Password,
            2 => SortKey::Name,
            3 => SortKey::Map,
            4 => SortKey::Players,
            5 => SortKey::Type,
            _ => SortKey::Ping,
        }
    }
}

/// The server browser.
#[derive(Resource)]
pub struct Browser {
    socket: Option<UdpSocket>,
    pub source: Source,
    /// Who answered, sorted, and filtered by `gametype` when it's set.
    pub servers: Vec<Server>,
    pub sort: (SortKey, bool),
    pub gametype: Option<String>,
    /// Queries waiting for an answer: who, the challenge, when sent.
    pending: HashMap<SocketAddr, (String, Instant)>,
    /// The master's address (resolved) while its list is awaited.
    master: Option<SocketAddr>,
    pub refreshed: Option<std::time::SystemTime>,
    pub favorites: Vec<String>,
    favorites_path: Option<PathBuf>,
    /// Last problem to show (no master, ...).
    pub status: Option<String>,
}

impl Default for Browser {
    fn default() -> Browser {
        let favorites_path = (!debug_run()).then(|| data_dir().map(|d| d.join("favorites.txt"))).flatten();
        let favorites = favorites_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect())
            .unwrap_or_default();
        Browser {
            socket: None,
            source: Source::Local,
            servers: Vec::new(),
            sort: (SortKey::Ping, true),
            gametype: None,
            pending: HashMap::new(),
            master: None,
            refreshed: None,
            favorites,
            favorites_path,
            status: None,
        }
    }
}

impl Browser {
    fn socket(&mut self) -> Option<&UdpSocket> {
        if self.socket.is_none() {
            let s = UdpSocket::bind("0.0.0.0:0").ok()?;
            s.set_nonblocking(true).ok()?;
            s.set_broadcast(true).ok()?;
            self.socket = Some(s);
        }
        self.socket.as_ref()
    }

    /// Ask `source` for its games afresh.
    pub fn refresh(&mut self, source: Source) {
        self.source = source;
        self.servers.clear();
        self.pending.clear();
        self.master = None;
        self.status = None;
        self.refreshed = Some(std::time::SystemTime::now());
        match source {
            Source::Local => {
                // A broadcast for the network, and this machine directly (a
                // broadcast doesn't always come back to its sender).
                for port in PORTS {
                    self.query(SocketAddr::from(([255, 255, 255, 255], port)));
                    self.query(SocketAddr::from(([127, 0, 0, 1], port)));
                }
            }
            Source::Internet => {
                let master = master();
                match master.to_socket_addrs().ok().and_then(|mut a| a.find(SocketAddr::is_ipv4)) {
                    Some(addr) => {
                        let ask = padded(packet(&format!("getservers {} {} full empty", protocol::GAME, protocol::PROTOCOL)));
                        if let Some(s) = self.socket() {
                            let _ = s.send_to(&ask, addr);
                        }
                        self.master = Some(addr);
                    }
                    None => self.status = Some(format!("Can't find the master server {master}")),
                }
            }
            Source::Favorites => {
                let favorites = self.favorites.clone();
                for f in favorites {
                    match resolve(&f) {
                        Some(addr) => {
                            self.query(addr);
                            self.servers.push(unanswered(addr, &f));
                        }
                        None => warn!("net: can't find favourite {f}"),
                    }
                }
            }
        }
        self.order();
    }

    /// Ask the games already listed again (CoD4's "Quick Refresh").
    pub fn requery(&mut self) {
        let addrs: Vec<SocketAddr> = self.servers.iter().map(|s| s.addr).collect();
        for a in addrs {
            self.query(a);
        }
    }

    fn query(&mut self, addr: SocketAddr) {
        let challenge = protocol::challenge();
        let ask = padded(packet(&format!("getinfo {challenge}")));
        if let Some(s) = self.socket() {
            if s.send_to(&ask, addr).is_ok() {
                // A broadcast's answers come from each game's own address.
                self.pending.insert(addr, (challenge, Instant::now()));
            }
        }
    }

    /// Read whatever has arrived.
    fn poll(&mut self) {
        let mut buf = [0u8; 4096];
        let mut answers = Vec::new();
        if let Some(s) = self.socket.as_ref() {
            while let Ok((n, from)) = s.recv_from(&mut buf) {
                answers.push((buf[..n].to_vec(), from));
            }
        }
        for (data, from) in answers {
            let Some((command, rest)) = parse(&data) else { continue };
            match command {
                "infoResponse" => self.answer(from, rest),
                "getserversResponse" if self.master == Some(from) => {
                    for addr in parse_servers(rest) {
                        self.query(addr);
                    }
                }
                _ => {}
            }
        }
        self.pending.retain(|_, (_, at)| at.elapsed() < TIMEOUT);
    }

    fn answer(&mut self, from: SocketAddr, rest: &[u8]) {
        let info = parse_info(&String::from_utf8_lossy(rest));
        if info.get("gamename").is_some_and(|g| g != protocol::GAME) {
            return;
        }
        // Asked directly, or by a broadcast to its port.
        let asked = self
            .pending
            .iter()
            .find(|(a, _)| **a == from || (a.ip().to_string() == "255.255.255.255" && a.port() == from.port()))
            .map(|(a, p)| (*a, p.clone()));
        let Some((asked, (challenge, at))) = asked else { return };
        if info.get("challenge") != Some(&challenge) {
            return;
        }
        if asked == from {
            self.pending.remove(&asked);
        }
        let server = Server::from_info(from, &info, at.elapsed().as_millis() as u32);
        // The same game by broadcast and by 127.0.0.1: keep one.
        let same = |s: &Server| s.addr == from || (s.addr.port() == from.port() && (s.addr.ip().is_loopback() || from.ip().is_loopback()));
        match self.servers.iter_mut().find(|s| same(s)) {
            Some(s) => {
                let addr = if s.addr.ip().is_loopback() { from } else { s.addr };
                *s = Server { addr, ..server };
            }
            None => self.servers.push(server),
        }
        self.order();
    }

    /// Sort, and hide what the game type filter excludes.
    fn order(&mut self) {
        let (key, ascending) = self.sort;
        self.servers.sort_by(|a, b| {
            let o = match key {
                SortKey::Password => a.password.cmp(&b.password),
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::Map => a.map.cmp(&b.map),
                SortKey::Players => b.players.cmp(&a.players),
                SortKey::Type => a.gametype.cmp(&b.gametype),
                SortKey::Ping => a.ping.unwrap_or(u32::MAX).cmp(&b.ping.unwrap_or(u32::MAX)),
            };
            if ascending { o } else { o.reverse() }
        });
    }

    /// The games to show (the game type filter applied).
    pub fn shown(&self) -> Vec<&Server> {
        self.servers.iter().filter(|s| self.gametype.as_ref().is_none_or(|g| g.eq_ignore_ascii_case(&s.gametype))).collect()
    }

    /// Sort by a column (again: the other way round).
    pub fn sort_by(&mut self, key: SortKey) {
        self.sort = (key, if self.sort.0 == key { !self.sort.1 } else { true });
        self.order();
    }

    pub fn add_favorite(&mut self, address: &str) {
        let address = address.trim();
        if address.is_empty() || self.favorites.iter().any(|f| f.eq_ignore_ascii_case(address)) {
            return;
        }
        self.favorites.push(address.to_owned());
        self.save_favorites();
    }

    pub fn remove_favorite(&mut self, address: &str) {
        self.favorites.retain(|f| !f.eq_ignore_ascii_case(address.trim()));
        self.servers.retain(|s| s.addr.to_string() != address.trim());
        self.save_favorites();
    }

    fn save_favorites(&self) {
        if let Some(p) = &self.favorites_path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, self.favorites.join("\n") + "\n");
        }
    }
}

/// A favourite before (or without) an answer.
fn unanswered(addr: SocketAddr, name: &str) -> Server {
    Server {
        addr,
        name: name.to_owned(),
        map: String::new(),
        gametype: String::new(),
        players: 0,
        max_players: 0,
        hardcore: false,
        password: false,
        ping: None,
        version: String::new(),
    }
}

/// `host[:port]` (the port defaults to CoD4's).
pub fn resolve(address: &str) -> Option<SocketAddr> {
    let with_port = if address.contains(':') { address.to_owned() } else { format!("{address}:{}", protocol::PORT) };
    with_port.to_socket_addrs().ok()?.find(SocketAddr::is_ipv4)
}

fn poll_browser(mut browser: ResMut<Browser>) {
    if browser.socket.is_some() {
        browser.poll();
    }
}

/// The `COD4RW_*` variables meant for real play (network settings and the
/// Wet Work showcase's weather and clock, first-person feel `COD4RW_FP_*`), so they don't make a run a debug
/// run (menus, sound, saving).
pub fn setting(key: &str) -> bool {
    matches!(
        key,
        "COD4RW_ADVERTISE"
            | "COD4RW_MASTER"
            | "COD4RW_RELAY"
            | "COD4RW_RELAY_CERT"
            | "COD4RW_RELAY_BIND"
            | "COD4RW_SHOWCASE"
            | "COD4RW_TOD"
            | "COD4RW_TOD_SPEED"
            | "COD4RW_RAIN"
    ) || key.starts_with("COD4RW_FP_")
}

/// Whether matches announce themselves (`COD4RW_ADVERTISE=1`).
pub fn advertise() -> bool {
    std::env::var("COD4RW_ADVERTISE").is_ok_and(|v| v == "1")
}

fn debug_run() -> bool {
    std::env::vars().any(|(k, _)| k.starts_with("COD4RW_") && !setting(&k))
}

fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("cod4rw"))
}

/// The match being played, announcing itself.
#[derive(Resource, Default)]
pub struct Host {
    socket: Option<UdpSocket>,
    /// Tried to open the socket (a port may all be taken).
    tried: bool,
    next_heartbeat: f32,
    /// The master's challenge-checking `getinfo` is answered like anyone's.
    master: Option<SocketAddr>,
    /// Queries answered are rationed ([`RateLimit`]).
    limit: Option<RateLimit>,
}

impl Host {
    fn close(&mut self) {
        *self = Host::default();
    }

    /// The port it's answering on.
    pub fn port(&self) -> Option<u16> {
        self.socket.as_ref().and_then(|s| s.local_addr().ok()).map(|a| a.port())
    }
}

#[allow(clippy::too_many_arguments)]
fn host(
    time: Res<Time>,
    mut host: ResMut<Host>,
    config: Option<Res<crate::tdm::MatchConfig>>,
    map: Option<Res<crate::world::MapName>>,
    pawns: Query<(&crate::combat::Pawn, Has<crate::player::LocalPlayer>)>,
    fe: Option<Res<crate::ui::Frontend>>,
) {
    if !host.tried {
        host.tried = true;
        if !advertise() {
            return;
        }
        host.socket = PORTS.into_iter().find_map(|p| UdpSocket::bind(("0.0.0.0", p)).ok());
        match &host.socket {
            Some(s) => {
                let _ = s.set_nonblocking(true);
                info!("net: answering server queries on port {}", host.port().unwrap_or_default());
            }
            None => warn!("net: ports {}-{} are taken; this game won't show in server browsers", PORTS.start(), PORTS.end()),
        }
        host.master = resolve(&master());
        host.next_heartbeat = 0.0;
    }
    let host = &mut *host;
    let Some(socket) = host.socket.as_ref() else { return };
    let now = time.elapsed_secs();
    if now >= host.next_heartbeat {
        if let Some(m) = host.master {
            let _ = socket.send_to(&padded(packet(&format!("heartbeat {}\n", protocol::GAME))), m);
        }
        host.next_heartbeat = now + HEARTBEAT;
    }
    let limit = host.limit.get_or_insert_with(|| RateLimit::new(5.0, 2.0, 100.0));
    let mut buf = [0u8; 1024];
    while let Ok((n, from)) = socket.recv_from(&mut buf) {
        // Padded queries only (a reply no bigger than the ask), and only so
        // many a second, per address and in all.
        if n < MIN_QUERY || !limit.allow(from.ip(), std::time::Instant::now()) {
            continue;
        }
        let Some(("getinfo", rest)) = parse(&buf[..n]) else { continue };
        let challenge: String = String::from_utf8_lossy(unpad(rest)).trim().chars().take(32).collect();
        // The player's profile name, never the computer's user name: anyone
        // can ask for this.
        let me = fe.as_deref().map_or_else(|| "Player".to_owned(), crate::ui::Frontend::profile_name);
        let mut info = Info::new();
        let mut put = |k: &str, v: String| {
            info.insert(k.to_owned(), v);
        };
        put("challenge", challenge);
        put("gamename", protocol::GAME.into());
        put("protocol", protocol::PROTOCOL.to_string());
        put("version", env!("CARGO_PKG_VERSION").into());
        put("hostname", format!("{me}'s game"));
        put("mapname", map.as_ref().map_or_else(String::new, |m| m.0.clone()));
        put("gametype", config.as_ref().map_or("war", |c| c.mode.gametype()).into());
        put("clients", pawns.iter().count().to_string());
        put("sv_maxclients", "18".into());
        put("hc", (config.as_ref().is_some_and(|c| c.hardcore) as u8).to_string());
        put("pswrd", "0".into());
        let _ = socket.send_to(&protocol::info_response(&info), from);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browser_finds_a_host_on_this_machine() {
        // A host on a spare port, answering like `host` does.
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let addr = server.local_addr().unwrap();
        let mut browser = Browser { favorites_path: None, ..Browser::default() };
        browser.favorites = vec![addr.to_string()];
        browser.refresh(Source::Favorites);
        assert_eq!(browser.servers[0].ping, None);
        let mut buf = [0u8; 512];
        let mut answered = false;
        for _ in 0..200 {
            if let Ok((n, from)) = server.recv_from(&mut buf) {
                assert!(n >= MIN_QUERY, "queries are padded");
                let (command, rest) = parse(&buf[..n]).unwrap();
                assert_eq!(command, "getinfo");
                let mut info = Info::new();
                info.insert("challenge".into(), String::from_utf8_lossy(unpad(rest)).trim().to_owned());
                info.insert("gamename".into(), protocol::GAME.into());
                info.insert("hostname".into(), "Test".into());
                info.insert("mapname".into(), "mp_crash".into());
                info.insert("clients".into(), "5".into());
                server.send_to(&protocol::info_response(&info), from).unwrap();
                answered = true;
            }
            browser.poll();
            if browser.servers.first().is_some_and(|s| s.ping.is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(answered);
        let s = &browser.servers[0];
        assert_eq!((s.name.as_str(), s.map.as_str(), s.players), ("Test", "mp_crash", 5));
        assert!(s.ping.is_some());
    }
}
