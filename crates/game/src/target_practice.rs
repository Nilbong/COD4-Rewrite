//! Target practice (`COD4RW_TARGETS=<metres>[,<metres>...]`, from
//! "Target Practice.bat"): enemies that stand still at those distances in
//! front of the player, coming back to the same spot a second after each
//! kill, in a match that never ends. For trying guns, hit markers and the
//! like. The direction is the clearest one around the player's first spawn,
//! and the player is turned to face it. A teammate stands still beside the
//! player too (for the minimap's friendly mark).

use crate::combat::{Dead, Pawn};
use crate::movement::{Frozen, Mover, ViewAngles};
use crate::player::LocalPlayer;
use crate::units::u;
use avian3d::prelude::*;
use bevy::prelude::*;

pub struct TargetPracticePlugin;

impl Plugin for TargetPracticePlugin {
    fn build(&self, app: &mut App) {
        if distances().is_empty() {
            return;
        }
        app.add_systems(Update, (spawn_targets, place_targets, hold_targets).chain().run_if(crate::state::in_game));
    }
}

/// The targets' distances (metres), from `COD4RW_TARGETS`.
pub fn distances() -> Vec<f32> {
    std::env::var("COD4RW_TARGETS")
        .ok()
        .map(|v| v.split(',').filter_map(|x| x.trim().parse::<f32>().ok()).filter(|&m| m > 0.5).collect())
        .unwrap_or_default()
}

/// Whether target practice is on (the match then never ends).
pub fn active() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !distances().is_empty())
}

/// A target and where it stands (set once the map's collision is in).
#[derive(Component)]
pub struct Target {
    metres: f32,
    spot: Option<(Vec3, f32)>,
    /// The player's teammate beside them, not a target.
    friend: bool,
}

fn spawn_targets(
    mut commands: Commands,
    assets: Option<Res<crate::combat::PawnAssets>>,
    player: Query<(&Transform, &Pawn), With<LocalPlayer>>,
    targets: Query<(), With<Target>>,
    mut done: Local<bool>,
) {
    let (Some(assets), Ok((tf, me))) = (assets, player.single()) else { return };
    if *done || !targets.is_empty() {
        return;
    }
    *done = true;
    let sp = crate::world::SpawnPoint { pos: tf.translation, yaw: 0.0, kind: crate::world::SpawnKind::Tdm };
    for (i, m) in distances().into_iter().enumerate() {
        let e = crate::combat::spawn_pawn(&mut commands, &assets, &format!("Target {}", i + 1), me.team.other(), &sp);
        commands.entity(e).insert((Target { metres: m, spot: None, friend: false }, Frozen));
    }
    let e = crate::combat::spawn_pawn(&mut commands, &assets, "Buddy", me.team, &sp);
    commands.entity(e).insert((Target { metres: 0.0, spot: None, friend: true }, Frozen));
}

/// Once there's collision to test against: the clearest of 16 directions
/// around the player (up to the farthest target), the player turned to face
/// it, each target on the line at its distance (short of any wall), a little
/// apart.
fn place_targets(
    spatial: SpatialQuery,
    mut player: Query<(&Transform, &Mover, &mut ViewAngles), (With<LocalPlayer>, Without<Target>)>,
    mut targets: Query<(&mut Target, &mut Transform), Without<LocalPlayer>>,
) {
    if targets.iter().all(|(t, _)| t.spot.is_some()) {
        return;
    }
    let Ok((tf, mover, mut view)) = player.single_mut() else { return };
    let eye = mover.eye(tf.translation);
    let far = u(targets.iter().map(|(t, _)| t.metres).fold(1.0, f32::max) / crate::units::INCH);
    let filter = crate::collision::sight_filter();
    let clear = |dir: Vec3| {
        Dir3::new(dir).ok().map_or(0.0, |d| spatial.cast_ray(eye, d, far + 1.0, true, &filter).map_or(far + 1.0, |h| h.distance))
    };
    let (yaw, room) = (0..16)
        .map(|k| {
            let yaw = view.yaw + k as f32 * std::f32::consts::TAU / 16.0;
            (yaw, clear(Quat::from_rotation_y(yaw) * Vec3::NEG_Z))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((view.yaw, far));
    view.yaw = yaw;
    view.pitch = 0.0;
    let fwd = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
    let right = Quat::from_rotation_y(yaw) * Vec3::X;
    let floor = |p: Vec3| {
        spatial
            .cast_ray(p + Vec3::Y * u(60.0), Dir3::NEG_Y, u(400.0), true, &filter)
            .map_or(p, |h| p + Vec3::Y * u(60.0) - Vec3::Y * h.distance)
    };
    let count = targets.iter().filter(|(t, _)| !t.friend).count();
    for (i, (mut t, mut ttf)) in targets.iter_mut().filter(|(t, _)| !t.friend).enumerate() {
        let d = (u(t.metres / crate::units::INCH)).min(room - u(40.0)).max(u(60.0));
        let side = (i as f32 - (count as f32 - 1.0) * 0.5) * u(40.0);
        let at = floor(tf.translation + fwd * d + right * side);
        // Facing the player.
        let face = yaw + std::f32::consts::PI;
        t.spot = Some((at, face));
        ttf.translation = at + Vec3::Y * u(1.0);
    }
    // The teammate: beside the player and a little behind, wherever there's
    // room (right, else left), facing the same way.
    let beside = [right, -right].into_iter().map(|side| {
        let room = Dir3::new(side).ok().map_or(0.0, |d| spatial.cast_ray(eye, d, u(250.0), true, &filter).map_or(u(250.0), |h| h.distance));
        (side, room)
    });
    let (side, room) = beside.max_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((right, u(100.0)));
    for (mut t, mut ttf) in targets.iter_mut().filter(|(t, _)| t.friend) {
        let at = floor(tf.translation + side * (room - u(30.0)).clamp(u(60.0), u(160.0)) - fwd * u(60.0));
        t.spot = Some((at, yaw));
        ttf.translation = at + Vec3::Y * u(1.0);
    }
}

/// Keep each target on its spot, standing still and facing the player; a
/// second after a kill it's back there with full health.
fn hold_targets(
    mut commands: Commands,
    time: Res<Time>,
    mut targets: Query<(Entity, &Target, &mut Transform, &mut ViewAngles, &mut Mover, Option<&mut Dead>, Has<Frozen>)>,
) {
    let now = time.elapsed_secs();
    for (e, t, mut tf, mut view, mut mover, dead, frozen) in &mut targets {
        let Some((at, face)) = t.spot else { continue };
        if let Some(mut d) = dead {
            d.respawn_at = d.respawn_at.min(now + 1.0);
            continue;
        }
        if tf.translation.distance(at) > u(2.0) {
            tf.translation = at + Vec3::Y * u(1.0);
        }
        *view = ViewAngles { yaw: face, pitch: 0.0 };
        mover.velocity = Vec3::ZERO;
        mover.on_ground = true;
        if !frozen {
            commands.entity(e).insert(Frozen);
        }
    }
}
