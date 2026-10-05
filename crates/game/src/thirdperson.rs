//! Third-person character models: the characters pawns wear
//! ([`crate::wardrobe`]; otherwise the map's team bodies and heads), the
//! world weapon in the right hand, and CoD4's player animations chosen from
//! movement state.

use crate::combat::{Dead, Pawn, Team};
use crate::content::{Content, PreparedModel};
use crate::models::{spawn_model, AnimPlayer, Skeleton, SpawnModel};
use crate::movement::{Mover, Stance};
use crate::player::LocalPlayer;
use crate::wardrobe::{LocalBody, Wardrobe};
use crate::weapons::WeaponInput;
use bevy::camera::visibility::RenderLayers;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use iw3::xanim::XAnim;
use rand::Rng;
use std::sync::Arc;

pub struct ThirdPersonPlugin;

impl Plugin for ThirdPersonPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (attach_bodies, hold_guns, drive_body_anims).chain().after(crate::movement::MovementSet).run_if(crate::state::in_game),
        )
            .add_systems(
                PostUpdate,
                turn_bodies
                    .after(crate::models::animate_skeletons)
                    .before(bevy::transform::TransformSystems::Propagate)
                    .run_if(crate::state::in_game),
            );
    }
}

/// Link from a pawn to its character model's skeleton owner.
#[derive(Component)]
pub struct Body(pub Entity);

#[derive(Component)]
struct BodyOwner;

/// On a body with a Black Ops rig, which takes Black Ops' animations.
#[derive(Component)]
struct BlackOpsRig;

/// A body animation: Black Ops' own for its rigs (CoD4's tip their heads
/// down and drop their jaws).
fn body_anim(content: &mut Content, black_ops: bool, name: &str) -> Option<Arc<XAnim>> {
    black_ops.then(|| crate::wardrobe::black_ops_anim(name)).flatten().or_else(|| content.anim(name))
}

/// Body and head model names for a team on the current map.
fn character_models(content: &Content, team: Team) -> (Option<String>, Option<String>) {
    let models: Vec<&str> = content
        .map()
        .assets
        .iter()
        .filter_map(|a| match a {
            iw3::zone::Asset::XModel(x) => Some(x.name.as_str()),
            _ => None,
        })
        .collect();
    let (body_prefixes, head_prefixes): (&[&str], &[&str]) = match team {
        Team::Allies => (&["body_mp_sas", "body_mp_usmc"], &["head_mp_sas", "head_mp_usmc"]),
        Team::Axis => (&["body_mp_opforce", "body_mp_arab"], &["head_mp_opforce", "head_mp_arab"]),
    };
    let pick = |prefixes: &[&str], prefer: &str| {
        let matching: Vec<&str> = models.iter().copied().filter(|m| prefixes.iter().any(|p| m.starts_with(p))).collect();
        matching.iter().find(|m| m.contains(prefer)).or(matching.first()).map(|s| s.to_string())
    };
    (pick(body_prefixes, "assault"), pick(head_prefixes, "headwrap"))
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn attach_bodies(
    mut commands: Commands,
    mut content: ResMut<Content>,
    wardrobe: Res<Wardrobe>,
    pawns: Query<(Entity, &Pawn, Has<LocalPlayer>, Option<&crate::splitscreen::LocalSlot>), Without<Body>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    for (pawn, p, local, slot) in &pawns {
        let outfit = wardrobe.outfit(&p.name, p.team, local);
        let models: Vec<Arc<PreparedModel>> = match outfit {
            Some(o) => o.models.clone(),
            None => {
                let (body, head) = character_models(&content, p.team);
                [body, head]
                    .into_iter()
                    .flatten()
                    .filter_map(|name| content.model(&name, &mut meshes, &mut materials, &mut images, &mut bindposes))
                    .collect()
            }
        };
        // The player's own body is seen in third person, its shadow always
        // ([`crate::wardrobe::ThirdPerson`]); in splitscreen each player's
        // is on its own layer, for the others to see.
        let layers = slot.map(|s| {
            RenderLayers::layer(if crate::splitscreen::active() { crate::splitscreen::body_layer(s.0) } else { crate::world::SHADOW_PROXY_LAYER })
        });
        // CoD models face +X; pawns face -Z.
        let owner = commands
            .spawn((
                BodyOwner,
                Name::new("body"),
                Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
                Visibility::default(),
                ChildOf(pawn),
            ))
            .id();
        let mut skeleton = Skeleton::default();
        let mut surfaces = Vec::new();
        for m in &models {
            surfaces.extend(spawn_model(&mut commands, &mut skeleton, SpawnModel { model: m, owner, attach_to: None, layers: layers.clone(), shadows: true }));
        }
        // The gun in hand comes next ([`hold_guns`]).
        let _ = layers;
        let black_ops = outfit.is_some_and(|o| o.character.game == crate::characters::Game::BlackOps);
        let mut player = AnimPlayer::default();
        if let Some(a) = body_anim(&mut content, black_ops, "pb_stand_alert") {
            player.play(a, 0.0);
        }
        commands.entity(owner).insert((skeleton, player));
        if black_ops {
            commands.entity(owner).insert(BlackOpsRig);
        }
        if let Some(s) = slot {
            commands.entity(owner).insert(LocalBody(surfaces, s.0));
        }
        commands.entity(pawn).insert(Body(owner));
        // Its own character is on the way: [`crate::wardrobe`] swaps it in.
        if outfit.is_none() && wardrobe.pending(&p.name, p.team, local) {
            commands.entity(pawn).insert(crate::wardrobe::Provisional);
        }
    }
}

/// The gun a body holds: which weapon it is and the joints and surfaces it
/// added to the body.
#[derive(Component)]
struct HeldGun {
    def: usize,
    first_joint: usize,
    joints: Vec<Entity>,
    surfaces: Vec<Entity>,
}

/// Put the weapon in each pawn's hands into its body's: its world model with
/// the class's attachments and camo, from Black Ops' or World at War's
/// content for their guns, swapped whenever the weapon changes (a class, a
/// weapon switch, a bot's own gun).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn hold_guns(
    mut commands: Commands,
    mut content: ResMut<Content>,
    mut bo1_content: ResMut<crate::bo1::MatchContent>,
    mut waw_content: ResMut<crate::waw::MatchContent>,
    pawns: Query<(&Body, &crate::weapons::WeaponState, Option<&crate::loadout::Loadout>)>,
    mut owners: Query<(&mut Skeleton, &mut AnimPlayer, Option<&HeldGun>, Option<&mut LocalBody>)>,
    layers: Query<&RenderLayers>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut camo_materials: ResMut<Assets<crate::gunmodel::CamoMaterial>>,
    mut camos: Local<crate::gunmodel::CamoCache>,
) {
    for (body, weapon, loadout) in &pawns {
        let def = weapon.def as *const _ as usize;
        let Ok((mut skeleton, mut anim, held, local_body)) = owners.get_mut(body.0) else { continue };
        if held.is_some_and(|h| h.def == def) {
            continue;
        }
        // The class's gun when it's the one in hand, else the weapon's own
        // name (`m4_reflex_mp` -> `m4:reflex`).
        let (spec, camo) = loadout
            .and_then(|l| l.gun_for(weapon.def).map(|g| (g.spec.clone(), g.camo)))
            .unwrap_or_else(|| {
                let mut parts = weapon.def.name.trim_end_matches("_mp").split('_');
                let gun = parts.next().unwrap_or_default().to_owned();
                (format!("{gun}:{}", parts.collect::<Vec<_>>().join("+")), 0)
            });
        let gun = crate::gunmodel::parse(&spec).0;
        let (bo1, waw) = (crate::bo1::is_bo1(gun), crate::waw::is_waw(gun));
        if (bo1 && bo1_content.get().is_none()) || (waw && waw_content.get().is_none()) {
            continue;
        }
        if let Some(h) = held {
            for &e in &h.joints {
                commands.entity(e).try_despawn();
            }
            for &e in &h.surfaces {
                commands.entity(e).try_despawn();
            }
            skeleton.truncate(h.first_joint);
        }
        let first_joint = skeleton.joints.len();
        let hand = skeleton.joint("tag_weapon_right");
        // Seen as the body is (the player's own: per the third person view).
        let shown = local_body.as_ref().and_then(|b| b.0.first()).and_then(|&e| layers.get(e).ok()).cloned();
        let content: &mut Content = match (bo1_content.get().filter(|_| bo1), waw_content.get().filter(|_| waw)) {
            (Some(c), _) | (_, Some(c)) => c,
            _ => &mut content,
        };
        let mut assets = crate::gunmodel::GunAssets {
            meshes: &mut meshes,
            materials: &mut materials,
            images: &mut images,
            bindposes: &mut bindposes,
            camo_materials: &mut camo_materials,
        };
        let target = crate::gunmodel::GunTarget { owner: body.0, attach_to: hand, layers: shown };
        let surfaces =
            crate::gunmodel::spawn_held_gun(&mut commands, content, &mut camos, &mut assets, &mut skeleton, &spec, camo, target)
                .unwrap_or_default();
        let joints = skeleton.joints[first_joint..].iter().map(|j| j.entity).collect();
        if let Some(mut local) = local_body {
            let old: Vec<Entity> = held.map(|h| h.surfaces.clone()).unwrap_or_default();
            local.0.retain(|e| !old.contains(e));
            local.0.extend(surfaces.iter().copied());
        }
        // New joints: the animation player maps them afresh.
        anim.remap();
        commands.entity(body.0).insert(HeldGun { def, first_joint, joints, surfaces });
    }
}

const DEATHS_STAND: &[&str] = &[
    "pb_stand_death_headchest_topple",
    "pb_stand_death_frontspin",
    "pb_stand_death_nervedeath",
    "pb_stand_death_neckdeath",
    "pb_stand_death_head_collapse",
    "pb_stand_death_lowerback",
    "pb_stand_death_legs",
];
const DEATHS_RUN: &[&str] = &["pb_death_run_forward_crumple", "pb_death_run_onfront", "pb_death_run_stumble", "pb_death_run_back"];
const DEATHS_CROUCH: &[&str] = &["pb_crouch_death_falltohands", "pb_crouch_death_fetal", "pb_crouch_death_flip", "pb_crouch_death_clutchchest"];

/// How far the view can turn from the legs before they swing round to it
/// (standing still), and how fast they swing; moving, they keep up.
const LEGS_TOLERANCE: f32 = 45.0 * std::f32::consts::PI / 180.0;
const LEGS_SWING: f32 = 6.0;
const LEGS_FOLLOW: f32 = 12.0;
/// The spine joints that turn the upper body to the aim, and their shares.
const SPINE: [(&str, f32); 3] = [("j_spinelower", 0.3), ("j_spineupper", 0.3), ("j_spine4", 0.4)];

/// Legs and torso, as CoD4's `BG_PlayerAngles` does it: standing, the legs
/// keep facing until the view has turned past [`LEGS_TOLERANCE`] from them,
/// then swing round; moving, they follow the view (the run animations
/// handle direction). The spine turns the rest of the way, and tilts with
/// the view's pitch, so the chest and gun point where the player aims
/// instead of the whole body spinning on every flick.
#[allow(clippy::type_complexity)]
fn turn_bodies(
    time: Res<Time>,
    pawns: Query<(&Body, &Mover, &crate::movement::ViewAngles, Has<Dead>)>,
    mut owners: Query<(&mut Transform, &Skeleton), With<BodyOwner>>,
    mut joints: Query<&mut Transform, Without<BodyOwner>>,
    parents: Query<(&ChildOf, &GlobalTransform)>,
    globals: Query<&GlobalTransform>,
    replay: Res<crate::killcam::ReplayPoses>,
    mut legs: Local<std::collections::HashMap<Entity, (f32, bool)>>,
) {
    let dt = time.delta_secs();
    let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    for (body, mover, view, dead) in &pawns {
        let Ok((mut owner_tf, skeleton)) = owners.get_mut(body.0) else { continue };
        // A killcam replaying this body: the view and twist it had then.
        let (view_yaw, view_pitch, twist, dead) = match replay.0.get(&body.0) {
            Some(&(yaw, pitch, twist, was_dead)) => (yaw, pitch, twist, was_dead),
            None => {
                let (yaw, swinging) = legs.entry(body.0).or_insert((view.yaw, false));
                let diff = wrap(view.yaw - *yaw);
                if dead {
                    *yaw = view.yaw;
                } else if mover.horizontal_speed() > crate::units::u(20.0) || mover.stance == Stance::Prone {
                    *yaw += diff * (1.0 - (-LEGS_FOLLOW * dt).exp());
                } else {
                    if diff.abs() > LEGS_TOLERANCE {
                        *swinging = true;
                    }
                    if *swinging {
                        *yaw += diff * (1.0 - (-LEGS_SWING * dt).exp());
                        *swinging = wrap(view.yaw - *yaw).abs() > 0.08;
                    }
                }
                *yaw = wrap(*yaw);
                (view.yaw, view.pitch, wrap(view.yaw - *yaw), dead)
            }
        };
        // The body hangs off the pawn, which faces the view (and CoD models
        // face +X: a quarter turn).
        owner_tf.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - twist);
        if dead {
            continue;
        }
        // The spine makes up the twist and the pitch, about the world's up and
        // the view's right, in each joint's parent's frame.
        let right = Quat::from_rotation_y(view_yaw) * Vec3::X;
        for (name, share) in SPINE {
            let Some(joint) = skeleton.joint(name) else { continue };
            let Ok((child_of, _)) = parents.get(joint) else { continue };
            let Ok(parent) = globals.get(child_of.parent()) else { continue };
            let Ok(mut tf) = joints.get_mut(joint) else { continue };
            let p = parent.compute_transform().rotation;
            let world = Quat::from_rotation_y(twist * share) * Quat::from_axis_angle(right, view_pitch * share);
            tf.rotation = p.inverse() * world * p * tf.rotation;
        }
    }
    legs.retain(|e, _| owners.contains(*e));
}

/// A body's jumping and falling.
#[derive(Default)]
struct Air {
    airborne_since: Option<f32>,
    /// Showing the take-off / in-air pose (so landing plays the landing).
    in_air_pose: bool,
    landing_until: f32,
}

/// Walking off a ledge looks like a jump once the fall lasts this long.
const FALL_POSE_AFTER: f32 = 0.25;
/// Most of a landing is the recovery; cut back to running after this.
const LAND_FOR: f32 = 0.35;

/// Pick the locomotion animation for a pawn's movement.
fn locomotion(mover: &Mover, yaw_forward: Vec3, ads: bool) -> (&'static str, f32) {
    let vel = Vec3::new(mover.velocity.x, 0.0, mover.velocity.z);
    let speed = vel.length();
    let right_dir = Vec3::new(-yaw_forward.z, 0.0, yaw_forward.x);
    let (fwd, side) = (vel.dot(yaw_forward), vel.dot(right_dir));
    let moving = speed > crate::units::u(20.0);
    let rate = |reference: f32| (speed / crate::units::u(reference)).clamp(0.5, 1.6);
    let dir = |f: &'static str, b: &'static str, l: &'static str, r: &'static str| {
        if fwd.abs() >= side.abs() {
            if fwd > 0.0 { f } else { b }
        } else if side > 0.0 {
            r
        } else {
            l
        }
    };
    match mover.stance {
        Stance::Stand if mover.sprinting => ("pb_sprint", rate(285.0)),
        Stance::Stand if moving => (
            dir("pb_combatrun_forward_loop", "pb_combatrun_back_loop", "pb_combatrun_left_loop", "pb_combatrun_right_loop"),
            rate(190.0),
        ),
        Stance::Stand => (if ads { "pb_stand_ads" } else { "pb_stand_alert" }, 1.0),
        Stance::Crouch if moving => (
            dir("pb_crouch_run_forward", "pb_crouch_run_back", "pb_crouch_run_left", "pb_crouch_run_right"),
            rate(123.0),
        ),
        Stance::Crouch => (if ads { "pb_crouch_ads" } else { "pb_crouch_alert" }, 1.0),
        Stance::Prone if moving => (dir("pb_prone_crawl", "pb_prone_crawl_back", "pb_prone_crawl_left", "pb_prone_crawl_right"), rate(28.0)),
        Stance::Prone => ("pb_prone_aim", 1.0),
    }
}

#[allow(clippy::type_complexity)]
fn drive_body_anims(
    mut content: ResMut<Content>,
    pawns: Query<(&Body, &Mover, &Transform, Option<&WeaponInput>, Has<Dead>, Option<&crate::melee::Melee>)>,
    mut players: Query<(&mut AnimPlayer, Has<BlackOpsRig>)>,
    mut dead_anim: Local<std::collections::HashMap<Entity, bool>>,
    mut air: Local<std::collections::HashMap<Entity, Air>>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs();
    let mut rng = rand::rng();
    for (body, mover, tf, input, dead, melee) in &pawns {
        let Ok((mut anim, black_ops)) = players.get_mut(body.0) else { continue };
        // A knife swing on the torso over whatever the legs are doing
        // (playeranim.script's `knife_melee`, `knife_melee_charge`).
        anim.overlay = melee.filter(|_| !dead).and_then(|m| {
            let name = match (mover.stance, m.charge) {
                (Stance::Prone, _) => "pt_melee_prone_pistol",
                (_, true) => "pt_melee_pistol_2",
                _ => "pt_melee_pistol_1",
            };
            let progress = ((now - m.started) / (m.until - m.started).max(0.05)).clamp(0.0, 1.0);
            body_anim(&mut content, black_ops, name).map(|a| (a, progress))
        });
        let was_dead = dead_anim.get(&body.0).copied().unwrap_or(false);
        if dead {
            if !was_dead {
                let list = match mover.stance {
                    Stance::Prone => &["pb_prone_death_quickdeath"][..],
                    Stance::Crouch => DEATHS_CROUCH,
                    Stance::Stand if mover.horizontal_speed() > crate::units::u(100.0) => DEATHS_RUN,
                    Stance::Stand => DEATHS_STAND,
                };
                if let Some(a) = body_anim(&mut content, black_ops, list[rng.random_range(0..list.len())]) {
                    anim.play(a, 0.1);
                    anim.speed = 1.0;
                }
                dead_anim.insert(body.0, true);
            }
            continue;
        }
        if was_dead {
            dead_anim.insert(body.0, false);
        }
        let forward = tf.rotation * Vec3::NEG_Z;
        let ads = input.is_some_and(|i| i.ads);
        // Jumps and falls: the take-off animation (it ends in the in-air
        // pose and holds there), then a landing.
        let state = air.entry(body.0).or_default();
        let running = mover.horizontal_speed() > crate::units::u(100.0);
        if !mover.on_ground {
            let since = *state.airborne_since.get_or_insert(now);
            let leaping = mover.velocity.y > crate::units::u(60.0);
            if (leaping || now - since > FALL_POSE_AFTER) && mover.stance == Stance::Stand {
                let name = if running { "pb_runjump_takeoff" } else { "pb_standjump_takeoff" };
                if let Some(a) = body_anim(&mut content, black_ops, name) {
                    anim.ensure(a, 0.1);
                    anim.speed = 1.0;
                }
                state.in_air_pose = true;
            }
            continue;
        }
        state.airborne_since = None;
        if std::mem::take(&mut state.in_air_pose) {
            let name = if running { "pb_runjump_land" } else { "pb_standjump_land" };
            if let Some(a) = body_anim(&mut content, black_ops, name) {
                state.landing_until = now + a.duration().min(LAND_FOR);
                anim.play(a, 0.08);
                anim.speed = 1.0;
            }
        }
        if now < state.landing_until && mover.stance == Stance::Stand && !mover.sprinting {
            continue;
        }
        let (name, speed) = locomotion(mover, forward, ads);
        if let Some(a) = body_anim(&mut content, black_ops, name) {
            anim.ensure(a, 0.2);
            anim.speed = speed;
        }
    }
}
