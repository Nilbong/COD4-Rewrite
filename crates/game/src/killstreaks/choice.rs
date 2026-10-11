//! Kill streak loadouts: the (up to) three kill streaks a player can earn,
//! chosen in Create a Class and kept per profile in its stats. Only those
//! are earned, fewest kills first; keys 5, 6 and 7 call in the first,
//! second and third of them, and the HUD stacks just those. A profile that
//! never chose has CoD4's: UAV, airstrike, helicopter.
//!
//! The menu's side ([`crate::ui`]) reads and writes through closures over
//! the profile's stats:
//! ```ignore
//! let all = choice::available();                       // icons, names, kills
//! let mine = choice::chosen(|i| fe.stats.get(i));      // the current three
//! choice::set(&mut |i, v| fe.stats.set(i, v), &[Hardpoint::Uav]);
//! choice::toggle(|i| fe.stats.get(i), &mut |i, v| fe.stats.set(i, v), Hardpoint::Sentry);
//! ```

use super::Hardpoint;

/// The profile stats: the picks (each a hardpoint's place in
/// [`Hardpoint::ALL`] plus one, 0 for none), and whether the player has
/// ever chosen (until then [`DEFAULT`] applies).
pub const STATS: i32 = 6950;
const CHOSEN_FLAG: i32 = STATS + MAX as i32;
/// At most this many.
pub const MAX: usize = 3;
/// CoD4's three.
pub const DEFAULT: [Hardpoint; 3] = [Hardpoint::Uav, Hardpoint::Airstrike, Hardpoint::Helicopter];

/// A kill streak for the menu.
#[derive(Clone, Copy, Debug)]
pub struct StreakInfo {
    pub hardpoint: Hardpoint,
    /// Its id (`"sentry_gun"`, for saved strings and the UI's rows).
    pub id: &'static str,
    pub name: &'static str,
    /// Its HUD picture (`ks:...`, drawable by the UI's image ops).
    pub icon: &'static str,
    /// Kills in a row that earn it.
    pub kills: u32,
}

impl Hardpoint {
    pub fn id(self) -> &'static str {
        match self {
            Hardpoint::Uav => "uav",
            Hardpoint::CarePackage => "care_package",
            Hardpoint::Airstrike => "airstrike",
            Hardpoint::Sentry => "sentry_gun",
            Hardpoint::Helicopter => "helicopter",
        }
    }

    pub fn from_id(id: &str) -> Option<Hardpoint> {
        let id = id.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Hardpoint::ALL.into_iter().find(|h| h.id() == id || h.id().replace('_', "") == id.replace('_', ""))
    }
}

/// Every kill streak, fewest kills first.
pub fn available() -> Vec<StreakInfo> {
    Hardpoint::ALL.into_iter().map(|h| StreakInfo { hardpoint: h, id: h.id(), name: h.name(), icon: h.hud_icon(), kills: h.kills() }).collect()
}

/// The profile's chosen kill streaks, fewest kills first.
pub fn chosen(stat: impl Fn(i32) -> i32) -> Vec<Hardpoint> {
    if stat(CHOSEN_FLAG) == 0 {
        return DEFAULT.to_vec();
    }
    normalised((0..MAX as i32).filter_map(|k| Hardpoint::ALL.get((stat(STATS + k) - 1).max(-1) as usize).copied()))
}

/// Choose these (the first three different ones count), saved in order of
/// kills.
pub fn set(set_stat: &mut impl FnMut(i32, i32), picks: &[Hardpoint]) {
    let picks = normalised(picks.iter().copied());
    for k in 0..MAX {
        let v = picks.get(k).map_or(0, |h| Hardpoint::ALL.iter().position(|x| x == h).unwrap_or(0) as i32 + 1);
        set_stat(STATS + k as i32, v);
    }
    set_stat(CHOSEN_FLAG, 1);
}

/// Take `h` off if it's chosen, else add it when there's room. Whether it's
/// chosen now.
pub fn toggle(stat: impl Fn(i32) -> i32, set_stat: &mut impl FnMut(i32, i32), h: Hardpoint) -> bool {
    let mut now = chosen(stat);
    let on = if let Some(i) = now.iter().position(|&x| x == h) {
        now.remove(i);
        false
    } else if now.len() < MAX {
        now.push(h);
        true
    } else {
        false
    };
    set(set_stat, &now);
    on
}

/// No repeats, at most three, fewest kills first.
fn normalised(picks: impl Iterator<Item = Hardpoint>) -> Vec<Hardpoint> {
    let mut v: Vec<Hardpoint> = Vec::new();
    for h in picks {
        if !v.contains(&h) && v.len() < MAX {
            v.push(h);
        }
    }
    v.sort_by_key(|h| h.kills());
    v
}

/// A bot's three: one of a few sensible sets, by chance.
pub fn for_bot(rng: &mut impl rand::Rng) -> Vec<Hardpoint> {
    use Hardpoint::*;
    let sets: [[Hardpoint; 3]; 5] = [
        [Uav, Airstrike, Helicopter],
        [Uav, CarePackage, Sentry],
        [CarePackage, Airstrike, Helicopter],
        [Uav, Sentry, Helicopter],
        [Uav, CarePackage, Airstrike],
    ];
    sets[rng.random_range(0..sets.len())].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn defaults_set_and_toggle() {
        let mut stats: HashMap<i32, i32> = HashMap::new();
        let get = |s: &HashMap<i32, i32>| {
            let s = s.clone();
            move |i: i32| s.get(&i).copied().unwrap_or(0)
        };
        assert_eq!(chosen(get(&stats)), DEFAULT.to_vec());
        set(&mut |i, v| { stats.insert(i, v); }, &[Hardpoint::Helicopter, Hardpoint::Sentry, Hardpoint::Sentry, Hardpoint::Uav, Hardpoint::Airstrike]);
        assert_eq!(chosen(get(&stats)), vec![Hardpoint::Uav, Hardpoint::Sentry, Hardpoint::Helicopter]);
        // Full: no room for another; taking one off makes room.
        assert!(!toggle(get(&stats), &mut |i, v| { stats.insert(i, v); }, Hardpoint::CarePackage));
        assert!(!toggle(get(&stats), &mut |i, v| { stats.insert(i, v); }, Hardpoint::Sentry));
        assert!(toggle(get(&stats), &mut |i, v| { stats.insert(i, v); }, Hardpoint::CarePackage));
        assert_eq!(chosen(get(&stats)), vec![Hardpoint::Uav, Hardpoint::CarePackage, Hardpoint::Helicopter]);
        // Cleared: none (not the default).
        set(&mut |i, v| { stats.insert(i, v); }, &[]);
        assert!(chosen(get(&stats)).is_empty());
        assert_eq!(Hardpoint::from_id("Sentry Gun"), Some(Hardpoint::Sentry));
        assert_eq!(Hardpoint::from_id("carepackage"), Some(Hardpoint::CarePackage));
    }
}
