//! The missions: their levels, starts, the player's weapons and the
//! objectives' waypoints, taken from each level's script (its
//! `objective_main`) until the scripts themselves run.

use crate::loadout::{ClassLoadout, Gun};

pub struct Mission {
    /// The level's zone.
    pub map: &'static str,
    pub title: &'static str,
    /// Where the player starts (CoD units) and faces (degrees).
    pub start: [f32; 3],
    pub yaw: f32,
    /// The player's weapons (`weapon:attachments`, as the MP guns) and their
    /// HUD names.
    pub guns: &'static [(&'static str, &'static str)],
    pub special: Option<&'static str>,
    pub objectives: &'static [Objective],
    /// The death quote's localized string, and its English if missing.
    pub failed: (&'static str, &'static str),
    /// The level's own scripts run it (its actors staged as they animate
    /// them, not fighting pawns).
    pub scripted: bool,
}

pub struct Objective {
    /// Its localized string, and the English if the zone lacks it.
    pub text: &'static str,
    pub english: &'static str,
    /// Where its marker stands, in turn (CoD units).
    pub waypoints: &'static [[f32; 3]],
}

pub const MISSIONS: [Mission; 21] = [scripted("killhouse", "F.N.G."), Mission {
    map: "cargoship",
    title: "Crew Expendable",
    // The deck aft of the bridge, where the Black Hawk's ropes come down.
    start: [3904.0, 0.0, 216.0],
    yaw: 180.0,
    guns: &[("mp5:silencer", "MP5SD"), ("usp:silencer", "USP .45")],
    special: Some("flash_grenade"),
    objectives: &[Objective {
        text: "CARGOSHIP_OBJ_PACKAGE",
        english: "Search the ship for the package.",
        // `objective_main`: the bridge, the stairs down to the deck, the
        // deck's far end, the hallways and stairs down, the cargo holds,
        // then the package.
        waypoints: &[
            [3052.0, 15.0, 407.0],
            [2640.0, 624.0, 208.0],
            [-2116.0, 0.0, 80.0],
            [-2506.0, -496.0, 96.0],
            [-2806.0, -122.0, 96.0],
            [-3292.0, -248.0, -65.0],
            [2254.0, 197.0, -320.0],
        ],
    }],
    failed: ("", "You were killed in action."),
    // (Its own scripts and original AI, as every level now.)
    scripted: true,
},
Mission {
    map: "coup",
    title: "The Coup",
    // Al-Fulani, in the back of the car (the level's own start; its script
    // carries him from here).
    start: [48.0, -618.0, 0.0],
    yaw: 90.0,
    // No weapon: he's a prisoner.
    guns: &[],
    special: None,
    objectives: &[],
    failed: ("", ""),
    scripted: true,
},
scripted("blackout", "Blackout"),
scripted("armada", "Charlie Don't Surf"),
scripted("bog_a", "The Bog"),
scripted("hunted", "Hunted"),
scripted("ac130", "Death From Above"),
scripted("bog_b", "War Pig"),
scripted("airlift", "Shock and Awe"),
scripted("aftermath", "Aftermath"),
scripted("village_assault", "Safehouse"),
scripted("scoutsniper", "All Ghillied Up"),
scripted("sniperescape", "One Shot, One Kill"),
scripted("village_defend", "Heat"),
scripted("ambush", "The Sins of the Father"),
scripted("icbm", "Ultimatum"),
scripted("launchfacility_a", "All In"),
scripted("launchfacility_b", "No Fighting in the War Room"),
scripted("jeepride", "Game Over"),
scripted("airplane", "Mile High Club")];

/// A level its own scripts run from its `info_player_start` (NaN start).
const fn scripted(map: &'static str, title: &'static str) -> Mission {
    Mission { map, title, start: [f32::NAN; 3], yaw: 0.0, guns: &[], special: None, objectives: &[], failed: ("", ""), scripted: true }
}

/// A mission by its level's name.
pub fn find(map: &str) -> Option<&'static Mission> {
    MISSIONS.iter().find(|m| m.map.eq_ignore_ascii_case(map))
}

impl Mission {
    /// The player's class for the mission.
    pub fn class(&self) -> ClassLoadout {
        ClassLoadout {
            name: self.title.to_owned(),
            guns: self.guns.iter().map(|(spec, name)| gun(spec, name)).collect(),
            perks: Vec::new(),
            special: self.special.map(str::to_owned),
            inventory: None,
            inventory_camo: 0,
        }
    }
}

fn gun(spec: &str, name: &str) -> Gun {
    Gun { spec: spec.to_owned(), camo: 0, name: name.to_owned(), variant: None }
}

/// An enemy actor's weapon, from its spawner's class (`actor_enemy_` taken
/// off: `merc_ar_ak47`, `merc_smg_miniuzi`...), as the MP gun.
pub fn actor_gun(class: &str) -> &'static str {
    const GUNS: [(&str, &str); 14] = [
        ("ak74u", "ak74u:"),
        ("ak47", "ak47:"),
        ("miniuzi", "uzi:"),
        ("uzi", "uzi:"),
        ("deserteagle", "deserteagle:"),
        ("rpd", "rpd:"),
        ("saw", "saw:"),
        ("g3", "g3:"),
        ("dragunov", "dragunov:"),
        ("m4", "m4:"),
        ("mp5", "mp5:"),
        ("winchester", "winchester1200:"),
        ("rpg", "ak47:"),
        ("skorpion", "skorpion:"),
    ];
    GUNS.iter().find(|(k, _)| class.contains(k)).map_or("ak47:", |g| g.1)
}

/// An actor's class: its gun and a sidearm.
pub fn actor_class(spec: &str) -> ClassLoadout {
    ClassLoadout {
        name: "actor".into(),
        guns: vec![gun(spec, spec.trim_end_matches(':')), gun("beretta:", "M9")],
        perks: Vec::new(),
        special: None,
        inventory: None,
        inventory_camo: 0,
    }
}

/// Names for the kill feed (the level's soldiers have none).
pub fn rank_name(i: usize) -> &'static str {
    const NAMES: [&str; 4] = ["Mercenary", "Sentry", "Gunner", "Rifleman"];
    NAMES[i % NAMES.len()]
}

/// A single-player weapon (`giveweapon` name: `g36c`, `m4m203_silencer_reflex`,
/// `m14_scoped_silencer_woodland`...) as the multiplayer gun closest to it
/// (`weapon:attachment+attachment`), with its HUD name; None for grenades
/// and equipment.
pub fn sp_gun(name: &str) -> Option<(String, String)> {
    const BASES: [(&str, &str, &str); 29] = [
        ("m14_scoped", "m21", "M21"),
        ("m4m203", "m4", "M4A1"),
        ("m4_", "m4", "M4A1"),
        ("m40a3", "m40a3", "R700"),
        ("m16", "m16", "M16A4"),
        ("m4", "m4", "M4A1"),
        ("ak74u", "ak74u", "AK-74u"),
        ("ak47", "ak47", "AK-47"),
        ("g36c", "g36c", "G36C"),
        ("g3", "g3", "G3"),
        ("m14", "m14", "M14"),
        ("m21", "m21", "M21"),
        ("mp5", "mp5", "MP5"),
        ("mp44", "mp44", "MP44"),
        ("p90", "p90", "P90"),
        ("uzi", "uzi", "Mini-Uzi"),
        ("skorpion", "skorpion", "Skorpion"),
        ("rpd", "rpd", "RPD"),
        ("saw", "saw", "M249 SAW"),
        ("m60", "m60e4", "M60E4"),
        ("dragunov", "dragunov", "Dragunov"),
        ("barrett", "barrett", "Barrett .50cal"),
        ("remington700", "remington700", "R700"),
        ("winchester1200", "winchester1200", "W1200"),
        ("m1014", "m1014", "M1014"),
        ("usp", "usp", "USP .45"),
        ("beretta", "beretta", "M9"),
        ("colt45", "colt45", "M1911 .45"),
        ("deserteagle", "deserteagle", "Desert Eagle"),
    ];
    let n = name.to_ascii_lowercase();
    let (_, base, hud) = BASES.iter().find(|(k, ..)| n.starts_with(k))?;
    let mut atts = Vec::new();
    if n.contains("m203") || n.contains("grenadier") || n.contains("_gl") {
        atts.push("gl");
    }
    for a in ["silencer", "reflex", "acog", "grip"] {
        if n.contains(a) {
            atts.push(a);
        }
    }
    // (Single-player guns whose own model carries a red dot, as F.N.G.'s
    // G36C: the MP gun with its reflex sight, which aims down it right.)
    if n == "g36c" && !atts.contains(&"reflex") {
        atts.push("reflex");
    }
    Some((format!("{base}:{}", atts.join("+")), (*hud).to_owned()))
}
