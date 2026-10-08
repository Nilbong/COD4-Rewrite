//! Sabotage (`sab.gsc`): one bomb in the middle of the map (`sab_bomb`)
//! and a target for each team (`sab_bomb_allies`, `sab_bomb_axis`). Either
//! team picks the bomb up and plants it at the other's target (hold use for
//! 5 seconds); it goes off 45 seconds later unless the target's team
//! defuses it (5 seconds), when the defuser takes it, and its timer keeps
//! only what was left (`hotpotato`). The match clock stops while it's
//! planted; dropped, it goes home after a minute. Destroying the enemy's
//! target wins the round (the default limit is one). When the time runs
//! out with no bomb planted, overtime: everyone back in, then sudden death
//! with no respawns, for 90 seconds (not counting while the bomb's down),
//! then a draw.

use super::{Bomb, BombSite, GameMode, Objectives, UseObjective, brush_box, current, sd::in_pickup_reach};
use crate::audio::{Sfx, Sides};
use crate::combat::{Damage, Dead, HitLocation, Pawn, Team};
use crate::content::Content;
use crate::fx::{Anchor, Effects, Frame, FxLayer};
use crate::models::{Skeleton, SpawnModel, spawn_model};
use crate::movement::{Frozen, MoveInput};
use crate::player::LocalPlayer;
use crate::state::{GameState, Setup, in_game};
use crate::tdm::MatchState;
use crate::units::u;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

pub struct SabPlugin;

impl Plugin for SabPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::InGame), setup.in_set(Setup::Spawn))
            .add_systems(Update, hold_still.after(crate::player::InputSet).before(crate::movement::MovementSet).run_if(in_game.and_then(playing)))
            .add_systems(Update, (sudden_death, bomb, rounds, show_models).chain().after(crate::movement::MovementSet).run_if(in_game.and_then(playing)));
    }
}

fn playing() -> bool {
    current() == GameMode::Sab
}

/// `scr_sab_planttime`, `_defusetime`, `_bombtimer`.
const PLANT_TIME: f32 = 5.0;
const DEFUSE_TIME: f32 = 5.0;
const BOMB_TIMER: f32 = 45.0;
/// `autoResetTime`: a dropped bomb goes home after this long.
const BOMB_RESET: f32 = 60.0;
/// `onOvertime`: sudden death lasts this long, the bomb's time down aside.
const OVERTIME: f32 = 90.0;
const DEFUSE_RANGE: f32 = 64.0;
/// From a round's end to the next.
const ROUND_DELAY: f32 = 7.0;
const BLAST_RADIUS: f32 = 512.0;
const BLAST_MAX: f32 = 200.0;
const BLAST_MIN: f32 = 20.0;

#[derive(Resource)]
struct SabRound {
    /// The round's length (the match's time limit).
    length: f32,
    started: f32,
    over: Option<f32>,
    overtime: bool,
    bomb_home: Vec3,
    match_started: f32,
    planter: Option<Entity>,
    tick: f32,
    carrier_at: Vec3,
    /// The bomb's timer this round (`hotpotato`: what a defuse leaves).
    bomb_timer: f32,
    /// When it was planted (the clock stops meanwhile), and when it was
    /// last dropped.
    planted_at: Option<f32>,
    dropped_at: Option<f32>,
    /// Sudden death's time left.
    overtime_left: f32,
    /// In sudden death the planting team is all dead with the bomb down
    /// (`plantingTeamDead`): it's up to the bomb.
    planters_dead: bool,
    /// The round's length before the clock stopped for the bomb.
    full_length: f32,
    /// Debug aid (`COD4RW_SDBOMB=1`): the player starts with the bomb.
    hand_bomb: bool,
}

#[derive(Component)]
struct SiteModel(usize);

#[derive(Component)]
struct BombModel;

/// Each team's target (the `sab_bomb_<team>` use trigger and the model it
/// targets) and the bomb's spot.
fn read_map(content: &Content) -> (Vec<BombSite>, Option<Vec3>) {
    let map = content.map();
    let ents = map.map_ents().map(|m| iw3::ents::parse(&m.entity_string)).unwrap_or_default();
    let clip = map.clip_map();
    let sites = [(Team::Allies, "sab_bomb_allies", 'A'), (Team::Axis, "sab_bomb_axis", 'B')]
        .into_iter()
        .filter_map(|(team, name, label)| {
            let e = ents.iter().find(|e| e.get("targetname") == Some(name))?;
            let (min, max) = brush_box(e, clip, 96.0)?;
            let target = e.get("target").and_then(|t| ents.iter().find(|x| x.get("targetname") == Some(t))).and_then(|x| x.origin());
            let pos = target.or(e.origin()).map(crate::units::pos)?;
            Some(BombSite { label, min, max, pos, destroyed: false, team: Some(team) })
        })
        .collect();
    let bomb = ents.iter().find(|e| e.get("targetname") == Some("sab_bomb")).and_then(|e| e.origin()).map(crate::units::pos);
    (sites, bomb)
}

fn setup(
    mut commands: Commands,
    time: Res<Time>,
    content: Res<Content>,
    config: Res<crate::tdm::MatchConfig>,
    mut objectives: ResMut<Objectives>,
) {
    if config.mode != GameMode::Sab {
        return;
    }
    let (sites, home) = read_map(&content);
    info!("sab: targets {:?}, bomb at {:?}", sites.iter().map(|s| (s.team, crate::units::to_cod(s.pos))).collect::<Vec<_>>(), home.map(crate::units::to_cod));
    let home = home.unwrap_or(Vec3::ZERO);
    for (i, s) in sites.iter().enumerate() {
        commands.spawn((Name::new(format!("sabotage target {:?}", s.team)), SiteModel(i), Transform::from_translation(s.pos), Visibility::default()));
    }
    commands.spawn((Name::new("bomb"), BombModel, Transform::from_translation(home), Visibility::default()));
    let now = time.elapsed_secs();
    let length = config.time_limit.max(60.0);
    objectives.sites = sites;
    objectives.bomb = Some(Bomb { pos: home, carrier: None, planted: None, explodes_at: None });
    objectives.timer = Some((now + length, false));
    commands.insert_resource(SabRound {
        length,
        started: now,
        over: None,
        overtime: false,
        bomb_home: home,
        match_started: now,
        planter: None,
        tick: now,
        carrier_at: home,
        bomb_timer: BOMB_TIMER,
        planted_at: None,
        dropped_at: None,
        overtime_left: OVERTIME,
        planters_dead: false,
        full_length: length,
        hand_bomb: std::env::var_os("COD4RW_SDBOMB").is_some(),
    });
}

/// Planting or defusing, nobody moves.
fn hold_still(objectives: Res<Objectives>, mut pawns: Query<&mut MoveInput>) {
    for (e, ..) in &objectives.using {
        if let Ok(mut mv) = pawns.get_mut(*e) {
            (mv.forward, mv.right, mv.jump, mv.sprint) = (0.0, 0.0, false, false);
        }
    }
}

/// In overtime the dead wait for the next round.
fn sudden_death(time: Res<Time>, round: Option<Res<SabRound>>, mut dead: Query<&mut Dead, Added<Dead>>) {
    let now = time.elapsed_secs();
    if round.is_some_and(|r| r.overtime && r.over.is_none()) {
        for mut d in &mut dead {
            if d.respawn_at > now + 0.01 {
                d.respawn_at = f32::INFINITY;
            }
        }
    }
}

/// The announcer, to the player's team.
fn say(lines: &[(Team, &str)], mine: Option<Team>, sides: Option<&Sides>, sfx: &mut Sfx) {
    let Some(sides) = sides else { return };
    for (team, line) in lines.iter().filter(|(t, _)| Some(*t) == mine) {
        sfx.play(format!("{}_1mc_{line}", sides.of(*team).voice), None);
    }
}

/// The bomb: picked up, carried, dropped, planted, defused or gone off.
#[allow(clippy::too_many_arguments)]
fn bomb(
    time: Res<Time>,
    round: Option<ResMut<SabRound>>,
    mut objectives: ResMut<Objectives>,
    pawns: Query<(Entity, &Pawn, &Transform, &UseObjective, Has<Dead>)>,
    // In Last Stand the bomb is dropped and can't be taken, planted or
    // defused (`_gameobjects::onPlayerLastStand`).
    downed: Query<(), With<crate::perks::Downed>>,
    me: Query<(Entity, &Pawn), With<LocalPlayer>>,
    sides: Option<Res<Sides>>,
    mut sfx: ResMut<Sfx>,
    mut effects: ResMut<Effects>,
    mut damage: MessageWriter<Damage>,
    mut awards: MessageWriter<crate::ui::progression::Award>,
) {
    use crate::ui::progression::{Award, AwardKind};
    let Some(mut round) = round else { return };
    if round.over.is_some() {
        objectives.using.clear();
        return;
    }
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    let Some(mut b) = objectives.bomb else { return };
    let mine = me.single().ok().map(|(_, p)| p.team);
    let mut lines: Vec<(Team, &str)> = Vec::new();
    if round.hand_bomb && b.planted.is_none() {
        if let Ok((e, _)) = me.single() {
            round.hand_bomb = false;
            b.carrier = Some(e);
        }
    }
    let team_of = |e: Entity| pawns.get(e).ok().map(|x| x.1.team);

    if let Some(c) = b.carrier {
        match pawns.get(c) {
            Ok((_, _, tf, _, false)) if !downed.contains(c) => round.carrier_at = tf.translation,
            _ => {
                if let Some(t) = team_of(c) {
                    lines.push((t, "bomb_lost"));
                }
                b.carrier = None;
                b.pos = round.carrier_at;
                round.dropped_at = Some(now);
            }
        }
    }
    // Left lying a minute, it goes home.
    if b.carrier.is_none() && b.planted.is_none() && round.dropped_at.is_some_and(|t| now - t >= BOMB_RESET) {
        b.pos = round.bomb_home;
        round.dropped_at = None;
    }
    if b.carrier.is_none() && b.planted.is_none() {
        if let Some((e, p, ..)) = pawns.iter().find(|(e, _, tf, _, dead)| !dead && !downed.contains(*e) && in_pickup_reach(tf.translation, b.pos)) {
            b.carrier = Some(e);
            round.dropped_at = None;
            info!("sab: {} took the bomb", p.name);
            lines.push((p.team, "bomb_taken"));
        }
    }

    let previous = std::mem::take(&mut objectives.using);
    let progress_of = |e: Entity| previous.iter().find(|u| u.0 == e).map(|u| u.1);
    let planted_site = b.planted.and_then(|l| objectives.sites.iter().position(|s| s.label == l));
    for (e, p, tf, use_input, dead) in &pawns {
        if dead || downed.contains(e) || !use_input.0 {
            continue;
        }
        let at = tf.translation;
        let (defusing, needed) = if planted_site.is_some() { (true, DEFUSE_TIME) } else { (false, PLANT_TIME) };
        let target = if defusing {
            planted_site.filter(|&i| objectives.sites[i].team == Some(p.team) && at.distance(b.pos) < u(DEFUSE_RANGE))
        } else if b.carrier == Some(e) {
            objectives.sites.iter().position(|s| !s.destroyed && s.team != Some(p.team) && s.contains(at))
        } else {
            None
        };
        let Some(site) = target else { continue };
        if progress_of(e).is_none() {
            sfx.play(if defusing { "mp_bomb_start_defusing" } else { "MP_bomb_plant" }, Some(at));
        }
        let progress = progress_of(e).unwrap_or(0.0) + dt / needed;
        if progress < 1.0 {
            objectives.using.push((e, progress, defusing));
            continue;
        }
        let other = p.team.other();
        if defusing {
            info!("sab: {} defused the bomb", p.name);
            awards.write(Award { pawn: e, kind: AwardKind::Defuse });
            sfx.play("MP_bomb_defuse", Some(b.pos));
            // The defuser takes it; its timer keeps what was left, and the
            // clock goes on from where it stopped.
            if let Some(at) = round.planted_at.take() {
                round.bomb_timer = (round.bomb_timer - (now - at)).max(1.0);
                round.length += now - at;
            }
            b = Bomb { pos: b.pos, carrier: Some(e), planted: None, explodes_at: None };
            round.carrier_at = at;
            objectives.timer = (!round.overtime).then_some((round.started + round.length, false));
            // Sudden death with the planters gone: the defusers win.
            if round.overtime && round.planters_dead {
                objectives.round_over = Some((p.team, "MP_ENEMIES_ELIMINATED", "Enemies Eliminated"));
            }
            lines.extend([(p.team, "bomb_defused"), (other, "bomb_defused")]);
        } else {
            info!("sab: {} planted the bomb at the {:?} target", p.name, objectives.sites[site].team);
            awards.write(Award { pawn: e, kind: AwardKind::Plant });
            let timer = round.bomb_timer;
            b = Bomb { pos: objectives.sites[site].pos, carrier: None, planted: Some(objectives.sites[site].label), explodes_at: Some(now + timer) };
            round.planter = Some(e);
            round.planted_at = Some(now);
            objectives.timer = Some((now + timer, true));
            lines.extend([(p.team, "bomb_planted"), (other, "bomb_planted")]);
        }
        break;
    }

    if let Some(at) = b.explodes_at {
        if now - round.tick >= 1.0 {
            round.tick = now;
            sfx.play("bomb_tick", Some(b.pos));
        }
        if now >= at {
            effects.play("explosions/tanker_explosion", Anchor::Fixed(Frame::facing(b.pos, Vec3::Y, 0.0)), FxLayer::World);
            sfx.play("exp_suitcase_bomb_main", Some(b.pos));
            for (e, _, tf, _, dead) in &pawns {
                let d = tf.translation.distance(b.pos) / crate::units::INCH;
                if !dead && d < BLAST_RADIUS {
                    let amount = BLAST_MAX + (BLAST_MIN - BLAST_MAX) * d / BLAST_RADIUS;
                    damage.write(Damage { target: e, attacker: round.planter, amount, location: HitLocation::Torso, weapon: "briefcase_bomb_mp" });
                }
            }
            let mut winner = None;
            if let Some(site) = objectives.sites.iter_mut().find(|s| Some(s.label) == b.planted) {
                site.destroyed = true;
                winner = site.team.map(|t| t.other());
            }
            info!("sab: the bomb went off; round to {winner:?}");
            b.explodes_at = None;
            if let Some(w) = winner {
                objectives.round_over = Some((w, "MP_TARGET_DESTROYED", "Target Destroyed"));
            }
        }
    }
    objectives.bomb = Some(b);
    say(&lines, mine, sides.as_deref(), &mut sfx);
}

/// Who won the round; overtime; the next round.
#[allow(clippy::too_many_arguments)]
fn rounds(
    mut commands: Commands,
    time: Res<Time>,
    round: Option<ResMut<SabRound>>,
    state: Option<ResMut<MatchState>>,
    mut objectives: ResMut<Objectives>,
    pawns: Query<(Entity, &Pawn, Has<Dead>)>,
    me: Query<&Pawn, With<LocalPlayer>>,
    sides: Option<Res<Sides>>,
    mut sfx: ResMut<Sfx>,
) {
    let (Some(mut round), Some(mut state)) = (round, state) else { return };
    let now = time.elapsed_secs();
    if round.match_started != state.started {
        round.match_started = state.started;
        start_round(&mut commands, &mut round, &mut objectives, now, &pawns, false);
        return;
    }
    if state.ended.is_some() {
        return;
    }
    let mine = me.single().ok().map(|p| p.team);
    match round.over {
        None => {
            let planted = objectives.bomb.is_some_and(|b| b.planted.is_some());
            // Time's up with no bomb down: overtime. Everyone comes back,
            // then sudden death (`onOvertime`).
            if !round.overtime && !planted && now >= round.started + round.length {
                info!("sab: overtime");
                round.overtime = true;
                round.overtime_left = OVERTIME;
                objectives.sudden_death = true;
                objectives.timer = Some((now + OVERTIME, false));
                for (e, _, dead) in &pawns {
                    if dead {
                        commands.entity(e).insert(Dead { respawn_at: now, killer: None });
                    }
                }
                if let (Some(s), Some(t)) = (&sides, mine) {
                    sfx.play(format!("{}_1mc_overtime", s.of(t).voice), None);
                }
            }
            // Its clock runs while the bomb isn't down; out of time, a tie.
            let mut tie = false;
            if round.overtime && !planted {
                round.overtime_left -= time.delta_secs();
                objectives.timer = Some((now + round.overtime_left.max(0.0), false));
                tie = round.overtime_left <= 0.0;
            }
            // `onDeadEvent` (sudden death only: before it, the dead come
            // back): everyone dead is the planters' with the bomb down, else
            // a tie; the planters dead with it down, wait for it; anyone
            // else dead loses.
            let wiped = |team: Team| pawns.iter().any(|(_, p, _)| p.team == team) && !pawns.iter().any(|(_, p, dead)| p.team == team && !dead);
            let planters = round.planter.and_then(|e| pawns.get(e).ok()).map(|x| x.1.team).filter(|_| planted);
            let eliminated = Some((Team::Allies, "MP_ENEMIES_ELIMINATED", "Enemies Eliminated"));
            let result = objectives.round_over.or_else(|| {
                if !round.overtime || objectives.round_over.is_some() {
                    return None;
                }
                let (allies, axis) = (wiped(Team::Allies), wiped(Team::Axis));
                match (allies, axis) {
                    (true, true) => match planters {
                        Some(t) => Some((t, "MP_TARGET_DESTROYED", "Target Destroyed")),
                        None => {
                            tie = true;
                            None
                        }
                    },
                    (true, false) | (false, true) => {
                        let dead_team = if allies { Team::Allies } else { Team::Axis };
                        if planters == Some(dead_team) {
                            round.planters_dead = true;
                            None
                        } else {
                            eliminated.map(|(_, k, t)| (dead_team.other(), k, t))
                        }
                    }
                    _ => None,
                }
            });
            if tie {
                // No winner: with its one round, the match is a draw
                // (`endGame("tie")`).
                info!("sab: a tie");
                round.over = Some(now);
                crate::killcam::round_over(now);
                state.ended = Some((None, now));
                objectives.using.clear();
                for (e, ..) in &pawns {
                    commands.entity(e).insert(Frozen);
                }
                return;
            }
            let Some((winner, key, text)) = result else { return };
            info!("sab: round to {:?} ({text})", winner);
            match winner {
                Team::Allies => state.allies += 1,
                Team::Axis => state.axis += 1,
            }
            objectives.round_over = Some((winner, key, text));
            objectives.using.clear();
            round.over = Some(now);
            crate::killcam::round_over(now);
            for (e, ..) in &pawns {
                commands.entity(e).insert(Frozen);
            }
        }
        Some(at) if now - at >= ROUND_DELAY && !crate::killcam::busy() => start_round(&mut commands, &mut round, &mut objectives, now, &pawns, true),
        Some(_) => {}
    }
}

fn start_round(commands: &mut Commands, round: &mut SabRound, objectives: &mut Objectives, now: f32, pawns: &Query<(Entity, &Pawn, Has<Dead>)>, respawn: bool) {
    (round.started, round.over, round.overtime, round.planter, round.carrier_at) = (now, None, false, None, round.bomb_home);
    (round.bomb_timer, round.planted_at, round.dropped_at, round.overtime_left) = (BOMB_TIMER, None, None, OVERTIME);
    round.planters_dead = false;
    round.length = round.full_length;
    objectives.bomb = Some(Bomb { pos: round.bomb_home, carrier: None, planted: None, explodes_at: None });
    objectives.timer = Some((now + round.length, false));
    objectives.using.clear();
    objectives.round_over = None;
    objectives.sudden_death = false;
    for s in &mut objectives.sites {
        s.destroyed = false;
    }
    if respawn {
        for (e, ..) in pawns {
            commands.entity(e).insert((Dead { respawn_at: now, killer: None }, Frozen));
        }
    }
}

/// The targets (whole or blown up) and the bomb where it lies.
#[allow(clippy::too_many_arguments)]
fn show_models(
    mut commands: Commands,
    objectives: Res<Objectives>,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    sites: Query<(Entity, &SiteModel), Without<BombModel>>,
    mut bomb: Query<(Entity, &mut Transform, &mut Visibility, Option<&Children>), With<BombModel>>,
    mut shown: Local<Vec<Option<bool>>>,
) {
    let mut model = |commands: &mut Commands, owner: Entity, name: &str| {
        if let Some(m) = content.model(name, &mut meshes, &mut materials, &mut images, &mut bindposes) {
            spawn_model(commands, &mut Skeleton::default(), SpawnModel { model: &m, owner, attach_to: None, layers: None, shadows: true });
        }
    };
    shown.resize(objectives.sites.len(), None);
    for (e, SiteModel(i)) in &sites {
        let Some(site) = objectives.sites.get(*i) else { continue };
        if shown[*i] == Some(site.destroyed) {
            continue;
        }
        shown[*i] = Some(site.destroyed);
        commands.entity(e).despawn_children();
        model(&mut commands, e, if site.destroyed { "com_bomb_objective_d" } else { "com_bomb_objective" });
    }
    let Some(b) = objectives.bomb else { return };
    for (e, mut tf, mut vis, children) in &mut bomb {
        if children.is_none_or(|c| c.is_empty()) {
            model(&mut commands, e, "mil_tntbomb_mp");
        }
        if tf.translation != b.pos {
            tf.translation = b.pos;
        }
        vis.set_if_neq(if b.carrier.is_some() { Visibility::Hidden } else { Visibility::Inherited });
    }
}
