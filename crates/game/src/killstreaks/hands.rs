//! The player's hands calling in a hardpoint, as CoD4 shows it: each
//! hardpoint is a weapon (`radar_mp`, `airstrike_mp`, `helicopter_mp`, all
//! three the C4's viewmodel, with its detonator), and `_hardpoints.gsc`'s
//! `hardpointItemWaiter` switches to it, triggers it and switches back to
//! the gun. So: the gun goes down, the detonator comes up and is pressed,
//! goes away, and the gun comes back up. The airstrike's is held up while
//! its spot is picked on the map (`selectAirstrikeLocation` waits for the
//! confirm before switching back), and only pressed once it's picked.
//!
//! Modern Warfare 2's sentry gun is carried out instead ([`super::sentry`]):
//! the gun goes down and the hands are out of sight until it's planted
//! (as Modern Warfare 2 shows it), then the gun comes back up. Its care
//! package's marker is thrown as a grenade is (the grenades' own anims).
//!
//! [`HardpointHands`] on a local player's pawn is what `crate::viewmodel`
//! shows and animates; meanwhile the gun can't fire, aim or reload.

use super::{Hardpoint, StreakNotice, airstrike};
use crate::combat::{Dead, Pawn};
use crate::splitscreen::LocalSlot;
use crate::weapons::{WeaponDef, WeaponInput, WeaponState};
use bevy::prelude::*;

pub(super) fn build(app: &mut App) {
    app.init_resource::<HardpointDefs>()
        .add_systems(OnEnter(crate::state::GameState::InGame), load.after(crate::world::load_map).in_set(crate::state::Setup::Content))
        .add_systems(Update, (start, carry_sentry, advance).chain().after(super::use_hardpoints).run_if(crate::state::in_game))
        .add_systems(Update, shots.run_if(|| std::env::var_os("COD4RW_HANDSHOT").is_some()))
        .add_systems(PostUpdate, place_props.after(crate::models::animate_skeletons).before(bevy::transform::TransformSystems::Propagate).run_if(crate::state::in_game))
        .add_systems(Update, hold_gun.after(crate::splitscreen::InputGathered).before(crate::weapons::WeaponSet).run_if(crate::state::in_game));
}

/// The hardpoints' weapon files (their viewmodel and animations).
#[derive(Resource, Default)]
struct HardpointDefs {
    uav: Option<&'static WeaponDef>,
    airstrike: Option<&'static WeaponDef>,
    helicopter: Option<&'static WeaponDef>,
}

impl HardpointDefs {
    fn of(&self, item: Hardpoint) -> Option<&'static WeaponDef> {
        match item {
            Hardpoint::Uav => self.uav,
            Hardpoint::Airstrike => self.airstrike,
            Hardpoint::Helicopter => self.helicopter,
            // The care package's marker is thrown as a grenade, and the
            // sentry is carried out in the hands' place.
            Hardpoint::CarePackage | Hardpoint::Sentry => None,
        }
    }
}

fn load(mut defs: ResMut<HardpointDefs>, content: Res<crate::content::Content>) {
    if defs.uav.is_some() {
        return;
    }
    let def = |name: &str| crate::loadout::bot_weapon(&content, &format!("{name}:")).filter(|d| d.name == format!("{name}_mp"));
    // The airstrike's laptop is held as the Search and Destroy bomb is
    // (`briefcase_bomb_mp`: two hands round an open case of the laptop's
    // size), the case hidden.
    *defs = HardpointDefs { uav: def("radar"), airstrike: def("briefcase_bomb").or_else(|| def("airstrike")), helicopter: def("helicopter") };
    if defs.uav.is_none() {
        warn!("hardpoint hands: no radar_mp weapon file; calling in shows no hands");
    }
}

/// Where a call-in's hands are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandsPhase {
    /// The gun going down (its drop).
    GunDown,
    /// The detonator coming up (the hardpoint's raise).
    Raise,
    /// Holding it up (the airstrike's, while its spot is picked).
    Hold,
    /// Pressing it.
    Press,
    /// Putting it away (the hardpoint's drop).
    PutAway,
    /// The gun coming back up (its raise).
    GunUp,
    /// No hands in sight (carrying a sentry gun out).
    Away,
}

/// A hardpoint being called in by this local player's hands.
#[derive(Component, Clone, Copy, Debug)]
pub struct HardpointHands {
    /// The hardpoint's weapon (the detonator's viewmodel and anims).
    pub def: &'static WeaponDef,
    pub phase: HandsPhase,
    pub started: f32,
    pub until: f32,
    /// Pressed once it's up (UAV, helicopter; the airstrike's once its
    /// spot is picked), else put away unpressed (the pick called off).
    press: bool,
    /// Waiting on the airstrike's spot.
    selecting: bool,
    /// When its map closed (the call-in can come a frame after).
    closed_at: f32,
    /// Carrying a sentry gun out: away until it's planted.
    carry: bool,
}

impl HardpointHands {
    /// Is the detonator (rather than the gun) in hand?
    pub fn item_in_hand(&self) -> bool {
        matches!(self.phase, HandsPhase::Raise | HandsPhase::Hold | HandsPhase::Press | HandsPhase::PutAway)
    }
}

/// How long after the airstrike's map closes its call-in may still come.
const CALL_GRACE: f32 = 0.2;

/// How long the detonator's press lasts (`iFireTime`, 800 ms, is the time
/// to the next press; the press itself is over in about this).
const PRESS: f32 = 0.6;

/// Start the hands on a call-in (the airstrike's on opening its map).
fn start(
    mut commands: Commands,
    time: Res<Time>,
    defs: Res<HardpointDefs>,
    mut notices: MessageReader<StreakNotice>,
    selecting: Option<Res<airstrike::Selecting>>,
    mut hands: Query<(Entity, &Pawn, &WeaponState, Option<&mut HardpointHands>), (With<LocalSlot>, Without<Dead>)>,
    mut was_selecting: Local<Option<Entity>>,
) {
    let now = time.elapsed_secs();
    let begin = |w: &WeaponState, def: &'static WeaponDef, press: bool, selecting: bool| HardpointHands {
        def,
        phase: HandsPhase::GunDown,
        started: now,
        until: now + w.def.drop_time.max(0.05),
        press,
        selecting,
        closed_at: f32::INFINITY,
        carry: false,
    };
    // The airstrike's map opened: up comes the detonator, held while the
    // spot is picked.
    let owner = selecting.as_ref().map(|s| s.owner);
    if owner != *was_selecting {
        if let (Some(e), Some(def)) = (owner, defs.airstrike) {
            if let Ok((_, _, w, None)) = hands.get_mut(e) {
                commands.entity(e).insert(begin(w, def, false, true));
            }
        }
        // Closed: picked (pressed, below) or called off (put away).
        if let Some(e) = was_selecting.filter(|_| owner.is_none()) {
            if let Ok((_, _, _, Some(mut h))) = hands.get_mut(e) {
                h.selecting = false;
                h.closed_at = now;
            }
        }
        *was_selecting = owner;
    }
    for n in notices.read() {
        let StreakNotice::CalledIn { item, by, .. } = n else { continue };
        let Some(def) = defs.of(*item) else { continue };
        for (e, pawn, w, h) in &mut hands {
            if pawn.name != *by {
                continue;
            }
            match h {
                // The airstrike's spot picked: pressed once it's up.
                Some(mut h) if h.def.name == def.name => {
                    h.press = true;
                    h.selecting = false;
                }
                Some(_) => {}
                None => {
                    commands.entity(e).insert(begin(w, def, true, false));
                }
            }
        }
    }
}

/// A sentry gun picked up to carry out: the gun down and the hands away.
fn carry_sentry(
    mut commands: Commands,
    time: Res<Time>,
    carriers: Query<(Entity, &WeaponState), (With<LocalSlot>, With<super::sentry::Carrying>, Without<HardpointHands>, Without<Dead>)>,
) {
    let now = time.elapsed_secs();
    for (e, w) in &carriers {
        commands.entity(e).insert(HardpointHands {
            def: w.def,
            phase: HandsPhase::GunDown,
            started: now,
            until: now + w.def.drop_time.max(0.05),
            press: false,
            selecting: false,
            closed_at: f32::INFINITY,
            carry: true,
        });
    }
}

/// Each part of the call-in to its time, the next after it.
fn advance(
    mut commands: Commands,
    time: Res<Time>,
    mut hands: Query<(Entity, &WeaponState, &mut HardpointHands, Has<Dead>, Has<super::sentry::Carrying>)>,
) {
    let now = time.elapsed_secs();
    for (e, w, mut h, dead, carrying) in &mut hands {
        if dead {
            commands.entity(e).remove::<HardpointHands>();
            continue;
        }
        let next = match h.phase {
            // (A spot picked is called in a frame or so after the map
            // closes: called off only if nothing came.)
            HandsPhase::Hold if !h.selecting && (h.press || now - h.closed_at > CALL_GRACE) => Some(if h.press { (HandsPhase::Press, PRESS) } else { (HandsPhase::PutAway, drop_time(h.def)) }),
            HandsPhase::Hold => None,
            // The sentry planted: the gun back up.
            HandsPhase::Away if !carrying => Some((HandsPhase::GunUp, w.def.raise_time)),
            HandsPhase::Away => None,
            _ if now < h.until => None,
            HandsPhase::GunDown if h.carry => Some((HandsPhase::Away, 0.0)),
            HandsPhase::GunDown => Some((HandsPhase::Raise, raise_time(h.def))),
            HandsPhase::Raise if h.selecting || !h.press => Some((HandsPhase::Hold, 0.0)),
            HandsPhase::Raise => Some((HandsPhase::Press, PRESS)),
            HandsPhase::Press => Some((HandsPhase::PutAway, drop_time(h.def))),
            HandsPhase::PutAway => Some((HandsPhase::GunUp, w.def.raise_time)),
            HandsPhase::GunUp => {
                commands.entity(e).remove::<HardpointHands>();
                None
            }
        };
        if let Some((phase, seconds)) = next {
            // The phone's raise, press and putting away: the captured take at
            // its own pace (CoD4's at its set time), times
            // `hands.phone.speed`.
            let seconds = if !is_laptop(h.def) && matches!(phase, HandsPhase::Raise | HandsPhase::Press | HandsPhase::PutAway) {
                let speed = crate::tune::get("hands.phone.speed", 1.8).clamp(0.1, 5.0);
                super::mocap::phone_anim(phase).map_or(seconds, |a| a.duration()) / speed
            } else {
                seconds
            };
            h.phase = phase;
            h.started = now;
            h.until = now + seconds.max(0.05);
        }
    }
}

/// No firing, aiming or reloading with the detonator in hand (or the gun
/// on its way down or up).
fn hold_gun(mut pawns: Query<&mut WeaponInput, With<HardpointHands>>) {
    for mut input in &mut pawns {
        input.fire = false;
        input.ads = false;
        input.reload = false;
        input.inspect = false;
    }
}

/// What the hands hold calling one in, CoD4's own (in every multiplayer
/// map's zone): its phone (`com_cellphone_on`) for the UAV and the
/// helicopter, its open laptop (`com_laptop_2_open`) for the airstrike,
/// held while its spot is picked on the map. In the C4 viewmodel's hands,
/// its detonator hidden: the detonator's anims are a hand holding something
/// small and a thumb pressing it.
pub fn prop(def: &WeaponDef) -> &'static str {
    if is_laptop(def) { "com_laptop_2_open" } else { "com_cellphone_on" }
}

/// The airstrike's: the laptop, in the bomb case's place.
pub fn is_laptop(def: &WeaponDef) -> bool {
    def.name == "briefcase_bomb_mp" || def.name == "airstrike_mp"
}

/// How a prop is held: what of the weapon's model is hidden, the joint the
/// prop goes under and the one it follows (the anims move that one, and
/// hidden it's scaled to nothing, but still placed). The phone: in the C4's
/// hands, its detonator hidden. The laptop: in the bomb's, its case (all of
/// it, on `j_gun`) hidden.
struct Hold {
    hide: &'static [&'static str],
    under: &'static str,
    follow: &'static str,
}

fn hold(def: &WeaponDef) -> Hold {
    if def.name == "briefcase_bomb_mp" {
        Hold { hide: &["j_gun"], under: "tag_weapon", follow: "j_gun" }
    } else {
        Hold { hide: &["tag_clip2"], under: "j_gun", follow: "tag_clip2" }
    }
}

/// A call-in's raise and putting away (seconds): the bomb's (2.5 s and
/// 1.7 s, for planting it) quickened to about the C4's.
fn raise_time(def: &WeaponDef) -> f32 {
    def.raise_time.min(1.4)
}

fn drop_time(def: &WeaponDef) -> f32 {
    def.drop_time.min(1.0)
}

/// The laptop's screen (`crate::ui` draws the airstrike's map on it): its
/// picture, made at start-up.
pub static SCREEN_IMAGE: std::sync::OnceLock<Handle<Image>> = std::sync::OnceLock::new();
/// The laptop's screen is up with its map (the HUD's map stands down).
static LAPTOP_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn laptop_open() -> bool {
    LAPTOP_OPEN.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_laptop_open(open: bool) {
    LAPTOP_OPEN.store(open, std::sync::atomic::Ordering::Relaxed);
}

/// The quad over the laptop's lid showing [`SCREEN_IMAGE`]: its size
/// (metres, before the prop's scale), for finding where the mouse points.
#[derive(Component)]
pub struct LaptopScreen {
    pub size: Vec2,
}

/// Where the prop sits in the hand (live tuned: `hands.phone.*`,
/// `hands.laptop.*`: x, y, z in inches, yaw, pitch, roll in degrees, scale).
#[derive(Component)]
pub struct PropAnchor {
    laptop: bool,
    follow: Option<Entity>,
}

/// Put the prop in the hand of a hardpoint's viewmodel (`crate::viewmodel`
/// calls this as it builds one), and hide the detonator.
#[allow(clippy::too_many_arguments)]
pub fn spawn_prop(
    commands: &mut Commands,
    content: &mut crate::content::Content,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    bindposes: &mut Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>,
    skeleton: &mut crate::models::Skeleton,
    def: &WeaponDef,
    layer: bevy::camera::visibility::RenderLayers,
) {
    let how = hold(def);
    for j in how.hide {
        skeleton.hide(j);
    }
    let name = prop(def);
    let (Some(model), Some(joint)) = (content.model(name, meshes, materials, images, bindposes), skeleton.joint(how.under)) else {
        warn!("hardpoint hands: no {name} (or no {}) to hold", how.under);
        return;
    };
    let follow = skeleton.joint(how.follow);
    let laptop = is_laptop(def);
    let anchor = commands.spawn((PropAnchor { laptop, follow }, Transform::default(), Visibility::default(), ChildOf(joint))).id();
    // Its own skeleton: its `tag_origin` isn't the gun's.
    let mut own = crate::models::Skeleton::default();
    crate::models::spawn_model(commands, &mut own, crate::models::SpawnModel { model: &model, owner: anchor, attach_to: None, layers: Some(layer.clone()), shadows: false });
    // The laptop's screen: a quad on the lid's face (the model's own
    // inches: its screen surface, `mtl_laptop_envu_screen`, leans back from
    // x -5.8, z 2.6 to x -8.9, z 14.2, 19.1 across, its face to +x),
    // showing the airstrike's map.
    if let (true, Some(image)) = (laptop, SCREEN_IMAGE.get()) {
        let g = |n: &str, d: f32| crate::tune::get(&format!("hands.screen.{n}"), d);
        let (bottom, top) = (Vec3::new(-5.8, -0.05, 2.6), Vec3::new(-8.9, -0.05, 14.2));
        let up = (top - bottom).normalize();
        let right = Vec3::Y;
        let normal = right.cross(up);
        let middle = (bottom + top) * 0.5 + normal * g("off", 0.06) + up * g("up", 0.0);
        let size = Vec2::new(g("w", 19.1), g("h", (top - bottom).length()));
        let rotation = Quat::from_mat3(&Mat3::from_cols(crate::units::dir(right.to_array()), crate::units::dir(up.to_array()), crate::units::dir(normal.to_array())));
        let size_m = size * crate::units::INCH;
        commands.spawn((
            LaptopScreen { size: size_m },
            Mesh3d(meshes.add(Rectangle::new(size_m.x, size_m.y))),
            MeshMaterial3d(materials.add(StandardMaterial { base_color_texture: Some(image.clone()), unlit: true, ..default() })),
            Transform::from_translation(crate::units::pos(middle.to_array())).with_rotation(rotation),
            bevy::light::NotShadowCaster,
            layer,
            ChildOf(anchor),
        ));
    }
}

fn place_props(mut anchors: Query<(&PropAnchor, &mut Transform)>, joints: Query<&Transform, Without<PropAnchor>>) {
    for (a, mut tf) in &mut anchors {
        // Where the detonator is now (at full size).
        let at = a.follow.and_then(|f| joints.get(f).ok()).map_or(Transform::IDENTITY, |f| Transform::from_translation(f.translation).with_rotation(f.rotation));
        let k = if a.laptop { "hands.laptop" } else { "hands.phone" };
        let g = |n: &str, d: f32| crate::tune::get(&format!("{k}.{n}"), d);
        // Fitted by eye: the phone upright in the fist, its screen to the
        // eye; the laptop where the bomb's case was, its lid where the
        // case's lid was (the case opens to +x, the laptop to -x).
        let (x, y, z, yaw, pitch, roll, scale) =
            if a.laptop { (0.0, 0.0, 0.0, 180.0, 0.0, 0.0, 1.0) } else { (0.0, 0.5, -2.0, 190.0, -20.0, 0.0, 0.9) };
        let want = Transform::from_translation(crate::units::pos([g("x", x), g("y", y), g("z", z)]))
            .with_rotation(Quat::from_euler(EulerRot::YXZ, g("yaw", yaw).to_radians(), g("pitch", pitch).to_radians(), g("roll", roll).to_radians()))
            .with_scale(Vec3::splat(g("scale", scale)));
        let want = at * want;
        if *tf != want {
            *tf = want;
        }
    }
}

/// Test aid: `COD4RW_HANDSHOT=<dir>` saves a screenshot every 0.1 s while
/// a call-in's hands are up.
fn shots(mut commands: Commands, time: Res<Time>, hands: Query<&HardpointHands>, mut next: Local<f32>, mut n: Local<u32>) {
    let now = time.elapsed_secs();
    if hands.is_empty() || now < *next {
        return;
    }
    *next = now + 0.1;
    let dir = std::path::PathBuf::from(std::env::var("COD4RW_HANDSHOT").unwrap_or_default());
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(format!("h{:03}.png", *n));
    *n += 1;
    commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(path));
}
