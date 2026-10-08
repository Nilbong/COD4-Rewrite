//! Playing an online private match together ([`crate::online`]'s lobby
//! started it).
//!
//! The host's game is the match: its pawns, bots, shots, damage, deaths and
//! score are the real ones. A friend's soldier on the host is a pawn like a
//! bot, driven by the controls the friend's game sends ([`RemotePlayer`]).
//! Twenty times a second the host sends everyone where every pawn is
//! (a snapshot), and when a pawn's name, team, weapon or score changes, that
//! too ([`PawnInfo`]).
//!
//! A friend's game ("guest") moves its own soldier itself, straight away,
//! as the host will (same movement code, same controls), and is put right
//! when the two drift apart. Everyone else is a [`Puppet`]: posed from the
//! snapshots, a tenth of a second behind so there's always a next one to
//! move towards. The guest's game decides nothing: health, deaths and
//! respawns come from the host ([`authority`] turns its own off).
//!
//! The host also tells guests about kills (their kill feeds, ragdolls,
//! killcams and "Killed by"), team scores, grenades thrown and explosives
//! launched (seen and heard; their damage is the host's), and the end of
//! the match (everyone sees the result and goes back to the lobby).
//!
//! Not yet: killstreaks and objectives (Domination's flags, the bomb) are
//! seen only by the host, so online play is Team Deathmatch and
//! free-for-all for now; shots aren't rewound for lag.

use crate::combat::{Dead, Health, Pawn, PawnAssets, Team, spawn_pawn};
use crate::loadout::{ClassLoadout, Gun, PawnClass};
use crate::movement::{Frozen, MoveInput, Mover, Stance, ViewAngles};
use crate::online::lobby::{BOT, Class, LobbyMsg, Member, PawnInfo};
use crate::online::{EVERYONE, Event, HOST, Online};
use crate::player::LocalPlayer;
use crate::weapons::{ShotFired, WeaponInput, WeaponState};
use bevy::prelude::*;
use cod4rw_multiplayer::protocol::{INPUT_REDUNDANCY, InputCommand, PawnState, PeerId, Snapshot, button, pawn_flags};
use cod4rw_multiplayer::netcode::Interpolation;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct NetplayPlugin;

impl Plugin for NetplayPlugin {
    fn build(&self, app: &mut App) {
        use crate::movement::MovementSet;
        use crate::player::InputSet;
        app.add_systems(OnEnter(crate::state::GameState::InGame), begin.in_set(crate::state::Setup::Spawn))
            .add_systems(OnExit(crate::state::GameState::InGame), end)
            .add_systems(
                Update,
                (
                    (receive_host, spawn_remotes).chain().before(InputSet),
                    drive_remotes.after(InputSet).before(MovementSet),
                    send_world.after(crate::weapons::WeaponSet),
                    announce_end,
                    send_kills,
                    send_scores,
                    send_throws,
                    report.run_if(bevy::time::common_conditions::on_timer(std::time::Duration::from_secs(2))),
                    look_at_friend.after(InputSet).before(MovementSet).run_if(|| std::env::var_os("COD4RW_NETLOOK").is_some()),
                )
                    .run_if(crate::state::in_game.and_then(hosting)),
            )
            .add_systems(
                Update,
                (
                    (receive_guest, send_class).chain().before(InputSet),
                    aim_at_host.after(InputSet).before(send_inputs).run_if(|| std::env::var("COD4RW_NETLOOK").is_ok_and(|v| v == "fire")),
                    send_inputs.after(InputSet).before(MovementSet),
                    leave_with_host,
                    (pose_puppets, correct_own).after(MovementSet).before(crate::thirdperson::BodyAnimSet),
                    report.run_if(bevy::time::common_conditions::on_timer(std::time::Duration::from_secs(2))),
                )
                    .run_if(crate::state::in_game.and_then(guesting)),
            );
    }
}

/// The online match about to start: the lobby's members, and who we are.
#[derive(Resource, Clone, Debug)]
pub struct NetStart {
    pub me: PeerId,
    pub members: Vec<Member>,
}

/// Set while this game is a guest: its own game logic stands down.
static GUEST: AtomicBool = AtomicBool::new(false);

/// Does this game decide what happens (health, deaths, score)? Everyone
/// but an online guest. A run condition.
pub fn authority() -> bool {
    !GUEST.load(Ordering::Relaxed)
}

fn hosting(net: Option<Res<NetStart>>) -> bool {
    net.is_some_and(|n| n.me == HOST)
}

fn guesting(net: Option<Res<NetStart>>) -> bool {
    net.is_some_and(|n| n.me != HOST)
}

/// A friend's soldier, on the host.
#[derive(Component, Clone, Copy, Debug)]
pub struct RemotePlayer {
    pub peer: PeerId,
}

/// Someone else's pawn, on a guest: posed from the host's snapshots.
#[derive(Component, Clone, Debug)]
pub struct Puppet {
    pub id: u16,
    /// The weapon spec it's holding ([`PawnInfo::weapon`]).
    weapon: String,
    /// The last shot count seen.
    shots: u8,
    /// Alive at some point: before then it's picking a class, unseen.
    lived: bool,
}

/// Host: what's been heard from each friend, and what's been told.
#[derive(Resource, Default)]
struct HostState {
    /// Each friend's newest controls, and the buttons of the ones before.
    inputs: HashMap<PeerId, (InputCommand, u16)>,
    /// Friends whose pawn is still to be spawned.
    waiting: Vec<Member>,
    /// What each pawn's info was when last sent.
    sent: HashMap<u16, PawnInfo>,
    next_snapshot: f32,
}

/// Guest: the host's world as it arrives.
#[derive(Resource, Default)]
struct GuestState {
    snapshots: Interpolation,
    latest: Option<Snapshot>,
    info: HashMap<u16, PawnInfo>,
    /// Where puppets are drawn, in host ticks (a little behind the newest).
    clock: Option<f32>,
    /// Our controls: the next number, and the last few sent.
    sequence: u32,
    recent: Vec<InputCommand>,
    next_send: f32,
    /// This game's pawn as the host numbers it.
    my_id: Option<u16>,
    /// Who last killed us, for "Killed by".
    killer: Option<String>,
}

/// Host ticks (60 a second), the snapshots' clock.
const TICK_RATE: f32 = 60.0;
/// Snapshots a second.
const SNAPSHOT_RATE: f32 = 20.0;
/// Puppets are drawn this far behind the newest snapshot (ticks: 100 ms).
const DELAY_TICKS: f32 = 6.0;
/// Our soldier is put where the host has it when they're this far apart.
const SNAP_DISTANCE: f32 = 1.5;

/// The class a friend starts with until they pick one.
fn default_class() -> ClassLoadout {
    let gun = |spec: &str| Gun { spec: spec.into(), camo: 0, name: spec.to_ascii_uppercase(), variant: None };
    ClassLoadout {
        name: "Online".into(),
        guns: vec![gun("m4"), gun("beretta")],
        perks: Vec::new(),
        special: None,
        inventory: None,
        inventory_camo: 0,
    }
}

fn begin(mut commands: Commands, net: Option<Res<NetStart>>) {
    let Some(net) = net else {
        GUEST.store(false, Ordering::Relaxed);
        return;
    };
    let guest = net.me != HOST;
    GUEST.store(guest, Ordering::Relaxed);
    if guest {
        info!("netplay: joining the host's match as player {}", net.me);
        commands.insert_resource(GuestState::default());
    } else {
        info!("netplay: hosting {} friends", net.members.len().saturating_sub(1));
        commands.insert_resource(HostState { waiting: net.members.iter().filter(|m| m.peer != HOST).cloned().collect(), ..default() });
    }
}

fn end(mut commands: Commands) {
    GUEST.store(false, Ordering::Relaxed);
    commands.remove_resource::<NetStart>();
    commands.remove_resource::<HostState>();
    commands.remove_resource::<GuestState>();
}

fn team_of(t: u8) -> Team {
    if t == 0 { Team::Allies } else { Team::Axis }
}

fn side_of(t: Team) -> u8 {
    (t == Team::Axis) as u8
}

/// A class from the network, kept to names this game can load.
fn class_from(c: &Class, content: &crate::content::Content) -> Option<ClassLoadout> {
    let guns: Vec<Gun> = c
        .guns
        .iter()
        .filter(|(spec, _)| crate::loadout::bot_weapon(content, spec).is_some())
        .map(|(spec, camo)| Gun { spec: spec.clone(), camo: *camo as usize, name: spec.to_ascii_uppercase(), variant: None })
        .collect();
    let known = |s: &String| s.starts_with("specialty_") || s.ends_with("_mp") || s.ends_with("_grenade");
    (!guns.is_empty()).then(|| ClassLoadout {
        name: "Online".into(),
        guns,
        perks: c.perks.iter().filter(|p| p.starts_with("specialty_")).cloned().collect(),
        special: c.special.clone().filter(known),
        inventory: c.inventory.clone().filter(known),
        inventory_camo: 0,
    })
}

fn class_to(c: &ClassLoadout) -> Class {
    let asset = |s: &str| s.len() <= 48 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_:+".contains(&b));
    Class {
        guns: c.guns.iter().take(2).filter(|g| asset(&g.spec)).map(|g| (g.spec.clone(), (g.camo & 0xFFFF).min(255) as u8)).collect(),
        perks: c.perks.iter().take(3).filter(|p| asset(p)).cloned().collect(),
        special: c.special.clone().filter(|s| asset(s)),
        inventory: c.inventory.clone().filter(|s| asset(s)),
    }
}

/// Where everyone is, now and then (debug).
fn report(pawns: Query<(&Pawn, &Transform, Has<Dead>, Has<LocalPlayer>, Has<RemotePlayer>, Has<Puppet>, &Health, &WeaponState, Option<&WeaponInput>)>) {
    let line: Vec<String> = pawns
        .iter()
        .map(|(p, tf, dead, local, remote, puppet, health, weapon, wi)| {
            let kind = if local { "me" } else if remote { "friend" } else if puppet { "puppet" } else { "bot" };
            let t = tf.translation;
            format!(
                "{} ({kind}{}) {:.1},{:.1},{:.1} hp {:.0} {} clip {} fire {}",
                p.name,
                if dead { ", dead" } else { "" },
                t.x,
                t.y,
                t.z,
                health.current,
                weapon.def.name,
                weapon.clip,
                wi.is_some_and(|w| w.fire)
            )
        })
        .collect();
    debug!("netplay: {}", line.join("; "));
}

/// Debug (`COD4RW_NETLOOK`): the host looks at its first friend, for
/// screenshots of them; `COD4RW_NETLOOK=bring` first stands the friend
/// 8 m in front of the host, once they're up; `=kill` also kills them
/// (as if shot by the host) 3 s after.
fn look_at_friend(
    time: Res<Time>,
    mut brought: Local<Option<f32>>,
    mut friends: Query<(Entity, &mut Transform), (With<RemotePlayer>, Without<Dead>)>,
    mut me: Query<(Entity, &Transform, &mut ViewAngles), (With<LocalPlayer>, Without<RemotePlayer>, Without<Dead>)>,
    mut damage: MessageWriter<crate::combat::Damage>,
) {
    let (Some((friend_e, mut friend)), Ok((me_e, tf, mut view))) = (friends.iter_mut().next(), me.single_mut()) else { return };
    let mode = std::env::var("COD4RW_NETLOOK").unwrap_or_default();
    let now = time.elapsed_secs();
    if brought.is_none() && (mode == "bring" || mode == "kill") {
        *brought = Some(now);
        let ahead = Quat::from_rotation_y(view.yaw) * Vec3::NEG_Z;
        friend.translation = tf.translation + ahead * 8.0;
    }
    if mode == "kill" && brought.is_some_and(|t| now > t + 3.0) {
        *brought = Some(f32::INFINITY);
        info!("netplay: debug: killing the friend");
        damage.write(crate::combat::Damage {
            target: friend_e,
            attacker: Some(me_e),
            amount: 500.0,
            location: crate::combat::HitLocation::Torso,
            weapon: "m16_mp",
        });
    }
    let d = friend.translation - tf.translation;
    view.yaw = (-d.x).atan2(-d.z);
    view.pitch = -0.08;
}

// ---- Host ----

/// Friends' controls and classes; friends who left.
fn receive_host(
    mut commands: Commands,
    mut online: ResMut<Online>,
    mut state: ResMut<HostState>,
    content: Res<crate::content::Content>,
    time: Res<Time>,
    mut remotes: Query<(Entity, &RemotePlayer, Option<&mut Dead>)>,
    mut pawns: Query<&mut Pawn>,
) {
    let mut switch = Vec::new();
    for event in online.take_game() {
        match event {
            Event::Inputs(peer, commands) => {
                for c in commands {
                    let entry = state.inputs.entry(peer).or_insert((c, 0));
                    if cod4rw_multiplayer::security::newer(c.sequence, entry.0.sequence) {
                        // Buttons pressed between the newest two count.
                        entry.1 |= entry.0.buttons;
                        entry.0 = c;
                    }
                }
            }
            Event::Message(peer, LobbyMsg::Class(c)) => {
                let Some(class) = class_from(&c, &content) else { continue };
                if let Some((e, _, dead)) = remotes.iter_mut().find(|(_, r, _)| r.peer == peer) {
                    commands.entity(e).insert(PawnClass(class));
                    // Waiting for a class to spawn: spawn now.
                    let locked = pawns.get(e).is_ok_and(|p| crate::modes::respawn_locked(p.team));
                    if let Some(mut dead) = dead
                        && dead.respawn_at == f32::INFINITY
                        && !locked
                    {
                        dead.respawn_at = time.elapsed_secs();
                    }
                }
            }
            // A friend changed sides from the in-game menu: their soldier
            // goes over, waiting for their class for the new side.
            Event::Message(peer, LobbyMsg::SwitchTeam) => {
                for (e, r, _) in &remotes {
                    if r.peer == peer {
                        switch.push(e);
                    }
                }
            }
            Event::PeerLeft(peer) => {
                state.waiting.retain(|m| m.peer != peer);
                state.inputs.remove(&peer);
                for (e, r, _) in &remotes {
                    if r.peer == peer {
                        commands.entity(e).despawn();
                    }
                }
            }
            _ => {}
        }
    }
    for e in switch {
        if let Ok(mut pawn) = pawns.get_mut(e) {
            pawn.team = if pawn.team == crate::combat::Team::Allies { crate::combat::Team::Axis } else { crate::combat::Team::Allies };
            info!("netplay: {} switched to {:?}", pawn.name, pawn.team);
            commands.entity(e).insert((Dead { respawn_at: f32::INFINITY, killer: None }, Frozen));
        }
    }
}

/// Friends' soldiers, once the match is up: waiting for their class.
fn spawn_remotes(
    mut commands: Commands,
    mut state: ResMut<HostState>,
    assets: Option<Res<PawnAssets>>,
    map: Option<Res<crate::world::MapInfo>>,
) {
    let (Some(assets), Some(map)) = (assets, map) else { return };
    for m in std::mem::take(&mut state.waiting) {
        let team = team_of(m.team);
        let spawn = crate::combat::pick_spawn(&map, &crate::combat::Spawning::start(team, &[]));
        let pawn = spawn_pawn(&mut commands, &assets, &m.profile.name, team, &spawn);
        commands.entity(pawn).insert((
            RemotePlayer { peer: m.peer },
            WeaponInput::default(),
            PawnClass(default_class()),
            // Spawns when the friend's class arrives (or right away if it
            // came first); unseen until then, like a player picking a class.
            Dead { respawn_at: f32::INFINITY, killer: None },
            Frozen,
            Visibility::Hidden,
        ));
        info!("netplay: {} joined the match on {team:?}", m.profile.name);
    }
}

/// Each friend's soldier does what their controls say.
#[allow(clippy::type_complexity)]
fn drive_remotes(
    mut state: ResMut<HostState>,
    mut remotes: Query<
        (
            &RemotePlayer,
            &mut ViewAngles,
            &mut MoveInput,
            &mut WeaponInput,
            &WeaponState,
            Option<&mut crate::grenades::GrenadeInput>,
            Option<&mut crate::loadout::SwitchInput>,
            Option<&crate::loadout::Loadout>,
            &mut Visibility,
        ),
        Without<Dead>,
    >,
) {
    for (remote, mut view, mut mv, mut wi, weapon, grenades, switch, loadout, mut visibility) in &mut remotes {
        // Spawned: seen from now on.
        visibility.set_if_neq(Visibility::Inherited);
        let Some((c, earlier)) = state.inputs.get_mut(&remote.peer) else { continue };
        let pressed = |b: u16| c.buttons & b != 0;
        // Taps: pressed now, or in a command since the last frame.
        let tapped = |b: u16| (c.buttons | *earlier) & b != 0;
        view.yaw = c.yaw;
        view.pitch = c.pitch;
        *mv = MoveInput {
            forward: c.forward as f32 / 127.0,
            right: c.right as f32 / 127.0,
            jump: pressed(button::JUMP),
            sprint: pressed(button::SPRINT),
            stance: match c.stance {
                1 => Stance::Crouch,
                2 => Stance::Prone,
                _ => Stance::Stand,
            },
            speed_scale: weapon.speed_scale(),
            lean: 0.0,
        };
        *wi = WeaponInput { fire: pressed(button::FIRE), ads: pressed(button::AIM), reload: tapped(button::RELOAD), inspect: false };
        if let Some(mut g) = grenades {
            g.frag = tapped(button::FRAG);
            g.special = tapped(button::SPECIAL);
        }
        // The friend changed between primary and secondary.
        if let (Some(mut s), Some(l)) = (switch, loadout)
            && tapped(button::SWITCH)
            && l.current < 2
            && l.class.guns.len() > 1
        {
            s.to = Some(1 - l.current);
        }
        *earlier = 0;
    }
}

fn pawn_state(id: u16, tf: &Transform, mover: &Mover, view: &ViewAngles, health: &Health, dead: bool, weapon: &WeaponState, ads: bool) -> PawnState {
    let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    let mut flags = 0;
    if ads {
        flags |= pawn_flags::ADS;
    }
    if mover.sprinting {
        flags |= pawn_flags::SPRINT;
    }
    if mover.on_ground {
        flags |= pawn_flags::ON_GROUND;
    }
    if weapon.reload_until.is_some() {
        flags |= pawn_flags::RELOADING;
    }
    let v = mover.velocity.clamp_length_max(999.0);
    PawnState {
        id,
        position: tf.translation.to_array(),
        velocity: v.to_array(),
        yaw: wrap(view.yaw),
        pitch: view.pitch.clamp(-1.55, 1.55),
        health: health.current.round().clamp(0.0, 100.0) as u8,
        stance: mover.stance as u8,
        life: dead as u8,
        shots: weapon.shots_fired_total as u8,
        flags,
    }
}

/// The world to every friend, and pawns' details when they change.
#[allow(clippy::type_complexity)]
fn send_world(
    online: Res<Online>,
    net: Res<NetStart>,
    time: Res<Time>,
    mut state: ResMut<HostState>,
    pawns: Query<(
        &Pawn,
        &Transform,
        &Mover,
        &ViewAngles,
        &Health,
        Has<Dead>,
        &WeaponState,
        Option<&WeaponInput>,
        Option<&RemotePlayer>,
        Has<LocalPlayer>,
        Option<&crate::loadout::Loadout>,
    )>,
) {
    let now = time.elapsed_secs();
    // Details first, so a new pawn's name arrives with it.
    let mut seen = Vec::new();
    for (pawn, _, _, _, _, _, weapon, _, remote, local, loadout) in &pawns {
        let id = pawn.id as u16;
        seen.push(id);
        let info = PawnInfo {
            id,
            peer: remote.map_or(if local { HOST } else { BOT }, |r| r.peer),
            name: crate::online::lobby::clean_name(&pawn.name),
            team: side_of(pawn.team),
            // The gun's spec (what the guest loads it by), or the
            // equipment's name.
            weapon: loadout
                .and_then(|l| l.class.guns.get(l.current))
                .map_or(weapon.def.name.as_str(), |g| g.spec.as_str())
                .chars()
                .take(48)
                .filter(|c| c.is_ascii_alphanumeric() || "_:+".contains(*c))
                .collect(),
            kills: pawn.kills.min(u16::MAX as u32) as u16,
            deaths: pawn.deaths.min(u16::MAX as u32) as u16,
        };
        if state.sent.get(&id) != Some(&info) {
            online.send(EVERYONE, &LobbyMsg::Pawn(info.clone()));
            state.sent.insert(id, info);
        }
    }
    let gone: Vec<u16> = state.sent.keys().filter(|id| !seen.contains(id)).copied().collect();
    for id in gone {
        online.send(EVERYONE, &LobbyMsg::PawnGone(id));
        state.sent.remove(&id);
    }

    if now < state.next_snapshot {
        return;
    }
    state.next_snapshot = now + 1.0 / SNAPSHOT_RATE;
    let tick = (now * TICK_RATE) as u32;
    let all: Vec<PawnState> = pawns
        .iter()
        .take(cod4rw_multiplayer::protocol::MAX_PLAYERS)
        .map(|(pawn, tf, mover, view, health, dead, weapon, wi, _, _, _)| {
            pawn_state(pawn.id as u16, tf, mover, view, health, dead, weapon, wi.is_some_and(|w| w.ads))
        })
        .filter(|p| p.validate().is_ok())
        .collect();
    for m in net.members.iter().filter(|m| m.peer != HOST) {
        let ack = state.inputs.get(&m.peer).map_or(0, |(c, _)| c.sequence);
        online.send_snapshot(m.peer, Snapshot { tick, acknowledged_input: ack, pawns: all.clone() });
    }
}

// ---- Guest ----

/// Snapshots and pawns' details from the host.
fn receive_guest(
    mut commands: Commands,
    mut online: ResMut<Online>,
    net: Res<NetStart>,
    mut state: ResMut<GuestState>,
    puppets: Query<(Entity, &Puppet)>,
    (time, mut match_state, everyone): (Res<Time>, Option<ResMut<crate::tdm::MatchState>>, Query<Entity, With<Pawn>>),
    (mine, mut killed): (Query<Entity, With<LocalPlayer>>, MessageWriter<crate::combat::Killed>),
    (mut pending, mut launches): (ResMut<crate::grenades::Pending>, MessageWriter<crate::explosives::Launch>),
) {
    // Another pawn's puppet, by the host's ID (not ours: we threw it).
    let puppet_of = |id: u16, puppets: &Query<(Entity, &Puppet)>| puppets.iter().find(|(_, p)| p.id == id).map(|(e, _)| e);
    for event in online.take_game() {
        match event {
            Event::Snapshot(s) => {
                if state.latest.as_ref().is_none_or(|l| cod4rw_multiplayer::security::newer(s.tick, l.tick)) {
                    let _ = state.snapshots.push(s.clone());
                    state.latest = Some(s);
                }
            }
            Event::Message(_, LobbyMsg::Pawn(info)) => {
                if info.peer == net.me {
                    state.my_id = Some(info.id);
                }
                state.info.insert(info.id, info);
            }
            Event::Message(_, LobbyMsg::MatchOver(winner)) => {
                if let Some(state) = match_state.as_deref_mut()
                    && state.ended.is_none()
                {
                    let winner = match winner {
                        0 => Some(crate::combat::Team::Allies),
                        1 => Some(crate::combat::Team::Axis),
                        _ => None,
                    };
                    state.ended = Some((winner, time.elapsed_secs()));
                    for e in &everyone {
                        commands.entity(e).insert(Frozen);
                    }
                }
            }
            Event::Message(_, LobbyMsg::Kill { victim, attacker, weapon, location }) => {
                // The pawns as this game has them: puppets, and our own.
                let my_id = state.my_id;
                let entity = |id: u16| {
                    if Some(id) == my_id {
                        mine.single().ok()
                    } else {
                        puppets.iter().find(|(_, p)| p.id == id).map(|(e, _)| e)
                    }
                };
                let Some(victim_e) = entity(victim) else { continue };
                if Some(victim) == state.my_id {
                    state.killer = state.info.get(&attacker).filter(|_| attacker != victim).map(|i| i.name.clone());
                }
                killed.write(crate::combat::Killed {
                    victim: victim_e,
                    attacker: if attacker == BOT { None } else { entity(attacker) },
                    weapon: intern(&weapon),
                    location: match location {
                        0 => crate::combat::HitLocation::Head,
                        1 => crate::combat::HitLocation::Neck,
                        3 => crate::combat::HitLocation::Legs,
                        4 => crate::combat::HitLocation::TorsoLower,
                        _ => crate::combat::HitLocation::Torso,
                    },
                });
            }
            Event::Message(_, LobbyMsg::Scores { allies, axis }) => {
                if let Some(s) = match_state.as_deref_mut() {
                    (s.allies, s.axis) = (allies, axis);
                }
            }
            // Others' grenades and launches, ours to see and hear (our own
            // our game threw already; the damage is the host's).
            Event::Message(_, LobbyMsg::Throw { thrower, kind, at, velocity, fuse }) => {
                let (Some(kind), Some(by)) = (crate::grenades::Kind::from_index(kind), puppet_of(thrower, &puppets)) else { continue };
                pending.queue_throw(kind, by, Vec3::from_array(at), Vec3::from_array(velocity), time.elapsed_secs() + fuse.clamp(0.0, 60.0));
                debug!("netplay: a {kind:?} from {thrower}");
            }
            Event::Message(_, LobbyMsg::Launch { shooter, weapon, from, dir, yaw }) => {
                let Some(by) = puppet_of(shooter, &puppets) else { continue };
                launches.write(crate::explosives::Launch { shooter: by, weapon, from: Vec3::from_array(from), dir: Vec3::from_array(dir), yaw });
            }
            Event::Message(_, LobbyMsg::PawnGone(id)) => {
                state.info.remove(&id);
                for (e, p) in &puppets {
                    if p.id == id {
                        commands.entity(e).despawn();
                    }
                }
            }
            _ => {}
        }
    }
}

/// Debug (`COD4RW_NETLOOK=fire`): a guest aims at the first puppet in
/// sight range and holds the trigger, to test shots through the host.
fn aim_at_host(
    puppets: Query<&Transform, (With<Puppet>, Without<Dead>)>,
    mut me: Query<(&Transform, &mut ViewAngles, &mut WeaponInput), (With<LocalPlayer>, Without<Puppet>, Without<Dead>)>,
) {
    let Ok((tf, mut view, mut wi)) = me.single_mut() else { return };
    let Some(target) = puppets.iter().min_by(|a, b| a.translation.distance(tf.translation).total_cmp(&b.translation.distance(tf.translation))) else { return };
    let d = target.translation + Vec3::Y * 1.2 - (tf.translation + Vec3::Y * 1.5);
    view.yaw = (-d.x).atan2(-d.z);
    view.pitch = (d.y / d.xz().length().max(0.1)).atan();
    // Pulled and let go, for burst and single-shot guns.
    wi.fire = d.length() < 12.0 && (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_millis()) / 150) % 2 == 0;
}

/// A weapon name as the kill messages keep it (`&'static`): each distinct
/// name is kept once.
fn intern(name: &str) -> &'static str {
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(n) = names.iter().find(|n| **n == name) {
        return n;
    }
    let n: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.push(n);
    n
}

/// Host: every kill, for friends' kill feeds, ragdolls, killcams and XP.
fn send_kills(online: Res<Online>, mut killed: MessageReader<crate::combat::Killed>, pawns: Query<&Pawn>) {
    for k in killed.read() {
        let Ok(victim) = pawns.get(k.victim) else { continue };
        let attacker = k.attacker.and_then(|a| pawns.get(a).ok()).map_or(BOT, |p| p.id as u16);
        let weapon: String = k.weapon.chars().take(48).filter(|c| c.is_ascii_alphanumeric() || "_:+".contains(*c)).collect();
        let location = match k.location {
            crate::combat::HitLocation::Head => 0,
            crate::combat::HitLocation::Neck => 1,
            crate::combat::HitLocation::Torso => 2,
            crate::combat::HitLocation::Legs => 3,
            crate::combat::HitLocation::TorsoLower => 4,
        };
        online.send(EVERYONE, &LobbyMsg::Kill { victim: victim.id as u16, attacker, weapon, location });
    }
}

/// Host: the teams' scores when they change (a guest's own game doesn't
/// keep score).
fn send_scores(online: Res<Online>, state: Option<Res<crate::tdm::MatchState>>, mut sent: Local<Option<(u32, u32)>>) {
    let Some(s) = state else { return };
    if *sent != Some((s.allies, s.axis)) {
        *sent = Some((s.allies, s.axis));
        online.send(EVERYONE, &LobbyMsg::Scores { allies: s.allies, axis: s.axis });
    }
}

/// Host: grenades thrown and explosives launched, for guests to see.
fn send_throws(
    online: Res<Online>,
    time: Res<Time>,
    grenades: Query<(&crate::grenades::LiveGrenade, &Transform), Added<crate::grenades::LiveGrenade>>,
    mut launches: MessageReader<crate::explosives::Launch>,
    pawns: Query<&Pawn>,
) {
    let id = |e: Entity| pawns.get(e).map_or(BOT, |p| p.id as u16);
    let now = time.elapsed_secs();
    for (g, tf) in &grenades {
        online.send(
            EVERYONE,
            &LobbyMsg::Throw {
                thrower: id(g.thrower),
                kind: g.kind.index(),
                at: tf.translation.to_array(),
                velocity: g.velocity.to_array(),
                fuse: (g.explode_at - now).clamp(0.0, 60.0),
            },
        );
    }
    for l in launches.read() {
        let weapon: String = l.weapon.chars().take(48).filter(|c| c.is_ascii_alphanumeric() || "_:+".contains(*c)).collect();
        online.send(EVERYONE, &LobbyMsg::Launch { shooter: id(l.shooter), weapon, from: l.from.to_array(), dir: l.dir.to_array(), yaw: l.yaw });
    }
}

/// Host: the match is over, for everyone.
fn announce_end(online: Res<Online>, state: Option<Res<crate::tdm::MatchState>>, mut sent: Local<bool>) {
    let ended = state.and_then(|s| s.ended);
    match ended {
        Some((winner, _)) if !*sent => {
            *sent = true;
            let code = match winner {
                Some(crate::combat::Team::Allies) => 0,
                Some(crate::combat::Team::Axis) => 1,
                None => 2,
            };
            online.send(EVERYONE, &LobbyMsg::MatchOver(code));
        }
        None => *sent = false,
        _ => {}
    }
}

/// Guest: back to the lobby with the host, after the results, or at once
/// if the host is gone.
fn leave_with_host(
    mut commands: Commands,
    time: Res<Time>,
    online: Res<Online>,
    state: Option<Res<crate::tdm::MatchState>>,
    over: Option<Res<crate::tdm::MatchOver>>,
    mut left: Local<bool>,
) {
    let shown = state.and_then(|s| s.ended).is_some_and(|(_, at)| time.elapsed_secs() - at > crate::tdm::POST_MATCH);
    let gone = !matches!(online.status, crate::online::Status::Connected(_));
    if !(shown || gone) {
        *left = false;
        return;
    }
    // (Inserted at the end of the frame: once.)
    if over.is_none() && !*left {
        info!("netplay: {}", if gone { "the host left; back to the menus" } else { "match over" });
        commands.insert_resource(crate::tdm::MatchOver);
        *left = true;
    }
}

/// Our class, to the host, when it's picked.
fn send_class(online: Res<Online>, mine: Query<&PawnClass, (With<LocalPlayer>, Changed<PawnClass>)>) {
    for class in &mine {
        online.send(HOST, &LobbyMsg::Class(class_to(&class.0)));
    }
}

/// Our controls to the host, up to 60 times a second, each with the few
/// before it in case some go missing.
#[allow(clippy::type_complexity)]
fn send_inputs(
    online: Res<Online>,
    time: Res<Time>,
    mut state: ResMut<GuestState>,
    me: Query<
        (&MoveInput, &ViewAngles, &WeaponInput, Option<&crate::grenades::GrenadeInput>, Option<&crate::loadout::Loadout>),
        With<LocalPlayer>,
    >,
    mut held: Local<u16>,
    mut in_hand: Local<Option<usize>>,
) {
    let Ok((mv, view, wi, grenades, loadout)) = me.single() else { return };
    let mut buttons = 0;
    let mut set = |on: bool, b: u16| {
        if on {
            buttons |= b;
        }
    };
    set(wi.fire, button::FIRE);
    set(wi.ads, button::AIM);
    set(mv.jump, button::JUMP);
    set(mv.sprint, button::SPRINT);
    set(wi.reload, button::RELOAD);
    set(grenades.is_some_and(|g| g.frag), button::FRAG);
    set(grenades.is_some_and(|g| g.special), button::SPECIAL);
    // Changed between primary and secondary: the host follows.
    let current = loadout.map(|l| l.current);
    set(in_hand.is_some() && current != *in_hand && current.is_some_and(|c| c < 2), button::SWITCH);
    *in_hand = current;
    // Taps between sends still count.
    *held |= buttons;
    let now = time.elapsed_secs();
    if now < state.next_send {
        return;
    }
    state.next_send = now + 1.0 / TICK_RATE;
    let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    state.sequence = state.sequence.wrapping_add(1).max(1);
    let axis = |v: f32| (v.clamp(-1.0, 1.0) * 127.0).round() as i8;
    let command = InputCommand {
        sequence: state.sequence,
        forward: axis(mv.forward),
        right: axis(mv.right),
        yaw: wrap(view.yaw),
        pitch: view.pitch.clamp(-1.55, 1.55),
        buttons: std::mem::take(&mut *held),
        stance: mv.stance as u8,
    };
    state.recent.push(command);
    let excess = state.recent.len().saturating_sub(INPUT_REDUNDANCY);
    state.recent.drain(..excess);
    online.send_inputs(state.recent.iter().rev().copied().collect());
}

/// Everyone else, posed from the snapshots.
#[allow(clippy::type_complexity)]
fn pose_puppets(
    mut commands: Commands,
    time: Res<Time>,
    mut state: ResMut<GuestState>,
    assets: Option<Res<PawnAssets>>,
    content: Res<crate::content::Content>,
    mut puppets: Query<(Entity, &mut Puppet, &mut Pawn, &mut Transform, &mut Mover, &mut ViewAngles, &mut Health, &mut WeaponState, &mut WeaponInput, &mut Visibility, Has<Dead>)>,
    mut shots: MessageWriter<ShotFired>,
) {
    let Some(latest) = state.latest.as_ref().map(|s| s.tick as f32) else { return };
    // The puppets' clock: steady, a little behind the newest snapshot.
    let target = latest - DELAY_TICKS;
    let clock = match state.clock {
        Some(c) if (target - c).abs() < 12.0 => {
            let c = c + time.delta_secs() * TICK_RATE;
            c + (target - c) * 0.05
        }
        _ => target,
    };
    state.clock = Some(clock);
    let (tick, fraction) = (clock.floor().max(0.0) as u32, clock.fract());

    let ids: Vec<u16> = state.latest.as_ref().map_or(Vec::new(), |s| s.pawns.iter().map(|p| p.id).collect());
    let mut posed = Vec::new();
    for (e, mut puppet, mut pawn, mut tf, mut mover, mut view, mut health, mut weapon, mut wi, mut visibility, dead) in &mut puppets {
        posed.push(puppet.id);
        let Some(p) = state.snapshots.sample(puppet.id, tick, fraction) else { continue };
        if p.life == 0 && !puppet.lived {
            puppet.lived = true;
            *visibility = Visibility::Inherited;
        }
        if let Some(info) = state.info.get(&puppet.id) {
            pawn.name.clone_from(&info.name);
            pawn.team = team_of(info.team);
            pawn.kills = info.kills as u32;
            pawn.deaths = info.deaths as u32;
            if puppet.weapon != info.weapon {
                puppet.weapon.clone_from(&info.weapon);
                if let Some(def) = crate::loadout::bot_weapon(&content, &info.weapon) {
                    weapon.def = def;
                }
            }
        }
        tf.translation = Vec3::from_array(p.position);
        tf.rotation = Quat::from_rotation_y(p.yaw);
        mover.velocity = Vec3::from_array(p.velocity);
        mover.stance = match p.stance {
            1 => Stance::Crouch,
            2 => Stance::Prone,
            _ => Stance::Stand,
        };
        mover.sprinting = p.flags & pawn_flags::SPRINT != 0;
        mover.on_ground = p.flags & pawn_flags::ON_GROUND != 0;
        view.yaw = p.yaw;
        view.pitch = p.pitch;
        health.current = p.health as f32;
        wi.ads = p.flags & pawn_flags::ADS != 0;
        match (p.life == 1, dead) {
            (true, false) => {
                commands.entity(e).insert(Dead { respawn_at: f32::INFINITY, killer: None });
            }
            (false, true) => {
                commands.entity(e).remove::<Dead>();
            }
            _ => {}
        }
        // It fired: the third-person animation, the sound and a tracer.
        let fired = p.shots.wrapping_sub(puppet.shots);
        if fired > 0 && fired < 32 && p.life == 0 {
            weapon.shots_fired_total = weapon.shots_fired_total.wrapping_add(fired as u32);
            let eye = tf.translation + Vec3::Y * mover.eye_height.max(1.0);
            let dir = Quat::from_euler(EulerRot::YXZ, p.yaw, p.pitch, 0.0) * Vec3::NEG_Z;
            shots.write(ShotFired {
                shooter: e,
                weapon: Some(weapon.def),
                from: eye,
                to: eye + dir * 60.0,
                hit_pawn: false,
                normal: Vec3::ZERO,
                hit_world: false,
            });
        }
        puppet.shots = p.shots;
    }
    // New pawns: a puppet each (not ours).
    let Some(assets) = assets else { return };
    let my_id = state.my_id;
    for id in ids.into_iter().filter(|id| !posed.contains(id) && Some(*id) != my_id) {
        let Some(info) = state.info.get(&id) else { continue };
        let Some(p) = state.latest.as_ref().and_then(|s| s.pawns.iter().find(|p| p.id == id)).copied() else { continue };
        let spawn = crate::world::SpawnPoint { pos: Vec3::from_array(p.position), yaw: p.yaw, kind: crate::world::SpawnKind::Tdm };
        let e = spawn_pawn(&mut commands, &assets, &info.name, team_of(info.team), &spawn);
        let mut weapon = WeaponState::default();
        if let Some(def) = crate::loadout::bot_weapon(&content, &info.weapon) {
            weapon.def = def;
        }
        let lived = p.life == 0;
        let visibility = if lived { Visibility::Inherited } else { Visibility::Hidden };
        commands.entity(e).insert((Puppet { id, weapon: info.weapon.clone(), shots: p.shots, lived }, Frozen, WeaponInput::default(), weapon, visibility));
    }
}

/// Our own soldier: health, death and respawn from the host, and put where
/// the host has it if the two drift apart.
#[allow(clippy::type_complexity)]
fn correct_own(
    mut commands: Commands,
    state: Res<GuestState>,
    mut me: Query<(Entity, &mut Transform, &mut Mover, &mut ViewAngles, &mut Health, &mut Pawn, Has<Dead>), With<LocalPlayer>>,
) {
    let (Some(id), Some(latest)) = (state.my_id, state.latest.as_ref()) else { return };
    let Some(p) = latest.pawns.iter().find(|p| p.id == id) else { return };
    let Ok((e, mut tf, mut mover, mut view, mut health, mut pawn, dead)) = me.single_mut() else { return };
    health.current = p.health as f32;
    if let Some(info) = state.info.get(&id) {
        pawn.kills = info.kills as u32;
        pawn.deaths = info.deaths as u32;
        pawn.team = team_of(info.team);
    }
    let at = Vec3::from_array(p.position);
    match (p.life == 1, dead) {
        (true, false) => {
            commands.entity(e).insert((Dead { respawn_at: f32::INFINITY, killer: state.killer.clone() }, Frozen));
        }
        (false, true) => {
            // Respawned: where the host put us, facing its way.
            commands.entity(e).remove::<(Dead, Frozen, crate::loadout::AwaitingClass)>();
            tf.translation = at;
            mover.velocity = Vec3::ZERO;
            view.yaw = p.yaw;
            view.pitch = 0.0;
        }
        (false, false) if tf.translation.distance(at) > SNAP_DISTANCE => {
            tf.translation = at;
            mover.velocity = Vec3::from_array(p.velocity);
        }
        _ => {}
    }
}
