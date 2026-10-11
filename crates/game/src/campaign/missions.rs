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
}

pub struct Objective {
    /// Its localized string, and the English if the zone lacks it.
    pub text: &'static str,
    pub english: &'static str,
    /// Where its marker stands, in turn (CoD units).
    pub waypoints: &'static [[f32; 3]],
}

pub const MISSIONS: [Mission; 1] = [Mission {
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
}];

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
