//! More life about Headquarters, from CoD4's other missions: soldiers on a
//! bored patrol (stopping for a smoke, a phone check, a stretch; ambush), a
//! pair walking and talking (coup), a smoker leaning on the hangar, two
//! playing chess at a table and a guard asleep in his chair (blackout), and
//! a dog sitting by the door (ambush), all in the training level's SAS kit.
//!
//! The missions' zones load in the background once Headquarters is up; as
//! each arrives only the animations and props named here are kept, its
//! scenes appear, and the zone goes.

use crate::characters::Game;
use crate::content::Content;
use crate::models::{AnimPlayer, Skeleton, SpawnModel, spawn_model};
use crate::units::u;
use crate::wardrobe::{Source, load};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use iw3::xanim::XAnim;
use std::sync::Arc;
use std::thread::JoinHandle;

pub(super) fn build(app: &mut App) {
    app.add_systems(OnEnter(crate::state::GameState::InGame), start.after(super::spawn_figures).in_set(crate::state::Setup::Spawn))
        .add_systems(Update, (arrive, patrol).run_if(|| super::active()))
        .add_systems(OnExit(crate::state::GameState::InGame), |mut c: Commands| c.remove_resource::<Loading>());
}

/// The zones on their way.
#[derive(Resource)]
struct Loading(Vec<(&'static str, JoinHandle<anyhow::Result<crate::wardrobe::load::Loaded>>, f32)>);

const ZONES: [&str; 3] = ["ambush", "blackout", "coup"];

/// SAS woodland kit (the training level's), by head.
const BODY: &str = "body_sp_sas_woodland_assault_a";
const HEADS: [&str; 5] = ["head_sp_sas_woodland_hugh", "head_sp_sas_woodland_zied", "head_sp_sas_woodland_peter", "head_sp_sas_woodland_todd", "head_sp_sas_woodland_mac"];

fn start(mut commands: Commands, hq: Option<Res<super::Headquarters>>, time: Res<Time>) {
    if hq.is_none() || std::env::var("COD4RW_PERF").is_ok_and(|p| p.split(',').any(|o| o.trim() == "noambient")) {
        return;
    }
    let now = time.elapsed_secs();
    commands.insert_resource(Loading(ZONES.iter().map(|&z| (z, load(Source::zone(Game::Cod4, z)), now)).collect()));
}

/// A scene: who, where, doing what.
struct Scene {
    zone: &'static str,
    /// CoD units, degrees.
    at: [f32; 3],
    yaw: f32,
    /// Its people's animations (one person each; several are cycled).
    people: &'static [&'static [&'static str]],
    /// Props from the zone: model, offset (forward, left, up) and turn.
    props: &'static [(&'static str, [f32; 3], f32)],
    /// The people carry rifles.
    armed: bool,
    /// A dog instead of a soldier.
    dog: bool,
    /// Walking a beat: its far end (CoD units).
    patrol: Option<[f32; 2]>,
}

const SCENES: &[Scene] = &[
    // Two at chess at a tent table, the board on it (blackout's chess_ent).
    Scene {
        zone: "blackout",
        at: [350.0, -850.0, 4.0],
        yaw: 90.0,
        people: &[&["parabolic_chessgame_idle_a"], &["parabolic_chessgame_idle_b"]],
        props: &[("bc_militarytent_wood_table", [-5.9, -1.7, -2.0], 0.0), ("com_chess", [-9.1, -2.1, 44.0], 0.0)],
        armed: false,
        dog: false,
        patrol: None,
    },
    // Asleep in a chair (blackout's shack sleeper), his back to the
    // sawhorse tables, facing the room (facing them, the tables hid him).
    Scene {
        zone: "blackout",
        at: [-150.0, -805.0, 4.0],
        yaw: -90.0,
        people: &[&["parabolic_guard_sleeper_idle"]],
        props: &[("com_cafe_chair", [8.0, 0.0, 0.0], 180.0)],
        armed: false,
        dog: false,
        patrol: None,
    },
    // A smoke against the hangar's front.
    Scene {
        zone: "blackout",
        at: [-495.0, -1060.0, 4.0],
        yaw: 0.0,
        people: &[&["parabolic_leaning_guy_smoking_idle", "parabolic_leaning_guy_smoking_twitch"]],
        props: &[],
        armed: true,
        dog: false,
        patrol: None,
    },
    // The dog sits by him.
    Scene { zone: "ambush", at: [-440.0, -1110.0, 4.0], yaw: 20.0, people: &[&["german_shepherd_sitting_idle", "german_shepherd_sitting_looking_idle", "german_shepherd_sitting_look_left", "german_shepherd_sitting_look_right"]], props: &[], armed: false, dog: true, patrol: None },
    // On a bored patrol along the tarmac.
    Scene {
        zone: "ambush",
        at: [-200.0, -1900.0, 4.0],
        yaw: 0.0,
        people: &[&["patrol_bored_patrolwalk", "patrol_bored_idle", "patrol_bored_idle_smoke", "patrol_bored_idle_cellphone", "patrol_bored_twitch_stretch", "patrol_bored_twitch_checkphone", "patrol_bored_walk_2_bored", "patrol_bored_2_walk"]],
        props: &[],
        armed: true,
        dog: false,
        patrol: Some([900.0, -1900.0]),
    },
    // Two walking and talking (coup's talking patrol).
    Scene {
        zone: "coup",
        at: [1200.0, -1450.0, 4.0],
        yaw: 180.0,
        people: &[&["coup_talking_patrol_guy1"], &["coup_talking_patrol_guy2"]],
        props: &[],
        armed: true,
        dog: false,
        patrol: Some([-100.0, -1450.0]),
    },
];

/// A beat walked: its ends, which way now, and the animations.
#[derive(Component)]
struct Beat {
    ends: [Vec3; 2],
    to: usize,
    anims: Vec<Arc<XAnim>>,
    /// Stopped (an idle) until then.
    rest_until: f32,
    /// Moving at this pace (units a second), from the walk's own.
    pace: f32,
    yaw: f32,
    talking: bool,
}

/// Several idles: one now, the next later.
#[derive(Component)]
struct Cycle {
    anims: Vec<Arc<XAnim>>,
    next_at: f32,
}

fn ground(spatial: &avian3d::prelude::SpatialQuery, at: Vec3) -> Vec3 {
    let from = at + Vec3::Y * u(80.0);
    match spatial.cast_ray(from, Dir3::NEG_Y, u(300.0), true, &crate::collision::movement_filter()) {
        Some(h) => Vec3::new(at.x, from.y - h.distance, at.z),
        None => at,
    }
}

/// A zone in: its scenes made, then it's dropped.
#[allow(clippy::too_many_arguments)]
fn arrive(
    mut commands: Commands,
    time: Res<Time>,
    loading: Option<ResMut<Loading>>,
    mut content: ResMut<Content>,
    spatial: avian3d::prelude::SpatialQuery,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    (mut camo_materials, mut camos): (ResMut<Assets<crate::gunmodel::CamoMaterial>>, ResMut<crate::gunmodel::WorldCamos>),
) {
    let Some(mut loading) = loading else { return };
    let Some(i) = loading.0.iter().position(|(_, h, _)| h.is_finished()) else { return };
    let (zone, handle, started) = loading.0.remove(i);
    if loading.0.is_empty() {
        commands.remove_resource::<Loading>();
    }
    let t0 = std::time::Instant::now();
    let (zones, vfs) = match handle.join() {
        Ok(Ok(z)) => z,
        _ => {
            warn!("headquarters: {zone} didn't load");
            return;
        }
    };
    let mut theirs = Content::new(zones, vfs.unwrap_or_else(|| content.vfs.clone()));
    let now = time.elapsed_secs();
    let mut count = 0;
    for (k, scene) in SCENES.iter().enumerate().filter(|(_, s)| s.zone == zone) {
        let base = crate::units::pos(scene.at);
        let turn = Quat::from_rotation_y(scene.yaw.to_radians());
        let place = ground(&spatial, base);
        for &(model, [f, l, up], yaw) in scene.props {
            let Some(m) = theirs.model(model, &mut meshes, &mut materials, &mut images, &mut bindposes) else { continue };
            let offset = turn * crate::units::pos([f, l, up]);
            let e = commands
                .spawn((
                    super::HqFigure,
                    Name::new(format!("hq prop {model}")),
                    Transform::from_translation(place + offset).with_rotation(turn * Quat::from_rotation_y(yaw.to_radians())),
                    Visibility::default(),
                ))
                .id();
            let mut skeleton = Skeleton::default();
            spawn_model(&mut commands, &mut skeleton, SpawnModel { model: &m, owner: e, attach_to: None, layers: None, shadows: true });
            commands.entity(e).insert(skeleton);
        }
        for (p, names) in scene.people.iter().enumerate() {
            let anims: Vec<Arc<XAnim>> = names.iter().filter_map(|n| theirs.anim(n)).collect();
            if anims.is_empty() {
                warn!("headquarters: no {names:?} in {zone}");
                continue;
            }
            let models: Vec<_> = if scene.dog {
                theirs.model("german_sheperd_dog", &mut meshes, &mut materials, &mut images, &mut bindposes).into_iter().collect()
            } else {
                [BODY, HEADS[(k + p) % HEADS.len()]].iter().filter_map(|m| content.model(m, &mut meshes, &mut materials, &mut images, &mut bindposes)).collect()
            };
            if models.is_empty() {
                continue;
            }
            // A pair side by side on a beat; otherwise all at the scene's place.
            let side = turn * Vec3::Z * u(28.0) * p as f32;
            let owner = commands
                .spawn((super::HqFigure, Name::new(format!("hq life {zone} {k}.{p}")), Transform::from_translation(place + side).with_rotation(turn), Visibility::default()))
                .id();
            let mut skeleton = Skeleton::default();
            for m in &models {
                spawn_model(&mut commands, &mut skeleton, SpawnModel { model: m, owner, attach_to: None, layers: None, shadows: true });
            }
            if scene.armed {
                let hand = skeleton.joint("tag_weapon_right");
                let mut assets = crate::gunmodel::GunAssets { meshes: &mut meshes, materials: &mut materials, images: &mut images, bindposes: &mut bindposes, camo_materials: &mut camo_materials };
                let target = crate::gunmodel::GunTarget { owner, attach_to: hand, layers: None };
                crate::gunmodel::spawn_held_gun(&mut commands, &mut content, &mut camos.0, &mut assets, &mut skeleton, "m4:", 0, target);
            }
            let mut player = AnimPlayer::default();
            player.play(anims[0].clone(), 0.0);
            player.time = (k * 3 + p) as f32 * 1.7 % anims[0].duration().max(0.1);
            commands.entity(owner).insert((skeleton, player));
            match scene.patrol {
                Some(end) => {
                    let pace = own_pace(&anims[0]).unwrap_or(u(50.0));
                    let far = ground(&spatial, crate::units::pos([end[0], end[1], scene.at[2]])) + side;
                    let talking = names.len() == 1;
                    commands.entity(owner).insert(Beat { ends: [place + side, far], to: 1, anims, rest_until: 0.0, pace, yaw: (scene.yaw).to_radians(), talking });
                }
                None if anims.len() > 1 => {
                    commands.entity(owner).insert(Cycle { anims, next_at: now + rand::random_range(4.0..10.0) });
                }
                None => {}
            }
            count += 1;
        }
    }
    info!("headquarters: {zone} here {:.1} s after the start, {count} people ({:.0} ms to set out)", now - started, t0.elapsed().as_secs_f32() * 1000.0);
}

/// An animation's pace over the ground (units a second).
fn own_pace(anim: &XAnim) -> Option<f32> {
    let d = anim.delta_at(1.0);
    let own = Vec2::new(d[0], d[1]).length() / anim.duration().max(1e-3);
    (anim.delta && own > 5.0).then(|| u(own))
}

/// Beats walked, and idles cycled.
fn patrol(
    time: Res<Time>,
    spatial: avian3d::prelude::SpatialQuery,
    mut beats: Query<(&mut Beat, &mut Transform, &mut AnimPlayer), Without<Cycle>>,
    mut cycles: Query<(&mut Cycle, &mut AnimPlayer), Without<Beat>>,
) {
    let _t = crate::perf::Probe::start("hq patrol");
    let now = time.elapsed_secs();
    let dt = time.delta_secs().min(0.1);
    let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    for (mut c, mut player) in &mut cycles {
        if now < c.next_at {
            continue;
        }
        let pick = if rand::random_bool(0.4) { rand::random_range(1..c.anims.len()) } else { 0 };
        let a = c.anims[pick].clone();
        c.next_at = now + a.duration().max(1.0) * if pick == 0 { 2.0 } else { 1.0 };
        player.play(a, 0.4);
    }
    for (mut b, mut tf, mut player) in &mut beats {
        let walk = b.anims[0].clone();
        if now < b.rest_until {
            continue;
        }
        let to = b.ends[b.to];
        let flat = (to - tf.translation).with_y(0.0);
        if flat.length() < u(12.0) {
            // At the end: (the bored patrol) a stop and an idle a while,
            // then back; (the talking pair) straight back.
            b.to = 1 - b.to;
            if !b.talking && b.anims.len() > 2 {
                let idle = b.anims[1 + rand::random_range(0..(b.anims.len() - 3).max(1))].clone();
                b.rest_until = now + idle.duration().clamp(4.0, 12.0);
                player.play(idle, 0.5);
            }
            continue;
        }
        let dir = flat.normalize();
        let want = (-dir.z).atan2(dir.x);
        b.yaw = wrap(b.yaw + wrap(want - b.yaw) * (1.0 - (-3.0 * dt).exp()));
        // Walk once turned most of the way.
        let facing = wrap(want - b.yaw).abs() < 0.6;
        if facing {
            let mut at = tf.translation + dir * b.pace * dt;
            at = ground(&spatial, at);
            tf.translation = tf.translation.lerp(at, 1.0);
        }
        tf.rotation = Quat::from_rotation_y(b.yaw);
        if facing {
            player.ensure(walk, 0.5);
        }
    }
}

