//! The player's progress, as CoD4's `_rank.gsc` keeps it: XP for kills
//! (10 in team games, 5 in free-for-all), headshots (as much again),
//! assists (2: damage on someone a teammate then kills), objectives
//! ([`Award`]), challenges (their own XP), and the match bonus at the end
//! (`updateMatchBonusScores`: the win, loss or tie scale, 1/0.5/0.75 or
//! Search and Destroy's 2/1/1.5, × the score per minute of the rank,
//! `3 + level / 2`, × the minutes played). XP climbs the ranks of `mp/rankTable.csv`, with "Promoted!"
//! and its sound at each. Everything goes in CoD4's own stats
//! (`mp/playerStatsTable.csv`: 2301 RANKXP, 2303 KILLS, 2350 RANK, ...),
//! kept with the rest ([`super::stats`]).
//!
//! Ranks bring weapons, perks and features, and each gun's Marksman (kills)
//! and Expert (headshots) challenges its sights, silencer and camos, as
//! `mp/rankTable.csv` and `mp/challengeTable_tierN.csv` say. Whether they
//! are locked until then is the player's choice ([`UNLOCKS_DVAR`]); the
//! XP, ranks, challenges and their notices are the same either way.

use super::Frontend;
use super::assets::UiAssets;
use super::hud::HudState;
use super::stats::Stats;
use crate::combat::{Damage, Killed, Pawn, free_for_all, hostile};
use crate::player::LocalPlayer;
use bevy::prelude::*;
use std::collections::HashMap;

pub(super) fn setup(app: &mut App) {
    app.add_message::<Award>().init_resource::<Progress>().add_systems(
        Update,
        (assists, award, award_slots, match_bonus, rank_up).chain().after(crate::weapons::WeaponSet).run_if(crate::state::in_game),
    )
    .add_systems(OnEnter(crate::state::GameState::InGame), |mut p: ResMut<Progress>| *p = Progress::default());
    if let Ok(dir) = std::env::var("COD4RW_CHALLENGETEST") {
        app.insert_resource(TestDir(dir.into())).add_systems(Update, test.before(assists).run_if(crate::state::in_game));
    }
}

/// XP for something the game counts (`registerScoreInfo`): a pawn
/// capturing or defending an objective.
#[derive(Message, Clone, Copy, Debug)]
pub struct Award {
    pub pawn: Entity,
    pub kind: AwardKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AwardKind {
    Capture,
    /// A kill near what your team holds, or (Headquarters) holding it.
    Defend,
    /// A kill near what the enemy holds.
    Assault,
    Plant,
    Defuse,
    /// Calling in a hardpoint (`_hardpoints.gsc`'s `"hardpoint"`).
    Hardpoint,
}

impl AwardKind {
    /// Each mode's `registerScoreInfo`: Domination and Headquarters capture
    /// 15, defend and assault 5; Search and Destroy plant and defuse 10;
    /// Sabotage plant 20, defuse 15.
    fn xp(self) -> i32 {
        use crate::modes::GameMode as M;
        match (self, crate::modes::current()) {
            (AwardKind::Capture, _) => 15,
            (AwardKind::Defend | AwardKind::Assault, _) => 5,
            (AwardKind::Plant, M::Sab) => 20,
            (AwardKind::Defuse, M::Sab) => 15,
            (AwardKind::Plant | AwardKind::Defuse | AwardKind::Hardpoint, _) => 10,
        }
    }
}

/// Each mode's kill, headshot and assist XP (`_rank.gsc` and the modes'
/// `registerScoreInfo`): 10/10/2 in team play, 5/5/1 in Domination and
/// Headquarters, 5/5/2 in Search and Destroy, 5/5/0 free for all. A
/// headshot is paid instead of the kill, not on top.
fn kill_values() -> (i32, i32, i32) {
    use crate::modes::GameMode as M;
    match crate::modes::current() {
        M::Ffa => (5, 5, 0),
        M::Dom | M::Koth => (5, 5, 1),
        M::Sd => (5, 5, 2),
        M::Tdm | M::Tdm3 | M::Sab => (10, 10, 2),
    }
}

/// `giveRankXP`'s `max(1, int(10 / numLives))`: one-life Search and
/// Destroy pays ten times over.
fn lives_scale() -> i32 {
    if crate::modes::current() == crate::modes::GameMode::Sd { 10 } else { 1 }
}

/// CoD4's player stats (`mp/playerStatsTable.csv`).
pub mod stat {
    pub const RANKXP: i32 = 2301;
    pub const SCORE: i32 = 2302;
    pub const KILLS: i32 = 2303;
    pub const KILL_STREAK: i32 = 2304;
    pub const DEATHS: i32 = 2305;
    pub const ASSISTS: i32 = 2307;
    pub const HEADSHOTS: i32 = 2308;
    pub const TIME_PLAYED_TOTAL: i32 = 2314;
    pub const WINS: i32 = 2316;
    pub const LOSSES: i32 = 2317;
    pub const RANK: i32 = 2350;
    pub const MINXP: i32 = 2351;
    pub const MAXXP: i32 = 2352;
    pub const LASTXP: i32 = 2353;
}

/// This match's progress.
#[derive(Resource, Default)]
pub struct Progress {
    /// Who hurt whom since they last spawned, for assists: victim → attackers.
    damaged: HashMap<Entity, Vec<Entity>>,
    /// The last hit on each victim (for headshot kills).
    last_hit: HashMap<Entity, crate::combat::HitLocation>,
    streak: i32,
    /// The player's assists already given XP.
    counted_assists: i32,
    /// Seconds played (alive or waiting to respawn, not picking a class).
    pub(super) played: f32,
    /// The match bonus has been given.
    bonus_given: bool,
}

/// Assists for the scoreboard, everyone's: whoever hurt a pawn that
/// someone on their side then killed. Also notes the last hit on each.
fn assists(
    mut progress: ResMut<Progress>,
    mut damage: MessageReader<Damage>,
    mut killed: MessageReader<Killed>,
    mut pawns: Query<&mut Pawn>,
    health: Query<&crate::combat::Health>,
) {
    // `Callback_PlayerDamage` forgets who hurt someone once they're hurt
    // again at full health: those back to full have no one to credit.
    let full = crate::combat::max_health();
    progress.damaged.retain(|e, _| health.get(*e).is_ok_and(|h| h.current < full));
    for d in damage.read() {
        progress.last_hit.insert(d.target, d.location);
        if let Some(a) = d.attacker.filter(|&a| a != d.target) {
            let list = progress.damaged.entry(d.target).or_default();
            if !list.contains(&a) {
                list.push(a);
            }
        }
    }
    for k in killed.read() {
        let helpers = progress.damaged.remove(&k.victim).unwrap_or_default();
        // No assists free for all.
        if free_for_all() {
            continue;
        }
        let Ok(victim) = pawns.get(k.victim).cloned() else { continue };
        for h in helpers.into_iter().filter(|&h| Some(h) != k.attacker) {
            if let Ok(mut p) = pawns.get_mut(h) {
                if hostile(&p, &victim) {
                    p.assists += 1;
                }
            }
        }
    }
}

/// XP and stats for the player's kills, assists, deaths and awards.
#[allow(clippy::too_many_arguments)]
fn award(
    time: Res<Time>,
    mut fe: Option<ResMut<Frontend>>,
    mut hud: ResMut<HudState>,
    mut progress: ResMut<Progress>,
    mut killed: MessageReader<Killed>,
    mut awards: MessageReader<Award>,
    pawns: Query<(Entity, &Pawn, Has<LocalPlayer>)>,
    player: Query<(Has<crate::combat::Dead>, Has<crate::loadout::AwaitingClass>, Has<crate::perks::Downed>), With<LocalPlayer>>,
) {
    let now = time.elapsed_secs();
    let Some(fe) = fe.as_deref_mut() else { return };
    let Some((me, me_pawn, _)) = pawns.iter().find(|p| p.2) else { return };
    let mut downed = false;
    if let Ok((_, waiting, last_stand)) = player.single() {
        downed = last_stand;
        if !waiting {
            progress.played += time.delta_secs();
        }
    }
    let (kill_xp, headshot_xp, assist_xp) = kill_values();
    let mut xp = 0;
    for k in killed.read() {
        let victim = pawns.get(k.victim).ok().map(|p| p.1.clone());
        if k.victim == me {
            fe.stats.add(stat::DEATHS, 1);
            progress.streak = 0;
        }
        if !victim.is_some_and(|v| hostile(me_pawn, &v)) {
            continue;
        }
        if k.attacker == Some(me) && k.victim != me {
            let head = progress.last_hit.get(&k.victim) == Some(&crate::combat::HitLocation::Head);
            // In Last Stand, kills pay double.
            xp += if head { headshot_xp } else { kill_xp } * if downed { 2 } else { 1 };
            fe.stats.add(stat::KILLS, 1);
            if head {
                fe.stats.add(stat::HEADSHOTS, 1);
            }
            progress.streak += 1;
            if progress.streak > fe.stats.get(stat::KILL_STREAK) {
                fe.stats.set(stat::KILL_STREAK, progress.streak);
            }
        }
    }
    // Assists the scoreboard has just counted for the player (`assists`).
    let n = me_pawn.assists as i32 - progress.counted_assists;
    if n > 0 {
        xp += assist_xp * n;
        fe.stats.add(stat::ASSISTS, n);
    }
    progress.counted_assists = me_pawn.assists as i32;
    for a in awards.read() {
        if a.pawn == me {
            xp += a.kind.xp();
        }
    }
    xp *= lives_scale();
    if xp > 0 {
        debug!("progression: +{xp} XP");
        give_xp(fe, &mut hud, xp, now);
    }
}

/// Splitscreen players 2 to 4 playing as a profile: what `award` and
/// `match_bonus` give player 1, in their own stats, and their rank kept up
/// with it (no notices: player 1's HUD has those).
#[derive(Default)]
struct SlotProgress {
    streak: [i32; 4],
    counted_assists: [i32; 4],
    bonus_given: bool,
}

#[allow(clippy::too_many_arguments)]
fn award_slots(
    mut fe: Option<ResMut<Frontend>>,
    progress: Res<Progress>,
    mut killed: MessageReader<Killed>,
    mut awards: MessageReader<Award>,
    pawns: Query<(Entity, &Pawn, &crate::splitscreen::LocalSlot)>,
    all: Query<&Pawn>,
    state: Option<Res<crate::tdm::MatchState>>,
    mut slots: Local<SlotProgress>,
) {
    let Some(fe) = fe.as_deref_mut() else { return };
    if !crate::splitscreen::active() {
        killed.clear();
        awards.clear();
        return;
    }
    let (kill_xp, headshot_xp, assist_xp) = kill_values();
    let kills: Vec<_> = killed.read().cloned().collect();
    let given: Vec<_> = awards.read().copied().collect();
    let ended = state.as_ref().and_then(|s| s.ended).is_some();
    if !ended {
        slots.bonus_given = false;
    }
    let give_bonus = ended && !slots.bonus_given;
    slots.bonus_given |= ended;
    for (me, pawn, slot) in &pawns {
        let s = slot.0;
        if s == 0 || s >= 4 {
            continue;
        }
        let Some((stats, assets)) = fe.slot_stats(s) else { continue };
        let mut xp = 0;
        for k in &kills {
            if k.victim == me {
                stats.add(stat::DEATHS, 1);
                slots.streak[s] = 0;
            }
            let hostile_victim = all.get(k.victim).is_ok_and(|v| hostile(pawn, v));
            if k.attacker == Some(me) && k.victim != me && hostile_victim {
                let head = progress.last_hit.get(&k.victim) == Some(&crate::combat::HitLocation::Head);
                xp += if head { headshot_xp } else { kill_xp };
                stats.add(stat::KILLS, 1);
                if head {
                    stats.add(stat::HEADSHOTS, 1);
                }
                slots.streak[s] += 1;
                if slots.streak[s] > stats.get(stat::KILL_STREAK) {
                    stats.set(stat::KILL_STREAK, slots.streak[s]);
                }
            }
        }
        let n = pawn.assists as i32 - slots.counted_assists[s];
        if n > 0 {
            xp += assist_xp * n;
            stats.add(stat::ASSISTS, n);
        }
        slots.counted_assists[s] = pawn.assists as i32;
        xp += given.iter().filter(|a| a.pawn == me).map(|a| a.kind.xp()).sum::<i32>();
        xp *= lives_scale();
        if give_bonus {
            let outcome = state.as_ref().and_then(|st| st.outcome(pawn));
            match outcome {
                Some(true) => stats.add(stat::WINS, 1),
                Some(false) => stats.add(stat::LOSSES, 1),
                None => {}
            }
            let sd = state.as_ref().is_some_and(|st| st.mode == crate::modes::GameMode::Sd);
            let scale = match outcome {
                Some(true) => 1.0,
                Some(false) => 0.5,
                None => 0.75,
            } * if sd { 2.0 } else { 1.0 };
            let spm = 3.0 + (stats.get(stat::RANK) % 61 + 1) as f32 * 0.5;
            xp += (scale * spm * progress.played / 60.0) as i32;
        }
        if xp > 0 {
            stats.add(stat::RANKXP, xp);
            stats.add(stat::SCORE, xp);
            catch_up_rank(stats, assets);
        }
        if give_bonus {
            info!("progression: player {}'s match bonus: their profile has {} XP", s + 1, stats.get(stat::RANKXP) + xp);
        }
        if xp > 0 || give_bonus {
            stats.save_if_changed();
        }
    }
}

/// A profile's rank for its XP (and its unlocks with it), quietly.
fn catch_up_rank(stats: &mut Stats, assets: &UiAssets) {
    let xp = stats.get(stat::RANKXP);
    let Some(table) = assets.table("mp/rankTable.csv") else { return };
    let num = |r: usize, c: usize| table.get(r, c).and_then(|v| v.trim().parse::<i32>().ok());
    let Some((rank, row)) = (0..table.rows).filter_map(|r| Some((num(r, 0)?, r)).filter(|&(_, r)| num(r, 2).is_some_and(|min| xp >= min))).max() else { return };
    if rank == stats.get(stat::RANK) {
        return;
    }
    stats.set(stat::RANK, rank);
    stats.set(stat::MINXP, num(row, 2).unwrap_or(0));
    stats.set(stat::MAXXP, num(row, 7).unwrap_or(0));
    stats.set(stat::LASTXP, xp);
    refresh_unlocks(stats, assets);
}

/// Add XP: to the stats, and the "+N" by the crosshair.
pub(super) fn give_xp(fe: &mut Frontend, hud: &mut HudState, xp: i32, now: f32) {
    fe.stats.add(stat::RANKXP, xp);
    fe.stats.add(stat::SCORE, xp);
    hud.add_xp(xp.max(0) as u32, now);
}

/// The match bonus when the match ends, and the win or loss.
fn match_bonus(
    time: Res<Time>,
    mut fe: Option<ResMut<Frontend>>,
    mut hud: ResMut<HudState>,
    mut progress: ResMut<Progress>,
    state: Option<Res<crate::tdm::MatchState>>,
    player: Query<&Pawn, With<LocalPlayer>>,
) {
    let (Some(fe), Some(state)) = (fe.as_deref_mut(), state) else { return };
    let Some((_, _)) = state.ended else {
        progress.bonus_given = false;
        return;
    };
    let Ok(me) = player.single() else { return };
    if progress.bonus_given {
        return;
    }
    progress.bonus_given = true;
    let outcome = state.outcome(me);
    let sd = state.mode == crate::modes::GameMode::Sd;
    let scale = match outcome {
        Some(true) => 1.0,
        Some(false) => 0.5,
        None => 0.75,
    } * if sd { 2.0 } else { 1.0 };
    match outcome {
        Some(true) => fe.stats.add(stat::WINS, 1),
        Some(false) => fe.stats.add(stat::LOSSES, 1),
        None => {}
    }
    // The script's match length cancels out (`length / 60 × played /
    // length`): rounds and overtime count as played.
    let spm = 3.0 + (fe.stats.get(stat::RANK) % 61 + 1) as f32 * 0.5;
    let bonus = (scale * spm * progress.played / 60.0) as i32;
    // Combat Record accounts for match time as it is played, including
    // matches left early. Do not count the whole match a second time here.
    info!("progression: match bonus {bonus} ({outcome:?}, {:.0}s played)", progress.played);
    if bonus > 0 {
        give_xp(fe, &mut hud, bonus, time.elapsed_secs());
        hud.message(format!("Match Bonus: +{bonus}"), time.elapsed_secs());
    }
    fe.stats.save_if_changed();
}

/// Climb the ranks of `mp/rankTable.csv`: "Promoted!" with the rank's name
/// and icon, and `mp_level_up`.
fn rank_up(time: Res<Time>, mut fe: Option<ResMut<Frontend>>, mut hud: ResMut<HudState>, mut sfx: ResMut<crate::audio::Sfx>) {
    let Some(fe) = fe.as_deref_mut() else { return };
    let xp = fe.stats.get(stat::RANKXP);
    let current = fe.stats.get(stat::RANK);
    let Some(table) = fe.assets.table("mp/rankTable.csv") else { return };
    let num = |r: usize, c: usize| table.get(r, c).and_then(|v| v.trim().parse::<i32>().ok());
    // Rows "0".."54": the highest whose minimum XP is reached.
    let mut rank = 0;
    let mut row_of = HashMap::new();
    for r in 0..table.rows {
        let (Some(id), Some(min)) = (num(r, 0), num(r, 2)) else { continue };
        row_of.insert(id, r);
        if xp >= min {
            rank = rank.max(id);
        }
    }
    if rank == current {
        return;
    }
    debug!("progression: {xp} XP is rank {rank} (was {current}), {} table rows", table.rows);
    let Some(&row) = row_of.get(&rank) else { return };
    let (min, max) = (num(row, 2).unwrap_or(0), num(row, 7).unwrap_or(0));
    let full = table.get(row, 5).unwrap_or("").to_owned();
    let icon = table.get(row, 6).unwrap_or("").to_owned();
    let level = num(row, 14).unwrap_or(rank + 1);
    fe.stats.set(stat::RANK, rank);
    fe.stats.set(stat::MINXP, min);
    fe.stats.set(stat::MAXXP, max);
    fe.stats.set(stat::LASTXP, xp);
    refresh_unlocks(&mut fe.stats, &fe.assets);
    // Only up: a profile simply catching up on its rank says nothing.
    if rank > current {
        let now = time.elapsed_secs();
        let name = fe.assets.localize(&format!("@{full}"));
        info!("progression: promoted to {name} (level {level}, {xp} XP)");
        hud.notify(fe.assets.localize("@RANK_PROMOTED"), name, icon, now);
        // What it brings, whether or not it was locked before.
        for line in (current + 1..=rank).flat_map(|r| unlock_lines(&fe.assets, r)) {
            info!("progression: {line}");
            hud.message(line, now);
        }
        sfx.play("mp_level_up", None);
        fe.stats.save_if_changed();
    }
}

/// The saved choice of unlocks: `all` (everything from the start) or `cod4`
/// (as CoD4 hands them out). `COD4RW_UNLOCKS` changes it (and is saved).
pub const UNLOCKS_DVAR: &str = "cod4rw_unlocks";

/// Are unlocks CoD4's (rather than everything)?
pub(super) fn cod4_unlocks(stats: &Stats) -> bool {
    stats.dvars.get(UNLOCKS_DVAR).is_some_and(|v| v == "cod4")
}

/// A table cell, trimmed ("" past its end).
fn cell(t: &iw3::menu::StringTable, r: usize, c: usize) -> &str {
    t.get(r, c).unwrap_or("").trim()
}

/// A level of a challenge (`mp/challengeTable_tierN.csv`): its progress
/// stat and target, XP, name and what it unlocks (`m16 reflex`, and the
/// line saying so).
#[derive(Clone, Debug)]
pub struct Level {
    pub progress: i32,
    pub target: i32,
    pub xp: i32,
    pub name: String,
    pub unlock: Option<(String, String)>,
    pub unlock_text: String,
}

/// A challenge, with all its levels (Marksman I, II, III): its name in
/// the tables (`ch_marksman_m16`), first level's (`ch_marksman_m16_1`, what
/// `mp/rankTable.csv` unlocks), tier (`tier_1_a`), and the stat keeping
/// its state (`_missions.gsc`: 0 locked, then the level being worked on,
/// 255 all done).
#[derive(Clone, Debug)]
pub struct Challenge {
    pub name: String,
    pub first: String,
    pub tier: String,
    pub state: i32,
    pub levels: Vec<Level>,
}

/// `_missions.gsc`'s state of a challenge with every level done.
pub(super) const CHALLENGE_DONE: i32 = 255;

/// Every challenge in the tier tables.
pub fn challenges(assets: &UiAssets) -> Vec<Challenge> {
    let mut out: Vec<Challenge> = Vec::new();
    for tier in 1..=10 {
        let Some(t) = assets.table(&format!("mp/challengeTable_tier{tier}.csv")) else { continue };
        for r in 0..t.rows {
            let (Ok(progress), Ok(target)) = (cell(t, r, 3).parse(), cell(t, r, 4).parse()) else { continue };
            let reference = cell(t, r, 7);
            // A challenge's first level has its state stat; the rest follow.
            if let Ok(state) = cell(t, r, 2).parse() {
                let levels = cell(t, r, 6).parse::<u32>().unwrap_or(1);
                let name = match reference.rsplit_once('_') {
                    Some((name, n)) if levels > 1 && n.parse::<u32>().is_ok() => name,
                    _ => reference,
                };
                out.push(Challenge {
                    name: name.to_owned(),
                    first: reference.to_owned(),
                    tier: format!("tier_{tier}_{}", cell(t, r, 14)),
                    state,
                    levels: Vec::new(),
                });
            }
            let Some(challenge) = out.last_mut() else { continue };
            let unlock = [cell(t, r, 13), cell(t, r, 12)]
                .into_iter()
                .find(|u| !u.is_empty())
                .and_then(|u| u.split_once(' '))
                .map(|(w, i)| (w.to_owned(), i.to_owned()));
            challenge.levels.push(Level {
                progress,
                target,
                xp: cell(t, r, 10).parse().unwrap_or(0),
                name: cell(t, r, 8).to_owned(),
                unlock,
                unlock_text: cell(t, r, 11).to_owned(),
            });
        }
    }
    out
}

/// Challenges become available (state 1) as `mp/rankTable.csv` column 10
/// says (`tier_1_a`, `ch_marksman_m21`), or all at once with everything
/// unlocked.
fn unlock_challenges(stats: &mut Stats, assets: &UiAssets, all: bool) {
    let rank = stats.get(stat::RANK);
    let mut available: Vec<String> = Vec::new();
    if let Some(t) = assets.table("mp/rankTable.csv") {
        for r in 0..t.rows {
            if cell(t, r, 0).parse::<i32>().is_ok_and(|at| at <= rank) {
                available.extend(cell(t, r, 10).split(';').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()));
            }
        }
    }
    for c in challenges(assets) {
        if stats.get(c.state) == 0 && (all || available.iter().any(|a| *a == c.tier || *a == c.first)) {
            stats.set(c.state, 1);
        }
    }
}

/// Bring the unlocks up to date with the rank and challenges: CoD4's, or
/// everything (then only the challenges need opening).
pub(super) fn refresh_unlocks(stats: &mut Stats, assets: &UiAssets) {
    let cod4 = cod4_unlocks(stats);
    if cod4 {
        apply_unlocks(stats, assets);
    }
    unlock_challenges(stats, assets, !cod4);
    super::mastery::refresh(stats, assets);
}

/// What each rank unlocks (`mp/rankTable.csv`): weapons (column 8), perks
/// (9), camos and attachments (11, 12: `m4 gl`) and features (15).
#[derive(Default)]
struct RankUnlocks {
    weapons: HashMap<String, i32>,
    perks: HashMap<String, i32>,
    items: Vec<(i32, String, String)>,
    features: HashMap<String, i32>,
}

fn rank_unlocks(assets: &UiAssets) -> RankUnlocks {
    let mut out = RankUnlocks::default();
    let Some(t) = assets.table("mp/rankTable.csv") else { return out };
    for r in 0..t.rows {
        let Ok(rank) = cell(t, r, 0).parse::<i32>() else { continue };
        for (col, map) in [(8, &mut out.weapons), (9, &mut out.perks), (15, &mut out.features)] {
            if !cell(t, r, col).is_empty() {
                map.insert(cell(t, r, col).to_owned(), rank);
            }
        }
        for col in [11, 12] {
            for (w, i) in cell(t, r, col).split(';').filter_map(|x| x.trim().split_once(' ')) {
                out.items.push((rank, w.to_owned(), i.to_owned()));
            }
        }
    }
    out
}

/// Unlocks as CoD4 hands them out, from the rank and the challenges done:
/// weapons, perks and features at the ranks `mp/rankTable.csv` gives them
/// (what it doesn't list is there from the start); with a weapon its grip,
/// launcher and desert and woodland camos (unless a rank brings them
/// later); its sights, silencer and other camos from its Marksman and
/// Expert challenges. Black Ops' and World at War's guns, which CoD4's
/// tables don't know, at their levels ([`other_levels`]), with everything
/// for them; one a custom class already uses stays unlocked.
pub(super) fn apply_unlocks(stats: &mut Stats, assets: &UiAssets) {
    let rank = stats.get(stat::RANK);
    let ranks = rank_unlocks(assets);
    let bits: HashMap<String, i32> = assets.table("mp/attachmentTable.csv").map_or_else(HashMap::new, |t| {
        (0..t.rows).filter_map(|r| Some((cell(t, r, 4).to_ascii_lowercase(), cell(t, r, 10).parse().ok()?))).collect()
    });
    let bit = |item: &str| bits.get(item).copied().unwrap_or(0);
    let earned: Vec<(String, String)> = challenges(assets)
        .into_iter()
        .flat_map(|c| c.levels)
        .filter(|l| stats.get(l.progress) >= l.target)
        .filter_map(|l| l.unlock)
        .collect();
    let reached = |at: Option<&i32>| at.is_none_or(|&r| r <= rank);
    let others = other_levels(assets);
    let Some(t) = assets.table("mp/statsTable.csv") else { return };
    for r in 0..t.rows {
        let (Ok(index), kind, name) = (cell(t, r, 1).parse::<i32>(), cell(t, r, 2), cell(t, r, 4)) else { continue };
        if name.is_empty() {
            continue;
        }
        let value = if kind.starts_with("weapon_") {
            if index >= 3000 + crate::bo1::FIRST_INDEX {
                // Black Ops' and World at War's guns by level (`other_levels`);
                // others' (MW2's) stay as they are.
                let Some(&level) = others.get(name) else { continue };
                let gun = index - 3000;
                let in_class = (0..5).any(|c| [201, 203].iter().any(|k| stats.get(k + 10 * c) == gun));
                let old = stats.get(index);
                let v = if rank + 1 >= level || in_class { OTHER_GAME_UNLOCKED } else { 0 };
                // Newly unlocked: the menus' "new" mark.
                let new = if old & 1 == 0 && v & 1 != 0 && stats.get(stat::RANKXP) > 0 { 65536 } else { old & 65536 };
                stats.set(index, v | new);
                continue;
            }
            let mut v = 0;
            if reached(ranks.weapons.get(name)) {
                v |= 1;
                for item in ["grip", "gl", "camo_brockhaurd", "camo_bushdweller"] {
                    let at = ranks.items.iter().find(|(_, w, i)| w == name && i == item).map(|(r, ..)| r);
                    if reached(at) {
                        v |= bit(item);
                    }
                }
            }
            for (_, item) in earned.iter().filter(|(w, _)| w == name) {
                v |= bit(item);
            }
            // The menus' "new" mark stays as it was.
            v | (stats.get(index) & 65536)
        } else if kind == "feature" {
            i32::from(reached(ranks.features.get(name)))
        } else if matches!(kind, "specialty" | "grenade" | "specialgrenade" | "inventory") {
            i32::from(reached(ranks.perks.get(name)))
        } else {
            continue;
        };
        stats.set(index, value);
    }
}

/// A Black Ops or World at War gun, unlocked: every attachment and camo.
const OTHER_GAME_UNLOCKED: i32 = 1 | 2 | 4 | 8 | 16 | 32 | 256 | 512 | 1024 | 2048 | 4096 | (((1 << 28) - (1 << 6)) & !65536);

/// The level each Black Ops and World at War gun unlocks at (CoD4's tables
/// don't know them). Each game's guns by class, in their own order: the
/// first of a class from the start, the rest spread over levels 4 to 50
/// (Black Ops' cap), each class a step apart so a level brings one or two.
pub(super) fn other_levels(assets: &UiAssets) -> HashMap<String, i32> {
    let mut out = HashMap::new();
    let Some(t) = assets.table("mp/statsTable.csv") else { return out };
    let mut groups: HashMap<(bool, String), Vec<(i32, String)>> = HashMap::new();
    for r in 0..t.rows {
        let (Ok(index), kind, name) = (cell(t, r, 0).parse::<i32>(), cell(t, r, 2), cell(t, r, 4)) else { continue };
        let bo1 = crate::bo1::is_index(index);
        if !kind.starts_with("weapon_") || name.is_empty() || !(bo1 || crate::waw::is_index(index)) {
            continue;
        }
        groups.entry((bo1, kind.to_owned())).or_default().push((index, name.to_owned()));
    }
    const ORDER: [&str; 6] = ["weapon_assault", "weapon_smg", "weapon_lmg", "weapon_sniper", "weapon_shotgun", "weapon_pistol"];
    for ((bo1, kind), mut guns) in groups {
        guns.sort();
        let step = ORDER.iter().position(|k| *k == kind).unwrap_or(6) as f32 + if bo1 { 0.0 } else { 0.5 };
        let n = guns.len();
        for (k, (_, name)) in guns.into_iter().enumerate() {
            let level = if k == 0 {
                1
            } else {
                let f = if n > 2 { (k - 1) as f32 / (n - 2) as f32 } else { 0.0 };
                (4.0 + step + f * (46.0 - step)).round() as i32
            };
            out.insert(name, level);
        }
    }
    out
}

/// What reaching `rank` unlocks, for the player: "New Weapon: M4 Carbine".
fn unlock_lines(assets: &UiAssets, rank: i32) -> Vec<String> {
    let ranks = rank_unlocks(assets);
    let name_of = |item: &str| -> String {
        let Some(t) = assets.table("mp/statsTable.csv") else { return item.to_owned() };
        (0..t.rows)
            .find(|&r| cell(t, r, 4) == item)
            .map(|r| cell(t, r, 3))
            .filter(|k| !k.is_empty())
            .map_or_else(|| item.to_owned(), |k| assets.localize(&format!("@{k}")))
    };
    let mut lines = Vec::new();
    let others: HashMap<String, i32> = other_levels(assets).into_iter().filter(|(_, l)| *l > 1).map(|(n, l)| (n, l - 1)).collect();
    for (what, map) in [("Weapon", &ranks.weapons), ("Weapon", &others), ("Perk", &ranks.perks), ("Feature", &ranks.features)] {
        let mut names: Vec<_> = map.iter().filter(|(_, r)| **r == rank).map(|(n, _)| name_of(n)).collect();
        names.sort();
        lines.extend(names.into_iter().map(|n| format!("New {what}: {n}")));
    }
    lines
}

#[derive(Resource)]
struct TestDir(std::path::PathBuf);

/// Debug aid: with `COD4RW_CHALLENGETEST=<dir>`, the gun in hand's Marksman
/// and Expert challenges are a kill from done; 5 s in the player kills an
/// enemy with it, a headshot. Screenshots of both notices and the
/// promotion after them, the unlocks logged, then exit.
#[allow(clippy::too_many_arguments)]
fn test(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<TestDir>,
    mut fe: Option<ResMut<Frontend>>,
    player: Query<(Entity, &Pawn, &crate::weapons::WeaponState), With<LocalPlayer>>,
    pawns: Query<(Entity, &Pawn), Without<LocalPlayer>>,
    mut damage: MessageWriter<Damage>,
    mut step: Local<usize>,
    mut killed_at: Local<f32>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let t = time.elapsed_secs();
    let (Some(fe), Ok((me, pawn, w))) = (fe.as_deref_mut(), player.single()) else { return };
    let gun = w.def.name.split('_').next().unwrap_or_default().to_owned();
    let unlock_stat = fe.assets.table("mp/statsTable.csv").and_then(|t| {
        (0..t.rows).find(|&r| cell(t, r, 4) == gun).and_then(|r| cell(t, r, 1).parse::<i32>().ok())
    });
    let mut shot = |name: &str, step: &mut usize| {
        std::fs::create_dir_all(&dir.0).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.0.join(format!("challenge_{step}_{name}.png"))));
        *step += 1;
    };
    match *step {
        0 if t >= 4.0 => {
            for c in challenges(&fe.assets).iter().filter(|c| c.name == format!("ch_marksman_{gun}") || c.name == format!("ch_expert_{gun}")) {
                fe.stats.set(c.state, 1);
                fe.stats.set(c.levels[0].progress, c.levels[0].target - 1);
            }
            info!("challenge test: {gun}, unlocks {:?}", unlock_stat.map(|s| fe.stats.get(s)));
            *step = 1;
        }
        1 if t >= 5.0 => {
            let Some(enemy) = pawns.iter().find(|(_, p)| hostile(p, pawn)).map(|(e, _)| e) else { return };
            damage.write(Damage { target: enemy, attacker: Some(me), amount: 500.0, location: crate::combat::HitLocation::Head, weapon: w.def.display_name });
            *killed_at = t;
            *step = 2;
        }
        2 if t >= *killed_at + 1.0 => shot("first", &mut step),
        3 if t >= *killed_at + 5.0 => {
            info!("challenge test: unlocks now {:?}", unlock_stat.map(|s| fe.stats.get(s)));
            shot("second", &mut step);
        }
        4 if t >= *killed_at + 9.0 => shot("promoted", &mut step),
        5 if t >= *killed_at + 10.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
