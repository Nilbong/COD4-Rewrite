//! CoD4's hardpoints (`_hardpoints.gsc`): 3, 5 and 7 kills since your last
//! death earn a UAV, an airstrike and a helicopter (none on maps without
//! helicopter paths). One is held at a time (a
//! better one replaces a worse) and kept through death until it's used:
//! [`HardpointInput`], which the player's 6 key sets (CoD4's `bind 6 "+actionslot 4"`) and bots set
//! themselves. Each hardpoint lives in its own module; what the local
//! player should hear about comes out as [`StreakNotice`]s for the HUD.

pub mod airstrike;
pub mod helicopter;
mod test;
pub mod uav;

use crate::audio::{Sfx, Sides};
use crate::combat::{Dead, Killed, Pawn, Team, hostile};
use crate::content::Content;
use crate::player::LocalPlayer;
use crate::state::in_game;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

pub struct KillstreaksPlugin;

impl Plugin for KillstreaksPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<StreakNotice>()
            .add_message::<helicopter::ShotDown>()
            .init_resource::<uav::Radar>()
            .init_resource::<airstrike::Airstrikes>()
            .init_resource::<helicopter::HeliPaths>()
            .init_resource::<helicopter::Calls>()
            .add_systems(OnEnter(crate::state::GameState::InGame), reset)
            .add_systems(
                OnEnter(crate::state::GameState::InGame),
                helicopter::load.after(crate::world::load_map).in_set(crate::state::Setup::Content),
            )
            .add_systems(
                Update,
                (
                    equip,
                    count_streaks,
                    player_input,
                    use_hardpoints,
                    selecting_cursor,
                    airstrike::run,
                    helicopter::spawn,
                    helicopter::take_damage,
                    helicopter::fly,
                    helicopter::fight,
                )
                    .chain()
                    .run_if(in_game),
            );
        test::register(app);
    }
}

/// A hardpoint, worst first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hardpoint {
    Uav,
    Airstrike,
    Helicopter,
}

impl Hardpoint {
    /// The kill streak that earns it.
    pub fn kills(self) -> u32 {
        match self {
            Hardpoint::Uav => 3,
            Hardpoint::Airstrike => 5,
            Hardpoint::Helicopter => 7,
        }
    }

    /// Its weapon's `hudIcon` (`radar_mp`, `airstrike_mp`, `helicopter_mp`).
    pub fn icon(self) -> &'static str {
        match self {
            Hardpoint::Uav => "compass_objpoint_satallite",
            Hardpoint::Airstrike => "compass_objpoint_airstrike",
            Hardpoint::Helicopter => "compass_objpoint_helicopter",
        }
    }

    /// `level.hardpointHints`: "Press [{+actionslot 4}] for RADAR.".
    pub fn hint(self) -> &'static str {
        match self {
            Hardpoint::Uav => "MP_EARNED_RADAR",
            Hardpoint::Airstrike => "MP_EARNED_AIRSTRIKE",
            Hardpoint::Helicopter => "MP_EARNED_HELICOPTER",
        }
    }

    /// `level.hardpointInforms`: the sound on earning it.
    fn inform(self) -> &'static str {
        match self {
            Hardpoint::Uav => "mp_killstreak_radar",
            Hardpoint::Airstrike => "mp_killstreak_jet",
            Hardpoint::Helicopter => "mp_killstreak_heli",
        }
    }

    /// `game["dialog"][hardpoint]`: the announcer's line on earning it.
    fn leader(self) -> &'static str {
        match self {
            Hardpoint::Uav => "uavrecon",
            Hardpoint::Airstrike => "airstrike",
            Hardpoint::Helicopter => "helisupport",
        }
    }

    fn for_streak(kills: u32) -> Option<Hardpoint> {
        [Hardpoint::Uav, Hardpoint::Airstrike, Hardpoint::Helicopter].into_iter().find(|h| h.kills() == kills)
    }
}

/// A pawn's kills since it last died, and the hardpoint it holds.
#[derive(Component, Default, Debug)]
pub struct Killstreak {
    pub kills: u32,
    pub held: Option<Hardpoint>,
}

/// Use the held hardpoint now; an airstrike also wants where (Bevy
/// space; the player's is picked on the map, bots give one).
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct HardpointInput {
    pub use_now: bool,
    pub target: Option<Vec3>,
}

/// News for the local player's HUD.
#[derive(Message, Clone, Debug)]
pub enum StreakNotice {
    /// The player's streak earned a hardpoint.
    Earned { streak: u32, item: Hardpoint },
    /// Someone used one ("UAV Recon called in by Soap for 30 seconds").
    CalledIn { item: Hardpoint, by: String, team: Team },
    /// The player's can't be used now (another airstrike or helicopter is up).
    Unavailable(Hardpoint),
}

fn reset(
    mut commands: Commands,
    mut radar: ResMut<uav::Radar>,
    mut strikes: ResMut<airstrike::Airstrikes>,
    mut calls: ResMut<helicopter::Calls>,
    helis: Query<Entity, With<helicopter::Helicopter>>,
) {
    *radar = uav::Radar::default();
    *strikes = airstrike::Airstrikes::default();
    calls.0.clear();
    for e in &helis {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<airstrike::Selecting>();
}

/// A hardpoint weapon's kill icon and its width over height, for the kill
/// feed.
pub fn kill_icon(weapon: &str) -> Option<(&'static str, f32)> {
    match weapon {
        airstrike::WEAPON => Some(("death_airstrike", 4.0)),
        w if w == helicopter::WEAPON || helicopter::ROCKETS.contains(&w) => Some(("death_helicopter", 4.0)),
        _ => None,
    }
}

/// A model's first LOD as a static entity (vehicles, bombs): one child per
/// surface, CoD's x forward along Bevy +X.
pub(crate) fn spawn_static_model(
    commands: &mut Commands,
    content: &mut Content,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    name: &str,
    transform: Transform,
) -> Option<Entity> {
    let (zi, id) = content.find(name)?;
    let xm = content.zones[zi].xmodel(id)?;
    let lod = *xm.lods.first()?;
    let surfaces: Vec<(usize, Option<iw3::zone::AssetId>)> =
        (lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize).map(|s| (s, xm.materials.get(s).copied().flatten())).collect();
    let parent = commands.spawn((Name::new(name.to_owned()), transform, Visibility::default())).id();
    for (surf, material) in surfaces {
        let (Some(mesh), Some(material)) =
            (content.static_mesh(zi, id, surf, meshes), material.and_then(|m| content.material(zi, m, materials, images)))
        else {
            continue;
        };
        commands.spawn((Mesh3d(mesh), MeshMaterial3d(material.handle), Transform::default(), ChildOf(parent)));
    }
    Some(parent)
}

/// Every pawn gets a streak and a hardpoint input.
fn equip(mut commands: Commands, pawns: Query<Entity, (With<Pawn>, Without<Killstreak>)>) {
    for e in &pawns {
        commands.entity(e).insert((Killstreak::default(), HardpointInput::default()));
    }
}

/// Count kills of enemies by the living; dying ends a streak. A streak of
/// 3, 5 or 7 gives its hardpoint unless a better one is held
/// (`giveHardpointItemForStreak`).
#[allow(clippy::too_many_arguments)]
fn count_streaks(
    time: Res<Time>,
    mut killed: MessageReader<Killed>,
    mut pawns: Query<(&Pawn, &mut Killstreak, Has<LocalPlayer>, Has<Dead>)>,
    mut notices: MessageWriter<StreakNotice>,
    mut sfx: ResMut<Sfx>,
    sides: Option<Res<Sides>>,
    heli_paths: Res<helicopter::HeliPaths>,
) {
    for k in killed.read() {
        let victim_team = pawns.get(k.victim).ok().map(|(p, ..)| p.team);
        if let Ok((_, mut s, ..)) = pawns.get_mut(k.victim) {
            s.kills = 0;
        }
        let Some(attacker) = k.attacker.filter(|&a| a != k.victim) else { continue };
        let Ok((pawn, mut streak, local, dead)) = pawns.get_mut(attacker) else { continue };
        if victim_team.is_none_or(|t| t == pawn.team) || dead {
            continue;
        }
        streak.kills += 1;
        let Some(item) = Hardpoint::for_streak(streak.kills) else { continue };
        if streak.held.is_some_and(|h| h > item) || item == Hardpoint::Helicopter && !heli_paths.available() {
            continue;
        }
        streak.held = Some(item);
        if local {
            notices.write(StreakNotice::Earned { streak: streak.kills, item });
            sfx.play(item.inform(), None);
            if let Some(sides) = &sides {
                let now = time.elapsed_secs();
                sfx.play_later(format!("{}_1mc_{}", sides.of(pawn.team).voice, item.leader()), None, now, 1.0);
            }
        }
    }
}

/// The player's 6 key (CoD4's `+actionslot 4`).
fn player_input(
    spatial: avian3d::prelude::SpatialQuery,
    mut players: Query<(&crate::splitscreen::PlayerInput, &mut HardpointInput, &Killstreak, &Transform, &crate::movement::Mover, &crate::movement::ViewAngles)>,
) {
    for (player, mut input, streak, tf, mover, view) in &mut players {
        if !player.live || !player.keys.just_pressed(KeyCode::Digit6) {
            continue;
        }
        input.use_now = true;
        // Splitscreen has no map to pick on: the airstrike goes where the
        // player aims (as far as 6000 units, onto the ground below).
        if crate::splitscreen::active() && streak.held == Some(Hardpoint::Airstrike) {
            let eye = mover.eye(tf.translation);
            let filter = crate::collision::sight_filter();
            let reach = crate::units::u(6000.0);
            let Ok(dir) = Dir3::new(view.forward()) else { continue };
            let aim = spatial.cast_ray(eye, dir, reach, true, &filter).map_or(eye + dir * reach, |h| eye + dir * h.distance);
            let top = aim + Vec3::Y * crate::units::u(4000.0);
            let ground = spatial.cast_ray(top, Dir3::NEG_Y, crate::units::u(40000.0), true, &filter);
            input.target = Some(ground.map_or(aim, |h| top - Vec3::Y * h.distance));
        }
    }
}

/// Use held hardpoints asked for (`triggerHardPoint`).
#[allow(clippy::too_many_arguments)]
fn use_hardpoints(
    mut commands: Commands,
    time: Res<Time>,
    mut pawns: Query<(Entity, &Pawn, &mut Killstreak, &mut HardpointInput, Has<Dead>, Has<LocalPlayer>)>,
    local: Query<&Pawn, With<LocalPlayer>>,
    mut radar: ResMut<uav::Radar>,
    mut strikes: ResMut<airstrike::Airstrikes>,
    selecting: Option<Res<airstrike::Selecting>>,
    mut notices: MessageWriter<StreakNotice>,
    mut sfx: ResMut<Sfx>,
    sides: Option<Res<Sides>>,
    mut heli_calls: ResMut<helicopter::Calls>,
    helis: Query<(), With<helicopter::Helicopter>>,
) {
    let now = time.elapsed_secs();
    let me = local.single().ok().cloned();
    let me = me.as_ref();
    for (entity, pawn, mut streak, mut input, dead, is_local) in &mut pawns {
        let wanted = std::mem::take(&mut input.use_now);
        let target = input.target.take();
        let (Some(item), false, true) = (streak.held, dead, wanted) else { continue };
        let used = match item {
            Hardpoint::Uav => {
                radar.call(uav::Side::of(pawn.team, entity), now);
                // `UAVAcquiredPrintAndSound`: "our UAV is online" to the team,
                // "enemy UAV" to the others.
                if let (Some(sides), Some(me)) = (&sides, me) {
                    let line = if hostile(me, pawn) { "enemyuavair" } else { "ouruavonline" };
                    sfx.play(format!("{}_1mc_{line}", sides.of(me.team).voice), None);
                }
                true
            }
            Hardpoint::Airstrike if strikes.in_progress(now) => {
                if is_local {
                    notices.write(StreakNotice::Unavailable(item));
                }
                false
            }
            Hardpoint::Airstrike => match target {
                Some(at) => {
                    strikes.call(entity, at, now);
                    // `leaderDialog( "airstrike_inbound", team )`.
                    if let (Some(sides), Some(me)) = (&sides, me.filter(|me| !hostile(me, pawn))) {
                        sfx.play(format!("{}_1mc_friendlyair", sides.of(me.team).voice), None);
                    }
                    true
                }
                // The player picks the spot on the map; 6 again puts it away.
                None if is_local => {
                    if selecting.is_some() {
                        commands.remove_resource::<airstrike::Selecting>();
                    } else {
                        commands.insert_resource(airstrike::Selecting { owner: entity });
                    }
                    false
                }
                None => false,
            },
            // One helicopter at a time (`level.chopper`).
            Hardpoint::Helicopter if !helis.is_empty() || !heli_calls.0.is_empty() => {
                if is_local {
                    notices.write(StreakNotice::Unavailable(item));
                }
                false
            }
            Hardpoint::Helicopter => {
                heli_calls.0.push((entity, pawn.team));
                // `leaderDialog( "helicopter_inbound", team )`, and
                // `"enemy_helicopter_inbound"` to the others.
                if let (Some(sides), Some(me)) = (&sides, me) {
                    let line = if hostile(me, pawn) { "enemyheli" } else { "friendlyheli" };
                    sfx.play(format!("{}_1mc_{line}", sides.of(me.team).voice), None);
                }
                true
            }
        };
        if used {
            streak.held = None;
            notices.write(StreakNotice::CalledIn { item, by: pawn.name.clone(), team: pawn.team });
        }
    }
}

/// While the player picks an airstrike's spot the map takes the mouse (the
/// view stops turning, as in CoD4's location selection); dying or a menu
/// coming up ends it.
fn selecting_cursor(
    mut commands: Commands,
    selecting: Option<Res<airstrike::Selecting>>,
    fe: Option<Res<crate::ui::Frontend>>,
    owners: Query<Has<Dead>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut was: Local<bool>,
) {
    let active = selecting.as_ref().is_some_and(|s| owners.get(s.owner).is_ok_and(|dead| !dead)) && !crate::ui::menu_open(fe.as_deref());
    if selecting.is_some() && !active {
        commands.remove_resource::<airstrike::Selecting>();
    }
    if active {
        if cursor.grab_mode != CursorGrabMode::None {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = false;
        }
    } else if *was && !crate::ui::menu_open(fe.as_deref()) {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    *was = active;
}
