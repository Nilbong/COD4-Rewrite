//! A real QUIC roundtrip with a tiny movement simulation, independent of
//! game data and graphics. `host`/`join` also work in separate processes.

use anyhow::{Result, bail, ensure};
use cod4rw_multiplayer::{
    client::Session,
    netcode::{InputQueue, Prediction, STEP},
    protocol::*,
    relay::Relay,
    transport,
};
use std::{collections::HashMap, net::SocketAddr, time::Duration};
use tokio::time::{MissedTickBehavior, interval, timeout};

fn simulate(position: &mut f32, c: InputCommand, dt: f32) {
    *position += c.forward as f32 / 127.0 * 5.0 * dt;
}
fn snapshot(tick: u32, peer: PeerId, x: f32, ack: u32) -> Snapshot {
    Snapshot {
        tick,
        acknowledged_input: ack,
        pawns: vec![PawnState { id: peer, position: [x, 0.0, 0.0], health: 100, ..Default::default() }],
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("loopback") => loopback().await,
        Some("host") => {
            ensure!(args.len() == 3, "usage: net-demo host <relay-address:port> <relay.der>");
            let mut host = Session::connect(
                args[1].parse()?,
                std::fs::read(&args[2])?,
                Hello::Create { map: "mp_killhouse".into() },
            )
            .await?;
            // Deliberate user-visible development invitation, never relay log.
            println!("Room: {}\nInvite (share privately): {}", host.room, host.invite.as_ref().unwrap().to_hex());
            let mut timer = interval(Duration::from_secs_f64(1.0 / TICK_RATE as f64));
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut peers: HashMap<PeerId, (InputQueue, f32)> = HashMap::new();
            let mut tick = 0u32;
            loop {
                tokio::select! {
                    _ = timer.tick() => {
                        while let Some(control) = host.try_control() {
                            match control {
                                Control::PeerJoined(peer) => { peers.insert(peer, (InputQueue::default(), 0.0)); println!("Player {peer} joined"); }
                                Control::PeerLeft(peer) => { peers.remove(&peer); println!("Player {peer} left"); }
                                _ => {}
                            }
                        }
                        if host.is_closed() { bail!("relay closed the room"); }
                        tick = tick.wrapping_add(1);
                        for (&peer, (q, x)) in &mut peers {
                            simulate(x, q.next_tick(), STEP);
                            if tick.is_multiple_of(TICK_RATE / SNAPSHOT_RATE) { host.send_snapshot(peer, snapshot(tick, peer, *x, q.acknowledged()))?; }
                        }
                    }
                    packet = host.next_packet() => {
                        if let Packet::RemoteInputs { peer, commands } = packet? {
                            // Inputs may beat a reliable join event. Drain it
                            // before looking up the remote input queue.
                            while let Some(control) = host.try_control() {
                                match control {
                                    Control::PeerJoined(id) => { peers.entry(id).or_default(); }
                                    Control::PeerLeft(id) => { peers.remove(&id); }
                                    _ => {}
                                }
                            }
                            if let Some((q, _)) = peers.get_mut(&peer) { for c in commands { q.insert(c)?; } }
                        }
                    }
                }
            }
        }
        Some("join") => {
            ensure!(args.len() == 5, "usage: net-demo join <relay-address:port> <relay.der> <room-id> <invite>");
            let player = Session::connect(
                args[1].parse()?,
                std::fs::read(&args[2])?,
                Hello::Join { room: args[3].parse()?, invite: Invite::from_hex(&args[4])? },
            )
            .await?;
            println!("Joined as player {}; relay RTT {:?}", player.peer, player.relay_rtt());
            let mut timer = interval(Duration::from_secs_f64(1.0 / TICK_RATE as f64));
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut prediction = Prediction::new(0.0);
            let mut latest = None;
            loop {
                tokio::select! {
                    _ = timer.tick() => {
                        prediction.advance(InputCommand { forward: 127, ..Default::default() }, simulate)?;
                        player.send_inputs(prediction.redundant_inputs())?;
                    }
                    packet = player.next_packet() => {
                        if let Packet::Snapshot { state, .. } = packet? {
                            if latest.is_some_and(|tick| !cod4rw_multiplayer::security::newer(state.tick, tick)) { continue; }
                            latest = Some(state.tick);
                            if let Some(pawn) = state.pawns.iter().find(|p| p.id == player.peer) {
                                prediction.reconcile(pawn.position[0], state.acknowledged_input, simulate)?;
                                if state.tick % 60 == 0 { println!("tick {}: host x={:.2}, predicted x={:.2}, ack {}", state.tick, pawn.position[0], prediction.state, state.acknowledged_input); }
                            }
                        }
                    }
                }
            }
        }
        _ => bail!(
            "usage: net-demo loopback | host <relay-address:port> <relay.der> | join <relay-address:port> <relay.der> <room-id> <invite>"
        ),
    }
}

async fn loopback() -> Result<()> {
    let (cert, key) = transport::generate_identity()?;
    let endpoint = transport::server_endpoint("127.0.0.1:0".parse::<SocketAddr>()?, cert.clone(), key)?;
    let address = endpoint.local_addr()?;
    let shutdown = endpoint.clone();
    let relay = tokio::spawn(Relay::new().run(endpoint));
    let mut host = Session::connect(address, cert.clone(), Hello::Create { map: "mp_killhouse".into() }).await?;
    let player =
        Session::connect(address, cert, Hello::Join { room: host.room, invite: host.invite.clone().unwrap() }).await?;
    ensure!(
        timeout(Duration::from_secs(2), host.next_control()).await? == Some(Control::PeerJoined(player.peer)),
        "missing join event"
    );
    let mut queue = InputQueue::default();
    let mut authoritative = 0.0;
    let mut prediction = Prediction::new(0.0);
    for tick in 1..=120 {
        prediction.advance(InputCommand { forward: 127, ..Default::default() }, simulate)?;
        player.send_inputs(prediction.redundant_inputs())?;
        let Packet::RemoteInputs { peer, commands } = timeout(Duration::from_secs(2), host.next_packet()).await??
        else {
            bail!("missing input")
        };
        ensure!(peer == player.peer, "sender identity changed");
        for c in commands {
            queue.insert(c)?;
        }
        simulate(&mut authoritative, queue.next_tick(), STEP);
        if tick % 3 == 0 {
            host.send_snapshot(peer, snapshot(tick, peer, authoritative, queue.acknowledged()))?;
            let Packet::Snapshot { state, .. } = timeout(Duration::from_secs(2), player.next_packet()).await?? else {
                bail!("missing snapshot")
            };
            prediction.reconcile(state.pawns[0].position[0], state.acknowledged_input, simulate)?;
        }
        tokio::time::sleep(Duration::from_millis(17)).await;
    }
    ensure!((prediction.state - authoritative).abs() < 0.001 && authoritative > 9.0, "simulation diverged");
    println!(
        "PASS: 120 player inputs through encrypted relay; 40 authoritative snapshots; prediction reconciled at x={authoritative:.2}. No direct peer sockets."
    );
    drop(player);
    drop(host);
    shutdown.close(quinn::VarInt::from_u32(0), b"demo complete");
    timeout(Duration::from_secs(3), relay).await???;
    Ok(())
}
