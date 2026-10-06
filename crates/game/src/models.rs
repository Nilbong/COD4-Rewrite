//! Spawning skinned CoD models and playing XAnims on them.
//!
//! A model instance is a hierarchy of joint entities (one per bone) plus one
//! skinned mesh entity per surface. Several models can share one skeleton
//! the way CoD's DObjs do: bones with the same name are shared (a head on a
//! body), and a model's root can be attached to another model's tag (a gun
//! on the hands' `tag_weapon`).

use crate::content::{quat, PreparedModel};
use crate::units;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::light::NotShadowCaster;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use iw3::xanim::XAnim;
use std::collections::HashMap;
use std::sync::Arc;

pub struct ModelsPlugin;

impl Plugin for ModelsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, animate_skeletons.before(TransformSystems::Propagate));
    }
}

/// All joints of one animated object (possibly spanning several models).
#[derive(Component, Default)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
    by_name: HashMap<String, usize>,
    /// Joints scaled to zero, hiding the geometry skinned to them
    /// (`WeaponDef::hideTags`).
    hidden: Vec<bool>,
}

pub struct Joint {
    pub name: String,
    pub entity: Entity,
    pub bind: Transform,
    /// Translation XAnim offsets are relative to (see `PreparedBone::anim_base`).
    pub anim_base: Vec3,
}

impl Skeleton {
    pub fn joint(&self, name: &str) -> Option<Entity> {
        self.by_name.get(name).map(|&i| self.joints[i].entity)
    }

    /// Drop the joints from `n` on (a held gun being swapped for another).
    pub fn truncate(&mut self, n: usize) {
        let n = n.min(self.joints.len());
        for j in self.joints.drain(n..) {
            self.by_name.remove(&j.name);
        }
        self.hidden.truncate(n);
    }

    pub fn hide(&mut self, name: &str) {
        if let Some(&i) = self.by_name.get(name) {
            self.hidden.resize(self.joints.len(), false);
            self.hidden[i] = true;
        }
    }
}

/// Options for spawning a model into a skeleton.
pub struct SpawnModel<'a> {
    pub model: &'a PreparedModel,
    /// Entity holding the [`Skeleton`]; joints are parented under it.
    pub owner: Entity,
    /// Parent the model's root bones under this joint instead of the owner.
    pub attach_to: Option<Entity>,
    pub layers: Option<RenderLayers>,
    pub shadows: bool,
}

/// One bone to spawn: the model's own, or one [`mp_torso`] adds.
struct Bone {
    name: String,
    parent: Option<usize>,
    bind_local: Transform,
    anim_base: Vec3,
}

/// Campaign characters lack the multiplayer rig's `pelvis` (between
/// `j_mainroot` and the hips) and `torso_stabilizer` (the spine's parent,
/// facing model space). The multiplayer `pb_*` anims rotate the spine
/// relative to those, so without them campaign bodies bend over backwards.
/// Add both, with the multiplayer bodies' bind pose, keeping every bone's
/// bind pose in model space (so skinning is unchanged). They go after the
/// model's own bones, which skinning indexes.
fn mp_torso(m: &PreparedModel) -> Option<Vec<Bone>> {
    let find = |name: &str| m.bones.iter().position(|b| b.name == name);
    if find("torso_stabilizer").is_some() || find("pelvis").is_some() {
        return None;
    }
    let (root, spine) = (find("j_mainroot")?, find("j_spinelower")?);
    if m.bones[spine].parent != Some(root) {
        return None;
    }
    // j_mainroot's rotation in model space.
    let mut root_rot = Quat::IDENTITY;
    let mut at = Some(root);
    while let Some(i) = at {
        root_rot = m.bones[i].bind_local.rotation * root_rot;
        at = m.bones[i].parent;
    }
    let (pelvis, torso) = (m.bones.len(), m.bones.len() + 1);
    let mut bones: Vec<Bone> = m
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let mut bone = Bone { name: b.name.clone(), parent: b.parent, bind_local: b.bind_local, anim_base: b.anim_base };
            if i == spine {
                bone.parent = Some(torso);
                bone.bind_local = Transform {
                    translation: root_rot * b.bind_local.translation,
                    rotation: root_rot * b.bind_local.rotation,
                    scale: b.bind_local.scale,
                };
                bone.anim_base = root_rot * b.anim_base;
            } else if b.parent == Some(root) {
                bone.parent = Some(pelvis);
            }
            bone
        })
        .collect();
    let new = |name: &str, parent: usize, rotation: Quat| Bone {
        name: name.into(),
        parent: Some(parent),
        bind_local: Transform::from_rotation(rotation),
        anim_base: Vec3::ZERO,
    };
    bones.push(new("pelvis", root, Quat::IDENTITY));
    bones.push(new("torso_stabilizer", pelvis, root_rot.inverse()));
    Some(bones)
}

/// Spawn a model's joints (reusing same-named joints already in the
/// skeleton) and its skinned surfaces.
pub fn spawn_model(commands: &mut Commands, skeleton: &mut Skeleton, spec: SpawnModel) -> Vec<Entity> {
    let m = spec.model;
    let bones = mp_torso(m).unwrap_or_else(|| {
        m.bones.iter().map(|b| Bone { name: b.name.clone(), parent: b.parent, bind_local: b.bind_local, anim_base: b.anim_base }).collect()
    });
    // Parents first: bones added by `mp_torso` come after their children.
    let mut slots: Vec<Option<Entity>> = vec![None; bones.len()];
    let mut pending: Vec<usize> = (0..bones.len()).collect();
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|&i| {
            let bone = &bones[i];
            if let Some(&existing) = skeleton.by_name.get(&bone.name) {
                slots[i] = Some(skeleton.joints[existing].entity);
                return false;
            }
            let parent = match bone.parent {
                Some(p) => match slots[p] {
                    Some(e) => e,
                    None => return true,
                },
                None => spec.attach_to.unwrap_or(spec.owner),
            };
            let attached_root = bone.parent.is_none() && spec.attach_to.is_some();
            let local = if attached_root { Transform::IDENTITY } else { bone.bind_local };
            let anim_base = if attached_root { Vec3::ZERO } else { bone.anim_base };
            let e = commands.spawn((Name::new(bone.name.clone()), local, Visibility::default(), ChildOf(parent))).id();
            skeleton.by_name.insert(bone.name.clone(), skeleton.joints.len());
            skeleton.joints.push(Joint { name: bone.name.clone(), entity: e, bind: local, anim_base });
            slots[i] = Some(e);
            false
        });
        if pending.len() == before {
            warn!("{}: bones with missing parents", m.name);
            break;
        }
    }
    let entities: Vec<Entity> = slots.into_iter().map(|e| e.unwrap_or(spec.owner)).collect();
    let mut surfaces = Vec::new();
    for (mesh, material) in &m.surfaces {
        let mut e = commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            SkinnedMesh { inverse_bindposes: m.inverse_bindposes.clone(), joints: entities.clone() },
            NoFrustumCulling,
            Transform::IDENTITY,
            ChildOf(spec.owner),
        ));
        if let Some(l) = &spec.layers {
            e.insert(l.clone());
        }
        if !spec.shadows {
            e.insert(NotShadowCaster);
        }
        surfaces.push(e.id());
    }
    surfaces
}

/// Plays one animation at a time on a [`Skeleton`], crossfading on change.
#[derive(Component)]
pub struct AnimPlayer {
    pub anim: Option<Arc<XAnim>>,
    pub time: f32,
    pub speed: f32,
    prev: Option<(Arc<XAnim>, f32)>,
    fade: f32,
    fade_len: f32,
    /// Track index per skeleton joint for `anim` and `prev`.
    map: Vec<Option<usize>>,
    prev_map: Vec<Option<usize>>,
    /// An animation posed at a fixed progress (0..1) and applied over the
    /// base animation for the bones it animates. CoD4's viewmodel ADS anims
    /// work this way: they only move `tag_ads`, driven by the ADS fraction.
    pub overlay: Option<(Arc<XAnim>, f32)>,
    /// How much of the overlay shows (0..1): a third-person torso
    /// animation blending in and out over the legs'.
    pub overlay_weight: f32,
}

impl Default for AnimPlayer {
    fn default() -> Self {
        AnimPlayer { anim: None, time: 0.0, speed: 1.0, prev: None, fade: 1.0, fade_len: 0.0, map: Vec::new(), prev_map: Vec::new(), overlay: None, overlay_weight: 1.0 }
    }
}

impl AnimPlayer {
    /// Switch animation (restarting it) with a crossfade of `fade` seconds.
    pub fn play(&mut self, anim: Arc<XAnim>, fade: f32) {
        if let Some(cur) = self.anim.take() {
            self.prev = Some((cur, self.time));
            self.prev_map = std::mem::take(&mut self.map);
        }
        self.anim = Some(anim);
        self.time = 0.0;
        self.fade = if fade > 0.0 { 0.0 } else { 1.0 };
        self.fade_len = fade;
        self.map.clear();
    }

    /// Hold `anim` at `time` with no crossfade (a replay posing a body as it
    /// was): a different animation is mapped onto the skeleton afresh.
    pub fn pose(&mut self, anim: Arc<XAnim>, time: f32) {
        if !self.anim.as_ref().is_some_and(|a| Arc::ptr_eq(a, &anim)) {
            self.anim = Some(anim);
            self.map.clear();
        }
        self.time = time;
        self.prev = None;
        self.fade = 1.0;
    }

    /// Play `anim` unless it is already playing.
    pub fn ensure(&mut self, anim: Arc<XAnim>, fade: f32) {
        if self.anim.as_ref().is_some_and(|a| Arc::ptr_eq(a, &anim)) {
            return;
        }
        self.play(anim, fade);
    }

    /// Re-match the skeleton's joints to the animation's tracks (after
    /// joints were added or removed).
    pub fn remap(&mut self) {
        self.map.clear();
        self.prev_map.clear();
    }

    pub fn finished(&self) -> bool {
        self.anim.as_ref().is_none_or(|a| !a.looping && self.time >= a.duration())
    }
}

fn track_map(skel: &Skeleton, anim: &XAnim) -> Vec<Option<usize>> {
    let by_name: HashMap<&str, usize> = anim.bones.iter().enumerate().map(|(i, b)| (b.name.as_str(), i)).collect();
    skel.joints.iter().map(|j| by_name.get(j.name.as_str()).copied()).collect()
}

/// Local pose of one joint from an animation (or the bind pose).
fn sample(anim: &XAnim, track: Option<usize>, frame: f32, joint: &Joint) -> Transform {
    let Some(b) = track.and_then(|i| anim.bones.get(i)) else { return joint.bind };
    let mut t = joint.bind;
    if let Some(q) = b.rot.sample(frame) {
        t.rotation = quat(q);
    }
    // Like CoD's DObj skeleton: an animated bone's local translation is the
    // model's own `trans` plus the anim's; rotations replace the bind
    // rotation.
    t.translation = joint.anim_base + b.trans.sample(frame).map_or(Vec3::ZERO, units::pos);
    t
}

/// Campaign (single-player) anims turn the spine relative to `j_mainroot`;
/// the multiplayer rig hangs it from `torso_stabilizer` (turned against
/// the root, [`mp_torso`]), which they leave alone. For such an anim (it
/// moves the root, not the stabilizer): the spine's joint, and the turn
/// that takes its pose into the stabilizer's frame.
fn spine_fix(skel: &Skeleton, anim: &XAnim) -> Option<(usize, Quat)> {
    let has = |name: &str| anim.bones.iter().any(|b| b.name == name);
    // Player anims (CoD4's or Black Ops') are made for the rig as it is.
    let player = anim.name.starts_with("pb_") || anim.name.starts_with("pt_");
    if player || !has("j_mainroot") || has("torso_stabilizer") {
        return None;
    }
    let joint = |name: &str| skel.by_name.get(name).copied();
    let (pelvis, torso, spine) = (joint("pelvis")?, joint("torso_stabilizer")?, joint("j_spinelower")?);
    let r = skel.joints[pelvis].bind.rotation * skel.joints[torso].bind.rotation;
    Some((spine, r.inverse()))
}

/// [`spine_fix`] on a sampled pose.
fn fix_spine(pose: &mut Transform, joint: &Joint, i: usize, fix: Option<(usize, Quat)>) {
    if let Some((_, inv)) = fix.filter(|f| f.0 == i) {
        pose.rotation = inv * pose.rotation;
        pose.translation = joint.anim_base + inv * (pose.translation - joint.anim_base);
    }
}

pub(crate) fn animate_skeletons(time: Res<Time>, mut q: Query<(&Skeleton, &mut AnimPlayer)>, mut joints: Query<&mut Transform>) {
    let dt = time.delta_secs();
    for (skel, mut player) in &mut q {
        let Some(anim) = player.anim.clone() else { continue };
        if player.map.len() != skel.joints.len() {
            player.map = track_map(skel, &anim);
        }
        player.time += dt * player.speed;
        if player.fade < 1.0 {
            player.fade = (player.fade + dt / player.fade_len.max(1e-3)).min(1.0);
        }
        let frame = anim.frame_at(player.time);
        let prev = if player.fade < 1.0 { player.prev.clone() } else { None };
        if let Some((p, _)) = &prev {
            if player.prev_map.len() != skel.joints.len() {
                player.prev_map = track_map(skel, p);
            }
        }
        let w = player.fade;
        let fix = spine_fix(skel, &anim);
        let prev_fix = prev.as_ref().and_then(|(p, _)| spine_fix(skel, p));
        let overlay = player.overlay.as_ref().map(|(a, progress)| (a, track_map(skel, a), progress * a.num_frames as f32));
        let ow = player.overlay_weight.clamp(0.0, 1.0);
        for (i, j) in skel.joints.iter().enumerate() {
            let Ok(mut tf) = joints.get_mut(j.entity) else { continue };
            let mut pose = sample(&anim, player.map[i], frame, j);
            fix_spine(&mut pose, j, i, fix);
            if let Some((p, pt)) = &prev {
                let mut from = sample(p, player.prev_map[i], p.frame_at(*pt), j);
                fix_spine(&mut from, j, i, prev_fix);
                pose.translation = from.translation.lerp(pose.translation, w);
                pose.rotation = from.rotation.slerp(pose.rotation, w);
            }
            if let Some((a, map, frame)) = &overlay {
                if map[i].is_some() {
                    let over = sample(a, map[i], *frame, j);
                    if ow >= 1.0 {
                        pose = over;
                    } else {
                        pose.translation = pose.translation.lerp(over.translation, ow);
                        pose.rotation = pose.rotation.slerp(over.rotation, ow);
                    }
                }
            }
            if skel.hidden.get(i).copied().unwrap_or(false) {
                pose.scale = Vec3::ZERO;
            }
            *tf = pose;
        }
    }
}
