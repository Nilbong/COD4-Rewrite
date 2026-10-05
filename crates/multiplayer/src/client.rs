//! Both match hosts and players make outbound connections to the relay.
//! There is intentionally no direct peer address API or P2P fallback.

use crate::{protocol::*, security::HANDSHAKE_TIMEOUT, transport};
use anyhow::{Result, ensure};
use bytes::Bytes;
use quinn::{Connection, Endpoint, SendStream, VarInt};
use std::net::SocketAddr;
use tokio::{sync::mpsc, task::JoinHandle, time::timeout};

pub struct Session {
    endpoint: Endpoint,
    connection: Connection,
    // Keep the initiating stream open for the entire session.
    _control_send: SendStream,
    control_rx: mpsc::Receiver<Control>,
    control_task: JoinHandle<()>,
    pub room: u64,
    pub peer: PeerId,
    pub map: String,
    pub invite: Option<Invite>,
}

impl Session {
    pub async fn connect(relay: SocketAddr, trusted_cert: Vec<u8>, hello: Hello) -> Result<Self> {
        let bind = if relay.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }.parse()?;
        let endpoint = transport::client_endpoint(bind, trusted_cert)?;
        let connection = timeout(HANDSHAKE_TIMEOUT, endpoint.connect(relay, transport::SERVER_NAME)?).await??;
        let (mut send, mut recv) = timeout(HANDSHAKE_TIMEOUT, connection.open_bi()).await??;
        timeout(HANDSHAKE_TIMEOUT, transport::write_frame(&mut send, &hello.encode()?)).await??;
        let welcome = Control::decode(&timeout(HANDSHAKE_TIMEOUT, transport::read_frame(&mut recv)).await??)?;
        let Control::Welcome { room, peer, map, invite } = welcome else { anyhow::bail!("relay did not send welcome") };
        match &hello {
            Hello::Create { map: requested } => {
                ensure!(peer == HOST && invite.is_some() && map == *requested, "invalid host welcome")
            }
            Hello::Join { room: requested, .. } => {
                ensure!(room == *requested && peer != HOST && invite.is_none(), "invalid player welcome")
            }
        }
        let (tx, control_rx) = mpsc::channel(crate::security::CONTROL_QUEUE);
        let conn = connection.clone();
        let control_task = tokio::spawn(async move {
            // One persistent reader: selecting next_control alongside packet
            // reads never cancels a partially read length-prefixed frame.
            while let Ok(data) = transport::read_frame(&mut recv).await {
                let Ok(control) = Control::decode(&data) else { break };
                if matches!(control, Control::Welcome { .. }) || peer != HOST || tx.try_send(control).is_err() {
                    break;
                }
            }
            conn.close(VarInt::from_u32(1), b"control channel closed or invalid");
        });
        Ok(Self { endpoint, connection, _control_send: send, control_rx, control_task, room, peer, map, invite })
    }

    pub fn send_inputs(&self, commands: Vec<InputCommand>) -> Result<()> {
        ensure!(self.peer != HOST, "host applies its own local inputs directly");
        self.send(Packet::Inputs(commands))
    }
    pub fn send_snapshot(&self, recipient: PeerId, state: Snapshot) -> Result<()> {
        ensure!(self.peer == HOST && recipient != HOST, "only host sends snapshots to players");
        self.send(Packet::Snapshot { recipient, state })
    }
    fn send(&self, packet: Packet) -> Result<()> {
        self.connection.send_datagram(Bytes::from(packet.encode()?))?;
        Ok(())
    }
    pub async fn next_packet(&self) -> Result<Packet> {
        let data = self.connection.read_datagram().await?;
        let packet = Packet::decode(&data)?;
        match &packet {
            Packet::RemoteInputs { peer, .. } => {
                ensure!(self.peer == HOST && *peer != HOST, "unexpected input delivery")
            }
            Packet::Snapshot { recipient, .. } => {
                ensure!(self.peer != HOST && *recipient == self.peer, "unexpected snapshot delivery")
            }
            _ => anyhow::bail!("unexpected relay packet"),
        }
        Ok(packet)
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
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
        self.control_task.abort();
        self.endpoint.close(VarInt::from_u32(0), b"session dropped");
    }
}
