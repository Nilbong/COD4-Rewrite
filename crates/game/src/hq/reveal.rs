//! Supply drops in Headquarters. With drops to open, holding the Frag key
//! anywhere calls one in: a crate rises out of the floor in front of the
//! player, with dust and a thud. Use opens it: the camera frames it, it
//! rattles and bursts, and the drop's cards (the Supply Drops menu's own)
//! are dealt up over it one after another, rarer ones with a bigger sound.
//! Use again puts them away, and the crate sinks back into the floor.

use super::USE;
use crate::fx::{Anchor, Effects, Frame, FxLayer};
use crate::models::{Skeleton, SpawnModel, spawn_model};
use crate::supply::{Item, Rarity};
use crate::units::u;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const CRATE_MODEL: &str = "com_plasticcase_green_big";
/// How long the Frag key is held to call one in, how far ahead it comes
/// up (CoD units), and from how deep.
const CALL_HOLD: f32 = 0.6;
const CRATE_AHEAD: f32 = 90.0;
const CRATE_DEPTH: f32 = 34.0;
/// Its rise and sink, seconds.
const RISE: f32 = 0.8;
const SINK: f32 = 0.6;
/// How near Use opens it.
const REACH: f32 = 110.0;
/// The hangar's SAS this near the crate step out of the shot.
const CLEAR_AROUND: f32 = 220.0;
/// Opened: the rattle, then the cards, this far apart, each dealt up over
/// this long.
const RATTLE: f32 = 1.1;
const FIRST_CARD: f32 = 1.6;
const CARD_GAP: f32 = 1.0;
pub const DEAL: f32 = 0.35;
/// Face down a beat before turning (the suspense).
pub const FACE_DOWN: f32 = 0.35;
const CAMERA_MOVE: f32 = 0.8;
/// The light the crate's opened under.
const CRATE_LIGHT: f32 = 400_000.0;

/// The Frag key held ([`super::strip`]), and how far into the hold (0..1).
static CALL_HELD: AtomicBool = AtomicBool::new(false);
static HOLD: AtomicU32 = AtomicU32::new(0);
/// What's happening, for the prompts and the overlay ([`Phase`] as a u32).
static PHASE: AtomicU32 = AtomicU32::new(0);
/// The cards: each with whether it's new and how long since it was dealt.
static CARDS: Mutex<Vec<(Item, bool, Option<f32>)>> = Mutex::new(Vec::new());

pub(super) fn set_call_held(held: bool) {
    CALL_HELD.store(held, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    None = 0,
    Rising = 1,
    /// Up, waiting to be opened.
    Waiting = 2,
    Opening = 3,
    /// Every card out: Use puts it away.
    Done = 4,
    Sinking = 5,
}

pub fn phase() -> Phase {
    match PHASE.load(Ordering::Relaxed) {
        1 => Phase::Rising,
        2 => Phase::Waiting,
        3 => Phase::Opening,
        4 => Phase::Done,
        5 => Phase::Sinking,
        _ => Phase::None,
    }
}

fn set_phase(p: Phase) {
    PHASE.store(p as u32, Ordering::Relaxed);
}

/// How long a drop's been opening (seconds; 0 when none is).
static SINCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub fn reveal_since() -> f32 {
    f32::from_bits(SINCE.load(Ordering::Relaxed))
}

/// Is a drop being opened (the player held, the camera on the crate)?
pub fn revealing() -> bool {
    matches!(phase(), Phase::Opening | Phase::Done)
}

/// How far into calling one in the hold is (0..1).
pub fn hold() -> f32 {
    f32::from_bits(HOLD.load(Ordering::Relaxed))
}

/// The cards dealt so far: the item, whether it's new, seconds since it
/// was dealt (None: not yet).
pub fn cards() -> Vec<(Item, bool, Option<f32>)> {
    CARDS.lock().map(|c| c.clone()).unwrap_or_default()
}

/// Is the player by a crate that's up, waiting to be opened?
static NEAR_CRATE: AtomicBool = AtomicBool::new(false);
pub fn near_crate() -> bool {
    NEAR_CRATE.load(Ordering::Relaxed)
}

pub(super) fn build(app: &mut App) {
    app.add_systems(Update, run.after(super::stations).run_if(|| super::active()))
        .add_systems(
            PostUpdate,
            frame_camera
                .after(crate::wardrobe::place_camera)
                .before(bevy::transform::TransformSystems::Propagate)
                .run_if(|| revealing()),
        )
        .add_systems(OnExit(crate::state::GameState::InGame), end_all);
}

#[derive(Component)]
struct SupplyCrate;

/// A drop called in.
#[derive(Resource)]
struct Drop {
    entity: Entity,
    /// Where it stands once up, and its yaw.
    at: Vec3,
    yaw: f32,
    /// From the crate toward the player (the camera's side), level.
    front: Vec3,
    since: f32,
    items: Vec<(Item, bool)>,
    dealt: usize,
    burst: bool,
    /// Its light while open.
    light: Option<Entity>,
}

/// Calling one in, opening it, and putting it away.
#[allow(clippy::too_many_arguments)]
fn run(
    mut commands: Commands,
    time: Res<Time>,
    drop: Option<ResMut<Drop>>,
    player: Query<(Entity, &Transform, &crate::movement::ViewAngles, Option<&crate::thirdperson::Body>), With<crate::player::LocalPlayer>>,
    mut crates: Query<&mut Transform, (With<SupplyCrate>, Without<crate::player::LocalPlayer>)>,
    mut figures: Query<(&GlobalTransform, &mut Visibility), With<super::HqFigure>>,
    fe: Option<Res<crate::ui::Frontend>>,
    mut content: ResMut<crate::content::Content>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut effects: ResMut<Effects>,
    (mut meshes, mut materials, mut images, mut bindposes): (
        ResMut<Assets<Mesh>>,
        ResMut<Assets<StandardMaterial>>,
        ResMut<Assets<Image>>,
        ResMut<Assets<SkinnedMeshInverseBindposes>>,
    ),
    mut held_for: Local<f32>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let Ok((me, me_tf, view, body)) = player.single() else { return };
    let dust = |effects: &mut Effects, at: Vec3| {
        effects.play("impacts/large_brick", Anchor::Fixed(Frame::facing(at + Vec3::Y * u(6.0), Vec3::Y, 0.0)), FxLayer::World);
    };
    let Some(mut d) = drop else {
        // Hold the Frag key to call one in (not with a menu up, nor with
        // nothing to open).
        let menu = crate::ui::menu_open(fe.as_deref());
        let can = crate::supply::inventory().unopened > 0 && !menu;
        *held_for = if CALL_HELD.load(Ordering::Relaxed) && can { *held_for + dt } else { 0.0 };
        HOLD.store((*held_for / CALL_HOLD).min(1.0).to_bits(), Ordering::Relaxed);
        if *held_for < CALL_HOLD {
            return;
        }
        *held_for = 0.0;
        HOLD.store(0f32.to_bits(), Ordering::Relaxed);
        // Ahead of the player, on the floor they stand on, facing them.
        let ahead = Quat::from_rotation_y(view.yaw) * Vec3::NEG_Z;
        let ahead = ahead.with_y(0.0).normalize_or(Vec3::NEG_Z);
        let at = me_tf.translation + ahead * u(CRATE_AHEAD);
        let yaw = view.yaw + std::f32::consts::FRAC_PI_2;
        let Some(model) = content.model(CRATE_MODEL, &mut meshes, &mut materials, &mut images, &mut bindposes) else {
            warn!("headquarters: no {CRATE_MODEL}");
            return;
        };
        let entity = commands
            .spawn((
                SupplyCrate,
                Name::new("supply crate"),
                Transform::from_translation(at - Vec3::Y * u(CRATE_DEPTH)).with_rotation(Quat::from_rotation_y(yaw)),
                Visibility::default(),
            ))
            .id();
        let mut skeleton = Skeleton::default();
        spawn_model(&mut commands, &mut skeleton, SpawnModel { model: &model, owner: entity, attach_to: None, layers: None, shadows: true });
        commands.entity(entity).insert(skeleton);
        commands.insert_resource(Drop { entity, at, yaw, front: -ahead, since: now, items: Vec::new(), dealt: 0, burst: false, light: None });
        set_phase(Phase::Rising);
        dust(&mut effects, at);
        sfx.play("grenade_bounce_default", Some(at));
        return;
    };
    let t = now - d.since;
    let near = me_tf.translation.with_y(0.0).distance(d.at.with_y(0.0)) < u(REACH);
    NEAR_CRATE.store(near && phase() == Phase::Waiting, Ordering::Relaxed);
    let base = Quat::from_rotation_y(d.yaw);
    match phase() {
        Phase::Rising => {
            let k = (t / RISE).clamp(0.0, 1.0);
            let ease = 1.0 - (1.0 - k).powi(3);
            if let Ok(mut tf) = crates.get_mut(d.entity) {
                tf.translation = d.at - Vec3::Y * u(CRATE_DEPTH) * (1.0 - ease);
                // A wobble as it comes up.
                tf.rotation = base * Quat::from_rotation_z((t * 30.0).sin() * 0.04 * (1.0 - k));
            }
            if k >= 1.0 {
                set_phase(Phase::Waiting);
                sfx.play("claymore_plant", Some(d.at));
            }
        }
        Phase::Waiting => {
            if near && USE.swap(false, Ordering::Relaxed) {
                // Open one: the same draw as the Supply Drops menu's.
                let items: Vec<(Item, bool)> = {
                    let mut inv = crate::supply::inventory();
                    let items = inv.open().map(|x| x.to_vec()).unwrap_or_default();
                    inv.save_if_changed();
                    items
                };
                if items.is_empty() {
                    return;
                }
                d.items = items;
                d.since = now;
                d.front = (me_tf.translation - d.at).with_y(0.0).normalize_or(d.front);
                commands.entity(me).insert(crate::movement::Frozen);
                if let Some(b) = body {
                    commands.entity(b.0).insert(Visibility::Hidden);
                }
                // The SAS near the crate out of the way.
                for (place, mut v) in &mut figures {
                    if place.translation().distance(d.at) < u(CLEAR_AROUND) {
                        *v = Visibility::Hidden;
                    }
                }
                set_phase(Phase::Opening);
                // A warm light over it, as the camera comes round.
                let light = commands
                    .spawn((
                        PointLight { color: Color::srgb(1.0, 0.86, 0.66), intensity: CRATE_LIGHT, range: u(260.0), shadow_maps_enabled: false, ..default() },
                        Transform::from_translation(d.at + d.front * u(40.0) + Vec3::Y * u(70.0)),
                    ))
                    .id();
                d.light = Some(light);
                sfx.play("claymore_plant", Some(d.at));
            }
        }
        Phase::Opening | Phase::Done => {
            SINCE.store(t.to_bits(), Ordering::Relaxed);
            // A rattle, then the burst.
            if let Ok(mut tf) = crates.get_mut(d.entity) {
                tf.rotation = if t < RATTLE {
                    let shake = (t / RATTLE) * 0.06;
                    base * Quat::from_rotation_x((t * 47.0).sin() * shake) * Quat::from_rotation_z((t * 61.0).sin() * shake)
                } else {
                    base
                };
            }
            if t >= RATTLE && !d.burst {
                d.burst = true;
                dust(&mut effects, d.at + Vec3::Y * u(10.0));
                sfx.play("grenade_bounce_default", Some(d.at));
            }
            // Each card on its turn, with its rarity's sound as it lands.
            let mut list = Vec::new();
            for (k, &(item, new)) in d.items.clone().iter().enumerate() {
                let due = FIRST_CARD + CARD_GAP * k as f32;
                let age = (t >= due).then_some(t - due);
                if age.is_some_and(|a| a >= DEAL + FACE_DOWN) && d.dealt <= k {
                    d.dealt = k + 1;
                    let sound = match item.rarity() {
                        Rarity::Elite => "mp_level_up",
                        Rarity::Professional | Rarity::Veteran => "mp_challenge_complete",
                        _ => "mp_hit_alert",
                    };
                    sfx.play(sound, None);
                }
                list.push((item, new, age));
            }
            if let Ok(mut c) = CARDS.lock() {
                *c = list;
            }
            let last = FIRST_CARD + CARD_GAP * (d.items.len() as f32 - 1.0) + DEAL + FACE_DOWN + 0.5;
            if t > last {
                set_phase(Phase::Done);
            }
            if phase() == Phase::Done && USE.swap(false, Ordering::Relaxed) {
                commands.entity(me).remove::<crate::movement::Frozen>();
                if let Some(b) = body {
                    commands.entity(b.0).insert(Visibility::Inherited);
                }
                for (_, mut v) in &mut figures {
                    *v = Visibility::Inherited;
                }
                if let Ok(mut c) = CARDS.lock() {
                    c.clear();
                }
                d.since = now;
                if let Some(l) = d.light.take() {
                    commands.entity(l).despawn();
                }
                set_phase(Phase::Sinking);
            } else if phase() != Phase::Done {
                // Use waits until every card's out.
                USE.store(false, Ordering::Relaxed);
            }
        }
        Phase::Sinking => {
            let k = (t / SINK).clamp(0.0, 1.0);
            if let Ok(mut tf) = crates.get_mut(d.entity) {
                tf.translation = d.at - Vec3::Y * u(CRATE_DEPTH) * k * k;
            }
            if k >= 1.0 {
                commands.entity(d.entity).despawn();
                commands.remove_resource::<Drop>();
                set_phase(Phase::None);
            }
        }
        Phase::None => {}
    }
}

fn end_all(mut commands: Commands) {
    commands.remove_resource::<Drop>();
    set_phase(Phase::None);
    NEAR_CRATE.store(false, Ordering::Relaxed);
    HOLD.store(0f32.to_bits(), Ordering::Relaxed);
    if let Ok(mut c) = CARDS.lock() {
        c.clear();
    }
}

/// While it's opened: the camera moves to look at the crate from the
/// player's side, from above, leaving room over it for the cards.
fn frame_camera(
    time: Res<Time>,
    drop: Option<Res<Drop>>,
    spatial: avian3d::prelude::SpatialQuery,
    mut cameras: Query<(&crate::splitscreen::SlotCamera, &mut Transform)>,
) {
    let Some(d) = drop else { return };
    let Some((_, mut tf)) = cameras.iter_mut().find(|c| c.0.0 == 0) else { return };
    // Low and close, the crate in the lower part of the frame (the cards
    // go over it), pushing in slowly while it's open.
    let t = time.elapsed_secs() - d.since;
    let push = 1.0 - 0.12 * (t / 8.0).min(1.0);
    let side = d.front.cross(Vec3::Y).normalize_or(Vec3::X);
    let look = d.at + Vec3::Y * u(40.0);
    let mut eye = d.at + (d.front * u(95.0) + side * u(22.0)) * push + Vec3::Y * u(46.0);
    // Not through a wall or scaffold: in front of whatever's between.
    if let Ok(dir) = Dir3::new(eye - look) {
        let far = (eye - look).length();
        if let Some(hit) = spatial.cast_ray(look, dir, far, true, &crate::collision::sight_filter()) {
            eye = look + *dir * (hit.distance - u(6.0)).max(u(30.0));
        }
    }
    let target = Transform::from_translation(eye).looking_at(look, Vec3::Y);
    let k = (t / CAMERA_MOVE).clamp(0.0, 1.0);
    let k = k * k * (3.0 - 2.0 * k);
    tf.translation = tf.translation.lerp(target.translation, k);
    tf.rotation = tf.rotation.slerp(target.rotation, k);
}
