//! Playing a kill back: the camera at the killer's eyes (riding the grenade
//! or projectile for explosive kills), the killer's gun in hand, and every
//! body where and as it was, posed from the history while the live game
//! carries on underneath.

use super::record::{Frame, History, Kill, PawnSample, ProjectileSample};
use crate::combat::Team;
use crate::content::Content;
use crate::fx::{Anchor, Effects, Frame as FxFrame, FxLayer};
use crate::gunmodel::{CamoCache, CamoMaterial, GunAssets, GunTarget};
use crate::models::{AnimPlayer, Skeleton, SpawnModel, spawn_model};
use crate::player::{HIP_FOV, MainCamera, VIEWMODEL_LAYER, ViewModelCamera};
use crate::thirdperson::Body;
use crate::units::u;
use crate::viewmodel::{WeaponAnims, anim_slot};
use crate::weapons::WeaponDef;
use bevy::camera::visibility::RenderLayers;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use std::collections::HashMap;

/// After a death, how long until the killcam, and how much of the past it
/// shows: before the kill and after (`scr_killcam_time`'s 5 s and a little).
pub const DELAY: f32 = 1.5;
pub const BEFORE: f32 = 5.0;
pub const AFTER: f32 = 1.5;
/// The final killcam's share of the match's 10 s end: less before.
pub const FINAL_BEFORE: f32 = 4.5;

/// A kill being played back.
pub struct Replay {
    pub title: &'static str,
    pub kill: Kill,
    pub killer_name: String,
    pub victim_name: String,
    pub killer_team: Team,
    /// History times shown, and when the playback began (game time).
    pub from: f32,
    pub to: f32,
    pub started: f32,
    pub skippable: bool,
    /// The grenade or projectile that killed: the camera rides it.
    pub projectile: Option<Entity>,
    /// The killcam's own viewmodel, and the weapon it was built for.
    viewmodel: Option<(Entity, &'static WeaponDef)>,
    /// Stand-ins for the grenades and projectiles of the past, by the
    /// recorded entity.
    ghosts: HashMap<Entity, Entity>,
    /// History time shown last frame, the killer's shot count then, and the
    /// knife swing last played.
    last_t: f32,
    last_shots: Option<u32>,
    last_melee: Option<f32>,
    /// What was hidden for the replay, to show again after.
    hidden: Vec<Entity>,
    /// Where the camera was riding the projectile last (after it's gone).
    ride: Option<Transform>,
}

impl Replay {
    pub fn new(title: &'static str, kill: Kill, names: (String, String), killer_team: Team, window: (f32, f32), now: f32, skippable: bool) -> Replay {
        Replay {
            title,
            kill,
            killer_name: names.0,
            victim_name: names.1,
            killer_team,
            from: window.0,
            to: window.1,
            started: now,
            skippable,
            projectile: None,
            viewmodel: None,
            ghosts: HashMap::new(),
            last_t: window.0,
            last_shots: None,
            last_melee: None,
            hidden: Vec::new(),
            ride: None,
        }
    }

    /// The history time shown now.
    pub fn time(&self, now: f32) -> f32 {
        self.from + (now - self.started)
    }

    pub fn done(&self, now: f32) -> bool {
        self.time(now) >= self.to
    }

    pub fn viewmodel_entity(&self) -> Option<(Entity, &'static WeaponDef)> {
        self.viewmodel
    }

    pub fn hidden_entities(&self) -> &[Entity] {
        &self.hidden
    }
}

/// A pawn between two samples.
fn pawn_at<'a>(a: &'a Frame, b: &'a Frame, k: f32, e: Entity) -> Option<(PawnSample, &'a PawnSample)> {
    let pa = a.pawns.iter().find(|p| p.entity == e);
    let pb = b.pawns.iter().find(|p| p.entity == e);
    let (pa, pb) = match (pa, pb) {
        (Some(x), Some(y)) => (x, y),
        (Some(x), None) | (None, Some(x)) => (x, x),
        _ => return None,
    };
    let mut s = pa.clone();
    // Teleports (respawns) snap rather than slide.
    if pa.feet.distance(pb.feet) < u(64.0) {
        s.feet = pa.feet.lerp(pb.feet, k);
    } else if k > 0.5 {
        s.feet = pb.feet;
    }
    s.yaw = pa.yaw + angle_diff(pb.yaw, pa.yaw) * k;
    s.pitch = pa.pitch + (pb.pitch - pa.pitch) * k;
    s.eye_height = pa.eye_height + (pb.eye_height - pa.eye_height) * k;
    s.ads = pa.ads + (pb.ads - pa.ads) * k;
    s.body_turn = pa.body_turn.slerp(pb.body_turn, k);
    s.anim = match (&pa.anim, &pb.anim) {
        (Some((x, tx)), Some((y, ty))) if std::sync::Arc::ptr_eq(x, y) && ty >= tx => Some((x.clone(), tx + (ty - tx) * k)),
        _ if k > 0.5 => pb.anim.clone(),
        _ => pa.anim.clone(),
    };
    Some((s, pb))
}

fn angle_diff(a: f32, b: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a - b + PI).rem_euclid(TAU) - PI
}

fn projectile_at(a: &Frame, b: &Frame, k: f32, e: Entity) -> Option<(Vec3, Quat, Vec3)> {
    let pa = a.projectiles.iter().find(|p| p.entity == e);
    let pb = b.projectiles.iter().find(|p| p.entity == e);
    match (pa, pb) {
        (Some(x), Some(y)) => Some((x.at.lerp(y.at, k), x.rotation.slerp(y.rotation, k), y.at - x.at)),
        (Some(x), None) | (None, Some(x)) => Some((x.at, x.rotation, Vec3::ZERO)),
        _ => None,
    }
}

/// Bodies take their recorded animations (before they're animated).
pub fn pose_anims(
    time: Res<Time>,
    killcam: Res<super::Killcam>,
    history: Res<History>,
    pawns: Query<&Body>,
    mut players: Query<&mut AnimPlayer>,
    mut poses: ResMut<super::ReplayPoses>,
) {
    poses.0.clear();
    let Some(r) = &killcam.replay else { return };
    let Some((a, b, k)) = history.at(r.time(time.elapsed_secs())) else { return };
    for p in &a.pawns {
        let Some((s, _)) = pawn_at(a, b, k, p.entity) else { continue };
        let Ok(body) = pawns.get(p.entity) else { continue };
        // The spine turned as it was (the body's own turn is a quarter turn
        // less its twist; see `thirdperson::turn_bodies`).
        let (turn, _, _) = s.body_turn.to_euler(EulerRot::YXZ);
        poses.0.insert(body.0, (s.yaw, s.pitch, std::f32::consts::FRAC_PI_2 - turn, s.dead));
        let Some((anim, t)) = s.anim else { continue };
        let Ok(mut player) = players.get_mut(body.0) else { continue };
        // Posed outright: no crossfade from whatever the live game played,
        // and a new animation mapped onto the skeleton afresh (keeping the
        // old one's bone mapping is what made bodies warp).
        player.pose(anim, t);
        player.overlay = s.overlay.clone();
    }
}

/// The camera at the killer's eyes as they were, or riding the projectile
/// that killed; and the stand-ins for the past's grenades.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn place_camera(
    mut commands: Commands,
    time: Res<Time>,
    mut killcam: ResMut<super::Killcam>,
    history: Res<History>,
    mut camera: Single<(&mut Transform, &mut Projection), (With<MainCamera>, Without<super::Ghost>)>,
    mut vm_camera: Single<&mut Projection, (With<ViewModelCamera>, Without<MainCamera>)>,
    mut ghosts: Query<&mut Transform, (With<super::Ghost>, Without<MainCamera>)>,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let now = time.elapsed_secs();
    let Some(r) = killcam.replay.as_mut() else { return };
    let t = r.time(now);
    let Some((a, b, k)) = history.at(t).or_else(|| history.last().map(|f| (f, f, 0.0))) else { return };
    let (cam_tf, proj) = &mut *camera;
    // Riding the projectile once it's in the air.
    let riding = r.projectile.and_then(|e| projectile_at(a, b, k, e));
    if let Some((at, _, vel)) = riding {
        let dir = vel.try_normalize().unwrap_or(cam_tf.forward().as_vec3());
        let flat = Vec3::new(dir.x, 0.0, dir.z).try_normalize().unwrap_or(Vec3::NEG_Z);
        let eye = at - flat * u(48.0) + Vec3::Y * u(14.0);
        r.ride = Some(Transform::from_translation(eye).looking_at(at, Vec3::Y));
    }
    let ride = r.ride.filter(|_| r.projectile.is_some());
    if let Some(ride) = ride {
        **cam_tf = ride;
        set_fov(proj, &mut vm_camera, HIP_FOV);
    } else if let Some((s, _)) = pawn_at(a, b, k, r.kill.attacker) {
        cam_tf.translation = s.feet + Vec3::Y * s.eye_height;
        cam_tf.rotation = Quat::from_euler(EulerRot::YXZ, s.yaw, s.pitch, 0.0);
        let fov = HIP_FOV + (s.weapon.ads_fov.max(1.0) - HIP_FOV) * s.ads * f32::from(s.weapon.ads_fov > 0.0);
        set_fov(proj, &mut vm_camera, fov);
    }
    // Test aid (`COD4RW_KILLCAMTEST_ORBIT=1`): watch the victim's replayed
    // body from behind and to the side instead.
    if std::env::var_os("COD4RW_KILLCAMTEST_ORBIT").is_some() {
        if let Some((s, _)) = pawn_at(a, b, k, r.kill.victim) {
            let back = Quat::from_rotation_y(s.yaw) * Vec3::new(u(70.0), u(60.0), u(150.0));
            **cam_tf = Transform::from_translation(s.feet + back).looking_at(s.feet + Vec3::Y * u(40.0), Vec3::Y);
            set_fov(proj, &mut vm_camera, HIP_FOV);
        }
    }
    // The past's grenades and projectiles.
    let present: Vec<&ProjectileSample> = a.projectiles.iter().chain(b.projectiles.iter()).collect();
    for p in &present {
        if r.ghosts.contains_key(&p.entity) {
            continue;
        }
        let ghost = commands.spawn((Name::new("killcam ghost"), super::Ghost, Transform::from_translation(p.at), Visibility::default())).id();
        if let Some(m) = content.model(&p.model, &mut meshes, &mut materials, &mut images, &mut bindposes) {
            spawn_model(&mut commands, &mut Skeleton::default(), SpawnModel { model: &m, owner: ghost, attach_to: None, layers: None, shadows: true });
        }
        r.ghosts.insert(p.entity, ghost);
    }
    for (&recorded, &ghost) in &r.ghosts {
        let Ok(mut tf) = ghosts.get_mut(ghost) else { continue };
        match projectile_at(a, b, k, recorded) {
            Some((at, rot, _)) => *tf = Transform::from_translation(at).with_rotation(rot),
            // Not yet thrown, or gone: out of sight.
            None => tf.translation = Vec3::splat(-1.0e4),
        }
    }
}

fn set_fov(proj: &mut Projection, vm: &mut Projection, fov: f32) {
    for p in [proj, vm] {
        if let Projection::Perspective(p) = p {
            p.fov = fov.to_radians();
        }
    }
}

/// Bodies where they were: each pawn's body (and everything on it) moved
/// from where the live game put it to where the history has it.
#[allow(clippy::type_complexity)]
pub fn place_bodies(
    time: Res<Time>,
    killcam: Res<super::Killcam>,
    history: Res<History>,
    pawns: Query<&Body>,
    children: Query<&Children>,
    mut globals: Query<&mut GlobalTransform>,
) {
    let Some(r) = &killcam.replay else { return };
    let Some((a, b, k)) = history.at(r.time(time.elapsed_secs())) else { return };
    for p in &a.pawns {
        let Some((s, _)) = pawn_at(a, b, k, p.entity) else { continue };
        let Ok(body) = pawns.get(p.entity) else { continue };
        let Ok(live) = globals.get(body.0).copied() else { continue };
        let root = Transform::from_translation(s.feet).with_rotation(Quat::from_rotation_y(s.yaw));
        let wanted = GlobalTransform::from(root) * Transform::from_rotation(s.body_turn);
        let delta = wanted.affine() * live.affine().inverse();
        let mut stack = vec![body.0];
        while let Some(e) = stack.pop() {
            if let Ok(mut g) = globals.get_mut(e) {
                *g = GlobalTransform::from(delta * g.affine());
            }
            if let Ok(c) = children.get(e) {
                stack.extend(c.iter());
            }
        }
    }
}

/// The past's shots and explosions heard (and seen) again, the killer's
/// gun in hand doing what it did, and who's hidden.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn play_events(
    mut commands: Commands,
    time: Res<Time>,
    mut killcam: ResMut<super::Killcam>,
    history: Res<History>,
    camera: Single<Entity, With<MainCamera>>,
    mut content: ResMut<Content>,
    mut bo1_content: ResMut<crate::bo1::MatchContent>,
    mut waw_content: ResMut<crate::waw::MatchContent>,
    mut assets: (
        ResMut<Assets<Mesh>>,
        ResMut<Assets<StandardMaterial>>,
        ResMut<Assets<Image>>,
        ResMut<Assets<SkinnedMeshInverseBindposes>>,
        ResMut<Assets<CamoMaterial>>,
    ),
    mut camos: Local<CamoCache>,
    mut players: Query<(&mut AnimPlayer, &WeaponAnims)>,
    mut visibility: Query<&mut Visibility>,
    bodies: Query<&Body>,
    live: (Query<Entity, With<crate::grenades::LiveGrenade>>, Query<Entity, With<crate::explosives::Explosive>>),
    mut effects: ResMut<Effects>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let now = time.elapsed_secs();
    let Some(r) = killcam.replay.as_mut() else { return };
    let t = r.time(now);
    let Some((a, b, k)) = history.at(t).or_else(|| history.last().map(|f| (f, f, 0.0))) else { return };
    let killer = pawn_at(a, b, k, r.kill.attacker).map(|(s, _)| s);
    let riding = r.ride.is_some() && r.projectile.is_some();
    // The live game's grenades and the killer's own body are out of sight;
    // the killer's body comes back for the ride.
    let mut hide = |e: Entity, hidden: bool, list: &mut Vec<Entity>| {
        if let Ok(mut v) = visibility.get_mut(e) {
            let want = if hidden { Visibility::Hidden } else { Visibility::Inherited };
            if *v != want {
                *v = want;
                if hidden && !list.contains(&e) {
                    list.push(e);
                }
            }
        }
    };
    for e in live.0.iter().chain(live.1.iter()) {
        hide(e, true, &mut r.hidden);
    }
    if let Ok(body) = bodies.get(r.kill.attacker) {
        hide(body.0, !riding, &mut r.hidden);
    }
    // Shots and explosions since last frame.
    for (at, shot) in history.shots.iter().filter(|(at, _)| *at > r.last_t && *at <= t) {
        let weapon = a.pawns.iter().chain(b.pawns.iter()).find(|p| p.entity == shot.shooter).map(|p| p.weapon);
        let Some(w) = weapon else { continue };
        let mine = shot.shooter == r.kill.attacker && !riding;
        let alias = if mine { &w.sounds.fire_player } else { &w.sounds.fire };
        if !alias.is_empty() {
            sfx.play(alias.clone(), (!mine).then_some(shot.from));
        }
        let _ = at;
    }
    for (_, blast) in history.blasts.iter().filter(|(at, _)| *at > r.last_t && *at <= t) {
        effects.play(&blast.effect, Anchor::Fixed(FxFrame::facing(blast.at, Vec3::Y, 0.0)), FxLayer::World);
        sfx.play(blast.sound.clone(), Some(blast.at));
    }
    r.last_t = t;
    // The killer's gun in hand, rebuilt when they switch.
    let Some(s) = killer.filter(|_| !riding) else {
        if let Some((vm, _)) = r.viewmodel {
            hide(vm, true, &mut Vec::new());
        }
        return;
    };
    let built = r.viewmodel.filter(|(_, d)| std::ptr::eq(*d, s.weapon)).map(|(e, _)| e);
    let vm = match built {
        Some(vm) => vm,
        None => {
            if let Some((old, _)) = r.viewmodel.take() {
                commands.entity(old).despawn();
            }
            let (spec, camo) = s.gun.clone().unwrap_or_else(|| (format!("{}:", s.weapon.name.trim_end_matches("_mp")), 0));
            let gun = crate::gunmodel::parse(&spec).0;
            let content: &mut Content = if crate::bo1::is_bo1(gun) {
                match bo1_content.get() {
                    Some(c) => c,
                    None => return,
                }
            } else if crate::waw::is_waw(gun) {
                match waw_content.get() {
                    Some(c) => c,
                    None => return,
                }
            } else {
                &mut content
            };
            let layer = RenderLayers::layer(VIEWMODEL_LAYER);
            let owner = commands
                .spawn((
                    Name::new("killcam viewmodel"),
                    super::Ghost,
                    crate::viewmodel::rest_transform(),
                    Visibility::default(),
                    layer.clone(),
                    ChildOf(*camera),
                ))
                .id();
            let (meshes, materials, images, bindposes, camo_materials) = &mut assets;
            let mut skeleton = Skeleton::default();
            let hands = crate::viewmodel::team_viewhands(content, r.killer_team == Team::Allies);
            if let Some(h) = content.model(hands, meshes, materials, images, bindposes) {
                spawn_model(&mut commands, &mut skeleton, SpawnModel { model: &h, owner, attach_to: None, layers: Some(layer.clone()), shadows: false });
            }
            let mut a = GunAssets { meshes, materials, images, bindposes, camo_materials };
            let target = GunTarget { owner, attach_to: skeleton.joint("tag_weapon"), layers: Some(layer) };
            crate::gunmodel::spawn_gun(&mut commands, content, &mut camos, &mut a, &mut skeleton, &spec, camo, target);
            let slots: Vec<_> = s.weapon.xanims.iter().map(|n| n.as_deref().filter(|n| !n.is_empty()).and_then(|n| content.anim(n))).collect();
            let anims = WeaponAnims { slots };
            let mut player = AnimPlayer::default();
            if let Some(idle) = anims.slots.get(anim_slot::IDLE).cloned().flatten() {
                player.play(idle, 0.0);
            }
            commands.entity(owner).insert((skeleton, player, anims));
            r.viewmodel = Some((owner, s.weapon));
            r.last_shots = Some(s.shots);
            return;
        }
    };
    hide(vm, false, &mut Vec::new());
    // Firing, aiming and reloading as they did.
    let Ok((mut anim, anims)) = players.get_mut(vm) else { return };
    let slot = |i: usize| anims.slots.get(i).cloned().flatten();
    // A knife swing (slots 7 and 8: melee, the lunge).
    let swing = s.melee.filter(|(started, _)| r.last_melee != Some(*started));
    if let Some((started, charge)) = swing {
        r.last_melee = Some(started);
        if let Some(knife) = slot(if charge { anim_slot::MELEE_CHARGE } else { anim_slot::MELEE }) {
            anim.play(knife, 0.0);
        }
    } else if r.last_shots.is_some_and(|n| s.shots > n) {
        if let Some(fire) = slot(if s.ads > 0.5 { anim_slot::ADS_FIRE } else { anim_slot::FIRE }) {
            anim.play(fire, 0.0);
        }
    } else if anim.finished() {
        if let Some(idle) = slot(if s.reloading { anim_slot::RELOAD } else { anim_slot::IDLE }) {
            anim.play(idle, 0.15);
        }
    }
    r.last_shots = Some(s.shots);
    anim.overlay = (s.ads > 0.0).then(|| slot(anim_slot::ADS_UP).map(|a| (a, s.ads))).flatten();
}
