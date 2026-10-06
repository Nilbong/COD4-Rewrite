//! Domination (`_dom.gsc`): the map's flags A, B and C (`flag_primary`
//! triggers). Standing in one the other team holds (or nobody does) takes it
//! in 10 seconds, less with more of the team on it (`updateUseRate`), while
//! no enemy stands in it too; leaving for more than a second starts it over
//! (`setClaimTeam`). Every 5 seconds each team scores a point per flag it
//! holds. The flags fly their holder's colours, and the announcer calls
//! captures. Taking a flag is worth XP to everyone who took it, and so is
//! killing an enemy standing in one: an assault if they held it, else a
//! defence (`giveRankXP( "capture" )`, `"assault"`, `"defend"`).

use super::{Flag, GameMode, Objectives, current};
use crate::audio::{Sfx, Sides};
use crate::combat::{Dead, Killed, Pawn, Team, hostile};
use crate::content::Content;
use crate::models::{Skeleton, SpawnModel, spawn_model};
use crate::player::LocalPlayer;
use crate::state::{GameState, Setup, in_game};
use crate::tdm::MatchState;
use crate::ui::progression::{Award, AwardKind};
use crate::units::u;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

pub struct DomPlugin;

impl Plugin for DomPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::InGame), setup.in_set(Setup::Spawn))
            .add_systems(
                Update,
                (capture, defends, score, show_flags).chain().after(crate::movement::MovementSet).run_if(in_game.and_then(playing)),
            );
    }
}

fn playing() -> bool {
    current() == GameMode::Dom
}

/// `setUseTime( 10.0 )`.
const CAPTURE_TIME: f32 = 10.0;
/// `setClaimTeam`: the takers can step out this long and keep their
/// progress.
const CLAIM_GRACE: f32 = 1.0;
/// `updateDomScores`: a point per flag every 5 seconds.
const SCORE_EVERY: f32 = 5.0;

/// A flag's model, for flag `index`; `shown` is the holder it was made for.
#[derive(Component)]
struct FlagModel {
    index: usize,
    shown: Option<Option<Team>>,
}

/// When the teams next score, and the round that's for.
#[derive(Resource)]
struct DomClock {
    next_score: f32,
    round: f32,
}

/// The map's flags, from its entities: `trigger_radius`es named
/// `flag_primary` (and `flag_secondary`), labelled by `script_label`.
pub fn read_flags(ents: &[iw3::ents::Entity]) -> Vec<Flag> {
    let mut flags: Vec<Flag> = ents
        .iter()
        .filter(|e| matches!(e.get("targetname"), Some("flag_primary" | "flag_secondary")))
        .filter_map(|e| {
            let label = e.get("script_label")?.trim_start_matches('_').chars().next()?.to_ascii_uppercase();
            let num = |k: &str, d: f32| e.get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
            Some(Flag {
                label,
                pos: crate::units::pos(e.origin()?),
                radius: u(num("radius", 160.0)),
                height: u(num("height", 128.0)),
                owner: None,
                capture: None,
                contested: false,
            })
        })
        .collect();
    flags.sort_by_key(|f| f.label);
    flags.dedup_by_key(|f| f.label);
    flags
}

fn setup(
    mut commands: Commands,
    time: Res<Time>,
    content: Res<Content>,
    config: Res<crate::tdm::MatchConfig>,
    mut objectives: ResMut<Objectives>,
) {
    *objectives = Objectives::default();
    // The match's own mode: `current` is set alongside, in the same step.
    if config.mode != GameMode::Dom {
        return;
    }
    let ents = content.map().map_ents().map(|m| iw3::ents::parse(&m.entity_string)).unwrap_or_default();
    objectives.flags = read_flags(&ents);
    info!("dom: flags {:?}", objectives.flags.iter().map(|f| f.label).collect::<String>());
    for (index, flag) in objectives.flags.iter().enumerate() {
        commands.spawn((Name::new(format!("flag {}", flag.label)), FlagModel { index, shown: None }, Transform::from_translation(flag.pos), Visibility::default()));
    }
    let now = time.elapsed_secs();
    commands.insert_resource(DomClock { next_score: now + SCORE_EVERY, round: now });
}

/// Take and lose flags; the announcer tells the player's team.
#[allow(clippy::too_many_arguments)]
fn capture(
    time: Res<Time>,
    state: Option<Res<MatchState>>,
    mut objectives: ResMut<Objectives>,
    pawns: Query<(Entity, &Pawn, &Transform), Without<Dead>>,
    me: Query<&Pawn, With<LocalPlayer>>,
    sides: Option<Res<Sides>>,
    mut sfx: ResMut<Sfx>,
    mut awards: MessageWriter<Award>,
    mut left_at: Local<Vec<Option<f32>>>,
) {
    let Some(state) = state else { return };
    if state.ended.is_some() {
        return;
    }
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    left_at.resize(objectives.flags.len(), None);
    let mine = me.single().ok().map(|p| p.team);
    // For each team: announcer lines (`<voice>_1mc_<line>`) and sounds.
    let mut heard: Vec<(Team, String)> = Vec::new();
    let mut say = |team: Team, line: String| heard.push((team, line));
    for (i, flag) in objectives.flags.iter_mut().enumerate() {
        let mut present = [0usize; 2];
        let mut inside: Vec<(Entity, Team)> = Vec::new();
        for (e, p, tf) in &pawns {
            if flag.contains(tf.translation) {
                present[(p.team == Team::Axis) as usize] += 1;
                inside.push((e, p.team));
            }
        }
        flag.contested = present[0] > 0 && present[1] > 0;
        let claim = match present {
            [a, 0] if a > 0 => Some(Team::Allies),
            [0, x] if x > 0 => Some(Team::Axis),
            _ => None,
        };
        let l = flag.letter();
        match claim.filter(|&t| Some(t) != flag.owner) {
            Some(team) => {
                left_at[i] = None;
                let progress = match flag.capture {
                    Some((t, p)) if t == team => p,
                    _ => {
                        say(team, format!("securing_{l}"));
                        if let Some(owner) = flag.owner {
                            say(owner, format!("losing_{l}"));
                        }
                        0.0
                    }
                };
                // Each of them on it speeds it up.
                let progress = progress + dt * present[(team == Team::Axis) as usize] as f32 / CAPTURE_TIME;
                if progress < 1.0 {
                    flag.capture = Some((team, progress));
                    continue;
                }
                let previous = flag.owner.replace(team);
                flag.capture = None;
                info!("dom: {:?} took {}", team, flag.label);
                for &(e, _) in inside.iter().filter(|(_, t)| *t == team) {
                    awards.write(Award { pawn: e, kind: AwardKind::Capture });
                }
                say(team, format!("secure_{l}"));
                say(team, "mp_war_objective_taken".into());
                match previous {
                    Some(lost) => {
                        say(lost, format!("lost_{l}"));
                        say(lost, "mp_war_objective_lost".into());
                    }
                    None => say(team.other(), format!("enemy_has_{l}")),
                }
            }
            // Contested: the capture holds where it got to.
            None if flag.contested => {}
            // Left (or the holders are back): after a second's grace, it
            // starts over.
            None => {
                if flag.capture.is_some() && now - *left_at[i].get_or_insert(now) > CLAIM_GRACE {
                    flag.capture = None;
                }
                if flag.capture.is_none() {
                    left_at[i] = None;
                }
            }
        }
    }
    for (team, line) in heard.into_iter().filter(|(t, _)| mine == Some(*t)) {
        match (line.starts_with("mp_"), &sides) {
            (true, _) => sfx.play(line, None),
            (false, Some(sides)) => sfx.play(format!("{}_1mc_{line}", sides.of(team).voice), None),
            (false, None) => {}
        }
    }
}

/// Killing an enemy standing in a flag: assaulting it if they held it,
/// else defending it (`onPlayerKilled`).
fn defends(
    mut killed: MessageReader<Killed>,
    objectives: Res<Objectives>,
    pawns: Query<(&Pawn, &Transform)>,
    mut awards: MessageWriter<Award>,
) {
    for k in killed.read() {
        let Some(attacker) = k.attacker.filter(|&a| a != k.victim) else { continue };
        let (Ok((a, _)), Ok((v, at))) = (pawns.get(attacker), pawns.get(k.victim)) else { continue };
        let Some(flag) = objectives.flags.iter().find(|f| f.contains(at.translation)) else { continue };
        if hostile(a, v) {
            let kind = if flag.owner == Some(v.team) { AwardKind::Assault } else { AwardKind::Defend };
            awards.write(Award { pawn: attacker, kind });
        }
    }
}

/// A point a flag held, every five seconds; a new round starts neutral.
fn score(time: Res<Time>, state: Option<ResMut<MatchState>>, clock: Option<ResMut<DomClock>>, mut objectives: ResMut<Objectives>) {
    let (Some(mut state), Some(mut clock)) = (state, clock) else { return };
    let now = time.elapsed_secs();
    if clock.round != state.started {
        clock.round = state.started;
        clock.next_score = now + SCORE_EVERY;
        for f in &mut objectives.flags {
            (f.owner, f.capture, f.contested) = (None, None, false);
        }
    }
    if state.ended.is_some() || now < clock.next_score {
        return;
    }
    clock.next_score += SCORE_EVERY;
    let held = |team: Team| objectives.flags.iter().filter(|f| f.owner == Some(team)).count() as u32;
    state.allies += held(Team::Allies);
    state.axis += held(Team::Axis);
}

/// Each flag flies its holder's colours (`game["flagmodels"]`).
#[allow(clippy::too_many_arguments)]
fn show_flags(
    mut commands: Commands,
    objectives: Res<Objectives>,
    sides: Option<Res<Sides>>,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut flags: Query<(Entity, &mut FlagModel)>,
) {
    for (e, mut model) in &mut flags {
        let Some(flag) = objectives.flags.get(model.index) else { continue };
        if model.shown == Some(flag.owner) {
            continue;
        }
        model.shown = Some(flag.owner);
        let name = match (flag.owner, sides.as_deref().map(|s| (s.of(Team::Allies).music, s.of(Team::Axis).music))) {
            (None, _) => "prop_flag_neutral",
            (Some(Team::Allies), Some(("sas", _))) => "prop_flag_brit",
            (Some(Team::Allies), _) => "prop_flag_american",
            (Some(Team::Axis), Some((_, "soviet"))) => "prop_flag_russian",
            (Some(Team::Axis), _) => "prop_flag_opfor",
        };
        commands.entity(e).despawn_children();
        if let Some(m) = content.model(name, &mut meshes, &mut materials, &mut images, &mut bindposes) {
            spawn_model(&mut commands, &mut Skeleton::default(), SpawnModel { model: &m, owner: e, attach_to: None, layers: None, shadows: true });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_flag_triggers() {
        let ents = iw3::ents::parse(
            r#"{ "classname" "trigger_radius" "targetname" "flag_primary" "script_label" "_b" "origin" "618 512 137" "radius" "160" "height" "128" }
               { "classname" "trigger_radius" "targetname" "flag_primary" "script_label" "_a" "origin" "-347 1723 232" "radius" "120" "height" "128" }
               { "classname" "trigger_radius" "targetname" "ctf_trig_allies" "origin" "0 0 0" }"#,
        );
        let flags = read_flags(&ents);
        assert_eq!(flags.iter().map(|f| f.label).collect::<String>(), "AB");
        assert!((flags[0].radius - u(120.0)).abs() < 1e-5);
        assert!(flags.iter().all(|f| f.owner.is_none()));
    }
}
