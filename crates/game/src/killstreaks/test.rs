//! Debug aid: with `COD4RW_STREAKTEST=<dir>`, the player is credited with
//! kills (real [`Killed`] messages, so the streak counts as in play) and
//! uses each hardpoint, with screenshots: the UAV earned and its radar
//! sweeps on the compass; the airstrike's map, then the strike (called on
//! the ground ahead, as a click on the map would) as its bombs burst and
//! land; the helicopter coming in and fighting, watched from behind it,
//! then shot up by an enemy's rockets until it crashes (or, with
//! `COD4RW_STREAKTEST_LEAVE=1`, left to circle the map and leave). Then it
//! exits. Run with `COD4RW_DUMMY=1` for enemies, outdoors.

use super::HardpointInput;
use crate::movement::ViewAngles;
use avian3d::prelude::SpatialQuery;
use crate::combat::{Damage, HitLocation, Killed, Pawn};
use crate::player::LocalPlayer;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

pub(super) fn register(app: &mut App) {
    if let Ok(dir) = std::env::var("COD4RW_STREAKTEST") {
        app.insert_resource(StreakTest(dir.into())).add_systems(Update, drive.run_if(crate::state::in_game));
    }
}

#[derive(Resource)]
struct StreakTest(std::path::PathBuf);

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
    Exit,
}

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
    mut follow: Local<bool>,
    mut firing_shot: Local<Option<&'static str>>,
    mut damage: MessageWriter<Damage>,
    feed: Res<crate::combat::KillFeed>,
) {
    let t = time.elapsed_secs();
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
    while let Some((at, act)) = SCRIPT.get(*step).filter(|(at, _)| t >= *at) {
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
