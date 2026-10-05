//! Supply drops, after *Advanced Warfare*'s: crates earned by playing (one
//! per [`DROP_INTERVAL`] of match time), opened from the main menu, each
//! holding three items: weapon variants and characters
//! ([`crate::characters`]). A variant is one of the game's guns (CoD4's,
//! and Black Ops' and World at War's when installed) under a name of its own
//! with its stats tweaked: Enlisted ones a little each way, Professional ones
//! more, Elite ones two ways better and with a signature camo where the gun
//! takes one (there are no Veteran variants: that rarity is all characters).
//! Each game's guns are as likely as another's, however many it has.
//! Variants are equipped per class weapon in Create a Class ([`crate::ui`])
//! and change the weapon's real stats in matches ([`crate::loadout`]).
//!
//! The collection is kept in `%LOCALAPPDATA%\cod4rw\supply.txt`. Debug runs
//! (any `COD4RW_*` variable) neither read nor write it: they can read a
//! test collection from `COD4RW_SUPPLYFILE=<file>`, start with n drops to
//! open with `COD4RW_SUPPLYDROPS=<n>`, and earn one every
//! `COD4RW_SUPPLYTIME=<seconds>`.

use crate::characters::{self, Character};
use crate::loadout::AwaitingClass;
use crate::player::LocalPlayer;
use crate::tdm::MatchState;
use crate::weapons::WeaponDef;
use bevy::prelude::*;
use rand::Rng;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard};

pub struct SupplyPlugin;

impl Plugin for SupplyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(crate::state::GameState::InGame), spawn_notice.in_set(crate::state::Setup::Spawn))
            .add_systems(Update, (earn, show_notice).chain().run_if(crate::state::in_game));
    }
}

/// Match time it takes to earn a drop: one TDM match.
pub const DROP_INTERVAL: f32 = 10.0 * 60.0;
/// Items in a drop.
pub const DROP_SIZE: usize = 3;
/// The chance of an item of a rarity with weapon variants being a
/// character instead.
const CHARACTER_SHARE: f64 = 0.3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rarity {
    Enlisted,
    Veteran,
    Professional,
    Elite,
}

pub const RARITIES: [Rarity; 4] = [Rarity::Enlisted, Rarity::Veteran, Rarity::Professional, Rarity::Elite];

impl Rarity {
    pub fn name(self) -> &'static str {
        match self {
            Rarity::Enlisted => "ENLISTED",
            Rarity::Veteran => "VETERAN",
            Rarity::Professional => "PROFESSIONAL",
            Rarity::Elite => "ELITE",
        }
    }

    pub fn color(self) -> [f32; 4] {
        match self {
            Rarity::Enlisted => [0.45, 0.85, 0.35, 1.0],
            Rarity::Veteran => [0.35, 0.6, 1.0, 1.0],
            Rarity::Professional => [0.72, 0.42, 1.0, 1.0],
            Rarity::Elite => [1.0, 0.68, 0.15, 1.0],
        }
    }

    /// The chance of each item in a drop being this rare.
    pub fn chance(self) -> f64 {
        match self {
            Rarity::Enlisted => 0.55,
            Rarity::Veteran => 0.22,
            Rarity::Professional => 0.15,
            Rarity::Elite => 0.08,
        }
    }

    /// What its items are (see [`generate`] and [`crate::characters`]).
    pub fn summary(self) -> &'static str {
        match self {
            Rarity::Enlisted => "guns +1/-1, soldiers",
            Rarity::Veteran => "ghillies, NVG, pilots",
            Rarity::Professional => "guns +2/-1, side cast",
            Rarity::Elite => "guns +2+1/-1, leads",
        }
    }
}

/// Something a drop holds.
#[derive(Clone, Copy)]
pub enum Item {
    Variant(&'static Variant),
    Character(&'static Character),
}

impl Item {
    pub fn rarity(self) -> Rarity {
        match self {
            Item::Variant(v) => v.rarity,
            Item::Character(c) => c.rarity,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Item::Variant(v) => &v.id,
            Item::Character(c) => c.id,
        }
    }
}

/// The weapon qualities a variant tweaks, as Advanced Warfare lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attribute {
    Damage,
    Accuracy,
    Range,
    FireRate,
    Handling,
    Mobility,
}

const ATTRIBUTES: [Attribute; 6] =
    [Attribute::Damage, Attribute::Accuracy, Attribute::Range, Attribute::FireRate, Attribute::Handling, Attribute::Mobility];

impl Attribute {
    pub fn name(self) -> &'static str {
        match self {
            Attribute::Damage => "DAMAGE",
            Attribute::Accuracy => "ACCURACY",
            Attribute::Range => "RANGE",
            Attribute::FireRate => "FIRE RATE",
            Attribute::Handling => "HANDLING",
            Attribute::Mobility => "MOBILITY",
        }
    }
}

pub struct Variant {
    /// `ak47/viper`, `t5_ak47/...`: what the collection file stores.
    pub id: String,
    /// The weapon (`mp/statstable.csv` column 4): `ak47`, `t5_ak47`,
    /// `t4_thompson`.
    pub weapon: &'static str,
    pub name: &'static str,
    pub rarity: Rarity,
    /// Attributes and how many steps up (or down) each goes.
    pub tweaks: Vec<(Attribute, i32)>,
    /// Elite variants' signature camo (`mp/attachmenttable.csv` column 11),
    /// worn unless the class picks one; 0 for none.
    pub camo: usize,
}

/// The games guns come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GunGame {
    Cod4,
    BlackOps,
    WorldAtWar,
}

impl GunGame {
    fn of(weapon: &str) -> GunGame {
        if crate::bo1::is_bo1(weapon) {
            GunGame::BlackOps
        } else if crate::waw::is_waw(weapon) {
            GunGame::WorldAtWar
        } else {
            GunGame::Cod4
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            GunGame::Cod4 => "CALL OF DUTY 4",
            GunGame::BlackOps => "BLACK OPS",
            GunGame::WorldAtWar => "WORLD AT WAR",
        }
    }
}

impl Variant {
    pub fn game(&self) -> GunGame {
        GunGame::of(self.weapon)
    }

    /// "+DAMAGE", "++RANGE", "-MOBILITY".
    pub fn tweak_texts(&self) -> Vec<(String, bool)> {
        self.tweaks
            .iter()
            .map(|&(a, s)| {
                let sign = if s > 0 { "+" } else { "-" };
                (format!("{}{}", sign.repeat(s.unsigned_abs() as usize), a.name()), s > 0)
            })
            .collect()
    }
}

/// CoD4's guns that come in variants, and whether they take camos.
const WEAPONS: [(&str, bool); 26] = [
    ("beretta", false),
    ("colt45", false),
    ("usp", false),
    ("deserteagle", false),
    ("mp5", true),
    ("skorpion", true),
    ("uzi", true),
    ("ak74u", true),
    ("p90", true),
    ("ak47", true),
    ("m14", true),
    ("mp44", true),
    ("g3", true),
    ("g36c", true),
    ("m16", true),
    ("m4", true),
    ("dragunov", true),
    ("m40a3", true),
    ("barrett", true),
    ("remington700", true),
    ("m21", true),
    ("m1014", true),
    ("winchester1200", true),
    ("rpd", true),
    ("saw", true),
    ("m60e4", true),
];

/// Variant names. Each gun's come from a shuffle of this list seeded by the
/// gun, so changing it renames (and orphans) variants already collected.
const NAMES: [&str; 48] = [
    "Viper", "Nomad", "Havoc", "Specter", "Warden", "Talon", "Vandal", "Outlaw", "Reaper", "Sentinel", "Cobra", "Raptor",
    "Brute", "Phantom", "Jackal", "Marauder", "Banshee", "Hellhound", "Striker", "Tempest", "Ironside", "Wraith",
    "Grizzly", "Mongoose", "Hornet", "Bulldog", "Saber", "Vulture", "Rattler", "Maverick", "Typhoon", "Bastion",
    "Ravager", "Nightjar", "Kestrel", "Warthog", "Tundra", "Dustdevil", "Ember", "Glacier", "Harbinger", "Longbow",
    "Hammer", "Anvil", "Razor", "Widow", "Monarch", "Sidewinder",
];

/// Each gun's variants, in order.
const VARIANT_RARITIES: [Rarity; 6] =
    [Rarity::Enlisted, Rarity::Enlisted, Rarity::Professional, Rarity::Professional, Rarity::Professional, Rarity::Elite];

/// splitmix64, so every run (and build) generates the same variants.
struct Seq(u64);

impl Seq {
    fn new(seed: &str) -> Seq {
        // FNV-1a.
        Seq(seed.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3)))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i + 1));
        }
    }
}

/// The guns that come in variants, each with the camos its Elite variant's
/// signature one is picked from (none for guns without camos): CoD4's
/// [`WEAPONS`], then Black Ops' and World at War's from their installs
/// (World at War's multiplayer has no camos).
fn guns() -> Vec<(&'static str, Vec<usize>)> {
    // CoD4's five standard camos (1..=5).
    let mut out: Vec<(&'static str, Vec<usize>)> =
        WEAPONS.iter().map(|&(w, camos)| (w, if camos { (1..=5).collect() } else { Vec::new() })).collect();
    // Names made at run time, once.
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    if let Some(d) = crate::bo1::data() {
        let camos: Vec<usize> = d.camos.iter().map(|c| c.index).filter(|&i| i > 0).map(crate::bo1::camo_stat).collect();
        for g in &d.guns {
            // Black Ops' pistols take no camo.
            let camos = if g.group == crate::bo1::Group::Pistol { Vec::new() } else { camos.clone() };
            out.push((leak(g.id()), camos));
        }
    }
    if let Some(d) = crate::waw::data() {
        out.extend(d.guns.iter().map(|g| (leak(g.id()), Vec::new())));
    }
    out
}

fn generate() -> Vec<Variant> {
    let mut out = Vec::new();
    // Each gun's variants come from a sequence seeded by its name alone, so
    // adding guns leaves the others' as they were.
    for (weapon, camos) in guns() {
        let mut seq = Seq::new(weapon);
        let mut names = NAMES;
        seq.shuffle(&mut names);
        // Each variant is best at something different.
        let mut strengths = ATTRIBUTES;
        seq.shuffle(&mut strengths);
        for (k, rarity) in VARIANT_RARITIES.into_iter().enumerate() {
            let plus = strengths[k];
            let others: Vec<Attribute> = ATTRIBUTES.into_iter().filter(|a| *a != plus).collect();
            let minus = others[seq.below(others.len())];
            let tweaks = match rarity {
                Rarity::Enlisted => vec![(plus, 1), (minus, -1)],
                Rarity::Veteran | Rarity::Professional => vec![(plus, 2), (minus, -1)],
                Rarity::Elite => {
                    let rest: Vec<Attribute> = others.iter().copied().filter(|a| *a != minus).collect();
                    vec![(plus, 2), (rest[seq.below(rest.len())], 1), (minus, -1)]
                }
            };
            let camo = if rarity == Rarity::Elite && !camos.is_empty() { camos[seq.below(camos.len())] } else { 0 };
            let name = names[k];
            out.push(Variant { id: format!("{weapon}/{}", name.to_ascii_lowercase()), weapon, name, rarity, tweaks, camo });
        }
    }
    out
}

static CATALOGUE: LazyLock<Vec<Variant>> = LazyLock::new(generate);

pub fn catalogue_size() -> usize {
    CATALOGUE.len()
}

pub fn variant(id: &str) -> Option<&'static Variant> {
    CATALOGUE.iter().find(|v| v.id.eq_ignore_ascii_case(id))
}

/// Change a weapon def by a variant's tweaks. One step is a few percent:
/// 6% damage, 12% range, 6% fire rate, 10% spread and kick, 8% faster
/// handling (aiming, raising, reloading), 4% move speed.
pub fn apply(d: &mut WeaponDef, v: &Variant) {
    for &(attribute, steps) in &v.tweaks {
        let s = steps as f32;
        // Up for amounts, down for times and spreads.
        let more = |k: f32| 1.0 + k * s;
        let less = |k: f32| 1.0 / (1.0 + k * s);
        match attribute {
            Attribute::Damage => {
                d.damage *= more(0.06);
                d.min_damage *= more(0.06);
            }
            Attribute::Range => {
                d.max_damage_range *= more(0.12);
                d.min_damage_range *= more(0.12);
            }
            Attribute::FireRate => d.fire_time *= less(0.06),
            Attribute::Accuracy => {
                let k = less(0.10);
                d.hip_spread_min = d.hip_spread_min.map(|x| x * k);
                d.hip_spread_max = d.hip_spread_max.map(|x| x * k);
                d.ads_spread *= k;
                for kick in [&mut d.kick_pitch, &mut d.kick_yaw, &mut d.ads_kick_pitch, &mut d.ads_kick_yaw] {
                    kick.0 *= k;
                    kick.1 *= k;
                }
            }
            Attribute::Handling => {
                let k = less(0.08);
                for t in [
                    &mut d.ads_trans_in,
                    &mut d.ads_trans_out,
                    &mut d.raise_time,
                    &mut d.drop_time,
                    &mut d.first_raise_time,
                    &mut d.reload_time,
                    &mut d.reload_empty_time,
                    &mut d.reload_start_time,
                    &mut d.reload_start_add_time,
                    &mut d.reload_add_time,
                    &mut d.reload_end_time,
                    &mut d.sprint_out_time,
                ] {
                    *t *= k;
                }
            }
            Attribute::Mobility => {
                d.move_speed_scale *= more(0.04);
                d.ads_move_speed_scale *= more(0.04);
            }
        }
    }
}

/// The player's drops and variants.
#[derive(Default)]
pub struct Inventory {
    /// Drops waiting to be opened.
    pub unopened: u32,
    /// Match time played towards the next drop, in seconds.
    pub progress: f32,
    /// Variant id -> how many.
    owned: BTreeMap<String, u32>,
    /// Class weapon stat (201 class 1 primary, 203 its secondary, ...) ->
    /// variant id.
    equipped: BTreeMap<i32, String>,
    /// Character ids (besides [`characters::DEFAULT`]).
    characters: BTreeSet<String>,
    /// The character worn.
    wearing: Option<String>,
    /// What the last drop opened held, each with whether it was new.
    pub last_drop: Vec<(Item, bool)>,
    path: Option<PathBuf>,
    dirty: bool,
}

/// Variants to keep in the collection: those in the catalogue, and Black Ops'
/// and World at War's while the game isn't installed (so they're still there
/// when it is again).
fn kept(id: &str) -> bool {
    variant(id).is_some() || crate::bo1::is_bo1(id) || crate::waw::is_waw(id)
}

fn debug_run() -> bool {
    std::env::vars().any(|(k, _)| k.starts_with("COD4RW_") && !crate::net::setting(&k))
}

impl Inventory {
    fn load() -> Inventory {
        let mut inv = Inventory::default();
        if debug_run() {
            // Read-only test collections.
            if let Some(text) = std::env::var_os("COD4RW_SUPPLYFILE").and_then(|p| std::fs::read_to_string(p).ok()) {
                inv.read(&text);
            }
            if let Some(n) = std::env::var("COD4RW_SUPPLYDROPS").ok().and_then(|n| n.parse().ok()) {
                inv.unopened = n;
            }
            return inv;
        }
        inv.path = std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("XDG_DATA_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("cod4rw").join("supply.txt"));
        if let Some(text) = inv.path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            inv.read(&text);
        }
        inv
    }

    fn read(&mut self, text: &str) {
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts[..] {
                ["unopened", n] => self.unopened = n.parse().unwrap_or(0),
                ["progress", s] => self.progress = s.parse().unwrap_or(0.0),
                ["own", id, n] if kept(id) => {
                    self.owned.insert(id.to_owned(), n.parse().unwrap_or(1));
                }
                ["equip", stat, id] if kept(id) => {
                    if let Ok(stat) = stat.parse() {
                        self.equipped.insert(stat, id.to_owned());
                    }
                }
                ["character", id] if characters::character(id).is_some() => {
                    self.characters.insert(id.to_owned());
                }
                ["wear", id] if characters::character(id).is_some() => self.wearing = Some(id.to_owned()),
                _ => {}
            }
        }
    }

    pub fn save_if_changed(&mut self) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        let Some(path) = &self.path else { return };
        let mut text = format!("unopened {}\nprogress {:.0}\n", self.unopened, self.progress);
        for (id, n) in &self.owned {
            text += &format!("own {id} {n}\n");
        }
        for (stat, id) in &self.equipped {
            text += &format!("equip {stat} {id}\n");
        }
        for id in &self.characters {
            text += &format!("character {id}\n");
        }
        if let Some(id) = &self.wearing {
            text += &format!("wear {id}\n");
        }
        let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(path, text));
        if let Err(e) = saved {
            warn!("supply drops: can't save to {}: {e}", path.display());
        }
    }

    /// Open a drop: three items, each by rarity, then whether it is a
    /// character (always for Veteran), then which one. Characters not in the
    /// collection yet come first. Each is paired with whether it is new.
    pub fn open(&mut self) -> Option<&[(Item, bool)]> {
        if self.unopened == 0 {
            return None;
        }
        self.unopened -= 1;
        let mut rng = rand::rng();
        self.last_drop.clear();
        for _ in 0..DROP_SIZE {
            let mut roll: f64 = rng.random();
            let rarity = RARITIES
                .into_iter()
                .rev()
                .find(|r| {
                    roll -= r.chance();
                    roll < 0.0
                })
                .unwrap_or(Rarity::Enlisted);
            let variants: Vec<&'static Variant> = CATALOGUE.iter().filter(|v| v.rarity == rarity).collect();
            if variants.is_empty() || rng.random_bool(CHARACTER_SHARE) {
                let pool: Vec<&'static Character> = characters::droppable(rarity).collect();
                let new: Vec<&'static Character> = pool.iter().copied().filter(|c| !self.characters.contains(c.id)).collect();
                let from = if new.is_empty() { &pool } else { &new };
                let c = from[rng.random_range(0..from.len())];
                let fresh = self.characters.insert(c.id.to_owned());
                self.last_drop.push((Item::Character(c), fresh));
            } else {
                // A game, then one of its guns: each game's as likely as
                // another's.
                let mut games: Vec<GunGame> = Vec::new();
                for v in &variants {
                    if !games.contains(&v.game()) {
                        games.push(v.game());
                    }
                }
                let game = games[rng.random_range(0..games.len())];
                let of_game: Vec<&'static Variant> = variants.iter().copied().filter(|v| v.game() == game).collect();
                let v = of_game[rng.random_range(0..of_game.len())];
                let count = self.owned.entry(v.id.clone()).or_insert(0);
                *count += 1;
                self.last_drop.push((Item::Variant(v), *count == 1));
            }
        }
        info!("supply drop: {:?}", self.last_drop.iter().map(|(i, _)| i.id()).collect::<Vec<_>>());
        self.dirty = true;
        Some(&self.last_drop)
    }

    /// The variants of a gun in the collection, best first.
    pub fn owned_of(&self, weapon: &str) -> Vec<&'static Variant> {
        let mut out: Vec<&'static Variant> = self.owned.keys().filter_map(|id| variant(id)).filter(|v| v.weapon == weapon).collect();
        out.sort_by(|a, b| b.rarity.cmp(&a.rarity).then(a.name.cmp(b.name)));
        out
    }

    pub fn owned_count(&self) -> usize {
        self.owned.keys().filter(|id| variant(id).is_some()).count()
    }

    /// The variant on a class weapon, if it is still for the gun there.
    pub fn equipped(&self, weapon_stat: i32, weapon: &str) -> Option<&'static Variant> {
        variant(self.equipped.get(&weapon_stat)?).filter(|v| v.weapon == weapon && self.owned.contains_key(&v.id))
    }

    /// The characters in the collection, best first, the starting one last.
    pub fn characters_owned(&self) -> Vec<&'static Character> {
        let mut out: Vec<&'static Character> = self.characters.iter().filter_map(|id| characters::character(id)).collect();
        out.sort_by(|a, b| b.rarity.cmp(&a.rarity).then(a.name.cmp(b.name)));
        out.extend(characters::character(characters::DEFAULT));
        out
    }

    pub fn owns_character(&self, id: &str) -> bool {
        id == characters::DEFAULT || self.characters.contains(id)
    }

    /// The character worn ([`characters::DEFAULT`] until another is picked).
    pub fn wearing(&self) -> &'static Character {
        self.wearing
            .as_deref()
            .filter(|id| self.owns_character(id))
            .and_then(characters::character)
            .or_else(|| characters::character(characters::DEFAULT))
            .expect("the default character")
    }

    pub fn wear(&mut self, id: &str) {
        if self.owns_character(id) && self.wearing.as_deref() != Some(id) {
            self.wearing = Some(id.to_owned());
            self.dirty = true;
        }
    }

    /// Put a variant on a class weapon (`None`: the standard issue gun).
    pub fn equip(&mut self, weapon_stat: i32, id: Option<&str>) {
        let old = match id.filter(|id| self.owned.contains_key(*id)) {
            Some(id) => self.equipped.insert(weapon_stat, id.to_owned()),
            None => self.equipped.remove(&weapon_stat),
        };
        if old.as_deref() != id {
            self.dirty = true;
        }
    }
}

static INVENTORY: LazyLock<Mutex<Inventory>> = LazyLock::new(|| Mutex::new(Inventory::load()));

/// The collection, shared by the menus and the match.
pub fn inventory() -> MutexGuard<'static, Inventory> {
    INVENTORY.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn drop_interval() -> f32 {
    static INTERVAL: LazyLock<f32> =
        LazyLock::new(|| std::env::var("COD4RW_SUPPLYTIME").ok().and_then(|s| s.parse().ok()).unwrap_or(DROP_INTERVAL));
    *INTERVAL
}

/// "6:12".
pub fn format_time(secs: f32) -> String {
    let s = secs.max(0.0).ceil() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

#[derive(Component)]
struct Notice {
    shown_at: Option<f32>,
}

#[derive(Component)]
struct NoticeText;

/// How long "Supply Drop Earned" stays up, fading in and out.
const NOTICE_TIME: f32 = 6.0;

fn spawn_notice(mut commands: Commands) {
    commands
        .spawn((
            Notice { shown_at: None },
            Node {
                position_type: PositionType::Absolute,
                top: percent(18),
                width: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                ..default()
            },
            Visibility::Hidden,
        ))
        .with_children(|p| {
            for (text, size) in [("SUPPLY DROP EARNED", 34.0), ("Open it from the main menu", 17.0)] {
                p.spawn((
                    NoticeText,
                    Text::new(text),
                    TextFont { font_size: FontSize::Px(size), ..default() },
                    TextColor(Color::NONE),
                    TextShadow::default(),
                ));
            }
        });
}

/// Count match time towards the next drop while the local player is in
/// the match, and award it.
fn earn(
    time: Res<Time>,
    state: Option<Res<MatchState>>,
    player: Query<(), (With<LocalPlayer>, Without<AwaitingClass>)>,
    mut notice: Query<&mut Notice>,
    mut since_save: Local<f32>,
) {
    if player.is_empty() || state.is_none_or(|s| s.ended.is_some()) {
        return;
    }
    let dt = time.delta_secs();
    let mut inv = inventory();
    inv.progress += dt;
    *since_save += dt;
    let interval = drop_interval();
    if inv.progress >= interval {
        // One at a time, even after a long time saved up (a shorter
        // interval than the file was saved with).
        inv.progress = (inv.progress - interval) % interval;
        inv.unopened += 1;
        info!("supply drop earned ({} to open)", inv.unopened);
        for mut n in &mut notice {
            n.shown_at = Some(time.elapsed_secs());
        }
        *since_save = 30.0;
    }
    if *since_save >= 30.0 {
        *since_save = 0.0;
        inv.dirty = true;
        inv.save_if_changed();
    }
}

fn show_notice(
    time: Res<Time>,
    mut notice: Query<(&mut Notice, &mut Visibility)>,
    mut texts: Query<(&mut TextColor, &TextFont), With<NoticeText>>,
) {
    let Ok((mut n, mut vis)) = notice.single_mut() else { return };
    let Some(at) = n.shown_at else { return };
    let t = time.elapsed_secs() - at;
    if t > NOTICE_TIME {
        n.shown_at = None;
        *vis = Visibility::Hidden;
        return;
    }
    *vis = Visibility::Inherited;
    let alpha = (t / 0.3).min((NOTICE_TIME - t) / 0.8).clamp(0.0, 1.0);
    let [r, g, b, _] = Rarity::Elite.color();
    for (mut color, font) in &mut texts {
        let title = matches!(font.font_size, FontSize::Px(s) if s > 20.0);
        color.0 = if title { Color::srgba(r, g, b, alpha) } else { Color::srgba(1.0, 1.0, 1.0, alpha * 0.85) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_stable_and_distinct() {
        let guns = guns();
        assert_eq!(CATALOGUE.len(), guns.len() * VARIANT_RARITIES.len());
        for (weapon, _) in guns {
            let mine: Vec<&Variant> = CATALOGUE.iter().filter(|v| v.weapon == weapon).collect();
            let mut names: Vec<&str> = mine.iter().map(|v| v.name).collect();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), VARIANT_RARITIES.len(), "{weapon}: duplicate names");
            for v in &mine {
                let (plus, minus): (Vec<_>, Vec<_>) = v.tweaks.iter().partition(|(_, s)| *s > 0);
                assert!(!plus.is_empty() && minus.len() == 1, "{}: {:?}", v.id, v.tweaks);
                assert!(plus.iter().all(|(a, _)| *a != minus[0].0), "{}: up and down at once", v.id);
            }
        }
        // Same seed, same catalogue.
        let again = generate();
        assert!(again.iter().zip(CATALOGUE.iter()).all(|(a, b)| a.id == b.id && a.tweaks == b.tweaks && a.camo == b.camo));
        // CoD4's are as they were before other games' guns joined.
        let ak = variant("ak47/viper").or_else(|| CATALOGUE.iter().find(|v| v.weapon == "ak47"));
        assert!(ak.is_some_and(|v| v.game() == GunGame::Cod4));
        let elite: Vec<&Variant> = CATALOGUE.iter().filter(|v| v.rarity == Rarity::Elite && v.game() == GunGame::Cod4).collect();
        assert!(elite.iter().all(|v| v.camo <= 5));
        // Other games' Elite camos are theirs (Black Ops' from 100 up; World
        // at War has none).
        for v in CATALOGUE.iter().filter(|v| v.rarity == Rarity::Elite) {
            match v.game() {
                GunGame::BlackOps => assert!(v.camo == 0 || v.camo > crate::bo1::CAMO_STAT_BASE, "{}", v.id),
                GunGame::WorldAtWar => assert_eq!(v.camo, 0, "{}", v.id),
                GunGame::Cod4 => {}
            }
        }
    }

    #[test]
    fn games_are_equally_likely() {
        let mut inv = Inventory { unopened: 3000, ..default() };
        let mut counts = [0usize; 3];
        while let Some(items) = inv.open() {
            for (item, _) in items {
                if let Item::Variant(v) = item {
                    counts[v.game() as usize] += 1;
                }
            }
        }
        let games = [true, crate::bo1::data().is_some(), crate::waw::data().is_some()];
        let total: usize = counts.iter().sum();
        let share = total as f32 / games.iter().filter(|g| **g).count() as f32;
        for (k, &installed) in games.iter().enumerate() {
            if installed {
                assert!((counts[k] as f32 - share).abs() < share * 0.15, "{counts:?}");
            } else {
                assert_eq!(counts[k], 0);
            }
        }
    }

    #[test]
    fn tweaks_move_the_right_way() {
        let base = WeaponDef::fallback();
        let v = Variant {
            id: "ak47/test".into(),
            weapon: "ak47",
            name: "Test",
            rarity: Rarity::Professional,
            tweaks: vec![(Attribute::Damage, 2), (Attribute::Handling, -1)],
            camo: 0,
        };
        let mut d = base.clone();
        apply(&mut d, &v);
        assert!(d.damage > base.damage && d.min_damage > base.min_damage);
        assert!(d.reload_time > base.reload_time && d.ads_trans_in > base.ads_trans_in);
        assert_eq!(d.fire_time, base.fire_time);
    }

    #[test]
    fn opening_and_equipping() {
        let mut inv = Inventory { unopened: 40, ..default() };
        let mut got: Vec<Item> = Vec::new();
        while let Some(items) = inv.open() {
            assert_eq!(items.len(), DROP_SIZE);
            got.extend(items.iter().map(|(i, _)| *i));
        }
        assert_eq!(got.len(), 40 * DROP_SIZE);
        let v = got.iter().find_map(|i| match i {
            Item::Variant(v) => Some(*v),
            _ => None,
        });
        let v = v.expect("a variant in 120 items");
        inv.equip(201, Some(&v.id));
        assert_eq!(inv.equipped(201, v.weapon).map(|e| &e.id), Some(&v.id));
        // A different gun in the slot: the variant no longer applies.
        assert!(inv.equipped(201, "not_a_gun").is_none());
        // Variants not in the collection can't be equipped.
        inv.equip(203, Some("ak47/nothing"));
        assert!(inv.equipped(203, "ak47").is_none());
        // Characters: the starting one until another collected one is worn.
        assert_eq!(inv.wearing().id, characters::DEFAULT);
        let c = got.iter().find_map(|i| match i {
            Item::Character(c) => Some(*c),
            _ => None,
        });
        let c = c.expect("a character in 120 items");
        inv.wear("not_a_character");
        assert_eq!(inv.wearing().id, characters::DEFAULT);
        inv.wear(c.id);
        assert_eq!(inv.wearing().id, c.id);
        assert!(inv.characters_owned().iter().any(|o| o.id == characters::DEFAULT));
    }
}
