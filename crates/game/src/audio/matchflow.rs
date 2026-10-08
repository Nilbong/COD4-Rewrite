//! The match's music and voices: the team's spawn music and the announcer
//! opening Team Deathmatch, lead changes, the closing minute and the last
//! ten seconds' ticks, victory or defeat, squad chatter when a teammate
//! kills nearby, and the map's ambience: its looping track and the sound
//! emitters its createfx script places (palms rustling, generators, flies).

use super::{Bank, Sfx, Sides};
use crate::combat::{Dead, Killed, Pawn};
use crate::content::Content;
use crate::loadout::AwaitingClass;
use crate::player::LocalPlayer;
use crate::tdm::MatchState;
use crate::units::u;
use bevy::prelude::*;
use std::cmp::Ordering;

pub(super) fn build(app: &mut App) {
    app.init_resource::<Ambience>()
        .add_systems(OnEnter(crate::state::GameState::InGame), |mut a: ResMut<Ambience>| *a = Ambience::default())
        // The ambient track's carrier goes with the match (its loop with it).
        .add_systems(OnExit(crate::state::GameState::InGame), |mut c: Commands, q: Query<Entity, With<AmbientTrack>>| {
            for e in &q {
                c.entity(e).despawn();
            }
        })
        .add_systems(Update, (spawn_cue, lead_changes, closing_time, match_end, chatter, ambience, emitters).run_if(crate::state::in_game));
}

/// The map's ambience still to start this match: its track, and its
/// emitters (read from its createfx script on the first frame).
#[derive(Resource, Default)]
struct Ambience {
    track_done: bool,
    emitters: Option<Vec<([f32; 3], String)>>,
}

/// The round the player last heard the spawn music in.
fn spawn_cue(
    time: Res<Time>,
    mut sfx: ResMut<Sfx>,
    sides: Option<Res<Sides>>,
    state: Option<Res<MatchState>>,
    player: Query<(&Pawn, Has<Dead>, Has<AwaitingClass>), With<LocalPlayer>>,
    mut heard: Local<Option<f32>>,
) {
    let (Some(sides), Some(state), Ok((p, dead, waiting))) = (sides, state, player.single()) else { return };
    if dead || waiting || *heard == Some(state.started) {
        return;
    }
    *heard = Some(state.started);
    let side = sides.of(p.team);
    let now = time.elapsed_secs();
    sfx.play(format!("mp_spawn_{}", side.music), None);
    sfx.play_later(format!("{}_1mc_{}", side.voice, state.mode.intro_line()), None, now, 1.5);
}

/// "We've taken the lead" / "We've lost the lead".
fn lead_changes(
    time: Res<Time>,
    mut sfx: ResMut<Sfx>,
    sides: Option<Res<Sides>>,
    state: Option<Res<MatchState>>,
    player: Query<&Pawn, With<LocalPlayer>>,
    mut last: Local<Option<Ordering>>,
    mut spoke: Local<f32>,
) {
    let (Some(sides), Some(state), Ok(p)) = (sides, state, player.single()) else { return };
    let lead = state.score(p.team).cmp(&state.score(p.team.other()));
    let now = time.elapsed_secs();
    if state.ended.is_none() && last.is_some_and(|l| l != lead) && now - *spoke > 8.0 {
        let line = match (*last, lead) {
            (_, Ordering::Greater) => Some("lead_taken"),
            (Some(Ordering::Greater), Ordering::Less) => Some("lead_lost"),
            (Some(Ordering::Greater), Ordering::Equal) => Some("tied"),
            _ => None,
        };
        if let Some(line) = line {
            sfx.play(format!("{}_1mc_{line}", sides.of(p.team).voice), None);
            *spoke = now;
        }
    }
    *last = Some(lead);
}

/// The closing minute's music while winning, and a tick each of the last
/// ten seconds.
fn closing_time(
    time: Res<Time>,
    mut sfx: ResMut<Sfx>,
    state: Option<Res<MatchState>>,
    player: Query<&Pawn, With<LocalPlayer>>,
    mut last_left: Local<f32>,
) {
    let (Some(state), Ok(p)) = (state, player.single()) else { return };
    let left = state.time_left(time.elapsed_secs());
    if state.ended.is_none() && left < *last_left {
        // Hardcore has no music for it (`_globallogic.gsc`'s `musicController`).
        if *last_left > 60.0 && left <= 60.0 && state.score(p.team) > state.score(p.team.other()) && !crate::tdm::hardcore() {
            sfx.play("mp_time_running_out_winning", None);
        }
        if left <= 10.0 && left.ceil() < last_left.ceil() {
            sfx.play("ui_mp_timer_countdown", None);
        }
    }
    *last_left = left;
}

fn match_end(
    mut sfx: ResMut<Sfx>,
    sides: Option<Res<Sides>>,
    state: Option<Res<MatchState>>,
    player: Query<&Pawn, With<LocalPlayer>>,
    mut over: Local<bool>,
) {
    let (Some(sides), Some(state), Ok(p)) = (sides, state, player.single()) else { return };
    let ended = state.ended.is_some();
    if ended && !*over {
        let side = sides.of(p.team);
        match state.outcome(p) {
            Some(true) => {
                sfx.play(format!("mp_victory_{}", side.music), None);
                sfx.play(format!("{}_1mc_mission_success", side.voice), None);
            }
            Some(false) => {
                sfx.play("mp_defeat", None);
                sfx.play(format!("{}_1mc_mission_fail", side.voice), None);
            }
            None => sfx.play(format!("{}_1mc_draw", side.voice), None),
        }
    }
    *over = ended;
}

/// A teammate calling "enemy down" after a kill nearby, now and then.
#[allow(clippy::too_many_arguments)]
fn chatter(
    time: Res<Time>,
    mut sfx: ResMut<Sfx>,
    sides: Option<Res<Sides>>,
    mut killed: MessageReader<Killed>,
    pawns: Query<(&Pawn, &Transform)>,
    player: Query<(Entity, &Pawn, &Transform), With<LocalPlayer>>,
    mut spoke: Local<f32>,
) {
    let (Some(sides), Ok((me, mine, at))) = (sides, player.single()) else {
        killed.clear();
        return;
    };
    let now = time.elapsed_secs();
    for k in killed.read() {
        let Some(killer) = k.attacker.filter(|&a| a != me && a != k.victim) else { continue };
        let Ok((p, tf)) = pawns.get(killer) else { continue };
        let near = tf.translation.distance(at.translation) < u(1500.0);
        if p.team == mine.team && near && now - *spoke > 6.0 && rand::random::<f32>() < 0.35 {
            *spoke = now;
            sfx.play_later(format!("{}_mp_stm_enemydown", sides.of(p.team).voice), Some(tf.translation), now, 0.6);
        }
    }
}

/// The map script's `ambientPlay("ambient_...")`, looped once its alias
/// has loaded.
/// What the map's ambient track plays on: a looping alias loops only on an
/// entity (`Sfx::play_flat_on`); played free it played once and ended.
#[derive(Component)]
struct AmbientTrack;

fn ambience(mut commands: Commands, mut sfx: ResMut<Sfx>, content: Res<Content>, map: Res<crate::world::MapName>, bank: Option<Res<Bank>>, mut state: ResMut<Ambience>) {
    if state.track_done {
        return;
    }
    let script = format!("maps/mp/{}.gsc", map.0);
    let alias = content.zones.iter().flat_map(|z| &z.assets).find_map(|a| match a {
        iw3::zone::Asset::RawFile(r) if r.name.eq_ignore_ascii_case(&script) => {
            let text = String::from_utf8_lossy(&r.data);
            let at = text.find("ambientPlay(")?;
            text[at..].split('"').nth(1).map(str::to_owned)
        }
        _ => None,
    });
    let Some(alias) = alias else {
        state.track_done = true;
        return;
    };
    if bank.is_some_and(|b| b.has(&alias)) {
        let carrier = commands.spawn((Name::new("ambient track"), AmbientTrack, Transform::default())).id();
        sfx.play_flat_on(alias, carrier);
        state.track_done = true;
    }
}

/// The map's ambient emitters: `maps/createfx/<map>_fx.gsc`'s
/// `createLoopSound()`s, each a loop at its `origin`, fading with distance
/// as its alias says. Started once their aliases have loaded.
fn emitters(
    mut commands: Commands,
    mut sfx: ResMut<Sfx>,
    content: Res<Content>,
    map: Res<crate::world::MapName>,
    bank: Option<Res<Bank>>,
    mut state: ResMut<Ambience>,
) {
    let list = state.emitters.get_or_insert_with(|| {
        let script = format!("maps/createfx/{}_fx.gsc", map.0);
        let text = content.zones.iter().flat_map(|z| &z.assets).find_map(|a| match a {
            iw3::zone::Asset::RawFile(r) if r.name.eq_ignore_ascii_case(&script) => Some(String::from_utf8_lossy(&r.data).into_owned()),
            _ => None,
        });
        let list = text.map(|t| loop_sounds(&t)).unwrap_or_default();
        if !list.is_empty() {
            info!("audio: {} ambient emitters", list.len());
        }
        list
    });
    let Some(bank) = bank else { return };
    list.retain(|(origin, alias)| {
        if !bank.has(alias) {
            // Not loaded yet (localized_common_mp's come in the background).
            return true;
        }
        // In the showcase's storm the rain is one bed round the listener
        // (`crate::weather`), not the map's dozens of rain loops: right by
        // one, its steady noise sounded like hissing gas.
        if crate::weather::showcase() && alias.to_ascii_lowercase().starts_with("emt_rain_") {
            return false;
        }
        let at = crate::units::pos(*origin);
        let e = commands.spawn((Name::new(format!("emitter {alias}")), Transform::from_translation(at), GlobalTransform::from_translation(at))).id();
        sfx.play_on(alias.clone(), e);
        false
    });
}

/// The loop sounds of a createfx script's entities: its `createLoopSound()`s
/// and any effect with a `soundalias` (`CG_AddClientEntSound`): origin and
/// alias.
fn loop_sounds(script: &str) -> Vec<([f32; 3], String)> {
    script
        .split("ent = ")
        .skip(1)
        .filter_map(|block| {
            let origin = block.split("\"origin\" ] = (").nth(1)?.split(')').next()?;
            let v: Vec<f32> = origin.split(',').filter_map(|c| c.trim().parse().ok()).collect();
            let alias = block.split("\"soundalias\" ] = \"").nth(1)?.split('"').next()?;
            (v.len() == 3 && !alias.is_empty()).then(|| ([v[0], v[1], v[2]], alias.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod emitter_tests {
    #[test]
    fn reads_loop_sounds() {
        let s = r#"
     	ent = maps\mp\_createfx::createLoopSound();
     	ent.v[ "origin" ] = ( 4166.18, -42.8449, 277.479 );
     	ent.v[ "angles" ] = ( 270, 0, 0 );
     	ent.v[ "soundalias" ] = "emt_tree_palm_rustle";

     	ent = maps\mp\_utility::createOneshotEffect( "smoke" );
     	ent.v[ "origin" ] = ( 1, 2, 3 );
"#;
        assert_eq!(super::loop_sounds(s), vec![([4166.18, -42.8449, 277.479], "emt_tree_palm_rustle".to_owned())]);
    }
}
