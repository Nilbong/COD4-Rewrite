//! Debug aid: with `COD4RW_SHOT=<dir>` set, save screenshots of a few
//! viewpoints after loading and then exit. Used to check rendering without a
//! human at the keyboard.

use crate::movement::ViewAngles;
use crate::player::LocalPlayer;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

pub struct AutoShotPlugin;

impl Plugin for AutoShotPlugin {
    fn build(&self, app: &mut App) {
        if let Ok(dir) = std::env::var("COD4RW_SHOT") {
            app.insert_resource(ShotDir(dir.into())).add_systems(Update, (run, dump_joints));
        }
    }
}

#[derive(Resource)]
struct ShotDir(std::path::PathBuf);

fn run(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<ShotDir>,
    mut player: Query<&mut ViewAngles, With<LocalPlayer>>,
    mut step: Local<usize>,
    mut initial_yaw: Local<Option<f32>>,
    mut exit: MessageWriter<AppExit>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
) {
    // (time, yaw offset in degrees, pitch in degrees)
    const SHOTS: [(f32, f32, f32); 4] = [(6.0, 0.0, 0.0), (7.0, 90.0, -10.0), (8.0, 180.0, 10.0), (9.0, 270.0, 0.0)];
    let t = time.elapsed_secs();
    if *step < SHOTS.len() {
        let (at, yaw, pitch) = SHOTS[*step];
        // Aim slightly before capturing so the frame reflects the new view.
        if t >= at - 0.5 {
            if let Ok(mut v) = player.single_mut() {
                let base = *initial_yaw.get_or_insert(v.yaw);
                v.yaw = base + yaw.to_radians();
                v.pitch = pitch.to_radians();
            }
        }
        if t >= at {
            std::fs::create_dir_all(&dir.0).ok();
            let path = dir.0.join(format!("shot{}.png", *step));
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
            *step += 1;
        }
    } else if t > SHOTS[SHOTS.len() - 1].0 + 1.5 {
        if let Some(fps) =
            diagnostics.get(&bevy::diagnostic::FrameTimeDiagnosticsPlugin::FPS).and_then(|d| d.smoothed())
        {
            info!("fps {fps:.0}");
        }
        exit.write(AppExit::Success);
    }
}

/// Debug aid: with `COD4RW_SIM=<seconds>` set, log the match state every few
/// seconds and exit after that long.
pub struct SimLogPlugin;

impl Plugin for SimLogPlugin {
    fn build(&self, app: &mut App) {
        if let Some(secs) = std::env::var("COD4RW_SIM").ok().and_then(|s| s.parse::<f32>().ok()) {
            app.insert_resource(SimFor(secs)).add_systems(Update, sim_log);
        }
    }
}

#[derive(Resource)]
struct SimFor(f32);

fn sim_log(
    time: Res<Time>,
    sim: Res<SimFor>,
    mut next: Local<f32>,
    mut exit: MessageWriter<AppExit>,
    pawns: Query<(
        &crate::combat::Pawn,
        &Transform,
        &crate::movement::Mover,
        &crate::weapons::WeaponState,
        Has<crate::combat::Dead>,
    )>,
    state: Option<Res<crate::tdm::MatchState>>,
    feed: Res<crate::combat::KillFeed>,
    mut shots: MessageReader<crate::weapons::ShotFired>,
    mut hits: MessageReader<crate::weapons::HitConfirmed>,
    mut counts: Local<std::collections::HashMap<String, (u32, u32)>>,
    teams: Query<&crate::combat::Pawn>,
) {
    for s in shots.read() {
        if let Ok(p) = teams.get(s.shooter) {
            counts.entry(format!("{:?}", p.team)).or_default().0 += 1;
        }
    }
    for h in hits.read() {
        if let Ok(p) = teams.get(h.shooter) {
            counts.entry(format!("{:?}", p.team)).or_default().1 += 1;
        }
    }
    let t = time.elapsed_secs();
    if t < *next {
        return;
    }
    info!("shots/hits by team: {:?}", *counts);
    *next = t + 5.0;
    let s = state.map(|s| format!("allies {} axis {}", s.allies, s.axis)).unwrap_or_default();
    info!("t={t:.0}s {s} feed={}", feed.entries.len());
    for (p, tf, m, w, dead) in &pawns {
        info!(
            "  {:<10} {:?} pos ({:7.2},{:6.2},{:7.2}) ground {} speed {:4.1} ammo {}/{} k/d {}/{}{}",
            p.name,
            p.team,
            tf.translation.x,
            tf.translation.y,
            tf.translation.z,
            m.on_ground,
            m.horizontal_speed(),
            w.clip,
            w.reserve,
            p.kills,
            p.deaths,
            if dead { " DEAD" } else { "" }
        );
    }
    if t > sim.0 {
        for e in feed.entries.iter().rev().take(8) {
            info!("  feed: {}", e.text);
        }
        exit.write(AppExit::Success);
    }
}

/// Debug aid: with `COD4RW_MOVETEST=1`, drive the local player through a
/// scripted set of moves and log measured speeds (CoD units/s) and jump
/// height, then exit.
pub struct MoveTestPlugin;

impl Plugin for MoveTestPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("COD4RW_MOVETEST").is_some() {
            app.add_systems(Update, move_test.before(crate::movement::MovementSet).after(crate::player::InputSet));
        }
        if let Ok(path) = std::env::var("COD4RW_LIPTEST") {
            let cases = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| {
                    let v: Vec<f32> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                    (v.len() >= 5).then(|| ([v[0], v[1], v[2]], [v[3], v[4]]))
                })
                .collect();
            let jump = std::env::var_os("COD4RW_LIPTEST_JUMP").is_some();
            let stairs = std::env::var_os("COD4RW_LIPTEST_STAIRS").is_some();
            let sprint = std::env::var_os("COD4RW_LIPTEST_SPRINT").is_some();
            app.insert_resource(LipTest { cases, jump, stairs, sprint, ..default() })
                .add_systems(Update, lip_test.before(crate::movement::MovementSet).after(crate::player::InputSet));
        }
    }
}

#[derive(Default)]
struct MoveTestState {
    phase: usize,
    phase_start: f32,
    max_speed: f32,
    start_y: f32,
    max_y: f32,
}

fn move_test(
    time: Res<Time>,
    mut st: Local<MoveTestState>,
    mut player: Query<(&mut crate::movement::MoveInput, &crate::movement::Mover, &Transform), With<LocalPlayer>>,
    mut exit: MessageWriter<AppExit>,
) {
    use crate::movement::{MoveInput, Stance};
    let Ok((mut input, mover, tf)) = player.single_mut() else { return };
    // (name, duration, input)
    let phases: [(&str, f32, MoveInput); 10] = [
        ("settle", 1.5, MoveInput::default()),
        ("strafe left", 1.5, MoveInput { right: -1.0, ..default() }),
        ("diagonal", 1.5, MoveInput { forward: 1.0, right: 1.0, ..default() }),
        ("run forward", 1.5, MoveInput { forward: 1.0, ..default() }),
        ("sprint", 1.5, MoveInput { forward: 1.0, sprint: true, ..default() }),
        ("backpedal", 1.5, MoveInput { forward: -1.0, ..default() }),
        ("strafe", 1.5, MoveInput { right: 1.0, ..default() }),
        ("crouch walk", 1.5, MoveInput { forward: 1.0, stance: Stance::Crouch, ..default() }),
        ("prone crawl", 2.0, MoveInput { forward: 1.0, stance: Stance::Prone, ..default() }),
        ("jump", 1.5, MoveInput { jump: true, ..default() }),
    ];
    let t = time.elapsed_secs();
    if t < 3.0 {
        return;
    }
    if st.phase_start == 0.0 {
        st.phase_start = t;
        st.start_y = tf.translation.y;
    }
    let Some((name, dur, inp)) = phases.get(st.phase) else {
        exit.write(AppExit::Success);
        return;
    };
    *input = MoveInput { speed_scale: 1.0, ..*inp };
    if *name == "jump" && t - st.phase_start > 0.1 {
        input.jump = false;
    }
    let inch = crate::units::INCH;
    // Ignore the first part of each phase while accelerating.
    if t - st.phase_start > dur * 0.5 {
        st.max_speed = st.max_speed.max(mover.horizontal_speed() / inch);
    }
    st.max_y = st.max_y.max(tf.translation.y);
    if t - st.phase_start >= *dur {
        info!(
            "movetest {name:<12} speed {:6.1} u/s  peak {:5.1} u  eye {:4.1} u  sprint_left {:.2}s pos {:?} ground {} vel {:?}",
            st.max_speed,
            (st.max_y - st.start_y) / inch,
            mover.eye_height / inch,
            mover.sprint_left, tf.translation, mover.on_ground, mover.velocity
        );
        st.phase += 1;
        st.phase_start = t;
        st.max_speed = 0.0;
        st.start_y = tf.translation.y;
        st.max_y = tf.translation.y;
    }
}

/// Debug aid: with `COD4RW_LIPTEST=<cases>`, walk the local player over
/// each low obstacle in the file (lines of `x y z dx dy`: start feet and
/// direction, CoD units; `iw3 --example lips` writes them) and log how many
/// it got over, then exit.
#[derive(Resource, Default)]
struct LipTest {
    cases: Vec<([f32; 3], [f32; 2])>,
    case: usize,
    started: f32,
    passed: usize,
    failed: Vec<usize>,
    /// Cases with a wall in the way or no floor beyond: not about the step.
    invalid: usize,
    /// Hold jump while walking (`COD4RW_LIPTEST_JUMP`): mantling tests, which
    /// pass on climbing at least 16 units and skip the clear-way checks.
    jump: bool,
    /// Staircases (`COD4RW_LIPTEST_STAIRS`): no clear-way checks (the steps
    /// rise into the way), passing on climbing at least 24 units.
    stairs: bool,
    /// Sprint up them (`COD4RW_LIPTEST_SPRINT`).
    sprint: bool,
    /// Feet height at the start of the case.
    start_y: f32,
    /// Cases where the mantle hint showed, and whether this one has.
    hinted: usize,
    hint_now: bool,
}

/// Seconds per case: settling after the teleport, then walking.
const LIP_SETTLE: f32 = 0.3;
const LIP_WALK: f32 = 1.2;

fn lip_test(
    time: Res<Time>,
    spatial: avian3d::prelude::SpatialQuery,
    mut test: ResMut<LipTest>,
    mut player: Query<(&mut crate::movement::MoveInput, &mut crate::movement::Mover, &mut Transform, &mut crate::movement::ViewAngles), With<LocalPlayer>>,
    mut exit: MessageWriter<AppExit>,
) {
    use crate::movement::MoveInput;
    let Ok((mut input, mut mover, mut tf, mut view)) = player.single_mut() else { return };
    let t = time.elapsed_secs();
    if t < 4.0 {
        return;
    }
    let Some(&(start, dir)) = test.cases.get(test.case) else {
        let n = test.cases.len() - test.invalid;
        info!("liptest: {} of {n} crossed ({} cases skipped, mantle hint in {}); failed cases {:?}", test.passed, test.invalid, test.hinted, test.failed);
        exit.write(AppExit::Success);
        return;
    };
    let from = crate::units::pos(start);
    let along = crate::units::dir([dir[0], dir[1], 0.0]);
    if test.started == 0.0 {
        // Only cases where the way is clear at waist height and there's
        // floor beyond: what's left in the way is the step.
        let filter = crate::collision::movement_filter();
        let u = crate::units::u;
        let waist = from + Vec3::Y * u(40.0);
        let clear = Dir3::new(along).ok().is_some_and(|d| spatial.cast_ray(waist, d, u(110.0), true, &filter).is_none());
        let beyond = from + along * u(90.0) + Vec3::Y * u(30.0);
        let floor = spatial.cast_ray(beyond, Dir3::NEG_Y, u(50.0), true, &filter).is_some();
        // Floor just under the start, and room to stand there.
        let down = spatial.cast_ray(from + Vec3::Y * u(4.0), Dir3::NEG_Y, u(24.0), true, &filter).filter(|h| h.distance > 0.0);
        let standing = down.is_some_and(|h| {
            let feet = from + Vec3::Y * (u(4.0) - h.distance);
            spatial
                .shape_intersections(&avian3d::prelude::Collider::capsule(u(15.0), u(38.0)), feet + Vec3::Y * u(36.0), Quat::IDENTITY, &filter)
                .is_empty()
        });
        if !standing || !test.jump && !test.stairs && (!clear || !floor) {
            test.invalid += 1;
            test.case += 1;
            return;
        }
        test.started = t;
        test.start_y = from.y + u(4.0) - down.map_or(u(4.0), |h| h.distance);
        tf.translation = from + Vec3::Y * (u(4.0) - down.map_or(u(4.0), |h| h.distance));
        mover.velocity = Vec3::ZERO;
        view.yaw = f32::atan2(-along.x, -along.z);
        view.pitch = 0.0;
    }
    let walking = t - test.started > LIP_SETTLE;
    if walking && mover.mantle_hint && !test.hint_now {
        test.hint_now = true;
        test.hinted += 1;
    }
    *input = MoveInput {
        speed_scale: 1.0,
        forward: if walking { 1.0 } else { 0.0 },
        jump: walking && test.jump,
        sprint: walking && test.sprint,
        ..default()
    };
    if t - test.started > LIP_SETTLE + LIP_WALK {
        let gone = (tf.translation - from).dot(along) / crate::units::INCH;
        let rose = (tf.translation.y - test.start_y) / crate::units::INCH;
        let passed = match (test.jump, test.stairs) {
            (true, _) => rose > 16.0,
            (_, true) => rose > 24.0,
            _ => gone > 56.0,
        };
        if passed {
            test.passed += 1;
        } else {
            let i = test.case;
            test.failed.push(i);
            let moved = (tf.translation - from) / crate::units::INCH;
            info!("liptest: case {i} stopped {gone:.1} units in, rose {:.1}, on ground {}, speed {:.0}", moved.y, mover.on_ground, mover.horizontal_speed() / crate::units::INCH);
        }
        test.case += 1;
        test.started = 0.0;
        test.hint_now = false;
    }
}

/// Debug: with `COD4RW_DUMMY`, print where key joints of each body end up.
pub fn dump_joints(
    time: Res<Time>,
    mut done: Local<bool>,
    pawns: Query<(&Name, &Transform, &crate::thirdperson::Body)>,
    skeletons: Query<&crate::models::Skeleton>,
    globals: Query<&GlobalTransform>,
) {
    if *done || time.elapsed_secs() < 7.0 || std::env::var_os("COD4RW_DUMMY").is_none() {
        return;
    }
    *done = true;
    for (name, tf, body) in &pawns {
        let Ok(skel) = skeletons.get(body.0) else { continue };
        let mut line = format!("{name}:");
        for j in ["j_mainroot", "pelvis", "j_head", "j_wrist_ri", "j_wrist_le", "tag_weapon_right", "j_ankle_ri"] {
            if let Some(e) = skel.joint(j) {
                if let Ok(g) = globals.get(e) {
                    let rel = (g.translation() - tf.translation) / crate::units::INCH;
                    line += &format!(" {j}=({:.0},{:.0},{:.0})", rel.x, rel.y, rel.z);
                }
            }
        }
        // Gun direction: tag_weapon_right's CoD +X axis (Bevy +X locally),
        // in the pawn's frame where forward is -Z.
        if let Some(e) = skel.joint("tag_weapon_right") {
            if let Ok(g) = globals.get(e) {
                let axis = tf.rotation.inverse() * (g.rotation() * Vec3::X);
                line += &format!(" gun_dir=({:.2},{:.2},{:.2})", axis.x, axis.y, axis.z);
            }
        }
        info!("{line}");
    }
}

/// Debug aid: with `COD4RW_VMTEST=<dir>`, drive the local player's weapon
/// through idle, fire, ADS, ADS fire, un-ADS, reload, sprint (going in, two
/// moments of the loop, coming out), a walk and an inspect (each side),
/// saving a screenshot of each, then exit.
pub struct ViewModelTestPlugin;

impl Plugin for ViewModelTestPlugin {
    fn build(&self, app: &mut App) {
        if let Ok(dir) = std::env::var("COD4RW_VMTEST") {
            app.insert_resource(VmTestDir(dir.into()))
                .add_systems(Update, vm_test.after(crate::player::InputSet).before(crate::movement::MovementSet));
        }
    }
}

#[derive(Resource)]
struct VmTestDir(std::path::PathBuf);

fn vm_test(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<VmTestDir>,
    mut player: Query<(&mut crate::weapons::WeaponInput, &mut crate::movement::MoveInput), With<LocalPlayer>>,
    mut shot: Local<usize>,
    mut exit: MessageWriter<AppExit>,
) {
    use crate::movement::MoveInput;
    use crate::weapons::WeaponInput;
    // (start, end, weapon input, move input, screenshot time)
    let sprint = MoveInput { forward: 1.0, sprint: true, ..default() };
    let phases: [(&str, f32, f32, WeaponInput, MoveInput, f32); 13] = [
        ("idle", 6.0, 7.0, WeaponInput::default(), MoveInput::default(), 6.9),
        ("fire", 7.0, 7.6, WeaponInput { fire: true, ..default() }, MoveInput::default(), 7.45),
        ("ads", 7.6, 8.6, WeaponInput { ads: true, ..default() }, MoveInput::default(), 8.5),
        ("ads_fire", 8.6, 9.1, WeaponInput { ads: true, fire: true, ..default() }, MoveInput::default(), 9.0),
        ("unads", 9.1, 10.1, WeaponInput::default(), MoveInput::default(), 10.0),
        ("reload", 10.1, 11.6, WeaponInput { reload: true, ..default() }, MoveInput::default(), 11.0),
        ("sprint_in", 11.6, 11.75, WeaponInput::default(), sprint, 11.74),
        ("sprint", 11.75, 12.6, WeaponInput::default(), sprint, 12.5),
        ("sprint_loop", 12.6, 13.0, WeaponInput::default(), sprint, 12.85),
        ("sprint_out", 13.0, 13.6, WeaponInput::default(), MoveInput::default(), 13.12),
        ("walk", 13.6, 14.6, WeaponInput::default(), MoveInput { forward: 1.0, ..default() }, 14.4),
        ("inspect_left", 14.6, 16.9, WeaponInput { inspect: true, ..default() }, MoveInput::default(), 15.7),
        ("inspect_right", 16.9, 18.4, WeaponInput::default(), MoveInput::default(), 17.2),
    ];
    let t = time.elapsed_secs();
    let Ok((mut wi, mut mi)) = player.single_mut() else { return };
    for (name, start, end, w, m, at) in phases {
        if t >= start && t < end {
            *wi = w;
            *mi = MoveInput { speed_scale: 1.0, ..m };
        }
        if *shot < phases.len() && phases[*shot].0 == name && t >= at {
            std::fs::create_dir_all(&dir.0).ok();
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.0.join(format!("vm_{}_{name}.png", *shot))));
            *shot += 1;
        }
    }
    if t > 18.4 {
        exit.write(AppExit::Success);
    }
}

/// Debug aid: with `COD4RW_FRAMES=<dir>`, save every frame from 10 s (the
/// map can take 6 s to load) for `COD4RW_FRAMES_COUNT` frames (default 30),
/// the view held still, then exit: for finding what flickers.
pub struct FramesPlugin;

impl Plugin for FramesPlugin {
    fn build(&self, app: &mut App) {
        if let Ok(dir) = std::env::var("COD4RW_FRAMES") {
            app.insert_resource(FramesDir(dir.into())).add_systems(Update, save_frames);
        }
    }
}

#[derive(Resource)]
struct FramesDir(std::path::PathBuf);

fn save_frames(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<FramesDir>,
    mut saved: Local<usize>,
    mut done_at: Local<Option<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    let count = std::env::var("COD4RW_FRAMES_COUNT").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    let t = time.elapsed_secs();
    if t < 10.0 {
        return;
    }
    if *saved < count {
        std::fs::create_dir_all(&dir.0).ok();
        let path = dir.0.join(format!("frame{:03}.png", *saved));
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        *saved += 1;
    } else if t > *done_at.get_or_insert(t) + 3.0 {
        exit.write(AppExit::Success);
    }
}
