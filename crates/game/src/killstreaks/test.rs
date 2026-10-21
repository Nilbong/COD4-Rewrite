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
//! Along the way it tries to fight with the gun out of hand: fire, a
//! frag and a knife held through the UAV's call-in (none may happen), a
//! dummy made to call one in with its trigger held (no shot until its
//! lock ends, then shots), and the trigger held again at the end (shots),
//! logged as `streak test: fighting ...`.
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
            .init_resource::<Nudging>()
            .init_resource::<Fighting>()
            .init_resource::<Clock>()
            .add_systems(Update, start_clock.before(drive).run_if(crate::state::in_game))
            .add_systems(Update, drive.run_if(crate::state::in_game))
            .add_systems(PreUpdate, (click, nudge, fight_keys).after(bevy::input::InputSystems))
            .add_systems(
                Update,
                (
                    dummy_fights.after(crate::player::InputSet).before(super::hands::CallInGate).run_if(crate::state::in_game),
                    count_fighting.after(crate::weapons::WeaponSet).run_if(crate::state::in_game),
                ),
            );
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

/// The script's clock: from the match's HUD being up (it comes a while
/// after the map, longer on a busy machine: counted from the start, the
/// script ran ahead of it, its shots of an unlit world and a blank
/// laptop), at 4 s, a second before the first step.
#[derive(Resource, Default)]
struct Clock(Option<f32>);

impl Clock {
    fn t(&self, time: &Time) -> Option<f32> {
        self.0.map(|at| time.elapsed_secs() - at + 4.0)
    }
}

fn start_clock(time: Res<Time>, mut clock: ResMut<Clock>, fe: Option<Res<crate::ui::Frontend>>, me: Query<(), With<LocalPlayer>>) {
    if clock.0.is_none() && fe.is_some() && !me.is_empty() {
        clock.0 = Some(time.elapsed_secs());
        info!("streak test: the HUD is up at {:.1} s; the script starts", time.elapsed_secs());
    }
}

/// Moving the mouse (down and right) until then (seconds in), as a player
/// moves the laptop's cursor.
#[derive(Resource, Default)]
struct Nudging(f32);

fn nudge(time: Res<Time>, clock: Res<Clock>, nudging: Res<Nudging>, mut motion: ResMut<bevy::input::mouse::AccumulatedMouseMotion>) {
    if clock.t(&time).is_some_and(|t| t < nudging.0) {
        motion.delta += Vec2::new(6.0, 3.0);
    }
}

/// The fighting tried with the gun out of hand (seconds in, while the
/// script has the UAV called in at 7 s): the player's trigger, frag and
/// knife held; a dummy's UAV and its trigger; the player's trigger at the
/// end, the gun in hand.
const FIGHT_LOCKED: (f32, f32) = (7.15, 8.6);
const DUMMY_CALL: f32 = 9.0;
const DUMMY_LOCKED: (f32, f32) = (9.1, 9.8);
const DUMMY_FREE: (f32, f32) = (17.0, 17.6);
const FIGHT_FREE: (f32, f32) = (58.6, 59.2);

#[derive(Resource, Default)]
struct Fighting {
    dummy: Option<Entity>,
    /// Frames in the locked window with the hands up, and without.
    hands_frames: (u32, u32),
    /// Shots, grenades and knife swings in each window.
    locked: (u32, u32, u32),
    free: u32,
    dummy_locked: u32,
    dummy_free: u32,
    /// When the dummy's lock came off.
    dummy_unlocked: Option<f32>,
    told: bool,
}

fn within(t: f32, (a, b): (f32, f32)) -> bool {
    t >= a && t < b
}

/// The player's fighting buttons, pressed as the keyboard and mouse would
/// (the whole way: the bindings, the pad's layer and the call-in's strip).
fn fight_keys(time: Res<Time>, clock: Res<Clock>, mut keys: ResMut<ButtonInput<KeyCode>>, mut mouse: ResMut<ButtonInput<MouseButton>>) {
    let Some(t) = clock.t(&time).filter(|_| !laptop_test()) else { return };
    let locked = within(t, FIGHT_LOCKED);
    for key in [KeyCode::KeyG, KeyCode::KeyV] {
        if locked {
            keys.press(key);
        } else if within(t - 0.1, FIGHT_LOCKED) {
            keys.release(key);
        }
    }
    if locked || within(t, FIGHT_FREE) {
        mouse.press(MouseButton::Left);
    } else if within(t - 0.1, FIGHT_LOCKED) || within(t - 0.1, FIGHT_FREE) {
        mouse.release(MouseButton::Left);
    }
}

/// A dummy calls a UAV in, its trigger held (as a bot's would be) through
/// its lock and again once it's off; the player kept alive meanwhile.
#[allow(clippy::type_complexity)]
fn dummy_fights(
    time: Res<Time>,
    clock: Res<Clock>,
    mut fighting: ResMut<Fighting>,
    mut dummies: Query<(Entity, &mut super::Killstreak, &mut HardpointInput, &mut crate::weapons::WeaponInput), (Without<LocalPlayer>, Without<crate::combat::Dead>)>,
    mut me: Query<&mut crate::combat::Health, With<LocalPlayer>>,
) {
    let Some(t) = clock.t(&time).filter(|_| !laptop_test()) else { return };
    if t >= DUMMY_CALL && fighting.dummy.is_none() {
        if let Some((e, mut streak, mut input, _)) = dummies.iter_mut().next() {
            streak.held.add(super::Hardpoint::Uav);
            input.use_now = true;
            input.item = Some(super::Hardpoint::Uav);
            fighting.dummy = Some(e);
        }
    }
    let Some(e) = fighting.dummy else { return };
    if within(t, DUMMY_LOCKED) || within(t, DUMMY_FREE) {
        if let Ok((.., mut w)) = dummies.get_mut(e) {
            w.fire = true;
        }
        for mut h in &mut me {
            h.current = h.current.max(5000.0);
        }
    }
}

#[allow(clippy::type_complexity)]
fn count_fighting(
    time: Res<Time>,
    clock: Res<Clock>,
    mut fighting: ResMut<Fighting>,
    mut shots: MessageReader<crate::weapons::ShotFired>,
    me: Query<(Entity, Has<super::hands::HardpointHands>), With<LocalPlayer>>,
    grenades: Query<&crate::grenades::LiveGrenade, Added<crate::grenades::LiveGrenade>>,
    swings: Query<(), (Added<crate::melee::Melee>, With<LocalPlayer>)>,
    locks: Query<(), With<super::hands::CallingIn>>,
) {
    let Some(t) = clock.t(&time) else { return };
    let Ok((me, hands)) = me.single() else { return };
    if within(t, FIGHT_LOCKED) {
        if hands {
            fighting.hands_frames.0 += 1;
        } else {
            fighting.hands_frames.1 += 1;
        }
        fighting.locked.1 += grenades.iter().filter(|g| g.thrower == me).count() as u32;
        fighting.locked.2 += swings.iter().count() as u32;
    }
    if let Some(d) = fighting.dummy {
        if fighting.dummy_unlocked.is_none() && t > DUMMY_CALL + 0.1 && !locks.contains(d) {
            fighting.dummy_unlocked = Some(t);
        }
    }
    for s in shots.read() {
        if s.weapon.is_none() {
            continue;
        }
        if s.shooter == me && within(t, FIGHT_LOCKED) {
            fighting.locked.0 += 1;
        } else if s.shooter == me && within(t, FIGHT_FREE) {
            fighting.free += 1;
        } else if Some(s.shooter) == fighting.dummy && within(t, DUMMY_LOCKED) {
            fighting.dummy_locked += 1;
        } else if Some(s.shooter) == fighting.dummy && within(t, DUMMY_FREE) {
            fighting.dummy_free += 1;
        }
    }
    if t >= FIGHT_FREE.1 && !fighting.told {
        fighting.told = true;
        let f = &*fighting;
        info!(
            "streak test: fighting with the call-in's hands up ({} of {} frames): {} shots, {} grenades, {} knife swings (want 0); a dummy's UAV: {} shots while locked (want 0), lock off at {:?} s, {} shots after (want > 0); the gun back in hand: {} shots (want > 0)",
            f.hands_frames.0,
            f.hands_frames.0 + f.hands_frames.1,
            f.locked.0,
            f.locked.1,
            f.locked.2,
            f.dummy_locked,
            f.dummy_unlocked.map(|t| (t * 10.0).round() / 10.0),
            f.dummy_free,
            f.free,
        );
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
    /// Move the mouse for this long (seconds).
    Nudge(f32),
    /// Despawn the laptop screen's camera and picture, as the match's end
    /// clears the match's things.
    ClearLaptop,
    /// Hold this one too.
    Give(super::Hardpoint),
    /// Put the airstrike's map (and laptop) away.
    CloseMap,
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
    WatchMarker,
    WatchCourier,
    WatchCrate,
    /// Stand at the crate looking at it, and hold Use on it (or let go).
    ToCrate,
    HoldUse(bool),
    /// Click the map where the mouse is (until the map closes).
    ClickMap,
    /// Walk the player straight through the sentry (it doesn't block).
    WalkThrough,
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
    (8.5, Act::WatchMarker),
    (9.6, Act::Shot("cp_flare")),
    (10.4, Act::WatchCourier),
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
    (25.8, Act::WalkThrough),
    (27.6, Act::Shot("sentry_walked_through")),
    (28.0, Act::EnemyBeforeSentry(400.0)),
    (28.1, Act::ShotSentryFiring("sentry_firing")),
    (30.5, Act::Shot("sentry_after_fire")),
    (31.5, Act::HitSentry(1200.0)),
    (31.8, Act::Shot("sentry_destroyed")),
    (34.0, Act::Shot("sentry_wreck")),
    (35.5, Act::Exit),
];

/// `COD4RW_STREAKTEST_LAPTOP=1`: the airstrike's laptop held up half a
/// minute, a shot a second (for fitting the hold, its knobs changed in a
/// `COD4RW_TUNING` file meanwhile).
const LAPTOP_SCRIPT: &[(f32, Act)] = &[
    (5.0, Act::Give(super::Hardpoint::Airstrike)),
    (5.2, Act::Use),
    (8.0, Act::Shot("hold_00")), (9.0, Act::Shot("hold_01")), (10.0, Act::Shot("hold_02")), (11.0, Act::Shot("hold_03")), (12.0, Act::Shot("hold_04")), (13.0, Act::Shot("hold_05")), (14.0, Act::Shot("hold_06")), (15.0, Act::Shot("hold_07")), (16.0, Act::Shot("hold_08")), (17.0, Act::Shot("hold_09")), (18.0, Act::Shot("hold_10")), (19.0, Act::Shot("hold_11")), (20.0, Act::Shot("hold_12")), (21.0, Act::Shot("hold_13")), (22.0, Act::Shot("hold_14")), (23.0, Act::Shot("hold_15")), (24.0, Act::Shot("hold_16")), (25.0, Act::Shot("hold_17")), (26.0, Act::Shot("hold_18")), (27.0, Act::Shot("hold_19")), (28.0, Act::Shot("hold_20")), (29.0, Act::Shot("hold_21")), (30.0, Act::Shot("hold_22")), (31.0, Act::Shot("hold_23")), (32.0, Act::Shot("hold_24")), (33.0, Act::Shot("hold_25")), (34.0, Act::Shot("hold_26")), (35.0, Act::Shot("hold_27")), (36.0, Act::Shot("hold_28")), (37.0, Act::Shot("hold_29")), (38.0, Act::Shot("hold_30")), (39.0, Act::Shot("hold_31")),
    (40.5, Act::Exit),
];

fn laptop_test() -> bool {
    std::env::var_os("COD4RW_STREAKTEST_LAPTOP").is_some()
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
    // (The laptop up in the hands, its screen showing the map; settled
    // before the eye; its cursor moved.)
    (14.4, Act::Shot("airstrike_laptop")),
    (15.2, Act::Shot("airstrike_laptop_held")),
    (15.3, Act::Nudge(0.5)),
    (16.0, Act::Shot("airstrike_laptop_cursor")),
    (16.2, Act::UseAt(1200.0)),
    (22.0, Act::Shot("airstrike_burst")),
    (22.6, Act::Shot("airstrike_bomblets")),
    (23.7, Act::Shot("airstrike_after")),
    // (A second airstrike after the laptop's screen is cleared away, as a
    // match's end clears it: its laptop builds a new one.)
    (24.0, Act::ClearLaptop),
    (24.2, Act::Give(super::Hardpoint::Airstrike)),
    (24.4, Act::Use),
    (26.8, Act::Shot("airstrike_laptop_again")),
    (27.0, Act::CloseMap),
    (28.1, Act::Kills(2)),
    (28.7, Act::Shot("heli_earned")),
    (29.1, Act::Use),
    (29.2, Act::Follow),
    (29.6, Act::ShotFiring("heli_firing")),
    (32.6, Act::Shot("heli_inbound")),
    (36.6, Act::Shot("heli_2")),
    (40.6, Act::Shot("heli_3")),
    (44.6, Act::Shot("heli_4")),
    (47.6, Act::HitHeli(600.0)),
    (49.6, Act::Shot("heli_smoking")),
    (51.6, Act::HitHeli(600.0)),
    (52.0, Act::Shot("heli_hit")),
    (53.6, Act::Shot("heli_crashing")),
    (56.1, Act::Shot("heli_crash")),
    (58.6, Act::Shot("heli_after")),
    (59.6, Act::Exit),
];

fn drive(
    mut commands: Commands,
    time: Res<Time>,
    (test, clock): (Res<StreakTest>, Res<Clock>),
    mut step: Local<usize>,
    mut look_at: Local<Option<Vec3>>,
    mut player: Query<
        (Entity, &Pawn, &mut HardpointInput, &mut Transform, &mut ViewAngles, &mut crate::movement::Mover, &mut super::Killstreak),
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
        Res<super::models::StreakProps>,
        Local<Option<f32>>,
        Local<Option<(Entity, Vec3, f32)>>,
        ResMut<Clicking>,
        Query<(&crate::grenades::LiveGrenade, &Transform), (Without<LocalPlayer>, Without<Pawn>)>,
        Query<&mut crate::combat::Health, With<LocalPlayer>>,
        ResMut<Nudging>,
        Local<Option<(Vec3, Vec3, f32)>>,
        Query<(Entity, &Name)>,
    ),
) {
    let (couriers, crates, sentries, mut others, mut watching, mut sentry_shot, mw2_content, mut started, mut pinned, mut clicking, markers, mut health, mut nudging, mut walk, names) = mw2;
    let mw2_test = std::env::var_os("COD4RW_STREAKTEST_MW2").is_some();
    // MW2's script walks about among the bots: kept alive for it.
    if mw2_test {
        for mut h in &mut health {
            h.current = h.current.max(5000.0);
        }
    }
    let script = if mw2_test {
        MW2_SCRIPT
    } else if laptop_test() {
        LAPTOP_SCRIPT
    } else {
        SCRIPT
    };
    let Some(t) = clock.t(&time) else { return };
    // MW2's script waits for the props to load (or a minute).
    let t = if mw2_test {
        let ready = !mw2_content.busy() || t > 60.0;
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
    let Ok((me, mine, mut input, mut feet, mut view, mut mover, mut streak)) = player.single_mut() else { return };
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
    // Walking through the sentry: on along its line, then how far past it.
    if let Some((at, forward, until)) = *walk {
        if t < until {
            mover.velocity = forward * crate::units::u(200.0) + Vec3::Y * mover.velocity.y;
        } else {
            let past = (feet.translation - at).dot(forward) / crate::units::u(1.0);
            info!("streak test: walked through the sentry: {past:.0} units past it (started 90 before)");
            *walk = None;
        }
    }
    // Watching the care package come in and fall.
    match *watching {
        1 => *look_at = couriers.iter().next().map(|g| g.translation()).or(*look_at),
        3 => *look_at = markers.iter().find(|(g, _)| g.how == crate::grenades::How::Marker).map(|(_, tf)| tf.translation).or(*look_at),
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
            Act::ClearLaptop => {
                for (e, name) in &names {
                    if name.as_str() == "laptop screen camera" || name.as_str() == "laptop screen" {
                        commands.entity(e).try_despawn();
                    }
                }
            }
            Act::Give(item) => streak.held.add(*item),
            Act::CloseMap => commands.remove_resource::<super::airstrike::Selecting>(),
            Act::UseItem(item) => {
                input.use_now = true;
                input.item = Some(*item);
                let ahead = (view.rotation() * Vec3::NEG_Z).with_y(0.0).normalize_or_zero();
                // (The marker thrown up and out, the sentry looked down at.)
                *look_at = Some(eye + ahead * crate::units::u(300.0) + Vec3::Y * crate::units::u(if *item == super::Hardpoint::Sentry { -140.0 } else { 20.0 }));
            }
            Act::WatchCourier => *watching = 1,
            Act::WatchMarker => {
                *watching = 3;
                // Back off a few metres to see it (and not our own legs).
                if let Some((_, m)) = markers.iter().find(|(g, _)| g.how == crate::grenades::How::Marker) {
                    let away = (feet.translation - m.translation).with_y(0.0).normalize_or(Vec3::X);
                    feet.translation = m.translation.with_y(feet.translation.y) + away * crate::units::u(260.0);
                    mover.velocity = Vec3::ZERO;
                }
            }
            Act::WatchCrate => *watching = 2,
            Act::ToCrate => {
                *watching = 0;
                if let Some((_, g, _)) = crates.iter().find(|c| c.2.landed() && c.2.owner == me) {
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
            Act::WalkThrough => {
                if let Some((_, stf, _)) = sentries.iter().next() {
                    let forward = (stf.rotation * Vec3::X).with_y(0.0).normalize_or(Vec3::X);
                    feet.translation = stf.translation - forward * crate::units::u(90.0);
                    *walk = Some((stf.translation, forward, t + 1.6));
                    *look_at = Some(stf.translation + forward * crate::units::u(400.0) + Vec3::Y * crate::units::u(50.0));
                }
            }
            Act::Nudge(secs) => nudging.0 = t + secs,
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
