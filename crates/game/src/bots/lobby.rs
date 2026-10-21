//! The people in a public lobby: the bots' names, who each one is and how
//! they play, kept for the whole session so the same names come back match
//! after match playing the same way, as regulars on a server do.
//!
//! A lobby is mostly ordinary players, with a few sweats, a few newcomers
//! and the odd camper. Each name slot gets one of those, a name in the
//! style that goes with it (newcomers often keep a default-looking name,
//! sweats wear a clan tag) and, at its first match, a real player's persona
//! of that kind ([`super::persona`]). Sniper rifles are held to about the
//! share real public lobbies have.

use super::GunKind;
use super::persona::Persona;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Regular,
    Sweat,
    Newcomer,
    Camper,
}

/// How a lobby is made up.
/// (Newcomers were 16% and sweats 14%: a good player won by double.)
const MIX: [(Kind, f32); 4] = [(Kind::Regular, 0.62), (Kind::Sweat, 0.18), (Kind::Newcomer, 0.1), (Kind::Camper, 0.1)];
/// At most this share of the lobby on sniper rifles.
const SNIPER_SHARE: f32 = 0.2;

struct Slot {
    name: String,
    kind: Kind,
    persona: Option<Persona>,
}

struct Lobby {
    slots: Vec<Slot>,
    by_name: HashMap<String, usize>,
    rng: StdRng,
}

static LOBBY: LazyLock<Mutex<Lobby>> = LazyLock::new(|| {
    // `COD4RW_LOBBY_SEED=<n>`: the same lobby every run (tests, A/B sims).
    let seed = std::env::var("COD4RW_LOBBY_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or_else(rand::random);
    Mutex::new(Lobby { slots: Vec::new(), by_name: HashMap::new(), rng: StdRng::seed_from_u64(seed) })
});

fn pick<T: Copy>(rng: &mut impl Rng, options: &[(T, f32)]) -> T {
    let total: f32 = options.iter().map(|o| o.1).sum();
    let mut r = rng.random::<f32>() * total;
    for &(v, w) in options {
        r -= w;
        if r <= 0.0 {
            return v;
        }
    }
    options[options.len() - 1].0
}

impl Lobby {
    fn grow(&mut self, n: usize) {
        while self.slots.len() <= n {
            let kind = pick(&mut self.rng, &MIX);
            let taken: Vec<String> = self.slots.iter().map(|s| s.name.to_ascii_lowercase()).collect();
            let name = (0..50).map(|_| gamer_tag(kind, &mut self.rng)).find(|n| !taken.contains(&n.to_ascii_lowercase())).unwrap_or_else(|| format!("Player{}", self.slots.len()));
            self.by_name.insert(name.clone(), self.slots.len());
            self.slots.push(Slot { name, kind, persona: None });
        }
    }
}

/// The `i`th name in this session's lobby.
pub fn name(i: usize) -> String {
    let mut l = LOBBY.lock().unwrap_or_else(|e| e.into_inner());
    l.grow(i);
    l.slots[i].name.clone()
}

/// Who the bot called `name` is: the same persona every match this
/// session (one of its slot's kind, near `skill`, the first time).
pub fn persona_for(name: &str, skill: f32) -> Option<Persona> {
    let mut guard = LOBBY.lock().unwrap_or_else(|e| e.into_inner());
    let l = &mut *guard;
    let Some(&i) = l.by_name.get(name) else {
        // Not one of ours (renamed, an old save): someone ordinary.
        return Persona::sample(skill, &mut rand::rng());
    };
    if let Some(p) = &l.slots[i].persona {
        return Some(p.clone());
    }
    let kind = l.slots[i].kind;
    let snipers = l.slots.iter().filter(|s| s.persona.as_ref().is_some_and(|p| p.kind == GunKind::Sniper)).count();
    let people = l.slots.iter().filter(|s| s.persona.is_some()).count() + 1;
    let room_for_sniper = (snipers + 1) as f32 <= (SNIPER_SHARE * people as f32).max(1.0);
    let fits = |p: &Persona| {
        let kind_ok = match kind {
            Kind::Regular => (0.2..0.85).contains(&p.skill),
            Kind::Sweat => p.skill > 0.75 && p.rank.0 >= 30,
            Kind::Newcomer => p.rank.0 < 15 && p.rank.1 == 0,
            Kind::Camper => p.camper(),
        };
        kind_ok && (room_for_sniper || p.kind != GunKind::Sniper)
    };
    // Sweats play above the lobby's setting, newcomers below.
    let aim = match kind {
        Kind::Sweat => (skill + 0.35).min(1.0),
        Kind::Newcomer => (skill - 0.35).max(0.0),
        _ => skill,
    };
    let mut best = None;
    for _ in 0..60 {
        let Some(p) = Persona::sample(aim, &mut l.rng) else { break };
        if fits(&p) {
            best = Some(p);
            break;
        }
        if best.is_none() && (room_for_sniper || p.kind != GunKind::Sniper) {
            best = Some(p);
        }
    }
    l.slots[i].persona = best.clone();
    best
}

/// The local player's skill as the balancer should count it (0..1), from
/// how their finished matches went; `None` before any (the setting stands).
/// Kept in `%LOCALAPPDATA%\cod4rw\human_rating.txt`.
pub fn human_rating() -> Option<f32> {
    let text = std::fs::read_to_string(rating_path()?).ok()?;
    text.trim().parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0))
}

/// A finished match's kills and deaths for the local player: the rating
/// moves halfway to what that K/D says (1 = 0.5, 3 = 0.70, 8 = 0.87).
pub fn rate_human(kills: u32, deaths: u32) {
    let Some(path) = rating_path() else { return };
    if kills + deaths < 5 {
        return;
    }
    let kd = (kills as f32 + 0.5) / (deaths as f32 + 0.5);
    let this = (0.5 + 0.18 * kd.ln()).clamp(0.05, 0.98);
    let new = human_rating().map_or(this, |old| 0.5 * old + 0.5 * this);
    std::fs::write(&path, format!("{new:.3}
")).ok();
    bevy::log::info!("lobby: your rating for balancing {new:.2} (this match {kills}/{deaths})");
}

fn rating_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)?;
    Some(base.join("cod4rw").join("human_rating.txt"))
}

/// The kind of player a lobby name is (for the blind test's answer sheet).
pub fn kind_of(name: &str) -> Option<Kind> {
    let l = LOBBY.lock().unwrap_or_else(|e| e.into_inner());
    l.by_name.get(name).map(|&i| l.slots[i].kind)
}

// ------------------------------------------------------------------ names

const WORDS: [&str; 40] = [
    "Wolf", "Viper", "Hawk", "Reaper", "Ghost", "Panda", "Tiger", "Bullet", "Hunter", "Knight", "Fox", "Bear", "Cobra", "Rider",
    "Duck", "Potato", "Llama", "Taco", "Shadow", "Storm", "Blade", "Raven", "Phantom", "Rogue", "Toaster", "Moose", "Spartan",
    "Nomad", "Pirate", "Badger", "Comet", "Rocket", "Dingo", "Gecko", "Jackal", "Mamba", "Otter", "Pickle", "Waffle", "Yeti",
];
const ADJECTIVES: [&str; 24] = [
    "Silent", "Dark", "Lone", "Crazy", "Toxic", "Frozen", "Rapid", "Lucky", "Mad", "Iron", "Night", "Red", "Swift", "Grim",
    "Wild", "Lazy", "Angry", "Sly", "Big", "Little", "Sneaky", "Salty", "Sleepy", "Rusty",
];
const FIRST: [&str; 24] = [
    "mike", "danny", "alex", "kev", "jonny", "sam", "nico", "marco", "tom", "luka", "ivan", "sergey", "pavel", "max", "leo",
    "dave", "chris", "matt", "andrei", "tomek", "jake", "ryan", "omar", "pete",
];
const SHORT: [&str; 16] = ["Vex", "Zyn", "Kryo", "Nox", "Jinx", "Axl", "Rift", "Hex", "Volt", "Zeal", "Onyx", "Flux", "Kade", "Riot", "Saint", "Envy"];

fn leet(s: &str, rng: &mut impl Rng) -> String {
    s.chars()
        .map(|c| match c {
            'a' | 'A' if rng.random::<f32>() < 0.5 => '4',
            'e' | 'E' if rng.random::<f32>() < 0.5 => '3',
            'o' | 'O' if rng.random::<f32>() < 0.5 => '0',
            'i' | 'I' if rng.random::<f32>() < 0.3 => '1',
            c => c,
        })
        .collect()
}

fn clan_tag(rng: &mut impl Rng) -> String {
    const LETTERS: &[u8] = b"ABCDEFGHKLMNPRSTVWXZ";
    let n = rng.random_range(2..=4);
    let tag: String = (0..n)
        .map(|k| {
            let c = LETTERS[rng.random_range(0..LETTERS.len())] as char;
            if k > 0 && rng.random::<f32>() < 0.4 { c.to_ascii_lowercase() } else { c }
        })
        .collect();
    match rng.random_range(0..3) {
        0 => format!("[{tag}]"),
        1 => format!("{{{tag}}}"),
        _ => format!("{tag}|"),
    }
}

/// A made-up name in a style that goes with the kind of player.
fn gamer_tag(kind: Kind, rng: &mut impl Rng) -> String {
    let w = |rng: &mut dyn rand::RngCore| WORDS[rng.random_range(0..WORDS.len())];
    let a = |rng: &mut dyn rand::RngCore| ADJECTIVES[rng.random_range(0..ADJECTIVES.len())];
    let f = |rng: &mut dyn rand::RngCore| FIRST[rng.random_range(0..FIRST.len())];
    let year = |rng: &mut dyn rand::RngCore| rng.random_range(1985..2010).to_string();
    let base = match rng.random_range(0..6) {
        0 => format!("{}{}", a(rng), w(rng)),
        1 => format!("{}_{}", f(rng), year(rng)),
        2 => format!("{}{}", w(rng), rng.random_range(1..999)),
        3 => SHORT[rng.random_range(0..SHORT.len())].to_string(),
        4 => format!("x{}x", w(rng)),
        _ => format!("{}{}", f(rng), ["", "_", "."][rng.random_range(0..3)].to_string() + w(rng)),
    };
    match kind {
        // Default-looking names, or a plain first name and year.
        Kind::Newcomer => match rng.random_range(0..5) {
            0 => "Unknown Soldier".to_string(),
            1 => format!("Player{}", rng.random_range(1..99)),
            2 => format!("{}{}", f(rng), rng.random_range(1..99)),
            _ => base.to_ascii_lowercase(),
        },
        // Clan tags and styled names.
        Kind::Sweat => {
            let n = if rng.random::<f32>() < 0.5 { leet(&base, rng) } else { base };
            if rng.random::<f32>() < 0.7 { format!("{}{n}", clan_tag(rng)) } else { n }
        }
        _ => {
            let n = if rng.random::<f32>() < 0.15 { leet(&base, rng) } else { base };
            if rng.random::<f32>() < 0.2 { format!("{}{n}", clan_tag(rng)) } else { n }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_stable_and_personas_persist() {
        let a = name(3);
        assert_eq!(a, name(3));
        assert!(!a.is_empty() && a.len() <= 31, "{a}");
        let p1 = persona_for(&a, 0.5).expect("persona");
        let p2 = persona_for(&a, 0.9).expect("persona");
        assert_eq!(p1.source, p2.source, "same person next match");
    }

    #[test]
    fn snipers_are_held_to_a_share() {
        let names: Vec<String> = (20..60).map(name).collect();
        let snipers = names.iter().filter_map(|n| persona_for(n, 0.5)).filter(|p| p.kind == GunKind::Sniper).count();
        assert!(snipers as f32 <= 0.25 * names.len() as f32 + 1.0, "{snipers} snipers of {}", names.len());
    }
}
