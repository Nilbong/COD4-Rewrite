//! Both match hosts and players make outbound connections to the relay.
//! There is intentionally no direct peer address API or P2P fallback.

use crate::matchmaking::{Filter, LobbyInfo, Listing};
use crate::{protocol::*, security::HANDSHAKE_TIMEOUT, transport};
use anyhow::{Result, ensure};
use bytes::Bytes;
use quinn::{Connection, Endpoint, RecvStream, SendStream, VarInt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use tokio::{sync::mpsc, task::JoinHandle, time::timeout};

pub struct Session {
    /// This side's peer ID now (an election may change it; [`Session::peer`]
    /// is the one it joined with).
    id: Arc<AtomicU16>,
    endpoint: Endpoint,
    connection: Connection,
    // Kept open for the entire session; carries this side's messages.
    control_send: SendStream,
    control_rx: mpsc::Receiver<Control>,
    control_task: JoinHandle<()>,
    pub room: u64,
    pub peer: PeerId,
    pub map: String,
    pub invite: Option<Invite>,
    /// The short code to give friends (host only).
    pub code: Option<LobbyCode>,
}

impl Session {
    pub async fn connect(relay: SocketAddr, trusted_cert: Vec<u8>, hello: Hello) -> Result<Self> {
        let bind = if relay.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }.parse()?;
        let endpoint = transport::client_endpoint(bind, trusted_cert)?;
        let connection = timeout(HANDSHAKE_TIMEOUT, endpoint.connect(relay, transport::SERVER_NAME)?).await??;
        let (mut send, mut recv) = timeout(HANDSHAKE_TIMEOUT, connection.open_bi()).await??;
        timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut send, &hello.encode()?)).await??;
        let welcome = Control::decode(&timeout(HANDSHAKE_TIMEOUT, transport::read_frame(&mut recv)).await??)?;
        let Control::Welcome { room, peer, map, invite, code } = welcome else {
            anyhow::bail!("relay did not send welcome")
        };
        match &hello {
            Hello::Create { map: requested } => {
                ensure!(peer == HOST && invite.is_some() && code.is_some() && map == *requested, "invalid host welcome")
            }
            Hello::Join { room: requested, .. } => {
                ensure!(room == *requested && peer != HOST && invite.is_none() && code.is_none(), "invalid player welcome")
            }
            Hello::JoinCode(_) => ensure!(peer != HOST && invite.is_none() && code.is_none(), "invalid player welcome"),
            Hello::JoinListed { room: requested } => {
                ensure!(room == *requested && peer != HOST && invite.is_none() && code.is_none(), "invalid player welcome")
            }
            Hello::Browse => anyhow::bail!("browsing is a Browser, not a Session"),
        }
        let (tx, control_rx) = mpsc::channel(crate::security::CONTROL_QUEUE);
        let conn = connection.clone();
        let id = Arc::new(AtomicU16::new(peer));
        let me = id.clone();
        let control_task = tokio::spawn(async move {
            // One persistent reader: selecting next_control alongside packet
            // reads never cancels a partially read length-prefixed frame.
            while let Ok(data) = transport::read_frame(&mut recv).await {
                let Ok(control) = Control::decode(&data) else { break };
                // Players only ever hear from the host, through the relay.
                let peer = me.load(Ordering::Relaxed);
                let allowed = match &control {
                    Control::Welcome { .. } | Control::Publish(_) | Control::List(_) | Control::QuickMatch(_) | Control::Lobbies { .. } | Control::Elect => false,
                    Control::Elected { .. } => true,
                    Control::PeerJoined(_) | Control::PeerLeft(_) => peer == HOST,
                    Control::Message { peer: from, .. } => (peer == HOST) != (*from == HOST),
                };
                if !allowed {
                    break;
                }
                // A new host: it and the old one swapped IDs.
                if let Control::Elected { swap: Some(new) } = control {
                    if peer == HOST {
                        me.store(new, Ordering::Relaxed);
                    } else if peer == new {
                        me.store(HOST, Ordering::Relaxed);
                    }
                }
                if tx.try_send(control).is_err() {
                    break;
                }
            }
            conn.close(VarInt::from_u32(1), b"control channel closed or invalid");
        });
        Ok(Self { id, endpoint, connection, control_send: send, control_rx, control_task, room, peer, map, invite, code })
    }

    /// A reliable message for the game: a player to the host, or the host to
    /// one player or [`EVERYONE`].
    pub async fn send_message(&mut self, to: PeerId, data: Vec<u8>) -> Result<()> {
        ensure!((self.current_peer() == HOST) != (to == HOST), "players message the host; the host messages players");
        let frame = Control::Message { peer: to, data }.encode()?;
        timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut self.control_send, &frame)).await??;
        Ok(())
    }

    pub fn send_inputs(&self, commands: Vec<InputCommand>) -> Result<()> {
        ensure!(self.current_peer() != HOST, "host applies its own local inputs directly");
        self.send(Packet::Inputs(commands))
    }
    pub fn send_snapshot(&self, recipient: PeerId, state: Snapshot) -> Result<()> {
        ensure!(self.current_peer() == HOST && recipient != HOST, "only host sends snapshots to players");
        self.send(Packet::Snapshot { recipient, state })
    }
    fn send(&self, packet: Packet) -> Result<()> {
        self.connection.send_datagram(Bytes::from(packet.encode()?))?;
        Ok(())
    }
    pub async fn next_packet(&self) -> Result<Packet> {
        self.packets().next().await
    }
    /// Reads this session's datagrams on its own, so they can be awaited
    /// alongside [`Session::next_control`].
    pub fn packets(&self) -> Packets {
        Packets { connection: self.connection.clone(), id: self.id.clone() }
    }
    pub async fn next_control(&mut self) -> Option<Control> {
        self.control_rx.recv().await
    }
    pub fn try_control(&mut self) -> Option<Control> {
        self.control_rx.try_recv().ok()
    }
    /// Round-trip to the relay, not end-to-end latency to the match host.
    pub fn relay_rtt(&self) -> std::time::Duration {
        self.connection.rtt()
    }
    pub fn is_closed(&self) -> bool {
        self.connection.close_reason().is_some()
    }
    pub fn close(&self) {
        self.connection.close(VarInt::from_u32(0), b"left match");
    }
    /// This side's peer ID now: [`HOST`] after being elected the host.
    pub fn current_peer(&self) -> PeerId {
        self.id.load(Ordering::Relaxed)
    }
    /// Host: the match is about to start; let the relay choose its host
    /// (answered with [`Control::Elected`] to everyone).
    pub async fn elect(&mut self) -> Result<()> {
        ensure!(self.current_peer() == HOST, "only the host calls an election");
        let frame = Control::Elect.encode()?;
        timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut self.control_send, &frame)).await??;
        Ok(())
    }
    /// Host: list this lobby publicly, or (`None`) take it off the list.
    pub async fn publish(&mut self, listing: Option<Listing>) -> Result<()> {
        ensure!(self.current_peer() == HOST, "only the host lists a lobby");
        let frame = Control::Publish(listing).encode()?;
        timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut self.control_send, &frame)).await??;
        Ok(())
    }
}

/// A look at a relay's public lobbies, without joining one. Its round
/// trip is the relay's distance, for choosing between relays.
pub struct Browser {
    endpoint: Endpoint,
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
}

impl Browser {
    pub async fn connect(relay: SocketAddr, trusted_cert: Vec<u8>) -> Result<Self> {
        let bind = if relay.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }.parse()?;
        let endpoint = transport::client_endpoint(bind, trusted_cert)?;
        let connection = timeout(HANDSHAKE_TIMEOUT, endpoint.connect(relay, transport::SERVER_NAME)?).await??;
        let (mut send, recv) = timeout(HANDSHAKE_TIMEOUT, connection.open_bi()).await??;
        timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut send, &Hello::Browse.encode()?)).await??;
        Ok(Self { endpoint, connection, send, recv })
    }
    /// The lobbies matching `filter`, best first, and how many matched.
    pub async fn list(&mut self, filter: Filter) -> Result<(u16, Vec<LobbyInfo>)> {
        self.ask(Control::List(filter)).await
    }
    /// The few best open lobbies to join now, best first.
    pub async fn quick_match(&mut self, filter: Filter) -> Result<(u16, Vec<LobbyInfo>)> {
        self.ask(Control::QuickMatch(filter)).await
    }
    async fn ask(&mut self, question: Control) -> Result<(u16, Vec<LobbyInfo>)> {
        timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut self.send, &question.encode()?)).await??;
        let mut out = Vec::new();
        loop {
            let frame = timeout(HANDSHAKE_TIMEOUT, transport::read_frame(&mut self.recv)).await??;
            let Control::Lobbies { total, last, lobbies } = Control::decode(&frame)? else {
                anyhow::bail!("relay sent something other than lobbies")
            };
            ensure!(out.len() + lobbies.len() <= crate::matchmaking::MAX_LOBBIES, "relay sent too many lobbies");
            out.extend(lobbies);
            if last {
                return Ok((total, out));
            }
        }
    }
    /// Round trip to the relay.
    pub fn rtt(&self) -> std::time::Duration {
        self.connection.rtt()
    }
    pub fn is_closed(&self) -> bool {
        self.connection.close_reason().is_some()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.connection.close(VarInt::from_u32(0), b"done browsing");
        self.endpoint.close(VarInt::from_u32(0), b"done browsing");
    }
}

/// A session's incoming datagrams, checked for its role.
pub struct Packets {
    connection: Connection,
    id: Arc<AtomicU16>,
}

impl Packets {
    pub async fn next(&self) -> Result<Packet> {
        let data = self.connection.read_datagram().await?;
        let packet = Packet::decode(&data)?;
        let peer = self.id.load(Ordering::Relaxed);
        match &packet {
            Packet::RemoteInputs { peer: from, .. } => {
                ensure!(peer == HOST && *from != HOST, "unexpected input delivery")
            }
            Packet::Snapshot { recipient, .. } => {
                ensure!(peer != HOST && *recipient == peer, "unexpected snapshot delivery")
            }
            _ => anyhow::bail!("unexpected relay packet"),
        }
        Ok(packet)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
        self.control_task.abort();
        self.endpoint.close(VarInt::from_u32(0), b"session dropped");
    }
}
