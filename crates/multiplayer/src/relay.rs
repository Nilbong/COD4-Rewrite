//! A trusted relay forwards validated messages and stamps sender identities.
//! It never forwards connection addresses or lets a client impersonate a host.

use crate::{protocol::*, security::*, transport};
use anyhow::{Result, bail, ensure};
use bytes::Bytes;
use quinn::{Connection, Endpoint, VarInt};
use std::{
    collections::HashMap,
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
}
struct Room {
    map: String,
    invite: Invite,
    code: LobbyCode,
    next_peer: PeerId,
    members: HashMap<PeerId, Member>,
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

/// Failed joins allowed per address per [`FAILURE_WINDOW`].
const MAX_FAILURES: u32 = 10;
const FAILURE_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// Owns only local infrastructure state. QUIC remote addresses never enter
/// protocol messages, room advertisements, or client event queues.
#[derive(Clone, Default)]
pub struct Relay {
    registry: Arc<Mutex<Registry>>,
}

impl Relay {
    pub fn new() -> Self {
        Self::default()
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
        let (control_tx, mut control_rx) = mpsc::channel(CONTROL_QUEUE);
        let welcome = self.admit(hello, Member { connection: connection.clone(), control: control_tx }, ip)?;
        let Control::Welcome { room, peer, .. } = &welcome else { unreachable!() };
        let (room, peer) = (*room, *peer);
        let lease = Membership { relay: self.clone(), room, peer };
        // This is the only server-side control writer, preserving order.
        timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut send, &welcome.encode()?)).await??;
        self.announce_join(room, peer)?;

        let now = Instant::now();
        let host = peer == HOST;
        let mut packets = RateLimit::new(if host { 400.0 } else { 90.0 }, if host { 800.0 } else { 180.0 }, now);
        let mut bytes =
            RateLimit::new(if host { 512_000.0 } else { 12_000.0 }, if host { 1_024_000.0 } else { 24_000.0 }, now);
        let mut replay = ReplayWindow::default();
        let mut snapshot_ticks: HashMap<PeerId, u32> = HashMap::new();
        let mut message_count = RateLimit::new(20.0, 60.0, now);
        let mut message_bytes = RateLimit::new(16_000.0, 32_000.0, now);
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
                    let Control::Message { peer: to, data } = Control::decode(&frame)? else {
                        bail!("only messages are accepted after joining")
                    };
                    self.message(room, peer, to, data)?;
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
        let result = Self::admit_locked(&mut registry, hello, member);
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

    fn admit_locked(registry: &mut Registry, hello: Hello, member: Member) -> Result<Control> {
        let hello = match hello {
            Hello::JoinCode(code) => {
                let Some(&room) = registry.codes.get(&code) else { bail!("join rejected") };
                let invite = registry.rooms.get(&room).map(|r| r.invite.clone());
                let Some(invite) = invite else { bail!("join rejected") };
                Hello::Join { room, invite }
            }
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
                registry
                    .rooms
                    .insert(id, Room { map, invite, code, next_peer: 1, members: HashMap::from([(HOST, member)]) });
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
                r.members.insert(peer, member);
                Ok(Control::Welcome { room, peer, map, invite: None, code: None })
            }
            Hello::JoinCode(_) => unreachable!(),
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
    peer: PeerId,
}
impl Drop for Membership {
    fn drop(&mut self) {
        self.relay.leave(self.room, self.peer);
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
