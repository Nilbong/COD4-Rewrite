//! Debug aid: with `COD4RW_STREAKTEST=<dir>`, the player is credited with
//! kills (real [`Killed`] messages, so the streak counts as in play) and
//! uses each hardpoint, with screenshots: the UAV earned and its radar
//! sweeps on the compass; the airstrike's map, then the strike (called on
//! the ground ahead, as a click on the map would) as its bombs burst and
//! land; the helicopter coming in and fighting, watched from behind it,
//! then shot up by an enemy's rockets until it crashes (or, with
//! `COD4RW_STREAKTEST_LEAVE=1`, left to circle the map and leave). Then it
//! exits. Run with `COD4RW_DUMMY=1` for enemies, outdoors.
//!
//! With `COD4RW_STREAKTEST_MW2=1`, MW2's two instead: four kills, the care
//! package's marker thrown ahead, the Little Bird coming in and dropping the
//! crate, the crate opened; two more kills, the sentry carried and planted,
//! an enemy put in front of it to shoot, then the sentry shot up.

use super::HardpointInput;
use crate::movement::ViewAngles;
use avian3d::prelude::SpatialQuery;
use crate::combat::{Damage, HitLocation, Killed, Pawn};
use crate::player::LocalPlayer;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

pub(super) fn register(app: &mut App) {
    if let Ok(dir) = std::env::var("COD4RW_STREAKTEST") {
        app.insert_resource(StreakTest(dir.into()))
            .init_resource::<Clicking>()
            .add_systems(Update, drive.run_if(crate::state::in_game))
            .add_systems(PreUpdate, click.after(bevy::input::InputSystems));
    }
}

#[derive(Resource)]
struct StreakTest(std::path::PathBuf);

/// Clicking the map as a player would: a fresh press each frame (after the
/// frame's input is read, so the HUD's pick sees it) until the map closes.
#[derive(Resource, Default)]
struct Clicking(bool);

fn click(mut clicking: ResMut<Clicking>, mut mouse: ResMut<ButtonInput<MouseButton>>, selecting: Option<Res<super::airstrike::Selecting>>) {
    if clicking.0 {
        mouse.release(MouseButton::Left);
        if selecting.is_some() {
            mouse.press(MouseButton::Left);
        } else {
            clicking.0 = false;
        }
    }
}

/// What happens when: credit kills, press 5, or take a screenshot.
enum Act {
    Kills(u32),
    Use,
    /// Use it on the ground this far ahead (units), looking there.
    UseAt(f32),
    /// Put the mouse on the map, a little off its centre.
    Mouse,
    /// Keep behind the helicopter (held there in the air), looking at it.
    Follow,
    /// An enemy's rocket hits the helicopter for this much.
    HitHeli(f32),
    /// A screenshot once the helicopter's turret is firing (if it does
    /// before the next step).
    ShotFiring(&'static str),
    Shot(&'static str),
    /// Call in this one, looking a little down ahead.
    UseItem(super::Hardpoint),
    /// Keep looking at the care package's helicopter, then its crate.
    WatchCourier,
    WatchCrate,
    /// Stand at the crate looking at it, and hold Use on it (or let go).
    ToCrate,
    HoldUse(bool),
    /// Click the map where the mouse is (until the map closes).
    ClickMap,
    /// Plant the carried sentry.
    Place,
    /// Put an enemy this far in front of the sentry, and watch from beside
    /// it.
    EnemyBeforeSentry(f32),
    /// A screenshot once the sentry fires (if it does before the next step).
    ShotSentryFiring(&'static str),
    /// An enemy's rocket hits the sentry for this much.
    HitSentry(f32),
    Exit,
}

const MW2_SCRIPT: &[(f32, Act)] = &[
    (5.0, Act::Kills(4)),
    (5.6, Act::Shot("cp_earned")),
    (6.5, Act::UseItem(super::Hardpoint::CarePackage)),
    (6.9, Act::Shot("cp_throw")),
    (9.0, Act::Shot("cp_marker")),
    (10.0, Act::WatchCourier),
    (12.0, Act::Shot("cp_inbound")),
    (14.6, Act::Shot("cp_hover")),
    (15.6, Act::WatchCrate),
    (15.8, Act::Shot("cp_falling")),
    (19.0, Act::Shot("cp_landed")),
    (19.5, Act::ToCrate),
    (19.8, Act::HoldUse(true)),
    (20.0, Act::Shot("cp_opening")),
    (21.0, Act::Shot("cp_opened")),
    (21.2, Act::HoldUse(false)),
    (22.0, Act::Kills(2)),
    (22.6, Act::Shot("sentry_earned")),
    (23.5, Act::UseItem(super::Hardpoint::Sentry)),
    (24.2, Act::Shot("sentry_carry")),
    (25.0, Act::Place),
    (25.6, Act::Shot("sentry_planted")),
    (26.5, Act::EnemyBeforeSentry(400.0)),
    (26.6, Act::ShotSentryFiring("sentry_firing")),
    (29.0, Act::Shot("sentry_after_fire")),
    (30.0, Act::HitSentry(1200.0)),
    (30.3, Act::Shot("sentry_destroyed")),
    (32.5, Act::Shot("sentry_wreck")),
    (34.0, Act::Exit),
];

const SCRIPT: &[(f32, Act)] = &[
    (5.0, Act::Kills(3)),
    (5.6, Act::Shot("uav_earned")),
    (7.0, Act::Use),
    (7.4, Act::Shot("uav_called")),
    (9.5, Act::Shot("uav_sweep")),
    (11.0, Act::Kills(2)),
    (11.6, Act::Shot("airstrike_earned")),
    (12.5, Act::Use),
    (12.8, Act::Mouse),
    (13.0, Act::Shot("airstrike_map")),
    (13.5, Act::UseAt(1200.0)),
    (19.3, Act::Shot("airstrike_burst")),
    (19.9, Act::Shot("airstrike_bomblets")),
    (21.0, Act::Shot("airstrike_after")),
    (22.5, Act::Kills(2)),
    (23.1, Act::Shot("heli_earned")),
    (23.5, Act::Use),
    (23.6, Act::Follow),
    (24.0, Act::ShotFiring("heli_firing")),
    (27.0, Act::Shot("heli_inbound")),
    (31.0, Act::Shot("heli_2")),
    (35.0, Act::Shot("heli_3")),
    (39.0, Act::Shot("heli_4")),
    (42.0, Act::HitHeli(600.0)),
    (44.0, Act::Shot("heli_smoking")),
    (46.0, Act::HitHeli(600.0)),
    (46.4, Act::Shot("heli_hit")),
    (48.0, Act::Shot("heli_crashing")),
    (50.5, Act::Shot("heli_crash")),
    (53.0, Act::Shot("heli_after")),
    (54.0, Act::Exit),
];

fn drive(
    mut commands: Commands,
    time: Res<Time>,
    test: Res<StreakTest>,
    mut step: Local<usize>,
    mut look_at: Local<Option<Vec3>>,
    mut player: Query<
        (Entity, &Pawn, &mut HardpointInput, &mut Transform, &mut ViewAngles, &mut crate::movement::Mover),
        (With<LocalPlayer>, Without<super::helicopter::Helicopter>),
    >,
    spatial: SpatialQuery,
    pawns: Query<(Entity, &Pawn)>,
    mut killed: MessageWriter<Killed>,
    mut exit: MessageWriter<AppExit>,
    mut window: Single<&mut Window, With<bevy::window::PrimaryWindow>>,
    helis: Query<(Entity, &Transform, &super::helicopter::Helicopter)>,
    (mut follow, mut firing_shot): (Local<bool>, Local<Option<&'static str>>),
    mut damage: MessageWriter<Damage>,
    feed: Res<crate::combat::KillFeed>,
    mw2: (
        Query<&GlobalTransform, With<super::carepackage::Courier>>,
        Query<(Entity, &GlobalTransform, &super::carepackage::CarePackage)>,
        Query<(Entity, &Transform, &super::sentry::Sentry), Without<LocalPlayer>>,
        Query<&mut Transform, (Without<LocalPlayer>, Without<super::sentry::Sentry>, Without<super::helicopter::Helicopter>, With<Pawn>)>,
        Local<u8>,
        Local<Option<&'static str>>,
        Option<Res<crate::mw2guns::MatchContent>>,
        Local<Option<f32>>,
        Local<Option<(Entity, Vec3, f32)>>,
        ResMut<Clicking>,
    ),
) {
    let (couriers, crates, sentries, mut others, mut watching, mut sentry_shot, mw2_content, mut started, mut pinned, mut clicking) = mw2;
    let mw2_test = std::env::var_os("COD4RW_STREAKTEST_MW2").is_some();
    let script = if mw2_test { MW2_SCRIPT } else { SCRIPT };
    let t = time.elapsed_secs();
    // MW2's script waits for MW2's models to load (or a minute).
    let t = if mw2_test {
        let ready = mw2_content.as_ref().is_none_or(|c| !c.busy()) || t > 60.0;
        match *started {
            Some(at) => t - at + 4.0,
            None if ready => {
                *started = Some(t);
                4.0
            }
            None => return,
        }
    } else {
        t
    };
    let Ok((me, mine, mut input, mut feet, mut view, mut mover)) = player.single_mut() else { return };
    if let Some((_, h, _)) = helis.iter().next().filter(|_| *follow) {
        let back = (h.rotation * Vec3::X).with_y(0.0).normalize_or_zero();
        feet.translation = h.translation - back * crate::units::u(1100.0) + Vec3::Y * crate::units::u(60.0);
        mover.velocity = Vec3::ZERO;
        *look_at = Some(h.translation - Vec3::Y * crate::units::u(250.0));
    }
    let eye = feet.translation + Vec3::Y * crate::units::u(60.0);
    if let Some(name) = firing_shot.filter(|_| helis.iter().any(|(_, _, h)| h.firing())) {
        *firing_shot = None;
        info!("streak test: {name} at {t:.1} s");
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.0.join(format!("{name}.png"))));
    }
    if let Some(at) = *look_at {
        let d = (at - eye).normalize_or_zero();
        view.yaw = (-d.x).atan2(-d.z);
        view.pitch = d.y.asin();
    }
    // An enemy held in front of the sentry a while.
    if let Some((enemy, spot, until)) = *pinned {
        match others.get_mut(enemy) {
            Ok(mut etf) if t < until => etf.translation = spot,
            _ => *pinned = None,
        }
    }
    // Watching the care package come in and fall.
    match *watching {
        1 => *look_at = couriers.iter().next().map(|g| g.translation()).or(*look_at),
        2 => *look_at = crates.iter().next().map(|(_, g, _)| g.translation()).or(*look_at),
        _ => {}
    }
    if let Some(name) = sentry_shot.filter(|_| sentries.iter().any(|(_, _, s)| s.firing())) {
        *sentry_shot = None;
        info!("streak test: {name} at {t:.1} s");
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.0.join(format!("{name}.png"))));
    }
    while let Some((at, act)) = script.get(*step).filter(|(at, _)| t >= *at) {
        *step += 1;
        match act {
            Act::Kills(n) => {
                let enemies: Vec<Entity> = pawns.iter().filter(|(_, p)| p.team != mine.team).map(|(e, _)| e).collect();
                for k in 0..*n as usize {
                    if let Some(&victim) = enemies.get(k % enemies.len().max(1)) {
                        killed.write(Killed { victim, attacker: Some(me), weapon: "", location: HitLocation::Torso });
                    }
                }
            }
            Act::Use => input.use_now = true,
            Act::UseItem(item) => {
                input.use_now = true;
                input.item = Some(*item);
                let ahead = (view.rotation() * Vec3::NEG_Z).with_y(0.0).normalize_or_zero();
                // (The marker thrown up and out, the sentry looked down at.)
                *look_at = Some(eye + ahead * crate::units::u(300.0) + Vec3::Y * crate::units::u(if *item == super::Hardpoint::Sentry { -140.0 } else { 90.0 }));
            }
            Act::WatchCourier => *watching = 1,
            Act::WatchCrate => *watching = 2,
            Act::ToCrate => {
                *watching = 0;
                if let Some((_, g, _)) = crates.iter().find(|c| c.2.landed()) {
                    let c = g.translation();
                    let away = (feet.translation - c).with_y(0.0).normalize_or(Vec3::X);
                    feet.translation = c.with_y(feet.translation.y.max(c.y - crate::units::u(10.0))) + away * crate::units::u(60.0);
                    mover.velocity = Vec3::ZERO;
                    *look_at = Some(c);
                }
            }
            Act::HoldUse(on) => {
                commands.entity(me).insert(super::carepackage::UseCrate(*on));
            }
            // `COD4RW_STREAKTEST_CLICK=1`: click the map instead, as a
            // player does.
            Act::ClickMap => clicking.0 = true,
            Act::UseAt(_) if std::env::var_os("COD4RW_STREAKTEST_CLICK").is_some() => clicking.0 = true,
            Act::Place => {
                commands.entity(me).insert(super::sentry::PlaceNow);
            }
            Act::EnemyBeforeSentry(dist) => {
                let enemies = || pawns.iter().filter(|(_, p)| p.team != mine.team);
                let enemy = enemies().find(|(_, p)| p.name.contains("dummy")).or_else(|| enemies().next()).map(|(e, _)| e);
                if let (Some((_, stf, _)), Some(enemy)) = (sentries.iter().next(), enemy) {
                    let forward = stf.rotation * Vec3::X;
                    let side = forward.cross(Vec3::Y).normalize_or_zero();
                    // The nearest-to-ahead spot in its arc it can see, on the ground.
                    let pivot = stf.translation + Vec3::Y * crate::units::u(48.0);
                    let filter = crate::collision::sight_filter();
                    let open = |to: Vec3| {
                        let d = to - pivot;
                        Dir3::new(d).ok().is_some_and(|dir| spatial.cast_ray(pivot, dir, d.length(), true, &filter).is_none())
                    };
                    let spot = [0.0f32, 15.0, -15.0, 30.0, -30.0, 45.0, -45.0]
                        .into_iter()
                        .flat_map(|a| [*dist, *dist * 0.6, *dist * 0.35].map(move |r| (a, r)))
                        .find_map(|(a, r)| {
                            let dir = Quat::from_rotation_y(a.to_radians()) * forward;
                            let top = stf.translation + dir * crate::units::u(r) + Vec3::Y * crate::units::u(60.0);
                            let down = spatial.cast_ray(top, Dir3::NEG_Y, crate::units::u(200.0), true, &filter)?;
                            let ground = top - Vec3::Y * down.distance;
                            open(ground + Vec3::Y * crate::units::u(40.0)).then_some(ground)
                        })
                        .unwrap_or(stf.translation + forward * crate::units::u(*dist));
                    *pinned = Some((enemy, spot, t + 2.5));
                    feet.translation = stf.translation - forward * crate::units::u(90.0) + side * crate::units::u(70.0);
                    mover.velocity = Vec3::ZERO;
                    *look_at = Some(spot + Vec3::Y * crate::units::u(40.0));
                }
            }
            Act::ShotSentryFiring(name) => *sentry_shot = Some(name),
            Act::HitSentry(amount) => {
                let enemy = pawns.iter().find(|(_, p)| p.team != mine.team).map(|(e, _)| e);
                if let (Some((sentry, stf, _)), Some(enemy)) = (sentries.iter().next(), enemy) {
                    damage.write(Damage { target: sentry, attacker: Some(enemy), amount: *amount, location: HitLocation::Torso, weapon: "RPG-7" });
                    *look_at = Some(stf.translation + Vec3::Y * crate::units::u(30.0));
                }
            }
            Act::Follow => *follow = true,
            Act::ShotFiring(name) => *firing_shot = Some(name),
            Act::HitHeli(_) if std::env::var_os("COD4RW_STREAKTEST_LEAVE").is_some() => {}
            Act::HitHeli(amount) => {
                let enemy = pawns.iter().find(|(_, p)| p.team != mine.team).map(|(e, _)| e);
                if let (Some((heli, ..)), Some(enemy)) = (helis.iter().next(), enemy) {
                    damage.write(Damage { target: heli, attacker: Some(enemy), amount: *amount, location: HitLocation::Torso, weapon: "RPG-7" });
                }
            }
            Act::Mouse => {
                let centre = Vec2::new(window.width(), window.height()) / 2.0;
                window.set_cursor_position(Some(centre + Vec2::new(40.0, 25.0)));
            }
            Act::UseAt(dist) => {
                commands.remove_resource::<super::airstrike::Selecting>();
                let ahead = (view.rotation() * Vec3::NEG_Z).with_y(0.0).normalize_or_zero();
                let above = eye + ahead * crate::units::u(*dist) + Vec3::Y * crate::units::u(2000.0);
                let filter = crate::collision::sight_filter();
                if let Some(hit) = spatial.cast_ray(above, Dir3::NEG_Y, crate::units::u(8000.0), true, &filter) {
                    let ground = above - Vec3::Y * hit.distance;
                    input.use_now = true;
                    input.item = Some(super::Hardpoint::Airstrike);
                    input.target = Some(ground);
                    *look_at = Some(ground + Vec3::Y * crate::units::u(250.0));
                }
            }
            Act::Shot(name) => {
                std::fs::create_dir_all(&test.0).ok();
                info!("streak test: {name} at {at} s");
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.0.join(format!("{name}.png"))));
            }
            // Leaving it be: until it's gone (or two minutes in).
            Act::Exit if std::env::var_os("COD4RW_STREAKTEST_LEAVE").is_some() && !helis.is_empty() && t < 120.0 => {
                *step -= 1;
                break;
            }
            Act::Exit => {
                for e in &feed.entries {
                    info!("streak test: feed: {}", e.text);
                }
                exit.write(AppExit::Success);
            }
        }
    }
}
