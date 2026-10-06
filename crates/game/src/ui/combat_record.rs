//! Saved career records and the player's identity. Player 1 owns the local
//! profile, as with the existing XP/challenge system. Network verification
//! and sharing are deliberately left to the multiplayer integration.

mod menu;

use super::{Frontend, progression::stat, stats::Stats};
use crate::combat::{Dead, HitLocation, Killed, Pawn, hostile};
use crate::loadout::{AwaitingClass, Loadout};
use crate::player::LocalPlayer;
use crate::weapons::{ShotFired, WeaponState};
use bevy::prelude::*;
use std::collections::HashMap;

const NAME: &str = "cod4rw_player_name";
const CLAN: &str = "cod4rw_clan_tag";
const EMBLEM: &str = "cod4rw_emblem";
const CARD: &str = "cod4rw_calling_card";
const WEAPON_PREFIX: &str = "cod4rw_weapon_";
const GRID: usize = 16;
const PIXELS: usize = GRID * GRID;
const PALETTE: [[f32; 4]; 16] = [
    [0.0; 4],
    [0.95, 0.95, 0.92, 1.0],
    [0.09, 0.10, 0.12, 1.0],
    [0.44, 0.47, 0.50, 1.0],
    [0.80, 0.18, 0.16, 1.0],
    [0.95, 0.44, 0.16, 1.0],
    [0.91, 0.73, 0.33, 1.0],
    [0.37, 0.54, 0.30, 1.0],
    [0.13, 0.35, 0.25, 1.0],
    [0.20, 0.70, 0.64, 1.0],
    [0.20, 0.52, 0.79, 1.0],
    [0.16, 0.26, 0.52, 1.0],
    [0.49, 0.31, 0.67, 1.0],
    [0.86, 0.39, 0.62, 1.0],
    [0.43, 0.30, 0.21, 1.0],
    [0.70, 0.60, 0.45, 1.0],
];

#[derive(Clone)]
pub(super) struct Editor {
    pixels: [u8; PIXELS],
    undo: Vec<[u8; PIXELS]>,
    color: u8,
}

impl Default for Editor {
    fn default() -> Self {
        Self { pixels: default_emblem(), undo: Vec::new(), color: 6 }
    }
}

impl Editor {
    fn checkpoint(&mut self) {
        if self.undo.len() == 32 {
            self.undo.remove(0);
        }
        self.undo.push(self.pixels);
    }

    fn paint(&mut self, index: usize) {
        if index < PIXELS && self.pixels[index] != self.color {
            self.checkpoint();
            self.pixels[index] = self.color;
        }
    }
}

fn default_emblem() -> [u8; PIXELS] {
    let mut pixels = [0; PIXELS];
    // Two gold chevrons; also serves as a useful editable starting point.
    for y in 3..13 {
        for x in 2usize..14 {
            let distance = x.abs_diff(7).min(x.abs_diff(8));
            if [6usize, 10].into_iter().any(|base| y == base.saturating_sub(distance / 2)) {
                pixels[y * GRID + x] = 6;
            }
        }
    }
    pixels
}

fn encode_emblem(pixels: &[u8; PIXELS]) -> String {
    pixels.iter().map(|&n| char::from(b"0123456789abcdef"[usize::from(n.min(15))])).collect()
}

fn decode_emblem(text: &str) -> Option<[u8; PIXELS]> {
    if text.len() != PIXELS {
        return None;
    }
    let mut pixels = [0; PIXELS];
    for (slot, ch) in pixels.iter_mut().zip(text.chars()) {
        *slot = ch.to_digit(16)? as u8;
    }
    Some(pixels)
}

/// The bundled fonts are ASCII. Exclude colour escapes, localisation keys,
/// control characters and save-file line breaks from user identity fields.
fn clean_name(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || " _-.".contains(*c))
        .take(limit)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn identity(stats: &Stats) -> String {
    let name = clean_name(stats.dvars.get(NAME).map_or("Player", String::as_str), 16);
    let name = if name.is_empty() { "Player" } else { &name };
    let clan = clean_name(stats.dvars.get(CLAN).map_or("", String::as_str), 4);
    if clan.is_empty() { name.to_owned() } else { format!("[{clan}] {name}") }
}

pub(super) fn keeps_dvar(name: &str) -> bool {
    if [NAME, CLAN, EMBLEM, CARD].contains(&name) {
        return true;
    }
    let Some((weapon, metric)) = name.strip_prefix(WEAPON_PREFIX).and_then(|s| s.rsplit_once('_')) else {
        return false;
    };
    !weapon.is_empty()
        && weapon.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && ["kills", "heads", "shots", "seconds"].contains(&metric)
}

fn metric_key(weapon: &str, metric: &str) -> String {
    format!("{WEAPON_PREFIX}{weapon}_{metric}")
}
pub(super) fn metric(stats: &Stats, weapon: &str, field: &str) -> u64 {
    stats.dvars.get(&metric_key(weapon, field)).and_then(|s| s.parse().ok()).unwrap_or(0)
}
fn add_metric(stats: &mut Stats, weapon: &str, field: &str, amount: u64) {
    let key = metric_key(weapon, field);
    if keeps_dvar(&key) {
        stats.set_dvar(&key, &metric(stats, weapon, field).saturating_add(amount).to_string());
    }
}

fn ratio(numerator: i32, denominator: i32) -> String {
    if denominator <= 0 {
        if numerator <= 0 { "0.00".into() } else { "--".into() }
    } else {
        format!("{:.2}", numerator.max(0) as f64 / denominator as f64)
    }
}

fn duration(seconds: u64) -> String {
    format!("{}h {:02}m {:02}s", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

/// Resolve definitions with attachment suffixes without merging the games'
/// AK-47s or weapons whose names contain underscores (e.g. desert_eagle).
fn canonical(def: &str, known: &[String]) -> Option<String> {
    known
        .iter()
        .filter(|key| def == key.as_str() || def.strip_prefix(key.as_str()).is_some_and(|s| s.starts_with('_')))
        .max_by_key(|key| key.len())
        .cloned()
}

fn credit_kill(stats: &mut Stats, weapon: Option<String>, killed: &Killed, me: Entity, enemy: bool) {
    if killed.attacker != Some(me) || killed.victim == me || !enemy {
        return;
    }
    if let Some(key) = weapon {
        add_metric(stats, &key, "kills", 1);
        if killed.location == HitLocation::Head {
            add_metric(stats, &key, "heads", 1);
        }
    }
}

#[derive(Resource, Default)]
struct Tracking {
    // Damage uses the actual definition's static display-name allocation.
    // Remember that allocation across weapon switches, including delayed
    // Last Stand damage, instead of crediting the gun held at death time.
    definitions: HashMap<usize, String>,
    known: Vec<String>,
    seconds: f64,
    weapon_seconds: HashMap<String, f64>,
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct TrackingSet;

pub(super) fn setup(app: &mut App) {
    app.init_resource::<Tracking>()
        .add_systems(OnEnter(crate::state::GameState::InGame), |mut t: ResMut<Tracking>| *t = Tracking::default())
        .add_systems(Update, sync_identity.before(crate::movement::MovementSet).run_if(crate::state::in_game))
        // Read messages after all of the match's damage and weapon systems.
        .add_systems(PostUpdate, track.in_set(TrackingSet).run_if(crate::state::in_game));
}

fn sync_identity(fe: Option<Res<Frontend>>, mut player: Query<&mut Pawn, With<LocalPlayer>>) {
    let Some(fe) = fe else { return };
    let name = identity(&fe.stats);
    for mut pawn in &mut player {
        if pawn.name != name {
            pawn.name = name.clone();
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn track(
    time: Res<Time>,
    mut fe: Option<ResMut<Frontend>>,
    mut t: ResMut<Tracking>,
    mut killed: MessageReader<Killed>,
    mut shots: MessageReader<ShotFired>,
    player: Query<(Entity, &Pawn, &WeaponState, Option<&Loadout>, Has<Dead>, Has<AwaitingClass>), With<LocalPlayer>>,
    pawns: Query<&Pawn>,
    state: Option<Res<crate::tdm::MatchState>>,
) {
    let Some(fe) = fe.as_deref_mut() else { return };
    let Ok((me, pawn, held, loadout, dead, waiting)) = player.single() else { return };
    if t.known.is_empty() {
        t.known = menu::weapon_keys(fe);
    }
    let weapon = loadout
        .and_then(|l| l.gun_for(held.def))
        .map(|g| g.spec.split(':').next().unwrap_or("").to_owned())
        .filter(|key| t.known.contains(key))
        .or_else(|| canonical(&held.def.name, &t.known));
    if let Some(key) = &weapon {
        t.definitions.insert(held.def.display_name.as_ptr() as usize, key.clone());
    }

    if !waiting && state.as_ref().is_none_or(|s| s.ended.is_none()) {
        let dt = time.delta_secs_f64();
        t.seconds += dt;
        let seconds = t.seconds.floor() as i32;
        if seconds > 0 {
            fe.stats.add(stat::TIME_PLAYED_TOTAL, seconds);
            t.seconds -= f64::from(seconds);
        }
        if !dead && let Some(key) = &weapon {
            let pending = t.weapon_seconds.entry(key.clone()).or_default();
            *pending += dt;
            let seconds = pending.floor() as u64;
            if seconds > 0 {
                add_metric(&mut fe.stats, key, "seconds", seconds);
                *pending -= seconds as f64;
            }
        }
    }
    for shot in shots.read().filter(|s| s.shooter == me) {
        if let Some(def) = shot.weapon
            && let Some(key) = canonical(&def.name, &t.known)
        {
            t.definitions.insert(def.display_name.as_ptr() as usize, key.clone());
            add_metric(&mut fe.stats, &key, "shots", 1);
        }
    }
    for k in killed.read() {
        let weapon = t
            .definitions
            .get(&(k.weapon.as_ptr() as usize))
            .cloned()
            // Projectile damage uses a static launcher label, rather than
            // the allocated weapon display name used by bullet damage.
            .or_else(|| (k.weapon == "RPG-7").then(|| "rpg".into()));
        let enemy = pawns.get(k.victim).is_ok_and(|v| hostile(pawn, v));
        credit_kill(&mut fe.stats, weapon, k, me, enemy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emblem_round_trip_and_rejects_bad_saves() {
        let pixels = default_emblem();
        assert_eq!(decode_emblem(&encode_emblem(&pixels)), Some(pixels));
        assert!(decode_emblem("abc").is_none());
        assert!(decode_emblem(&"z".repeat(PIXELS)).is_none());
        assert!(decode_emblem(&"é".repeat(PIXELS / 2)).is_none());
    }

    #[test]
    fn editor_undo_is_bounded_and_preserves_pixels() {
        let mut editor = Editor { pixels: [0; PIXELS], color: 4, ..Editor::default() };
        editor.paint(PIXELS);
        assert!(editor.undo.is_empty());
        editor.paint(2);
        editor.paint(2);
        assert_eq!(editor.undo.len(), 1);
        editor.pixels = editor.undo.pop().unwrap();
        assert_eq!(editor.pixels[2], 0);
        for i in 0..40 {
            editor.paint(i);
        }
        assert_eq!(editor.undo.len(), 32);
    }

    #[test]
    fn identity_fields_cannot_inject_save_lines_or_ui_markup() {
        assert_eq!(clean_name("^1Bob\n@MENU;\t😀", 16), "1BobMENU");
        assert_eq!(clean_name("  A B  ", 16), "A B");
        assert_eq!(clean_name("abcdefgh", 4), "abcd");
        assert!(keeps_dvar("cod4rw_weapon_t5_ak47_kills"));
        assert!(!keeps_dvar("cod4rw_weapon_ak47_unknown"));
        assert!(!keeps_dvar("cod4rw_weapon_ak47\nkills_kills"));
        assert!(!keeps_dvar("cr_name_draft"));
    }

    #[test]
    fn canonical_weapon_keeps_cross_game_and_underscore_names_distinct() {
        let known = ["ak47", "t5_ak47", "desert_eagle", "m4"].map(str::to_owned);
        assert_eq!(canonical("t5_ak47_reflex_mp", &known).as_deref(), Some("t5_ak47"));
        assert_eq!(canonical("desert_eagle_mp", &known).as_deref(), Some("desert_eagle"));
        assert_eq!(canonical("ak47_silencer_mp", &known).as_deref(), Some("ak47"));
        assert!(canonical("m40a3_mp", &known).is_none());
        assert!(canonical("frag_grenade_mp", &known).is_none());
    }

    #[test]
    fn ratios_and_time_handle_empty_profiles() {
        assert_eq!(ratio(0, 0), "0.00");
        assert_eq!(ratio(5, 0), "--");
        assert_eq!(ratio(3, 2), "1.50");
        assert_eq!(duration(3661), "1h 01m 01s");
    }

    #[test]
    fn weapon_kills_use_the_recorded_hit_and_exclude_other_killers_and_friendlies() {
        let mut world = World::new();
        let me = world.spawn_empty().id();
        let enemy = world.spawn_empty().id();
        let other = world.spawn_empty().id();
        let mut stats = Stats::in_memory();
        let weapon = Some("t5_ak47".into());
        let kill = Killed { victim: enemy, attacker: Some(me), weapon: "AK-47", location: HitLocation::Head };
        credit_kill(&mut stats, weapon.clone(), &kill, me, true);
        assert_eq!(metric(&stats, "t5_ak47", "kills"), 1);
        assert_eq!(metric(&stats, "t5_ak47", "heads"), 1);
        assert_eq!(metric(&stats, "ak47", "kills"), 0);
        credit_kill(&mut stats, weapon.clone(), &kill, me, false);
        credit_kill(&mut stats, weapon.clone(), &Killed { attacker: Some(other), ..kill.clone() }, me, true);
        credit_kill(&mut stats, weapon, &Killed { victim: me, ..kill.clone() }, me, true);
        credit_kill(&mut stats, None, &kill, me, true);
        assert_eq!(metric(&stats, "t5_ak47", "kills"), 1);
    }

    #[test]
    fn weapon_counters_are_saved_and_do_not_overflow() {
        let mut stats = Stats::in_memory();
        add_metric(&mut stats, "ak47", "kills", u64::MAX);
        add_metric(&mut stats, "ak47", "kills", 1);
        assert_eq!(metric(&stats, "ak47", "kills"), u64::MAX);
        assert!(Stats::keeps(&metric_key("ak47", "kills")));
        add_metric(&mut stats, "malformed\nweapon", "kills", 1);
        assert_eq!(stats.dvars.len(), 1);
    }
}
