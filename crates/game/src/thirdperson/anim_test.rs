//! Test runs for the bodies' animation: `COD4RW_TPANIM=<dir>` spawns a
//! pawn that runs a fixed routine (idle, walk, run, sprint, stop, turn on
//! the spot, crouch, stand up running, backpedal, strafe, stop), films it
//! from the side with the camera keeping pace, saves a frame every 0.1 s
//! (`f000.png`...) and logs, per step of the routine:
//! - foot slide: how fast the lower foot moves over the ground while it's
//!   down (under 1.6 units), the median (units/s);
//! - pops: the largest jump in any joint's turn from one frame to the next,
//!   beyond what its turn the frame before would carry it (deg/s).
//!
//! `COD4RW_TPANIM_AT=x,y,z,yaw` (CoD units, degrees) puts it somewhere
//! flat and open; otherwise it starts 4 m ahead of the player's spawn,
//! going to the player's right. It exits when done.

use super::Body;
use crate::combat::Team;
use crate::models::Skeleton;
use crate::movement::{MoveInput, Mover, Stance, ViewAngles};
use crate::units::u;
use bevy::prelude::*;

pub(super) fn build(app: &mut App) {
    if std::env::var_os("COD4RW_TPANIM").is_none() {
        return;
    }
    // A steady 60 frames a second of game time, however slow the frames.
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / 60.0)));
    app.add_systems(Update, drive.run_if(crate::state::in_game).before(crate::movement::MovementSet))
        .add_systems(
            PostUpdate,
            film.after(crate::wardrobe::place_camera).after(crate::player::follow_camera).before(bevy::transform::TransformSystems::Propagate).run_if(crate::state::in_game),
        )
        .add_systems(Last, measure.run_if(crate::state::in_game));
}

/// One step: how long, the move keys, stance, sprint, aiming, and the view's
/// turn from the way along (degrees, yaw growing left); `turn` sweeps there
/// and back over the step.
struct Step {
    name: &'static str,
    secs: f32,
    forward: f32,
    right: f32,
    stance: Stance,
    sprint: bool,
    ads: bool,
    view: f32,
    turn: f32,
}

const fn step(name: &'static str, secs: f32, forward: f32, right: f32, stance: Stance) -> Step {
    Step { name, secs, forward, right, stance, sprint: false, ads: false, view: 0.0, turn: 0.0 }
}

/// Out along the way, round, and back (so it needs about 20 m).
const ROUTINE: [Step; 13] = [
    step("idle", 1.0, 0.0, 0.0, Stance::Stand),
    Step { ads: true, ..step("walk", 1.6, 1.0, 0.0, Stance::Stand) },
    step("run", 1.6, 1.0, 0.0, Stance::Stand),
    Step { sprint: true, ..step("sprint", 1.2, 1.0, 0.0, Stance::Stand) },
    step("stop", 1.2, 0.0, 0.0, Stance::Stand),
    Step { turn: 100.0, ..step("turn", 2.0, 0.0, 0.0, Stance::Stand) },
    Step { turn: 180.0, ..step("about face", 1.2, 0.0, 0.0, Stance::Stand) },
    Step { view: 180.0, ..step("crouch run", 1.6, 1.0, 0.0, Stance::Crouch) },
    Step { view: 180.0, ..step("crouch stop", 1.0, 0.0, 0.0, Stance::Crouch) },
    Step { view: 180.0, ..step("stand running", 1.4, 1.0, 0.0, Stance::Stand) },
    Step { view: 180.0, ..step("backpedal", 1.6, -1.0, 0.0, Stance::Stand) },
    Step { view: 90.0, ..step("strafe", 1.6, 0.0, -1.0, Stance::Stand) },
    Step { view: 90.0, ..step("stop", 1.2, 0.0, 0.0, Stance::Stand) },
];

/// Before the routine: time to settle once spawned.
const LEAD_IN: f32 = 1.5;
const FRAME_EVERY: f32 = 0.1;
/// The camera: this far to the side, this high, looking at the hips.
const CAMERA_SIDE: f32 = 4.0;
const CAMERA_UP: f32 = 1.0;

#[derive(Component)]
struct Actor {
    /// The way along (yaw).
    heading: f32,
}

#[derive(Default)]
struct Run {
    started: Option<f32>,
    actor: Option<Entity>,
}

fn step_at(t: f32) -> Option<(usize, f32)> {
    let mut at = 0.0;
    for (i, s) in ROUTINE.iter().enumerate() {
        if t < at + s.secs {
            return Some((i, t - at));
        }
        at += s.secs;
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    time: Res<Time>,
    assets: Option<Res<crate::combat::PawnAssets>>,
    players: Query<(Entity, &Transform, &ViewAngles), With<crate::player::LocalPlayer>>,
    mut actors: Query<(&Actor, &mut MoveInput, &mut ViewAngles, Option<&mut crate::weapons::WeaponInput>, Option<&crate::weapons::WeaponState>), Without<crate::player::LocalPlayer>>,
    mut run: Local<Run>,
    mut exit: MessageWriter<AppExit>,
    spatial: avian3d::prelude::SpatialQuery,
    map: Option<Res<crate::world::MapInfo>>,
) {
    let now = time.elapsed_secs();
    let Some(actor) = run.actor else {
        let (Some(assets), Ok((player, at, view))) = (assets, players.single()) else { return };
        let (pos, heading) = match std::env::var("COD4RW_TPANIM_AT").ok().map(|v| v.split(',').filter_map(|x| x.trim().parse::<f32>().ok()).collect::<Vec<_>>()) {
            Some(v) if v.len() >= 4 => (crate::units::pos([v[0], v[1], v[2]]), v[3].to_radians()),
            _ => {
                // The spawn point and way with the most room (of 16), at
                // knee and chest height: along its line, the camera's and
                // between, and nothing in the way of the camera.
                let filter = crate::collision::movement_filter();
                let room = |pos: Vec3, yaw: f32| {
                    let Ok(dir) = Dir3::new(Quat::from_rotation_y(yaw) * Vec3::NEG_Z) else { return 0.0 };
                    let side = Quat::from_rotation_y(yaw) * Vec3::X;
                    let ahead = [0.0, 0.5, 1.0]
                        .iter()
                        .flat_map(|k| [0.4, 1.2].map(|h| pos + side * CAMERA_SIDE * *k + Vec3::Y * h))
                        .map(|from| spatial.cast_ray(from, dir, 45.0, true, &filter).map_or(45.0, |hit| hit.distance))
                        .fold(f32::MAX, f32::min);
                    let Ok(across) = Dir3::new(side) else { return 0.0 };
                    (0..ahead as i32)
                        .find(|m| {
                            let at = pos + *dir * *m as f32;
                            // Flat: the floor within 30 cm of the start's.
                            let floor = spatial.cast_ray(at + Vec3::Y * 0.5, Dir3::NEG_Y, 2.0, true, &filter).map_or(9.0, |h| h.distance - 0.5);
                            floor.abs() > 0.3 || [0.4, 1.2].iter().any(|h| spatial.cast_ray(at + Vec3::Y * *h, across, CAMERA_SIDE, true, &filter).is_some())
                        })
                        .map_or(ahead, |m| m as f32)
                };
                let mut places: Vec<Vec3> = map.as_ref().map(|m| m.spawns.iter().map(|s| s.pos).collect()).unwrap_or_default();
                places.push(at.translation + Quat::from_rotation_y(view.yaw) * Vec3::NEG_Z * 4.0);
                let (pos, best, metres) = places
                    .iter()
                    .flat_map(|p| (0..16).map(move |i| (*p, i as f32 * std::f32::consts::TAU / 16.0)))
                    .map(|(p, yaw)| (p, yaw, room(p, yaw)))
                    .max_by(|a, b| a.2.total_cmp(&b.2))
                    .unwrap_or((at.translation, 0.0, 0.0));
                info!("tpanim: {metres:.0} m of room");
                (pos, best)
            }
        };
        let spawn = crate::world::SpawnPoint { pos, yaw: heading, kind: crate::world::SpawnKind::Tdm };
        let e = crate::combat::spawn_pawn(&mut commands, &assets, "actor", Team::Axis, &spawn);
        commands.entity(e).insert((Actor { heading }, crate::weapons::WeaponInput::default()));
        commands.entity(player).insert(crate::movement::Frozen);
        run.actor = Some(e);
        run.started = Some(now);
        info!("tpanim: actor at {:?}, heading {:.0}", crate::units::to_cod(pos), heading.to_degrees());
        return;
    };
    let Ok((a, mut mv, mut view, wi, weapon)) = actors.get_mut(actor) else { return };
    let t = now - run.started.unwrap_or(now) - LEAD_IN;
    let Some((i, into)) = step_at(t.max(0.0)) else {
        info!("tpanim: done");
        exit.write(AppExit::Success);
        return;
    };
    let s = &ROUTINE[i];
    let s = if t < 0.0 { &ROUTINE[0] } else { s };
    // The turn: out over the first 30% of the step, held, back over 30%.
    let sweep = if s.turn >= 180.0 {
        s.turn * (into / (s.secs * 0.6)).min(1.0)
    } else if s.turn != 0.0 {
        let k = into / s.secs;
        let x = if k < 0.3 { k / 0.3 } else if k < 0.5 { 1.0 } else if k < 0.8 { 1.0 - (k - 0.5) / 0.3 } else { 0.0 };
        s.turn * x
    } else {
        0.0
    };
    // Turning as a player would (fast, but not in one frame).
    let target = a.heading + (s.view + sweep).to_radians();
    let diff = (target - view.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    let most = 540f32.to_radians() * time.delta_secs();
    view.yaw += diff.clamp(-most, most);
    view.pitch = 0.0;
    // (Aiming slows it as the gun says.)
    let speed_scale = weapon.map_or(1.0, |w| w.speed_scale());
    *mv = MoveInput { forward: s.forward, right: s.right, sprint: s.sprint, stance: s.stance, speed_scale, ..default() };
    if let Some(mut wi) = wi {
        wi.ads = s.ads;
        wi.fire = false;
    }
}

/// The camera to the actor's right, keeping pace with it.
fn film(
    actors: Query<(&Actor, &Transform)>,
    mut camera: Query<&mut Transform, (With<crate::player::MainCamera>, Without<Actor>)>,
    mut ui: Query<&mut Visibility, (With<Node>, Without<ChildOf>)>,
) {
    // No HUD in the way.
    for mut v in &mut ui {
        *v = Visibility::Hidden;
    }
    let (Ok((a, at)), Ok(mut cam)) = (actors.single(), camera.single_mut()) else { return };
    let side = Quat::from_rotation_y(a.heading) * Vec3::X;
    let hips = at.translation + Vec3::Y * 0.9;
    let eye = at.translation + side * CAMERA_SIDE + Vec3::Y * CAMERA_UP;
    *cam = Transform::from_translation(eye).looking_at(hips, Vec3::Y);
}

#[derive(Default)]
struct Measure {
    next_shot: f32,
    shot: usize,
    step: Option<usize>,
    /// Per foot: last world position.
    feet: [Option<Vec3>; 2],
    /// Per joint: last turn (relative to the body) and its change then.
    turns: Vec<(Quat, f32)>,
    slides: Vec<f32>,
    pops: Vec<f32>,
    frames: u32,
    playing: String,
}

const FEET: [&str; 2] = ["j_ball_le", "j_ball_ri"];

#[allow(clippy::too_many_arguments)]
fn measure(
    mut commands: Commands,
    time: Res<Time>,
    actors: Query<(&Body, &Transform, &Mover), With<Actor>>,
    skeletons: Query<(&Skeleton, &GlobalTransform, &crate::models::AnimPlayer)>,
    globals: Query<&GlobalTransform>,
    mut m: Local<Measure>,
    mut started: Local<Option<f32>>,
) {
    let Ok((body, at, _mover)) = actors.single() else { return };
    let Ok((skel, owner, player)) = skeletons.get(body.0) else { return };
    let now = time.elapsed_secs();
    let t0 = *started.get_or_insert(now);
    let t = now - t0 - LEAD_IN;
    let dt = time.delta_secs().max(1e-4);
    let step = if t < 0.0 { None } else { step_at(t).map(|s| s.0) };
    if step != m.step {
        if let Some(i) = m.step {
            report(&ROUTINE[i], &mut m);
        }
        m.step = step;
        m.slides.clear();
        m.pops.clear();
        m.frames = 0;
    }
    // Frames to look at.
    if t >= -0.3 && now >= m.next_shot {
        let dir = std::env::var("COD4RW_TPANIM").unwrap_or_default();
        std::fs::create_dir_all(&dir).ok();
        let path = std::path::Path::new(&dir).join(format!("f{:03}.png", m.shot));
        commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(path));
        m.shot += 1;
        m.next_shot = now + FRAME_EVERY;
    }
    m.frames += 1;
    if let Some(a) = player.anim.as_ref().filter(|a| a.name != m.playing) {
        info!("tpanim   {t:.2} s: {} -> {}", m.playing, a.name);
        m.playing = a.name.clone();
    }
    // Halfway through a step: what's playing, and how fast.
    if let (Some(i), Some(a)) = (step, player.anim.as_ref()) {
        let (_, into) = step_at(t).unwrap_or((0, 0.0));
        if (into - ROUTINE[i].secs * 0.5).abs() < dt * 0.5 {
            let d = a.delta_at(1.0);
            let own = Vec2::new(d[0], d[1]).length() / a.duration().max(1e-3);
            info!("tpanim   {}: {} rate {:.2} own {:.0} u/s moving {:.0} u/s, {} frames {:.1}s delta {:?}", ROUTINE[i].name, a.name, player.speed, own, _mover.horizontal_speed() / u(1.0), a.num_frames, a.duration(), d);
        }
    }
    // Feet: the lower one, while on the ground (within 4 units of the
    // floor), how fast it moves over it.
    let floor = at.translation.y;
    let mut lowest: Option<(f32, f32)> = None;
    let mut row = format!("{t:.3},{}", step.map_or("", |i| ROUTINE[i].name));
    for (k, name) in FEET.iter().enumerate() {
        let Some(p) = skel.joint(name).and_then(|j| globals.get(j).ok()).map(|g| g.translation()) else { continue };
        if let Some(last) = m.feet[k] {
            let speed = ((p - last) / dt).with_y(0.0).length() / u(1.0);
            row += &format!(",{:.1},{:.0}", (p.y - floor) / u(1.0), speed);
            if lowest.is_none_or(|l| p.y < l.0) {
                lowest = Some((p.y, speed));
            }
        }
        m.feet[k] = Some(p);
    }
    {
        use std::io::Write;
        let dir = std::env::var("COD4RW_TPANIM").unwrap_or_default();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::path::Path::new(&dir).join("feet.csv")) {
            // And in the body's frame: the pelvis and the feet (x along its facing).
            let inv = owner.affine().inverse();
            let local = |name: &str| skel.joint(name).and_then(|j| globals.get(j).ok()).map_or(Vec3::ZERO, |g| inv.transform_point3(g.translation()) / u(1.0));
            let (pl, fl, fr) = (local("pelvis"), local("j_ball_le"), local("j_ball_ri"));
            let sc = owner.compute_transform().scale;
            writeln!(f, "{row},{:.0},{:.3},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.2}", _mover.horizontal_speed() / u(1.0), player.time, pl.x, pl.z, fl.x, fl.z, fr.x, fr.z, sc.x).ok();
        }
    }
    // (Under 1.6 units: a foot just lifting is already higher.)
    if let Some((_, speed)) = lowest.filter(|l| l.0 - floor < u(1.6)) {
        m.slides.push(speed);
    }
    // Pops: each joint's turn in the body's frame, against the last frame's.
    let inv = owner.affine().inverse();
    m.turns.resize(skel.joints.len(), (Quat::IDENTITY, -1.0));
    let mut worst = 0.0f32;
    let mut worst_at = "";
    for (i, j) in skel.joints.iter().enumerate() {
        let Ok(g) = globals.get(j.entity) else { continue };
        let (_, rot, _) = (inv * g.affine()).to_scale_rotation_translation();
        let (last, last_rate) = m.turns[i];
        let rate = if last_rate < 0.0 { 0.0 } else { last.angle_between(rot).to_degrees() / dt };
        if last_rate >= 0.0 {
            if rate - last_rate > worst {
                worst = rate - last_rate;
                worst_at = &j.name;
            }
        }
        m.turns[i] = (rot, rate);
    }
    if worst > 1500.0 {
        info!("tpanim   pop {worst:.0} deg/s at {t:.2} s in {}: {}", step.map_or("-", |i| ROUTINE[i].name), worst_at);
    }
    m.pops.push(worst);
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

fn report(s: &Step, m: &mut Measure) {
    m.pops.sort_by(f32::total_cmp);
    let p = |q: f32| m.pops.get(((m.pops.len() as f32 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
    let big = m.pops.iter().filter(|&&x| x > 1500.0).count();
    info!(
        "tpanim {:<14} slide p50 {:5.1} u/s  pops p90 {:5.0} max {:5.0} deg/s, {} over 1500 ({} frames)",
        s.name,
        median(&mut m.slides),
        p(0.9),
        p(1.0),
        big,
        m.frames
    );
}
