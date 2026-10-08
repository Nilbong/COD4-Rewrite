//! Headquarters (`koth.gsc` as ranked play ran it): an HQ comes up at one
//! of the map's `hq_hardpoint`s, 5 seconds into the match. Standing in its
//! `radiotrigger` with no enemy there takes it in 20 seconds, less with
//! more of the team in it (leaving for more than a second starts over).
//! The holders score 5 points every 5 seconds, each of them XP for it
//! (`awardHQPoints`), and can't respawn while they hold it, until it goes:
//! the enemy standing in it for 10 seconds destroys it, or it goes offline
//! after 60 seconds held. The next HQ comes up elsewhere 3 seconds later.
//! Taking or destroying it is XP to whoever started it; kills in it are
//! assaults or defences. 250 wins (30 minutes); spawns are Team
//! Deathmatch's.

use super::{GameMode, Hq, Objectives, brush_box, current};
use crate::audio::{Sfx, Sides};
use crate::combat::{Dead, Pawn, Team};
use crate::content::Content;
use crate::models::{Skeleton, SpawnModel, spawn_model};
use crate::player::LocalPlayer;
use crate::state::{GameState, Setup, in_game};
use crate::tdm::MatchState;
use crate::ui::progression::{Award, AwardKind};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use rand::seq::SliceRandom;

pub struct KothPlugin;

impl Plugin for KothPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::InGame), setup.in_set(Setup::Spawn)).add_systems(
            Update,
            (headquarters, kill_awards, hold_respawns, show_radio).chain().after(crate::movement::MovementSet).run_if(in_game.and_then(playing)),
        );
    }
}

fn playing() -> bool {
    current() == GameMode::Koth
}

/// `koth_capturetime`, `koth_destroytime`, `koth_autodestroytime`.
const CAPTURE_TIME: f32 = 20.0;
const DESTROY_TIME: f32 = 10.0;
const HOLD_TIME: f32 = 60.0;
/// `awardHQPoints`: 5 points every 5 seconds.
const SCORE_EVERY: f32 = 5.0;
const SCORE_POINTS: u32 = 5;
/// The first HQ comes up this long into the match, the next this long
/// after one goes.
const FIRST_HQ: f32 = 5.0;
const NEXT_HQ: f32 = 3.0;
/// `setClaimTeam`: the takers can step out this long and keep their
/// progress.
const CLAIM_GRACE: f32 = 1.0;

/// An HQ spot: the radio, its trigger and its props (`hq_hardpoint` and
/// what it targets).
#[derive(Clone, Debug)]
struct Radio {
    pos: Vec3,
    min: Vec3,
    max: Vec3,
    models: Vec<(String, Transform)>,
}

#[derive(Resource)]
struct Headquarters {
    radios: Vec<Radio>,
    active: usize,
    /// Spots yet to come up, so they take turns.
    queue: Vec<usize>,
    next_score: f32,
    match_started: f32,
    /// None up: when the next comes.
    next_at: Option<f32>,
    /// Who started the capture under way (`claimPlayer`), and when its
    /// takers last stepped out.
    claimer: Option<Entity>,
    left_at: Option<f32>,
}

/// The props of the HQ up now.
#[derive(Component)]
struct RadioProps(usize);

/// CoD angles (pitch, yaw, roll in degrees) as a Bevy rotation.
pub(crate) fn cod_rotation([p, y, r]: [f32; 3]) -> Quat {
    let (sp, cp) = p.to_radians().sin_cos();
    let (sy, cy) = y.to_radians().sin_cos();
    let (sr, cr) = r.to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let left = [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    crate::units::axis_rotation([forward, left, up])
}

/// The map's HQ spots: each `hq_hardpoint` model with the models it
/// targets, in the `radiotrigger` it stands in.
fn read_radios(content: &Content) -> Vec<Radio> {
    let map = content.map();
    let ents = map.map_ents().map(|m| iw3::ents::parse(&m.entity_string)).unwrap_or_default();
    let clip = map.clip_map();
    let triggers: Vec<(Vec3, Vec3)> = ents.iter().filter(|e| e.get("targetname") == Some("radiotrigger")).filter_map(|e| brush_box(e, clip, 160.0)).collect();
    let model = |e: &iw3::ents::Entity| -> Option<(String, Transform)> {
        let pos = crate::units::pos(e.origin()?);
        Some((e.get("model")?.to_owned(), Transform::from_translation(pos).with_rotation(cod_rotation(e.angles()))))
    };
    ents.iter()
        .filter(|e| e.get("targetname") == Some("hq_hardpoint"))
        .filter_map(|radio| {
            let pos = crate::units::pos(radio.origin()?);
            let mut models: Vec<_> = model(radio).into_iter().collect();
            if let Some(t) = radio.get("target") {
                models.extend(ents.iter().filter(|e| e.get("targetname") == Some(t) && e.classname() == "script_model").filter_map(model));
            }
            let pad = Vec3::splat(crate::units::u(8.0));
            let (min, max) = triggers
                .iter()
                .copied()
                .find(|(min, max)| (pos.cmpge(*min - pad) & pos.cmple(*max + pad)).all())
                .unwrap_or((pos - Vec3::splat(crate::units::u(80.0)), pos + Vec3::splat(crate::units::u(80.0))));
            Some(Radio { pos, min, max, models })
        })
        .collect()
}

fn hq_at(radio: &Radio) -> Hq {
    Hq { pos: radio.pos, min: radio.min, max: radio.max, owner: None, capture: None, contested: false, expires_at: None }
}

fn setup(
    mut commands: Commands,
    time: Res<Time>,
    content: Res<Content>,
    config: Res<crate::tdm::MatchConfig>,
    mut objectives: ResMut<Objectives>,
) {
    if config.mode != GameMode::Koth {
        return;
    }
    let radios = read_radios(&content);
    info!("koth: {} HQ spots", radios.len());
    if radios.is_empty() {
        return;
    }
    // The first comes up shortly (`onStartGameType`'s `wait 5`).
    let now = time.elapsed_secs();
    let mut hq = Headquarters { radios, active: 0, queue: Vec::new(), next_score: 0.0, match_started: now, next_at: Some(now + FIRST_HQ), claimer: None, left_at: None };
    objectives.hq = None;
    // Debug runs placing the player (`COD4RW_SPAWN`): the first HQ is the
    // one nearest them, for screenshots.
    let spawn: Option<Vec<f32>> = std::env::var("COD4RW_SPAWN").ok().map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect());
    if let Some(at) = spawn.filter(|v| v.len() >= 3).map(|v| crate::units::pos([v[0], v[1], v[2]])) {
        let nearest = (0..hq.radios.len()).min_by(|&a, &b| hq.radios[a].pos.distance(at).total_cmp(&hq.radios[b].pos.distance(at)));
        if let Some(i) = nearest {
            hq.active = i;
            hq.queue.retain(|&q| q != i);
            hq.next_at = None;
            objectives.hq = Some(hq_at(&hq.radios[i]));
        }
    }
    commands.insert_resource(hq);
}

/// The next spot comes up (not the one just gone).
fn next_hq(hq: &mut Headquarters, objectives: &mut Objectives) {
    if hq.queue.is_empty() {
        hq.queue = (0..hq.radios.len()).filter(|&i| hq.radios.len() < 2 || i != hq.active).collect();
        hq.queue.shuffle(&mut rand::rng());
    }
    hq.active = hq.queue.pop().unwrap_or(0);
    objectives.hq = Some(hq_at(&hq.radios[hq.active]));
}

/// Take, hold, score and lose the HQ; the announcer tells the player's team.
#[allow(clippy::too_many_arguments)]
fn headquarters(
    time: Res<Time>,
    state: Option<ResMut<MatchState>>,
    hq: Option<ResMut<Headquarters>>,
    mut objectives: ResMut<Objectives>,
    pawns: Query<(Entity, &Pawn, &Transform), Without<Dead>>,
    everyone: Query<(Entity, &Pawn)>,
    me: Query<&Pawn, With<LocalPlayer>>,
    sides: Option<Res<Sides>>,
    mut sfx: ResMut<Sfx>,
    mut awards: MessageWriter<Award>,
) {
    let (Some(mut state), Some(mut hq)) = (state, hq) else { return };
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    let mut lines: Vec<(Option<Team>, &str)> = Vec::new();
    // A new match: a fresh HQ, shortly.
    if hq.match_started != state.started {
        hq.match_started = state.started;
        objectives.hq = None;
        hq.next_at = Some(now + FIRST_HQ);
    }
    if state.ended.is_some() {
        return;
    }
    if hq.next_at.is_some_and(|at| now >= at) {
        hq.next_at = None;
        next_hq(&mut hq, &mut objectives);
        (hq.claimer, hq.left_at) = (None, None);
        info!("koth: HQ up at {:?}", objectives.hq.as_ref().map(|h| h.pos));
        lines.push((None, "hq_located"));
    }
    let Some(mut current) = objectives.hq.clone() else {
        announce(lines, &me, sides.as_deref(), &mut sfx);
        return;
    };
    let mut inside: Vec<(Entity, Team)> = Vec::new();
    for (e, p, tf) in &pawns {
        if current.contains(tf.translation) {
            inside.push((e, p.team));
        }
    }
    let count = |t: Team| inside.iter().filter(|x| x.1 == t).count();
    let (allies, axis) = (count(Team::Allies), count(Team::Axis));
    current.contested = allies > 0 && axis > 0;
    let alone = match (allies, axis) {
        (a, 0) if a > 0 => Some(Team::Allies),
        (0, x) if x > 0 => Some(Team::Axis),
        _ => None,
    };
    let mut gone = false;
    match current.owner {
        // Up for grabs: taken by a team alone in it.
        None => match alone {
            Some(team) => {
                if take(&mut hq, &mut current, &inside, team, dt / CAPTURE_TIME, &mut awards) {
                    info!("koth: {:?} took the HQ", team);
                    (current.owner, current.capture, current.expires_at) = (Some(team), None, Some(now + HOLD_TIME));
                    hq.next_score = now + SCORE_EVERY;
                    lines.push((Some(team), "hq_secured"));
                    lines.push((Some(team.other()), "hq_captured"));
                }
            }
            None => lapse(&mut hq, &mut current, now),
        },
        // Held: scoring, until destroyed or offline.
        Some(owner) => {
            if now >= hq.next_score {
                hq.next_score += SCORE_EVERY;
                match owner {
                    Team::Allies => state.allies += SCORE_POINTS,
                    Team::Axis => state.axis += SCORE_POINTS,
                }
                for (e, p) in &everyone {
                    if p.team == owner {
                        awards.write(Award { pawn: e, kind: AwardKind::Defend });
                    }
                }
            }
            let enemy = owner.other();
            if alone == Some(enemy) {
                if take(&mut hq, &mut current, &inside, enemy, dt / DESTROY_TIME, &mut awards) {
                    info!("koth: {:?} destroyed the HQ", enemy);
                    lines.push((None, "hq_destroyed"));
                    gone = true;
                }
            } else {
                lapse(&mut hq, &mut current, now);
            }
            if !gone && current.expires_at.is_some_and(|at| now >= at) {
                info!("koth: the HQ went offline");
                lines.push((None, "hq_offline"));
                gone = true;
            }
        }
    }
    if gone {
        objectives.hq = None;
        hq.next_at = Some(now + NEXT_HQ);
    } else {
        objectives.hq = Some(current);
    }
    announce(lines, &me, sides.as_deref(), &mut sfx);
}

/// Take (or destroy) it a step: `updateUseRate`, each taker in it speeds it
/// up. Done, the credit is whoever started it, if still there.
fn take(hq: &mut Headquarters, current: &mut Hq, inside: &[(Entity, Team)], team: Team, step: f32, awards: &mut MessageWriter<Award>) -> bool {
    hq.left_at = None;
    let start = match current.capture {
        Some((t, p)) if t == team => p,
        _ => {
            hq.claimer = inside.iter().find(|x| x.1 == team).map(|x| x.0);
            0.0
        }
    };
    let progress = start + step * inside.iter().filter(|x| x.1 == team).count() as f32;
    if progress < 1.0 {
        current.capture = Some((team, progress));
        return false;
    }
    let credit = hq.claimer.filter(|c| inside.iter().any(|x| x.0 == *c)).or_else(|| inside.iter().find(|x| x.1 == team).map(|x| x.0));
    if let Some(e) = credit {
        awards.write(Award { pawn: e, kind: AwardKind::Capture });
    }
    hq.claimer = None;
    true
}

/// Nobody's taking it: `setClaimTeam`, a second out keeps the progress
/// (contested, it holds).
fn lapse(hq: &mut Headquarters, current: &mut Hq, now: f32) {
    if current.capture.is_some() && !current.contested && now - *hq.left_at.get_or_insert(now) > CLAIM_GRACE {
        current.capture = None;
    }
}

/// The announcer: to one team, or both.
fn announce(lines: Vec<(Option<Team>, &str)>, me: &Query<&Pawn, With<LocalPlayer>>, sides: Option<&Sides>, sfx: &mut Sfx) {
    let mine = me.single().ok().map(|p| p.team);
    if let (Some(sides), Some(mine)) = (sides, mine) {
        for (_, line) in lines.into_iter().filter(|(t, _)| t.is_none_or(|t| t == mine)) {
            sfx.play(format!("{}_1mc_{line}", sides.of(mine).voice), None);
        }
    }
}

/// Kills in the held HQ (`onPlayerKilled`): of its holders there, an
/// assault, of their enemies a defence; and killing from in it, a defence
/// for its holders, an assault for the rest.
fn kill_awards(
    mut killed: MessageReader<crate::combat::Killed>,
    objectives: Res<Objectives>,
    pawns: Query<(&Pawn, &Transform)>,
    mut awards: MessageWriter<Award>,
) {
    for k in killed.read() {
        let Some(attacker) = k.attacker.filter(|&a| a != k.victim) else { continue };
        let (Some(hq), Ok((a, a_at)), Ok((v, v_at))) = (objectives.hq.as_ref(), pawns.get(attacker), pawns.get(k.victim)) else { continue };
        let Some(owner) = hq.owner else { continue };
        if !crate::combat::hostile(a, v) {
            continue;
        }
        if hq.contains(v_at.translation) {
            let kind = if v.team == owner { AwardKind::Assault } else { AwardKind::Defend };
            awards.write(Award { pawn: attacker, kind });
        }
        if hq.contains(a_at.translation) {
            let kind = if a.team == owner { AwardKind::Defend } else { AwardKind::Assault };
            awards.write(Award { pawn: attacker, kind });
        }
    }
}

/// While a team holds the HQ its dead stay dead; when it goes, they're back
/// (those it held, not the pawns still choosing a class).
fn hold_respawns(
    time: Res<Time>,
    objectives: Res<Objectives>,
    mut dead: Query<(Entity, &Pawn, &mut Dead)>,
    mut held: Local<std::collections::HashSet<Entity>>,
) {
    let now = time.elapsed_secs();
    let holders = objectives.hq.as_ref().and_then(|h| h.owner);
    for team in [Team::Allies, Team::Axis] {
        crate::modes::lock_respawns(team, holders == Some(team));
    }
    held.retain(|e| dead.contains(*e));
    for (e, p, mut d) in &mut dead {
        match holders {
            Some(t) if p.team == t && d.respawn_at.is_finite() => {
                d.respawn_at = f32::INFINITY;
                held.insert(e);
            }
            Some(t) if p.team == t => {}
            _ if held.remove(&e) => d.respawn_at = now,
            _ => {}
        }
    }
}

/// The radio and its props where the HQ is up.
#[allow(clippy::too_many_arguments)]
fn show_radio(
    mut commands: Commands,
    hq: Option<Res<Headquarters>>,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    props: Query<(Entity, &RadioProps)>,
) {
    let Some(hq) = hq else { return };
    if props.iter().any(|(_, p)| p.0 == hq.active) {
        return;
    }
    for (e, _) in &props {
        commands.entity(e).despawn();
    }
    for (name, tf) in &hq.radios[hq.active].models {
        let e = commands.spawn((Name::new(format!("hq {name}")), RadioProps(hq.active), *tf, Visibility::default())).id();
        if let Some(m) = content.model(name, &mut meshes, &mut materials, &mut images, &mut bindposes) {
            spawn_model(&mut commands, &mut Skeleton::default(), SpawnModel { model: &m, owner: e, attach_to: None, layers: None, shadows: true });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cod_angles_turn_models() {
        // Yaw 90: CoD's forward (+X) turns to +Y, which is Bevy's -Z.
        let q = cod_rotation([0.0, 90.0, 0.0]);
        let f = q * crate::units::dir([1.0, 0.0, 0.0]);
        assert!((f - crate::units::dir([0.0, 1.0, 0.0])).length() < 1e-4, "{f}");
        assert!((cod_rotation([0.0, 0.0, 0.0]).w.abs() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn hq_spots_take_turns() {
        let radio = |x: f32| Radio { pos: Vec3::X * x, min: Vec3::ZERO, max: Vec3::ONE, models: Vec::new() };
        let mut hq = Headquarters { radios: (0..3).map(|i| radio(i as f32)).collect(), active: 0, queue: Vec::new(), next_score: 0.0, match_started: 0.0, next_at: None, claimer: None, left_at: None };
        let mut o = Objectives::default();
        let mut last = hq.active;
        for _ in 0..10 {
            next_hq(&mut hq, &mut o);
            assert_ne!(hq.active, last);
            last = hq.active;
        }
    }
}
