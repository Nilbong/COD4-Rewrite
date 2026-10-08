//! Third-person character models: the characters pawns wear
//! ([`crate::wardrobe`]; otherwise the map's team bodies and heads), the
//! world weapon in the right hand, and CoD4's player animations as
//! `mp/playeranim.script` picks them: by stance, movetype, direction and the
//! gun's class for the legs, with torso events (firing, reloading, throws,
//! flinches, weapon switches, the knife) over them, and the script's deaths.

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
        app.init_resource::<Gaits>().init_resource::<crate::gunmodel::WorldCamos>().add_systems(
            Update,
            (attach_bodies, hold_guns, drive_body_anims).chain().in_set(BodyAnimSet).after(crate::movement::MovementSet).run_if(crate::state::in_game),
        )
            .add_systems(
                PostUpdate,
                turn_bodies
                    .in_set(BodyPoseSet)
                    .after(crate::models::animate_skeletons)
                    .before(bevy::transform::TransformSystems::Propagate)
                    .run_if(crate::state::in_game),
            );
    }
}

/// Bodies made, armed and animated (each frame, before they're posed).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BodyAnimSet;

/// Bodies turned and bent to their aim (after they're animated).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BodyPoseSet;

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
    // Cover ([`crate::cover`]): the campaign's, loaded on a thread of their own.
    if crate::cover::is_cover_anim(name) {
        return crate::cover::anim(name);
    }
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

/// A body gone limp ([`crate::ragdoll`]): it holds no gun any more.
#[derive(Component)]
pub struct Limp;

/// The gun a body holds: which weapon it is and the joints and surfaces it
/// added to the body.
#[derive(Component)]
pub(crate) struct HeldGun {
    def: usize,
    pub(crate) first_joint: usize,
    pub(crate) joints: Vec<Entity>,
    pub(crate) surfaces: Vec<Entity>,
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
    pawns: Query<(&Body, &crate::weapons::WeaponState, Option<&crate::loadout::Loadout>, Option<&crate::grenades::Offhand>, Has<Dead>)>,
    mut owners: Query<(&mut Skeleton, &mut AnimPlayer, Option<&HeldGun>, Option<&mut LocalBody>), Without<Limp>>,
    layers: Query<&RenderLayers>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut camo_materials: ResMut<Assets<crate::gunmodel::CamoMaterial>>,
    mut camos: ResMut<crate::gunmodel::WorldCamos>,
) {
    for (body, weapon, loadout, offhand, dead) in &pawns {
        // The pin out, the grenade's in the hand instead (its world model).
        let grenade = offhand.filter(|o| matches!(o.phase, crate::grenades::Phase::Pullback | crate::grenades::Phase::Hold)).map(|o| o.def);
        let def = grenade.map_or(weapon.def, |g| g) as *const _ as usize;
        let Ok((mut skeleton, mut anim, held, local_body)) = owners.get_mut(body.0) else { continue };
        // The dead hold nothing: their gun dropped ([`crate::pickups`]).
        if dead {
            if let Some(h) = held.filter(|h| !h.joints.is_empty()) {
                for &e in h.joints.iter().chain(&h.surfaces) {
                    commands.entity(e).try_despawn();
                }
                skeleton.truncate(h.first_joint);
                anim.remap();
                commands.entity(body.0).insert(HeldGun { def: 0, first_joint: h.first_joint, joints: Vec::new(), surfaces: Vec::new() });
            }
            continue;
        }
        if held.is_some_and(|h| h.def == def) {
            continue;
        }
        if let Some(g) = grenade {
            let Some(model) = content.model(&g.world_model, &mut meshes, &mut materials, &mut images, &mut bindposes) else { continue };
            if let Some(h) = held {
                for &e in h.joints.iter().chain(&h.surfaces) {
                    commands.entity(e).try_despawn();
                }
                skeleton.truncate(h.first_joint);
            }
            let first_joint = skeleton.joints.len();
            let hand = skeleton.joint("tag_weapon_right");
            let shown = local_body.as_ref().and_then(|b| b.0.first()).and_then(|&e| layers.get(e).ok()).cloned();
            let surfaces = spawn_model(&mut commands, &mut skeleton, SpawnModel { model: &model, owner: body.0, attach_to: hand, layers: shown, shadows: true });
            let joints = skeleton.joints[first_joint..].iter().map(|j| j.entity).collect();
            if let Some(mut local) = local_body {
                let old: Vec<Entity> = held.map(|h| h.surfaces.clone()).unwrap_or_default();
                local.0.retain(|e| !old.contains(e));
                local.0.extend(surfaces.iter().copied());
            }
            anim.remap();
            commands.entity(body.0).insert(HeldGun { def, first_joint, joints, surfaces });
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
            crate::gunmodel::spawn_held_gun(&mut commands, content, &mut camos.0, &mut assets, &mut skeleton, &spec, camo, target)
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

/// How a pawn is moving, as `PM_Footsteps` sorts it into the script's
/// movetypes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Motion {
    #[default]
    Idle,
    /// Aiming down the sights (or leaning): `walk`, `walkcr`.
    Walk(Dir),
    Run(Dir),
    Sprint,
}

/// Forward, back, or strafing (`player_strafeAnimCosAngle`: more than 60
/// degrees off the view's axis).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    Forward,
    Back,
    Left,
    Right,
}

impl Motion {
    /// The same gait the other way.
    fn with_dir(self, d: Dir) -> Motion {
        match self {
            Motion::Walk(_) => Motion::Walk(d),
            Motion::Run(_) => Motion::Run(d),
            other => other,
        }
    }
}

impl Dir {
    fn name(self) -> &'static str {
        match self {
            Dir::Forward => "forward",
            Dir::Back => "back",
            Dir::Left => "left",
            Dir::Right => "right",
        }
    }
}

/// The script's weapon classes, as far as the body shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    Rifle,
    Pistol,
    Rocket,
    /// A grenade in hand, the pin out (cooking a frag).
    Grenade,
}

/// `weapClass` 4 pistol, 6 rocket launcher (or `playerAnimType`
/// rocketlauncher).
fn held(def: &crate::weapons::WeaponDef) -> Held {
    match (def.class, def.player_anim) {
        (4, _) | (_, 2) => Held::Pistol,
        (6, _) | (_, 7) => Held::Rocket,
        _ => Held::Rifle,
    }
}

/// `player_moveThreshhold`: slower than this is standing still (units/s).
const MOVE_THRESHOLD: f32 = 10.0;
/// `player_strafeAnimCosAngle`.
const STRAFE_COS: f32 = 0.5;

/// Leeway either side of the thresholds, so a pawn on the edge doesn't
/// flick between animations every frame.
const HYSTERESIS: f32 = 0.1;

/// The movetype, and where the legs turn from the view (`movementDir`:
/// toward the way they're going, at most 90 degrees; strafing, none).
fn motion(mover: &Mover, view_yaw: f32, walking: bool, prev: Motion) -> (Motion, f32) {
    let vel = Vec3::new(mover.velocity.x, 0.0, mover.velocity.z);
    let speed = vel.length();
    let threshold = MOVE_THRESHOLD * if prev == Motion::Idle { 1.0 + HYSTERESIS * 5.0 } else { 1.0 };
    if speed < crate::units::u(threshold) {
        return (Motion::Idle, 0.0);
    }
    let was_strafing = matches!(prev, Motion::Walk(Dir::Left | Dir::Right) | Motion::Run(Dir::Left | Dir::Right));
    let strafe_cos = STRAFE_COS + if was_strafing { HYSTERESIS } else { -HYSTERESIS };
    let forward = Quat::from_rotation_y(view_yaw) * Vec3::NEG_Z;
    let right = Quat::from_rotation_y(view_yaw) * Vec3::X;
    let (f, s) = (vel.dot(forward) / speed, vel.dot(right) / speed);
    let half = std::f32::consts::FRAC_PI_2;
    let (dir, offset) = if f.abs() <= strafe_cos && mover.stance != Stance::Prone {
        (if s > 0.0 { Dir::Right } else { Dir::Left }, 0.0)
    } else if f < 0.0 {
        // Backpedalling: the legs face away from the way they go.
        (Dir::Back, (s).atan2(-f).clamp(-half, half))
    } else {
        // Yaw grows to the left.
        (Dir::Forward, (-s).atan2(f).clamp(-half, half))
    };
    let offset = if mover.stance == Stance::Prone { 0.0 } else { offset };
    let m = match () {
        _ if mover.sprinting && mover.stance == Stance::Stand => Motion::Sprint,
        _ if walking && mover.stance != Stance::Prone => Motion::Walk(dir),
        _ => Motion::Run(dir),
    };
    (m, offset)
}

/// The legs' (or whole body's) animation for a stance, movetype and gun:
/// `playeranim.script`'s `STATE COMBAT`.
fn base_anim(stance: Stance, m: Motion, held: Held, ads: bool) -> String {
    use Held::*;
    let s = |x: &str| x.to_owned();
    if held == Grenade {
        return grenade_anim(stance, m);
    }
    match (stance, m) {
        (Stance::Stand, Motion::Idle) => s(match (held, ads) {
            (Pistol, true) => "pb_stand_ads_pistol",
            (Pistol, false) => "pb_stand_alert_pistol",
            (Rocket, true) => "pb_stand_ads_RPG",
            (Rocket, false) => "pb_stand_alert_RPG",
            (Rifle | Grenade, true) => "pb_stand_ads",
            (Rifle | Grenade, false) => "pb_stand_alert",
        }),
        (Stance::Crouch, Motion::Idle) => s(match (held, ads) {
            (Pistol, true) => "pb_crouch_ads_pistol",
            (Pistol, false) => "pb_crouch_alert_pistol",
            (Rocket, true) => "pb_crouch_ads_RPG",
            (Rocket, false) => "pb_crouch_alert_RPG",
            (Rifle | Grenade, true) => "pb_crouch_ads",
            (Rifle | Grenade, false) => "pb_crouch_alert",
        }),
        (Stance::Prone, Motion::Idle) => s(match held {
            Pistol => "pb_prone_aim_pistol",
            Rocket => "pb_prone_aim_RPG",
            Rifle | Grenade => "pb_prone_aim",
        }),
        (Stance::Prone, Motion::Walk(d) | Motion::Run(d)) => s(match d {
            Dir::Forward => "pb_prone_crawl",
            Dir::Back => "pb_prone_crawl_back",
            Dir::Left => "pb_prone_crawl_left",
            Dir::Right => "pb_prone_crawl_right",
        }),
        (_, Motion::Sprint) => s(match held {
            Pistol => "pb_sprint_pistol",
            Rocket => "pb_sprint_RPG",
            Rifle | Grenade => "pb_sprint",
        }),
        (Stance::Stand, Motion::Walk(d)) => match held {
            Rocket => format!("pb_walk_{}_RPG_ads", d.name()),
            Pistol => format!("pb_combatwalk_{}_loop_pistol", d.name()),
            Rifle | Grenade => format!("pb_stand_shoot_walk_{}", d.name()),
        },
        (Stance::Crouch, Motion::Walk(d)) => match held {
            Rocket => format!("pb_crouch_walk_{}_RPG", d.name()),
            Pistol => format!("pb_crouch_walk_{}_pistol", d.name()),
            Rifle | Grenade => format!("pb_crouch_shoot_run_{}", d.name()),
        },
        (Stance::Stand, Motion::Run(d)) => match (held, d) {
            (Rocket, _) => format!("pb_combatrun_{}_RPG", d.name()),
            (Pistol, Dir::Forward) => s("pb_pistol_run_fast"),
            (Pistol, _) => format!("pb_combatrun_{}_loop_pistol", d.name()),
            (Rifle | Grenade, _) => format!("pb_combatrun_{}_loop", d.name()),
        },
        (Stance::Crouch, Motion::Run(d)) => match held {
            Rocket => format!("pb_crouch_run_{}_RPG", d.name()),
            Pistol => format!("pb_crouch_run_{}_pistol", d.name()),
            Rifle | Grenade => format!("pb_crouch_run_{}", d.name()),
        },
    }
}

fn angle_blend_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("COD4RW_ANIMBLEND").is_some())
}

/// For [`angle_blend_on`]: moving (walk or run, not prone or sprinting),
/// the forward-or-back direction, the side one, and the side's weight
/// (0 straight ahead, 1 straight sideways).
fn angle_blend(mover: &Mover, view_yaw: f32, m: Motion) -> Option<(Dir, Dir, f32)> {
    if !angle_blend_on() || mover.stance == Stance::Prone || !matches!(m, Motion::Walk(_) | Motion::Run(_)) {
        return None;
    }
    let vel = Vec3::new(mover.velocity.x, 0.0, mover.velocity.z);
    let speed = vel.length();
    if speed < 1e-3 {
        return None;
    }
    let forward = Quat::from_rotation_y(view_yaw) * Vec3::NEG_Z;
    let right = Quat::from_rotation_y(view_yaw) * Vec3::X;
    let (f, s) = (vel.dot(forward) / speed, vel.dot(right) / speed);
    let w = s.abs().atan2(f.abs()) / std::f32::consts::FRAC_PI_2;
    Some((if f >= 0.0 { Dir::Forward } else { Dir::Back }, if s >= 0.0 { Dir::Right } else { Dir::Left }, w.clamp(0.0, 1.0)))
}

/// Headquarters' relaxed standing and walking, from the single-player
/// level: a briefing idle, a patrol walk and a jog, gun held low.
fn hq_anim(stance: Stance, m: Motion) -> Option<&'static str> {
    match (stance, m) {
        (Stance::Stand, Motion::Idle) => Some("killhouse_sas_1_idle"),
        (Stance::Stand, Motion::Walk(Dir::Forward)) => Some("patrol_bored_patrolwalk"),
        (Stance::Stand, Motion::Run(Dir::Forward)) => Some("combat_jog"),
        _ => None,
    }
}

/// `weaponclass grenade`: holding one ready to throw.
fn grenade_anim(stance: Stance, m: Motion) -> String {
    let s = |x: &str| x.to_owned();
    match (stance, m) {
        (Stance::Stand, Motion::Idle) => s("pb_stand_grenade_pullpin"),
        (Stance::Crouch, Motion::Idle) => s("pb_crouch_grenade_pullpin"),
        (Stance::Prone, Motion::Idle) => s("pb_prone_aim_grenade"),
        (Stance::Prone, Motion::Walk(Dir::Forward) | Motion::Run(Dir::Forward)) => s("pb_prone_grenade_crawl"),
        (Stance::Prone, Motion::Walk(d) | Motion::Run(d)) => format!("pb_prone_grenade_crawl_{}", d.name()),
        (_, Motion::Sprint) => s("pb_sprint"),
        (Stance::Stand, Motion::Walk(Dir::Forward) | Motion::Run(Dir::Forward)) => s("pb_combatrun_forward_loop_stickgrenade"),
        (Stance::Stand, Motion::Walk(d) | Motion::Run(d)) => format!("pb_combatrun_{}_loop_grenade", d.name()),
        (Stance::Crouch, Motion::Walk(d)) => format!("pb_crouch_walk_{}_pistol", d.name()),
        (Stance::Crouch, Motion::Run(d)) => format!("pb_crouch_run_{}_grenade", d.name()),
    }
}

/// Hit while moving: the script's `stumble_*` in place of the run or walk.
fn stumble_anim(m: Motion, held: Held) -> Option<String> {
    let pistol = held == Held::Pistol;
    Some(match m {
        Motion::Idle => return None,
        Motion::Sprint => "pb_stumble_forward".to_owned(),
        Motion::Walk(d) if pistol => format!("pb_stumble_pistol_walk_{}", d.name()),
        Motion::Walk(d) => format!("pb_stumble_walk_{}", d.name()),
        Motion::Run(d) if pistol => format!("pb_stumble_pistol_{}", d.name()),
        Motion::Run(d) => format!("pb_stumble_{}", d.name()),
    })
}

/// `fireweapon`: the torso's recoil, and how long it holds (the automatic
/// guns' `duration 150`). Moving shows none.
fn fire_anim(def: &crate::weapons::WeaponDef, stance: Stance, m: Motion, ads: bool, downed: bool) -> Option<(&'static str, Option<f32>)> {
    if downed {
        return Some(("pt_laststand_fire", None));
    }
    let moving = m != Motion::Idle;
    let (prone, crouch) = (stance == Stance::Prone, stance == Stance::Crouch);
    let auto = Some(0.15);
    Some(match held(def) {
        // (Thrown, not fired: see [`throw_anim`].)
        Held::Grenade => return None,
        Held::Pistol if prone => ("pt_prone_shoot_pistol", None),
        Held::Pistol if moving => return None,
        Held::Pistol if crouch && ads => ("pt_crouch_shoot_ads_pistol", None),
        Held::Pistol if crouch => ("pt_crouch_shoot_pistol", None),
        Held::Pistol if ads => ("pb_stand__shoot_ads_pistol", None),
        Held::Pistol => ("pt_stand_shoot_pistol", None),
        Held::Rocket if moving && !prone => return None,
        Held::Rocket if crouch => ("pt_crouch_shoot_ads", None),
        Held::Rocket if prone => ("pt_prone_shoot_RPG", None),
        Held::Rocket => ("pt_stand_shoot_RPG", None),
        // `weaponclass autofire`: machine guns and SMGs.
        Held::Rifle if matches!(def.class, 1 | 2) => match () {
            _ if prone => ("pt_prone_shoot_auto", auto),
            _ if moving => return None,
            _ if crouch && ads => ("pt_crouch_shoot_auto_ads", auto),
            _ if crouch => ("pt_crouch_shoot_auto", auto),
            _ if ads => ("pt_stand_shoot_auto_ads", auto),
            _ => ("pt_stand_shoot_auto", auto),
        },
        // `playerAnimType other`: shotguns.
        Held::Rifle if def.player_anim == 1 => (if prone { "pt_prone_shoot_auto" } else { "pt_stand_shoot_shotgun" }, None),
        Held::Rifle => {
            let sniper = def.player_anim == 6;
            match () {
                _ if prone => ("pt_prone_shoot_auto", None),
                _ if moving => return None,
                _ if crouch && ads => ("pt_crouch_shoot_ads", None),
                _ if crouch => ("pt_crouch_shoot", None),
                _ if sniper && ads => ("pt_rifle_fire_ads", None),
                _ if sniper => ("pt_rifle_fire", None),
                _ if ads => ("pt_stand_shoot_ads", None),
                _ => ("pt_stand_shoot", None),
            }
        }
    })
}

/// `reload`.
fn reload_anim(def: &crate::weapons::WeaponDef, stance: Stance, m: Motion, downed: bool) -> &'static str {
    if downed {
        return "pt_laststand_reload";
    }
    let (prone, crouch) = (stance == Stance::Prone, stance == Stance::Crouch);
    let crouch_still = crouch && m == Motion::Idle;
    match (held(def), def.player_anim) {
        (Held::Pistol, _) if crouch_still => "pt_reload_crouch_pistol",
        (Held::Pistol, _) if crouch => "pt_reload_crouchwalk_pistol",
        (Held::Pistol, _) if prone => "pt_reload_prone_pistol",
        (Held::Pistol, _) => "pt_reload_stand_pistol",
        (Held::Rocket, _) if prone => "pt_reload_prone_RPG",
        (Held::Rocket, _) => "pt_reload_stand_RPG",
        (_, 3) if prone => "pt_reload_prone_auto",
        (_, 3) if crouch && !crouch_still => "pt_reload_crouchwalk",
        (_, 3) => "pt_reload_stand_auto_mp40",
        (_, 4) if prone => "pt_reload_prone_auto",
        (_, 4) if crouch_still => "pt_reload_crouch_rifle",
        (_, 4) if crouch => "pt_reload_crouchwalk",
        (_, 4) => "pt_reload_stand_auto",
        _ if crouch => "pt_reload_crouch_rifle",
        _ if prone => "pt_reload_prone_auto",
        _ => "pt_reload_stand_rifle",
    }
}

/// The grenade's throw: standing or crouched still, the whole body.
fn throw_anim(stance: Stance, m: Motion) -> &'static str {
    match (stance, m) {
        (Stance::Prone, _) => "pt_prone_grenade_throw",
        (Stance::Crouch, Motion::Idle) => "pb_crouch_grenade_throw",
        (Stance::Crouch, _) => "pt_crouch_grenade_throw",
        (Stance::Stand, Motion::Idle) => "pb_stand_grenade_throw",
        (Stance::Stand, _) => "pt_stand_grenade_throw",
    }
}

/// `DEATH`, by movetype only (CoD4 doesn't look at where the shot hit).
/// Crouched and walking it takes the crouched deaths (the script would
/// stand it up for the standing ones).
fn death_anims(stance: Stance, m: Motion, downed: bool) -> &'static [&'static str] {
    const CROUCH: &[&str] =
        &["pb_crouch_death_headshot_front", "pb_crouch_death_clutchchest", "pb_crouch_death_flip", "pb_crouch_death_fetal", "pb_crouch_death_falltohands"];
    const RUN: &[&str] = &["pb_death_run_forward_crumple", "pb_death_run_onfront", "pb_death_run_stumble"];
    const STAND: &[&str] = &[
        "pb_stand_death_neckdeath",
        "pb_stand_death_headchest_topple",
        "pb_stand_death_frontspin",
        "pb_stand_death_nervedeath",
        "pb_stand_death_legs",
        "pb_stand_death_lowerback",
        "pb_stand_death_head_collapse",
        "pb_stand_death_neckdeath_thrash",
    ];
    match (stance, m) {
        _ if downed => &["pb_laststand_death"],
        (Stance::Prone, _) => &["pb_prone_death_quickdeath"],
        (Stance::Stand, Motion::Run(Dir::Back)) => &["pb_death_run_back"],
        (_, Motion::Run(Dir::Left)) => &["pb_death_run_left"],
        (_, Motion::Run(Dir::Right)) => &["pb_death_run_right"],
        (Stance::Crouch, Motion::Run(_)) => &["pb_crouchrun_death_drop", "pb_crouchrun_death_crumple"],
        (Stance::Crouch, _) => CROUCH,
        (Stance::Stand, Motion::Run(_) | Motion::Sprint) => RUN,
        _ => STAND,
    }
}

/// `BG_RunLerpFrameRate`: a move animation plays at the pawn's speed over
/// its own (its root motion over its length), so the feet keep to the
/// ground; at least 0.1, at most 3 for slow animations, 2 for fast.
fn move_rate(anim: &XAnim, speed: f32) -> f32 {
    let d = anim.delta_at(1.0);
    let own = Vec2::new(d[0], d[1]).length() / anim.duration().max(1e-3);
    if !anim.delta || own < 1.0 {
        return 1.0;
    }
    let max = if own < 20.0 {
        3.0
    } else if own < 150.0 {
        3.0 - (own - 20.0) / 130.0
    } else {
        2.0
    };
    (speed / crate::units::u(1.0) / own).clamp(0.1, max)
}

/// Where a body's legs go, for [`turn_bodies`]: their offset from the view
/// and whether they must follow it (moving) or may lag behind.
#[derive(Clone, Copy, Default)]
struct Gait {
    offset: f32,
    moving: bool,
}

#[derive(Resource, Default)]
struct Gaits(std::collections::HashMap<Entity, Gait>);

/// `bg_legYawTolerance`: standing, the legs stay put until the view is this
/// far round from them; `bg_swingSpeed`, how fast they swing (per ms, by
/// the angle left); and how far they can trail (150 degrees).
const LEGS_TOLERANCE: f32 = 20.0;
const SWING_SPEED: f32 = 0.2;
const LEGS_CLAMP: f32 = 150.0;
/// Prone the legs keep with the view.
const LEGS_FOLLOW: f32 = 12.0;
/// The controller bones (`BG_Player_DoControllers`: back_low, back_mid,
/// back_up) and their shares of the torso's pitch, yaw and lean roll.
const SPINE: [(&str, f32, f32, f32); 3] = [("j_spinelower", 0.2, 0.4, 0.5), ("j_spineupper", 0.3, 0.4, 0.5), ("j_spine4", 0.5, 0.2, -0.6)];
/// The neck and head: their shares of the head's turn (the view less the
/// torso's).
const HEAD: [(&str, f32); 2] = [("j_neck", 0.3), ("j_head", 0.7)];
/// Leaning's roll at full lean (degrees: 50 x 0.925 x
/// `player_lean_rotate_*` 1.25).
const LEAN_ROLL: f32 = 50.0 * 0.925 * 1.25;

/// Legs and torso, as CoD4's `BG_PlayerAngles` does it: standing still, the
/// legs keep facing until the view has turned more than
/// [`LEGS_TOLERANCE`] from them, then swing round; moving, they turn toward
/// where they're going (the forward and back animations cover diagonals).
/// The spine turns the rest of the way, pitches with the view and rolls
/// with a lean; prone, the torso pitches less and the head the rest.
#[allow(clippy::type_complexity)]
fn turn_bodies(
    time: Res<Time>,
    spatial: avian3d::prelude::SpatialQuery,
    pawns: Query<(Entity, &Body, &Mover, &crate::movement::ViewAngles, Has<Dead>)>,
    mut owners: Query<(&mut Transform, &Skeleton), With<BodyOwner>>,
    mut joints: Query<&mut Transform, Without<BodyOwner>>,
    parents: Query<(&ChildOf, &GlobalTransform)>,
    globals: Query<&GlobalTransform>,
    replay: Res<crate::killcam::ReplayPoses>,
    gaits: Res<Gaits>,
    mut legs: Local<std::collections::HashMap<Entity, (f32, bool)>>,
    mut slopes: Local<std::collections::HashMap<Entity, (f32, f32)>>,
) {
    let _t = crate::perf::Probe::start("turn_bodies");
    let dt = time.delta_secs();
    let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    let filter = crate::collision::movement_filter();
    for (pawn, body, mover, view, dead) in &pawns {
        let Ok((mut owner_tf, skeleton)) = owners.get_mut(body.0) else { continue };
        // A killcam replaying this body: the view and twist it had then.
        let (view_yaw, view_pitch, twist, dead, lean, replayed) = match replay.0.get(&body.0) {
            Some(r) => (r.yaw, r.pitch, r.twist, r.dead, r.lean, Some(r.body_turn)),
            None => {
                let gait = gaits.0.get(&body.0).copied().unwrap_or_default();
                let target = wrap(view.yaw + gait.offset);
                let (yaw, swinging) = legs.entry(body.0).or_insert((view.yaw, false));
                let diff = wrap(target - *yaw);
                if dead {
                    *yaw = view.yaw;
                } else if mover.stance == Stance::Prone {
                    *yaw += diff * (1.0 - (-LEGS_FOLLOW * dt).exp());
                } else {
                    if gait.moving || diff.abs() > LEGS_TOLERANCE.to_radians() {
                        *swinging = true;
                    }
                    if *swinging {
                        // Degrees per ms: the speed, faster the further.
                        let rate = (SWING_SPEED * (diff.abs().to_degrees() * 0.05).max(0.5) * 1000.0).to_radians();
                        let step = (rate * dt).min(diff.abs());
                        *yaw += step * diff.signum();
                        *swinging = gait.moving || wrap(target - *yaw).abs() > 0.01;
                    }
                    // Never trailing more than 150 degrees.
                    let behind = wrap(*yaw - target);
                    let clamp = LEGS_CLAMP.to_radians();
                    if behind.abs() > clamp {
                        *yaw = target + clamp * behind.signum();
                    }
                }
                *yaw = wrap(*yaw);
                (view.yaw, view.pitch, wrap(view.yaw - *yaw), dead, if dead { 0.0 } else { mover.lean }, None)
            }
        };
        // Prone, the body lies along the ground (`BG_CheckProne`): the torso
        // pitched to the ground 18 units ahead, the waist to where the feet
        // are, bent at most 50 degrees down and 70 up from the torso.
        // (A replay has the tilt in its recorded turn.)
        let prone = mover.stance == Stance::Prone && !dead && replayed.is_none();
        let (torso_slope, waist_slope) = {
            let target = match (prone, globals.get(pawn)) {
                (true, Ok(at)) => ground_slopes(&spatial, &filter, at.translation(), view_yaw - twist),
                _ => (0.0, 0.0),
            };
            let s = slopes.entry(body.0).or_insert(target);
            let k = 1.0 - (-PRONE_SLOPE_RATE * dt).exp();
            s.0 += (target.0 - s.0) * k;
            s.1 += (target.1 - s.1) * k;
            *s
        };
        // The body hangs off the pawn, which faces the view (and CoD models
        // face +X: a quarter turn; about their Z, the nose comes up).
        owner_tf.rotation = replayed.unwrap_or(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - twist) * Quat::from_rotation_z(torso_slope));
        if dead {
            continue;
        }
        // Prone the torso pitches half as far up and a quarter as far down,
        // the head the rest.
        let prone = mover.stance == Stance::Prone;
        let torso_pitch = match () {
            _ if !prone => view_pitch,
            _ if view_pitch > 0.0 => view_pitch * 0.5,
            _ => view_pitch * 0.25,
        };
        let head_pitch = view_pitch - torso_pitch;
        let roll = (lean * LEAN_ROLL).to_radians();
        // About the world's up, the view's right and its forward, in each
        // joint's parent's frame.
        let turn = Quat::from_rotation_y(view_yaw);
        let (right, forward) = (turn * Vec3::X, turn * Vec3::NEG_Z);
        let mut apply = |name: &str, yaw: f32, pitch: f32, roll: f32| {
            let Some(joint) = skeleton.joint(name) else { return };
            let Ok((child_of, _)) = parents.get(joint) else { return };
            let Ok(parent) = globals.get(child_of.parent()) else { return };
            let Ok(mut tf) = joints.get_mut(joint) else { return };
            let p = parent.compute_transform().rotation;
            let world = Quat::from_rotation_y(yaw) * Quat::from_axis_angle(right, pitch) * Quat::from_axis_angle(forward, roll);
            tf.rotation = p.inverse() * world * p * tf.rotation;
        };
        // The hips and legs bend from the torso's slope to the waist's.
        if (waist_slope - torso_slope).abs() > 1e-3 {
            apply("pelvis", 0.0, waist_slope - torso_slope, 0.0);
        }
        for (name, pitch, yaw, rolls) in SPINE {
            // Prone the top of the back takes the whole pitch.
            // (and the bottom of it undoes the hips' bend).
            let pitch = match () {
                _ if !prone => torso_pitch * pitch,
                _ if name == "j_spine4" => torso_pitch,
                _ if name == "j_spinelower" => torso_slope - waist_slope,
                _ => 0.0,
            };
            apply(name, twist * yaw, pitch, roll * rolls);
        }
        if head_pitch.abs() > 1e-3 || roll.abs() > 1e-3 {
            for (name, share) in HEAD {
                let roll = if name == "j_head" { -0.3 * roll } else { 0.0 };
                apply(name, 0.0, head_pitch * share, roll);
            }
        }
    }
    legs.retain(|e, _| owners.contains(*e));
    slopes.retain(|e, _| owners.contains(*e));
}

/// `BG_CheckProne`'s probes: how far ahead the torso's and behind the
/// waist's (`prone_feet_dist` 45..50, less 6) the ground is found.
const PRONE_TORSO_AHEAD: f32 = 18.0;
const PRONE_WAIST_BEHIND: f32 = 41.0;
/// The waist's bend from the torso: down, up (degrees).
const PRONE_WAIST_BEND: (f32, f32) = (-50.0, 70.0);
/// How fast the lie of the body follows the ground (per second).
const PRONE_SLOPE_RATE: f32 = 8.0;

/// The ground's pitch (nose up positive) ahead of a prone pawn's feet
/// position and behind it, along the legs' heading.
fn ground_slopes(spatial: &avian3d::prelude::SpatialQuery, filter: &avian3d::prelude::SpatialQueryFilter, at: Vec3, legs_yaw: f32) -> (f32, f32) {
    use crate::units::u;
    let forward = Quat::from_rotation_y(legs_yaw) * Vec3::NEG_Z;
    // The ground's height near `p`: from a little above, down a little more.
    let ground = |p: Vec3| {
        let from = p + Vec3::Y * u(24.0);
        spatial.cast_ray(from, Dir3::NEG_Y, u(48.0), true, filter).map(|h| from.y - h.distance)
    };
    let Some(here) = ground(at) else { return (0.0, 0.0) };
    let pitch = |dist: f32, sign: f32| ground(at + forward * u(dist) * sign).map_or(0.0, |h| (sign * (h - here)).atan2(u(dist)));
    let torso = pitch(PRONE_TORSO_AHEAD, 1.0);
    let waist = pitch(PRONE_WAIST_BEHIND, -1.0);
    let (down, up) = PRONE_WAIST_BEND;
    (torso, torso + (waist - torso).clamp(down.to_radians(), up.to_radians()))
}

/// A torso animation over the legs' (`setTimer` events: firing,
/// reloading, a throw, a flinch, putting the gun away), until `until`.
struct TorsoEvent {
    anim: Arc<XAnim>,
    started: f32,
    until: f32,
    fade_in: f32,
    /// Held at its first frame (a pose) rather than played.
    hold: bool,
}

/// What a body is doing, between frames.
#[derive(Default)]
struct BodyState {
    /// Last frame in a moving animation.
    moving: bool,
    airborne_since: Option<f32>,
    /// The highest it got while off the ground (for the landing).
    peak: f32,
    /// Showing the take-off / in-air pose (so landing plays the landing).
    in_air_pose: bool,
    landing_until: f32,
    torso: Option<TorsoEvent>,
    shots: u32,
    reloading: bool,
    throwing: bool,
    /// Hit: `damageTimer`'s flinch or stumble until then, and from where.
    hurt_until: f32,
    hurt_from: Option<Dir>,
    stance: Stance,
    motion: Motion,
    /// Getting up from prone: the transition holds the legs until then.
    stance_until: f32,
    dead: bool,
}

/// Most of a landing is the recovery; cut back to running after this.
const LAND_FOR: f32 = 0.35;
/// `LAND` plays for falls over 12 units.
const LAND_HEIGHT: f32 = 12.0;
/// Walking off a ledge looks like a jump once the fall lasts this long.
const FALL_POSE_AFTER: f32 = 0.25;
/// A torso event lasts its animation (at least half a second) or the
/// script's `duration`, and 50 ms more; it blends out over this.
const TORSO_MIN: f32 = 0.5;
const TORSO_EXTRA: f32 = 0.05;
const TORSO_FADE_OUT: f32 = 0.15;
/// `player_dmgtimer_timePerPoint`, `_maxTime`, `_flinchTime` (s).
const HURT_PER_POINT: f32 = 0.1;
const HURT_MAX: f32 = 0.75;
const HURT_FLINCH: f32 = 0.5;

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn drive_body_anims(
    mut content: ResMut<Content>,
    pawns: Query<(
        Entity,
        &Body,
        &Mover,
        &crate::movement::ViewAngles,
        Option<&WeaponInput>,
        Option<&crate::weapons::WeaponState>,
        Option<&crate::loadout::Loadout>,
        Option<&crate::grenades::Offhand>,
        Option<&crate::melee::Melee>,
        Has<Dead>,
        Has<crate::perks::Downed>,
        Option<&crate::cover::InCover>,
        Option<&crate::cover::LeavingCover>,
    )>,
    places: Query<&GlobalTransform>,
    mut players: Query<(&mut AnimPlayer, Has<BlackOpsRig>)>,
    mut damage: MessageReader<crate::combat::Damage>,
    mantles: Option<Res<crate::movement::MantleAnims>>,
    mut gaits: ResMut<Gaits>,
    mut states: Local<std::collections::HashMap<Entity, BodyState>>,
    replay: Res<crate::killcam::ReplayPoses>,
    time: Res<Time>,
) {
    let _t = crate::perf::Probe::start("drive_body_anims");
    let now = time.elapsed_secs();
    let mut rng = rand::rng();
    // Hits: how long they shake the body, and which way (`damageTimer`).
    for d in damage.read() {
        let Ok((_, body, _, view, ..)) = pawns.get(d.target) else { continue };
        let state = states.entry(body.0).or_default();
        let length = (d.amount * HURT_PER_POINT).min(HURT_MAX);
        state.hurt_until = now + length.min(HURT_FLINCH);
        let (Some(a), Ok(t)) = (d.attacker.and_then(|a| places.get(a).ok()), places.get(d.target)) else {
            state.hurt_from = Some(Dir::Forward);
            continue;
        };
        // The damage's direction from the view: forward is being hit from
        // in front.
        let toward = (t.translation() - a.translation()).with_y(0.0);
        let local = Quat::from_rotation_y(-view.yaw) * toward;
        let yaw = (-local.x).atan2(-local.z).to_degrees().rem_euclid(360.0);
        state.hurt_from = Some(match yaw {
            y if !(45.0..315.0).contains(&y) => Dir::Forward,
            y if (135.0..225.0).contains(&y) => Dir::Back,
            y if (45.0..135.0).contains(&y) => Dir::Left,
            _ => Dir::Right,
        });
    }
    for (pawn, body, mover, view, input, weapon, loadout, offhand, melee, dead, downed, in_cover, leaving) in &pawns {
        // A killcam is posing it as it was.
        if replay.0.contains_key(&body.0) {
            continue;
        }
        let Ok((mut anim, black_ops)) = players.get_mut(body.0) else { continue };
        let state = states.entry(body.0).or_default();
        let def = weapon.map_or_else(fallback_def, |w| w.def);
        // The pin out, the grenade is what's held.
        let cooking = offhand.is_some_and(|o| matches!(o.phase, crate::grenades::Phase::Pullback | crate::grenades::Phase::Hold));
        let grip = if cooking { Held::Grenade } else { held(def) };
        let ads = input.is_some_and(|i| i.ads);
        let walking = ads || mover.lean.abs() > 0.1;
        // Headquarters walks when slow, at ease (`hq_anim`'s patrol walk).
        let (m, offset) = motion(mover, view.yaw, walking, state.motion);
        state.motion = m;
        // Debug, for comparing: `COD4RW_ANIMBLEND` mixes the forward (or
        // back) loop with the side one by the angle of travel, the legs
        // facing the view, instead of CoD4's switch at 60 degrees.
        let angled = angle_blend(mover, view.yaw, m);
        let offset = if angled.is_some() { 0.0 } else { offset };
        gaits.0.insert(body.0, Gait { offset, moving: m != Motion::Idle });
        let mut load = |name: &str| body_anim(&mut content, black_ops, name);

        if dead {
            anim.overlay = None;
            if !state.dead {
                state.dead = true;
                state.torso = None;
                let list = death_anims(mover.stance, m, downed);
                if let Some(a) = load(list[rng.random_range(0..list.len())]) {
                    anim.play(a, 0.1);
                    anim.speed = 1.0;
                }
            }
            continue;
        }
        state.dead = false;

        // Torso events, newest first: a throw, a reload, a shot, a hit;
        // putting the gun away holds its pose.
        let throwing = offhand.is_some_and(|o| o.phase == crate::grenades::Phase::Throw);
        if throwing && !state.throwing {
            start_torso(state, load(throw_anim(mover.stance, m)), None, now, 0.1, false);
        }
        state.throwing = throwing;
        let reloading = weapon.is_some_and(|w| w.reload_until.is_some());
        if reloading && !state.reloading {
            start_torso(state, load(reload_anim(def, mover.stance, m, downed)), None, now, 0.15, false);
        }
        state.reloading = reloading;
        let shots = weapon.map_or(0, |w| w.shots_fired_total);
        if shots > state.shots && !reloading {
            if let Some((name, duration)) = fire_anim(def, mover.stance, m, ads, downed) {
                start_torso(state, load(name), duration, now, 0.03, false);
            }
        }
        state.shots = shots;
        let hurt = now < state.hurt_until;
        if let (true, Some(from), Stance::Stand, Motion::Idle, true) = (hurt, state.hurt_from.take(), mover.stance, m, state.torso.is_none()) {
            let name = if grip == Held::Pistol { format!("pt_flinch_pistol_{}", from.name()) } else { format!("pt_flinch_{}", from.name()) };
            start_torso(state, load(&name), None, now, 0.05, false);
        }
        let putting_away = loadout.and_then(|l| l.switching).is_some_and(|s| !s.raising);
        if putting_away && state.torso.as_ref().is_none_or(|t| !t.hold) {
            let pose = match mover.stance {
                Stance::Prone => "pt_prone_pullout_pose",
                Stance::Crouch => "pt_crouch_pullout_pose",
                Stance::Stand => "pt_stand_pullout_pose",
            };
            start_torso(state, load(pose), Some(f32::INFINITY), now, 0.15, true);
        } else if !putting_away && state.torso.as_ref().is_some_and(|t| t.hold && t.until > now) {
            if let Some(t) = &mut state.torso {
                t.until = now;
            }
        }
        // A knife swing over all of it (`knife_melee`, `knife_melee_charge`).
        let knife = melee.and_then(|mel| {
            let name = match (mover.stance, mel.charge) {
                (Stance::Prone, _) => "pt_melee_prone_pistol",
                (_, true) => "pt_melee_pistol_2",
                _ => "pt_melee_pistol_1",
            };
            let progress = ((now - mel.started) / (mel.until - mel.started).max(0.05)).clamp(0.0, 1.0);
            load(name).map(|a| (a, progress))
        });
        match (knife, &state.torso) {
            (Some(k), _) => {
                anim.overlay = Some(k);
                anim.overlay_weight = 1.0;
            }
            (None, Some(t)) if now < t.until + TORSO_FADE_OUT => {
                let progress = if t.hold { 0.0 } else { ((now - t.started) / t.anim.duration().max(1e-3)).min(1.0) };
                let fade_in = ((now - t.started) / t.fade_in.max(1e-3)).min(1.0);
                let fade_out = (1.0 - (now - t.until) / TORSO_FADE_OUT).clamp(0.0, 1.0);
                anim.overlay = Some((t.anim.clone(), progress));
                anim.overlay_weight = fade_in.min(fade_out);
            }
            _ => {
                state.torso = None;
                anim.overlay = None;
            }
        }

        // The legs (or whole body).
        if let (Some(mt), Some(anims)) = (mover.mantle.as_ref(), mantles.as_deref()) {
            if let Some((name, t)) = mt.body_anim(anims).and_then(|(n, t)| load(n).map(|a| (a, t))) {
                anim.ensure(name, 0.1);
                anim.time = t;
                anim.speed = 1.0;
            }
            state.airborne_since = None;
            state.in_air_pose = false;
            continue;
        }
        // Up from prone: the transition first (`prone_to_crouch`).
        let was = std::mem::replace(&mut state.stance, mover.stance);
        if was == Stance::Prone && mover.stance != Stance::Prone {
            let name = if m == Motion::Idle { "pb_prone2crouch" } else { "pb_prone2crouchrun" };
            if let Some(a) = load(name) {
                state.stance_until = now + a.duration() + TORSO_EXTRA;
                anim.play(a, 0.2);
                anim.speed = 1.0;
            }
        }
        if now < state.stance_until && mover.stance != Stance::Prone {
            continue;
        }
        // Jumps and falls: the take-off animation (it ends in the in-air
        // pose and holds there), then a landing for drops over 12 units.
        let running = matches!(m, Motion::Run(_) | Motion::Sprint);
        let y = places.get(pawn).map_or(0.0, |t| t.translation().y) / crate::units::u(1.0);
        if !mover.on_ground {
            let since = *state.airborne_since.get_or_insert(now);
            if since == now {
                state.peak = y;
            }
            state.peak = state.peak.max(y);
            let leaping = mover.velocity.y > crate::units::u(60.0);
            if (leaping || now - since > FALL_POSE_AFTER) && mover.stance == Stance::Stand {
                // `jumpbk` (backing up) takes the standing jump.
                let back = matches!(m, Motion::Run(Dir::Back) | Motion::Walk(Dir::Back));
                let name = if running && !back { "pb_runjump_takeoff" } else { "pb_standjump_takeoff" };
                if let Some(a) = load(name) {
                    anim.ensure(a, 0.1);
                    anim.speed = 1.0;
                }
                state.in_air_pose = true;
            }
            continue;
        }
        state.airborne_since = None;
        if std::mem::take(&mut state.in_air_pose) {
            if state.peak - y > LAND_HEIGHT {
                let name = match () {
                    _ if running => "pb_runjump_land",
                    _ if grip == Held::Pistol => "pb_standjump_land_pistol",
                    _ => "pb_standjump_land",
                };
                if let Some(a) = load(name) {
                    state.landing_until = now + a.duration().min(LAND_FOR);
                    anim.play(a, 0.08);
                    anim.speed = 1.0;
                }
            }
        }
        if now < state.landing_until && mover.stance == Stance::Stand && !mover.sprinting {
            continue;
        }
        let cover_pose = in_cover
            .and_then(|c| crate::cover::anim_name(c, mover.lean, ads, input.is_some_and(|i| i.fire), m != Motion::Idle, now - c.since))
            .or_else(|| leaving.and_then(|l| crate::cover::exit_anim_name(l, m != Motion::Idle, mover.stance == Stance::Stand, now - l.since)));
        // In Last Stand, lying with the pistol.
        let name = match () {
            _ if downed && m == Motion::Idle => "pb_laststand_idle".to_owned(),
            _ if now < state.hurt_until && mover.stance != Stance::Prone => {
                stumble_anim(m, grip).unwrap_or_else(|| base_anim(mover.stance, m, grip, ads))
            }
            // In cover: the campaign's cover poses ([`crate::cover`]).
            _ if cover_pose.is_some() => cover_pose.unwrap_or_default().to_owned(),
            // Headquarters: at ease, the gun lowered.
            _ if crate::hq::active() => hq_anim(mover.stance, m).map_or_else(|| base_anim(mover.stance, m, grip, ads), str::to_owned),
            _ => base_anim(mover.stance, m, grip, ads),
        };
        // The angle blend's pair, instead of one direction's loop.
        let (name, side) = match angled {
            Some((fwd, side, w)) if !(downed || now < state.hurt_until || crate::hq::active()) => {
                let main = base_anim(mover.stance, m.with_dir(fwd), grip, ads);
                let other = load(&base_anim(mover.stance, m.with_dir(side), grip, ads)).map(|a| (a, w));
                (main, other)
            }
            _ => (name, None),
        };
        anim.blend = side;
        if let Some(a) = load(&name) {
            // Stance changes blend over 400 ms; moving to standing 250.
            // `BG_SetNewAnimation`'s blends: into a move 120 ms; out of one
            // to standing still 250, still to still 170; stance changes 400.
            let was_moving = state.moving;
            state.moving = m != Motion::Idle;
            let fade = match () {
                _ if was != mover.stance => 0.4,
                _ if m != Motion::Idle => 0.12,
                _ if was_moving => 0.25,
                _ => 0.17,
            };
            let rate = if m == Motion::Idle { 1.0 } else { move_rate(&a, mover.horizontal_speed()) };
            // Between moves the feet carry on where they were in the step;
            // setting off from still, each body at its own point of the
            // cycle (CoD4's by time and client number), not all in step.
            let changed = anim.anim.as_ref().is_none_or(|c| !Arc::ptr_eq(c, &a));
            let phase = match anim.anim.as_ref() {
                Some(c) if was_moving && c.looping && a.looping && m != Motion::Idle => Some(anim.time / c.duration().max(1e-3)),
                _ if a.looping && m != Motion::Idle => {
                    let cycle = a.duration() + 0.2;
                    Some((now % cycle) / cycle + (pawn.to_bits() % 64) as f32 * 0.36)
                }
                _ => None,
            };
            anim.ensure(a.clone(), fade);
            // Blind fire goes on while the trigger is held ([`crate::cover`]).
            if cover_pose.is_some_and(|n| n.contains("blindfire")) && anim.finished() {
                anim.play(a.clone(), 0.1);
            }
            if let (true, Some(phase)) = (changed, phase) {
                anim.time = phase.fract() * a.duration();
            }
            anim.speed = rate;
        }
    }
    let live: std::collections::HashSet<Entity> = pawns.iter().map(|p| p.1 .0).collect();
    states.retain(|e, _| live.contains(e));
    gaits.0.retain(|e, _| live.contains(e));
}

/// A pawn without a weapon state holds a rifle.
fn fallback_def() -> &'static crate::weapons::WeaponDef {
    static DEF: std::sync::OnceLock<crate::weapons::WeaponDef> = std::sync::OnceLock::new();
    DEF.get_or_init(crate::weapons::WeaponDef::fallback)
}

/// Start a torso event (`setTimer`): it lasts its animation, at least half
/// a second, or `duration`, and 50 ms more.
fn start_torso(state: &mut BodyState, anim: Option<Arc<XAnim>>, duration: Option<f32>, now: f32, fade_in: f32, hold: bool) {
    let Some(anim) = anim else { return };
    let length = duration.unwrap_or(anim.duration().max(TORSO_MIN)) + TORSO_EXTRA;
    state.torso = Some(TorsoEvent { anim, started: now, until: now + length, fade_in, hold });
}
