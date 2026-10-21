//! Players with different games installed in one online match.
//!
//! Everyone has CoD4; Black Ops, Modern Warfare 2 and World at War add guns
//! (and MW2 maps). Each player's lobby profile says which they have and
//! which their custom classes use ([`installed`], [`classes_use`]). With
//! that:
//!
//! - the host only picks maps everyone in the lobby can load, and a player
//!   who can't load the lobby's map isn't let in (or leaves, told why);
//! - the relay's best-host election runs only when every player could host
//!   every gun in play ([`can_host`]); otherwise the host stays;
//! - a class with a gun the host can't run takes the closest CoD4 gun
//!   instead at spawn, with a notice ([`standin`]);
//! - a gun a player can't load is drawn and heard as the closest CoD4 gun,
//!   while its stats stay the host's and the kill feed names the real one.

use crate::content::Content;
use gamepaths::Game;
use std::sync::{Mutex, OnceLock};

/// A game's bit in a profile's sets ([`Game::ALL`]'s order).
pub fn bit(g: Game) -> u8 {
    1 << Game::ALL.iter().position(|x| *x == g).unwrap_or(0)
}

/// The games this player has (CoD4 always: the game doesn't start without).
pub fn installed() -> u8 {
    static FOUND: OnceLock<u8> = OnceLock::new();
    *FOUND.get_or_init(|| {
        let bits = Game::ALL.into_iter().filter(|&g| g == Game::Cod4 || gamepaths::find(g).is_some()).fold(0, |b, g| b | bit(g));
        bevy::log::info!("online: games installed: {}", tags(bits).join(", "));
        bits
    })
}

/// Short names for a set of games, for badges ("MW2", "BO", "WaW"; CoD4 is
/// everyone's, so not shown).
pub fn tags(bits: u8) -> Vec<&'static str> {
    Game::ALL
        .into_iter()
        .filter(|&g| g != Game::Cod4 && bits & bit(g) != 0)
        .map(|g| match g {
            Game::BlackOps => "BO",
            Game::Mw2 => "MW2",
            Game::WaW => "WaW",
            Game::Cod4 => "CoD4",
        })
        .collect()
}

/// The game a gun spec (`iw4_m4:reflex`, `t5_ak47`, `t4_thompson`, `m4`) is
/// from.
pub fn gun_game(spec: &str) -> Game {
    if spec.starts_with(crate::mw2guns::PREFIX) {
        Game::Mw2
    } else if spec.starts_with(crate::bo1::PREFIX) {
        Game::BlackOps
    } else if spec.starts_with(crate::waw::PREFIX) {
        Game::WaW
    } else {
        Game::Cod4
    }
}

/// What a map needs: a game, or (a custom map in `usermaps/`) that very map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapNeed {
    Game(Game),
    Custom,
}

pub fn map_need(id: &str) -> MapNeed {
    if id.starts_with("mw2_") {
        MapNeed::Game(Game::Mw2)
    } else if crate::ui::MAPS.iter().any(|(m, _)| *m == id) {
        MapNeed::Game(Game::Cod4)
    } else {
        MapNeed::Custom
    }
}

/// Can a player with `games` load map `id`? (A custom map: only known for
/// this game, which has it or not.)
pub fn can_load_map(id: &str, games: u8) -> bool {
    match map_need(id) {
        MapNeed::Game(g) => games & bit(g) != 0,
        MapNeed::Custom => false,
    }
}

/// Can a player with `games` host a lobby whose classes use `used`?
pub fn can_host(games: u8, used: u8) -> bool {
    used & !games == 0
}

/// The games guns in `specs` come from.
pub fn guns_use<'a>(specs: impl IntoIterator<Item = &'a str>) -> u8 {
    specs.into_iter().fold(0, |b, s| b | bit(gun_game(s)))
}

/// The closest CoD4 gun to another game's (`t5_ak47:reflex` -> `ak47`,
/// `iw4_famas` -> `m16`), by what it is; the same name where CoD4 has it.
fn cod4_equivalent(base: &str) -> &'static str {
    match base {
        // Assault rifles.
        "ak47" | "galil" => "ak47",
        "m4" | "tavor" | "masada" | "commando" => "m4",
        "m16" | "famas" | "enfield" => "m16",
        "m14" | "scar" | "m1garand" | "m1carbine" | "svt40" | "gewehr43" => "m14",
        "fal" | "fnfal" | "g3" | "fg42" => "g3",
        "fn2000" | "g11" | "g36c" => "g36c",
        "stg44" | "mp44" => "mp44",
        // SMGs.
        "mp5k" | "mp5" | "ump45" | "mpl" | "thompson" | "mp40" => "mp5",
        "kriss" | "p90" | "spectre" | "ppsh" => "p90",
        "uzi" | "tmp" | "mac11" => "uzi",
        "skorpion" | "pm63" | "kiparis" | "type100smg" => "skorpion",
        "ak74u" => "ak74u",
        // Machine guns.
        "rpd" | "rpk" | "type99lmg" | "dp28" => "rpd",
        "sa80" | "mg4" | "aug" | "stoner63" | "bar" => "saw",
        "m240" | "m60" | "m60e4" | "hk21" | "mg42" | "30cal" => "m60e4",
        // Shotguns.
        "spas12" | "ranger" | "model1887" | "olympia" | "stakeout" | "shotgun" | "doublebarreledshotgun" | "winchester1200" => "winchester1200",
        "aa12" | "striker" | "m1014" | "spas" | "hs10" => "m1014",
        // Sniper rifles.
        "cheytac" | "l96a1" | "type99rifle" | "mosinrifle" | "m40a3" => "m40a3",
        "kar98k" | "springfield" | "remington700" => "remington700",
        "wa2000" | "psg1" | "m21" => "m21",
        "dragunov" => "dragunov",
        "barrett" | "ptrs41" => "barrett",
        // Pistols.
        "glock" | "beretta393" | "pp2000" | "beretta" | "asp" | "makarov" | "walther" | "nambu" => "beretta",
        "usp" | "cz75" | "tokarev" => "usp",
        "m1911" | "colt" | "colt45" => "colt45",
        "deserteagle" | "coltanaconda" | "python" | "357magnum" => "deserteagle",
        _ => "",
    }
}

/// A CoD4 gun spec standing in for `spec` (unchanged if it's CoD4's):
/// the closest by role, else the same name if CoD4 has it, else the M4.
/// Attachments don't carry over (each game's are its own).
pub fn standin(content: &Content, spec: &str) -> String {
    if gun_game(spec) == Game::Cod4 {
        return spec.to_owned();
    }
    let base = spec.split(':').next().unwrap_or(spec);
    let base = base
        .trim_start_matches(crate::mw2guns::PREFIX)
        .trim_start_matches(crate::bo1::PREFIX)
        .trim_start_matches(crate::waw::PREFIX)
        .trim_end_matches("_mp");
    let mapped = cod4_equivalent(base);
    for candidate in [mapped, base] {
        if !candidate.is_empty() && crate::loadout::bot_weapon(content, candidate).is_some() {
            return candidate.to_owned();
        }
    }
    "m4".to_owned()
}

/// A gun this game can draw and fire: `spec` if it loads here, else its
/// CoD4 stand-in.
pub fn loadable(content: &Content, spec: &str) -> String {
    // (CoD4's own, guns and equipment, are everyone's.)
    if gun_game(spec) == Game::Cod4 || crate::loadout::bot_weapon(content, spec).is_some() {
        return spec.to_owned();
    }
    let to = standin(content, spec);
    // The stand-in keeps the IR laser ([`crate::laser`]).
    let to = if crate::laser::fitted(spec) { crate::laser::with_laser(&to, true) } else { to };
    // (Once a gun, for the log.)
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    if let Ok(mut seen) = SEEN.lock()
        && !seen.iter().any(|s| s == spec)
    {
        seen.push(spec.to_owned());
        bevy::log::info!("online: drawing {spec} as {to} (no {} here)", gun_game(spec).title());
    }
    to
}

/// Test aid (`COD4RW_NETTEST_CLASS=iw4_famas,t5_python`): this game's
/// player's class guns, once, as if picked in Create a Class.
pub(super) fn test_class(mut mine: bevy::prelude::Query<&mut crate::loadout::PawnClass, (bevy::prelude::With<crate::player::LocalPlayer>, bevy::prelude::Changed<crate::loadout::PawnClass>)>, mut done: bevy::prelude::Local<bool>) {
    let Ok(list) = std::env::var("COD4RW_NETTEST_CLASS") else { return };
    if *done {
        return;
    }
    for mut class in &mut mine {
        *done = true;
        for (gun, spec) in class.0.guns.iter_mut().zip(list.split(',').map(str::trim).filter(|s| !s.is_empty())) {
            *gun = crate::loadout::Gun { spec: spec.to_owned(), camo: 0, name: spec.to_ascii_uppercase(), variant: None };
        }
        bevy::log::info!("online: test class: {:?}", class.0.guns.iter().map(|g| g.spec.as_str()).collect::<Vec<_>>());
    }
}

/// Lines for the player's HUD ("Class changed: ..."), for [`crate::ui`].
static NOTICES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A gun's name for players ("M16A4"), else its spec.
pub fn gun_name(content: &Content, spec: &str) -> String {
    crate::loadout::bot_weapon(content, spec).map_or_else(
        // (Another game's, not in CoD4's content: its name without the
        // game's prefix, "FAMAS".)
        || {
            let base = spec.split(':').next().unwrap_or(spec);
            base.trim_start_matches(crate::mw2guns::PREFIX).trim_start_matches(crate::bo1::PREFIX).trim_start_matches(crate::waw::PREFIX).to_ascii_uppercase()
        },
        |d| d.display_name.to_owned(),
    )
}

pub fn notify(text: String) {
    bevy::log::info!("online: notice: {text}");
    if let Ok(mut n) = NOTICES.lock() {
        n.push(text);
    }
}

pub fn take_notices() -> Vec<String> {
    NOTICES.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn games_of_guns_maps_and_hosts() {
        assert_eq!(gun_game("iw4_m4:reflex"), Game::Mw2);
        assert_eq!(gun_game("t5_ak47"), Game::BlackOps);
        assert_eq!(gun_game("t4_thompson"), Game::WaW);
        assert_eq!(gun_game("m4:acog"), Game::Cod4);
        assert_eq!(map_need("mw2_mp_terminal"), MapNeed::Game(Game::Mw2));
        assert_eq!(map_need("mp_crash"), MapNeed::Game(Game::Cod4));
        assert_eq!(map_need("mp_4t4scrap_s"), MapNeed::Custom);
        let cod4 = bit(Game::Cod4);
        assert!(can_load_map("mp_crash", cod4) && !can_load_map("mw2_mp_rust", cod4));
        assert!(can_load_map("mw2_mp_rust", cod4 | bit(Game::Mw2)));
        let used = guns_use(["m4", "iw4_famas", "t5_ak47"]);
        assert!(!can_host(cod4 | bit(Game::Mw2), used));
        assert!(can_host(cod4 | bit(Game::Mw2) | bit(Game::BlackOps), used));
        assert_eq!(tags(cod4 | bit(Game::Mw2) | bit(Game::WaW)), vec!["MW2", "WaW"]);
    }

    #[test]
    fn equivalents_are_cod4_guns() {
        for (other, cod4) in [("famas", "m16"), ("ak47", "ak47"), ("thompson", "mp5"), ("cheytac", "m40a3"), ("python", "deserteagle"), ("stg44", "mp44")] {
            assert_eq!(cod4_equivalent(other), cod4);
        }
        assert_eq!(cod4_equivalent("nothing_like_it"), "");
    }
}
