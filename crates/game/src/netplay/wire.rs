//! The bytes of [`LobbyMsg::Game`] and of a snapshot's `extra`: small,
//! bounded, every value checked on the way in, like the rest of the
//! protocol (`crates/multiplayer`). A bad message is dropped, never trusted.
//!
//! [`LobbyMsg::Game`]: crate::online::lobby::LobbyMsg::Game

use anyhow::{Result, bail, ensure};
use bevy::math::Vec3;

pub struct W(pub Vec<u8>);

impl W {
    pub fn new() -> W {
        W(Vec::with_capacity(64))
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend(if v.is_finite() { v.clamp(-100_000.0, 100_000.0) } else { 0.0 }.to_le_bytes());
        self
    }
    pub fn vec3(&mut self, v: Vec3) -> &mut Self {
        self.f32(v.x).f32(v.y).f32(v.z)
    }
    /// An angle in radians, to 1/10000 of a half turn.
    pub fn angle(&mut self, a: f32) -> &mut Self {
        let a = (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        self.u16(((a / std::f32::consts::PI * 32767.0).round() as i16) as u16)
    }
    /// Plain printable ASCII, cut to `max`.
    pub fn str(&mut self, s: &str, max: usize) -> &mut Self {
        let s: Vec<u8> = s.bytes().filter(|b| (0x20..0x7f).contains(b)).take(max.min(255)).collect();
        self.u8(s.len() as u8);
        self.0.extend(s);
        self
    }
}

pub struct R<'a>(pub &'a [u8]);

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(self.0.len() >= n, "truncated game message");
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => bail!("invalid flag"),
        }
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    pub fn f32(&mut self) -> Result<f32> {
        let v = f32::from_le_bytes(self.take(4)?.try_into()?);
        ensure!(v.is_finite() && v.abs() <= 100_000.0, "invalid number");
        Ok(v)
    }
    pub fn vec3(&mut self) -> Result<Vec3> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    pub fn angle(&mut self) -> Result<f32> {
        Ok(self.u16()? as i16 as f32 / 32767.0 * std::f32::consts::PI)
    }
    pub fn str(&mut self, max: usize) -> Result<String> {
        let n = self.u8()? as usize;
        ensure!(n <= max, "string too long");
        let s = std::str::from_utf8(self.take(n)?)?;
        ensure!(s.bytes().all(|b| (0x20..0x7f).contains(&b)), "invalid string");
        Ok(s.to_owned())
    }
    pub fn done(&self) -> Result<()> {
        ensure!(self.0.is_empty(), "trailing game bytes");
        Ok(())
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One-off news, reliable and in order ([`LobbyMsg::Game`]).
///
/// [`LobbyMsg::Game`]: crate::online::lobby::LobbyMsg::Game
#[derive(Clone, Debug, PartialEq)]
pub enum GameMsg {
    // ---- Host -> guests ----
    /// A round of S&D or Sabotage is over (its killcam, the freeze).
    RoundOver,
    /// XP for this guest ([`crate::ui::progression::AwardKind`] by index).
    Award(u8),
    /// A killstreak notice for the kill feed or this guest's banner
    /// ([`crate::killstreaks::StreakNotice`]): kind, item, who, their team.
    Notice { kind: u8, item: u8, by: String, team: u8 },
    /// This guest's own streak: kills, the streaks held (bits), and the
    /// ones picked for the class (by [`crate::killstreaks::Hardpoint`] index).
    Streak { kills: u16, held: u8, chosen: Vec<u8> },
    /// An airstrike called in: the owner's pawn, where, the planes'
    /// heading and their times after the call.
    Airstrike { owner: u16, target: Vec3, dir: Vec3, planes: [f32; 3] },
    /// An explosive (C4, claymore) went off on the host.
    Blast { weapon: String, at: Vec3 },
    /// Free-for-all: the winner's name.
    Winner(String),
    /// This guest's soldier was hurt: by whom ([`BOT`] for nobody), how
    /// much, where (as [`LobbyMsg::Kill`]'s location), for its damage
    /// direction and hit feedback.
    ///
    /// [`BOT`]: crate::online::lobby::BOT
    /// [`LobbyMsg::Kill`]: crate::online::lobby::LobbyMsg::Kill
    Hurt { attacker: u16, amount: u8, location: u8 },
    /// A crate opened by this guest: what it held (0 ammo, else 1 + streak).
    Opened(u8),

    // ---- Guest -> host ----
    /// Call in a killstreak (and for an airstrike, where).
    UseStreak { item: u8, target: Option<Vec3> },
    /// This guest's measured round trip (ms), for the host's net graph.
    Ping(u16),
    /// The kill streaks this guest picked (by [`crate::killstreaks::Hardpoint`]
    /// index).
    Chosen(Vec<u8>),
}

pub const MAX_NAME: usize = 24;
const MAX_ASSET: usize = 48;

impl GameMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        match self {
            GameMsg::RoundOver => {
                w.u8(1);
            }
            GameMsg::Award(k) => {
                w.u8(2).u8(*k);
            }
            GameMsg::Notice { kind, item, by, team } => {
                w.u8(3).u8(*kind).u8(*item).str(by, MAX_NAME).u8(*team);
            }
            GameMsg::Streak { kills, held, chosen } => {
                w.u8(4).u16(*kills).u8(*held).u8(chosen.len().min(8) as u8);
                for c in chosen.iter().take(8) {
                    w.u8(*c);
                }
            }
            GameMsg::Airstrike { owner, target, dir, planes } => {
                w.u8(5).u16(*owner).vec3(*target).vec3(*dir);
                for p in planes {
                    w.f32(*p);
                }
            }
            GameMsg::Blast { weapon, at } => {
                w.u8(6).str(weapon, MAX_ASSET).vec3(*at);
            }
            GameMsg::Winner(name) => {
                w.u8(7).str(name, MAX_NAME);
            }
            GameMsg::Hurt { attacker, amount, location } => {
                w.u8(8).u16(*attacker).u8(*amount).u8(*location);
            }
            GameMsg::Opened(c) => {
                w.u8(9).u8(*c);
            }
            GameMsg::UseStreak { item, target } => {
                w.u8(20).u8(*item).bool(target.is_some());
                if let Some(t) = target {
                    w.vec3(*t);
                }
            }
            GameMsg::Ping(ms) => {
                w.u8(21).u16(*ms);
            }
            GameMsg::Chosen(list) => {
                w.u8(22).u8(list.len().min(8) as u8);
                for c in list.iter().take(8) {
                    w.u8(*c);
                }
            }
        }
        w.0
    }

    pub fn decode(data: &[u8]) -> Result<GameMsg> {
        let mut r = R(data);
        let msg = match r.u8()? {
            1 => GameMsg::RoundOver,
            2 => GameMsg::Award(r.u8()?),
            3 => GameMsg::Notice { kind: r.u8()?, item: r.u8()?, by: r.str(MAX_NAME)?, team: r.u8()? },
            4 => {
                let kills = r.u16()?;
                let held = r.u8()?;
                let n = r.u8()? as usize;
                ensure!(n <= 8, "too many streaks");
                let chosen = (0..n).map(|_| r.u8()).collect::<Result<Vec<_>>>()?;
                GameMsg::Streak { kills, held, chosen }
            }
            5 => GameMsg::Airstrike { owner: r.u16()?, target: r.vec3()?, dir: r.vec3()?, planes: [r.f32()?, r.f32()?, r.f32()?] },
            6 => GameMsg::Blast { weapon: r.str(MAX_ASSET)?, at: r.vec3()? },
            7 => GameMsg::Winner(r.str(MAX_NAME)?),
            8 => {
                let m = GameMsg::Hurt { attacker: r.u16()?, amount: r.u8()?, location: r.u8()? };
                ensure!(matches!(m, GameMsg::Hurt { location: 0..=4, .. }), "invalid hit location");
                m
            }
            9 => GameMsg::Opened(r.u8()?),
            20 => {
                let item = r.u8()?;
                let target = if r.bool()? { Some(r.vec3()?) } else { None };
                GameMsg::UseStreak { item, target }
            }
            21 => GameMsg::Ping(r.u16()?),
            22 => {
                let n = r.u8()? as usize;
                ensure!(n <= 8, "too many streaks");
                GameMsg::Chosen((0..n).map(|_| r.u8()).collect::<Result<Vec<_>>>()?)
            }
            _ => bail!("unknown game message"),
        };
        r.done()?;
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_messages_roundtrip_and_reject_junk() {
        let msgs = [
            GameMsg::RoundOver,
            GameMsg::Award(3),
            GameMsg::Notice { kind: 1, item: 4, by: "Soap".into(), team: 1 },
            GameMsg::Streak { kills: 7, held: 0b101, chosen: vec![0, 2, 4] },
            GameMsg::Airstrike { owner: 3, target: Vec3::new(1.0, 2.0, 3.0), dir: Vec3::X, planes: [0.0, 0.5, 1.0] },
            GameMsg::Blast { weapon: "claymore_mp".into(), at: Vec3::ONE },
            GameMsg::Winner("Price".into()),
            GameMsg::Hurt { attacker: 2, amount: 40, location: 4 },
            GameMsg::Opened(0),
            GameMsg::UseStreak { item: 2, target: Some(Vec3::ZERO) },
            GameMsg::UseStreak { item: 0, target: None },
            GameMsg::Ping(87),
            GameMsg::Chosen(vec![1, 3]),
        ];
        for m in msgs {
            let data = m.encode();
            assert_eq!(GameMsg::decode(&data).unwrap(), m);
            for n in 0..data.len() {
                assert!(GameMsg::decode(&data[..n]).is_err());
            }
            let mut extra = data.clone();
            extra.push(0);
            assert!(GameMsg::decode(&extra).is_err());
        }
        assert!(GameMsg::decode(&[99]).is_err());
        let mut nan = GameMsg::Blast { weapon: "c4_mp".into(), at: Vec3::ZERO }.encode();
        let n = nan.len();
        nan[n - 4..].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(GameMsg::decode(&nan).is_err());
    }
}
