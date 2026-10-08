//! CoD4's challenges (`_missions.gsc`), counted as the player plays: each
//! gun's Marksman (kills) and Expert (headshots); kills crouched, prone,
//! through walls, with grenades (cooked, Martyrdom's, held too long),
//! C4, claymores, the RPG, the knife (in the back too), kill streaks'
//! airstrikes and helicopters; kills while downed, stunned or flashed, of
//! the stunned, the flashed and the airborne; multi-kills from one blast or
//! one sniper bullet; streaks in one life (Fearless, Slasher, Airborne, The
//! Brink), killing every enemy (Tango Down, Extreme Cruelty), Rival,
//! Counter-MVP, Fast Swap; calling in hardpoints, shooting a helicopter
//! down; assists, sprinting (Marathon), long falls (Base Jump, Goodbye),
//! regenerating (Invincible), staying alive (Survivalist); Search and
//! Destroy's (and Sabotage's) bomb carrier, planter and defuser kills,
//! Hero, Last Man Standing; and at a match's end the wins, placings, MVPs, The Edge,
//! Flawless and Star Player. What needs things the game doesn't have (cars,
//! shooting explosives, picking up guns and grenades, mounted guns,
//! grenade impacts) isn't counted.
//!
//! [`Counts`] gathers the progress; [`complete`] adds it to the stats,
//! and completing a level gives its XP and unlock with "Challenge
//! Completed!" (`processChallenge`).

use super::Frontend;
use super::hud::HudState;
use super::progression::{CHALLENGE_DONE, Challenge, challenges, give_xp, refresh_unlocks};
use crate::combat::{Damage, Dead, Health, HitLocation, Killed, Pawn, hostile};
use crate::grenades::{Flashed, How, Kind, Stunned, WentOff};
use crate::killstreaks::{Hardpoint, StreakNotice, helicopter::ShotDown};
use crate::modes::{GameMode, Objectives};
use crate::movement::{Landed, Mover, Stance, ViewAngles};
use crate::perks::Downed;
use crate::player::LocalPlayer;
use crate::weapons::{Penetrated, WeaponState};
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

pub(super) fn setup(app: &mut App) {
    app.init_resource::<Counts>()
        .init_resource::<Tracking>()
        .add_systems(OnEnter(crate::state::GameState::InGame), |mut t: ResMut<Tracking>, mut c: ResMut<Counts>| {
            *t = Tracking::default();
            c.0.clear();
        })
        .add_systems(
            Update,
            (kills, life, match_end, complete).chain().after(crate::weapons::WeaponSet).run_if(crate::state::in_game),
        );
}

/// Progress towards challenges by name (`ch_knifevet`, whose levels share
/// it; `ch_marksman_m16`), added up by [`complete`]. Other code can count
/// its own (a mode's wins).
#[derive(Resource, Default)]
pub struct Counts(Vec<(String, i32)>);

impl Counts {
    pub fn add(&mut self, name: &str, n: i32) {
        self.0.push((name.to_owned(), n));
    }
}

/// What the challenges need remembered.
#[derive(Resource, Default)]
struct Tracking {
    /// The last hit on each pawn: the weapon and where.
    last_hit: HashMap<Entity, (&'static str, HitLocation)>,
    /// Enemies the player has hurt with a primary weapon (Fast Swap).
    primary_hurt: HashSet<Entity>,
    /// Enemies the player's bullets reached through a wall, and when.
    through_wall: HashMap<Entity, f32>,
    /// The player's frag grenades going off lately: when, and how.
    frags: Vec<(f32, How)>,
    /// The player's long falls: when they landed, from how high.
    landing: Option<(f32, f32)>,
    life: Life,
    /// This match: everyone killed, kills of each, kills of the enemy's
    /// best, hardpoints called, deaths, who made the last kill.
    killed: HashSet<Entity>,
    kills_of: HashMap<Entity, u32>,
    mvp_kills: u32,
    calls: HashMap<Hardpoint, u32>,
    deaths: u32,
    last_killer: Option<Entity>,
    tango_down: bool,
    /// The player's airstrike kills: since when, how many.
    airstrike: Option<(f32, u32)>,
    /// Sprinting not yet counted (inches).
    sprint: f32,
    assists: u32,
    /// Search and Destroy as of last frame: the bomb's carrier, who was
    /// planting or defusing, whether a round was over.
    carrier: Option<Entity>,
    using: Vec<(Entity, bool)>,
    round_over: bool,
    ended: bool,
}

/// What the challenges need from the player's current life.
#[derive(Default)]
struct Life {
    since: f32,
    kills: u32,
    melee: u32,
    /// Bullet kills since leaving the ground.
    air: u32,
    /// Kills while near death, since last at full health.
    brink: u32,
    killed: HashSet<Entity>,
    /// Health regenerations, and whether hurt since the last.
    regens: u32,
    hurt: bool,
    survived: bool,
    cruel: bool,
}

/// Name → `mp/statsTable.csv` kind (`weapon_sniper`) for CoD4's guns.
fn gun_kinds(fe: &Frontend) -> HashMap<String, String> {
    let Some(t) = fe.assets.table("mp/statsTable.csv") else { return HashMap::new() };
    (0..t.rows)
        .filter_map(|r| {
            let (kind, name) = (t.get(r, 2)?.trim(), t.get(r, 4)?.trim());
            (kind.starts_with("weapon_") && !name.is_empty()).then(|| (name.to_owned(), kind.to_owned()))
        })
        .collect()
}

/// The player's kills (and death), with what was going on at the time.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn kills(
    time: Res<Time>,
    fe: Option<Res<Frontend>>,
    objectives: Res<Objectives>,
    mut t: ResMut<Tracking>,
    mut counts: ResMut<Counts>,
    mut damage: MessageReader<Damage>,
    mut killed: MessageReader<Killed>,
    mut penetrated: MessageReader<Penetrated>,
    mut went_off: MessageReader<WentOff>,
    mut landed: MessageReader<Landed>,
    me: Query<(Entity, &Pawn, &Mover, &Health, &WeaponState, Has<Downed>, Has<Flashed>, Has<Stunned>, Has<Dead>), With<LocalPlayer>>,
    pawns: Query<(Entity, &Pawn, &Mover, &Transform, &ViewAngles, Has<Flashed>, Has<Stunned>, Has<Dead>)>,
    mut kinds: Local<Option<HashMap<String, String>>>,
) {
    let now = time.elapsed_secs();
    let Ok((me, my_pawn, mover, health, held, downed, flashed, stunned, dead)) = me.single() else { return };
    if kinds.is_none() {
        *kinds = fe.as_deref().map(gun_kinds);
    }
    let gun = held.def.name.split('_').next().unwrap_or_default().to_ascii_lowercase();
    let kind = kinds.as_ref().and_then(|k| k.get(&gun)).map_or("", String::as_str);
    let pistol = held.def.class == 4;
    let sniper = kind == "weapon_sniper" || (kind.is_empty() && held.def.ads_overlay.is_some());
    let t = &mut *t;
    for d in damage.read() {
        t.last_hit.insert(d.target, (d.weapon, d.location));
        if d.attacker == Some(me) && d.target != me && d.weapon == held.def.display_name && !pistol {
            t.primary_hurt.insert(d.target);
        }
    }
    for p in penetrated.read().filter(|p| p.shooter == me) {
        t.through_wall.insert(p.target, now);
    }
    for g in went_off.read().filter(|g| g.thrower == me && g.kind == Kind::Frag) {
        t.frags.push((now, g.how));
    }
    t.frags.retain(|f| now - f.0 < 1.0);
    for l in landed.read().filter(|l| l.entity == me && l.fall_height >= 180.0) {
        t.landing = Some((now, l.fall_height));
    }
    let enemies: Vec<Entity> = pawns.iter().filter(|p| hostile(my_pawn, p.1)).map(|p| p.0).collect();
    // The enemy's best (Counter-MVP).
    let best = pawns.iter().filter(|p| hostile(my_pawn, p.1)).map(|p| p.1.kills).max().unwrap_or(0);
    let mut mine: Vec<&'static str> = Vec::new();
    for k in killed.read() {
        if let Some(a) = k.attacker.filter(|&a| a != k.victim) {
            t.last_killer = Some(a);
        }
        let (weapon, location) = t.last_hit.get(&k.victim).copied().unwrap_or(("", HitLocation::Torso));
        if k.victim == me {
            t.deaths += 1;
            // Goodbye: a fall of 30 feet or more to the death.
            if weapon == "falling" && t.landing.is_some_and(|(at, h)| now - at < 0.5 && h >= 360.0) {
                counts.add("ch_goodbye", 1);
            }
            t.landing = None;
            continue;
        }
        if k.attacker != Some(me) {
            continue;
        }
        let Ok((victim, v_pawn, v_mover, v_tf, v_view, v_flashed, v_stunned, _)) = pawns.get(k.victim) else { continue };
        if !hostile(my_pawn, v_pawn) {
            continue;
        }
        mine.push(weapon);
        let head = location == HitLocation::Head;
        let mut count = |name: &str| counts.add(name, 1);
        // With the gun in hand.
        if !weapon.is_empty() && weapon == held.def.display_name {
            count(&format!("ch_marksman_{gun}"));
            if head {
                count(&format!("ch_expert_{gun}"));
            }
            if held.def.name.contains("silencer") {
                count("ch_stealth");
            }
            if sniper && mover.stance == Stance::Prone {
                count("ch_invisible");
            }
            if t.through_wall.get(&victim).is_some_and(|&at| now - at < 0.5) {
                count("ch_xrayvision");
            }
            if pistol && t.primary_hurt.contains(&victim) {
                count("ch_fastswap");
            }
            if !mover.on_ground {
                t.life.air += 1;
                if t.life.air == 2 {
                    count("ch_airborne");
                }
            }
        }
        match mover.stance {
            Stance::Crouch => count("ch_crouchshot"),
            Stance::Prone => count("ch_proneshot"),
            Stance::Stand => {}
        }
        match weapon {
            "Frag Grenade" => match t.frags.last().map(|f| f.1) {
                Some(How::Martyrdom) => count("ch_martyrdomvet"),
                Some(How::InHand) if dead => count("ch_miserylovescompany"),
                Some(How::Cooked) => {
                    count("ch_grenadekill");
                    count("ch_masterchef");
                }
                _ => count("ch_grenadekill"),
            },
            "Claymore" => count("ch_claymoreshot"),
            w if w == crate::melee::WEAPON => {
                count("ch_knifevet");
                t.life.melee += 1;
                if t.life.melee == 3 {
                    count("ch_slasher");
                }
                // From behind: the victim faced away from the player.
                let to_me = (my_pos(&pawns, me) - v_tf.translation).with_y(0.0).normalize_or_zero();
                let facing = (v_view.rotation() * Vec3::NEG_Z).with_y(0.0).normalize_or_zero();
                if facing.dot(to_me) < -0.3 {
                    count("ch_backstabber");
                }
            }
            crate::killstreaks::airstrike::WEAPON => {
                count("ch_airstrikevet");
                let (since, n) = t.airstrike.filter(|(since, _)| now - since < 10.0).unwrap_or((now, 0));
                t.airstrike = Some((since, n + 1));
                if n + 1 == 5 {
                    count("ch_carpetbomb");
                }
            }
            w if crate::killstreaks::kill_icon(w).is_some() => count("ch_choppervet"),
            _ => {}
        }
        if downed {
            count("ch_laststandvet");
        }
        if stunned {
            count("ch_slowbutsure");
        }
        if flashed {
            count("ch_blindfire");
        }
        if v_stunned {
            count("ch_concussionvet");
        }
        if v_flashed {
            count("ch_flashbangvet");
        }
        if !v_mover.on_ground {
            count("ch_hardlanding");
        }
        // Near death: the low health overlay's critical pulse.
        if health.current < crate::combat::max_health() * 0.33 {
            t.life.brink += 1;
            if t.life.brink == 3 {
                count("ch_thebrink");
            }
        }
        t.life.kills += 1;
        if t.life.kills == 10 {
            count("ch_fearless");
        }
        let n = t.kills_of.entry(victim).or_default();
        *n += 1;
        if *n == 5 {
            count("ch_rival");
        }
        if best > 0 && v_pawn.kills >= best {
            t.mvp_kills += 1;
            if t.mvp_kills == 10 {
                count("ch_countermvp");
            }
        }
        // Search and Destroy, as things stood just before.
        if t.carrier == Some(victim) {
            count("ch_bombdown");
        }
        if let Some(&(_, defusing)) = t.using.iter().find(|u| u.0 == victim) {
            count(if defusing { "ch_bombdefender" } else { "ch_bombplanter" });
        }
        t.life.killed.insert(victim);
        t.killed.insert(victim);
    }
    // Several at once, from one blast or one sniper bullet.
    for (weapon, name) in [("RPG-7", "ch_multirpg"), ("Claymore", "ch_multiclaymore"), ("C4", "ch_multic4"), ("Frag Grenade", "ch_multifrag")] {
        if mine.iter().filter(|w| **w == weapon).count() >= 2 {
            counts.add(name, 1);
        }
    }
    if sniper && mine.iter().filter(|w| **w == held.def.display_name).count() >= 2 {
        counts.add("ch_collateraldamage", 1);
    }
    // Every enemy (at least four): this match, and this life.
    let all = |set: &HashSet<Entity>| enemies.len() >= 4 && enemies.iter().all(|e| set.contains(e));
    if !mine.is_empty() {
        if !t.tango_down && all(&t.killed) {
            t.tango_down = true;
            counts.add("ch_tangodown", 1);
        }
        if !t.life.cruel && all(&t.life.killed) {
            t.life.cruel = true;
            counts.add("ch_extremecruelty", 1);
        }
    }
    // Search and Destroy's state, for next frame's kills.
    t.carrier = objectives.bomb.as_ref().and_then(|b| b.carrier);
    t.using = objectives.using.iter().map(|u| (u.0, u.2)).collect();
}

fn my_pos(pawns: &Query<(Entity, &Pawn, &Mover, &Transform, &ViewAngles, Has<Flashed>, Has<Stunned>, Has<Dead>)>, me: Entity) -> Vec3 {
    pawns.get(me).map_or(Vec3::ZERO, |p| p.3.translation)
}

/// The player's life and doings outside kills: assists, health, sprinting,
/// falls, hardpoints, helicopters shot down.
#[allow(clippy::too_many_arguments)]
fn life(
    time: Res<Time>,
    mut t: ResMut<Tracking>,
    mut counts: ResMut<Counts>,
    mut notices: MessageReader<StreakNotice>,
    mut shot_down: MessageReader<ShotDown>,
    me: Query<(Entity, &Pawn, &Mover, &Health, Has<Dead>), With<LocalPlayer>>,
) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    let Ok((me, pawn, mover, health, dead)) = me.single() else { return };
    let t = &mut *t;
    if pawn.assists > t.assists {
        counts.add("ch_assists", (pawn.assists - t.assists) as i32);
    }
    t.assists = pawn.assists;
    for n in notices.read() {
        let StreakNotice::CalledIn { item, by, .. } = n else { continue };
        if *by != pawn.name {
            continue;
        }
        let calls = t.calls.entry(*item).or_default();
        *calls += 1;
        let (streak, per_match, times) = match item {
            Hardpoint::Uav => ("ch_uav", "ch_nosecrets", 3),
            Hardpoint::Airstrike => ("ch_airstrike", "ch_afterburner", 2),
            Hardpoint::Helicopter => ("ch_chopper", "ch_airsuperiority", 2),
        };
        counts.add(streak, 1);
        if *calls == times {
            counts.add(per_match, 1);
        }
        if *item == Hardpoint::Uav {
            counts.add("ch_exposed", 1);
        }
    }
    for s in shot_down.read() {
        if s.by == me {
            counts.add("ch_flyswatter", 1);
        }
    }
    if dead {
        t.life = Life { since: now, ..default() };
        return;
    }
    // Back to full health after being hurt: a regeneration.
    let max = crate::combat::max_health();
    if health.current < max {
        t.life.hurt = true;
    } else {
        if std::mem::take(&mut t.life.hurt) {
            t.life.regens += 1;
            if t.life.regens == 5 {
                counts.add("ch_invincible", 1);
            }
        }
        t.life.brink = 0;
    }
    if !t.life.survived && now - t.life.since >= 300.0 {
        t.life.survived = true;
        counts.add("ch_survivalist", 1);
    }
    if mover.on_ground {
        t.life.air = 0;
    }
    // Marathon, in inches.
    if mover.sprinting && mover.on_ground {
        t.sprint += mover.velocity.with_y(0.0).length() * dt / crate::units::INCH;
        if t.sprint >= 1.0 {
            counts.add("ch_marathon", t.sprint as i32);
            t.sprint = t.sprint.fract();
        }
    }
    // Base Jump: a fall of 15 feet or more, lived through.
    if t.landing.is_some_and(|(at, _)| now - at > 0.5) {
        t.landing = None;
        counts.add("ch_basejump", 1);
    }
}

/// Rounds and matches ending: Search and Destroy's Hero and Last Man
/// Standing; the wins, placings and MVPs, The Edge, Flawless, Star Player.
fn match_end(
    mut t: ResMut<Tracking>,
    mut counts: ResMut<Counts>,
    objectives: Res<Objectives>,
    state: Option<Res<crate::tdm::MatchState>>,
    progress: Res<super::progression::Progress>,
    me: Query<(Entity, &Pawn, Has<Dead>), With<LocalPlayer>>,
    pawns: Query<(Entity, &Pawn, Has<Dead>)>,
) {
    let Ok((me, my_pawn, dead)) = me.single() else { return };
    let t = &mut *t;
    let round_over = objectives.round_over.is_some();
    if let (Some((winner, key, _)), false) = (objectives.round_over, t.round_over) {
        if winner == my_pawn.team {
            if key == "MP_BOMB_DEFUSED" && t.using.iter().any(|u| u.0 == me && u.1) {
                counts.add("ch_hero", 1);
            }
            let team: Vec<bool> = pawns.iter().filter(|p| p.0 != me && p.1.team == my_pawn.team).map(|p| p.2).collect();
            if !dead && !team.is_empty() && team.iter().all(|d| *d) {
                counts.add("ch_lastmanstanding", 1);
            }
        }
    }
    t.round_over = round_over;
    let Some(state) = state else { return };
    let Some((_, ended_at)) = state.ended else {
        t.ended = false;
        return;
    };
    if std::mem::replace(&mut t.ended, true) {
        return;
    }
    let score = |p: &Pawn| p.kills * 10 + p.assists * 2;
    let top = pawns.iter().all(|p| score(p.1) <= score(my_pawn));
    let win = state.outcome(my_pawn) == Some(true);
    let hardcore = crate::tdm::hardcore();
    match state.mode {
        GameMode::Ffa => {
            if pawns.iter().filter(|p| p.1.kills > my_pawn.kills).count() < 3 {
                counts.add("ch_victor_dm", 1);
            }
        }
        GameMode::Tdm | GameMode::Tdm3 => {
            if win {
                counts.add(if hardcore { "ch_teamplayer_hc" } else { "ch_teamplayer" }, 1);
            }
            if top && !hardcore {
                counts.add("ch_mvp_tdm", 1);
            }
        }
        GameMode::Sd if win => counts.add("ch_victor_sd", 1),
        GameMode::Sab if win => counts.add("ch_victor_sab", 1),
        _ => {}
    }
    if hardcore && state.mode.teams() && win && top {
        counts.add("ch_mvp_thc", 1);
    }
    if t.last_killer == Some(me) {
        counts.add("ch_theedge", 1);
    }
    // The whole match played: Flawless (to the time limit, without dying),
    // Star Player (five kills to each death).
    let length = ended_at - state.started;
    let whole = progress.played >= length * 0.9;
    if whole && t.deaths == 0 && length >= state.time_limit - 1.0 {
        counts.add("ch_flawless", 1);
    }
    if whole && my_pawn.kills >= 5 && my_pawn.kills >= 5 * t.deaths {
        counts.add("ch_starplayer", 1);
    }
}

/// Add the counts up: each challenge's level being worked on gains them,
/// and reaching its target completes it, with its XP and unlock and
/// "Challenge Completed!".
fn complete(
    time: Res<Time>,
    mut fe: Option<ResMut<Frontend>>,
    mut hud: ResMut<HudState>,
    mut counts: ResMut<Counts>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut table: Local<Option<Vec<Challenge>>>,
) {
    let Some(fe) = fe.as_deref_mut() else {
        counts.0.clear();
        return;
    };
    if counts.0.is_empty() {
        return;
    }
    let table = table.get_or_insert_with(|| challenges(&fe.assets));
    let now = time.elapsed_secs();
    let mut done = false;
    for (name, n) in std::mem::take(&mut counts.0) {
        let Some(c) = table.iter().find(|c| c.name == name) else { continue };
        let state = fe.stats.get(c.state);
        // Locked (0) or all done.
        let Some(level) = usize::try_from(state - 1).ok().and_then(|i| c.levels.get(i)) else { continue };
        debug!("challenges: {name} +{n}");
        fe.stats.add(level.progress, n);
        if fe.stats.get(level.progress) < level.target {
            continue;
        }
        fe.stats.set(c.state, if state as usize >= c.levels.len() { CHALLENGE_DONE } else { state + 1 });
        let text = fe.assets.localize(&format!("@{}", level.name));
        info!("progression: challenge completed: {text} (+{} XP)", level.xp);
        hud.notify(fe.assets.localize("@CHALLENGE_COMPLETED"), text, String::new(), now);
        if !level.unlock_text.is_empty() {
            hud.message(fe.assets.localize(&format!("@{}", level.unlock_text)), now);
        }
        sfx.play("mp_challenge_complete", None);
        give_xp(fe, &mut hud, level.xp, now);
        done = true;
    }
    if done {
        refresh_unlocks(&mut fe.stats, &fe.assets);
        fe.stats.save_if_changed();
    }
}
