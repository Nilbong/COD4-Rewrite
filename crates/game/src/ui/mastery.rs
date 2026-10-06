//! Per-weapon CoD4 mastery, using the Combat Record's saved finishing-hit
//! counts. Unlock flags live in the native weapon stat; completion/XP
//! flags use an otherwise unused range so loading old profiles is safe.

use super::{Frontend, assets::UiAssets, combat_record, hud::HudState, progression, stats::Stats};
use bevy::prelude::*;
use iw3::menu::StringTable;
use std::collections::HashMap;

pub(super) const GOLD: usize = 6;
pub(super) const PLATINUM: usize = 200;
pub(super) const DIAMOND: usize = 201;
pub(super) const GOLD_BIT: i32 = 8192;
pub(super) const PLATINUM_BIT: i32 = 16384;
pub(super) const DIAMOND_BIT: i32 = 32768;
/// RPG availability is the native perk-1 inventory stat, rather than the
/// projectile weapon's separate camo-mask stat (3055).
pub(super) const RPG_PERK_STAT: i32 = 186;
#[cfg(feature = "diamond")]
const CAMO_BITS: i32 = GOLD_BIT | PLATINUM_BIT | DIAMOND_BIT;
#[cfg(not(feature = "diamond"))]
const CAMO_BITS: i32 = GOLD_BIT | PLATINUM_BIT;
pub(super) const GOLD_KILLS: u64 = 150;
pub(super) const GOLD_HEADSHOTS: u64 = 150;
pub(super) const PLATINUM_KILLS: u64 = 500;
const COMPLETED_BASE: i32 = 7000;
/// The mastery camos, in order. Diamond is a test feature (`--features
/// diamond`); it's last, so the others' stats don't move without it.
#[cfg(feature = "diamond")]
pub(crate) const CAMOS: &[usize] = &[GOLD, PLATINUM, DIAMOND];
#[cfg(not(feature = "diamond"))]
pub(crate) const CAMOS: &[usize] = &[GOLD, PLATINUM];

#[derive(Clone, Debug)]
pub(super) struct Weapon {
    pub key: String,
    pub index: i32,
    pub unlock_stat: i32,
    pub name: String,
    pub family: String,
}

#[derive(Clone, Debug)]
pub(super) struct Challenge {
    pub current: u64,
    pub target: u64,
    /// Earned through play, independently of the Unlock All setting.
    pub complete: bool,
    pub description: String,
}

pub(super) fn canonical(weapon: &str) -> &str {
    if weapon == "deserteaglegold" { "deserteagle" } else { weapon }
}

pub(super) fn camo_name(camo: usize) -> &'static str {
    match camo {
        GOLD => "Gold",
        PLATINUM => "Platinum",
        DIAMOND => "Diamond",
        _ => "",
    }
}

pub(super) fn bit(camo: usize) -> i32 {
    match camo {
        GOLD => GOLD_BIT,
        PLATINUM => PLATINUM_BIT,
        DIAMOND => DIAMOND_BIT,
        _ => 0,
    }
}

fn family(kind: &str, key: &str) -> Option<&'static str> {
    match kind {
        "weapon_assault" => Some("Assault Rifles"),
        "weapon_smg" => Some("Submachine Guns"),
        "weapon_lmg" => Some("Light Machine Guns"),
        "weapon_sniper" => Some("Sniper Rifles"),
        "weapon_shotgun" => Some("Shotguns"),
        "weapon_pistol" => Some("Pistols"),
        "weapon_projectile" if key == "rpg" => Some("Launchers"),
        _ => None,
    }
}

fn table_weapons(t: &StringTable, localize: impl Fn(&str) -> String) -> Vec<Weapon> {
    let mut out = Vec::new();
    for r in 0..t.rows {
        let key = t.get(r, 4).unwrap_or("").trim();
        let Some(family) = family(t.get(r, 2).unwrap_or("").trim(), key) else { continue };
        let Some(index) = t.get(r, 0).and_then(|s| s.parse::<i32>().ok()) else { continue };
        if key.is_empty() || !(0..crate::bo1::FIRST_INDEX).contains(&index) {
            continue;
        }
        let name = localize(&format!("@{}", t.get(r, 3).unwrap_or("")));
        out.push(Weapon { key: key.into(), index, unlock_stat: 3000 + index, name, family: family.into() });
    }
    out.sort_by_key(|w| w.index);
    out
}

/// Includes the separate native gold Desert Eagle menu entry; it shares
/// requirements with the ordinary Desert Eagle, rather than adding a grind.
pub(super) fn weapons(assets: &UiAssets) -> Vec<Weapon> {
    assets.table("mp/statstable.csv").map_or_else(Vec::new, |t| table_weapons(t, |s| assets.localize(s)))
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Counts {
    kills: u64,
    heads: u64,
}

fn career_counts(stats: &Stats, weapon: &str) -> Counts {
    let key = canonical(weapon);
    let mut counts =
        Counts { kills: combat_record::metric(stats, key, "kills"), heads: combat_record::metric(stats, key, "heads") };
    if key == "deserteagle" {
        counts.kills = counts.kills.saturating_add(combat_record::metric(stats, "deserteaglegold", "kills"));
        counts.heads = counts.heads.saturating_add(combat_record::metric(stats, "deserteaglegold", "heads"));
    }
    counts
}

/// Native Marksman/Expert counters are cumulative, shared by their stages.
/// Include completed-stage targets for older saves which retained the
/// challenge state but not its counter. Do not infer kills from unlock bits:
/// old Unlock All profiles contain those without having earned them.
fn native_counts(stats: &Stats, assets: &UiAssets) -> HashMap<String, Counts> {
    let mut out = HashMap::<String, Counts>::new();
    for tier in 1..=10 {
        let Some(t) = assets.table(&format!("mp/challengeTable_tier{tier}.csv")) else { continue };
        let mut state = 0;
        for r in 0..t.rows {
            if let Some(index) = t.get(r, 2).and_then(|s| s.parse::<i32>().ok()) {
                state = stats.get(index);
            }
            let reference = t.get(r, 7).unwrap_or("");
            let (rest, head) = if let Some(s) = reference.strip_prefix("ch_marksman_") {
                (s, false)
            } else if let Some(s) = reference.strip_prefix("ch_expert_") {
                (s, true)
            } else {
                continue;
            };
            let Some((key, stage)) = rest.rsplit_once('_').and_then(|(k, s)| Some((k, s.parse::<i32>().ok()?))) else {
                continue;
            };
            let Some(index) = t.get(r, 3).and_then(|s| s.parse::<i32>().ok()) else { continue };
            let target = t.get(r, 4).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let mut count = stats.get(index).max(0) as u64;
            if state == progression::CHALLENGE_DONE || state > stage {
                count = count.max(target);
            }
            let counts = out.entry(canonical(key).into()).or_default();
            if head {
                counts.heads = counts.heads.max(count);
            } else {
                counts.kills = counts.kills.max(count);
            }
        }
    }
    out
}

fn counts(stats: &Stats, native: &HashMap<String, Counts>, weapon: &str) -> Counts {
    let mut result = career_counts(stats, weapon);
    if let Some(old) = native.get(canonical(weapon)) {
        result.kills = result.kills.max(old.kills);
        result.heads = result.heads.max(old.heads);
    }
    // Every headshot kill was also a kill, including guns without a native
    // Marksman challenge (the MP44 and pistols).
    result.kills = result.kills.max(result.heads);
    result
}

fn unique_weapons(weapons: &[Weapon]) -> impl Iterator<Item = &Weapon> {
    weapons.iter().filter(|w| canonical(&w.key) == w.key)
}

fn gold(weapon: &Weapon, count: Counts) -> bool {
    count.kills >= GOLD_KILLS && (weapon.family == "Launchers" || count.heads >= GOLD_HEADSHOTS)
}

fn platinum(weapon: &Weapon, count: Counts) -> bool {
    gold(weapon, count) && count.kills >= PLATINUM_KILLS
}

fn challenge(
    stats: &Stats,
    native: &HashMap<String, Counts>,
    all: &[Weapon],
    weapon: &Weapon,
    camo: usize,
) -> Challenge {
    let count = counts(stats, native, &weapon.key);
    match camo {
        GOLD => {
            let launcher = weapon.family == "Launchers";
            Challenge {
                current: count.kills.min(GOLD_KILLS) + if launcher { 0 } else { count.heads.min(GOLD_HEADSHOTS) },
                target: GOLD_KILLS + if launcher { 0 } else { GOLD_HEADSHOTS },
                complete: gold(weapon, count),
                description: if launcher {
                    format!("Get {GOLD_KILLS} kills with this weapon ({}/{GOLD_KILLS}).", count.kills)
                } else {
                    format!(
                        "Get {GOLD_KILLS} kills and {GOLD_HEADSHOTS} headshots with this weapon ({}/{GOLD_KILLS} kills, {}/{GOLD_HEADSHOTS} headshots).",
                        count.kills, count.heads
                    )
                },
            }
        }
        PLATINUM => Challenge {
            current: count.kills.min(PLATINUM_KILLS),
            target: PLATINUM_KILLS,
            complete: platinum(weapon, count),
            description: format!(
                "Earn Gold and get {PLATINUM_KILLS} total kills with this weapon ({}/{PLATINUM_KILLS}).",
                count.kills
            ),
        },
        DIAMOND => {
            let family: Vec<_> = unique_weapons(all).filter(|w| w.family == weapon.family).collect();
            let current = family.iter().filter(|w| platinum(w, counts(stats, native, &w.key))).count() as u64;
            let target = family.len() as u64;
            Challenge {
                current,
                target,
                complete: target > 0 && current == target,
                description: format!(
                    "Earn Gold and Platinum on every weapon in {} ({current}/{target}).",
                    weapon.family
                ),
            }
        }
        _ => Challenge { current: 0, target: 0, complete: false, description: String::new() },
    }
}

pub(super) fn progress(stats: &Stats, assets: &UiAssets, weapon: &str, camo: usize) -> Challenge {
    let all = weapons(assets);
    let Some(w) = all.iter().find(|w| w.key == weapon) else {
        return Challenge { current: 0, target: 0, complete: false, description: String::new() };
    };
    challenge(stats, &native_counts(stats, assets), &all, w, camo)
}

pub(super) fn unlocked(stats: &Stats, assets: &UiAssets, weapon: &str, camo: usize) -> bool {
    let Some(w) = weapons(assets).into_iter().find(|w| w.key == weapon) else { return false };
    let availability = if w.key == "rpg" { RPG_PERK_STAT } else { w.unlock_stat };
    bit(camo) != 0
        && stats.get(availability) & 1 != 0
        && (!progression::cod4_unlocks(stats) || progress(stats, assets, weapon, camo).complete)
}

/// Copy old counters into the persistent career floor once. Variant
/// metrics stay separate; subtract that contribution when backfilling the
/// ordinary Desert Eagle so repeated refreshes cannot add it twice.
fn backfill(stats: &mut Stats, native: &HashMap<String, Counts>, all: &[Weapon]) {
    for w in unique_weapons(all) {
        let value = counts(stats, native, &w.key);
        for (field, total) in [("kills", value.kills), ("heads", value.heads)] {
            let variant =
                if w.key == "deserteagle" { combat_record::metric(stats, "deserteaglegold", field) } else { 0 };
            let base = total.saturating_sub(variant);
            if base > combat_record::metric(stats, &w.key, field) {
                stats.set_dvar(&format!("cod4rw_weapon_{}_{field}", w.key), &base.to_string());
            }
        }
    }
}

fn refresh_with(stats: &mut Stats, native: &HashMap<String, Counts>, all: &[Weapon]) {
    backfill(stats, native, all);
    let unrestricted = !progression::cod4_unlocks(stats);
    for w in all {
        let mut value = stats.get(w.unlock_stat) & !CAMO_BITS;
        if w.key == "rpg" {
            // Native camo-row expressions read a weapon mask, including
            // None's bit 0. Keep that mask's availability in step with the
            // actual equipment unlock, even in old profiles missing 3055.
            value = (value & !1) | (stats.get(RPG_PERK_STAT) & 1);
        }
        for &camo in CAMOS {
            if (w.key != "rpg" || value & 1 != 0) && (unrestricted || challenge(stats, native, all, w, camo).complete) {
                value |= bit(camo);
            }
        }
        stats.set(w.unlock_stat, value);
    }
}

pub(super) fn refresh(stats: &mut Stats, assets: &UiAssets) {
    let native = native_counts(stats, assets);
    let all = weapons(assets);
    refresh_with(stats, &native, &all);
}

fn completion_stat(weapon: &Weapon, camo: usize) -> i32 {
    COMPLETED_BASE + weapon.index * 3 + CAMOS.iter().position(|&c| c == camo).unwrap_or(0) as i32
}

/// Persistent grants are separate from selectable unlock bits. Unlock All
/// never awards mastery XP or marks a challenge earned by itself.
fn completions(stats: &mut Stats, native: &HashMap<String, Counts>, all: &[Weapon]) -> Vec<(String, usize, i32)> {
    let mut out = Vec::new();
    for w in unique_weapons(all) {
        for &camo in CAMOS {
            let index = completion_stat(w, camo);
            if stats.get(index) == 0 && challenge(stats, native, all, w, camo).complete {
                stats.set(index, 1);
                out.push((
                    w.name.clone(),
                    camo,
                    match camo {
                        GOLD => 1000,
                        PLATINUM => 2500,
                        _ => 5000,
                    },
                ));
            }
        }
    }
    out
}

#[derive(Resource, Default)]
struct Tracking {
    weapons: Vec<Weapon>,
    native_stats: Vec<i32>,
    snapshot: Vec<u64>,
    mode: Option<bool>,
    last_saved: f32,
}

pub(super) fn setup(app: &mut App) {
    app.init_resource::<Tracking>()
        .add_systems(OnEnter(crate::state::GameState::InGame), |mut t: ResMut<Tracking>| *t = Tracking::default())
        .add_systems(PostUpdate, update.after(combat_record::TrackingSet).run_if(crate::state::in_game));
}

fn update(
    time: Res<Time>,
    mut fe: Option<ResMut<Frontend>>,
    mut t: ResMut<Tracking>,
    mut hud: ResMut<HudState>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let Some(fe) = fe.as_deref_mut() else { return };
    if t.weapons.is_empty() {
        t.weapons = weapons(&fe.assets);
        t.native_stats = progression::challenges(&fe.assets)
            .into_iter()
            .filter(|c| c.name.starts_with("ch_marksman_") || c.name.starts_with("ch_expert_"))
            .flat_map(|c| c.levels.into_iter().map(|l| l.progress))
            .collect();
        t.native_stats.sort_unstable();
        t.native_stats.dedup();
    }
    let now = time.elapsed_secs();
    let mut snapshot = Vec::with_capacity(t.weapons.len() * 2 + t.native_stats.len());
    for w in &t.weapons {
        snapshot.extend([
            combat_record::metric(&fe.stats, &w.key, "kills"),
            combat_record::metric(&fe.stats, &w.key, "heads"),
        ]);
    }
    snapshot.extend(t.native_stats.iter().map(|&i| fe.stats.get(i).max(0) as u64));
    let mode = progression::cod4_unlocks(&fe.stats);
    if t.mode != Some(mode) || t.snapshot != snapshot {
        let native = native_counts(&fe.stats, &fe.assets);
        refresh_with(&mut fe.stats, &native, &t.weapons);
        let complete = completions(&mut fe.stats, &native, &t.weapons);
        for (weapon, camo, xp) in &complete {
            let text = format!("{weapon} {}", camo_name(*camo));
            hud.notify(fe.assets.localize("@CHALLENGE_COMPLETED"), text.clone(), String::new(), now);
            hud.message(format!("Unlocked {}: {weapon}", camo_name(*camo)), now);
            progression::give_xp(fe, &mut hud, *xp, now);
            info!("mastery: {text} completed (+{xp} XP)");
        }
        if !complete.is_empty() {
            sfx.play("mp_challenge_complete", None);
            fe.stats.save_if_changed();
        }
        t.snapshot = snapshot;
        t.mode = Some(mode);
    }
    // Preserve kills from a match left before the match bonus, without
    // writing the profile once per frame or once per bullet.
    if now - t.last_saved >= 10.0 {
        fe.stats.save_if_changed();
        t.last_saved = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weapon(key: &str, index: i32, family: &str) -> Weapon {
        Weapon { key: key.into(), index, unlock_stat: 3000 + index, name: key.into(), family: family.into() }
    }
    fn metric(stats: &mut Stats, key: &str, field: &str, value: u64) {
        stats.set_dvar(&format!("cod4rw_weapon_{key}_{field}"), &value.to_string());
    }

    #[test]
    fn mastery_requires_each_goal_and_family_completion() {
        let all = [
            weapon("ak47", 20, "Assault Rifles"),
            weapon("m4", 26, "Assault Rifles"),
            weapon("mp5", 10, "Submachine Guns"),
        ];
        let mut stats = Stats::in_memory();
        let native = HashMap::new();
        stats.set_dvar(progression::UNLOCKS_DVAR, "cod4");
        metric(&mut stats, "ak47", "kills", 500);
        metric(&mut stats, "ak47", "heads", 149);
        assert!(!challenge(&stats, &native, &all, &all[0], GOLD).complete);
        assert!(!challenge(&stats, &native, &all, &all[0], PLATINUM).complete);
        metric(&mut stats, "ak47", "heads", 150);
        assert!(challenge(&stats, &native, &all, &all[0], PLATINUM).complete);
        assert_eq!(challenge(&stats, &native, &all, &all[0], DIAMOND).current, 1);
        assert!(!challenge(&stats, &native, &all, &all[0], DIAMOND).complete);
        metric(&mut stats, "m4", "kills", 500);
        metric(&mut stats, "m4", "heads", 150);
        assert!(challenge(&stats, &native, &all, &all[0], DIAMOND).complete);
        assert!(!challenge(&stats, &native, &all, &all[2], DIAMOND).complete);
    }

    #[test]
    fn unlock_all_does_not_earn_xp_and_completed_challenges_pay_once() {
        let all = [weapon("ak47", 20, "Assault Rifles")];
        let mut stats = Stats::in_memory();
        let native = HashMap::new();
        stats.set(3020, 1 | 65536 | 2);
        stats.set_dvar(progression::UNLOCKS_DVAR, "all");
        refresh_with(&mut stats, &native, &all);
        assert_eq!(stats.get(3020), 1 | 65536 | 2 | CAMO_BITS);
        assert!(completions(&mut stats, &native, &all).is_empty());
        stats.set_dvar(progression::UNLOCKS_DVAR, "cod4");
        refresh_with(&mut stats, &native, &all);
        assert_eq!(stats.get(3020), 1 | 65536 | 2);
        metric(&mut stats, "ak47", "kills", 500);
        metric(&mut stats, "ak47", "heads", 150);
        assert_eq!(completions(&mut stats, &native, &all).len(), CAMOS.len());
        assert!(completions(&mut stats, &native, &all).is_empty());
    }

    #[test]
    fn gold_desert_eagle_shares_progress_without_double_backfill() {
        let all = [weapon("deserteagle", 3, "Pistols"), weapon("deserteaglegold", 4, "Pistols")];
        let mut stats = Stats::in_memory();
        metric(&mut stats, "deserteagle", "kills", 400);
        metric(&mut stats, "deserteagle", "heads", 100);
        metric(&mut stats, "deserteaglegold", "kills", 100);
        metric(&mut stats, "deserteaglegold", "heads", 50);
        let native = HashMap::from([("deserteagle".into(), Counts { kills: 550, heads: 175 })]);
        for _ in 0..3 {
            refresh_with(&mut stats, &native, &all);
        }
        assert_eq!(career_counts(&stats, "deserteagle"), Counts { kills: 550, heads: 175 });
        if cfg!(feature = "diamond") {
            let diamond = challenge(&stats, &native, &all, &all[1], DIAMOND);
            assert_eq!((diamond.current, diamond.target, diamond.complete), (1, 1, true));
        }
        assert_eq!(completions(&mut stats, &native, &all).len(), CAMOS.len());
    }

    #[test]
    fn launchers_have_reachable_kill_only_goals_and_legacy_progress_is_a_floor() {
        let all = [weapon("rpg", 55, "Launchers"), weapon("mp44", 22, "Assault Rifles")];
        let mut stats = Stats::in_memory();
        metric(&mut stats, "rpg", "kills", 150);
        let native = HashMap::from([("mp44".into(), Counts { kills: 0, heads: 150 })]);
        backfill(&mut stats, &native, &all);
        assert!(challenge(&stats, &native, &all, &all[0], GOLD).complete);
        assert!(!challenge(&stats, &native, &all, &all[0], PLATINUM).complete);
        assert_eq!(career_counts(&stats, "mp44"), Counts { kills: 150, heads: 150 });
        metric(&mut stats, "rpg", "kills", 500);
        assert!(challenge(&stats, &native, &all, &all[0], PLATINUM).complete);
    }

    #[test]
    #[ignore = "requires a local CoD4 asset installation"]
    fn native_inventory_excludes_attachments_explosives_and_other_games() {
        let assets = UiAssets::load().expect("installed native UI assets");
        let all = weapons(&assets);
        assert_eq!(all.len(), 28);
        assert_eq!(unique_weapons(&all).count(), 27);
        assert!(all.iter().any(|w| w.key == "rpg" && w.index == 55));
        assert!(!all.iter().any(|w| ["gl", "c4", "claymore"].contains(&w.key.as_str())
            || w.key.starts_with("t5_")
            || w.key.starts_with("t4_")));
    }

    #[test]
    #[ignore = "requires a local CoD4 asset installation"]
    fn native_profiles_keep_original_progress_and_gate_every_supported_gun() {
        let assets = UiAssets::load().expect("installed native UI assets");
        let all = weapons(&assets);
        let mut stats = Stats::in_memory();
        stats.set_dvar(progression::UNLOCKS_DVAR, "cod4");
        stats.set(progression::stat::RANK, 54);
        progression::refresh_unlocks(&mut stats, &assets);
        assert!(!unlocked(&stats, &assets, "ak47", GOLD));
        // An older profile with Expert III complete has already earned
        // its native progress even if the saved progress counter is absent.
        stats.set(504, progression::CHALLENGE_DONE);
        refresh(&mut stats, &assets);
        assert_eq!(combat_record::metric(&stats, "ak47", "heads"), 150);
        assert!(unlocked(&stats, &assets, "ak47", GOLD));
        assert!(!unlocked(&stats, &assets, "ak47", PLATINUM));
        for w in unique_weapons(&all) {
            metric(&mut stats, &w.key, "kills", PLATINUM_KILLS);
            if w.key != "rpg" {
                metric(&mut stats, &w.key, "heads", GOLD_HEADSHOTS);
            }
        }
        progression::refresh_unlocks(&mut stats, &assets);
        for w in &all {
            assert_eq!(stats.get(w.unlock_stat) & CAMO_BITS, CAMO_BITS, "{}", w.key);
            for &camo in CAMOS {
                assert!(progress(&stats, &assets, &w.key, camo).complete, "{}/{camo}", w.key);
                assert!(unlocked(&stats, &assets, &w.key, camo), "{}/{camo}", w.key);
            }
        }
        assert!(!unlocked(&stats, &assets, "t5_ak47", GOLD));
        assert!(!unlocked(&stats, &assets, "gl", DIAMOND));
    }

    #[test]
    #[ignore = "requires a local CoD4 asset installation"]
    fn rpg_camos_follow_the_native_equipment_unlock_and_reachable_kill_goals() {
        let assets = UiAssets::load().expect("installed native UI assets");
        let mut stats = Stats::in_memory();
        stats.set_dvar(progression::UNLOCKS_DVAR, "cod4");
        stats.set(RPG_PERK_STAT, 1);
        // Older profiles can have the RPG perk without a weapon mask.
        assert_eq!(stats.get(3055), 0);
        refresh(&mut stats, &assets);
        assert_eq!(stats.get(3055) & 1, 1, "None is available with the RPG perk");
        assert!(!unlocked(&stats, &assets, "rpg", GOLD));
        metric(&mut stats, "rpg", "kills", GOLD_KILLS);
        refresh(&mut stats, &assets);
        assert!(unlocked(&stats, &assets, "rpg", GOLD));
        assert!(!unlocked(&stats, &assets, "rpg", PLATINUM));
        metric(&mut stats, "rpg", "kills", PLATINUM_KILLS);
        refresh(&mut stats, &assets);
        for &camo in CAMOS {
            assert!(unlocked(&stats, &assets, "rpg", camo));
            assert_ne!(stats.get(3055) & bit(camo), 0);
        }
        assert_eq!(combat_record::metric(&stats, "rpg", "heads"), 0);
        stats.set(RPG_PERK_STAT, 0);
        refresh(&mut stats, &assets);
        assert_eq!(stats.get(3055) & (CAMO_BITS | 1), 0);
        for &camo in CAMOS {
            assert!(!unlocked(&stats, &assets, "rpg", camo));
        }
    }
}
