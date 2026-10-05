//! A master server for cod4rw: games announce themselves to it
//! (`heartbeat`), it checks each is real by asking it for its info with a
//! challenge, and server browsers ask it for the list (`getservers`).
//! Games that stop sending heartbeats drop off after ten minutes.
//!
//! `cargo run -p cod4rw-net --bin master [port]` (CoD4's 20810 by
//! default). Point the game at it with `COD4RW_MASTER=host:port`; without
//! that, the game uses one on this machine.

use cod4rw_net::{GAME, MASTER_PORT, challenge, packet, parse, parse_info, servers_response};
use std::collections::HashMap;
use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

/// A game drops off the list this long after its last heartbeat (Quake 3's
/// games send one every five minutes).
const EXPIRE: Duration = Duration::from_secs(600);
/// A heartbeat's challenge must be answered within this.
const CHECK_TIME: Duration = Duration::from_secs(5);

fn main() -> std::io::Result<()> {
    let port = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(MASTER_PORT);
    let socket = UdpSocket::bind(("0.0.0.0", port))?;
    socket.set_read_timeout(Some(Duration::from_secs(1)))?;
    println!("cod4rw master server on UDP port {port}");
    let mut master = Master::default();
    let mut buf = [0u8; 2048];
    loop {
        if let Ok((n, from)) = socket.recv_from(&mut buf) {
            for (reply, to) in master.handle(&buf[..n], from, Instant::now()) {
                let _ = socket.send_to(&reply, to);
            }
        }
        master.expire(Instant::now());
    }
}

#[derive(Default)]
struct Master {
    /// Listed games and their last heartbeat.
    servers: HashMap<SocketAddrV4, Instant>,
    /// Games being checked: the challenge sent and when.
    checking: HashMap<SocketAddrV4, (String, Instant)>,
}

impl Master {
    /// Replies to a packet from `from`.
    fn handle(&mut self, data: &[u8], from: SocketAddr, now: Instant) -> Vec<(Vec<u8>, SocketAddr)> {
        let SocketAddr::V4(from4) = from else { return Vec::new() };
        let Some((command, rest)) = parse(data) else { return Vec::new() };
        let text = String::from_utf8_lossy(rest);
        match command {
            "heartbeat" if text.trim() == GAME => {
                // Check it's a game answering there before listing it.
                let c = challenge();
                let ask = packet(&format!("getinfo {c}"));
                self.checking.insert(from4, (c, now));
                vec![(ask, from)]
            }
            "infoResponse" => {
                let info = parse_info(&text);
                let ok = self.checking.get(&from4).is_some_and(|(c, _)| info.get("challenge") == Some(c))
                    && info.get("gamename").is_some_and(|g| g == GAME);
                if ok {
                    self.checking.remove(&from4);
                    if self.servers.insert(from4, now).is_none() {
                        println!("listed {from4} ({})", info.get("hostname").map_or("", String::as_str));
                    }
                }
                Vec::new()
            }
            "getservers" if text.split_whitespace().next() == Some(GAME) => {
                let list: Vec<SocketAddrV4> = self.servers.keys().copied().collect();
                servers_response(&list).into_iter().map(|p| (p, from)).collect()
            }
            _ => Vec::new(),
        }
    }

    fn expire(&mut self, now: Instant) {
        self.servers.retain(|addr, seen| {
            let keep = now.duration_since(*seen) < EXPIRE;
            if !keep {
                println!("dropped {addr}");
            }
            keep
        });
        self.checking.retain(|_, (_, at)| now.duration_since(*at) < CHECK_TIME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cod4rw_net::{Info, info_response, parse_servers};

    #[test]
    fn games_are_checked_listed_and_expired() {
        let mut m = Master::default();
        let game: SocketAddr = "10.0.0.5:28960".parse().unwrap();
        let browser: SocketAddr = "10.0.0.9:50000".parse().unwrap();
        let t0 = Instant::now();
        // A heartbeat gets a challenge back, not a listing.
        let replies = m.handle(&packet("heartbeat cod4rw\n"), game, t0);
        let (command, rest) = parse(&replies[0].0).unwrap();
        assert_eq!(command, "getinfo");
        assert!(m.servers.is_empty());
        // The wrong challenge doesn't list it; the right one does.
        let answer = |c: &str| {
            let mut info = Info::new();
            info.insert("challenge".into(), c.into());
            info.insert("gamename".into(), GAME.into());
            info_response(&info)
        };
        m.handle(&answer("nope"), game, t0);
        assert!(m.servers.is_empty());
        m.handle(&answer(std::str::from_utf8(rest).unwrap()), game, t0);
        assert_eq!(m.servers.len(), 1);
        // Browsers get it.
        let list = m.handle(&packet("getservers cod4rw 1 full empty"), browser, t0);
        assert_eq!(parse_servers(parse(&list[0].0).unwrap().1), vec![game]);
        // Other games' browsers don't.
        assert!(m.handle(&packet("getservers QuakeArena-1 68 full empty"), browser, t0).is_empty());
        // Silent for long enough, it drops off.
        m.expire(t0 + EXPIRE + Duration::from_secs(1));
        assert!(m.servers.is_empty());
    }
}
