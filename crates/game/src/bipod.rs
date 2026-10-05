//! World at War's bipods. A gun with its bipod attachment deploys it when
//! the player aims prone, or crouched or standing at a ledge of the right
//! height. Deployed, the player can't move, the view stays within the
//! mount's arcs and the gun fires with its mounted weapon file's stats
//! (`<gun>_bipod_<stance>_mp`, a turret: faster, tighter, a small jitter for
//! recoil) through [`WeaponState::mounted`]. Aiming again, moving, jumping
//! or changing stance takes it down.

use crate::collision;
use crate::combat::Dead;
use crate::loadout::Loadout;
use crate::movement::{MoveInput, Mover, Stance, ViewAngles};
use crate::player::LocalPlayer;
use crate::units::{INCH, u};
use crate::weapons::{WeaponDef, WeaponInput, WeaponState};
use avian3d::prelude::*;
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct BipodPlugin;

impl Plugin for BipodPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BipodHint>()
            .add_systems(OnEnter(crate::state::GameState::InGame), spawn_hint.in_set(crate::state::Setup::Spawn))
            .add_systems(
                Update,
                (deploy.after(crate::player::InputSet).before(crate::movement::MovementSet), show_hint)
                    .run_if(crate::state::in_game),
            );
        if std::env::var_os("COD4RW_BIPODSCAN").is_some() {
            app.add_systems(Update, scan.run_if(crate::state::in_game));
        }
    }
}

/// A bipod in use: the mount's stats and where it points.
#[derive(Component, Clone, Copy, Debug)]
pub struct Deployed {
    /// The gun it's on (switching weapons takes it down).
    base: &'static WeaponDef,
    stance: Stance,
    /// The view's yaw when it went down; the arcs are about it.
    yaw: f32,
    mount: Mount,
}

/// A gun's mount for one stance.
#[derive(Clone, Copy, Debug)]
struct Mount {
    def: &'static WeaponDef,
    /// Degrees the view may turn: left, right, up, down.
    arcs: [f32; 4],
}

/// What the deploy hint shows.
#[derive(Resource, Default, Clone, Copy, PartialEq)]
struct BipodHint {
    can_deploy: bool,
}

/// Ledge tops a bipod rests on, in CoD units above the feet.
fn ledge_heights(stance: Stance) -> Option<(f32, f32)> {
    match stance {
        Stance::Stand => Some((30.0, 52.0)),
        Stance::Crouch => Some((16.0, 34.0)),
        Stance::Prone => None,
    }
}

/// Mounts made so far, by gun and stance (their defs live for the run, like
/// `loadout`'s).
static MOUNTS: Mutex<Option<HashMap<(String, u8), Option<Mount>>>> = Mutex::new(None);

/// The mount a gun (its spec, `t4_bar:bipod`, and def) has for `stance`.
fn mount(spec: &str, base: &'static WeaponDef, stance: Stance) -> Option<Mount> {
    let key = (format!("{spec} {}", base.name), stance as u8);
    let mut cache = MOUNTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = cache.get_or_insert_default().get(&key) {
        return *m;
    }
    let made = make_mount(spec, base, stance);
    cache.get_or_insert_default().insert(key, made);
    made
}

fn make_mount(spec: &str, base: &WeaponDef, stance: Stance) -> Option<Mount> {
    let (weapon, attachments) = crate::gunmodel::parse(spec);
    if !crate::waw::is_waw(weapon) || !attachments.contains(&"bipod") {
        return None;
    }
    let data = crate::waw::data()?;
    let file = data.weapon(&format!("{}_bipod_mp", data.gun(weapon)?.name))?;
    if file.get("mountableWeapon").trim() != "1" {
        return None;
    }
    let key = match stance {
        Stance::Stand => "standMountedWeapdef",
        Stance::Crouch => "crouchMountedWeapdef",
        Stance::Prone => "proneMountedWeapdef",
    };
    let m = data.weapon(file.get(key).trim())?;
    let f = |k: &str| m.get(k).trim().parse::<f32>().ok();
    let mut def = base.clone();
    if let Some(t) = f("fireTime").filter(|&t| t > 0.0) {
        def.fire_time = t;
    }
    for (field, k) in [
        (&mut def.damage, "damage"),
        (&mut def.min_damage, "minDamage"),
        (&mut def.max_damage_range, "maxDamageRange"),
        (&mut def.min_damage_range, "minDamageRange"),
    ] {
        if let Some(v) = f(k).filter(|&v| v > 0.0) {
            *field = v;
        }
    }
    // A turret's cone, no bloom, and its view jitter in place of kick.
    let spread = f("playerSpread").unwrap_or(0.5);
    def.hip_spread_min = [spread; 3];
    def.hip_spread_max = [spread; 3];
    def.ads_spread = spread;
    def.hip_spread_fire_add = 0.0;
    def.hip_spread_move_add = 0.0;
    let (h, v) = (f("horizViewJitter").unwrap_or(0.2), f("vertViewJitter").unwrap_or(0.2));
    def.kick_pitch = (-v * 0.5, v);
    def.kick_yaw = (-h, h);
    def.ads_kick_pitch = def.kick_pitch;
    def.ads_kick_yaw = def.kick_yaw;
    let arc = |k: &str| f(k).unwrap_or(60.0);
    Some(Mount { def: Box::leak(Box::new(def)), arcs: [arc("leftArc"), arc("rightArc"), arc("topArc"), arc("bottomArc")] })
}

/// Is there a ledge to rest on ahead, at a height for `stance`? Looks down
/// onto a flat top a little ahead, with open space above it.
fn ledge_ahead(spatial: &SpatialQuery, feet: Vec3, yaw: f32, stance: Stance) -> bool {
    let Some((lo, hi)) = ledge_heights(stance) else { return false };
    let filter = collision::movement_filter();
    let fwd = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
    [18.0, 26.0, 34.0].into_iter().any(|dist| {
        let top = feet + Vec3::Y * u(hi + 4.0) + fwd * u(dist);
        let Some(hit) = spatial.cast_ray(top, Dir3::NEG_Y, u(hi + 4.0 - lo), true, &filter) else { return false };
        let height = hi + 4.0 - hit.distance / INCH;
        let flat = hit.normal.y > 0.7 && hit.distance > 0.0;
        // Clear above the top, from the player out past it.
        let over = feet + Vec3::Y * u(height + 8.0);
        let open = Dir3::new(fwd).is_ok_and(|d| spatial.cast_ray(over, d, u(dist + 16.0), true, &filter).is_none());
        flat && open && (lo..=hi).contains(&height)
    })
}

/// A wrapped angle in -PI..PI.
fn wrap(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn deploy(
    mut commands: Commands,
    spatial: SpatialQuery,
    mut hint: ResMut<BipodHint>,
    mut player: Query<
        (
            Entity,
            &Transform,
            &Mover,
            &mut MoveInput,
            &mut WeaponInput,
            &mut ViewAngles,
            &mut WeaponState,
            Option<&Loadout>,
            Option<&Deployed>,
            Has<LocalPlayer>,
        ),
        (With<crate::splitscreen::LocalSlot>, Without<Dead>),
    >,
    dead: Query<Entity, (With<Deployed>, With<Dead>)>,
    mut states: Local<std::collections::HashMap<Entity, (bool, bool)>>,
) {
    for e in &dead {
        commands.entity(e).remove::<Deployed>();
    }
    if !player.iter().any(|p| p.9) {
        hint.set_if_neq(BipodHint::default());
    }
    states.retain(|e, _| player.contains(*e));
    // Each local player's (the hint is Player 1's).
    for (entity, tf, mover, mut mv, mut wi, mut view, mut weapon, loadout, deployed, first) in &mut player {
        let (last_ads, hold_off) = states.entry(entity).or_default();
        let mut own_hint = BipodHint::default();
        deploy_one(
            &mut commands,
            &spatial,
            (entity, tf, mover, &mut mv, &mut wi, &mut view, &mut weapon, loadout, deployed),
            (last_ads, hold_off),
            &mut own_hint,
        );
        if first {
            hint.set_if_neq(own_hint);
        }
    }
}

/// One player's bipod this frame.
#[allow(clippy::type_complexity)]
fn deploy_one(
    commands: &mut Commands,
    spatial: &SpatialQuery,
    (entity, tf, mover, mv, wi, view, weapon, loadout, deployed): (
        Entity,
        &Transform,
        &Mover,
        &mut MoveInput,
        &mut WeaponInput,
        &mut ViewAngles,
        &mut WeaponState,
        Option<&Loadout>,
        Option<&Deployed>,
    ),
    (last_ads, hold_off): (&mut bool, &mut bool),
    hint: &mut BipodHint,
) {
    let pressed = wi.ads && !*last_ads;
    *last_ads = wi.ads;
    // After taking it down by aiming, no aiming until the button is let go.
    if !wi.ads {
        *hold_off = false;
    }
    let busy = weapon.reloading() || mover.sprinting || loadout.is_some_and(|l| l.switching.is_some());
    let gun = weapon.def;
    let mount_for = |stance| loadout.and_then(|l| mount(&l.gun().spec, gun, stance));

    if let Some(d) = deployed {
        let moving = mv.forward.abs() > 0.3 || mv.right.abs() > 0.3;
        let undeploy = pressed || moving || mv.jump || mv.stance != d.stance || !std::ptr::eq(weapon.def, d.base);
        if undeploy {
            info!("bipod: taken down");
            commands.entity(entity).remove::<Deployed>();
            weapon.mounted = None;
            *hold_off = pressed;
        } else {
            // Down on the bipod: still, unaimed, within the arcs.
            (mv.forward, mv.right, mv.jump, mv.sprint) = (0.0, 0.0, false, false);
            wi.ads = false;
            weapon.mounted = Some(d.mount.def);
            let [left, right, up, down] = d.mount.arcs.map(f32::to_radians);
            view.yaw = d.yaw + wrap(view.yaw - d.yaw).clamp(-right, left);
            view.pitch = view.pitch.clamp(-down, up);
        }
        *hint = BipodHint::default();
    } else {
        let stance = mover.stance;
        // Settled (`on_ground` can miss some floors while standing still).
        let settled = mover.on_ground || mover.velocity.y.abs() < 0.05;
        let spot = settled && (stance == Stance::Prone || ledge_ahead(spatial, tf.translation, view.yaw, stance));
        let mount = if spot && !busy && mover.stance == mv.stance { mount_for(stance) } else { None };
        if let (Some(m), true) = (mount, pressed && !*hold_off) {
            commands.entity(entity).insert(Deployed { base: weapon.def, stance, yaw: view.yaw, mount: m });
            weapon.mounted = Some(m.def);
            weapon.ads = 0.0;
            wi.ads = false;
            info!("bipod: deployed ({stance:?})");
        } else if weapon.mounted.is_some() {
            weapon.mounted = None;
        }
        if *hold_off {
            wi.ads = false;
        }
        *hint = BipodHint { can_deploy: mount.is_some() };
    }
}

/// Debug aid: with `COD4RW_BIPODSCAN=1`, 3 s into a match, log whether the
/// player faces a ledge, and standing spots nearby with one to deploy on,
/// as `COD4RW_SPAWN` values (CoD units and degrees).
fn scan(
    time: Res<Time>,
    spatial: SpatialQuery,
    player: Query<(&Transform, &ViewAngles), With<LocalPlayer>>,
    mut done: Local<bool>,
) {
    if *done || time.elapsed_secs() < 3.0 {
        return;
    }
    let Ok((tf, view)) = player.single() else { return };
    *done = true;
    let filter = collision::movement_filter();
    // The player's own spot, as it faces.
    for stance in [Stance::Stand, Stance::Crouch] {
        info!("bipod scan: here, {stance:?}: a ledge ahead {}", ledge_ahead(&spatial, tf.translation, view.yaw, stance));
    }
    let mut found = 0;
    'grid: for ix in -40..=40 {
        for iz in -40..=40 {
            let probe = tf.translation + Vec3::new(ix as f32, 0.0, iz as f32) * u(24.0) + Vec3::Y * u(60.0);
            let Some(floor) = spatial.cast_ray(probe, Dir3::NEG_Y, u(200.0), true, &filter) else { continue };
            if floor.distance <= 0.0 || floor.normal.y < 0.9 {
                continue;
            }
            let feet = probe - Vec3::Y * floor.distance;
            for k in 0..8 {
                let yaw = k as f32 * std::f32::consts::FRAC_PI_4;
                if ledge_ahead(&spatial, feet, yaw, Stance::Stand) {
                    let c = crate::units::to_cod(feet);
                    info!("bipod scan: COD4RW_SPAWN={:.0},{:.0},{:.0},{:.0}", c[0], c[1], c[2] + 1.0, yaw.to_degrees() + 90.0);
                    found += 1;
                    if found >= 12 {
                        break 'grid;
                    }
                }
            }
        }
    }
    info!("bipod scan: {found} spots");
}

#[derive(Component)]
struct HintText;

#[derive(Component)]
struct HintPad;

fn spawn_hint(mut commands: Commands) {
    let node = || Node {
        position_type: PositionType::Absolute,
        top: percent(62),
        width: percent(100),
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        justify_content: JustifyContent::Center,
        column_gap: px(7),
        ..default()
    };
    commands.spawn((
        HintText,
        Text::new("Right click to deploy the bipod"),
        TextFont { font_size: FontSize::Px(18.0), ..default() },
        TextColor(Color::WHITE),
        TextShadow::default(),
        TextLayout::justify(Justify::Center),
        node(),
        GlobalZIndex(crate::gamepad::prompts::PROMPT_Z),
        Visibility::Hidden,
    ));
    use crate::gamepad::prompts::{PadPrompt, glyph, text};
    commands.spawn((
        HintPad,
        PadPrompt { parts: vec![glyph(crate::gamepad::glyphs::Glyph::LeftTrigger), text("Deploy the bipod")], height: 28.0 },
        node(),
        GlobalZIndex(crate::gamepad::prompts::PROMPT_Z),
        Visibility::Hidden,
    ));
}

/// The deploy hint, worded for the device in use.
fn show_hint(
    hint: Res<BipodHint>,
    active: Res<crate::gamepad::ActiveDevice>,
    mut text: Query<&mut Visibility, (With<HintText>, Without<HintPad>)>,
    mut pad: Query<&mut Visibility, (With<HintPad>, Without<HintText>)>,
) {
    let show = |on: bool| if on { Visibility::Inherited } else { Visibility::Hidden };
    let on_pad = active.pad.is_some();
    for mut v in &mut text {
        v.set_if_neq(show(hint.can_deploy && !on_pad));
    }
    for mut v in &mut pad {
        v.set_if_neq(show(hint.can_deploy && on_pad));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledges_fit_the_stance() {
        let (lo, hi) = ledge_heights(Stance::Stand).unwrap();
        assert!(lo < 40.0 && 40.0 < hi);
        let (lo, hi) = ledge_heights(Stance::Crouch).unwrap();
        assert!(lo < 24.0 && 24.0 < hi);
        assert!(ledge_heights(Stance::Prone).is_none());
    }

    #[test]
    fn arcs_wrap() {
        assert!((wrap(3.0 * std::f32::consts::PI).abs() - std::f32::consts::PI).abs() < 1e-4);
        assert!((wrap(0.3) - 0.3).abs() < 1e-6);
    }

    /// Against the local install: `cargo test -p game bipod -- --ignored`.
    #[test]
    #[ignore]
    fn reads_mounts() {
        let base = crate::weapons::WeaponDef::fallback();
        let base: &'static WeaponDef = Box::leak(Box::new(base));
        for stance in [Stance::Prone, Stance::Crouch, Stance::Stand] {
            let m = make_mount("t4_bar:bipod", base, stance).expect("BAR bipod mount");
            assert!(m.def.hip_spread_min[0] < 1.0 && m.arcs[0] > 0.0);
        }
        assert!(make_mount("t4_bar", base, Stance::Prone).is_none());
    }
}
