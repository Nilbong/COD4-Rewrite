//! The game's lobby messages, carried by `Control::Message` through the
//! relay: who's in the lobby (names and ranks from their combat records),
//! the host's settings, and the start of the match. Like the rest of the
//! protocol, every field is bounded and checked; names are plain printable
//! ASCII so they can't carry colour codes, localisation keys or line breaks.

use crate::protocol::{MAX_MESSAGE, MAX_PLAYERS, PeerId};
use anyhow::{Result, bail, ensure};

/// Lobby message format version, separate from the transport's.
pub const LOBBY_VERSION: u8 = 1;
/// "[CLAN] Name" from the combat record fits easily.
pub const MAX_NAME: usize = 24;
/// The game build's version string ("0.1.0").
pub const MAX_BUILD: usize = 24;
/// Rank index (0 = level 1) and prestige, as CoD4's tables have them.
pub const MAX_RANK: u8 = 54;
pub const MAX_PRESTIGE: u8 = 10;

/// A player as the lobby shows them.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Profile {
    pub name: String,
    pub rank: u8,
    pub prestige: u8,
}

/// Someone in the lobby: the host is peer 0.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Member {
    pub peer: PeerId,
    pub profile: Profile,
    /// 0 allies, 1 axis.
    pub team: u8,
}

/// The host's match settings, as indices into the game's own choice lists
/// (both sides run the same build, checked when joining).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Settings {
    pub map: String,
    pub mode: u8,
    pub time: u8,
    pub score: u8,
    pub difficulty: u8,
    pub hardcore: bool,
}

/// Why the host turned a player away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Full,
    /// A different game version: the match wouldn't line up.
    Version,
    /// The match already started.
    Started,
}

/// A pawn in the match, as guests need it beyond its snapshot state.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PawnInfo {
    /// The snapshot's pawn ID.
    pub id: u16,
    /// Whose it is: a member's peer ID, or [`BOT`].
    pub peer: PeerId,
    pub name: String,
    pub team: u8,
    /// The weapon in hand: its gun spec (`m4:acog`), or for equipment its
    /// asset name (`rpg_mp`); empty for none.
    pub weapon: String,
    pub kills: u16,
    pub deaths: u16,
}

/// [`PawnInfo::peer`] of a bot.
pub const BOT: PeerId = PeerId::MAX;
/// Weapon and perk asset names.
pub const MAX_ASSET: usize = 48;

/// A guest's class for their next spawn (what the class menu picked).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Class {
    /// Primary then secondary: gun spec (`m4_mp:acog`) and camo.
    pub guns: Vec<(String, u8)>,
    pub perks: Vec<String>,
    pub special: Option<String>,
    pub inventory: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LobbyMsg {
    /// Player -> host, first thing after joining.
    Join { build: String, profile: Profile },
    /// Player -> host: put me on the other team.
    SwitchTeam,
    /// Host -> everyone: the whole lobby, sent whenever it changes.
    Lobby { settings: Settings, members: Vec<Member>, bots: [Vec<String>; 2] },
    /// Host -> everyone: load the match with the last lobby's settings.
    Start,
    /// Host -> one player, who should leave.
    Refused(Refusal),
    /// Host -> everyone, in the match: a pawn's details, when they change.
    Pawn(PawnInfo),
    /// Host -> everyone: a pawn left the match.
    PawnGone(u16),
    /// Player -> host: my class from my next spawn.
    Class(Class),
    /// Host -> everyone: the match is over; 0 allies won, 1 axis, 2 nobody
    /// (a draw, or free-for-all).
    MatchOver(u8),
    /// Host -> everyone: a kill, by the pawns' snapshot IDs ([`BOT`] for no
    /// attacker), the weapon's asset name, and where it hit (0 head, 1
    /// neck, 2 torso, 3 legs).
    Kill { victim: u16, attacker: u16, weapon: String, location: u8 },
    /// Host -> everyone: the teams' scores, when they change.
    Scores { allies: u32, axis: u32 },
    /// Host -> everyone: a grenade thrown (kind 0 frag, 1 flash, 2 stun,
    /// 3 smoke), from where, how fast, and its fuse (s).
    Throw { thrower: u16, kind: u8, at: [f32; 3], velocity: [f32; 3], fuse: f32 },
    /// Host -> everyone: a launched explosive (rocket, launcher grenade, C4,
    /// claymore): the weapon, from where, which way.
    Launch { shooter: u16, weapon: String, from: [f32; 3], dir: [f32; 3], yaw: f32 },
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite() && x.abs() <= 100_000.0)
}

fn asset_ok(s: &str) -> bool {
    s.len() <= MAX_ASSET && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_:+".contains(&b))
}

fn name_ok(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| (0x20..0x7f).contains(&b) && b != b'^')
}

/// Clean any text into a name that encodes: printable ASCII, no colour
/// codes, at most [`MAX_NAME`], never empty.
pub fn clean_name(s: &str) -> String {
    let s: String = s.chars().filter(|c| c.is_ascii() && (' '..='~').contains(c) && *c != '^').take(MAX_NAME).collect();
    let s = s.trim().to_owned();
    if s.is_empty() { "Player".into() } else { s }
}

struct W(Vec<u8>);
impl W {
    fn str(&mut self, s: &str, max: usize) -> Result<()> {
        ensure!(name_ok(s, max), "invalid name");
        self.0.push(s.len() as u8);
        self.0.extend(s.bytes());
        Ok(())
    }
    fn asset(&mut self, s: &str) -> Result<()> {
        ensure!(asset_ok(s), "invalid asset name");
        self.0.push(s.len() as u8);
        self.0.extend(s.bytes());
        Ok(())
    }
    fn profile(&mut self, p: &Profile) -> Result<()> {
        ensure!(p.rank <= MAX_RANK && p.prestige <= MAX_PRESTIGE, "invalid rank");
        self.str(&p.name, MAX_NAME)?;
        self.0.extend([p.rank, p.prestige]);
        Ok(())
    }
}

struct R<'a>(&'a [u8]);
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(self.0.len() >= n, "truncated lobby message");
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn str(&mut self, max: usize) -> Result<String> {
        let n = self.u8()? as usize;
        let s = std::str::from_utf8(self.take(n)?)?.to_owned();
        ensure!(name_ok(&s, max), "invalid name");
        Ok(s)
    }
    fn asset(&mut self) -> Result<String> {
        let n = self.u8()? as usize;
        let s = std::str::from_utf8(self.take(n)?)?.to_owned();
        ensure!(asset_ok(&s), "invalid asset name");
        Ok(s)
    }
    fn profile(&mut self) -> Result<Profile> {
        let p = Profile { name: self.str(MAX_NAME)?, rank: self.u8()?, prestige: self.u8()? };
        ensure!(p.rank <= MAX_RANK && p.prestige <= MAX_PRESTIGE, "invalid rank");
        Ok(p)
    }
}

impl LobbyMsg {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = W(vec![b'L', LOBBY_VERSION]);
        match self {
            Self::Join { build, profile } => {
                w.0.push(1);
                w.str(build, MAX_BUILD)?;
                w.profile(profile)?;
            }
            Self::SwitchTeam => w.0.push(2),
            Self::Lobby { settings: s, members, bots } => {
                ensure!(members.len() + bots[0].len() + bots[1].len() <= MAX_PLAYERS, "lobby too large");
                w.0.push(3);
                w.str(&s.map, 48)?;
                w.0.extend([s.mode, s.time, s.score, s.difficulty, s.hardcore as u8]);
                w.0.push(members.len() as u8);
                for m in members {
                    ensure!(m.team <= 1, "invalid team");
                    w.0.extend(m.peer.to_le_bytes());
                    w.profile(&m.profile)?;
                    w.0.push(m.team);
                }
                for side in bots {
                    w.0.push(side.len() as u8);
                    for b in side {
                        w.str(b, MAX_NAME)?;
                    }
                }
            }
            Self::Start => w.0.push(4),
            Self::Refused(r) => w.0.extend([
                5,
                match r {
                    Refusal::Full => 0,
                    Refusal::Version => 1,
                    Refusal::Started => 2,
                },
            ]),
            Self::Pawn(p) => {
                ensure!(p.team <= 1, "invalid team");
                w.0.push(6);
                w.0.extend(p.id.to_le_bytes());
                w.0.extend(p.peer.to_le_bytes());
                w.str(&p.name, MAX_NAME)?;
                w.0.push(p.team);
                w.asset(&p.weapon)?;
                w.0.extend(p.kills.to_le_bytes());
                w.0.extend(p.deaths.to_le_bytes());
            }
            Self::PawnGone(id) => {
                w.0.push(7);
                w.0.extend(id.to_le_bytes());
            }
            Self::MatchOver(winner) => {
                ensure!(*winner <= 2, "invalid winner");
                w.0.extend([9, *winner]);
            }
            Self::Throw { thrower, kind, at, velocity, fuse } => {
                ensure!(*kind <= 3 && finite(at) && finite(velocity) && finite(&[*fuse]), "invalid throw");
                w.0.push(12);
                w.0.extend(thrower.to_le_bytes());
                w.0.push(*kind);
                for v in at.iter().chain(velocity).chain([fuse]) {
                    w.0.extend(v.to_le_bytes());
                }
            }
            Self::Launch { shooter, weapon, from, dir, yaw } => {
                ensure!(finite(from) && finite(dir) && finite(&[*yaw]), "invalid launch");
                w.0.push(13);
                w.0.extend(shooter.to_le_bytes());
                w.asset(weapon)?;
                for v in from.iter().chain(dir).chain([yaw]) {
                    w.0.extend(v.to_le_bytes());
                }
            }
            Self::Scores { allies, axis } => {
                w.0.push(11);
                w.0.extend(allies.to_le_bytes());
                w.0.extend(axis.to_le_bytes());
            }
            Self::Kill { victim, attacker, weapon, location } => {
                ensure!(*location <= 3, "invalid hit location");
                w.0.push(10);
                w.0.extend(victim.to_le_bytes());
                w.0.extend(attacker.to_le_bytes());
                w.asset(weapon)?;
                w.0.push(*location);
            }
            Self::Class(c) => {
                ensure!(c.guns.len() <= 2 && c.perks.len() <= 3, "invalid class");
                w.0.push(8);
                w.0.push(c.guns.len() as u8);
                for (spec, camo) in &c.guns {
                    w.asset(spec)?;
                    w.0.push(*camo);
                }
                w.0.push(c.perks.len() as u8);
                for perk in &c.perks {
                    w.asset(perk)?;
                }
                for o in [&c.special, &c.inventory] {
                    w.asset(o.as_deref().unwrap_or(""))?;
                }
            }
        }
        ensure!(w.0.len() <= MAX_MESSAGE, "lobby message too large");
        Ok(w.0)
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        ensure!(data.len() <= MAX_MESSAGE, "lobby message too large");
        let mut r = R(data);
        ensure!(r.take(2)? == [b'L', LOBBY_VERSION], "incompatible lobby message");
        let msg = match r.u8()? {
            1 => Self::Join { build: r.str(MAX_BUILD)?, profile: r.profile()? },
            2 => Self::SwitchTeam,
            3 => {
                let settings = Settings {
                    map: r.str(48)?,
                    mode: r.u8()?,
                    time: r.u8()?,
                    score: r.u8()?,
                    difficulty: r.u8()?,
                    hardcore: match r.u8()? {
                        0 => false,
                        1 => true,
                        _ => bail!("invalid flag"),
                    },
                };
                let n = r.u8()? as usize;
                ensure!(n <= MAX_PLAYERS, "lobby too large");
                let mut members = Vec::with_capacity(n);
                for _ in 0..n {
                    let m = Member { peer: r.u16()?, profile: r.profile()?, team: r.u8()? };
                    ensure!(m.team <= 1, "invalid team");
                    ensure!(members.iter().all(|o: &Member| o.peer != m.peer), "duplicate member");
                    members.push(m);
                }
                let mut bots = [Vec::new(), Vec::new()];
                for side in &mut bots {
                    let n = r.u8()? as usize;
                    ensure!(n <= MAX_PLAYERS, "lobby too large");
                    for _ in 0..n {
                        side.push(r.str(MAX_NAME)?);
                    }
                }
                ensure!(members.len() + bots[0].len() + bots[1].len() <= MAX_PLAYERS, "lobby too large");
                Self::Lobby { settings, members, bots }
            }
            4 => Self::Start,
            5 => Self::Refused(match r.u8()? {
                0 => Refusal::Full,
                1 => Refusal::Version,
                2 => Refusal::Started,
                _ => bail!("invalid refusal"),
            }),
            6 => {
                let p = PawnInfo {
                    id: r.u16()?,
                    peer: r.u16()?,
                    name: r.str(MAX_NAME)?,
                    team: r.u8()?,
                    weapon: r.asset()?,
                    kills: r.u16()?,
                    deaths: r.u16()?,
                };
                ensure!(p.team <= 1, "invalid team");
                Self::Pawn(p)
            }
            7 => Self::PawnGone(r.u16()?),
            12 => {
                let thrower = r.u16()?;
                let kind = r.u8()?;
                let mut f = || -> Result<f32> { Ok(f32::from_le_bytes(r.take(4)?.try_into()?)) };
                let at = [f()?, f()?, f()?];
                let velocity = [f()?, f()?, f()?];
                let fuse = f()?;
                ensure!(kind <= 3 && finite(&at) && finite(&velocity) && finite(&[fuse]), "invalid throw");
                Self::Throw { thrower, kind, at, velocity, fuse }
            }
            13 => {
                let shooter = r.u16()?;
                let weapon = r.asset()?;
                let mut f = || -> Result<f32> { Ok(f32::from_le_bytes(r.take(4)?.try_into()?)) };
                let from = [f()?, f()?, f()?];
                let dir = [f()?, f()?, f()?];
                let yaw = f()?;
                ensure!(finite(&from) && finite(&dir) && finite(&[yaw]), "invalid launch");
                Self::Launch { shooter, weapon, from, dir, yaw }
            }
            11 => {
                let mut u = || -> Result<u32> { Ok(u32::from_le_bytes(r.take(4)?.try_into()?)) };
                let allies = u()?;
                Self::Scores { allies, axis: u()? }
            }
            10 => {
                let k = Self::Kill { victim: r.u16()?, attacker: r.u16()?, weapon: r.asset()?, location: r.u8()? };
                if let Self::Kill { location, .. } = &k {
                    ensure!(*location <= 3, "invalid hit location");
                }
                k
            }
            9 => {
                let w = r.u8()?;
                ensure!(w <= 2, "invalid winner");
                Self::MatchOver(w)
            }
            8 => {
                let n = r.u8()? as usize;
                ensure!(n <= 2, "invalid class");
                let mut c = Class::default();
                for _ in 0..n {
                    c.guns.push((r.asset()?, r.u8()?));
                }
                let n = r.u8()? as usize;
                ensure!(n <= 3, "invalid class");
                for _ in 0..n {
                    c.perks.push(r.asset()?);
                }
                let opt = |s: String| (!s.is_empty()).then_some(s);
                c.special = opt(r.asset()?);
                c.inventory = opt(r.asset()?);
                Self::Class(c)
            }
            _ => bail!("unknown lobby message"),
        };
        ensure!(r.0.is_empty(), "trailing lobby bytes");
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_lobby() -> LobbyMsg {
        let name = "[ABCD] ".to_string() + &"x".repeat(MAX_NAME - 7);
        LobbyMsg::Lobby {
            settings: Settings { map: "mp_crossfire".into(), mode: 2, time: 3, score: 1, difficulty: 2, hardcore: true },
            members: (0..10)
                .map(|i| Member { peer: i, profile: Profile { name: name.clone(), rank: 54, prestige: 10 }, team: (i % 2) as u8 })
                .collect(),
            bots: [vec!["y".repeat(MAX_NAME); 4], vec!["z".repeat(MAX_NAME); 4]],
        }
    }

    #[test]
    fn roundtrip_and_fit() {
        let msgs = [
            LobbyMsg::Join { build: "0.1.0".into(), profile: Profile { name: "[SAS] Soap".into(), rank: 9, prestige: 1 } },
            LobbyMsg::SwitchTeam,
            full_lobby(),
            LobbyMsg::Start,
            LobbyMsg::Refused(Refusal::Version),
            LobbyMsg::Pawn(PawnInfo { id: 7, peer: BOT, name: "Gaz".into(), team: 1, weapon: "m4_acog_mp".into(), kills: 3, deaths: 9 }),
            LobbyMsg::PawnGone(7),
            LobbyMsg::MatchOver(1),
            LobbyMsg::Scores { allies: 7, axis: 75 },
            LobbyMsg::Throw { thrower: 2, kind: 0, at: [1.0, 2.0, 3.0], velocity: [0.0, 5.0, -9.0], fuse: 3.5 },
            LobbyMsg::Launch { shooter: 2, weapon: "rpg_mp".into(), from: [1.0, 2.0, 3.0], dir: [0.0, 0.0, -1.0], yaw: 0.5 },
            LobbyMsg::Kill { victim: 3, attacker: BOT, weapon: "m16_gl_mp".into(), location: 0 },
            LobbyMsg::Class(Class {
                guns: vec![("m4_mp:acog".into(), 2), ("deserteagle_mp".into(), 0)],
                perks: vec!["specialty_fraggrenade".into(), "specialty_bulletdamage".into(), "specialty_longersprint".into()],
                special: Some("flash_grenade".into()),
                inventory: None,
            }),
        ];
        for m in msgs {
            let data = m.encode().unwrap();
            assert!(data.len() <= MAX_MESSAGE);
            assert_eq!(LobbyMsg::decode(&data).unwrap(), m);
        }
    }

    #[test]
    fn hostile_input_rejected() {
        let data = full_lobby().encode().unwrap();
        for n in 0..data.len() {
            assert!(LobbyMsg::decode(&data[..n]).is_err());
        }
        let mut extra = data.clone();
        extra.push(0);
        assert!(LobbyMsg::decode(&extra).is_err());
        let bad = |name: &str| LobbyMsg::Join { build: "1".into(), profile: Profile { name: name.into(), rank: 0, prestige: 0 } };
        assert!(bad("^1red").encode().is_err());
        assert!(bad("line\nbreak").encode().is_err());
        assert!(bad("").encode().is_err());
        assert!(LobbyMsg::Join { build: "1".into(), profile: Profile { name: "a".into(), rank: 55, prestige: 0 } }.encode().is_err());
        assert_eq!(clean_name("^1Pr\u{e9}ce\n"), "1Prce");
        assert_eq!(clean_name("   "), "Player");
    }
}
