//! A trusted relay forwards validated messages and stamps sender identities.
//! It never forwards connection addresses or lets a client impersonate a host.

use crate::matchmaking::{self as mm, Filter, HostCandidate, LobbyInfo, Listing};
use crate::{protocol::*, security::*, transport};
use anyhow::{Result, bail, ensure};
use bytes::Bytes;
use quinn::{Connection, Endpoint, VarInt};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU16, Ordering},
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinSet,
    time::timeout,
};

struct Member {
    connection: Connection,
    control: mpsc::Sender<Control>,
    /// Its peer ID, shared with its connection's task: an election swaps it.
    id: Arc<AtomicU16>,
}
struct Room {
    map: String,
    invite: Invite,
    code: LobbyCode,
    next_peer: PeerId,
    members: HashMap<PeerId, Member>,
    /// Public rooms' listing, and when the host last sent it.
    listing: Option<(Listing, Instant)>,
}

impl Room {
    /// The room as a browser sees it, if it's listed and its host is still
    /// there. `asker_rtt`: the relay's round trip to whoever asks.
    fn info(&self, id: u64, now: Instant, ttl: std::time::Duration, asker_rtt: std::time::Duration) -> Option<LobbyInfo> {
        let (listing, at) = self.listing.as_ref()?;
        let host = self.members.get(&HOST)?;
        if now.duration_since(*at) > ttl || host.connection.close_reason().is_some() {
            return None;
        }
        let ping = (host.connection.rtt() + asker_rtt).as_millis().min(u16::MAX as u128) as u16;
        Some(LobbyInfo { room: id, listing: listing.clone(), humans: self.members.len().min(MAX_PLAYERS) as u8, ping })
    }
}
#[derive(Default)]
struct Registry {
    rooms: HashMap<u64, Room>,
    codes: HashMap<LobbyCode, u64>,
    by_ip: HashMap<IpAddr, usize>,
    /// Failed joins per address in the current window, so codes can't be
    /// found by guessing.
    failures: HashMap<IpAddr, (u32, Instant)>,
}

/// A player must be this much nearer the relay than the host to take over.
const ELECT_MARGIN: std::time::Duration = std::time::Duration::from_millis(10);

/// A browsing connection's questions: a few a second.
const BROWSE_RATE: f64 = 2.0;
const BROWSE_BURST: f64 = 10.0;
/// A browser is let go after this long (it reconnects to look again).
const BROWSE_LIFETIME: std::time::Duration = std::time::Duration::from_secs(600);
/// A host's listing updates: it only needs one every few seconds.
const PUBLISH_RATE: f64 = 2.0;
const PUBLISH_BURST: f64 = 6.0;

/// Failed joins allowed per address per [`FAILURE_WINDOW`].
const MAX_FAILURES: u32 = 10;
const FAILURE_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// Owns only local infrastructure state. QUIC remote addresses never enter
/// protocol messages, room advertisements, or client event queues.
#[derive(Clone)]
pub struct Relay {
    registry: Arc<Mutex<Registry>>,
    /// How long a listing lasts without a refresh.
    listing_ttl: std::time::Duration,
    /// Added to the host's round trip in elections (tests: a far host).
    host_penalty: std::time::Duration,
}

impl Default for Relay {
    fn default() -> Self {
        Self { registry: Default::default(), listing_ttl: mm::LISTING_TTL, host_penalty: Default::default() }
    }
}

impl Relay {
    pub fn new() -> Self {
        Self::default()
    }
    /// A relay that counts hosts as this much further away in elections
    /// (tests, where every connection is on this PC).
    pub fn with_host_penalty(penalty: std::time::Duration) -> Self {
        Self { host_penalty: penalty, ..Self::default() }
    }
    /// A relay whose listings expire sooner (tests).
    pub fn with_listing_ttl(ttl: std::time::Duration) -> Self {
        Self { listing_ttl: ttl, ..Self::default() }
    }

    /// Serve until the endpoint is closed. Shutdown closes rooms and awaits
    /// workers so permits, IP counters, and room memberships are released.
    pub async fn run(self, endpoint: Endpoint) -> Result<()> {
        let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let mut workers = JoinSet::new();
        let mut admissions = RateLimit::new(20.0, 40.0, Instant::now());
        loop {
            tokio::select! {
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else { break };
                    // QUIC address validation before spending a connection slot.
                    if !incoming.remote_address_validated() { let _ = incoming.retry(); continue; }
                    if !admissions.allow(1, Instant::now()) { incoming.refuse(); continue; }
                    let Ok(permit) = slots.clone().try_acquire_owned() else { incoming.refuse(); continue };
                    let ip = incoming.remote_address().ip();
                    {
                        let mut registry = self.registry.lock().unwrap();
                        let count = registry.by_ip.entry(ip).or_default();
                        if *count >= MAX_PER_IP { incoming.refuse(); continue; }
                        *count += 1;
                    }
                    let relay = self.clone();
                    workers.spawn(async move {
                        let _permit = permit;
                        let lease = IpLease { relay: relay.clone(), ip };
                        if let Ok(Ok(connection)) = timeout(HANDSHAKE_TIMEOUT, incoming).await {
                            let result = relay.handle(connection.clone(), ip).await;
                            if result.is_err() { connection.close(VarInt::from_u32(1), b"session rejected or closed"); }
                        }
                        drop(lease);
                    });
                }
                _ = workers.join_next(), if !workers.is_empty() => {}
            }
        }
        // Endpoint::close closes all QUIC connections. If the caller closed
        // only the incoming endpoint, explicitly terminate surviving rooms.
        let connections: Vec<_> = self
            .registry
            .lock()
            .unwrap()
            .rooms
            .values()
            .flat_map(|r| r.members.values().map(|m| m.connection.clone()))
            .collect();
        for connection in connections {
            connection.close(VarInt::from_u32(0), b"relay shutdown");
        }
        while workers.join_next().await.is_some() {}
        Ok(())
    }

    async fn handle(&self, connection: Connection, ip: IpAddr) -> Result<()> {
        let (mut send, mut recv) = timeout(HANDSHAKE_TIMEOUT, connection.accept_bi()).await??;
        let hello = Hello::decode(&timeout(HANDSHAKE_TIMEOUT, transport::read_frame(&mut recv)).await??)?;
        if hello == Hello::Browse {
            return self.browse(connection, send, recv).await;
        }
        let (control_tx, mut control_rx) = mpsc::channel(CONTROL_QUEUE);
        let id = Arc::new(AtomicU16::new(HOST));
        let welcome = self.admit(hello, Member { connection: connection.clone(), control: control_tx, id: id.clone() }, ip)?;
        let Control::Welcome { room, peer, .. } = &welcome else { unreachable!() };
        let (room, peer) = (*room, *peer);
        let lease = Membership { relay: self.clone(), room, id: id.clone() };
        // This is the only server-side control writer, preserving order.
        timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut send, &welcome.encode()?)).await??;
        self.announce_join(room, peer)?;

        // Allowances by role (an election can change it).
        struct Limits {
            host: bool,
            packets: RateLimit,
            bytes: RateLimit,
            message_count: RateLimit,
            message_bytes: RateLimit,
            replay: ReplayWindow,
            snapshot_ticks: HashMap<PeerId, u32>,
        }
        let limits = |host: bool| {
            let now = Instant::now();
            // The host tells everyone everything (kills, awards, streaks,
            // each guest's damage): three times a player's messages.
            let k = if host { 3.0 } else { 1.0 };
            Limits {
                host,
                packets: RateLimit::new(if host { 400.0 } else { 90.0 }, if host { 800.0 } else { 180.0 }, now),
                bytes: RateLimit::new(if host { 512_000.0 } else { 12_000.0 }, if host { 1_024_000.0 } else { 24_000.0 }, now),
                message_count: RateLimit::new(20.0 * k, 60.0 * k, now),
                message_bytes: RateLimit::new(16_000.0 * k, 32_000.0 * k, now),
                replay: ReplayWindow::default(),
                snapshot_ticks: HashMap::new(),
            }
        };
        let mut l = limits(peer == HOST);
        let mut publishes = RateLimit::new(PUBLISH_RATE, PUBLISH_BURST, Instant::now());
        // One persistent frame reader (read_exact isn't cancellation safe in
        // the select below); it ends on a malformed frame or a closed stream.
        let (frames_tx, mut frames) = mpsc::channel::<Vec<u8>>(CONTROL_QUEUE);
        let reader = tokio::spawn(async move {
            while let Ok(frame) = transport::read_frame(&mut recv).await {
                if frames_tx.send(frame).await.is_err() {
                    break;
                }
            }
        });
        let _reader = AbortOnDrop(reader);
        loop {
            let peer = id.load(Ordering::Relaxed);
            let host = peer == HOST;
            if host != l.host {
                l = limits(host);
            }
            let Limits { packets, bytes, message_count, message_bytes, replay, snapshot_ticks, .. } = &mut l;
            tokio::select! {
                data = connection.read_datagram() => {
                    let data = data?;
                    let now = Instant::now();
                    ensure!(packets.allow(1, now) && bytes.allow(data.len(), now), "rate limit exceeded");
                    let packet = Packet::decode(&data)?;
                    match packet {
                        Packet::Inputs(commands) if !host => {
                            let commands: Vec<_> = commands.into_iter().filter(|c| replay.accept(c.sequence)).collect();
                            if !commands.is_empty() {
                                self.forward(room, HOST, Packet::RemoteInputs { peer, commands })?;
                            }
                        }
                        Packet::Snapshot { recipient, state } if host && recipient != HOST => {
                            // Bound this map to allocated member IDs, not arbitrary
                            // recipient IDs supplied by a malicious host.
                            // A disconnect can race a host's last snapshot.
                            // Drop it; never close the entire match for that.
                            if !self.has_member(room, recipient) { continue; }
                            snapshot_ticks.retain(|id, _| self.has_member(room, *id));
                            if snapshot_ticks.get(&recipient).is_none_or(|last| newer(state.tick, *last)) {
                                snapshot_ticks.insert(recipient, state.tick);
                                self.forward(room, recipient, Packet::Snapshot { recipient, state })?;
                            }
                        }
                        _ => bail!("sender not authorized for packet type"),
                    }
                }
                control = control_rx.recv() => {
                    let Some(control) = control else { break };
                    timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut send, &control.encode()?)).await??;
                }
                frame = frames.recv() => {
                    let Some(frame) = frame else { break };
                    let now = Instant::now();
                    ensure!(
                        message_count.allow(1, now) && message_bytes.allow(frame.len(), now),
                        "message rate limit exceeded"
                    );
                    match Control::decode(&frame)? {
                        Control::Message { peer: to, data } => self.message(room, peer, to, data)?,
                        Control::Publish(listing) if host => {
                            ensure!(publishes.allow(1, now), "listing rate limit exceeded");
                            self.publish(room, listing);
                        }
                        Control::Elect if host => {
                            ensure!(publishes.allow(1, now), "election rate limit exceeded");
                            self.elect(room);
                        }
                        _ => bail!("only messages (and a host's listing) are accepted after joining"),
                    }
                }
                _ = connection.closed() => { break; }
            }
        }
        drop(lease);
        connection.close(VarInt::from_u32(0), b"session ended");
        Ok(())
    }

    fn admit(&self, hello: Hello, member: Member, ip: IpAddr) -> Result<Control> {
        let mut registry = self.registry.lock().unwrap();
        let now = Instant::now();
        if let Some(&(count, since)) = registry.failures.get(&ip) {
            if now.duration_since(since) >= FAILURE_WINDOW {
                registry.failures.remove(&ip);
            } else {
                ensure!(count < MAX_FAILURES, "join rejected");
            }
        }
        let result = Self::admit_locked(&mut registry, hello, member, self.listing_ttl);
        if result.is_err() {
            let entry = registry.failures.entry(ip).or_insert((0, now));
            entry.0 += 1;
            // Bounded: forget stale entries once it grows.
            if registry.failures.len() > 4096 {
                registry.failures.retain(|_, (_, since)| now.duration_since(*since) < FAILURE_WINDOW);
            }
        }
        result
    }

    fn admit_locked(registry: &mut Registry, hello: Hello, member: Member, ttl: std::time::Duration) -> Result<Control> {
        let hello = match hello {
            Hello::JoinCode(code) => {
                let Some(&room) = registry.codes.get(&code) else { bail!("join rejected") };
                let invite = registry.rooms.get(&room).map(|r| r.invite.clone());
                let Some(invite) = invite else { bail!("join rejected") };
                Hello::Join { room, invite }
            }
            Hello::JoinListed { room } => {
                // Only a listed room taking players; otherwise the same
                // failure as a wrong code.
                let now = Instant::now();
                let open = registry.rooms.get(&room).and_then(|r| r.info(room, now, ttl, Default::default())).is_some_and(|i| i.open());
                ensure!(open, "join rejected");
                let invite = registry.rooms[&room].invite.clone();
                Hello::Join { room, invite }
            }
            Hello::Browse => bail!("browsers don't join rooms"),
            hello => hello,
        };
        match hello {
            Hello::Create { map } => {
                ensure!(registry.rooms.len() < MAX_ROOMS, "relay full");
                let mut id: u64 = rand::random();
                while id == 0 || registry.rooms.contains_key(&id) {
                    id = rand::random();
                }
                let invite = Invite::generate();
                let mut code = LobbyCode::generate();
                while registry.codes.contains_key(&code) {
                    code = LobbyCode::generate();
                }
                let welcome = Control::Welcome {
                    room: id,
                    peer: HOST,
                    map: map.clone(),
                    invite: Some(invite.clone()),
                    code: Some(code),
                };
                registry.codes.insert(code, id);
                member.id.store(HOST, Ordering::Relaxed);
                registry
                    .rooms
                    .insert(id, Room { map, invite, code, next_peer: 1, members: HashMap::from([(HOST, member)]), listing: None });
                Ok(welcome)
            }
            Hello::Join { room, invite } => {
                let Some(r) = registry.rooms.get_mut(&room) else { bail!("join rejected") };
                // Rooms are unlisted and capabilities have 256 random bits.
                // Failure is deliberately indistinguishable from a missing room.
                ensure!(r.invite == invite && r.members.contains_key(&HOST), "join rejected");
                ensure!(r.members.len() < MAX_PLAYERS && r.next_peer < u16::MAX, "join rejected");
                let peer = r.next_peer;
                r.next_peer += 1;
                let map = r.map.clone();
                member.id.store(peer, Ordering::Relaxed);
                r.members.insert(peer, member);
                Ok(Control::Welcome { room, peer, map, invite: None, code: None })
            }
            Hello::JoinCode(_) | Hello::JoinListed { .. } | Hello::Browse => unreachable!(),
        }
    }

    /// Pass a game message on, stamped with its sender. A recipient too slow
    /// to keep up with its control queue is disconnected, not waited for.
    fn message(&self, room: u64, from: PeerId, to: PeerId, data: Vec<u8>) -> Result<()> {
        let registry = self.registry.lock().unwrap();
        let room = registry.rooms.get(&room).ok_or_else(|| anyhow::anyhow!("room closed"))?;
        let recipients: Vec<&Member> = if from == HOST {
            ensure!(to != HOST, "host cannot message itself");
            if to == EVERYONE {
                room.members.iter().filter(|(id, _)| **id != HOST).map(|(_, m)| m).collect()
            } else {
                // A player may have just left; drop it like a late snapshot.
                room.members.get(&to).into_iter().collect()
            }
        } else {
            ensure!(to == HOST, "players may only message the host");
            room.members.get(&HOST).into_iter().collect()
        };
        for member in recipients {
            if member.control.try_send(Control::Message { peer: from, data: data.clone() }).is_err() {
                member.connection.close(VarInt::from_u32(2), b"control consumer too slow");
            }
        }
        Ok(())
    }

    /// The host's match is about to start: the member nearest the relay
    /// (then the one losing fewest packets) becomes the host, if it's
    /// clearly nearer than the host. Everyone hears the result.
    fn elect(&self, room: u64) {
        let mut registry = self.registry.lock().unwrap();
        let Some(r) = registry.rooms.get_mut(&room) else { return };
        let candidates: Vec<HostCandidate> = r
            .members
            .iter()
            .map(|(id, m)| {
                let path = m.connection.stats().path;
                let penalty = if *id == HOST { self.host_penalty } else { Default::default() };
                HostCandidate {
                    peer: *id,
                    rtt: m.connection.rtt() + penalty,
                    loss_permille: path.lost_packets.saturating_mul(1000) / path.sent_packets.max(1),
                }
            })
            .collect();
        let swap = mm::choose_host(&candidates, ELECT_MARGIN);
        if let Some(new) = swap
            && let (Some(old_host), Some(new_host)) = (r.members.remove(&HOST), r.members.remove(&new))
        {
            old_host.id.store(new, Ordering::Relaxed);
            new_host.id.store(HOST, Ordering::Relaxed);
            r.members.insert(HOST, new_host);
            r.members.insert(new, old_host);
        }
        for m in r.members.values() {
            if m.control.try_send(Control::Elected { swap }).is_err() {
                m.connection.close(VarInt::from_u32(2), b"control consumer too slow");
            }
        }
    }

    fn publish(&self, room: u64, listing: Option<Listing>) {
        if let Some(r) = self.registry.lock().unwrap().rooms.get_mut(&room) {
            r.listing = listing.map(|l| (l, Instant::now()));
        }
    }

    /// The listed rooms matching `filter`, as `asker_rtt` away.
    fn lobbies(&self, filter: &Filter, asker_rtt: std::time::Duration) -> Vec<LobbyInfo> {
        let now = Instant::now();
        let registry = self.registry.lock().unwrap();
        registry.rooms.iter().filter_map(|(id, r)| r.info(*id, now, self.listing_ttl, asker_rtt)).filter(|l| filter.matches(l)).collect()
    }

    /// A browsing connection: questions about the public lobbies, answered
    /// on its stream, until it goes (or [`BROWSE_LIFETIME`]).
    async fn browse(&self, connection: Connection, mut send: quinn::SendStream, mut recv: quinn::RecvStream) -> Result<()> {
        let mut questions = RateLimit::new(BROWSE_RATE, BROWSE_BURST, Instant::now());
        let session = async {
            loop {
                let frame = tokio::select! {
                    f = transport::read_frame(&mut recv) => f?,
                    _ = connection.closed() => return Ok(()),
                };
                ensure!(questions.allow(1, Instant::now()), "browse rate limit exceeded");
                let (mut lobbies, total, quick) = match Control::decode(&frame)? {
                    Control::List(f) => {
                        let l = self.lobbies(&f, connection.rtt());
                        let n = l.len();
                        (l, n, false)
                    }
                    // `total`: the open lobbies at any ping, so a searcher
                    // knows whether loosening its ping cap could help.
                    Control::QuickMatch(f) => {
                        let open = self.lobbies(&Filter { open_only: true, max_ping: 0, ..f.clone() }, connection.rtt());
                        let n = open.len();
                        (open.into_iter().filter(|l| f.max_ping == 0 || l.ping <= f.max_ping).collect(), n, true)
                    }
                    _ => bail!("browsers may only ask for lobbies"),
                };
                let total = total.min(u16::MAX as usize) as u16;
                if quick {
                    mm::sort_for_quick_match(&mut lobbies);
                    lobbies.truncate(mm::QUICK_MATCH_CHOICES);
                } else {
                    mm::sort_for_list(&mut lobbies);
                    lobbies.truncate(mm::MAX_LOBBIES);
                }
                let mut frames: Vec<&[LobbyInfo]> = lobbies.chunks(mm::LOBBIES_PER_FRAME).collect();
                if frames.is_empty() {
                    frames.push(&[]);
                }
                let n = frames.len();
                for (i, chunk) in frames.into_iter().enumerate() {
                    let c = Control::Lobbies { total, last: i + 1 == n, lobbies: chunk.to_vec() };
                    timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut send, &c.encode()?)).await??;
                }
            }
        };
        let result: Result<()> = match timeout(BROWSE_LIFETIME, session).await {
            Ok(r) => r,
            Err(_) => Ok(()),
        };
        connection.close(VarInt::from_u32(0), b"browse ended");
        result
    }

    fn has_member(&self, room: u64, peer: PeerId) -> bool {
        self.registry.lock().unwrap().rooms.get(&room).is_some_and(|r| r.members.contains_key(&peer))
    }

    fn announce_join(&self, room: u64, peer: PeerId) -> Result<()> {
        if peer == HOST {
            return Ok(());
        }
        let registry = self.registry.lock().unwrap();
        let room = registry.rooms.get(&room).ok_or_else(|| anyhow::anyhow!("room closed"))?;
        let host = room.members.get(&HOST).ok_or_else(|| anyhow::anyhow!("host left"))?;
        if host.control.try_send(Control::PeerJoined(peer)).is_err() {
            host.connection.close(VarInt::from_u32(2), b"control consumer too slow");
            bail!("host control queue full");
        }
        Ok(())
    }

    fn forward(&self, room: u64, recipient: PeerId, packet: Packet) -> Result<()> {
        let connection = self
            .registry
            .lock()
            .unwrap()
            .rooms
            .get(&room)
            .and_then(|r| r.members.get(&recipient))
            .map(|m| m.connection.clone());
        if let Some(connection) = connection {
            // Realtime messages are replaceable. Congestion may drop older
            // datagrams; never turn snapshots into a reliable growing backlog.
            let _ = connection.send_datagram(Bytes::from(packet.encode()?));
        }
        Ok(())
    }

    fn leave(&self, room: u64, peer: PeerId) {
        let mut registry = self.registry.lock().unwrap();
        if peer == HOST {
            if let Some(room) = registry.rooms.remove(&room) {
                registry.codes.remove(&room.code);
                for m in room.members.values() {
                    m.connection.close(VarInt::from_u32(0), b"host left; room closed");
                }
            }
        } else if let Some(room) = registry.rooms.get_mut(&room)
            && room.members.remove(&peer).is_some()
            && let Some(host) = room.members.get(&HOST)
            && host.control.try_send(Control::PeerLeft(peer)).is_err()
        {
            host.connection.close(VarInt::from_u32(2), b"control consumer too slow");
        }
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Membership {
    relay: Relay,
    room: u64,
    /// Its current peer ID (an election may have changed it).
    id: Arc<AtomicU16>,
}
impl Drop for Membership {
    fn drop(&mut self) {
        self.relay.leave(self.room, self.id.load(Ordering::Relaxed));
    }
}
struct IpLease {
    relay: Relay,
    ip: IpAddr,
}
impl Drop for IpLease {
    fn drop(&mut self) {
        let mut r = self.relay.registry.lock().unwrap();
        if let Some(n) = r.by_ip.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                r.by_ip.remove(&self.ip);
            }
        }
    }
}
