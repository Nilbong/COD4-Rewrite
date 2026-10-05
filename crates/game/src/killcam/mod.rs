//! CoD4's killcam: killed, the player sees the last moments again through
//! the killer's eyes (or riding the grenade or projectile that did it)
//! before respawning; F skips it. At the end of a match, everyone sees the
//! final kill again ("FINAL KILLCAM"). Hardcore has neither
//! (`scr_game_allowkillcam 0`).
//!
//! [`record`] keeps the last few seconds of every pawn, projectile, shot and
//! explosion, read from the components and messages gameplay already has;
//! [`replay`] poses the live bodies as they were (the game carries on
//! underneath, out of sight), puts the camera where the killer's eyes were,
//! gives it the killer's gun, and plays the shots and explosions again.

pub mod record;
pub mod replay;

use crate::combat::{Dead, Killed, Pawn, Team};
use crate::player::LocalPlayer;
use bevy::prelude::*;
use record::{History, Kill};
use replay::Replay;

pub struct KillcamPlugin;

impl Plugin for KillcamPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<History>()
            .init_resource::<Killcam>()
            .init_resource::<ReplayPoses>()
            .add_systems(OnEnter(crate::state::GameState::InGame), reset)
            .add_systems(Update, (flow, replay::play_events).chain().after(crate::weapons::WeaponSet).run_if(crate::state::in_game))
            .add_systems(
                PostUpdate,
                (
                    (record::record, replay::pose_anims).chain().before(crate::models::animate_skeletons),
                    replay::place_camera
                        .after(crate::player::follow_camera)
                        .after(crate::bodycam::pose_view)
                        .before(TransformSystems::Propagate),
                    replay::place_bodies.after(TransformSystems::Propagate),
                )
                    .run_if(crate::state::in_game),
            );
        if let Ok(dir) = std::env::var("COD4RW_KILLCAMTEST") {
            app.insert_resource(TestDir(dir.into())).add_systems(Update, test.before(flow).run_if(crate::state::in_game)).add_systems(
                Update,
                test_moves.after(crate::player::InputSet).before(crate::movement::MovementSet).run_if(crate::state::in_game),
            );
        }
    }
}

/// Bodies being replayed, by their body entity: the view's yaw and pitch,
/// the body's twist and whether it was dead, as they were, for
/// [`crate::thirdperson`] to turn the spine by (rather than by the live
/// game's view). Empty outside a killcam.
#[derive(Resource, Default)]
pub struct ReplayPoses(pub std::collections::HashMap<Entity, (f32, f32, f32, bool)>);

/// The killcam playing (or about to).
#[derive(Resource, Default)]
pub struct Killcam {
    pub replay: Option<Replay>,
    /// A kill to play back from then (victim, attacker, when): its title
    /// and whether it can be skipped. Looked up in the history then, once
    /// it has been recorded.
    pending: Option<(f32, (Entity, Entity, f32), &'static str, bool)>,
    /// The match's final killcam has been played.
    final_shown: bool,
}

/// What's shown over a killcam (see [`crate::ui`]'s HUD).
pub struct Banner<'a> {
    pub title: &'a str,
    pub killer: &'a str,
    pub victim: &'a str,
    pub weapon: &'a str,
    pub headshot: bool,
    pub skippable: bool,
}

impl Killcam {
    /// Is a killcam playing?
    pub fn showing(&self) -> bool {
        self.replay.is_some()
    }

    pub fn banner(&self) -> Option<Banner<'_>> {
        let r = self.replay.as_ref()?;
        Some(Banner {
            title: r.title,
            killer: &r.killer_name,
            victim: &r.victim_name,
            weapon: r.kill.weapon,
            headshot: r.kill.headshot,
            skippable: r.skippable,
        })
    }
}

/// Marks what the killcam spawned (its viewmodel, the past's grenades).
#[derive(Component)]
pub struct Ghost;

fn reset(mut killcam: ResMut<Killcam>, mut history: ResMut<History>) {
    *killcam = Killcam::default();
    *history = History::default();
}

/// Start, skip and end killcams: the player's own after dying (holding the
/// respawn until it's over), and the final one when the match ends.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn flow(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    pad: Option<Res<crate::gamepad::PadFrame>>,
    mut pad_was: Local<bool>,
    mut killcam: ResMut<Killcam>,
    history: Res<History>,
    state: Option<Res<crate::tdm::MatchState>>,
    mut killed: MessageReader<Killed>,
    mut pawns: Query<(Entity, &Pawn, Has<LocalPlayer>, Option<&mut Dead>)>,
    ghosts: Query<Entity, (With<Ghost>, Without<ChildOf>)>,
    mut visibility: Query<&mut Visibility>,
) {
    // [Use] on a pad is X / Square (CoD4's console layout); read every
    // frame, so one held as the killcam starts doesn't skip it.
    let pad_use = pad.as_ref().is_some_and(|p| p.interact);
    let pad_pressed = pad_use && !*pad_was;
    *pad_was = pad_use;
    // None in Hardcore, nor in splitscreen (the views are the players').
    if crate::tdm::hardcore() || crate::splitscreen::active() {
        killed.clear();
        return;
    }
    let now = time.elapsed_secs();
    let me = pawns.iter().find(|p| p.2).map(|p| p.0);
    let ended = state.as_ref().and_then(|s| s.ended);
    // Killed by someone: the killcam, once the death has sunk in.
    for k in killed.read() {
        let Some(attacker) = k.attacker.filter(|&a| Some(k.victim) == me && a != k.victim) else { continue };
        if ended.is_some() {
            continue;
        }
        killcam.pending = Some((now + replay::DELAY, (k.victim, attacker, now), "KILLCAM", true));
        // No respawning until it's over.
        if let Ok((_, _, _, Some(mut dead))) = pawns.get_mut(k.victim) {
            dead.respawn_at = dead.respawn_at.max(now + replay::DELAY + replay::BEFORE + replay::AFTER + 0.25);
        }
    }
    // The match is over: its last kill again, for everyone.
    if let (Some((_, at)), false) = (ended, killcam.final_shown) {
        killcam.final_shown = true;
        if let Some(kill) = history.kills.iter().rev().find(|x| x.time <= at + 0.1).cloned() {
            let start = (kill.time + replay::AFTER).max(now + 0.5);
            killcam.pending = Some((start, (kill.victim, kill.attacker, kill.time), "FINAL KILLCAM", false));
            stop(&mut commands, &mut killcam, &ghosts, &mut visibility);
        }
    }
    if ended.is_none() {
        killcam.final_shown = false;
    }
    // Due: play it.
    if killcam.pending.as_ref().is_some_and(|p| now >= p.0) {
        let (_, (victim, attacker, at), title, skippable) = killcam.pending.take().expect("pending");
        let found = history.kills.iter().rev().find(|x| x.victim == victim && x.attacker == attacker && (x.time - at).abs() < 0.5);
        let Some(kill) = found.cloned() else {
            warn!("killcam: the kill isn't in the history");
            return;
        };
        let name = |e: Entity| pawns.get(e).map_or_else(|_| "?".to_owned(), |p| p.1.name.clone());
        let (killer, victim) = (name(kill.attacker), name(kill.victim));
        let team = pawns.get(kill.attacker).map_or(Team::Allies, |p| p.1.team);
        let before = if skippable { replay::BEFORE } else { replay::FINAL_BEFORE };
        let start = history.frames.front().map_or(kill.time, |f| f.time).max(kill.time - before);
        let end = (kill.time + replay::AFTER).min(now);
        let mut r = Replay::new(title, kill.clone(), (killer, victim), team, (start, end), now, skippable);
        // An explosive kill rides the explosive.
        r.projectile = explosive(&history, &kill);
        info!("killcam: {} killed {} with {} ({:.1}s){}", r.killer_name, r.victim_name, kill.weapon, end - start, if r.projectile.is_some() { ", riding the explosive" } else { "" });
        killcam.replay = Some(r);
    }
    // Over, or skipped (F, as CoD4's [Use]).
    let Some(r) = &killcam.replay else { return };
    let skipped = r.skippable && (keys.just_pressed(KeyCode::KeyF) || pad_pressed);
    if r.done(now) || skipped {
        if skipped {
            if let Some(Ok((_, _, _, Some(mut dead)))) = me.map(|e| pawns.get_mut(e)) {
                dead.respawn_at = now;
            }
        }
        stop(&mut commands, &mut killcam, &ghosts, &mut visibility);
    }
}

/// The explosive behind an explosive kill: the attacker's projectile that
/// went off nearest the victim's death.
fn explosive(history: &History, kill: &Kill) -> Option<Entity> {
    let blast = history.blasts.iter().rev().find(|(t, b)| b.owner == kill.attacker && *t <= kill.time + 0.1 && *t >= kill.time - 1.0)?;
    let (blast_time, blast) = (blast.0, &blast.1);
    history
        .frames
        .iter()
        .rev()
        .filter(|f| f.time <= blast_time + 0.1)
        .take(4)
        .flat_map(|f| f.projectiles.iter())
        .filter(|p| p.owner == kill.attacker)
        .min_by(|a, b| a.at.distance(blast.at).total_cmp(&b.at.distance(blast.at)))
        .filter(|p| p.at.distance(blast.at) < crate::units::u(96.0))
        .map(|p| p.entity)
}

/// End a killcam: its stand-ins gone and what it hid shown (the live
/// bodies' poses and places come back by themselves next frame).
fn stop(
    commands: &mut Commands,
    killcam: &mut Killcam,
    ghosts: &Query<Entity, (With<Ghost>, Without<ChildOf>)>,
    visibility: &mut Query<&mut Visibility>,
) {
    let Some(r) = killcam.replay.take() else { return };
    for e in ghosts {
        commands.entity(e).despawn();
    }
    if let Some((vm, _)) = r.viewmodel_entity() {
        commands.entity(vm).despawn();
    }
    for e in r.hidden_entities() {
        if let Ok(mut v) = visibility.get_mut(*e) {
            *v = Visibility::Inherited;
        }
    }
}

#[derive(Resource)]
struct TestDir(std::path::PathBuf);

/// Debug aid: with `COD4RW_KILLCAMTEST=<dir>`, an enemy bot "kills" the
/// player at 8 s (the last damage counted as its gun's), screenshots of
/// the killcam a second and three seconds in, then exit. With
/// `COD4RW_KILLCAMTEST_FINAL=1` the kill ends the match (the final killcam).
#[allow(clippy::type_complexity)]
/// The test's player runs, turns and jumps for the seconds before it's
/// killed, so its replayed body has something to show.
fn test_moves(time: Res<Time>, mut player: Query<(&mut crate::movement::MoveInput, &mut crate::movement::ViewAngles), With<LocalPlayer>>) {
    let t = time.elapsed_secs();
    let Ok((mut input, mut view)) = player.single_mut() else { return };
    if (3.0..8.0).contains(&t) {
        input.forward = 1.0;
        input.jump = (t % 1.5) < 0.1;
        view.yaw += time.delta_secs() * 0.7;
    }
}

fn test(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<TestDir>,
    killcam: Res<Killcam>,
    player: Query<(Entity, &Pawn), With<LocalPlayer>>,
    pawns: Query<(Entity, &Pawn, &crate::weapons::WeaponState), Without<LocalPlayer>>,
    mut damage: MessageWriter<crate::combat::Damage>,
    mut state: Option<ResMut<crate::tdm::MatchState>>,
    mut step: Local<usize>,
    mut started: Local<Option<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let t = time.elapsed_secs();
    let Ok((me, pawn)) = player.single() else { return };
    // `COD4RW_KILLCAMTEST_FINAL=1`: that kill ends the match.
    if *step == 1 && started.is_none() && std::env::var_os("COD4RW_KILLCAMTEST_FINAL").is_some() {
        if let Some(s) = state.as_mut().filter(|s| s.ended.is_none()) {
            s.ended = Some((None, t));
        }
    }
    if *step == 0 && t >= 8.0 {
        if let Some((enemy, _, w)) = pawns.iter().find(|(_, p, _)| crate::combat::hostile(p, pawn)) {
            damage.write(crate::combat::Damage {
                target: me,
                attacker: Some(enemy),
                amount: 500.0,
                location: crate::combat::HitLocation::Torso,
                weapon: w.def.display_name,
            });
            *step = 1;
        }
        return;
    }
    if killcam.showing() && started.is_none() {
        *started = Some(t);
    }
    let Some(s) = *started else { return };
    let mut shot = |name: &str, step: &mut usize| {
        std::fs::create_dir_all(&dir.0).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.0.join(format!("killcam_{step}_{name}.png"))));
        *step += 1;
    };
    match *step {
        1 if t >= s + 1.0 => shot("early", &mut step),
        2 if t >= s + 3.5 => shot("late", &mut step),
        3 if !killcam.showing() => shot("after", &mut step),
        4 if t >= s + 10.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
