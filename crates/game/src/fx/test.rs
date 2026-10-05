//! Debug aid: with `COD4RW_FXTEST=<dir>`, fire single shots and save
//! screenshots of their effects, then exit. In first person, down at the
//! floor (the view flash, then the impact as it develops); in third person
//! (the world flash); then at the nearest other pawn (blood; run with
//! `COD4RW_DUMMY=1` for the frozen lineup to shoot), ending in a burst
//! that kills it (the exit wound). Flashes last a frame or two, so shots
//! are taken on the frame a shot is fired and at set times after it.
//!
//! With `COD4RW_FXAT=<dir>`, an effect plays at a set view's screen point
//! over its life (see [`FxAt`]).
//!
//! With `COD4RW_FXPLAY=<dir>`, effects using clouds, trails and child
//! effects play where the player looks (a glass impact, a fatal exit wound,
//! brick and mud impacts) and on something flying past (a rocket's smoke
//! trail), with screenshots as they develop, then exit.

use crate::combat::{Dead, Pawn};
use crate::movement::ViewAngles;
use crate::player::LocalPlayer;
use crate::units::u;
use crate::weapons::{ShotFired, WeaponInput};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

pub(super) fn register(app: &mut App) {
    if std::env::var_os("COD4RW_FXLOG").is_some() {
        app.add_systems(Last, stats.run_if(crate::state::in_game));
    }
    if let Ok(dir) = std::env::var("COD4RW_FXPLAY") {
        app.insert_resource(FxPlay(dir.into())).add_systems(
            Update,
            play.after(crate::player::InputSet).before(crate::movement::MovementSet).run_if(crate::state::in_game),
        );
    }
    if let Ok(dir) = std::env::var("COD4RW_FXAT") {
        app.insert_resource(FxAt(dir.into())).add_systems(
            Update,
            play_at.after(crate::player::InputSet).before(crate::movement::MovementSet).run_if(crate::state::in_game),
        );
    }
    if let Ok(dir) = std::env::var("COD4RW_FXTEST") {
        app.insert_resource(FxTest { dir: dir.into(), ..default() })
            .add_systems(Update, drive.after(crate::player::InputSet).before(crate::movement::MovementSet))
            .add_systems(PostUpdate, capture.after(super::run_effects));
    }
}

#[derive(Resource, Default)]
struct FxTest {
    dir: std::path::PathBuf,
    /// The step under way, and the time the latest shot of interest fired.
    step: usize,
    shot_at: Option<f32>,
    /// Screenshots still to take: (seconds after the shot, frames after it, name).
    pending: Vec<(f32, u32, String)>,
    frames: u32,
}

/// What each step does: its start time, the view, where to aim, how long
/// to hold the trigger (0: one shot), and screenshots after its first shot
/// as (seconds, frames, name).
struct Step {
    at: f32,
    third_person: bool,
    target: Aim,
    fire: f32,
    shots: &'static [(f32, u32, &'static str)],
}

#[derive(Clone, Copy)]
enum Aim {
    /// Ahead and this far down (degrees).
    Down(f32),
    /// The nearest enemy's chest (a teammate's if there's none).
    Pawn,
}

const STEPS: &[Step] = &[
    Step {
        at: 6.0,
        third_person: false,
        target: Aim::Down(25.0),
        fire: 0.0,
        shots: &[
            (0.0, 0, "view_flash"),
            (0.0, 2, "view_flash_2f"),
            (0.08, 0, "view_shell_80ms"),
            (0.15, 0, "impact_150ms"),
            (0.6, 0, "impact_600ms"),
        ],
    },
    Step {
        at: 8.0,
        third_person: true,
        target: Aim::Down(15.0),
        fire: 0.0,
        shots: &[
            (0.0, 0, "world_flash"),
            (0.0, 1, "world_flash_1f"),
            (0.1, 0, "world_flash_100ms"),
            (0.6, 0, "world_shell_600ms"),
        ],
    },
    Step {
        at: 9.5,
        third_person: false,
        target: Aim::Pawn,
        fire: 0.0,
        shots: &[(0.0, 1, "blood_1f"), (0.1, 0, "blood_100ms"), (0.3, 0, "blood_300ms")],
    },
    Step { at: 11.0, third_person: false, target: Aim::Pawn, fire: 0.6, shots: &[(0.7, 0, "blood_burst_end")] },
];

const END: f32 = 13.5;

fn drive(
    time: Res<Time>,
    mut test: ResMut<FxTest>,
    mut third: ResMut<crate::wardrobe::ThirdPerson>,
    mut player: Query<(&Transform, &Pawn, &mut ViewAngles, &mut WeaponInput), With<LocalPlayer>>,
    pawns: Query<(&Transform, &Pawn), (Without<LocalPlayer>, Without<Dead>)>,
    mut exit: MessageWriter<AppExit>,
) {
    let t = time.elapsed_secs();
    if t > END {
        exit.write(AppExit::Success);
        return;
    }
    let Ok((feet, me, mut view, mut input)) = player.single_mut() else { return };
    // The step under way: from its start until the next one's.
    let Some(i) = STEPS.iter().rposition(|s| t >= s.at - 0.4) else { return };
    let step = &STEPS[i];
    if third.0[0] != step.third_person {
        third.0[0] = step.third_person;
    }
    match step.target {
        Aim::Down(deg) => view.pitch = -deg.to_radians(),
        Aim::Pawn => {
            let eye = feet.translation + Vec3::Y * u(60.0);
            // Enemies first, then the nearest.
            let key = |(tf, p): &(&Transform, &Pawn)| (p.team == me.team, tf.translation.distance(eye));
            let nearest = pawns
                .iter()
                .min_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(tf, _)| tf.translation);
            if let Some(at) = nearest {
                let d = (at + Vec3::Y * u(45.0) - eye).normalize_or_zero();
                view.yaw = (-d.x).atan2(-d.z);
                view.pitch = d.y.asin();
            }
        }
    }
    if test.step != i + 1 && t >= step.at {
        test.step = i + 1;
        test.shot_at = None;
        test.pending = step.shots.iter().map(|&(s, f, n)| (s, f, n.to_owned())).collect();
    }
    let holding = if step.fire > 0.0 { t < step.at + step.fire } else { test.shot_at.is_none() };
    input.fire = t >= step.at && holding;
    input.ads = false;
}

fn capture(
    mut commands: Commands,
    time: Res<Time>,
    mut test: ResMut<FxTest>,
    mut shots: MessageReader<ShotFired>,
    mut killed: MessageReader<crate::combat::Killed>,
    local: Query<(), With<LocalPlayer>>,
) {
    let t = time.elapsed_secs();
    let fired = shots.read().any(|s| local.contains(s.shooter));
    // A kill: its exit wound, from the moment it happens.
    if killed.read().any(|k| k.attacker.is_some_and(|a| local.contains(a))) {
        test.shot_at = Some(t);
        test.frames = 0;
        test.pending =
            vec![(0.0, 1, "kill_1f".to_owned()), (0.08, 0, "kill_80ms".to_owned()), (0.25, 0, "kill_250ms".to_owned())];
    }
    if fired && test.shot_at.is_none() && !test.pending.is_empty() {
        test.shot_at = Some(t);
        test.frames = 0;
    } else if test.shot_at.is_some() {
        test.frames += 1;
    }
    let Some(at) = test.shot_at else { return };
    let (since, frames) = (t - at, test.frames);
    let dir = test.dir.clone();
    test.pending.retain(|(secs, f, name)| {
        if since + 1e-4 < *secs || frames < *f {
            return true;
        }
        std::fs::create_dir_all(&dir).ok();
        info!("fx test: {name} ({since:.3} s, {frames} frames after the shot)");
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.join(format!("{name}.png"))));
        false
    });
}

/// Debug aid: with `COD4RW_FXLOG`, log what the effects system holds every
/// two seconds, with the frame rate.
fn stats(
    time: Res<Time>,
    fx: Option<ResMut<super::Effects>>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    mut next: Local<f32>,
) {
    let t = time.elapsed_secs();
    let Some(mut fx) = fx.filter(|_| t >= *next) else { return };
    let (busy, frames) = std::mem::take(&mut fx.busy);
    let per_frame = busy.as_secs_f64() * 1000.0 / frames.max(1) as f64;
    *next = t + 2.0;
    let fps =
        diagnostics.get(&bevy::diagnostic::FrameTimeDiagnosticsPlugin::FPS).and_then(|d| d.smoothed()).unwrap_or(0.0);
    let live = fx.instances.iter().flatten().count();
    let drawing = fx.batches.values().filter(|b| !b.empty).count();
    info!(
        "fx: {} particles ({} models), {live} effects playing, {} effects and {} materials loaded, {drawing}/{} batches drawing, {} marks, {per_frame:.2} ms a frame, {fps:.0} fps",
        fx.particles.len(),
        fx.model_count,
        fx.defs.len(),
        fx.materials.len(),
        fx.batches.len(),
        fx.decals.len()
    );
}

#[derive(Resource)]
struct FxPlay(std::path::PathBuf);

/// Debug aid: with `COD4RW_FXAT=<dir>`, stand at `COD4RW_FXAT_VIEW` (`x y z
/// yaw pitch`, CoD feet and degrees, as bug reports give them), play
/// `COD4RW_FXAT_EFFECT` where the screen point `COD4RW_FXAT_SCREEN` (`u v`,
/// 0..1 from the top left; the middle by default) meets the world, facing
/// up (as a grenade's), and screenshot it at 0.1, 0.25, 0.45, 0.7, 1, 3, 6, 9, 10, 11 and 12 s
/// (`<milliseconds>ms.png`), then exit. With `COD4RW_FXAT_REPEAT=<per second>`
/// it plays again and again for the first 8 s, facing out of the surface
/// (like a burst of bullet impacts), at points a little apart. With
/// `COD4RW_FXAT_FROM=<x y z>` (CoD) it rides something flying from there to
/// that point at a rocket's speed instead (a trail), and goes on arrival.
#[derive(Resource)]
struct FxAt(std::path::PathBuf);

/// Where `COD4RW_FXAT_FROM`'s flier is going.
#[derive(Component)]
struct FlyTo(Vec3);

#[allow(clippy::too_many_arguments)]
fn play_at(
    mut commands: Commands,
    time: Res<Time>,
    test: Res<FxAt>,
    fx: Option<ResMut<super::Effects>>,
    mut player: Query<(&mut ViewAngles, &mut Transform, &mut crate::movement::Mover), With<LocalPlayer>>,
    camera: Query<(&Camera, &GlobalTransform), With<crate::player::MainCamera>>,
    spatial: avian3d::prelude::SpatialQuery,
    mut started: Local<Option<f32>>,
    mut taken: Local<usize>,
    mut repeat: Local<(Vec3, Vec3, f32)>,
    surfaces: Query<&crate::collision::Surfaces>,
    mut flying: Query<(Entity, &mut Transform, &FlyTo), Without<LocalPlayer>>,
    mut exit: MessageWriter<AppExit>,
) {
    const SHOTS: [f32; 11] = [0.1, 0.25, 0.45, 0.7, 1.0, 3.0, 6.0, 9.0, 10.0, 11.0, 12.0];
    let t = time.elapsed_secs();
    let numbers = |var: &str| -> Vec<f32> {
        std::env::var(var).unwrap_or_default().split_whitespace().filter_map(|v| v.parse().ok()).collect()
    };
    let Ok((mut view, mut feet, mut mover)) = player.single_mut() else { return };
    if let [x, y, z, yaw, pitch] = numbers("COD4RW_FXAT_VIEW")[..] {
        feet.translation = crate::units::pos([x, y, z]);
        mover.velocity = Vec3::ZERO;
        view.yaw = crate::units::yaw_from_cod_degrees(yaw);
        view.pitch = -pitch.to_radians();
    }
    if t < 6.0 {
        return;
    }
    let Some(start) = *started else {
        let (Some(mut fx), Ok((cam, g))) = (fx, camera.single()) else { return };
        let screen = numbers("COD4RW_FXAT_SCREEN");
        let (su, sv) = if let [a, b] = screen[..] { (a, b) } else { (0.5, 0.5) };
        let Some(size) = cam.logical_viewport_size() else { return };
        let Ok(ray) = cam.viewport_to_world(g, Vec2::new(su * size.x, sv * size.y)) else { return };
        let filter = crate::collision::sight_filter();
        let Some(hit) = spatial.cast_ray(ray.origin, ray.direction, u(8000.0), true, &filter) else { return };
        let at = ray.origin + *ray.direction * hit.distance + hit.normal * u(2.0);
        let name = std::env::var("COD4RW_FXAT_EFFECT").unwrap_or_else(|_| "props/american_smoke_grenade_mp".into());
        let surface = surfaces.get(hit.entity).map_or("-", |s| s.facing(hit.normal));
        info!("fx at: {name} at CoD {:?} (surface {surface}, normal {:?})", crate::units::to_cod(at).map(f32::round), hit.normal);
        if let [x, y, z] = numbers("COD4RW_FXAT_FROM")[..] {
            let from = crate::units::pos([x, y, z]);
            let e = commands.spawn((Name::new("fx at flier"), FlyTo(at), Transform::from_translation(from).looking_at(at, Vec3::Y))).id();
            fx.play(&name, super::Anchor::Bolted(e), super::FxLayer::World);
        } else {
            fx.play(&name, super::Anchor::Fixed(super::Frame::facing(at, Vec3::Y, 0.0)), super::FxLayer::World);
        }
        *repeat = (at, hit.normal, t);
        *started = Some(t);
        return;
    };
    // A rocket's speed (`rpg_mp`'s `iProjectileSpeed`, about 1000 a second).
    for (e, mut tf, to) in &mut flying {
        let step = u(1000.0) * time.delta_secs();
        let left = to.0 - tf.translation;
        if left.length() <= step {
            // Gone on arrival, as a rocket is when it goes off.
            commands.entity(e).despawn();
        } else {
            tf.translation += left.normalize() * step;
        }
    }
    let rate: f32 = std::env::var("COD4RW_FXAT_REPEAT").ok().and_then(|r| r.parse().ok()).unwrap_or(0.0);
    if rate > 0.0 && t - start < 8.0 && t >= repeat.2 + 1.0 / rate {
        if let Some(mut fx) = fx {
            use rand::Rng;
            let mut rng = rand::rng();
            let (at, normal, _) = *repeat;
            let side = normal.any_orthonormal_vector();
            let up = normal.cross(side);
            let spot = at + side * u(rng.random_range(-12.0..12.0)) + up * u(rng.random_range(-12.0..12.0));
            let name = std::env::var("COD4RW_FXAT_EFFECT").unwrap_or_default();
            fx.play(&name, super::Anchor::Fixed(super::Frame::facing(spot, normal, rng.random_range(0.0..6.28))), super::FxLayer::World);
            repeat.2 = t;
        }
    }
    match SHOTS.get(*taken) {
        Some(&after) if t >= start + after => {
            *taken += 1;
            std::fs::create_dir_all(&test.0).ok();
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.0.join(format!("{:04}ms.png", (after * 1000.0) as u32))));
        }
        None if t >= start + SHOTS[SHOTS.len() - 1] + 1.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

/// Where an effect plays: where the view meets the world, or on something
/// flying across the view.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    Look,
    Flying,
}

/// (start, effect, place, screenshots as (seconds after, name)).
const PLAYS: &[(f32, &str, Place, &[(f32, &str)])] = &[
    (6.0, "impacts/small_glass", Place::Look, &[(0.08, "glass_80ms"), (0.2, "glass_200ms")]),
    (7.0, "impacts/flesh_hit_body_fatal_exit", Place::Look, &[(0.15, "exit_150ms"), (0.6, "exit_600ms")]),
    (8.0, "impacts/large_brick", Place::Look, &[(0.3, "brick_300ms"), (0.9, "brick_900ms")]),
    (9.5, "impacts/large_mud", Place::Look, &[(0.5, "mud_500ms")]),
    (10.5, "smoke/smoke_geotrail_rpg", Place::Flying, &[(2.5, "rpg_trail_2500ms"), (5.0, "rpg_trail_5000ms")]),
];

/// Something flying across the view for a trail to follow.
#[derive(Component)]
struct Flying(Vec3);

#[allow(clippy::too_many_arguments)]
fn play(
    mut commands: Commands,
    time: Res<Time>,
    test: Res<FxPlay>,
    fx: Option<ResMut<super::Effects>>,
    mut player: Query<&mut ViewAngles, With<LocalPlayer>>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    mut flying: Query<(&mut Transform, &Flying)>,
    spatial: avian3d::prelude::SpatialQuery,
    mut step: Local<usize>,
    mut shots: Local<Vec<(f32, String)>>,
    mut flier: Local<Option<Entity>>,
    mut exit: MessageWriter<AppExit>,
) {
    let t = time.elapsed_secs();
    for (mut tf, f) in &mut flying {
        tf.translation += f.0 * time.delta_secs();
    }
    shots.retain(|(at, name)| {
        if t < *at {
            return true;
        }
        std::fs::create_dir_all(&test.0).ok();
        info!("fx play: {name}");
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.0.join(format!("{name}.png"))));
        false
    });
    let (Some(mut fx), Ok(mut view), Ok(cam)) = (fx, player.single_mut(), camera.single()) else { return };
    let Some(&(start, name, place, pics)) = PLAYS.get(*step) else {
        // A second after the last screenshot, so it's saved.
        let end = PLAYS.iter().flat_map(|p| p.3.iter().map(move |s| p.0 + s.0)).fold(0.0, f32::max) + 1.0;
        if shots.is_empty() && t > end {
            exit.write(AppExit::Success);
        }
        return;
    };
    // Down at the floor ahead, or level for the trail: as the step under
    // way wants until just before the next.
    let current = PLAYS.iter().rposition(|p| t + 0.4 >= p.0).map_or(place, |i| PLAYS[i].2);
    view.pitch = if current == Place::Look { -(35f32.to_radians()) } else { 0.0 };
    // The flier exists a moment before its trail (an effect bolted to
    // something not yet spawned doesn't play).
    if place == Place::Flying && flier.is_none() && t >= start - 0.2 {
        let from = cam.translation() + cam.forward() * u(250.0) - cam.right() * u(250.0);
        let e = commands.spawn((Name::new("fx play flier"), Flying(cam.right() * u(100.0)), Transform::from_translation(from))).id();
        *flier = Some(e);
    }
    if t < start {
        return;
    }
    *step += 1;
    match place {
        Place::Look => {
            let (eye, ahead) = (cam.translation(), cam.forward());
            let hit = spatial.cast_ray(eye, ahead, u(2000.0), true, &crate::collision::sight_filter());
            let Some(hit) = hit else { return };
            let at = eye + ahead * hit.distance;
            fx.play(name, super::Anchor::Fixed(super::Frame::facing(at, hit.normal, 0.0)), super::FxLayer::World);
        }
        Place::Flying => {
            let Some(mover) = *flier else { return };
            fx.play(name, super::Anchor::Bolted(mover), super::FxLayer::World);
        }
    }
    shots.extend(pics.iter().map(|&(after, n)| (start + after, n.to_owned())));
}
