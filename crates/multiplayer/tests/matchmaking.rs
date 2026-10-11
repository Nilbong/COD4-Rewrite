//! Public lobbies through a real relay: listing, filters, quick match,
//! joining from the list, and listings expiring.

use cod4rw_multiplayer::client::{Browser, Session};
use cod4rw_multiplayer::matchmaking::{Filter, LobbyState, Listing};
use cod4rw_multiplayer::protocol::*;
use cod4rw_multiplayer::{relay::Relay, transport};
use quinn::{Endpoint, VarInt};
use std::{net::SocketAddr, time::Duration};
use tokio::time::{sleep, timeout};

struct Fixture {
    endpoint: Endpoint,
    address: SocketAddr,
    cert: Vec<u8>,
}
impl Fixture {
    async fn start(relay: Relay) -> Self {
        let (cert, key) = transport::generate_identity().unwrap();
        let endpoint = transport::server_endpoint("127.0.0.1:0".parse().unwrap(), cert.clone(), key).unwrap();
        let address = endpoint.local_addr().unwrap();
        tokio::spawn(relay.run(endpoint.clone()));
        Self { endpoint, address, cert }
    }
    async fn session(&self, hello: Hello) -> anyhow::Result<Session> {
        Session::connect(self.address, self.cert.clone(), hello).await
    }
    async fn host(&self, listing: Option<Listing>) -> Session {
        let mut s = self.session(Hello::Create { map: "mp_lobby".into() }).await.unwrap();
        if listing.is_some() {
            s.publish(listing).await.unwrap();
        }
        s
    }
    async fn browser(&self) -> Browser {
        Browser::connect(self.address, self.cert.clone()).await.unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.endpoint.close(VarInt::from_u32(0), b"test end");
    }
}

fn tdm(map: &str, state: LobbyState) -> Listing {
    Listing { mode: "tdm".into(), map: map.into(), bots: 0, max: 18, state, joinable: true, region: "local".into() }
}
const WAITING: LobbyState = LobbyState::Lobby { countdown: 30 };

/// The relay hears a publish a moment after it's sent.
async fn settle() {
    sleep(Duration::from_millis(150)).await;
}

#[tokio::test]
async fn listed_lobbies_are_browsed_filtered_and_quick_matched() {
    let f = Fixture::start(Relay::new()).await;
    let _private = f.host(None).await;
    let crash = f.host(Some(tdm("mp_crash", WAITING))).await;
    let mut dm = f.host(Some(Listing { mode: "dm".into(), ..tdm("mp_backlot", WAITING) })).await;
    let strike = f.host(Some(tdm("mp_strike", LobbyState::InMatch { time_left: 300 }))).await;
    // A friend in the Strike lobby makes it the fuller one.
    let friend = f.session(Hello::JoinListed { room: strike.room }).await.unwrap();
    assert_eq!(friend.room, strike.room);
    settle().await;

    let mut b = f.browser().await;
    let (total, all) = b.list(Filter::default()).await.unwrap();
    assert_eq!(total, 3, "private rooms aren't listed");
    assert_eq!(all[0].room, strike.room, "fuller first");
    assert_eq!(all[0].humans, 2);
    assert!(all.iter().all(|l| l.ping < 1000 && l.listing.region == "local"));

    let (_, tdm_only) = b.list(Filter { mode: Some("tdm".into()), ..Filter::default() }).await.unwrap();
    assert_eq!(tdm_only.len(), 2);
    let (_, crash_only) = b.list(Filter { map: Some("mp_crash".into()), ..Filter::default() }).await.unwrap();
    assert_eq!(crash_only.iter().map(|l| l.room).collect::<Vec<_>>(), [crash.room]);

    // Quick match: the fullest open TDM lobby.
    let (_, best) = b.quick_match(Filter { mode: Some("tdm".into()), max_ping: 500, ..Filter::default() }).await.unwrap();
    assert_eq!(best.iter().map(|l| l.room).collect::<Vec<_>>(), [strike.room, crash.room]);
    // Nothing near enough: the total says there are lobbies further away.
    let (total, near) = b.quick_match(Filter { mode: Some("tdm".into()), max_ping: 1, ..Filter::default() }).await.unwrap();
    assert!(near.iter().all(|l| l.ping <= 1));
    assert_eq!(total, 2);

    // The host closes its lobby to newcomers: quick match skips it, the
    // list still shows it, and joining it fails.
    dm.publish(Some(Listing { joinable: false, ..tdm("mp_backlot", WAITING) })).await.unwrap();
    settle().await;
    let (_, open) = b.list(Filter { open_only: true, ..Filter::default() }).await.unwrap();
    assert!(!open.iter().any(|l| l.room == dm.room));
    assert!(f.session(Hello::JoinListed { room: dm.room }).await.is_err());
    // Taking it off the list.
    dm.publish(None).await.unwrap();
    settle().await;
    assert_eq!(b.list(Filter::default()).await.unwrap().0, 2);
    // A room that's gone disappears at once.
    drop(crash);
    settle().await;
    let (_, left) = b.list(Filter::default()).await.unwrap();
    assert_eq!(left.iter().map(|l| l.room).collect::<Vec<_>>(), [strike.room]);
}

#[tokio::test]
async fn private_and_full_rooms_cannot_be_joined_from_the_list() {
    let f = Fixture::start(Relay::new()).await;
    let private = f.host(None).await;
    assert!(f.session(Hello::JoinListed { room: private.room }).await.is_err());
    // Room for the host and one more.
    let small = f.host(Some(Listing { max: 2, ..tdm("mp_vacant", WAITING) })).await;
    settle().await;
    let _one = f.session(Hello::JoinListed { room: small.room }).await.unwrap();
    assert!(f.session(Hello::JoinListed { room: small.room }).await.is_err(), "full");
    let mut b = f.browser().await;
    let (_, l) = b.list(Filter::default()).await.unwrap();
    assert!(!l[0].open());
    assert!(b.quick_match(Filter::default()).await.unwrap().1.is_empty());
}

#[tokio::test]
async fn listings_expire_unless_refreshed() {
    let f = Fixture::start(Relay::with_listing_ttl(Duration::from_millis(600))).await;
    let mut kept = f.host(Some(tdm("mp_crash", WAITING))).await;
    let _stale = f.host(Some(tdm("mp_bog", WAITING))).await;
    settle().await;
    let mut b = f.browser().await;
    assert_eq!(b.list(Filter::default()).await.unwrap().0, 2);
    for _ in 0..4 {
        sleep(Duration::from_millis(250)).await;
        kept.publish(Some(tdm("mp_crash", LobbyState::Lobby { countdown: 20 }))).await.unwrap();
    }
    settle().await;
    let (_, l) = b.list(Filter::default()).await.unwrap();
    assert_eq!(l.iter().map(|l| l.room).collect::<Vec<_>>(), [kept.room]);
    assert_eq!(l[0].listing.state, LobbyState::Lobby { countdown: 20 });
}

#[tokio::test]
async fn abusive_browsers_and_players_are_cut_off() {
    let f = Fixture::start(Relay::new()).await;
    // Asking too often closes the browser.
    let mut b = f.browser().await;
    let mut failed = false;
    for _ in 0..40 {
        if b.list(Filter::default()).await.is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed);
    // Players can't list a lobby (the host's connection survives others' tries).
    let host = f.host(Some(tdm("mp_crash", WAITING))).await;
    settle().await;
    let mut player = f.session(Hello::JoinListed { room: host.room }).await.unwrap();
    assert!(player.publish(Some(tdm("mp_crash", WAITING))).await.is_err());
    // Guessing rooms runs into the failed-join lockout.
    let mut refused = 0;
    for room in 1..=12u64 {
        if timeout(Duration::from_secs(6), f.session(Hello::JoinListed { room })).await.unwrap().is_err() {
            refused += 1;
        }
    }
    assert_eq!(refused, 12);
    assert!(f.session(Hello::JoinListed { room: host.room }).await.is_err(), "locked out after 10 failures");
}

#[tokio::test]
async fn the_nearest_player_is_elected_host_and_ids_swap() {
    // Every connection here is on this PC: count the host 200 ms further.
    let f = Fixture::start(Relay::with_host_penalty(Duration::from_millis(200))).await;
    let mut host = f.host(Some(tdm("mp_crash", WAITING))).await;
    let mut a = f.session(Hello::JoinListed { room: host.room }).await.unwrap();
    let mut b = f.session(Hello::JoinListed { room: host.room }).await.unwrap();
    let (a_id, b_id) = (a.peer, b.peer);
    assert_eq!(timeout(Duration::from_secs(2), host.next_control()).await.unwrap(), Some(Control::PeerJoined(a_id)));
    assert_eq!(timeout(Duration::from_secs(2), host.next_control()).await.unwrap(), Some(Control::PeerJoined(b_id)));
    // Only the host may call it.
    assert!(a.elect().await.is_err());
    host.elect().await.unwrap();
    let elected = |c: Option<Control>| match c {
        Some(Control::Elected { swap }) => swap,
        other => panic!("expected an election, got {other:?}"),
    };
    let winner = elected(timeout(Duration::from_secs(2), host.next_control()).await.unwrap()).expect("a swap");
    assert!(winner == a_id || winner == b_id);
    assert_eq!(elected(timeout(Duration::from_secs(2), a.next_control()).await.unwrap()), Some(winner));
    assert_eq!(elected(timeout(Duration::from_secs(2), b.next_control()).await.unwrap()), Some(winner));
    // The old host took the winner's ID; the winner is HOST.
    assert_eq!(host.current_peer(), winner);
    let (new_host, other) = if winner == a_id { (&mut a, &mut b) } else { (&mut b, &mut a) };
    assert_eq!(new_host.current_peer(), HOST);
    // The new host's powers: a message to everyone.
    new_host.send_message(EVERYONE, vec![1, 2, 3]).await.unwrap();
    assert_eq!(timeout(Duration::from_secs(2), host.next_control()).await.unwrap(), Some(Control::Message { peer: HOST, data: vec![1, 2, 3] }));
    assert_eq!(timeout(Duration::from_secs(2), other.next_control()).await.unwrap(), Some(Control::Message { peer: HOST, data: vec![1, 2, 3] }));
    // The old host is a player now: its inputs reach the new host.
    host.send_inputs(vec![InputCommand { sequence: 1, forward: 5, ..Default::default() }]).unwrap();
    let Packet::RemoteInputs { peer, .. } = timeout(Duration::from_secs(2), new_host.next_packet()).await.unwrap().unwrap() else {
        panic!("expected inputs")
    };
    assert_eq!(peer, winner);
    // The new host lists the room.
    new_host.publish(Some(tdm("mp_bog", WAITING))).await.unwrap();
    settle().await;
    let mut browser = f.browser().await;
    let (_, l) = browser.list(Filter::default()).await.unwrap();
    assert_eq!(l.iter().map(|l| l.listing.map.as_str()).collect::<Vec<_>>(), ["mp_bog"]);
    assert_eq!(l[0].humans, 3);
}

#[tokio::test]
async fn a_near_host_stays_host() {
    let f = Fixture::start(Relay::new()).await;
    let mut host = f.host(None).await;
    let mut a = f.session(Hello::Join { room: host.room, invite: host.invite.clone().unwrap() }).await.unwrap();
    let _ = timeout(Duration::from_secs(2), host.next_control()).await.unwrap();
    host.elect().await.unwrap();
    assert_eq!(timeout(Duration::from_secs(2), host.next_control()).await.unwrap(), Some(Control::Elected { swap: None }));
    assert_eq!(timeout(Duration::from_secs(2), a.next_control()).await.unwrap(), Some(Control::Elected { swap: None }));
    assert_eq!(host.current_peer(), HOST);
}
