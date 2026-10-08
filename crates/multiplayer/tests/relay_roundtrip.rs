use anyhow::Result;
use cod4rw_multiplayer::{client::Session, netcode::InputQueue, protocol::*, relay::Relay, transport};
use quinn::{Endpoint, VarInt};
use std::{net::SocketAddr, time::Duration};
use tokio::{task::JoinHandle, time::timeout};

struct Fixture {
    endpoint: Endpoint,
    address: SocketAddr,
    cert: Vec<u8>,
    task: JoinHandle<Result<()>>,
}
impl Fixture {
    async fn start() -> Self {
        let (cert, key) = transport::generate_identity().unwrap();
        let endpoint = transport::server_endpoint("127.0.0.1:0".parse().unwrap(), cert.clone(), key).unwrap();
        let address = endpoint.local_addr().unwrap();
        let task = tokio::spawn(Relay::new().run(endpoint.clone()));
        Self { endpoint, address, cert, task }
    }
    async fn host(&self) -> Session {
        Session::connect(self.address, self.cert.clone(), Hello::Create { map: "mp_crash".into() }).await.unwrap()
    }
    async fn join(&self, host: &Session) -> Session {
        Session::connect(
            self.address,
            self.cert.clone(),
            Hello::Join { room: host.room, invite: host.invite.clone().unwrap() },
        )
        .await
        .unwrap()
    }
    async fn stop(self) {
        self.endpoint.close(VarInt::from_u32(0), b"test end");
        timeout(Duration::from_secs(3), self.task).await.unwrap().unwrap().unwrap();
    }
}

#[tokio::test]
async fn encrypted_inputs_snapshots_and_disconnects() {
    let f = Fixture::start().await;
    let mut host = f.host().await;
    let player = f.join(&host).await;
    assert!(player.invite.is_none());
    assert_eq!(player.map, "mp_crash");
    assert_eq!(
        timeout(Duration::from_secs(2), host.next_control()).await.unwrap(),
        Some(Control::PeerJoined(player.peer))
    );
    player.send_inputs(vec![InputCommand { sequence: 1, forward: 127, ..Default::default() }]).unwrap();
    let received = timeout(Duration::from_secs(2), host.next_packet()).await.unwrap().unwrap();
    let Packet::RemoteInputs { peer, commands } = received else { panic!("expected input") };
    assert_eq!(peer, player.peer);
    let mut q = InputQueue::default();
    for command in commands {
        q.insert(command).unwrap();
    }
    assert_eq!(q.next_tick().forward, 127);
    let state = Snapshot {
        tick: 3,
        acknowledged_input: q.acknowledged(),
        pawns: vec![PawnState { id: peer, health: 100, ..Default::default() }],
    };
    host.send_snapshot(peer, state.clone()).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(2), player.next_packet()).await.unwrap().unwrap(),
        Packet::Snapshot { recipient: peer, state }
    );
    // Duplicate commands aren't forwarded as new actions.
    player.send_inputs(vec![InputCommand { sequence: 1, ..Default::default() }]).unwrap();
    assert!(timeout(Duration::from_millis(100), host.next_packet()).await.is_err());
    let id = player.peer;
    drop(player);
    assert_eq!(timeout(Duration::from_secs(2), host.next_control()).await.unwrap(), Some(Control::PeerLeft(id)));
    // An in-flight snapshot for someone who just left cannot kill the host.
    host.send_snapshot(id, Snapshot { tick: 6, acknowledged_input: 1, pawns: vec![] }).unwrap();
    let next = f.join(&host).await;
    assert_ne!(next.peer, id);
    drop(next);
    drop(host);
    f.stop().await;
}

#[tokio::test]
async fn invalid_invite_untrusted_certificate_and_room_isolation() {
    let f = Fixture::start().await;
    let host = f.host().await;
    assert!(
        Session::connect(f.address, f.cert.clone(), Hello::Join { room: host.room, invite: Invite::generate() })
            .await
            .is_err()
    );
    let (wrong_cert, _) = transport::generate_identity().unwrap();
    assert!(Session::connect(f.address, wrong_cert, Hello::Create { map: "mp_crash".into() }).await.is_err());
    let other_host = f.host().await;
    let player = f.join(&host).await;
    player.send_inputs(vec![InputCommand { sequence: 1, ..Default::default() }]).unwrap();
    assert!(timeout(Duration::from_secs(2), host.next_packet()).await.unwrap().is_ok());
    assert!(timeout(Duration::from_millis(100), other_host.next_packet()).await.is_err());
    drop(player);
    drop(host);
    drop(other_host);
    f.stop().await;
}

#[tokio::test]
async fn malicious_player_cannot_send_snapshots_or_spoof_another_player() {
    let f = Fixture::start().await;
    let mut host = f.host().await;
    // Use the raw transport to bypass Session's client-side role checks.
    let endpoint = transport::client_endpoint("127.0.0.1:0".parse().unwrap(), f.cert.clone()).unwrap();
    let connection = endpoint.connect(f.address, transport::SERVER_NAME).unwrap().await.unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    transport::write_frame(
        &mut send,
        &Hello::Join { room: host.room, invite: host.invite.clone().unwrap() }.encode().unwrap(),
    )
    .await
    .unwrap();
    let welcome = Control::decode(&transport::read_frame(&mut recv).await.unwrap()).unwrap();
    let Control::Welcome { peer, .. } = welcome else { panic!() };
    assert_eq!(timeout(Duration::from_secs(2), host.next_control()).await.unwrap(), Some(Control::PeerJoined(peer)));
    connection
        .send_datagram(
            Packet::RemoteInputs { peer: 99, commands: vec![InputCommand { sequence: 1, ..Default::default() }] }
                .encode()
                .unwrap()
                .into(),
        )
        .unwrap();
    timeout(Duration::from_secs(2), connection.closed()).await.unwrap();
    assert!(timeout(Duration::from_millis(100), host.next_packet()).await.is_err());
    // A second malicious connection tries to broadcast authoritative state.
    let connection = endpoint.connect(f.address, transport::SERVER_NAME).unwrap().await.unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    transport::write_frame(
        &mut send,
        &Hello::Join { room: host.room, invite: host.invite.clone().unwrap() }.encode().unwrap(),
    )
    .await
    .unwrap();
    transport::read_frame(&mut recv).await.unwrap();
    connection
        .send_datagram(
            Packet::Snapshot { recipient: HOST, state: Snapshot { tick: 1, acknowledged_input: 0, pawns: vec![] } }
                .encode()
                .unwrap()
                .into(),
        )
        .unwrap();
    timeout(Duration::from_secs(2), connection.closed()).await.unwrap();
    assert!(!host.is_closed());
    endpoint.close(VarInt::from_u32(0), b"done");
    drop(host);
    f.stop().await;
}

#[tokio::test]
async fn host_departure_closes_the_room_and_rejects_old_invite() {
    let f = Fixture::start().await;
    let host = f.host().await;
    let player = f.join(&host).await;
    let hello = Hello::Join { room: host.room, invite: host.invite.clone().unwrap() };
    drop(host);
    assert!(timeout(Duration::from_secs(2), player.next_packet()).await.unwrap().is_err());
    assert!(Session::connect(f.address, f.cert.clone(), hello).await.is_err());
    drop(player);
    f.stop().await;
}

#[tokio::test]
async fn malformed_and_oversized_frames_close_the_connection() {
    let f = Fixture::start().await;
    let endpoint = transport::client_endpoint("127.0.0.1:0".parse().unwrap(), f.cert.clone()).unwrap();
    let connection = endpoint.connect(f.address, transport::SERVER_NAME).unwrap().await.unwrap();
    let (mut send, _recv) = connection.open_bi().await.unwrap();
    // Declared allocation is rejected before waiting for any body bytes.
    send.write_all(&u16::MAX.to_le_bytes()).await.unwrap();
    timeout(Duration::from_secs(2), connection.closed()).await.unwrap();
    let host = f.host().await;
    let connection = endpoint.connect(f.address, transport::SERVER_NAME).unwrap().await.unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    transport::write_frame(
        &mut send,
        &Hello::Join { room: host.room, invite: host.invite.clone().unwrap() }.encode().unwrap(),
    )
    .await
    .unwrap();
    transport::read_frame(&mut recv).await.unwrap();
    connection.send_datagram(vec![0u8; MAX_PACKET + 1].into()).unwrap();
    timeout(Duration::from_secs(2), connection.closed()).await.unwrap();
    endpoint.close(VarInt::from_u32(0), b"done");
    drop(host);
    f.stop().await;
}

#[tokio::test]
async fn packet_flood_disconnects_only_the_abusive_player() {
    let f = Fixture::start().await;
    let host = f.host().await;
    let endpoint = transport::client_endpoint("127.0.0.1:0".parse().unwrap(), f.cert.clone()).unwrap();
    let connection = endpoint.connect(f.address, transport::SERVER_NAME).unwrap().await.unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    transport::write_frame(
        &mut send,
        &Hello::Join { room: host.room, invite: host.invite.clone().unwrap() }.encode().unwrap(),
    )
    .await
    .unwrap();
    transport::read_frame(&mut recv).await.unwrap();
    // Repeated valid-size duplicates still count against the packet budget,
    // even though replay filtering would reject their gameplay commands.
    let packet: bytes::Bytes =
        Packet::Inputs(vec![InputCommand { sequence: 1, ..Default::default() }]).encode().unwrap().into();
    for _ in 0..2000 {
        let _ = connection.send_datagram(packet.clone());
    }
    timeout(Duration::from_secs(2), connection.closed()).await.unwrap();
    assert!(!host.is_closed());
    endpoint.close(VarInt::from_u32(0), b"done");
    drop(host);
    f.stop().await;
}

#[tokio::test]
async fn per_ip_connection_cap_rejects_excess_sessions() {
    let f = Fixture::start().await;
    let mut host = f.host().await;
    let mut players = Vec::new();
    for _ in 1..cod4rw_multiplayer::security::MAX_PER_IP {
        let player = f.join(&host).await;
        assert_eq!(
            timeout(Duration::from_secs(2), host.next_control()).await.unwrap(),
            Some(Control::PeerJoined(player.peer))
        );
        players.push(player);
    }
    assert!(
        Session::connect(
            f.address,
            f.cert.clone(),
            Hello::Join { room: host.room, invite: host.invite.clone().unwrap() }
        )
        .await
        .is_err()
    );
    let departed = players.pop().unwrap();
    let id = departed.peer;
    drop(departed);
    assert_eq!(timeout(Duration::from_secs(2), host.next_control()).await.unwrap(), Some(Control::PeerLeft(id)));
    let replacement = f.join(&host).await;
    assert_ne!(replacement.peer, id);
    drop(replacement);
    drop(players);
    drop(host);
    f.stop().await;
}

#[tokio::test]
async fn code_join_and_lobby_messages() {
    let f = Fixture::start().await;
    let mut host = f.host().await;
    let code = host.code.unwrap();
    let mut a = Session::connect(f.address, f.cert.clone(), Hello::JoinCode(code)).await.unwrap();
    let mut b = Session::connect(f.address, f.cert.clone(), Hello::JoinCode(code)).await.unwrap();
    assert!(a.code.is_none() && a.invite.is_none() && a.room == host.room);
    for _ in 0..2 {
        assert!(matches!(timeout(Duration::from_secs(2), host.next_control()).await.unwrap(), Some(Control::PeerJoined(_))));
    }
    // Player -> host, stamped with the real sender.
    a.send_message(HOST, b"hello".to_vec()).await.unwrap();
    assert_eq!(
        timeout(Duration::from_secs(2), host.next_control()).await.unwrap(),
        Some(Control::Message { peer: a.peer, data: b"hello".to_vec() })
    );
    // Host -> everyone.
    host.send_message(EVERYONE, b"roster".to_vec()).await.unwrap();
    for s in [&mut a, &mut b] {
        assert_eq!(
            timeout(Duration::from_secs(2), s.next_control()).await.unwrap(),
            Some(Control::Message { peer: HOST, data: b"roster".to_vec() })
        );
    }
    // Host -> one player only.
    host.send_message(b.peer, b"just b".to_vec()).await.unwrap();
    assert_eq!(
        timeout(Duration::from_secs(2), b.next_control()).await.unwrap(),
        Some(Control::Message { peer: HOST, data: b"just b".to_vec() })
    );
    assert!(timeout(Duration::from_millis(150), a.next_control()).await.is_err());
    // Players can't message each other.
    assert!(a.send_message(b.peer, b"x".to_vec()).await.is_err());
    // The code dies with the room.
    drop(a);
    drop(b);
    drop(host);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(Session::connect(f.address, f.cert.clone(), Hello::JoinCode(code)).await.is_err());
    f.stop().await;
}

#[tokio::test]
async fn player_cannot_message_other_players_through_raw_frames() {
    let f = Fixture::start().await;
    let mut host = f.host().await;
    let endpoint = transport::client_endpoint("127.0.0.1:0".parse().unwrap(), f.cert.clone()).unwrap();
    let connection = endpoint.connect(f.address, transport::SERVER_NAME).unwrap().await.unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    transport::write_frame(&mut send, &Hello::JoinCode(host.code.unwrap()).encode().unwrap()).await.unwrap();
    transport::read_frame(&mut recv).await.unwrap();
    assert!(matches!(timeout(Duration::from_secs(2), host.next_control()).await.unwrap(), Some(Control::PeerJoined(_))));
    let spoof = Control::Message { peer: EVERYONE, data: b"fake roster".to_vec() }.encode().unwrap();
    transport::write_frame(&mut send, &spoof).await.unwrap();
    timeout(Duration::from_secs(2), connection.closed()).await.unwrap();
    assert!(!host.is_closed());
    endpoint.close(VarInt::from_u32(0), b"done");
    drop(host);
    f.stop().await;
}

#[tokio::test]
async fn guessing_codes_is_rate_limited() {
    let f = Fixture::start().await;
    let host = f.host().await;
    for _ in 0..10 {
        assert!(Session::connect(f.address, f.cert.clone(), Hello::JoinCode(LobbyCode::generate())).await.is_err());
    }
    // Locked out for a while, even with the right code.
    assert!(Session::connect(f.address, f.cert.clone(), Hello::JoinCode(host.code.unwrap())).await.is_err());
    drop(host);
    f.stop().await;
}
