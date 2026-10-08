//! First-person weapon: CoD4's viewhands with the weapon's viewmodel
//! attached at `tag_weapon`, animated with the weapon's own xanims.

use crate::combat::Dead;
use crate::content::Content;
use crate::gunmodel::{CamoCache, CamoMaterial, GunAssets, GunTarget};
use crate::loadout::Loadout;
use crate::models::{spawn_model, AnimPlayer, Skeleton, SpawnModel};
use crate::movement::Mover;

use crate::grenades::{Offhand, Phase};
use crate::weapons::{ReloadPhase, WeaponDef, WeaponInput, WeaponState};
use bevy::camera::visibility::RenderLayers;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use iw3::xanim::XAnim;
use std::sync::Arc;

pub struct ViewModelPlugin;

impl Plugin for ViewModelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (sync_viewmodel, drive_viewmodel_anims).chain().after(crate::weapons::WeaponSet).run_if(crate::state::in_game),
        )
            .add_systems(
                PostUpdate,
                weapon_angles
                    .after(crate::models::animate_skeletons)
                    .before(TransformSystems::Propagate)
                    .run_if(crate::state::in_game),
            );
    }
}

/// Indices into `WeaponDef::szXAnims`.
pub mod anim_slot {
    pub const IDLE: usize = 1;
    pub const FIRE: usize = 3;
    /// A grenade's pin pull.
    pub const HOLD_FIRE: usize = 4;
    pub const RECHAMBER: usize = 6;
    pub const MELEE: usize = 7;
    pub const MELEE_CHARGE: usize = 8;
    pub const LASTSHOT: usize = 5;
    pub const RELOAD: usize = 9;
    pub const RELOAD_EMPTY: usize = 10;
    pub const RELOAD_START: usize = 11;
    pub const RELOAD_END: usize = 12;
    pub const RAISE: usize = 13;
    pub const FIRST_RAISE: usize = 14;
    pub const DROP: usize = 15;
    /// Switching between a rifle and its grenade launcher.
    pub const ALT_RAISE: usize = 16;
    pub const ALT_DROP: usize = 17;
    /// Raising and putting the gun away around a grenade throw.
    pub const QUICK_RAISE: usize = 18;
    pub const QUICK_DROP: usize = 19;
    pub const SPRINT_IN: usize = 22;
    pub const SPRINT_LOOP: usize = 23;
    pub const SPRINT_OUT: usize = 24;
    pub const ADS_FIRE: usize = 28;
    pub const ADS_UP: usize = 31;
    pub const ADS_DOWN: usize = 32;
    /// How long switching between the ADS up and down anims cross-fades
    /// (CoD4's `PlayADSAnim` goal-weight time).
    pub const ADS_SWITCH_FADE: f32 = 0.1;
}

/// The weapon's animations, resolved once.
#[derive(Component, Default)]
pub struct WeaponAnims {
    pub slots: Vec<Option<Arc<XAnim>>>,
}

impl WeaponAnims {
    fn get(&self, slot: usize) -> Option<Arc<XAnim>> {
        self.slots.get(slot).cloned().flatten()
    }
}

#[derive(Component)]
pub struct ViewModelRoot;

/// Whose viewmodel it is (a local player's place, 0 for Player 1).
#[derive(Component, Clone, Copy, Debug)]
pub struct ViewModelSlot(pub usize);

/// The weapon def a viewmodel was built for.
#[derive(Component)]
struct BuiltFor(*const WeaponDef);

// Only compared, never dereferenced.
unsafe impl Send for BuiltFor {}
unsafe impl Sync for BuiltFor {}

/// The viewmodel's pose under the camera when nothing moves it. CoD models
/// face +X; the camera looks down -Z.
pub fn rest_transform() -> Transform {
    Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
}

/// Per-frame state behind the viewmodel's idle sway (`weaponState_t`).
#[derive(Component)]
pub(crate) struct WeaponMotion {
    /// Idle sway clock in ms, advanced at the weapon's idle speed.
    idle_time: f32,
    /// Stance idle factor, eased toward its target.
    idle_factor: f32,
    /// How far into its sprint pose a World at War gun is (0..1), and the
    /// pose (see [`crate::waw::sprint_pose`]).
    sprint: f32,
    sprint_pose: Option<crate::waw::SprintPose>,
    /// Seconds into an inspect, while the gun is being looked over.
    inspect: Option<f32>,
    /// The inspect's last pose and how much of it is still applied: an
    /// inspect cut short eases out of it rather than snapping back.
    inspect_pose: InspectPose,
    inspect_fade: f32,
}

impl WeaponMotion {
    fn new(sprint_pose: Option<crate::waw::SprintPose>) -> Self {
        WeaponMotion {
            idle_time: 0.0,
            idle_factor: 1.0,
            sprint: 0.0,
            sprint_pose,
            inspect: None,
            inspect_pose: InspectPose::default(),
            inspect_fade: 0.0,
        }
    }
}

/// None of CoD4's (or World at War's, or Black Ops') guns have an inspect
/// animation, so it's posed: the whole viewmodel turns about the gun to show
/// its left side, rolls over to show its right, and comes back.
const INSPECT_TIME: f32 = 3.4;
/// How long an inspect cut short takes to ease out.
const INSPECT_FADE: f32 = 0.18;

/// A pose an inspect turns the gun through: angles in CoD's degrees (pitch
/// down, yaw left, roll right side down), how far it moves (forward, left,
/// up) in inches, and how far the support hand has let go and dropped (0..1).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct InspectPose {
    angles: Vec3,
    shift: Vec3,
    drop: f32,
}

/// The inspect's keyframes: (seconds, pose), eased between.
const INSPECT_KEYS: [(f32, InspectPose); 6] = {
    const fn pose(pitch: f32, yaw: f32, roll: f32, forward: f32, left: f32, up: f32, drop: f32) -> InspectPose {
        InspectPose { angles: Vec3::new(pitch, yaw, roll), shift: Vec3::new(forward, left, up), drop }
    }
    [
        (0.0, pose(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)),
        // The left side: muzzle up and to the left.
        (0.55, pose(-14.0, 32.0, 8.0, 4.0, 0.0, 0.0, 0.0)),
        (1.35, pose(-11.0, 35.0, 11.0, 4.0, 0.5, 0.3, 0.0)),
        // The right side: the support hand lets go and the wrist rolls the
        // gun over, muzzle down a little.
        (2.0, pose(8.0, -12.0, -50.0, 4.0, 3.0, 0.0, 1.0)),
        (2.75, pose(7.0, -14.0, -55.0, 4.0, 3.0, 0.3, 1.0)),
        (INSPECT_TIME, pose(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)),
    ]
};

/// How much of the gun's drift during an inspect is taken back (0..1),
/// keeping it in view (see `weapon_angles`).
const INSPECT_KEEP_GUN: f32 = 0.6;

/// How the support arm swings about its shoulder once it has let go:
/// degrees down and out to the left.
const INSPECT_DROP: (f32, f32) = (45.0, 15.0);

/// The inspect's pose `t` seconds in.
fn inspect_pose(t: f32) -> InspectPose {
    let i = INSPECT_KEYS.iter().rposition(|(k, _)| *k <= t).unwrap_or(0).min(INSPECT_KEYS.len() - 2);
    let ((t0, a), (t1, b)) = (INSPECT_KEYS[i], INSPECT_KEYS[i + 1]);
    let x = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
    let k = x * x * (3.0 - 2.0 * x);
    InspectPose { angles: a.angles.lerp(b.angles, k), shift: a.shift.lerp(b.shift, k), drop: a.drop + (b.drop - a.drop) * k }
}

/// Which viewhands a map's teams use (from the models present in the zone).
pub fn team_viewhands(content: &Content, allies: bool) -> &'static str {
    let candidates: &[&str] = if allies {
        &["viewhands_black_kit", "viewhands_usmc", "viewhands_sas_woodland", "viewhands_marine_sniper"]
    } else {
        &["viewhands_op_force", "viewhands_desert_opfor", "viewhands_opforce", "viewhands_spetsnaz"]
    };
    candidates.iter().copied().find(|n| content.find(n).is_some()).unwrap_or("viewmodel_base_viewhands")
}

/// Build each local player's viewmodel, hands and gun, for the weapon in
/// their hands, again whenever that changes (a class, a weapon switch),
/// under their camera on their own layer.
#[allow(clippy::too_many_arguments)]
fn sync_viewmodel(
    mut commands: Commands,
    mut content: ResMut<Content>,
    cameras: Query<(Entity, &crate::splitscreen::SlotCamera)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut camo_materials: ResMut<Assets<CamoMaterial>>,
    mut camos: Local<CamoCache>,
    players: Query<(&crate::splitscreen::LocalSlot, &WeaponState, Option<&Loadout>, Option<&Offhand>)>,
    built: Query<(Entity, &BuiltFor, &ViewModelSlot), With<ViewModelRoot>>,
    wardrobe: Res<crate::wardrobe::Wardrobe>,
    mut bo1_content: ResMut<crate::bo1::MatchContent>,
    mut waw_content: ResMut<crate::waw::MatchContent>,
) {
  for (slot, w, loadout, offhand) in &players {
    let slot = slot.0;
    let Some(camera) = cameras.iter().find(|c| c.1.0 == slot).map(|c| c.0) else { continue };
    // A grenade in hand: its own viewmodel ([`crate::grenades`]).
    let grenade = offhand.filter(|o| o.grenade_in_hand());
    let shown: &'static WeaponDef = grenade.map_or(w.def, |o| o.def);
    let def: *const WeaponDef = shown;
    if built.iter().any(|(_, b, s)| s.0 == slot && b.0 == def) {
        continue;
    }
    for (e, _, _) in built.iter().filter(|b| b.2.0 == slot) {
        commands.entity(e).despawn();
    }
    let (spec, camo) = match (grenade, loadout) {
        (None, Some(l)) => (l.gun().spec.clone(), l.gun().camo),
        _ => (format!("{}:", shown.name.trim_end_matches("_mp")), 0),
    };
    // A Black Ops gun comes from Black Ops' content, a World at War gun from
    // World at War's (none until it has loaded).
    let bo1 = crate::bo1::is_bo1(crate::gunmodel::parse(&spec).0);
    let waw = crate::waw::is_waw(crate::gunmodel::parse(&spec).0);
    if (bo1 && bo1_content.get().is_none()) || (waw && waw_content.get().is_none()) {
        continue;
    }
    let layer = RenderLayers::layer(crate::splitscreen::viewmodel_layer(slot));
    let owner = commands
        .spawn((
            ViewModelRoot,
            ViewModelSlot(slot),
            BuiltFor(def),
            Name::new("viewmodel"),
            rest_transform(),
            Visibility::default(),
            layer.clone(),
            ChildOf(camera),
        ))
        .id();
    // The worn character's own arms, else the team's. Black Ops' and World
    // at War's guns' animations drive either (Black Ops' own
    // `viewhands_usmc` is a placeholder; World at War's arms belong to its
    // characters).
    // Player 1's character is the one worn; the other players' arms are
    // the team's.
    let team_hands = team_viewhands(&content, true);
    let hands = wardrobe.arms().filter(|_| slot == 0).or_else(|| content.model(team_hands, &mut meshes, &mut materials, &mut images, &mut bindposes));
    let content: &mut Content = match (bo1_content.get().filter(|_| bo1), waw_content.get().filter(|_| waw)) {
        (Some(c), _) | (_, Some(c)) => c,
        _ => &mut content,
    };
    let hands_name = hands.as_ref().map_or("no hands", |h| h.name.as_str());
    let mut skeleton = Skeleton::default();
    if let Some(h) = &hands {
        spawn_model(&mut commands, &mut skeleton, SpawnModel { model: h, owner, attach_to: None, layers: Some(layer.clone()), shadows: false });
    }
    let mut assets = GunAssets {
        meshes: &mut meshes,
        materials: &mut materials,
        images: &mut images,
        bindposes: &mut bindposes,
        camo_materials: &mut camo_materials,
    };
    let target = GunTarget { owner, attach_to: skeleton.joint("tag_weapon"), layers: Some(layer) };
    let gun = crate::gunmodel::spawn_gun(&mut commands, content, &mut camos, &mut assets, &mut skeleton, &spec, camo, target);
    if gun.is_none() {
        warn!("viewmodel: no gun model for {spec}");
    }
    let slots = shown.xanims.iter().map(|n| n.as_deref().filter(|s| !s.is_empty()).and_then(|n| content.anim(n))).collect();
    let anims = WeaponAnims { slots };
    // A grenade: its pin coming out. The gun after it: quickly raised.
    // Switching to it: raised over the raise time; otherwise just spawned.
    let mut player = AnimPlayer::default();
    let raising = loadout.and_then(|l| l.switching).filter(|s| s.raising);
    let after_grenade = offhand.filter(|o| o.phase == crate::grenades::Phase::Raise);
    let first = match (grenade, after_grenade, raising) {
        (Some(o), ..) => anims.get(anim_slot::HOLD_FIRE).map(|a| (a, o.until - o.started)),
        (_, Some(o), _) => anims.get(anim_slot::QUICK_RAISE).map(|a| (a, o.until - o.started)),
        (_, _, Some(s)) => anims.get(if s.alt { anim_slot::ALT_RAISE } else { anim_slot::RAISE }).map(|a| (a, s.until - s.started)),
        _ => anims.get(anim_slot::FIRST_RAISE).map(|a| (a, w.def.first_raise_time)),
    };
    if let Some((a, seconds)) = first.or(anims.get(anim_slot::IDLE).map(|a| (a, 0.0))) {
        if seconds > 0.0 {
            player.speed = (a.duration() / seconds).clamp(0.25, 4.0);
        }
        player.play(a, 0.0);
    }
    info!("viewmodel: {hands_name} + {spec} camo {camo} ({} joints){}", skeleton.joints.len(), if slot > 0 { format!(", player {}", slot + 1) } else { String::new() });
    commands.entity(owner).insert((skeleton, player, anims, WeaponMotion::new(crate::waw::sprint_pose(&shown.name)), VmAnim::default()));
  }
}

/// Where a viewmodel's animations are, between frames.
#[derive(Component, Default)]
struct VmAnim {
    state: VmState,
    last_shots: u32,
    rechamber_due: bool,
    last_ads_frac: f32,
    last_built: Option<Entity>,
    last_reload: (ReloadPhase, u32),
    last_offhand: Option<(Phase, u32)>,
    last_melee: Option<u32>,
}

#[derive(Default, PartialEq, Clone, Copy, Debug)]
enum VmState {
    #[default]
    Idle,
    /// A one-shot anim (fire, sprint out) is playing; idle resumes after it.
    OneShot,
    Reloading,
    Sprinting,
    /// Putting the weapon away to switch.
    Dropping,
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn drive_viewmodel_anims(
    players: Query<(
        &crate::splitscreen::LocalSlot,
        &WeaponState,
        &WeaponInput,
        &Mover,
        Option<&Loadout>,
        Has<Dead>,
        Option<&Offhand>,
        Option<&crate::melee::Melee>,
    )>,
    mut vm: Query<(Entity, &ViewModelSlot, &mut AnimPlayer, &WeaponAnims, &mut Visibility, &mut WeaponMotion, &mut VmAnim), With<ViewModelRoot>>,
    (third_person, killcam, time): (Res<crate::wardrobe::ThirdPerson>, Res<crate::killcam::Killcam>, Res<Time>),
) {
    for (built, slot, mut anim, anims, mut vis, mut motion, mut st) in &mut vm {
        let Some((_, w, input, mover, loadout, dead, offhand, melee)) = players.iter().find(|p| p.0.0 == slot.0) else { continue };
        // Seen from outside: third person, or Player 1's killcam.
        let outside = third_person.on(slot.0) || (slot.0 == 0 && killcam.showing());
        let VmAnim { state, last_shots, rechamber_due, last_ads_frac, last_built, last_reload, last_offhand, last_melee } = &mut *st;
        drive_one(
            (w, input, mover, loadout, dead, offhand, melee),
            (built, &mut anim, anims, &mut vis, &mut motion),
            (state, last_shots, rechamber_due, last_ads_frac, last_built, last_reload, last_offhand, last_melee),
            outside,
            time.delta_secs(),
        );
    }
}

/// One viewmodel's animations this frame.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn drive_one(
    (w, input, mover, loadout, dead, offhand, melee): (
        &WeaponState,
        &WeaponInput,
        &Mover,
        Option<&Loadout>,
        bool,
        Option<&Offhand>,
        Option<&crate::melee::Melee>,
    ),
    (built, mut anim, anims, vis, motion): (Entity, &mut AnimPlayer, &WeaponAnims, &mut Visibility, &mut WeaponMotion),
    (state, last_shots, rechamber_due, last_ads_frac, last_built, last_reload, last_offhand, last_melee): (
        &mut VmState,
        &mut u32,
        &mut bool,
        &mut f32,
        &mut Option<Entity>,
        &mut (ReloadPhase, u32),
        &mut Option<(Phase, u32)>,
        &mut Option<u32>,
    ),
    outside: bool,
    dt: f32,
) {
    use anim_slot::*;
    // An inspect carries on only while nothing else happens: every path
    // that returns early (a throw, a knife, a switch, a reload, a sprint, a
    // shot) ends it.
    let mut inspecting = motion.inspect.take();
    // A new viewmodel starts with its raise anim playing.
    if *last_built != Some(built) {
        *last_built = Some(built);
        *last_shots = w.shots_fired_total;
        *state = VmState::OneShot;
    }
    let debug_hidden = std::env::var_os("COD4RW_DUMMY").is_some() || std::env::var_os("COD4RW_DUMMY_AT").is_some();
    *vis = if dead || debug_hidden || outside { Visibility::Hidden } else { Visibility::Inherited };

    // Debug: hold one named animation.
    if let Ok(forced) = std::env::var("COD4RW_VMANIM") {
        if !anim.anim.as_ref().is_some_and(|a| a.name.eq_ignore_ascii_case(&forced)) {
            if let Some(a) = anims.slots.iter().flatten().find(|a| a.name.eq_ignore_ascii_case(&forced)).cloned() {
                anim.play(a, 0.0);
            }
        }
        anim.speed = 0.3;
        return;
    }
    let fired = w.shots_fired_total != *last_shots;
    *last_shots = w.shots_fired_total;
    let ads = w.ads > 0.5 && input.ads;

    // ADS only moves `tag_ads`, layered over whatever else is playing, as
    // CoD4 does (`PlayADSAnim`): ads_up while aiming, ads_down otherwise
    // (and while reloading), posed by the ADS fraction, and always applied:
    // at the hip it's ads_down's last frame, so finishing an aim-out lands
    // where the animation ends (dropping the layer there snapped the gun to
    // the rest pose, 3 units on the Skorpion ACOG). Which one plays follows
    // the aim, not whether the fraction grew this frame: frames on which it
    // didn't move flipped it to ads_down and back, jittering every aim.
    // (World at War's other animations leave `tag_torso` alone: its ADS
    // ones place it from the hip on, which this covers too.)
    let aiming = input.ads && *state != VmState::Reloading;
    let wanted = if aiming {
        anims.get(ADS_UP).map(|a| (a, w.ads))
    } else {
        anims.get(ADS_DOWN).or_else(|| anims.get(ADS_UP)).map(|a| if anims.get(ADS_DOWN).is_some() { (a, 1.0 - w.ads) } else { (a, w.ads) })
    };
    // Switching between up and down cross-fades over CoD4's 0.1 s (the two
    // seldom meet exactly: 1.3 units on the M14 ACOG, 3 on the Skorpion
    // ACOG's hip end). The one left keeps its pose as it fades.
    let switched = match (&anim.overlay, &wanted) {
        (Some((old, _)), Some((new, _))) => !Arc::ptr_eq(old, new),
        _ => false,
    };
    if switched {
        anim.overlay_from = anim.overlay.take().map(|(a, p)| (a, p, 1.0));
    }
    if let Some(f) = anim.overlay_from.as_mut() {
        f.2 -= dt / ADS_SWITCH_FADE;
    }
    if anim.overlay_from.as_ref().is_some_and(|f| f.2 <= 0.0) {
        anim.overlay_from = None;
    }
    anim.overlay = wanted;
    *last_ads_frac = w.ads;

    // Play an anim, optionally stretched to last `seconds` like CoD4's
    // timed weapon states (reload, sprint in/loop/out).
    let play = |anim: &mut AnimPlayer, slot: usize, fade: f32, seconds: Option<f32>| {
        if let Some(a) = anims.get(slot) {
            anim.speed = seconds.map_or(1.0, |s| (a.duration() / s.max(0.05)).clamp(0.25, 4.0));
            anim.play(a, fade);
        }
    };

    // A grenade throw: the gun quickly down, the pin out, the throw, the gun
    // quickly up, each to its time.
    if let Some(o) = offhand {
        let part = (o.phase, o.started.to_bits());
        if *last_offhand != Some(part) {
            *last_offhand = Some(part);
            let length = Some(o.until - o.started);
            match o.phase {
                Phase::Drop => play(&mut anim, QUICK_DROP, 0.05, length),
                Phase::Pullback => play(&mut anim, HOLD_FIRE, 0.0, length),
                Phase::Hold => {}
                Phase::Throw => play(&mut anim, FIRE, 0.0, length),
                Phase::Raise => play(&mut anim, QUICK_RAISE, 0.0, length),
            }
        }
        *state = VmState::OneShot;
        return;
    }
    *last_offhand = None;

    // A knife swing: its animation (the lunge's when lunging), to its time.
    if let Some(m) = melee {
        if *last_melee != Some(m.started.to_bits()) {
            *last_melee = Some(m.started.to_bits());
            play(&mut anim, if m.charge { MELEE_CHARGE } else { MELEE }, 0.05, Some(m.until - m.started));
        }
        *state = VmState::OneShot;
        return;
    }
    *last_melee = None;

    if let Some(s) = loadout.and_then(|l| l.switching) {
        if !s.raising && *state != VmState::Dropping {
            play(&mut anim, if s.alt { ALT_DROP } else { DROP }, 0.1, Some(s.until - s.started));
            *state = VmState::Dropping;
        }
        return;
    }

    if w.reloading() {
        // Each part of a segmented reload (start, each round's loop, end)
        // plays its own animation, to its own time.
        let part = (w.reload_phase, w.reload_until.unwrap_or_default().to_bits());
        if *state != VmState::Reloading || *last_reload != part {
            let d = w.def;
            match w.reload_phase {
                ReloadPhase::Whole => {
                    let empty = w.clip == 0;
                    let time = if empty { d.reload_empty_time } else { d.reload_time };
                    play(&mut anim, if empty { RELOAD_EMPTY } else { RELOAD }, 0.1, Some(time));
                }
                ReloadPhase::Start => play(&mut anim, RELOAD_START, 0.1, Some(d.reload_start_time)),
                ReloadPhase::Loop => play(&mut anim, RELOAD, 0.05, Some(d.reload_time)),
                ReloadPhase::End => play(&mut anim, RELOAD_END, 0.05, Some(d.reload_end_time)),
            }
            debug!("viewmodel: reload {:?}, {} in the clip", w.reload_phase, w.clip);
            *state = VmState::Reloading;
            *last_reload = part;
        }
        return;
    }

    if mover.sprinting {
        if *state != VmState::Sprinting {
            play(&mut anim, SPRINT_IN, 0.1, Some(w.def.sprint_in_time));
            *state = VmState::Sprinting;
        } else if anim.finished() {
            play(&mut anim, SPRINT_LOOP, 0.05, Some(w.def.sprint_loop_time));
        }
        return;
    }
    if *state == VmState::Sprinting {
        play(&mut anim, SPRINT_OUT, 0.05, Some(w.def.sprint_out_time));
        *state = VmState::OneShot;
        return;
    }

    if fired {
        let lastshot = w.clip == 0;
        play(&mut anim, if ads { ADS_FIRE } else if lastshot { LASTSHOT } else { FIRE }, 0.0, None);
        *state = VmState::OneShot;
        // Bolt actions work the bolt after the shot (not after the last).
        *rechamber_due = w.def.rechamber_time > 0.0 && !lastshot;
        return;
    }

    // Settle back into idle once one-shot anims end (after the bolt).
    if anim.finished() && std::mem::take(&mut *rechamber_due) {
        play(&mut anim, RECHAMBER, 0.05, Some(w.def.rechamber_time));
        *state = VmState::OneShot;
        return;
    }
    if anim.finished() {
        if *state != VmState::Idle || anim.anim.is_none() {
            play(&mut anim, IDLE, 0.15, None);
        }
        *state = VmState::Idle;
    }

    // Inspecting starts from idle; aiming or the trigger ends it.
    if dead || input.fire || input.ads || w.ads > 0.0 {
        inspecting = None;
    } else if inspecting.is_none() && input.inspect && *state == VmState::Idle {
        inspecting = Some(0.0);
    }
    motion.inspect = inspecting;
}

/// Gun angle offsets from `BG_CalculateWeaponAngles`: slow idle sway and the
/// movement bob, applied as a rotation about the eye on top of the view.
pub(crate) fn weapon_angles(
    time: Res<Time>,
    players: Query<(&crate::splitscreen::LocalSlot, &Mover, &WeaponState, Option<&crate::first_person::feel::Feel>)>,
    mut vm: Query<(&ViewModelSlot, &mut Transform, &mut WeaponMotion, &Skeleton, &GlobalTransform), With<ViewModelRoot>>,
    joints: Query<&GlobalTransform, Without<ViewModelRoot>>,
    mut poses: Query<(&mut Transform, &ChildOf), Without<ViewModelRoot>>,
) {
  for (vslot, mut tf, mut motion, skeleton, root) in &mut vm {
    let Some((_, mover, w, feel)) = players.iter().find(|p| p.0.0 == vslot.0) else { continue };
    let (d, f, dt) = (&w.def, w.ads, time.delta_secs());

    // Idle sway: three slow sines, stronger in ADS, eased per stance.
    let amount = d.hip_idle_amount + (d.ads_idle_amount - d.hip_idle_amount) * f;
    let speed = d.hip_idle_speed + (d.ads_idle_speed - d.hip_idle_speed) * f;
    let target = match mover.stance {
        crate::movement::Stance::Prone => d.idle_prone_factor,
        crate::movement::Stance::Crouch => d.idle_crouch_factor,
        crate::movement::Stance::Stand => 1.0,
    };
    let step = 0.5 * dt;
    motion.idle_factor += (target - motion.idle_factor).clamp(-step, step);
    motion.idle_time += speed * dt * 1000.0;
    let (t, sc) = (motion.idle_time, amount * motion.idle_factor * 0.01);
    let mut pitch = sc * (t * 0.001).sin();
    let mut yaw = sc * (t * 0.0007).sin();
    let mut roll = sc * (t * 0.0005).sin();

    // World at War's guns have no sprint animations: over the sprint in and
    // out times they move into a sprint pose (`sprintRot`, `sprintOfs`),
    // where the bob sways them as the view's bob does (by the full speed),
    // as far as `sprintBobH/V` allow.
    let mut k = 0.0;
    if let Some(pose) = motion.sprint_pose {
        let (target, time) = if mover.sprinting { (1.0, d.sprint_in_time) } else { (0.0, d.sprint_out_time) };
        let step = dt / time.max(0.05);
        motion.sprint += (target - motion.sprint).clamp(-step, step);
        k = motion.sprint * motion.sprint * (3.0 - 2.0 * motion.sprint);
        pitch += pose.rot.x * k;
        yaw += pose.rot.y * k;
        roll += pose.rot.z * k;
    }

    // Movement bob, phase-shifted from the view bob.
    let cycle = mover.bob_angle() + std::f32::consts::FRAC_PI_4 + std::f32::consts::TAU;
    let bob_speed = 0.16 * mover.xy_speed_units();
    let scale = 1.0 - (1.0 - d.ads_bob_factor) * f;
    let (mut bob_v, mut bob_h) = (mover.vertical_bob(cycle, bob_speed, 10.0), mover.horizontal_bob(cycle, bob_speed, 10.0));
    if let Some(pose) = motion.sprint_pose.filter(|_| k > 0.0) {
        let speed = mover.xy_speed_units();
        let (v, h) = (mover.vertical_bob(cycle, speed, pose.bob.y), mover.horizontal_bob(cycle, speed, pose.bob.x));
        bob_v += (v - bob_v) * k;
        bob_h += (h - bob_h) * k;
    }
    pitch -= bob_v * scale;
    yaw -= bob_h * scale;
    // Roll only tips one way: the horizontal amplitude at 1.5x speed on a
    // slightly earlier phase, clamped to <= 0.
    let side = mover.horizontal_bob(std::f32::consts::FRAC_PI_2, bob_speed * 1.5, 10.0) * (cycle - 0.471_238_9).sin();
    roll += side.min(0.0) * scale;

    // The gun's own kick ([`WeaponState::gun_offset`]).
    pitch += w.gun_offset.x;
    yaw += w.gun_offset.y;
    // Sway as the view turns and the richer bob ([`crate::first_person::feel`]).
    let (feel_angles, feel_shift) = feel.map_or((Vec3::ZERO, Vec3::ZERO), |f| (f.gun_angles, f.gun_shift));
    pitch += feel_angles.x;
    yaw += feel_angles.y;
    roll += feel_angles.z;
    // CoD angles: pitch down, yaw left, roll right side down.
    let euler = |a: Vec3| Quat::from_euler(EulerRot::YXZ, a.y.to_radians(), -a.x.to_radians(), -a.z.to_radians());
    // CoD's (forward, left, up) in inches, along the view's axes.
    let along = |v: Vec3| Vec3::new(-crate::units::u(v.y), crate::units::u(v.z), -crate::units::u(v.x));
    let offset = euler(Vec3::new(pitch, yaw, roll));
    let rest = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);

    // An inspect turns the whole viewmodel, arms and all: it rolls about the
    // right forearm (a twist of the wrist, which keeps that arm in place),
    // then pitches and turns about a point a third of the way from the gun
    // (a little ahead of `tag_weapon`, the grip) to the right elbow. About
    // the gun alone, pitching the muzzle down would lift the forearm into
    // the view; about the elbow, the gun swings too far.
    if let Some(t) = motion.inspect.as_mut() {
        *t += dt;
        if *t < INSPECT_TIME {
            motion.inspect_pose = inspect_pose(*t);
            motion.inspect_fade = 1.0;
        } else {
            motion.inspect = None;
            motion.inspect_fade = 0.0;
        }
    } else {
        motion.inspect_fade = (motion.inspect_fade - dt / INSPECT_FADE).max(0.0);
    }
    let fade = motion.inspect_fade * motion.inspect_fade * (3.0 - 2.0 * motion.inspect_fade);
    let (mut turn, mut moved) = (Quat::IDENTITY, Vec3::ZERO);
    if fade > 0.0 {
        let to_root = root.affine().inverse();
        let joint = |name: &str| skeleton.joint(name).and_then(|j| joints.get(j).ok());
        let at = |g: &GlobalTransform| rest * to_root.transform_point3(g.translation());
        let pivot = joint("tag_weapon").map_or(along(Vec3::new(14.0, -6.0, -6.0)), |g| {
            let ahead = to_root.transform_vector3(g.rotation() * Vec3::X).normalize_or_zero() * crate::units::u(4.0);
            at(g) + rest * ahead
        });
        let (elbow, forearm) = match (joint("j_elbow_ri"), joint("j_wrist_ri")) {
            (Some(elbow), Some(wrist)) => (at(elbow), (at(wrist) - at(elbow)).normalize_or(Vec3::NEG_Z)),
            _ => (pivot, Vec3::NEG_Z),
        };
        // The roll: about the barrel (grip to muzzle), through the right
        // wrist so the hand stays on the grip. (About the forearm, as it
        // was, it only rolled where the forearm runs along the gun; on rigs
        // holding it at an angle, the P90's and the Kar98k's among them, the
        // roll became a pitch or a turn that swung the gun out of view.)
        let barrel = match (joint("tag_weapon"), joint("tag_flash")) {
            (Some(grip), Some(muzzle)) => (at(muzzle) - at(grip)).normalize_or(forearm),
            _ => forearm,
        };
        let wrist = joint("j_wrist_ri").map_or(elbow, |g| at(g));
        let pivot = pivot.lerp(elbow, 0.35);
        // Debug aid: `COD4RW_INSPECT_DEBUG` logs the pivots once per inspect.
        if std::env::var_os("COD4RW_INSPECT_DEBUG").is_some() && motion.inspect.is_some_and(|t| t < 0.06) {
            let names = ["tag_weapon", "j_elbow_ri", "j_wrist_ri", "j_shoulder_le", "tag_flash", "tag_torso"];
            let found: Vec<String> = names.iter().map(|n| format!("{n}={:?}", joint(n).map(|g| (at(g) / crate::units::INCH).round()))).collect();
            info!("inspect {}: forearm-barrel {:.0} deg; pivot {:?} elbow {:?} forearm {:.2?} barrel {:.2?} | {}", w.def.name, forearm.angle_between(barrel).to_degrees(), (pivot / crate::units::INCH).round(), (elbow / crate::units::INCH).round(), forearm, barrel, found.join(" "));
        }
        let a = motion.inspect_pose.angles * fade;
        // CoD's roll turns the right side down: clockwise from behind,
        // about the barrel pointing away.
        let twist = Quat::from_axis_angle(forearm, a.z.to_radians());
        let aim = euler(Vec3::new(a.x, a.y, 0.0));
        turn = aim * twist;
        moved = pivot + along(motion.inspect_pose.shift * fade) + aim * (elbow - twist * elbow - pivot);
        // The roll is about the right forearm (so that arm stays put), but
        // each rig's forearm lies its own way to the gun: on some (the RPD,
        // the Thompson, the P90) the same roll carried the gun low or out of
        // view. The viewmodel is moved back most of the way towards keeping
        // the gun's middle (halfway from grip to muzzle) where it was.
        if let (Some(grip), Some(muzzle)) = (joint("tag_weapon"), joint("tag_flash")) {
            let middle = (at(grip) + at(muzzle)) * 0.5;
            moved += (middle - (turn * middle + moved)) * INSPECT_KEEP_GUN;
        }
        let _ = (wrist, barrel);

        // The support arm swings down and out to the left about its
        // shoulder, out of sight, once the hand lets go: down and left on
        // screen however the gun is turned, so worked out in the view's
        // frame and carried into the frame of the shoulder's parent (as last
        // frame left it; the animation sets the shoulder afresh each frame).
        let drop = motion.inspect_pose.drop * fade;
        let (down, out) = INSPECT_DROP;
        if let Some(Ok((mut pose, parent))) = skeleton.joint("j_shoulder_le").filter(|_| drop > 0.0).map(|j| poses.get_mut(j)) {
            if let Ok(p) = joints.get(parent.parent()) {
                let rig = turn * rest;
                let view = Quat::from_rotation_y((out * drop).to_radians()) * Quat::from_rotation_x((-down * drop).to_radians());
                let frame = root.rotation().inverse() * p.rotation();
                let swing = rig.inverse() * view * rig;
                pose.rotation = frame.inverse() * swing * frame * pose.rotation;
            }
        }
    }

    tf.rotation = offset * turn * rest;
    // The sprint pose's offset, along the view's axes (forward, `axis[1]`,
    // up): IW's second axis points left, so a negative `sprintOfsR` is to
    // the right.
    let ofs = motion.sprint_pose.map_or(Vec3::ZERO, |p| p.ofs * k);
    tf.translation = along(ofs + feel_shift) + offset * moved;
  }
}
