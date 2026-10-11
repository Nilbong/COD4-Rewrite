//! Small, fixed-layout packets. Bounds are checked before allocation, no
//! network-provided strings become file paths, and trailing bytes are rejected.

use anyhow::{Result, bail, ensure};

pub const VERSION: u16 = 4;
pub const MAX_PACKET: usize = 1100;
pub const MAX_PLAYERS: usize = 18;
pub const INPUT_REDUNDANCY: usize = 3;
pub const MAX_CONTROL: usize = 1400;
/// Largest relayed message payload (lobby rosters, match start).
pub const MAX_MESSAGE: usize = 1200;
/// `Control::Message` target meaning every other member of the room (host only).
pub const EVERYONE: PeerId = u16::MAX;
pub const TICK_RATE: u32 = 60;
pub const SNAPSHOT_RATE: u32 = 20;
pub type PeerId = u16;
pub const HOST: PeerId = 0;

/// Capability shared only with invited players over a trusted channel.
/// Debug deliberately does not reveal it. It is not an account identity.
#[derive(Clone, PartialEq, Eq)]
pub struct Invite(pub [u8; 32]);

impl std::fmt::Debug for Invite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invite([redacted])")
    }
}

impl Invite {
    pub fn generate() -> Self {
        Self(rand::random())
    }
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
    pub fn from_hex(s: &str) -> Result<Self> {
        ensure!(s.len() == 64 && s.is_ascii(), "invite must be 64 hex characters");
        let mut bytes = [0; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)?;
        }
        Ok(Self(bytes))
    }
}

/// The short code a host reads out to friends ("K7QM-2X9D"). The relay
/// makes one per room; it stops working when the host leaves. 8 characters
/// from 31 unambiguous ones (~40 bits), and the relay limits failed guesses
/// per address, so codes can't be found by trying them.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LobbyCode(pub [u8; 8]);

impl LobbyCode {
    /// No 0/O, 1/I/L: easy to read out loud and type.
    pub const ALPHABET: &'static [u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
    pub fn generate() -> Self {
        let mut code = [0; 8];
        for c in &mut code {
            *c = Self::ALPHABET[rand::random_range(0..Self::ALPHABET.len())];
        }
        Self(code)
    }
    /// Lenient about what people type: case, spaces and dashes.
    pub fn parse(text: &str) -> Result<Self> {
        let mut code = Vec::with_capacity(8);
        for c in text.chars().filter(|c| !c.is_whitespace() && *c != '-') {
            let c = c.to_ascii_uppercase();
            ensure!(c.is_ascii() && Self::ALPHABET.contains(&(c as u8)), "invalid code character");
            code.push(c as u8);
        }
        ensure!(code.len() == 8, "a lobby code is 8 characters");
        Ok(Self(code.try_into().unwrap()))
    }
    fn check(&self) -> Result<()> {
        ensure!(self.0.iter().all(|c| Self::ALPHABET.contains(c)), "invalid lobby code");
        Ok(())
    }
}

impl std::fmt::Display for LobbyCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = std::str::from_utf8(&self.0).unwrap_or("????????");
        write!(f, "{}-{}", &s[..4], &s[4..])
    }
}

impl std::fmt::Debug for LobbyCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LobbyCode([redacted])")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Hello {
    Create { map: String },
    Join { room: u64, invite: Invite },
    /// Join by the host's short code.
    JoinCode(LobbyCode),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// `invite` and `code` are only sent to the host.
    Welcome { room: u64, peer: PeerId, map: String, invite: Option<Invite>, code: Option<LobbyCode> },
    PeerJoined(PeerId),
    PeerLeft(PeerId),
    /// A reliable, ordered message for the game (lobby roster, match start).
    /// Sent to the relay, `peer` is the target: players may only message the
    /// host; the host messages a player or [`EVERYONE`]. Delivered, `peer`
    /// is the sender, stamped by the relay.
    Message { peer: PeerId, data: Vec<u8> },
}

/// Only controls are accepted from clients: never positions, damage, health,
/// speed multipliers, fire rates, or claimed shooter/peer IDs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InputCommand {
    pub sequence: u32,
    pub forward: i8,
    pub right: i8,
    pub yaw: f32,
    pub pitch: f32,
    pub buttons: u16,
    /// 0 stand, 1 crouch, 2 prone.
    pub stance: u8,
}

pub mod button {
    pub const FIRE: u16 = 1;
    pub const AIM: u16 = 2;
    pub const JUMP: u16 = 4;
    pub const SPRINT: u16 = 8;
    pub const RELOAD: u16 = 16;
    pub const FRAG: u16 = 32;
    pub const SPECIAL: u16 = 64;
    pub const MELEE: u16 = 128;
    pub const USE: u16 = 256;
    pub const SWITCH: u16 = 512;
    /// Took (or left) cover: 3rd Person TDM.
    pub const COVER: u16 = 1024;
    pub const ALL: u16 = 2047;
}

impl InputCommand {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.forward != i8::MIN && self.right != i8::MIN, "invalid move axis");
        ensure!(self.yaw.is_finite() && self.yaw.abs() <= std::f32::consts::PI, "invalid yaw");
        ensure!(self.pitch.is_finite() && self.pitch.abs() <= 1.55, "invalid pitch");
        ensure!(self.buttons & !button::ALL == 0 && self.stance <= 2, "invalid input flags");
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PawnState {
    /// Stable network ID, never a Bevy Entity or pointer. Bots also need IDs.
    pub id: u16,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub health: u8,
    pub stance: u8,
    /// 0 alive, 1 dead. Extend the versioned protocol for other states.
    pub life: u8,
    /// Shots fired, wrapping: a change means it fired (for animation, sound).
    pub shots: u8,
    /// [`pawn_flags`].
    pub flags: u8,
}

pub mod pawn_flags {
    pub const ADS: u8 = 1;
    pub const SPRINT: u8 = 2;
    pub const ON_GROUND: u8 = 4;
    pub const RELOADING: u8 = 8;
    /// In cover (3rd Person TDM), and the cover is a tall one.
    pub const IN_COVER: u8 = 16;
    pub const COVER_HIGH: u8 = 32;
    pub const ALL: u8 = 63;
}

impl PawnState {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.position.iter().all(|v| v.is_finite() && v.abs() <= 100_000.0), "invalid position");
        ensure!(self.velocity.iter().all(|v| v.is_finite() && v.abs() <= 1000.0), "invalid velocity");
        ensure!(self.yaw.is_finite() && self.yaw.abs() <= std::f32::consts::PI, "invalid yaw");
        ensure!(self.pitch.is_finite() && self.pitch.abs() <= 1.55, "invalid pitch");
        ensure!(self.health <= 100 && self.stance <= 2 && self.life <= 1, "invalid pawn state");
        ensure!(self.flags & !pawn_flags::ALL == 0, "invalid pawn flags");
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub tick: u32,
    /// Last input consumed for the receiving player, not the last received.
    pub acknowledged_input: u32,
    pub pawns: Vec<PawnState>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    /// Player -> relay -> host (relay stamps the actual peer ID).
    Inputs(Vec<InputCommand>),
    RemoteInputs {
        peer: PeerId,
        commands: Vec<InputCommand>,
    },
    /// Host -> one player. The relay checks the sender's role and recipient.
    Snapshot {
        recipient: PeerId,
        state: Snapshot,
    },
}

fn header(kind: u8) -> Vec<u8> {
    let mut out = b"C4MP".to_vec();
    out.extend(VERSION.to_le_bytes());
    out.push(kind);
    out
}
fn u16_out(out: &mut Vec<u8>, v: u16) {
    out.extend(v.to_le_bytes());
}
fn u32_out(out: &mut Vec<u8>, v: u32) {
    out.extend(v.to_le_bytes());
}
fn u64_out(out: &mut Vec<u8>, v: u64) {
    out.extend(v.to_le_bytes());
}
fn f32_out(out: &mut Vec<u8>, v: f32) {
    out.extend(v.to_le_bytes());
}

fn map_out(out: &mut Vec<u8>, map: &str) -> Result<()> {
    ensure!(
        !map.is_empty() && map.len() <= 48 && map.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "invalid map identifier"
    );
    out.push(map.len() as u8);
    out.extend(map.bytes());
    Ok(())
}

struct Reader<'a> {
    rest: &'a [u8],
}
impl<'a> Reader<'a> {
    fn new(data: &'a [u8], limit: usize) -> Result<(Self, u8)> {
        ensure!(data.len() <= limit, "packet too large");
        let mut r = Self { rest: data };
        ensure!(r.take(4)? == b"C4MP", "bad packet magic");
        ensure!(r.u16()? == VERSION, "incompatible protocol version");
        let kind = r.u8()?;
        Ok((r, kind))
    }
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        ensure!(self.rest.len() >= len, "truncated packet");
        let (head, tail) = self.rest.split_at(len);
        self.rest = tail;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    fn float(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn map(&mut self) -> Result<String> {
        let len = self.u8()? as usize;
        ensure!(len > 0 && len <= 48, "invalid map length");
        let map = std::str::from_utf8(self.take(len)?)?.to_owned();
        map_out(&mut Vec::new(), &map)?;
        Ok(map)
    }
    fn done(&self) -> Result<()> {
        ensure!(self.rest.is_empty(), "trailing packet bytes");
        Ok(())
    }
}

impl Hello {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = header(match self {
            Self::Create { .. } => 1,
            Self::Join { .. } => 2,
            Self::JoinCode(_) => 6,
        });
        match self {
            Self::Create { map } => map_out(&mut out, map)?,
            Self::Join { room, invite } => {
                u64_out(&mut out, *room);
                out.extend(invite.0);
            }
            Self::JoinCode(code) => {
                code.check()?;
                out.extend(code.0);
            }
        }
        Ok(out)
    }
    pub fn decode(data: &[u8]) -> Result<Self> {
        let (mut r, kind) = Reader::new(data, MAX_CONTROL)?;
        let hello = match kind {
            1 => Self::Create { map: r.map()? },
            2 => Self::Join { room: r.u64()?, invite: Invite(r.take(32)?.try_into()?) },
            6 => {
                let code = LobbyCode(r.take(8)?.try_into()?);
                code.check()?;
                Self::JoinCode(code)
            }
            _ => bail!("invalid hello"),
        };
        r.done()?;
        Ok(hello)
    }
}

impl Control {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = header(match self {
            Self::Welcome { .. } => 3,
            Self::PeerJoined(_) => 4,
            Self::PeerLeft(_) => 5,
            Self::Message { .. } => 7,
        });
        match self {
            Self::Welcome { room, peer, map, invite, code } => {
                u64_out(&mut out, *room);
                u16_out(&mut out, *peer);
                map_out(&mut out, map)?;
                out.push(invite.is_some() as u8);
                if let Some(invite) = invite {
                    out.extend(invite.0);
                }
                out.push(code.is_some() as u8);
                if let Some(code) = code {
                    code.check()?;
                    out.extend(code.0);
                }
            }
            Self::PeerJoined(peer) | Self::PeerLeft(peer) => u16_out(&mut out, *peer),
            Self::Message { peer, data } => {
                ensure!(!data.is_empty() && data.len() <= MAX_MESSAGE, "invalid message length");
                u16_out(&mut out, *peer);
                out.extend(data);
            }
        }
        ensure!(out.len() <= MAX_CONTROL, "control too large");
        Ok(out)
    }
    pub fn decode(data: &[u8]) -> Result<Self> {
        let (mut r, kind) = Reader::new(data, MAX_CONTROL)?;
        let control = match kind {
            3 => {
                let room = r.u64()?;
                let peer = r.u16()?;
                let map = r.map()?;
                let invite = match r.u8()? {
                    0 => None,
                    1 => Some(Invite(r.take(32)?.try_into()?)),
                    _ => bail!("invalid invite flag"),
                };
                let code = match r.u8()? {
                    0 => None,
                    1 => {
                        let code = LobbyCode(r.take(8)?.try_into()?);
                        code.check()?;
                        Some(code)
                    }
                    _ => bail!("invalid code flag"),
                };
                Self::Welcome { room, peer, map, invite, code }
            }
            4 => Self::PeerJoined(r.u16()?),
            5 => Self::PeerLeft(r.u16()?),
            7 => {
                let peer = r.u16()?;
                let data = r.take(r.rest.len())?.to_vec();
                ensure!(!data.is_empty() && data.len() <= MAX_MESSAGE, "invalid message length");
                Self::Message { peer, data }
            }
            _ => bail!("invalid relay control"),
        };
        r.done()?;
        Ok(control)
    }
}

fn commands_out(out: &mut Vec<u8>, commands: &[InputCommand]) -> Result<()> {
    ensure!(!commands.is_empty() && commands.len() <= INPUT_REDUNDANCY, "invalid input count");
    out.push(commands.len() as u8);
    for c in commands {
        c.validate()?;
        u32_out(out, c.sequence);
        out.extend([c.forward as u8, c.right as u8]);
        f32_out(out, c.yaw);
        f32_out(out, c.pitch);
        u16_out(out, c.buttons);
        out.push(c.stance);
    }
    Ok(())
}
fn commands_in(r: &mut Reader<'_>) -> Result<Vec<InputCommand>> {
    let count = r.u8()? as usize;
    ensure!(count > 0 && count <= INPUT_REDUNDANCY, "invalid input count");
    let mut commands = Vec::with_capacity(count);
    for _ in 0..count {
        let c = InputCommand {
            sequence: r.u32()?,
            forward: r.u8()? as i8,
            right: r.u8()? as i8,
            yaw: r.float()?,
            pitch: r.float()?,
            buttons: r.u16()?,
            stance: r.u8()?,
        };
        c.validate()?;
        commands.push(c);
    }
    Ok(commands)
}

impl Packet {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = header(match self {
            Self::Inputs(_) => 10,
            Self::RemoteInputs { .. } => 11,
            Self::Snapshot { .. } => 12,
        });
        match self {
            Self::Inputs(commands) => commands_out(&mut out, commands)?,
            Self::RemoteInputs { peer, commands } => {
                u16_out(&mut out, *peer);
                commands_out(&mut out, commands)?;
            }
            Self::Snapshot { recipient, state } => {
                ensure!(state.pawns.len() <= MAX_PLAYERS, "too many pawns");
                u16_out(&mut out, *recipient);
                u32_out(&mut out, state.tick);
                u32_out(&mut out, state.acknowledged_input);
                out.push(state.pawns.len() as u8);
                let mut ids = std::collections::HashSet::new();
                for p in &state.pawns {
                    p.validate()?;
                    ensure!(ids.insert(p.id), "duplicate pawn ID");
                    u16_out(&mut out, p.id);
                    for v in p.position.into_iter().chain(p.velocity) {
                        f32_out(&mut out, v);
                    }
                    f32_out(&mut out, p.yaw);
                    f32_out(&mut out, p.pitch);
                    out.extend([p.health, p.stance, p.life, p.shots, p.flags]);
                }
            }
        }
        ensure!(out.len() <= MAX_PACKET, "packet too large");
        Ok(out)
    }
    pub fn decode(data: &[u8]) -> Result<Self> {
        let (mut r, kind) = Reader::new(data, MAX_PACKET)?;
        let packet = match kind {
            10 => Self::Inputs(commands_in(&mut r)?),
            11 => Self::RemoteInputs { peer: r.u16()?, commands: commands_in(&mut r)? },
            12 => {
                let recipient = r.u16()?;
                let tick = r.u32()?;
                let acknowledged_input = r.u32()?;
                let count = r.u8()? as usize;
                ensure!(count <= MAX_PLAYERS, "too many pawns");
                let mut pawns = Vec::with_capacity(count);
                let mut ids = std::collections::HashSet::new();
                for _ in 0..count {
                    let p = PawnState {
                        id: r.u16()?,
                        position: [r.float()?, r.float()?, r.float()?],
                        velocity: [r.float()?, r.float()?, r.float()?],
                        yaw: r.float()?,
                        pitch: r.float()?,
                        health: r.u8()?,
                        stance: r.u8()?,
                        life: r.u8()?,
                        shots: r.u8()?,
                        flags: r.u8()?,
                    };
                    p.validate()?;
                    ensure!(ids.insert(p.id), "duplicate pawn ID");
                    pawns.push(p);
                }
                Self::Snapshot { recipient, state: Snapshot { tick, acknowledged_input, pawns } }
            }
            _ => bail!("invalid gameplay packet"),
        };
        r.done()?;
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets_roundtrip_and_fit_datagrams() {
        let packets = [
            Packet::Inputs(vec![InputCommand::default(); 3]),
            Packet::RemoteInputs { peer: 7, commands: vec![InputCommand::default()] },
            Packet::Snapshot {
                recipient: 1,
                state: Snapshot {
                    tick: 99,
                    acknowledged_input: 45,
                    pawns: (0..18).map(|id| PawnState { id, ..Default::default() }).collect(),
                },
            },
        ];
        for p in packets {
            let data = p.encode().unwrap();
            assert!(data.len() <= MAX_PACKET);
            assert_eq!(Packet::decode(&data).unwrap(), p);
        }
    }
    #[test]
    fn cover_flags_roundtrip() {
        let state = PawnState { id: 3, flags: pawn_flags::ADS | pawn_flags::IN_COVER | pawn_flags::COVER_HIGH, ..Default::default() };
        assert!(state.validate().is_ok());
        assert!(PawnState { flags: 64, ..Default::default() }.validate().is_err());
        let p = Packet::Snapshot { recipient: 1, state: Snapshot { tick: 1, acknowledged_input: 0, pawns: vec![state] } };
        assert_eq!(Packet::decode(&p.encode().unwrap()).unwrap(), p);
    }
    #[test]
    fn truncation_unknown_version_and_trailing_bytes_rejected() {
        let data = Packet::Inputs(vec![InputCommand::default()]).encode().unwrap();
        for n in 0..data.len() {
            assert!(Packet::decode(&data[..n]).is_err());
        }
        let mut extra = data.clone();
        extra.push(0);
        assert!(Packet::decode(&extra).is_err());
        let mut future = data;
        future[4] = 99;
        assert!(Packet::decode(&future).is_err());
        assert!(Packet::decode(&vec![0; MAX_PACKET + 1]).is_err());
    }
    #[test]
    fn hostile_values_rejected() {
        assert!(InputCommand { yaw: f32::NAN, ..Default::default() }.validate().is_err());
        assert!(InputCommand { buttons: 65535, ..Default::default() }.validate().is_err());
        assert!(PawnState { position: [f32::INFINITY, 0.0, 0.0], ..Default::default() }.validate().is_err());
        assert!(Hello::Create { map: "../../secret".into() }.encode().is_err());
        let mut data = Packet::Inputs(vec![InputCommand::default()]).encode().unwrap();
        data[7] = 255;
        assert!(Packet::decode(&data).is_err());
    }
    #[test]
    fn invites_are_redacted_and_control_roundtrips() {
        let invite = Invite::generate();
        assert_eq!(Invite::from_hex(&invite.to_hex()).unwrap(), invite);
        assert!(!format!("{invite:?}").contains(&invite.to_hex()));
        let code = LobbyCode::generate();
        let c = Control::Welcome { room: 123, peer: HOST, map: "mp_crash".into(), invite: Some(invite), code: Some(code) };
        assert_eq!(Control::decode(&c.encode().unwrap()).unwrap(), c);
        assert!(!format!("{c:?}").contains(&code.to_string()));
        let m = Control::Message { peer: EVERYONE, data: vec![7; MAX_MESSAGE] };
        assert_eq!(Control::decode(&m.encode().unwrap()).unwrap(), m);
        assert!(Control::Message { peer: 1, data: vec![7; MAX_MESSAGE + 1] }.encode().is_err());
        assert!(Control::Message { peer: 1, data: vec![] }.encode().is_err());
    }
    #[test]
    fn lobby_codes_parse_leniently() {
        let code = LobbyCode::generate();
        let shown = code.to_string();
        assert_eq!(shown.len(), 9);
        assert_eq!(LobbyCode::parse(&shown).unwrap(), code);
        assert_eq!(LobbyCode::parse(&shown.to_lowercase().replace('-', " ")).unwrap(), code);
        assert_eq!(LobbyCode::parse("k7qm 2x9d").unwrap().to_string(), "K7QM-2X9D");
        assert!(LobbyCode::parse("K7QM-0X1D").is_err());
        assert!(LobbyCode::parse("ABC").is_err());
        assert!(LobbyCode::parse("ABCD-EFG!").is_err());
        assert!(Hello::decode(&{
            let mut d = Hello::JoinCode(code).encode().unwrap();
            *d.last_mut().unwrap() = b'0';
            d
        })
        .is_err());
    }
}
