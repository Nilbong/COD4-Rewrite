//! CoD4 demos (`.dm_1`, written by `/record`): the server messages a client
//! received, plus the recording player's own position and view at frame
//! rate. Reading them gives every player the server sent: where they went,
//! where they looked, their stance, weapon and events.
//!
//! Supports CoD4X demos (protocol 17+). The format follows the community's
//! reverse engineering, notably Iswenzz's CoD4-DM1.

mod fields;
mod huffman;

use anyhow::{Result, bail};
use fields::Field;
use std::collections::HashMap;

/// The recording player's state at one client frame.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// Server time of the player's last move, in ms.
    pub command_time: i32,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    /// Pitch, yaw, roll in degrees.
    pub angles: [f32; 3],
    pub movement_dir: i32,
    pub bob_cycle: i32,
}

/// One entity in a snapshot, with its fields by name.
#[derive(Clone, Debug)]
pub struct Entity {
    pub number: u32,
    values: Vec<u32>,
}

impl Entity {
    pub fn e_type(&self) -> u32 {
        self.int("eType")
    }

    /// An integer field, 0 if it isn't one this kind of entity has.
    pub fn int(&self, name: &str) -> u32 {
        slot(Kind::Entity, name).map_or(0, |s| self.values[s])
    }

    pub fn float(&self, name: &str) -> f32 {
        f32::from_bits(self.int(name))
    }

    pub fn origin(&self) -> [f32; 3] {
        [self.float("lerp.pos.trBase[0]"), self.float("lerp.pos.trBase[1]"), self.float("lerp.pos.trBase[2]")]
    }

    /// Pitch, yaw, roll in degrees.
    pub fn angles(&self) -> [f32; 3] {
        [self.float("lerp.apos.trBase[0]"), self.float("lerp.apos.trBase[1]"), self.float("lerp.apos.trBase[2]")]
    }
}

/// A client's shared state (team, rank, ...), with fields by name.
#[derive(Clone, Debug)]
pub struct Client {
    pub number: u32,
    values: Vec<u32>,
}

impl Client {
    pub fn int(&self, name: &str) -> u32 {
        slot(Kind::Client, name).map_or(0, |s| self.values[s])
    }

    pub fn team(&self) -> u32 {
        self.int("team")
    }
}

/// One server snapshot: what the recording client could see at `server_time`.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub server_time: i32,
    pub entities: Vec<Entity>,
    pub clients: Vec<Client>,
}

/// Everything read from a demo.
#[derive(Default)]
pub struct Demo {
    pub protocol: u32,
    /// The recording player's client number.
    pub client_num: i32,
    pub config_strings: HashMap<usize, String>,
    /// Player names (and clan tags) by client number.
    pub names: HashMap<u32, (String, String)>,
    /// Every name a client number has had, with the server time it was
    /// given (client numbers are reused when players leave).
    pub name_changes: Vec<(i32, u32, String)>,
    pub frames: Vec<Frame>,
    pub snapshots: Vec<Snapshot>,
    /// Server commands with the server time of the last snapshot before them.
    pub commands: Vec<(i32, String)>,
}

impl Demo {
    /// The name client `client` had at server time `time`.
    pub fn name_at(&self, client: u32, time: i32) -> Option<&str> {
        let mine = self.name_changes.iter().filter(|c| c.1 == client);
        mine.clone().filter(|c| c.0 <= time).last().or_else(|| mine.clone().next()).map(|c| c.2.as_str())
    }

    /// A config string value: the map is `mapname` in the server info.
    pub fn server_info(&self, key: &str) -> Option<&str> {
        // Info strings are `\key\value\key\value...`.
        self.config_strings.values().find_map(|s| {
            let mut parts = s.split('\\').skip(1);
            while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                if k.eq_ignore_ascii_case(key) {
                    return Some(v);
                }
            }
            None
        })
    }
}

// ------------------------------------------------------------------- fields

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Entity,
    Player,
    Client,
    Hud,
    Objective,
}

/// Field states are stored by name: each kind's fields get a slot each (slot
/// 0 is the entity or client number). Union members that alias in the
/// engine's structs get separate slots, which only matters for the values of
/// those fields when an entity slot changes type.
struct Slots {
    index: HashMap<&'static str, usize>,
    len: usize,
}

fn slots(kind: Kind) -> &'static Slots {
    use std::sync::OnceLock;
    static TABLES: OnceLock<HashMap<Kind, Slots>> = OnceLock::new();
    let all = TABLES.get_or_init(|| {
        let mut out = HashMap::new();
        let lists: [(Kind, Vec<&'static [Field]>); 5] = [
            (
                Kind::Entity,
                vec![
                    &fields::ENTITY,
                    &fields::PLAYER_ENTITY,
                    &fields::CORPSE,
                    &fields::ITEM,
                    &fields::MISSILE,
                    &fields::SCRIPT_MOVER,
                    &fields::SOUND_BLEND,
                    &fields::FX,
                    &fields::LOOP_FX,
                    &fields::HELICOPTER,
                    &fields::PLANE,
                    &fields::VEHICLE,
                    &fields::EVENT,
                ],
            ),
            (Kind::Player, vec![&fields::PLAYER_STATE]),
            (Kind::Client, vec![&fields::CLIENT]),
            (Kind::Hud, vec![&fields::HUD_ELEM]),
            (Kind::Objective, vec![&fields::OBJECTIVE]),
        ];
        for (kind, tables) in lists {
            let mut index = HashMap::new();
            index.insert("number", 0);
            for table in tables {
                for f in table {
                    let n = index.len();
                    index.entry(f.0).or_insert(n);
                }
            }
            let len = index.len();
            out.insert(kind, Slots { index, len });
        }
        out
    });
    &all[&kind]
}

fn slot(kind: Kind, name: &str) -> Option<usize> {
    slots(kind).index.get(name).copied()
}

/// A field list resolved to slots: (slot, bits, change hint).
type Resolved = Vec<(usize, i32, u8)>;

fn resolve(kind: Kind, table: &[Field]) -> Resolved {
    table.iter().map(|f| (slots(kind).index[f.0], f.1, f.2)).collect()
}

struct Tables {
    entity: [Resolved; 18],
    player: Resolved,
    client: Resolved,
    hud: Resolved,
    objective: Resolved,
}

impl Tables {
    fn new() -> Tables {
        Tables {
            entity: std::array::from_fn(|t| resolve(Kind::Entity, fields::entity_fields(t as u32))),
            player: resolve(Kind::Player, &fields::PLAYER_STATE),
            client: resolve(Kind::Client, &fields::CLIENT),
            hud: resolve(Kind::Hud, &fields::HUD_ELEM),
            objective: resolve(Kind::Objective, &fields::OBJECTIVE),
        }
    }
}

/// Bits needed to hold `x`.
fn min_bits(x: u32) -> u32 {
    32 - x.leading_zeros()
}

// ------------------------------------------------------------------ message

/// A decompressed server message. Bytes and bits share one stream: reading
/// bits takes a fresh byte only when the current one is used up, and byte
/// reads continue after the last byte bits were taken from.
struct Msg<'a> {
    data: &'a [u8],
    /// Next whole byte.
    read: usize,
    /// Next bit.
    bit: usize,
    overflowed: bool,
    last_entity: i32,
    raw_origins: bool,
}

impl Msg<'_> {
    fn read_bit(&mut self) -> u32 {
        if self.bit & 7 == 0 {
            if self.read >= self.data.len() {
                self.overflowed = true;
                return 0;
            }
            self.bit = 8 * self.read;
            self.read += 1;
        }
        let v = (self.data[self.bit / 8] >> (self.bit & 7)) & 1;
        self.bit += 1;
        v as u32
    }

    fn read_bits(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for i in 0..n {
            v |= self.read_bit() << i;
        }
        v
    }

    fn read_byte(&mut self) -> u32 {
        match self.data.get(self.read) {
            Some(&b) => {
                self.read += 1;
                b as u32
            }
            None => {
                self.overflowed = true;
                0
            }
        }
    }

    fn read_short(&mut self) -> i32 {
        let lo = self.read_byte();
        let hi = self.read_byte();
        (lo | hi << 8) as u16 as i16 as i32
    }

    fn read_int(&mut self) -> i32 {
        let mut v = 0u32;
        for i in 0..4 {
            v |= self.read_byte() << (8 * i);
        }
        v as i32
    }

    fn read_string(&mut self) -> String {
        let mut s = Vec::new();
        loop {
            if self.read >= self.data.len() {
                break;
            }
            let c = self.read_byte();
            if c == 0 {
                break;
            }
            s.push(c as u8);
        }
        String::from_utf8_lossy(&s).into_owned()
    }

    fn read_angle16(&mut self) -> f32 {
        self.read_short() as f32 * (360.0 / 65536.0)
    }

    fn read_entity_index(&mut self, bits: u32) -> i32 {
        if self.read_bit() == 1 {
            self.last_entity += 1;
        } else if bits != 10 || self.read_bit() == 1 {
            self.last_entity = self.read_bits(bits) as i32;
        } else {
            self.last_entity += self.read_bits(4) as i32;
        }
        self.last_entity
    }

    fn read_origin(&mut self, old: f32) -> f32 {
        if self.raw_origins {
            return f32::from_bits(self.read_int() as u32);
        }
        // Old protocols: a 16-bit absolute value or a 7-bit step. (Relative
        // to the map centre, which CoD4X demos don't need.)
        if self.read_bit() == 1 {
            ((((old as i32) + 0x8000) ^ self.read_bits(16) as i32) - 0x8000) as f32
        } else {
            (self.read_bits(7) as i32 - 64) as f32 + old
        }
    }

    fn read_eflags(&mut self, old: u32) -> u32 {
        if self.read_bit() == 1 {
            let mut v = 0;
            for i in 0..3 {
                v |= self.read_byte() << (8 * i);
            }
            v
        } else {
            old ^ (1 << self.read_bits(5))
        }
    }

    fn read_ground_entity(&mut self) -> u32 {
        if self.read_bit() == 1 {
            return 1022;
        }
        if self.read_bit() == 1 {
            return 0;
        }
        self.read_bits(2) | self.read_byte() << 2
    }

    /// One delta-coded field of `to`, from `from`.
    fn read_field(&mut self, time: i32, from: &[u32], to: &mut [u32], field: (usize, i32, u8), no_xor: bool) {
        let (s, bits, hints) = field;
        let old = if no_xor && hints == 3 { 0 } else { from[s] };
        let oldf = f32::from_bits(old);
        if hints != 2 && self.read_bit() == 0 {
            to[s] = old;
            return;
        }
        // A float that is often a small integer.
        let small_int_float = |m: &mut Msg, low_bits: u32, bias: i32| -> u32 {
            let b = m.read_bits(low_bits) as i32;
            let v = ((((1 << low_bits) * m.read_byte() as i32) + b) ^ (oldf as i32 + bias)) - bias;
            (v as f32).to_bits()
        };
        to[s] = match bits {
            0 => {
                if self.read_bit() == 0 {
                    self.read_bit() << 31
                } else if self.read_bit() == 0 {
                    small_int_float(self, 5, 4096)
                } else {
                    self.read_int() as u32 ^ old
                }
            }
            -89 => {
                if self.read_bit() == 0 {
                    small_int_float(self, 5, 4096)
                } else {
                    self.read_int() as u32 ^ old
                }
            }
            -88 => self.read_int() as u32 ^ old,
            -100 => {
                if self.read_bit() == 0 {
                    0f32.to_bits()
                } else {
                    self.read_angle16().to_bits()
                }
            }
            -99 => {
                if self.read_bit() == 1 {
                    if self.read_bit() == 0 {
                        small_int_float(self, 4, 2048)
                    } else {
                        self.read_int() as u32 ^ old
                    }
                } else {
                    0
                }
            }
            -98 => self.read_eflags(old),
            -97 => {
                if self.read_bit() == 1 {
                    self.read_int() as u32
                } else {
                    (time - self.read_bits(8) as i32) as u32
                }
            }
            -96 => self.read_ground_entity(),
            -95 => 100 * self.read_bits(7),
            -94 | -93 => self.read_byte(),
            -92..=-90 => self.read_origin(oldf).to_bits(),
            -87 => self.read_angle16().to_bits(),
            -86 => (self.read_bits(5) as f32 / 10.0 + 1.4).to_bits(),
            -85 => {
                // RGBA colour: unchanged RGB with alpha on/off, or new bytes.
                let mut c = old.to_le_bytes();
                if self.read_bit() == 1 {
                    c[3] = if c[3] != 0 { 0 } else { 0xFF };
                } else {
                    if self.read_bit() == 0 {
                        c[0] = self.read_byte() as u8;
                        c[1] = self.read_byte() as u8;
                        c[2] = self.read_byte() as u8;
                    }
                    c[3] = 8 * self.read_bits(5) as u8;
                }
                u32::from_le_bytes(c)
            }
            _ => {
                if self.read_bit() == 0 {
                    0
                } else {
                    let n = bits.unsigned_abs();
                    let mut low = n & 7;
                    let mut t = if low > 0 { self.read_bits(low) } else { 0 };
                    while low < n {
                        t |= self.read_byte() << low;
                        low += 8;
                    }
                    let mask = if n == 32 { u32::MAX } else { (1 << n) - 1 };
                    t ^= old & mask;
                    if bits < 0 && (t >> (n - 1)) & 1 == 1 {
                        t |= !mask;
                    }
                    t
                }
            }
        };
    }
}

// ------------------------------------------------------------------- parser

const PACKET_BACKUP: usize = 32;
const PARSE_RING: usize = 2048;
const MAX_CLIENTS: u32 = 64;

#[derive(Clone, Default)]
struct PlayerState {
    fields: Vec<u32>,
    objectives: Vec<(u32, Vec<u32>)>,
    hud: [Vec<Vec<u32>>; 2],
}

impl PlayerState {
    fn new() -> PlayerState {
        PlayerState {
            fields: vec![0; slots(Kind::Player).len],
            objectives: vec![(0, vec![0; slots(Kind::Objective).len]); 16],
            hud: std::array::from_fn(|_| vec![vec![0; slots(Kind::Hud).len]; 31]),
        }
    }
}

#[derive(Clone, Default)]
struct SnapRecord {
    valid: bool,
    message_num: i32,
    ps: PlayerState,
    parse_entities: usize,
    num_entities: usize,
    parse_clients: usize,
    num_clients: usize,
}

struct Parser {
    tables: Tables,
    huffman: huffman::Huffman,
    demo: Demo,
    baselines: Vec<Vec<u32>>,
    entities: Vec<Vec<u32>>,
    clients: Vec<Vec<u32>>,
    parse_entities: usize,
    parse_clients: usize,
    snaps: Vec<SnapRecord>,
    last_message: i32,
    server_config_sequence: i32,
    last_time: i32,
}

impl Parser {
    fn new() -> Parser {
        let e = slots(Kind::Entity).len;
        let c = slots(Kind::Client).len;
        Parser {
            tables: Tables::new(),
            huffman: huffman::Huffman::cod4(),
            demo: Demo::default(),
            baselines: vec![vec![0; e]; 1024],
            entities: vec![vec![0; e]; PARSE_RING],
            clients: vec![vec![0; c]; PARSE_RING],
            parse_entities: 0,
            parse_clients: 0,
            snaps: vec![SnapRecord::default(); PACKET_BACKUP],
            last_message: 0,
            server_config_sequence: 0,
            last_time: 0,
        }
    }

    /// Delta-read a struct whose first field may switch the field list (eType).
    /// Returns false if the delta says the entity or client was removed.
    fn read_delta_struct(
        &self,
        msg: &mut Msg,
        time: i32,
        from: &[u32],
        to: &mut Vec<u32>,
        number: u32,
        kind: Kind,
    ) -> bool {
        if msg.read_bit() == 1 {
            return false;
        }
        to.clear();
        to.extend_from_slice(from);
        to[0] = number;
        if msg.read_bit() == 0 {
            // Unchanged (including the number, as the engine copies it).
            to.copy_from_slice(from);
            return true;
        }
        let base: &Resolved = match kind {
            Kind::Entity => &self.tables.entity[0],
            _ => &self.tables.client,
        };
        // The engine sizes entity deltas for 61 fields.
        let total = if kind == Kind::Entity { 61 } else { base.len() as u32 };
        let changed = msg.read_bits(min_bits(total)) as usize;
        if changed > base.len() {
            msg.overflowed = true;
            return true;
        }
        msg.read_field(time, from, to, base[0], false);
        let list = if kind == Kind::Entity { &self.tables.entity[(to[base[0].0]).min(17) as usize] } else { base };
        for &f in list.iter().take(changed).skip(1) {
            msg.read_field(time, from, to, f, false);
        }
        true
    }

    fn parse_gamestate(&mut self, msg: &mut Msg) {
        msg.last_entity = -1;
        let _command_sequence = msg.read_int();
        loop {
            match msg.read_byte() {
                2 => {
                    let n = msg.read_int();
                    if !(0..2 * 4884).contains(&n) {
                        break;
                    }
                    for _ in 0..n {
                        let idx = msg.read_int();
                        let s = msg.read_string();
                        if idx >= 0 && !s.is_empty() {
                            self.demo.config_strings.insert(idx as usize, s);
                        }
                    }
                }
                3 => {
                    let num = msg.read_entity_index(10);
                    if !(0..1024).contains(&num) {
                        msg.overflowed = true;
                        return;
                    }
                    let null = vec![0; slots(Kind::Entity).len];
                    let mut state = Vec::new();
                    if self.read_delta_struct(msg, 0, &null, &mut state, num as u32, Kind::Entity) {
                        self.baselines[num as usize] = state;
                    }
                }
                11 => {
                    let client = msg.read_byte();
                    let name = msg.read_string();
                    let tag = msg.read_string();
                    if client < MAX_CLIENTS {
                        self.demo.name_changes.push((self.last_time, client, name.clone()));
                        self.demo.names.insert(client, (name, tag));
                    }
                }
                _ => break, // svc_EOF ends it
            }
            if msg.overflowed {
                return;
            }
        }
        self.server_config_sequence = msg.read_int();
        self.demo.client_num = msg.read_int();
        let _checksum_feed = msg.read_int();
        if self.demo.protocol == 18 {
            let _db_checksum_feed = msg.read_int();
        }
    }

    fn read_player_state(&self, msg: &mut Msg, time: i32, from: &PlayerState, to: &mut PlayerState) {
        *to = from.clone();
        let origin_and_velocity = msg.read_bit() == 1;
        let changed = msg.read_bits(min_bits(self.tables.player.len() as u32)) as usize;
        if changed > self.tables.player.len() {
            msg.overflowed = true;
            return;
        }
        for &f in &self.tables.player[..changed] {
            msg.read_field(time, &from.fields, &mut to.fields, f, origin_and_velocity);
        }
        // Stats.
        if msg.read_bit() == 1 {
            let which = msg.read_bits(5);
            for (bit, read) in [(1, 2), (2, 2), (4, 2), (8, 6), (16, 1)] {
                if which & bit != 0 {
                    match read {
                        2 => {
                            msg.read_short();
                        }
                        6 => {
                            msg.read_bits(6);
                        }
                        _ => {
                            msg.read_byte();
                        }
                    }
                }
            }
        }
        // Ammo, then ammo in clips: 16-bit change masks.
        let masks = |msg: &mut Msg, groups: usize| {
            for _ in 0..groups {
                if msg.read_bit() == 1 {
                    let mask = msg.read_short() as u32;
                    for i in 0..16 {
                        if mask & (1 << i) != 0 {
                            msg.read_short();
                        }
                    }
                }
            }
        };
        if msg.read_bit() == 1 {
            masks(msg, 4);
        }
        masks(msg, 8);
        // Objectives.
        if msg.read_bit() == 1 {
            for i in 0..16 {
                to.objectives[i].0 = msg.read_bits(3);
                if msg.read_bit() == 1 {
                    let mut o = from.objectives[i].1.clone();
                    for &f in &self.tables.objective {
                        msg.read_field(time, &from.objectives[i].1, &mut o, f, false);
                    }
                    to.objectives[i].1 = o;
                }
            }
        }
        // HUD elements, archival then current.
        if msg.read_bit() == 1 {
            for h in 0..2 {
                let in_use = msg.read_bits(5) as usize;
                for i in 0..in_use.min(31) {
                    let last = msg.read_bits(6) as usize;
                    if last >= self.tables.hud.len() {
                        msg.overflowed = true;
                        return;
                    }
                    let mut e = from.hud[h][i].clone();
                    for &f in &self.tables.hud[..=last] {
                        msg.read_field(time, &from.hud[h][i], &mut e, f, false);
                    }
                    to.hud[h][i] = e;
                }
                let type_slot = slots(Kind::Hud).index["type"];
                for e in to.hud[h].iter_mut().skip(in_use) {
                    if e[type_slot] == 0 {
                        break;
                    }
                    e.fill(0);
                }
            }
        }
        // Weapon models.
        if msg.read_bit() == 1 {
            for _ in 0..128 {
                msg.read_byte();
            }
        }
    }

    fn parse_snapshot(&mut self, msg: &mut Msg, message_num: i32) {
        let time = msg.read_int();
        let delta = msg.read_byte() as i32;
        let delta_num = if delta == 0 { -1 } else { message_num - delta };
        let _flags = msg.read_byte();
        let mut old = SnapRecord { ps: PlayerState::new(), ..Default::default() };
        if delta_num > 0 {
            let o = &self.snaps[delta_num as usize % PACKET_BACKUP];
            let ok = o.valid
                && o.message_num == delta_num
                && self.parse_entities - o.parse_entities <= 1920
                && self.parse_clients - o.parse_clients <= 1920;
            if !ok {
                msg.overflowed = true;
                return;
            }
            old = o.clone();
        }
        let mut snap = SnapRecord { valid: true, message_num, ps: PlayerState::new(), ..Default::default() };
        let from_ps = if old.valid { old.ps.clone() } else { PlayerState::new() };
        self.read_player_state(msg, time, &from_ps, &mut snap.ps);

        // Entities: merge the old snapshot's list with the changes.
        msg.last_entity = -1;
        snap.parse_entities = self.parse_entities;
        let mut old_i = 0;
        let old_at = |_: &Parser, i: usize| -> Option<usize> {
            (old.valid && i < old.num_entities).then(|| (old.parse_entities + i) % PARSE_RING)
        };
        let mut old_slot = old_at(self, 0);
        let num_of = |p: &Parser, s: Option<usize>| s.map_or(99999, |s| p.entities[s][0] as i32);
        loop {
            if msg.overflowed {
                break;
            }
            let new = msg.read_entity_index(10);
            if new == 1023 {
                break;
            }
            if msg.read > msg.data.len() || !(0..1024).contains(&new) {
                msg.overflowed = true;
                return;
            }
            while num_of(self, old_slot) < new && !msg.overflowed {
                let s = old_slot.expect("old entity");
                let copy = self.entities[s].clone();
                self.entities[self.parse_entities % PARSE_RING] = copy;
                self.parse_entities += 1;
                snap.num_entities += 1;
                old_i += 1;
                old_slot = old_at(self, old_i);
            }
            let from = if num_of(self, old_slot) == new {
                let s = old_slot.expect("old entity");
                old_i += 1;
                old_slot = old_at(self, old_i);
                self.entities[s].clone()
            } else {
                self.baselines[new as usize].clone()
            };
            let mut to = Vec::new();
            if self.read_delta_struct(msg, time, &from, &mut to, new as u32, Kind::Entity) {
                self.entities[self.parse_entities % PARSE_RING] = to;
                self.parse_entities += 1;
                snap.num_entities += 1;
            }
        }
        while let (Some(s), false) = (old_slot, msg.overflowed) {
            let copy = self.entities[s].clone();
            self.entities[self.parse_entities % PARSE_RING] = copy;
            self.parse_entities += 1;
            snap.num_entities += 1;
            old_i += 1;
            old_slot = old_at(self, old_i);
        }

        // Clients, the same way.
        msg.last_entity = -1;
        snap.parse_clients = self.parse_clients;
        let mut old_i = 0;
        let old_client = |i: usize| -> Option<usize> {
            (old.valid && i < old.num_clients).then(|| (old.parse_clients + i) % PARSE_RING)
        };
        let mut old_slot = old_client(0);
        let cnum = |p: &Parser, s: Option<usize>| s.map_or(99999, |s| p.clients[s][0] as i32);
        let null_client = vec![0; slots(Kind::Client).len];
        while !msg.overflowed && msg.read_bit() == 1 {
            let new = msg.read_entity_index(6);
            if msg.read > msg.data.len() || !(0..MAX_CLIENTS as i32).contains(&new) {
                msg.overflowed = true;
                return;
            }
            while cnum(self, old_slot) < new {
                let s = old_slot.expect("old client");
                let copy = self.clients[s].clone();
                self.clients[self.parse_clients % PARSE_RING] = copy;
                self.parse_clients += 1;
                snap.num_clients += 1;
                old_i += 1;
                old_slot = old_client(old_i);
            }
            let from = if cnum(self, old_slot) == new {
                let s = old_slot.expect("old client");
                old_i += 1;
                old_slot = old_client(old_i);
                self.clients[s].clone()
            } else {
                null_client.clone()
            };
            let mut to = Vec::new();
            if self.read_delta_struct(msg, time, &from, &mut to, new as u32, Kind::Client) {
                self.clients[self.parse_clients % PARSE_RING] = to;
                self.parse_clients += 1;
                snap.num_clients += 1;
            }
        }
        while let (Some(s), false) = (old_slot, msg.overflowed) {
            let copy = self.clients[s].clone();
            self.clients[self.parse_clients % PARSE_RING] = copy;
            self.parse_clients += 1;
            snap.num_clients += 1;
            old_i += 1;
            old_slot = old_client(old_i);
        }
        if msg.overflowed {
            return;
        }

        // Snapshots skipped over can't be deltas any more.
        let mut m = (self.last_message + 1).max(message_num - (PACKET_BACKUP as i32 - 1));
        while m < message_num {
            self.snaps[m.rem_euclid(PACKET_BACKUP as i32) as usize].valid = false;
            m += 1;
        }
        self.last_message = message_num;
        self.last_time = time;
        self.demo.snapshots.push(Snapshot {
            server_time: time,
            entities: (0..snap.num_entities)
                .map(|i| {
                    let v = &self.entities[(snap.parse_entities + i) % PARSE_RING];
                    Entity { number: v[0], values: v.clone() }
                })
                .collect(),
            clients: (0..snap.num_clients)
                .map(|i| {
                    let v = &self.clients[(snap.parse_clients + i) % PARSE_RING];
                    Client { number: v[0], values: v.clone() }
                })
                .collect(),
        });
        self.snaps[message_num.rem_euclid(PACKET_BACKUP as i32) as usize] = snap;
    }

    fn parse_message(&mut self, data: &[u8], message_num: i32) {
        let raw_origins = self.demo.protocol > 17;
        let mut msg = Msg { data, read: 0, bit: 0, overflowed: false, last_entity: -1, raw_origins };
        while msg.read < msg.data.len() && !msg.overflowed {
            match msg.read_byte() {
                1 => self.parse_gamestate(&mut msg),
                4 => {
                    let _seq = msg.read_int();
                    let s = msg.read_string();
                    self.demo.commands.push((self.last_time, s));
                }
                6 => self.parse_snapshot(&mut msg, message_num),
                11 => {
                    let seq = msg.read_int();
                    let client = msg.read_byte();
                    let name = msg.read_string();
                    let tag = msg.read_string();
                    if seq == self.server_config_sequence + 1 {
                        self.server_config_sequence = seq;
                        if client < MAX_CLIENTS {
                            self.demo.name_changes.push((self.last_time, client, name.clone()));
                            self.demo.names.insert(client, (name, tag));
                        }
                    }
                }
                _ => break, // svc_EOF, or something this reader doesn't know
            }
        }
    }
}

/// Read a whole demo file.
pub fn read(data: &[u8]) -> Result<Demo> {
    let mut p = Parser::new();
    let mut o = 0;
    let int = |o: usize| -> Option<i32> { data.get(o..o + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap())) };
    let float = |o: usize| f32::from_bits(int(o).unwrap_or(0) as u32);
    while o < data.len() {
        let kind = data[o];
        o += 1;
        match kind {
            0 => {
                let (Some(seq), Some(len)) = (int(o), int(o + 4)) else { break };
                if seq == -1 {
                    break;
                }
                let start = o + 12;
                let end = start + (len as usize).saturating_sub(4);
                let Some(payload) = data.get(start..end) else { break };
                let message = p.huffman.decompress(payload);
                p.parse_message(&message, seq);
                o = end;
            }
            1 => {
                if o + 52 > data.len() {
                    break;
                }
                p.demo.frames.push(Frame {
                    origin: [float(o + 4), float(o + 8), float(o + 12)],
                    velocity: [float(o + 16), float(o + 20), float(o + 24)],
                    movement_dir: int(o + 28).unwrap_or(0),
                    bob_cycle: int(o + 32).unwrap_or(0),
                    command_time: int(o + 36).unwrap_or(0),
                    angles: [float(o + 40), float(o + 44), float(o + 48)],
                });
                o += 52;
            }
            2 => {
                p.demo.protocol = int(o).unwrap_or(0) as u32;
                if p.demo.protocol < 17 {
                    bail!("demo protocol {} (stock CoD4) isn't supported yet, only CoD4X", p.demo.protocol);
                }
                o += 16;
            }
            other => bail!("unknown demo record type {other} at byte {}", o - 1),
        }
    }
    Ok(p.demo)
}
