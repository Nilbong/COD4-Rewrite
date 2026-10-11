//! Debug aid: with `COD4RW_CRATETEST=<dir>`, care package crates are
//! dropped from high up on spots round where the player starts (on whatever
//! is there: slopes, props, rubble, roofs), one at a time. For each, once
//! it's down: a screenshot; the player runs into it and stands against it
//! (a crate on the ground must never hurt anyone); then stands by it, looking
//! ahead rather than at it, holding Use, with screenshots of the prompt and
//! after. The results are logged (`crate test:`), then it exits.

use super::carepackage::{CRUSH, CarePackage, UseCrate, drop_crate};
use super::models::StreakModels;
use crate::combat::{Damage, Dead, Health, Pawn};
use crate::movement::{Mover, ViewAngles};
use crate::player::LocalPlayer;
use crate::units::u;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

pub(super) fn register(app: &mut App) {
    if let Ok(dir) = std::env::var("COD4RW_CRATETEST") {
        app.insert_resource(CrateTest { dir: dir.into(), ..default() }).add_systems(Update, drive.run_if(crate::state::in_game));
    }
}

#[derive(Resource, Default)]
struct CrateTest {
    dir: std::path::PathBuf,
    origin: Option<Vec3>,
    spot: usize,
    since: f32,
    stage: u8,
    crate_at: Option<(Entity, Vec3)>,
    crushed: u32,
    results: Vec<String>,
}

/// Where crates come down, CoD units from the start (flat).
const SPOTS: [(f32, f32); 6] = [(0.0, 0.0), (260.0, 0.0), (0.0, 260.0), (-260.0, 0.0), (0.0, -260.0), (380.0, 380.0)];
/// Each spot's steps (s): drop, landed, run into it, stand by it holding
/// Use, after, next.
const LANDED: f32 = 5.0;
const RUN: (f32, f32) = (5.5, 7.0);
const USE: f32 = 7.5;
const PROMPT: f32 = 7.9;
const AFTER: f32 = 10.5;
const NEXT: f32 = 11.5;

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn drive(
    mut commands: Commands,
    time: Res<Time>,
    mut test: ResMut<CrateTest>,
    mut player: Query<(Entity, &Pawn, &mut Transform, &mut Mover, &mut ViewAngles, &mut Health, Has<Dead>), With<LocalPlayer>>,
    crates: Query<(&CarePackage, &GlobalTransform)>,
    mut models: StreakModels,
    mut damage: MessageReader<Damage>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.elapsed_secs();
    let Ok((me, pawn, mut feet, mut mover, mut view, mut health, dead)) = player.single_mut() else { return };
    // Kept alive against the bots (a crate's crush is counted, not felt).
    health.current = health.current.max(5000.0);
    for d in damage.read() {
        if d.target == me && d.weapon == CRUSH {
            test.crushed += 1;
            warn!("crate test: the player was crushed at stage {}", test.stage);
        }
    }
    if dead || now < 6.0 {
        return;
    }
    let origin = *test.origin.get_or_insert(feet.translation);
    let Some(&(dx, dz)) = SPOTS.get(test.spot) else {
        for r in &test.results {
            info!("crate test: {r}");
        }
        info!("crate test: crushed {} times", test.crushed);
        exit.write(AppExit::Success);
        return;
    };
    let spot = origin + crate::units::dir([dx, dz, 0.0]) * crate::units::INCH;
    let dir = test.dir.clone();
    let shot = |commands: &mut Commands, name: String| {
        std::fs::create_dir_all(&dir).ok();
        info!("crate test: shot {name}");
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.join(format!("{name}.png"))));
    };
    let look = |view: &mut ViewAngles, from: Vec3, at: Vec3| {
        let d = (at - from).normalize_or_zero();
        view.yaw = (-d.x).atan2(-d.z);
        view.pitch = d.y.asin();
    };
    let t = now - test.since;
    let i = test.spot;
    let found = test.crate_at.and_then(|(e, _)| crates.get(e).ok());
    match test.stage {
        // Drop one from high up, and watch from off to the side.
        0 => {
            let e = drop_crate(&mut commands, &mut models, me, pawn.team, spot + Vec3::Y * u(500.0));
            test.crate_at = Some((e, spot));
            test.since = now;
            test.stage = 1;
            // Off to its side, out from under it.
            let side = Quat::from_rotation_y(i as f32 * 1.1) * Vec3::X;
            feet.translation = spot + side * u(220.0);
            mover.velocity = Vec3::ZERO;
        }
        1 if t >= LANDED => {
            let (landed, at) = found.map_or((false, spot), |(c, g)| (c.landed(), g.translation()));
            test.crate_at = test.crate_at.map(|(e, _)| (e, at));
            info!("crate test: crate {i} landed {landed} at CoD {:?}", crate::units::to_cod(at).map(f32::round));
            look(&mut view, mover.eye(feet.translation), at);
            shot(&mut commands, format!("crate{i}_landed"));
            test.stage = 2;
        }
        // Run into it, straight at its middle.
        2 if t >= RUN.0 => {
            if let Some((_, at)) = test.crate_at {
                let away = (feet.translation - at).with_y(0.0).normalize_or(Vec3::X);
                feet.translation = at.with_y(at.y - u(12.0)) + away * u(140.0);
                look(&mut view, mover.eye(feet.translation), at);
            }
            test.stage = 3;
        }
        3 if t < RUN.1 => {
            if let Some((_, at)) = test.crate_at {
                let to = (at - feet.translation).with_y(0.0).normalize_or_zero();
                mover.velocity = to * u(260.0) + Vec3::Y * mover.velocity.y;
            }
        }
        3 => test.stage = 4,
        // Stand by it, looking ahead (not down at it), holding Use.
        4 if t >= USE => {
            if let Some((e, at)) = test.crate_at {
                let side = crates.get(e).map_or(u(30.0), |(c, _)| c.half_extent());
                let away = (feet.translation - at).with_y(0.0).normalize_or(Vec3::X);
                feet.translation = at.with_y(at.y - u(12.0)) + away * (side + u(26.0));
                mover.velocity = Vec3::ZERO;
                let eye = mover.eye(feet.translation);
                look(&mut view, eye, at.with_y(eye.y));
            }
            commands.entity(me).insert(UseCrate(true));
            test.stage = 5;
        }
        5 if t >= PROMPT => {
            shot(&mut commands, format!("crate{i}_prompt"));
            test.stage = 6;
        }
        6 if t >= AFTER => {
            let opened = found.is_none();
            commands.entity(me).insert(UseCrate(false));
            let line = format!(
                "crate {i}: landed at CoD {:?}, opened {opened}, crushed so far {}",
                test.crate_at.map(|(_, a)| crate::units::to_cod(a).map(f32::round)),
                test.crushed
            );
            test.results.push(line);
            shot(&mut commands, format!("crate{i}_after"));
            test.stage = 7;
        }
        7 if t >= NEXT => {
            if let Some((e, _)) = test.crate_at.take() {
                commands.entity(e).try_despawn();
            }
            test.spot += 1;
            test.stage = 0;
        }
        _ => {}
    }
}
