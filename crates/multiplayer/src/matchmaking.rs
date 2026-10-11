//! Public lobbies: what a host lists on the relay, how a browser asks for
//! them, and how the relay picks lobbies for quick match.
//!
//! A host's room is private (code or invite only) until it publishes a
//! [`Listing`]; it then appears to everyone browsing that relay and anyone
//! may join it ([`crate::protocol::Hello::JoinListed`]) while it is open.
//! Listings carry only what choosing a lobby needs: never names, codes or
//! addresses. The relay counts the humans itself and works out each
//! lobby's ping from its own round trips to the host and to the asker.

use crate::protocol::{MAX_PLAYERS, Reader, map_out, u16_out, u64_out};
use anyhow::{Result, bail, ensure};

/// Where a public lobby is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LobbyState {
    /// Waiting for players; `countdown` seconds to the start (0: not counting).
    Lobby { countdown: u16 },
    /// Playing, with this many seconds left.
    InMatch { time_left: u16 },
    /// The match is over: scores, then back to the lobby.
    Ending,
}

/// What a host tells the relay about its public lobby.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    /// The game type ("tdm"), an identifier.
    pub mode: String,
    pub map: String,
    pub bots: u8,
    /// Players the lobby takes, humans and bots ([`MAX_PLAYERS`] at most).
    pub max: u8,
    pub state: LobbyState,
    /// The host takes players now (in a match too, when it can).
    pub joinable: bool,
    /// The host's region tag ("eu", "local"), an identifier.
    pub region: String,
}

/// A public lobby as the relay shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct LobbyInfo {
    /// For [`crate::protocol::Hello::JoinListed`].
    pub room: u64,
    pub listing: Listing,
    /// Players connected, the host included.
    pub humans: u8,
    /// The relay's round trips to the host and to the asker, added (ms):
    /// how long a player's controls take to reach the host and back.
    pub ping: u16,
}

impl LobbyInfo {
    /// Room for one more human (a bot gives up its slot).
    pub fn open(&self) -> bool {
        self.listing.joinable && (self.humans as usize) < (self.listing.max as usize).min(MAX_PLAYERS)
    }
}

/// Which lobbies to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub mode: Option<String>,
    pub map: Option<String>,
    /// Only ones taking players.
    pub open_only: bool,
    /// Ping cap in ms (0: any).
    pub max_ping: u16,
}

impl Filter {
    pub fn matches(&self, l: &LobbyInfo) -> bool {
        self.mode.as_ref().is_none_or(|m| *m == l.listing.mode)
            && self.map.as_ref().is_none_or(|m| *m == l.listing.map)
            && (!self.open_only || l.open())
            && (self.max_ping == 0 || l.ping <= self.max_ping)
    }
}

/// Lobbies in one [`crate::protocol::Control::Lobbies`] frame.
pub const LOBBIES_PER_FRAME: usize = 10;
/// Lobbies in one answer, across its frames.
pub const MAX_LOBBIES: usize = 120;
/// Quick match's answer: the best few, best first.
pub const QUICK_MATCH_CHOICES: usize = 3;
/// A listing not refreshed for this long is hidden.
pub const LISTING_TTL: std::time::Duration = std::time::Duration::from_secs(15);
pub const MAX_MODE: usize = 16;
pub const MAX_REGION: usize = 16;

/// The browser's order: open lobbies first, then fuller, then nearer.
pub fn sort_for_list(lobbies: &mut [LobbyInfo]) {
    lobbies.sort_by_key(|l| (!l.open(), std::cmp::Reverse(l.humans), l.ping, l.room));
}

/// Quick match's order among open lobbies: fuller first (more humans,
/// then fewer slots left to bots), a lobby still waiting before one in a
/// match, then the nearest.
pub fn sort_for_quick_match(lobbies: &mut [LobbyInfo]) {
    lobbies.sort_by_key(|l| {
        let playing = !matches!(l.listing.state, LobbyState::Lobby { .. });
        (std::cmp::Reverse(l.humans), playing, l.ping, l.room)
    });
}

/// A room member as a possible match host: the relay's round trip to it
/// and the share of its packets lost (per mille).
#[derive(Clone, Copy, Debug)]
pub struct HostCandidate {
    pub peer: crate::protocol::PeerId,
    pub rtt: std::time::Duration,
    pub loss_permille: u64,
}

/// The best match host, if it isn't the host already: everyone's delay to
/// the host is their round trip to the relay plus the host's, so the one
/// nearest the relay (in 5 ms steps), then the one losing fewest packets;
/// and only when that's more than `margin` nearer than the host.
pub fn choose_host(candidates: &[HostCandidate], margin: std::time::Duration) -> Option<crate::protocol::PeerId> {
    use crate::protocol::HOST;
    let host = candidates.iter().find(|c| c.peer == HOST)?;
    let best = candidates.iter().min_by_key(|c| (c.rtt.as_millis() / 5, c.loss_permille, c.peer))?;
    (best.peer != HOST && best.rtt + margin < host.rtt).then_some(best.peer)
}

/// A short identifier (mode, region): ASCII letters, digits and `_`.
fn ident_out(out: &mut Vec<u8>, s: &str, max: usize) -> Result<()> {
    ensure!(s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'), "invalid identifier");
    out.push(s.len() as u8);
    out.extend(s.bytes());
    Ok(())
}
fn ident_in(r: &mut Reader<'_>, max: usize) -> Result<String> {
    let len = r.u8()? as usize;
    ensure!(len <= max, "identifier too long");
    let s = std::str::from_utf8(r.take(len)?)?.to_owned();
    ensure!(s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'), "invalid identifier");
    Ok(s)
}
fn flag_in(r: &mut Reader<'_>) -> Result<bool> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => bail!("invalid flag"),
    }
}

pub(crate) fn listing_out(out: &mut Vec<u8>, l: &Listing) -> Result<()> {
    ensure!(!l.mode.is_empty() && (l.max as usize) <= MAX_PLAYERS && l.bots <= l.max, "invalid listing");
    ident_out(out, &l.mode, MAX_MODE)?;
    map_out(out, &l.map)?;
    out.extend([l.bots, l.max]);
    let (state, secs) = match l.state {
        LobbyState::Lobby { countdown } => (0, countdown),
        LobbyState::InMatch { time_left } => (1, time_left),
        LobbyState::Ending => (2, 0),
    };
    out.push(state);
    u16_out(out, secs);
    out.push(l.joinable as u8);
    ident_out(out, &l.region, MAX_REGION)
}

pub(crate) fn listing_in(r: &mut Reader<'_>) -> Result<Listing> {
    let mode = ident_in(r, MAX_MODE)?;
    let map = r.map()?;
    let (bots, max) = (r.u8()?, r.u8()?);
    let state = match (r.u8()?, r.u16()?) {
        (0, countdown) => LobbyState::Lobby { countdown },
        (1, time_left) => LobbyState::InMatch { time_left },
        (2, _) => LobbyState::Ending,
        _ => bail!("invalid lobby state"),
    };
    let joinable = flag_in(r)?;
    let region = ident_in(r, MAX_REGION)?;
    ensure!(!mode.is_empty() && (max as usize) <= MAX_PLAYERS && bots <= max, "invalid listing");
    Ok(Listing { mode, map, bots, max, state, joinable, region })
}

pub(crate) fn filter_out(out: &mut Vec<u8>, f: &Filter) -> Result<()> {
    ensure!(f.mode.as_ref().is_none_or(|m| !m.is_empty()), "empty mode filter");
    ident_out(out, f.mode.as_deref().unwrap_or(""), MAX_MODE)?;
    match &f.map {
        Some(m) => map_out(out, m)?,
        None => out.push(0),
    }
    out.push(f.open_only as u8);
    u16_out(out, f.max_ping);
    Ok(())
}

pub(crate) fn filter_in(r: &mut Reader<'_>) -> Result<Filter> {
    let mode = Some(ident_in(r, MAX_MODE)?).filter(|m| !m.is_empty());
    let map = if r.peek() == Some(0) {
        r.u8()?;
        None
    } else {
        Some(r.map()?)
    };
    Ok(Filter { mode, map, open_only: flag_in(r)?, max_ping: r.u16()? })
}

pub(crate) fn lobbies_out(out: &mut Vec<u8>, total: u16, last: bool, lobbies: &[LobbyInfo]) -> Result<()> {
    ensure!(lobbies.len() <= LOBBIES_PER_FRAME, "too many lobbies in a frame");
    u16_out(out, total);
    out.push(last as u8);
    out.push(lobbies.len() as u8);
    for l in lobbies {
        ensure!(l.humans as usize <= MAX_PLAYERS, "invalid player count");
        u64_out(out, l.room);
        listing_out(out, &l.listing)?;
        out.push(l.humans);
        u16_out(out, l.ping);
    }
    Ok(())
}

pub(crate) fn lobbies_in(r: &mut Reader<'_>) -> Result<(u16, bool, Vec<LobbyInfo>)> {
    let total = r.u16()?;
    let last = flag_in(r)?;
    let n = r.u8()? as usize;
    ensure!(n <= LOBBIES_PER_FRAME, "too many lobbies in a frame");
    let mut lobbies = Vec::with_capacity(n);
    for _ in 0..n {
        let room = r.u64()?;
        let listing = listing_in(r)?;
        let humans = r.u8()?;
        ensure!(humans as usize <= MAX_PLAYERS, "invalid player count");
        lobbies.push(LobbyInfo { room, listing, humans, ping: r.u16()? });
    }
    Ok((total, last, lobbies))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Control, Hello, MAX_CONTROL};

    fn listing() -> Listing {
        Listing {
            mode: "tdm".into(),
            map: "mp_crossfire".into(),
            bots: 6,
            max: 18,
            state: LobbyState::InMatch { time_left: 412 },
            joinable: true,
            region: "eu_west".into(),
        }
    }
    fn info(room: u64, humans: u8, ping: u16, state: LobbyState) -> LobbyInfo {
        LobbyInfo { room, listing: Listing { state, ..listing() }, humans, ping }
    }

    #[test]
    fn controls_roundtrip_and_fit_a_frame() {
        let biggest = Listing { mode: "m".repeat(MAX_MODE), map: "m".repeat(48), region: "r".repeat(MAX_REGION), ..listing() };
        let lobbies = (0..LOBBIES_PER_FRAME as u64).map(|room| LobbyInfo { room, listing: biggest.clone(), humans: 3, ping: 61 }).collect();
        let filter = Filter { mode: Some("tdm".into()), map: Some("mp_crash".into()), open_only: true, max_ping: 150 };
        for c in [
            Control::Publish(Some(listing())),
            Control::Publish(None),
            Control::List(Filter::default()),
            Control::QuickMatch(filter),
            Control::Lobbies { total: 99, last: false, lobbies },
            Control::Lobbies { total: 0, last: true, lobbies: vec![] },
        ] {
            let data = c.encode().unwrap();
            assert!(data.len() <= MAX_CONTROL, "{} bytes", data.len());
            assert_eq!(Control::decode(&data).unwrap(), c);
            for n in 0..data.len() {
                assert!(Control::decode(&data[..n]).is_err());
            }
        }
        for h in [Hello::Browse, Hello::JoinListed { room: 77 }] {
            assert_eq!(Hello::decode(&h.encode().unwrap()).unwrap(), h);
        }
    }

    #[test]
    fn hostile_listings_rejected() {
        for l in [
            Listing { mode: "".into(), ..listing() },
            Listing { mode: "t d m".into(), ..listing() },
            Listing { region: "../x".into(), ..listing() },
            Listing { map: "../../x".into(), ..listing() },
            Listing { max: MAX_PLAYERS as u8 + 1, ..listing() },
            Listing { bots: 19, max: 18, ..listing() },
        ] {
            assert!(Control::Publish(Some(l)).encode().is_err());
        }
        // An unknown state byte.
        let mut data = Control::Publish(Some(listing())).encode().unwrap();
        let state = data.len() - (1 + "eu_west".len()) - 1 - 2 - 1;
        assert_eq!(data[state], 1);
        data[state] = 9;
        assert!(Control::decode(&data).is_err());
        let too_many = (0..11).map(|room| info(room, 1, 0, LobbyState::Ending)).collect();
        assert!(Control::Lobbies { total: 11, last: true, lobbies: too_many }.encode().is_err());
        assert!(Control::List(Filter { mode: Some(String::new()), ..Filter::default() }).encode().is_err());
    }

    #[test]
    fn quick_match_prefers_fuller_then_waiting_then_nearer() {
        let waiting = LobbyState::Lobby { countdown: 20 };
        let playing = LobbyState::InMatch { time_left: 300 };
        let mut l = vec![info(1, 2, 20, waiting), info(2, 5, 90, playing), info(3, 5, 40, waiting), info(4, 5, 30, playing)];
        sort_for_quick_match(&mut l);
        assert_eq!(l.iter().map(|l| l.room).collect::<Vec<_>>(), [3, 4, 2, 1]);
    }

    #[test]
    fn the_nearest_steady_player_hosts() {
        let ms = std::time::Duration::from_millis;
        let c = |peer, rtt, loss_permille| HostCandidate { peer, rtt: ms(rtt), loss_permille };
        let margin = ms(10);
        assert_eq!(choose_host(&[c(0, 60, 0), c(1, 20, 0), c(2, 30, 0)], margin), Some(1));
        // Not clearly nearer: the host stays.
        assert_eq!(choose_host(&[c(0, 25, 0), c(1, 20, 0)], margin), None);
        // Equal round trips (same 5 ms step): the one losing fewer packets.
        assert_eq!(choose_host(&[c(0, 80, 0), c(1, 21, 40), c(2, 23, 2)], margin), Some(2));
        // The host is best already.
        assert_eq!(choose_host(&[c(0, 5, 0), c(1, 40, 0)], margin), None);
        assert_eq!(choose_host(&[c(1, 5, 0)], margin), None);
    }

    #[test]
    fn filters_and_openness() {
        let mut full = info(1, 18, 10, LobbyState::Lobby { countdown: 0 });
        assert!(!full.open());
        full.humans = 17;
        assert!(full.open());
        full.listing.joinable = false;
        assert!(!full.open());
        let l = info(2, 3, 120, LobbyState::Ending);
        assert!(Filter::default().matches(&l));
        assert!(!Filter { max_ping: 80, ..Filter::default() }.matches(&l));
        assert!(!Filter { mode: Some("dm".into()), ..Filter::default() }.matches(&l));
        assert!(Filter { map: Some("mp_crossfire".into()), open_only: true, ..Filter::default() }.matches(&l));
    }
}
