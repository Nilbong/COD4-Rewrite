//! Full-body awareness: looking down, a local player sees their own body,
//! chest to feet, walking, running, crouching and lying with them.
//!
//! The real body ([`crate::thirdperson`]) stays as it is: seen in third
//! person and by others, casting the player's shadow. In first person a
//! second set of its surfaces is drawn, skinned to joints of its own posed
//! from the real body's each frame: the legs as the real body's (their
//! swing behind the view, the gait), the hips held still just behind the
//! eye (the walk and run animations slide them forward, which carried the
//! chest into the view), and the torso upright in the model's rest pose,
//! facing the view: the real body bends and leans its torso to the aim,
//! which looking down put the chest, collar and scarf into the eye. The
//! neck, head and arms are collapsed to nothing (the viewmodel's arms are
//! the ones seen), so looking down shows the chest and belly under the
//! viewmodel's arms and the legs stepping out beneath. The body moves with
//! the camera's bob and dips, as the gun does (otherwise, running, the gun
//! bobbed over a body that didn't), and its joints ease into new poses (the
//! crouch and stance animations switch at once on the real body). It casts no shadow
//! (the real body does), and isn't drawn in third person, the killcam,
//! Bodycam gunplay, Headquarters, or while dead or picking a class.

use crate::combat::Dead;
use crate::loadout::AwaitingClass;
use crate::models::Skeleton;
use crate::movement::ViewAngles;
use crate::splitscreen::LocalSlot;
use crate::thirdperson::Body;
use crate::units::u;
use crate::wardrobe::LocalBody;
use bevy::camera::visibility::RenderLayers;
use bevy::light::NotShadowCaster;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use std::collections::HashMap;

pub(super) fn build(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (
            follow.after(crate::thirdperson::BodyPoseSet).after(bevy::transform::TransformSystems::Propagate),
        )
            .run_if(crate::state::in_game),
    );
}

/// How far the body is moved back along the view's heading, in CoD units,
/// past where the real body lies (prone).
const BACK: f32 = 5.0;
/// How far behind the eye the hips are held, standing or crouched, in CoD
/// units (where the real body's stand).
const HIPS_BACK: f32 = 11.0;
/// ...and crouched: further back, so the raised thigh and knee sit behind
/// the view rather than right under it (in front of the gun).
const HIPS_BACK_CROUCHED: f32 = 20.0;
/// Eye heights standing and crouched (CoD units): how far into a crouch
/// the eye is, for easing between the two.
const EYE_STAND: f32 = 60.0;
const EYE_CROUCH: f32 = 40.0;
const HIPS: &str = "j_mainroot";
/// How long the copy's joints take to ease into a new pose (seconds).
const EASE: f32 = 0.1;
/// The most the camera's bob moves the body (CoD units).
const MAX_BOB: f32 = 8.0;
/// How far below the eye the top of the chest stays at least (CoD units).
const CHEST_BELOW_EYE: f32 = 12.0;
const CHEST: &str = "j_spine4";
/// The joint the torso starts at: from it up, the model's rest pose.
const TORSO: &str = "j_spinelower";
/// Joints whose geometry (with all below them) isn't drawn: the neck and
/// head, and the arms.
const HIDDEN_ROOTS: [&str; 5] = ["j_neck", "j_clavicle_le", "j_clavicle_ri", "j_shoulder_le", "j_shoulder_ri"];

/// The render layer of a player's own first-person body in splitscreen
/// (only their camera draws it). Alone, it's the world's.
pub fn layer(slot: usize) -> usize {
    35 + slot
}

/// One of its joints.
#[derive(Component)]
struct OwnBodyJoint;

/// A joint of the copy.
struct CopyJoint {
    mine: Entity,
    real: Entity,
    /// Its parent in the copy, if the parent is a joint.
    parent: Option<usize>,
    /// The torso and up: held in the model's rest pose, upright.
    torso: bool,
    rest: Transform,
    /// The joint its geometry collapses onto, if hidden.
    hidden_onto: Option<usize>,
}

/// A local player's first-person body, on the pawn.
#[derive(Component)]
pub struct OwnBody {
    /// Its root, holding the surfaces (hidden when it isn't drawn).
    root: Entity,
    /// What it copies: the real body's owner and surfaces.
    source: (Entity, Vec<Entity>),
    joints: Vec<CopyJoint>,
    /// Joint indices, parents first.
    order: Vec<usize>,
    /// The hips (`j_mainroot`) and the top of the chest (`j_spine4`).
    hips: Option<usize>,
    chest: Option<usize>,
    /// Which is the torso's first joint, and its rest turn from the body's
    /// root.
    torso: Option<(usize, Quat)>,
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn follow(
    mut commands: Commands,
    pawns: Query<(Entity, &LocalSlot, &Body, &ViewAngles, &crate::movement::Mover, Option<&OwnBody>, Has<Dead>, Has<AwaitingClass>)>,
    bodies: Query<(&LocalBody, &Skeleton, Option<&crate::thirdperson::HeldGun>)>,
    surfaces: Query<(&Mesh3d, Option<&MeshMaterial3d<StandardMaterial>>, &SkinnedMesh)>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    mut globals: Query<&mut GlobalTransform>,
    view: Res<crate::wardrobe::ThirdPerson>,
    killcam: Res<crate::killcam::Killcam>,
    gunplay: Res<crate::bodycam::Gunplay>,
    spectate: Option<Res<crate::bots::spectate::Spectate>>,
    mut visibility: Query<&mut Visibility>,
    cameras: Query<(&crate::splitscreen::SlotCamera, &Transform), Without<OwnBodyJoint>>,
    time: Res<Time>,
    mut poses: Local<Vec<Option<GlobalTransform>>>,
    mut eased: Local<HashMap<Entity, Vec<Option<Transform>>>>,
) {
    let ease = 1.0 - (-time.delta_secs() / EASE).exp();
    let on = super::full_body() && !killcam.showing() && !gunplay.is_bodycam() && spectate.is_none() && !crate::hq::active();
    let split = crate::splitscreen::active();
    for (pawn, slot, body, angles, mover, own, dead, waiting) in &pawns {
        let Ok((local, skeleton, held)) = bodies.get(body.0) else { continue };
        let gun: &[Entity] = held.map_or(&[], |h| &h.surfaces);
        let wanted: Vec<Entity> = local.0.iter().copied().filter(|e| !gun.contains(e)).collect();
        // (Re)built for the body it copies: a new outfit is a new body.
        let own = match own {
            Some(o) if o.source.0 == body.0 && o.source.1 == wanted => o,
            stale => {
                if let Some(o) = stale {
                    commands.entity(o.root).despawn();
                    for j in &o.joints {
                        commands.entity(j.mine).despawn();
                    }
                }
                let layers = RenderLayers::layer(if split { layer(slot.0) } else { 0 });
                if let Some(o) = spawn(&mut commands, body.0, skeleton, &wanted, &surfaces, &parents, layers) {
                    commands.entity(pawn).insert(o);
                }
                continue;
            }
        };
        let shown = on && !dead && !waiting && !view.on(slot.0);
        if let Ok(mut v) = visibility.get_mut(own.root) {
            let want = if shown { Visibility::Inherited } else { Visibility::Hidden };
            if *v != want {
                *v = want;
            }
        }
        if !shown {
            eased.remove(&pawn);
            continue;
        }
        let (Ok(owner), Ok(owner_local)) = (globals.get(body.0).copied(), locals.get(body.0)) else { continue };
        // The hips' turn from the view (the legs swing behind it): the
        // owner faces the legs' way (CoD models face +X, a quarter turn).
        let facing = owner_local.rotation * Vec3::X;
        let twist = std::f32::consts::FRAC_PI_2 - (-facing.z).atan2(facing.x);
        let back = Quat::from_rotation_y(angles.yaw) * Vec3::Z * u(BACK);
        let root = GlobalTransform::from(Transform { translation: owner.translation() + back, ..owner.compute_transform() });
        poses.clear();
        poses.resize(own.joints.len(), None);
        let eased = eased.entry(pawn).or_default();
        eased.resize(own.joints.len(), None);
        for &i in &own.order {
            let j = &own.joints[i];
            let parent = j.parent.and_then(|p| poses[p]).unwrap_or(root);
            let local_pose = if j.torso { j.rest } else { locals.get(j.real).copied().unwrap_or_default() };
            // Eased towards the real body's pose.
            let local_pose = match eased[i] {
                Some(was) => Transform {
                    translation: was.translation.lerp(local_pose.translation, ease),
                    rotation: was.rotation.slerp(local_pose.rotation, ease),
                    scale: was.scale.lerp(local_pose.scale, ease),
                },
                None => local_pose,
            };
            eased[i] = Some(local_pose);
            let mut pose = parent * GlobalTransform::from(local_pose);
            if let Some((_, rest)) = own.torso.filter(|t| t.0 == i) {
                // The torso: where the hips put it, upright on the body's
                // heading (the run and crouch animations lean the hips),
                // facing the view.
                let (s, _, t) = pose.to_scale_rotation_translation();
                pose = GlobalTransform::from(Transform { translation: t, rotation: Quat::from_rotation_y(twist) * root.rotation() * rest, scale: s });
            }
            poses[i] = Some(pose);
        }
        // Standing and crouched, the hips stay put under the view (the
        // walk and run animations slide them forward, carrying the chest
        // into the eye); the legs swing beneath them. Lying down, the body
        // stretches out behind as it lies.
        if mover.stance != crate::movement::Stance::Prone
            && let Some(hips) = own.hips.and_then(|h| poses[h])
        {
            let crouched = ((u(EYE_STAND) - mover.eye_height) / u(EYE_STAND - EYE_CROUCH)).clamp(0.0, 1.0);
            let back = HIPS_BACK + (HIPS_BACK_CROUCHED - HIPS_BACK) * crouched;
            let target = owner.translation() + Quat::from_rotation_y(angles.yaw) * Vec3::Z * u(back);
            let shift = (target - hips.translation()) * Vec3::new(1.0, 0.0, 1.0);
            for p in poses.iter_mut().flatten() {
                *p = GlobalTransform::from(Transform { translation: p.translation() + shift, ..p.compute_transform() });
            }
        }
        // The camera's bob, dips and lean: the body goes with them, as the
        // gun does.
        let plain_eye = locals.get(pawn).map_or(owner.translation(), |t| t.translation) + Vec3::Y * mover.eye_height;
        let eye = cameras.iter().find(|(c, _)| c.0 == slot.0).map_or(plain_eye, |(_, t)| t.translation);
        let bob = (eye - plain_eye).clamp_length_max(u(MAX_BOB));
        for p in poses.iter_mut().flatten() {
            *p = GlobalTransform::from(Transform { translation: p.translation() + bob, ..p.compute_transform() });
        }
        // Crouched, an upright torso is taller than the eye is high (a real
        // crouch bends the back): it's lowered to keep the chest below.
        if let Some(chest) = own.chest.and_then(|c| poses[c]) {
            let limit = plain_eye.y + bob.y - u(CHEST_BELOW_EYE);
            let over = chest.translation().y - limit;
            if over > 0.0 {
                for (j, p) in own.joints.iter().zip(poses.iter_mut()) {
                    if let (true, Some(p)) = (j.torso, p.as_mut()) {
                        *p = GlobalTransform::from(Transform { translation: p.translation() - Vec3::Y * over, ..p.compute_transform() });
                    }
                }
            }
        }
        for (i, j) in own.joints.iter().enumerate() {
            let pose = match j.hidden_onto {
                Some(onto) => poses[onto].map(|g| {
                    let (_, r, t) = g.to_scale_rotation_translation();
                    GlobalTransform::from(Transform { translation: t, rotation: r, scale: Vec3::splat(0.001) })
                }),
                None => poses[i],
            };
            if let (Some(pose), Ok(mut out)) = (pose, globals.get_mut(j.mine)) {
                *out = pose;
            }
        }
    }
}

/// The copy: joints of its own and the body's surfaces skinned to them.
fn spawn(
    commands: &mut Commands,
    owner: Entity,
    skeleton: &Skeleton,
    wanted: &[Entity],
    surfaces: &Query<(&Mesh3d, Option<&MeshMaterial3d<StandardMaterial>>, &SkinnedMesh)>,
    parents: &Query<&ChildOf>,
    layers: RenderLayers,
) -> Option<OwnBody> {
    if wanted.is_empty() {
        return None;
    }
    let index: HashMap<Entity, usize> = skeleton.joints.iter().enumerate().map(|(i, j)| (j.entity, i)).collect();
    let parent_of = |i: usize| parents.get(skeleton.joints[i].entity).ok().and_then(|p| index.get(&p.parent()).copied());
    let named = |i: usize, names: &[&str]| names.iter().any(|n| skeleton.joints[i].name.eq_ignore_ascii_case(n));
    // A joint and those above it, nearest first.
    let chain = |i: usize| {
        let mut c = vec![i];
        while let Some(p) = c.last().and_then(|&k| parent_of(k)) {
            if c.contains(&p) {
                break;
            }
            c.push(p);
        }
        c
    };
    let mut joints = Vec::new();
    for (i, j) in skeleton.joints.iter().enumerate() {
        let up = chain(i);
        // Hidden: collapsed onto the shown joint just above its hidden root.
        let hidden_onto = up.iter().rposition(|&k| named(k, &HIDDEN_ROOTS)).map(|top| up.get(top + 1).copied().unwrap_or(up[top]));
        joints.push(CopyJoint {
            mine: commands.spawn((Name::new("own body joint"), OwnBodyJoint, Transform::default(), GlobalTransform::default())).id(),
            real: j.entity,
            parent: parent_of(i),
            torso: up.iter().any(|&k| named(k, &[TORSO])),
            rest: j.bind,
            hidden_onto,
        });
    }
    // Parents first (the torso's joints come after their children).
    let mut order = Vec::new();
    let mut placed = vec![false; joints.len()];
    while order.len() < joints.len() {
        let before = order.len();
        for i in 0..joints.len() {
            if !placed[i] && joints[i].parent.is_none_or(|p| placed[p]) {
                placed[i] = true;
                order.push(i);
            }
        }
        if order.len() == before {
            break;
        }
    }
    // The torso's rest turn from the root: its and its parents' rest poses.
    let torso = (0..joints.len()).find(|&i| named(i, &[TORSO])).map(|i| {
        let rest = chain(i).iter().rev().fold(Quat::IDENTITY, |q, &k| q * skeleton.joints[k].bind.rotation);
        (i, rest)
    });
    let root = commands.spawn((Name::new("own body"), Transform::default(), Visibility::Hidden)).id();
    let map: HashMap<Entity, Entity> = joints.iter().map(|j| (j.real, j.mine)).collect();
    for &s in wanted {
        let Ok((mesh, material, skin)) = surfaces.get(s) else { continue };
        let Some(material) = material else { continue };
        let joints: Vec<Entity> = skin.joints.iter().map(|j| map.get(j).copied().unwrap_or(root)).collect();
        commands.spawn((
            Name::new("own body surface"),
            mesh.clone(),
            material.clone(),
            SkinnedMesh { inverse_bindposes: skin.inverse_bindposes.clone(), joints },
            bevy::camera::visibility::NoFrustumCulling,
            NotShadowCaster,
            layers.clone(),
            Transform::IDENTITY,
            ChildOf(root),
        ));
    }
    let hips = (0..joints.len()).find(|&i| named(i, &[HIPS]));
    let chest = (0..joints.len()).find(|&i| named(i, &[CHEST]));
    Some(OwnBody { root, source: (owner, wanted.to_vec()), joints, order, hips, chest, torso })
}
