//! Personas: every bot plays like one real player from a public CoD4X
//! match. `personas.csv` holds one row per real player-match, measured from
//! demos by `tools/demo_traits.py --export`: how much they sprinted, stood
//! still and held angles, how they closed on enemies, prefired, scoped in,
//! threw grenades, what they carried, their rank. A bot takes one whole row
//! (so habits that go together stay together: SMG, sprinting everywhere and
//! no utility), varied a little, then plays by it.
//!
//! The row's rank is the bot's experience: novices carry what CoD4 has
//! unlocked at their level (default classes before Create-a-Class), and know
//! the map less ([`Persona::knowledge`]).
//!
//! Rows are weighted so sniper-heavy recorded lobbies don't make every
//! lobby sniper-heavy; their players still count as sniper personas.

use super::GunKind;
use crate::units::u;
use rand::Rng;
use std::sync::LazyLock;

const TABLE: &str = include_str!("personas.csv");

/// CoD4's unlock rank (0-based, `mp/rankTable.csv`) for guns and perks not
/// there from the start.
const UNLOCKS: [(&str, i32); 30] = [
    ("m21", 6),
    ("m4", 9),
    ("uzi", 12),
    ("colt45", 15),
    ("m60e4", 18),
    ("dragunov", 21),
    ("g3", 24),
    ("ak74u", 27),
    ("m1014", 30),
    ("remington700", 33),
    ("g36c", 36),
    ("p90", 39),
    ("deserteagle", 42),
    ("m14", 45),
    ("barrett", 48),
    ("mp44", 51),
    ("deserteaglegold", 54),
    ("specialty_pistoldeath", 7),
    ("specialty_gpsjammer", 10),
    ("specialty_detectexplosive", 13),
    ("specialty_grenadepulldeath", 16),
    ("specialty_fastreload", 19),
    ("specialty_weapon_claymore", 22),
    ("specialty_holdbreath", 25),
    ("specialty_rof", 28),
    ("specialty_extraammo", 31),
    ("specialty_parabolic", 34),
    ("specialty_twoprimaries", 37),
    ("specialty_fraggrenade", 40),
    ("specialty_quieter", 43),
];

/// See [`Persona::reaction`]: set to their first shot + 0.1, bots measured
/// 0.45 s against real 0.34 (Crossfire TDM sim, 2026-10-18), so a little
/// under the real figure.
const REACTION_GAP: f32 = -0.05;

/// Create-a-Class opens at this rank (0-based): below it, default classes.
const CREATE_A_CLASS: i32 = 3;

/// Bots get personas unless `COD4RW_PERSONAS=0` (the old random
/// personalities, for A/B tests).
pub fn enabled() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("COD4RW_PERSONAS").map_or(true, |v| v != "0"));
    *ON
}

fn unlock_rank(item: &str) -> i32 {
    UNLOCKS.iter().find(|u| u.0 == item).map_or(0, |u| u.1)
}

/// CoD4's perk tiers.
const PERK1: [&str; 7] = [
    "specialty_weapon_c4",
    "specialty_specialgrenade",
    "specialty_weapon_rpg",
    "specialty_weapon_claymore",
    "specialty_fraggrenade",
    "specialty_extraammo",
    "specialty_detectexplosive",
];
const PERK2: [&str; 7] = [
    "specialty_bulletdamage",
    "specialty_armorvest",
    "specialty_fastreload",
    "specialty_rof",
    "specialty_twoprimaries",
    "specialty_gpsjammer",
    "specialty_explosivedamage",
];
const PERK3: [&str; 7] = [
    "specialty_longersprint",
    "specialty_bulletaccuracy",
    "specialty_pistoldeath",
    "specialty_grenadepulldeath",
    "specialty_bulletpenetration",
    "specialty_holdbreath",
    "specialty_quieter",
];

/// The guns of each kind that anyone may pick, most picked first (for a
/// gun the bot's rank hasn't unlocked yet).
const BY_KIND: [(GunKind, &[&str]); 5] = [
    (GunKind::Smg, &["mp5", "skorpion", "uzi", "ak74u", "p90"]),
    (GunKind::Rifle, &["m16", "ak47", "m4", "g3", "g36c", "m14", "mp44"]),
    (GunKind::Lmg, &["saw", "rpd", "m60e4"]),
    (GunKind::Shotgun, &["winchester1200", "m1014"]),
    (GunKind::Sniper, &["m40a3", "m21", "dragunov", "remington700", "barrett"]),
];

/// One real player's habits. Rates are per minute alive, shares 0..1,
/// times in seconds, distances in CoD units.
#[derive(Clone, Debug)]
pub struct Persona {
    /// Which demo and client it came from (`demo0009#7`).
    pub source: String,
    /// Rank (0-based level) and prestige.
    pub rank: (i32, i32),
    /// Where their results sat among all the real players (0..1).
    pub skill: f32,
    pub kd: f32,
    pub kind: GunKind,
    pub gun: String,
    pub attachment: String,
    pub pistol: String,
    pub perks: Vec<String>,
    // Moving.
    pub sprint: f32,
    pub still: f32,
    pub crouch: f32,
    pub prone: f32,
    pub jumps: f32,
    /// Typical stop to hold, and one angle within it.
    pub hold_secs: f32,
    pub angle_secs: f32,
    /// Share of time in stops of 8 s or more.
    pub camp: f32,
    /// Ground covered: distinct 256-unit cells a minute.
    pub roam: f32,
    /// Closing on the nearest enemy vs backing off (0.5 = even).
    pub aggression: f32,
    // Shooting.
    /// Share of contacts opened by shooting where they'd appear.
    pub prefire: f32,
    /// Seconds from an enemy coming into sight to their first shot.
    pub first_shot: f32,
    pub ads: f32,
    /// Snipers: shots fired within half a second of scoping in, and from
    /// the hip; typical time scoped in before a shot.
    pub quickscope: f32,
    pub noscope: f32,
    pub scope_secs: f32,
    pub kill_range: f32,
    pub headshots: f32,
    // Utility, per minute.
    pub frag: f32,
    pub smoke: f32,
    pub flash: f32,
    pub stun: f32,
    pub claymore: f32,
    pub c4: f32,
    pub tube: f32,
    pub utility: f32,
    /// Kill streaks they called in, in their match.
    pub streaks: f32,
    // Temperament.
    /// Next kill after a death is the killer, share.
    pub revenge: f32,
    /// Sprint share change after 2+ deaths in a row (tilting).
    pub tilt: f32,
    /// Standing-still share change with 3+ kills this life (protecting it).
    pub protect: f32,
    /// Time within 600 units of a Domination flag.
    pub objective: f32,
}

/// A row's traits: present or not (the measure needs enough data).
struct Row {
    source: String,
    cols: Vec<(String, String)>,
}

impl Row {
    fn get(&self, k: &str) -> Option<f32> {
        self.cols.iter().find(|c| c.0 == k).and_then(|c| c.1.parse::<f32>().ok()).filter(|v| v.is_finite())
    }

    fn text(&self, k: &str) -> &str {
        self.cols.iter().find(|c| c.0 == k).map_or("", |c| c.1.as_str())
    }
}

struct Table {
    rows: Vec<Row>,
    /// Each column's median, for traits a row doesn't have.
    medians: Vec<(String, f32)>,
}

static PEOPLE: LazyLock<Table> = LazyLock::new(|| {
    let mut lines = TABLE.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty());
    let head: Vec<String> = lines.next().unwrap_or("").split(',').map(|s| s.trim().to_string()).collect();
    let rows: Vec<Row> = lines
        .map(|l| {
            let cells: Vec<&str> = l.split(',').collect();
            Row {
                source: cells.first().unwrap_or(&"").to_string(),
                cols: head.iter().cloned().zip(cells.iter().map(|c| c.trim().to_string())).collect(),
            }
        })
        .collect();
    let medians = head
        .iter()
        .map(|k| {
            let mut v: Vec<f32> = rows.iter().filter_map(|r| r.get(k)).collect();
            v.sort_by(f32::total_cmp);
            (k.clone(), v.get(v.len() / 2).copied().unwrap_or(0.0))
        })
        .collect();
    Table { rows, medians }
});

fn kind_of(class: &str) -> GunKind {
    match class {
        "smg" => GunKind::Smg,
        "lmg" => GunKind::Lmg,
        "shotgun" => GunKind::Shotgun,
        "sniper" => GunKind::Sniper,
        _ => GunKind::Rifle,
    }
}

impl Persona {
    /// How many real players there are to be.
    pub fn count() -> usize {
        PEOPLE.rows.len()
    }

    /// A real player near `skill` (0..1 among everyone), varied a little.
    /// `None` without a table.
    pub fn sample(skill: f32, rng: &mut impl Rng) -> Option<Persona> {
        let t = &*PEOPLE;
        // Test aid: `COD4RW_PERSONA_KIND=shotgun` (smg, rifle, lmg, sniper):
        // only that kind of player.
        let only = std::env::var("COD4RW_PERSONA_KIND").ok();
        let weights: Vec<f32> = t
            .rows
            .iter()
            .map(|r| {
                if only.as_deref().is_some_and(|k| r.text("gun_class") != k) {
                    return 0.0;
                }
                let d = (r.get("skill").unwrap_or(0.5) - skill) / 0.12;
                // A longer match says more about how they play; `weight`
                // keeps any one lobby type (the sniper-only ones) from
                // setting the lobby's mix (`tools/demo_traits.py`).
                (-0.5 * d * d).exp() * r.get("alive_min").unwrap_or(2.0).sqrt() * r.get("weight").unwrap_or(1.0)
            })
            .collect();
        let total: f32 = weights.iter().sum();
        if total <= 0.0 {
            return None;
        }
        let mut pick = rng.random::<f32>() * total;
        let i = weights.iter().position(|w| {
            pick -= w;
            pick <= 0.0
        })?;
        Some(Persona::from_row(&t.rows[i], rng))
    }

    fn from_row(r: &Row, rng: &mut impl Rng) -> Persona {
        let t = &*PEOPLE;
        let median = |k: &str| t.medians.iter().find(|m| m.0 == k).map_or(0.0, |m| m.1);
        // Each trait a touch either side of theirs (a person isn't the same
        // two matches running), within its natural range.
        let mut vary = |k: &str, lo: f32, hi: f32| {
            let v = r.get(k).unwrap_or_else(|| median(k));
            (v * (0.12 * super::gaussian(rng)).exp()).clamp(lo, hi)
        };
        // Modded servers show levels past CoD4's 55: count those as prestige.
        let level = r.get("rank").unwrap_or(1.0) as i32;
        let rank = if level > 55 { (54, ((level - 55) / 15 + 1).min(10)) } else { ((level - 1).max(0), r.get("prestige").unwrap_or(0.0) as i32) };
        let kind = kind_of(r.text("gun_class"));
        Persona {
            source: r.source.clone(),
            rank,
            skill: r.get("skill").unwrap_or(0.5),
            kd: r.get("kd").unwrap_or(1.0),
            kind,
            gun: r.text("gun").to_string(),
            attachment: r.text("attachment").to_string(),
            pistol: r.text("pistol").to_string(),
            perks: r.text("perks").split('+').filter(|p| !p.is_empty()).map(String::from).collect(),
            sprint: vary("sprint_share", 0.0, 0.95),
            still: vary("still_share", 0.02, 0.9),
            crouch: vary("crouch_share", 0.0, 0.9),
            prone: vary("prone_share", 0.0, 0.6),
            jumps: vary("jumps_pm", 0.0, 20.0),
            hold_secs: vary("hold_p50", 1.0, 30.0),
            angle_secs: vary("angle_hold_p50", 0.5, 20.0),
            camp: vary("camp_share", 0.0, 0.8),
            roam: vary("cells_pm", 1.0, 60.0),
            aggression: vary("aggression", 0.3, 0.95),
            prefire: vary("prefire_rate", 0.0, 0.5),
            first_shot: vary("first_shot_p50", 0.05, 1.2),
            ads: vary("ads_fight_share", 0.05, 0.98),
            quickscope: vary("quickscope_share", 0.0, 1.0),
            noscope: vary("noscope_share", 0.0, 1.0),
            scope_secs: vary("scope_time_p50", 0.2, 10.0),
            kill_range: vary("kill_dist_p50", 150.0, 3500.0),
            headshots: vary("headshot_share", 0.0, 0.8),
            frag: vary("frag_pm", 0.0, 5.0),
            smoke: vary("smoke_pm", 0.0, 5.0),
            flash: vary("flash_pm", 0.0, 5.0),
            stun: vary("stun_pm", 0.0, 5.0),
            claymore: vary("claymore_pm", 0.0, 5.0),
            c4: vary("c4_pm", 0.0, 5.0),
            tube: vary("tube_pm", 0.0, 5.0),
            utility: vary("utility_pm", 0.0, 8.0),
            streaks: r.get("streaks_called").unwrap_or(0.0),
            revenge: vary("revenge_share", 0.0, 1.0),
            tilt: r.get("tilt_sprint_delta").unwrap_or(0.0).clamp(-0.5, 0.6),
            protect: r.get("streak_still_delta").unwrap_or(0.0).clamp(-0.5, 0.6),
            objective: vary("near_flag_share", 0.0, 1.0),
        }
    }

    /// 0 for a new player, 1 for one at the top rank or prestiged.
    pub fn experience(&self) -> f32 {
        if self.rank.1 > 0 { 1.0 } else { (self.rank.0 as f32 / 54.0).clamp(0.0, 1.0) }
    }

    /// How well they know maps (0..1): power positions, flanks, timings,
    /// nade spots, spawn flips. Rank mostly; better players learn quicker.
    pub fn knowledge(&self) -> f32 {
        (0.75 * self.experience().powf(0.7) + 0.25 * self.skill).clamp(0.0, 1.0)
    }

    /// Sits in spots far more than most (the top tenth or so of players).
    pub fn camper(&self) -> bool {
        self.camp > 0.06 || self.still > 0.45
    }

    /// A short word for how they play (debug overlay, lobby).
    pub fn style(&self) -> &'static str {
        if self.kind == GunKind::Sniper && self.quickscope + self.noscope > 0.45 {
            "quickscoper"
        } else if self.kind == GunKind::Sniper {
            "sniper"
        } else if self.camper() {
            "camper"
        } else if self.sprint > 0.6 && self.aggression > 0.72 {
            "rusher"
        } else if self.utility > 0.8 {
            "nade spammer"
        } else if self.aggression < 0.62 {
            "cautious"
        } else {
            "all-rounder"
        }
    }

    // ------------------------------------------------- the bots' knobs

    /// `Personality::aggression`: 0 careful .. 1 rushes.
    pub fn aggression_knob(&self) -> f32 {
        ((self.aggression - 0.55) / 0.3).clamp(0.05, 0.95)
    }

    /// `Personality::patience`: how long it holds.
    pub fn patience_knob(&self) -> f32 {
        ((self.still - 0.08) / 0.4 * 0.7 + (self.hold_secs - 2.0) / 6.0 * 0.3).clamp(0.05, 0.95)
    }

    /// `Personality::sprinter`: chance a trip is sprinted.
    pub fn sprint_knob(&self) -> f32 {
        // (Bots only sprint on long open legs, so the knob runs higher than
        // the share it measures.)
        (self.sprint / 0.45).clamp(0.03, 1.0)
    }

    /// The bot's reaction setting (seconds), so that it measures like its
    /// real player's first shot (noticing, surprise and aiming add a little
    /// to the setting). Newer players a touch slower.
    pub fn reaction(&self) -> f32 {
        // (The slowest tail cut: real reaction past ~0.6 s is the demo's
        // 20 Hz and spraying, not a player that slow. Bot lab `oldskill`:
        // as before.)
        if super::lab::on("oldskill") {
            return (self.first_shot + REACTION_GAP + 0.06 * (1.0 - self.experience())).clamp(0.14, 0.9);
        }
        // A first shot under 0.15 s in a demo is its 20 Hz snapshots and
        // spraying, not a player that quick (a third of the rows): those get
        // one by their results instead.
        let first = if self.first_shot < 0.15 && !super::lab::on("oldreaction") { 0.22 + 0.2 * (1.0 - self.skill) } else { self.first_shot };
        (first - 0.12 + 0.06 * (1.0 - self.experience())).clamp(0.12, 0.6)
    }

    /// `MoveStyle::ads_range`: the distance a standing bot has sights up
    /// half the time (CoD units: 170 for a player who has them up 55% of
    /// their fights, the median).
    pub fn ads_half(&self) -> f32 {
        let odds = (1.0 - self.ads) / self.ads.max(0.02);
        u(170.0 * (odds / (0.45 / 0.55)).powf(1.0 / 1.5)).clamp(u(20.0), u(900.0))
    }

    /// `MoveStyle::crouch`: chance of fighting crouched (the median player
    /// is crouched 11% of their time).
    pub fn crouch_knob(&self) -> f32 {
        ((self.crouch + self.prone) / 0.13 * 0.35).clamp(0.0, 0.9)
    }

    /// How much more (or less) than the bots' default they throw: frags,
    /// and flashbangs/stuns.
    pub fn frag_scale(&self) -> f32 {
        ((self.frag + 0.02) / 0.06).clamp(0.1, 6.0)
    }

    pub fn special_scale(&self) -> f32 {
        ((self.flash + self.stun + 0.05) / 0.3).clamp(0.1, 6.0)
    }

    /// A stop held (seconds): around their typical stop; campers now and
    /// then sit much longer.
    pub fn hold_time(&self, rng: &mut impl Rng) -> f32 {
        if rng.random::<f32>() < (self.camp * 8.0).min(0.6) {
            return rng.random_range(8.0..(8.0 + 60.0 * self.camp.min(0.4)).max(10.0));
        }
        (self.hold_secs * (0.45 * super::gaussian(rng)).exp()).clamp(0.8, 12.0)
    }

    /// One angle watched before switching to the next (seconds).
    pub fn angle_time(&self, rng: &mut impl Rng) -> f32 {
        (self.angle_secs * (0.4 * super::gaussian(rng)).exp()).clamp(0.5, 10.0)
    }

    /// A sniper's way of shooting this fight.
    pub fn scope_style(&self, rng: &mut impl Rng) -> Scope {
        if self.kind != GunKind::Sniper {
            return Scope::Hard;
        }
        let r = rng.random::<f32>();
        if r < self.noscope * 0.7 {
            Scope::No
        } else if r < self.noscope * 0.7 + self.quickscope {
            Scope::Quick
        } else {
            Scope::Hard
        }
    }

    // ---------------------------------------------------------- the class

    /// The gun they'd carry at this rank: theirs if unlocked, else the
    /// most picked one of its kind that is; a default class's below
    /// Create-a-Class.
    fn legal_gun(&self) -> (String, String) {
        let level = self.rank.0;
        let options = BY_KIND.iter().find(|b| b.0 == self.kind).map_or(&["m16"][..], |b| b.1);
        if level < CREATE_A_CLASS || self.gun.is_empty() || self.gun == "none" {
            return default_class(self.kind).0;
        }
        let gun = if unlock_rank(&self.gun) <= level && options.contains(&self.gun.as_str()) {
            self.gun.clone()
        } else {
            options.iter().rev().find(|g| unlock_rank(g) <= level).unwrap_or(&options[0]).to_string()
        };
        // Attachments come from kills with the gun: newer players have few.
        let att = match self.attachment.as_str() {
            "none" | "" => String::new(),
            "acog" | "silencer" if level < 8 => "reflex".into(),
            a => a.to_string(),
        };
        (gun, att)
    }

    /// Their class: gun and attachment, pistol, perks, special grenade and
    /// equipment, as they had them (where this rank has them).
    pub fn class(&self, mut base: crate::loadout::ClassLoadout, rng: &mut impl Rng) -> crate::loadout::ClassLoadout {
        let level = self.rank.0;
        if level < CREATE_A_CLASS {
            let ((gun, att), pistol, perks, inventory, special) = default_class(self.kind);
            base.guns[0].spec = if att.is_empty() { gun.clone() } else { format!("{gun}:{att}") };
            base.guns[0].name = gun.to_ascii_uppercase();
            base.guns[0].camo = 0;
            if let Some(p) = base.guns.get_mut(1) {
                p.spec = pistol.into();
                p.name = pistol.to_ascii_uppercase();
            }
            base.perks = perks.iter().map(|s| s.to_string()).collect();
            base.inventory = inventory.map(String::from);
            base.special = Some(special.into());
            return base;
        }
        let (gun, att) = self.legal_gun();
        let camo = base.guns[0].camo;
        base.guns[0] = crate::loadout::Gun {
            name: gun.to_ascii_uppercase(),
            spec: if att.is_empty() { gun.clone() } else { format!("{gun}:{att}") },
            camo,
            variant: None,
        };
        let pistol = self.pistol.replace("_silencer", ":silencer");
        let pistol_gun = pistol.split(':').next().unwrap_or("").to_string();
        if !pistol_gun.is_empty() && pistol_gun != "none" && unlock_rank(&pistol_gun) <= level {
            if let Some(p) = base.guns.get_mut(1) {
                p.spec = pistol.clone();
                p.name = pistol_gun.to_ascii_uppercase();
            }
        }
        let has = |p: &str| self.perks.iter().any(|x| x == p) && unlock_rank(p) <= level;
        let tier = |list: &[&str], fallback: &str| list.iter().find(|p| has(p)).map_or(fallback.to_string(), |p| p.to_string());
        // Perk 1: what they had, else what their throws say they carried.
        let launcher = att == "gl" || att == "grip";
        let perk1 = if launcher {
            None
        } else if self.claymore > 0.05 && unlock_rank("specialty_weapon_claymore") <= level {
            Some("specialty_weapon_claymore".to_string())
        } else if self.c4 > 0.05 {
            Some("specialty_weapon_c4".to_string())
        } else if self.tube > 0.05 {
            Some("specialty_weapon_rpg".to_string())
        } else if PERK1.iter().any(|p| has(p)) {
            Some(tier(&PERK1, ""))
        } else if self.flash + self.stun > 0.5 {
            Some("specialty_specialgrenade".to_string())
        } else {
            base.perks.first().filter(|p| PERK1.contains(&p.as_str())).cloned().or_else(|| base.inventory.as_ref().map(|i| format!("specialty_weapon_{}", i.trim_end_matches("_mp"))))
        };
        let perk2 = tier(&PERK2, base.perks.iter().find(|p| PERK2.contains(&p.as_str())).map_or("specialty_bulletdamage", |s| s.as_str()));
        let default3 = if self.kind == GunKind::Sniper { "specialty_bulletpenetration" } else { "specialty_bulletaccuracy" };
        let perk3 = tier(&PERK3, base.perks.iter().find(|p| PERK3.contains(&p.as_str())).map_or(default3, |s| s.as_str()));
        let inventory = match perk1.as_deref() {
            Some("specialty_weapon_claymore") => Some("claymore_mp"),
            Some("specialty_weapon_c4") => Some("c4_mp"),
            Some("specialty_weapon_rpg") => Some("rpg_mp"),
            _ => None,
        };
        base.inventory = inventory.map(String::from);
        base.perks = perk1.filter(|_| inventory.is_none()).filter(|p| !p.is_empty()).into_iter().chain([perk2, perk3]).collect();
        // The special grenade they threw most.
        let special = [(self.flash, "flash_grenade"), (self.stun, "concussion_grenade"), (self.smoke, "smoke_grenade")]
            .into_iter()
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .filter(|s| s.0 > 0.02)
            .map(|s| s.1);
        if let Some(s) = special {
            base.special = Some(s.into());
        } else if rng.random::<f32>() < 0.5 {
            base.special = Some("flash_grenade".into());
        }
        base
    }
}

/// How a sniper takes a shot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Scope in and settle.
    Hard,
    /// Scope in and fire as it lands, then out.
    Quick,
    /// From the hip, close up.
    No,
}

type DefaultClass = ((String, String), &'static str, &'static [&'static str], Option<&'static str>, &'static str);

/// CoD4's default classes (`mp/classTable.csv`), by the kind of gun:
/// Assault, Spec Ops, Heavy Gunner, Demolitions, Sniper.
fn default_class(kind: GunKind) -> DefaultClass {
    match kind {
        GunKind::Smg => (("mp5".into(), String::new()), "usp:silencer", &["specialty_explosivedamage", "specialty_bulletaccuracy"], Some("c4_mp"), "flash_grenade"),
        GunKind::Lmg => (("saw".into(), String::new()), "usp", &["specialty_specialgrenade", "specialty_armorvest", "specialty_bulletpenetration"], None, "concussion_grenade"),
        GunKind::Shotgun => (("winchester1200".into(), String::new()), "beretta", &["specialty_explosivedamage", "specialty_longersprint"], Some("rpg_mp"), "smoke_grenade"),
        GunKind::Sniper => (("m40a3".into(), String::new()), "beretta:silencer", &["specialty_specialgrenade", "specialty_bulletdamage", "specialty_bulletpenetration"], None, "flash_grenade"),
        GunKind::Rifle => (("m16".into(), "gl".into()), "beretta", &["specialty_bulletdamage", "specialty_longersprint"], None, "concussion_grenade"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_loads_and_samples() {
        assert!(Persona::count() > 50, "personas.csv has {} rows", Persona::count());
        let mut rng = rand::rng();
        for s in [0.05, 0.5, 0.95] {
            let p = Persona::sample(s, &mut rng).expect("a persona");
            assert!((0.0..=1.0).contains(&p.sprint));
            assert!(p.hold_time(&mut rng) > 0.5);
        }
    }

    #[test]
    fn novices_carry_what_they_have_unlocked() {
        let mut rng = rand::rng();
        for _ in 0..300 {
            let p = Persona::sample(rng.random(), &mut rng).unwrap();
            let (gun, _) = p.legal_gun();
            assert!(unlock_rank(&gun) <= p.rank.0.max(CREATE_A_CLASS) || p.rank.0 < CREATE_A_CLASS, "{gun} at rank {}", p.rank.0);
        }
    }

    #[test]
    fn ads_half_is_the_default_for_the_median_player() {
        let p = Persona::sample(0.5, &mut rand::rng()).unwrap();
        let median = Persona { ads: 0.55, ..p };
        assert!((median.ads_half() - u(170.0)).abs() < u(1.0));
        let scoped = Persona { ads: 0.8, ..median.clone() };
        assert!(scoped.ads_half() < median.ads_half());
    }
}
