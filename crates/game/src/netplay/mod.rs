//! Playing an online match together ([`crate::online`]'s lobby started it).
//!
//! The host's game is the match: its pawns, bots, shots, damage, deaths,
//! objectives, kill streaks and score are the real ones. A friend's soldier
//! on the host is a pawn like a bot, moved by the controls the friend's game
//! sends, command by command, for as long as each was held
//! ([`RemotePlayer`], [`step_remotes`]). Twenty times a second the host
//! sends everyone where every pawn is (a snapshot) with the rest of what's
//! going on (the clock, objectives, UAVs, killstreak aircraft and sentries:
//! [`world`], [`streaks`]); when a pawn's name, team, weapon or score
//! changes, that too ([`PawnInfo`]).
//!
//! A friend's game ("guest") moves its own soldier itself, straight away,
//! as the host will (same movement code, same controls), and replays what
//! the host hasn't seen yet over each snapshot's word ([`predict`]).
//! Everyone else is a [`Puppet`]: posed from the snapshots, a little behind
//! (more on a jittery or lossy connection) so there's always a next one to
//! move towards, and carried on along their way for a moment if they stop
//! coming. The guest's game decides nothing: health, deaths, respawns,
//! objectives and kill streaks come from the host ([`authority`] turns its
//! own off). The host traces a friend's shots against everyone where the
//! friend saw them ([`lagcomp`]).
//!
//! The host also tells guests about kills (their kill feeds, ragdolls,
//! killcams and "Killed by"), team scores, grenades thrown and explosives
//! launched and set off (seen and heard; their damage is the host's),
//! rounds ending, awards, killstreaks called in, and the end of the match
//! (everyone sees the result and goes back to the lobby). Friends can join
//! a match under way, in place of a bot ([`join`]).
//!
//! Testing: `COD4RW_NETSIM=ms,loss%[,jitter]` ([`crate::online::NetSim`]),
//! `COD4RW_NETGRAPH=1` ([`graph`]).

use crate::combat::{Dead, Health, Pawn, PawnAssets, Team, spawn_pawn};
use crate::loadout::{ClassLoadout, Gun, PawnClass};
use crate::movement::{Frozen, MoveInput, Mover, Stance, ViewAngles};
use crate::online::lobby::{BOT, Class, LobbyMsg, Member, PawnInfo};
use crate::online::{EVERYONE, Event, HOST, Online};
use crate::player::LocalPlayer;
use crate::weapons::{ShotFired, WeaponInput, WeaponState};
use bevy::prelude::*;
use cod4rw_multiplayer::netcode::Interpolation;
use cod4rw_multiplayer::protocol::{INPUT_REDUNDANCY, InputCommand, PawnState, PeerId, Snapshot, button, pawn_flags};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

pub mod graph;
pub mod join;
pub mod lagcomp;
pub mod layout_sync;
pub mod mixed;
mod predict;
mod streaks;
mod wire;
mod world;

pub(crate) use predict::send_inputs;

pub struct NetplayPlugin;

impl Plugin for NetplayPlugin {
    fn build(&self, app: &mut App) {
        use crate::movement::MovementSet;
        use crate::player::InputSet;
        app.init_resource::<graph::NetStats>()
            .init_resource::<world::HostInbox>()
            .init_resource::<world::ChatEcho>()
            .init_resource::<world::GuestInbox>()
            .add_systems(OnEnter(crate::state::GameState::InGame), begin.in_set(crate::state::Setup::Spawn))
            .add_systems(OnExit(crate::state::GameState::InGame), end)
            .add_systems(
                Update,
                (
                    (receive_host, join::admit_joiners, join::refill, spawn_remotes).chain().before(InputSet),
                    drive_remotes.after(InputSet).before(MovementSet),
                    step_remotes.in_set(MovementSet),
                    lagcomp::record.after(MovementSet).before(crate::weapons::WeaponSet),
                    send_world.after(crate::weapons::WeaponSet),
                    (announce_end, send_kills, send_throws, world::host_news, world::host_requests, world::host_chat, world::test_give, streaks::host_blasts, host_pings),
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
                    predict::predict_own.in_set(MovementSet),
                    leave_with_host,
                    (world::apply, world::guest_news, world::guest_chat, world::guest_streaks, streaks::mirror).chain().after(receive_guest),
                    (pose_puppets, correct_own).after(MovementSet).before(crate::thirdperson::BodyAnimSet),
                    report.run_if(bevy::time::common_conditions::on_timer(std::time::Duration::from_secs(2))),
                )
                    .run_if(crate::state::in_game.and_then(guesting)),
            )
            .add_systems(Update, mixed::test_class.before(send_class).run_if(crate::state::in_game.and_then(|n: Option<Res<NetStart>>| n.is_some())))
            .add_systems(Update, (graph::count_hits.after(crate::weapons::WeaponSet), graph::show).run_if(crate::state::in_game.and_then(|n: Option<Res<NetStart>>| n.is_some())));
    }
}

/// The online match about to start: the lobby's members, and who we are.
#[derive(Resource, Clone, Debug)]
pub struct NetStart {
    pub me: PeerId,
    pub members: Vec<Member>,
}

/// Players' pings for the scoreboard, by pawn ID ([`BOT_PING`] for bots):
/// the host's friends as their games measure it, the host 0, kept by the
/// host and sent to guests in the snapshots.
static PINGS: std::sync::Mutex<Vec<(u32, u16)>> = std::sync::Mutex::new(Vec::new());
const BOT_PING: u16 = u16::MAX;

/// The scoreboard's Ping for a pawn in an online match ("BOT" for a bot),
/// or `None` offline.
pub fn ping_text(pawn: u32) -> Option<String> {
    let pings = PINGS.lock().ok()?;
    let &(_, ms) = pings.iter().find(|(id, _)| *id == pawn)?;
    Some(if ms == BOT_PING { "BOT".into() } else { ms.to_string() })
}

/// Is there a ping to show (an online match)?
pub fn has_pings() -> bool {
    PINGS.lock().is_ok_and(|p| !p.is_empty())
}

fn set_pings(list: Vec<(u32, u16)>) {
    if let Ok(mut p) = PINGS.lock() {
        *p = list;
    }
}

/// Host: everyone's ping, once a second.
fn host_pings(time: Res<Time>, mut next: Local<f32>, stats: Res<graph::NetStats>, pawns: Query<(&Pawn, Option<&RemotePlayer>, Has<crate::bots::Bot>)>) {
    if time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 1.0;
    set_pings(
        pawns
            .iter()
            .map(|(p, remote, bot)| (p.id, if bot { BOT_PING } else { remote.and_then(|r| stats.peer_ping.get(&r.peer).copied()).unwrap_or(0).min(9999) }))
            .collect(),
    );
}

/// Set while this game is a guest: its own game logic stands down.
static GUEST: AtomicBool = AtomicBool::new(false);

/// Does this game decide what happens (health, deaths, score, objectives,
/// kill streaks)? Everyone but an online guest. A run condition.
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

impl Puppet {
    /// The weapon spec it's holding.
    pub fn weapon(&self) -> &str {
        &self.weapon
    }
}

/// Host: one friend's connection.
#[derive(Default)]
pub(crate) struct Peer {
    /// Commands to run, oldest first; the newest heard (for the gun,
    /// grenades...), and buttons tapped since the last frame.
    queue: VecDeque<InputCommand>,
    newest: Option<InputCommand>,
    taps: u16,
    /// Movement time the friend may still use (ms): it can't run its
    /// soldier faster than the clock.
    budget: f32,
    /// The last command run, and when.
    applied: u32,
    applied_at: f32,
    /// Commands heard and missed (net graph).
    heard: u32,
    missed: u32,
    rate_from: f32,
    /// Snapshots sent them.
    snapshots: u16,
}

/// Host: what's been heard from each friend, and what's been told.
#[derive(Resource, Default)]
pub(crate) struct HostState {
    peers: HashMap<PeerId, Peer>,
    /// Friends whose pawn is still to be spawned.
    waiting: Vec<Member>,
    /// Friends in the match (snapshots go to them).
    members: Vec<Member>,
    /// What each pawn's info was when last sent.
    sent: HashMap<u16, PawnInfo>,
    /// Sides to give a bot (a friend left).
    refill: Vec<Team>,
    next_snapshot: f32,
    /// How many pawns were in cover in the last snapshot (for the log).
    covered_logged: usize,
}

/// Guest: the host's world as it arrives.
#[derive(Resource, Default)]
pub(crate) struct GuestState {
    snapshots: Interpolation,
    latest: Option<Snapshot>,
    info: HashMap<u16, PawnInfo>,
    /// Where puppets are drawn, in host ticks (a little behind the newest).
    clock: Option<f32>,
    /// How far behind (ticks): more on a jittery or lossy connection.
    delay: f32,
    /// Ticks between snapshots (smoothed).
    interval: f32,
    /// This game's pawn as the host numbers it.
    my_id: Option<u16>,
    /// Who last killed us, for "Killed by".
    killer: Option<String>,
    /// Snapshots: when the last came (s), its sequence, the jitter (s),
    /// arrivals in the last second, and recent losses.
    arrived_at: Option<f32>,
    sequence: Option<u16>,
    jitter: f32,
    arrivals: VecDeque<f32>,
    lost: f32,
    next_ping: f32,
}

/// Host ticks (60 a second), the snapshots' clock.
const TICK_RATE: f32 = 60.0;
/// Snapshots a second (`net.snapshot_hz` in tuning.txt, 10 to 60).
fn snapshot_rate() -> f32 {
    // (`COD4RW_NETTEST_SNAPHZ` for tests: the old 20, to compare.)
    let test = std::env::var("COD4RW_NETTEST_SNAPHZ").ok().and_then(|v| v.parse::<f32>().ok());
    test.unwrap_or_else(|| crate::tune::get("net.snapshot_hz", 40.0)).clamp(10.0, 60.0)
}
/// Puppets are drawn about two snapshots behind the newest, plus the
/// jitter, plus more while snapshots go missing ([`GuestState::delay`]):
/// at least this far (ticks: 33 ms)...
const MIN_DELAY_TICKS: f32 = 2.0;
/// ...and at most this far (300 ms).
const MAX_DELAY_TICKS: f32 = 18.0;
/// Puppets carry on along their way for at most this long (ticks) when
/// snapshots stop coming.
const EXTRAPOLATE_TICKS: f32 = 9.0;

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
        join::accept(&mut commands, false);
        return;
    };
    let guest = net.me != HOST;
    GUEST.store(guest, Ordering::Relaxed);
    join::accept(&mut commands, !guest);
    commands.insert_resource(graph::NetStats::default());
    if guest {
        info!("netplay: joining the host's match as player {}", net.me);
        commands.insert_resource(GuestState { delay: 6.0, interval: 1.5, ..default() });
        commands.insert_resource(predict::Prediction::default());
    } else {
        info!("netplay: hosting {} friends", net.members.len().saturating_sub(1));
        let friends: Vec<Member> = net.members.iter().filter(|m| m.peer != HOST).cloned().collect();
        // Test aid: `COD4RW_NETTEST_BOTS=<n>` adds n bots a side.
        let bots = std::env::var("COD4RW_NETTEST_BOTS").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(8);
        let refill = [Team::Allies, Team::Axis].into_iter().flat_map(|t| std::iter::repeat_n(t, bots)).collect();
        commands.insert_resource(HostState { waiting: friends.clone(), members: friends, refill, ..default() });
    }
}

fn end(mut commands: Commands) {
    GUEST.store(false, Ordering::Relaxed);
    set_pings(Vec::new());
    // (The host's map editor layout was for that match.)
    layout_sync::clear();
    join::accept(&mut commands, false);
    commands.remove_resource::<NetStart>();
    commands.remove_resource::<HostState>();
    commands.remove_resource::<GuestState>();
    commands.remove_resource::<predict::Prediction>();
}

fn team_of(t: u8) -> Team {
    if t == 0 { Team::Allies } else { Team::Axis }
}

fn side_of(t: Team) -> u8 {
    (t == Team::Axis) as u8
}

/// A class from the network, kept to guns this game can load: one it
/// can't (a game it hasn't got) is the closest CoD4 gun ([`mixed`]; the
/// friend's game swaps it first, so this is a fallback).
fn class_from(c: &Class, content: &crate::content::Content) -> Option<ClassLoadout> {
    let guns: Vec<Gun> = c
        .guns
        .iter()
        .map(|(spec, camo)| {
            let here = mixed::loadable(content, spec);
            if here != *spec {
                warn!("netplay: a friend's {spec} can't run here: {here} instead");
            }
            let camo = if here == *spec { *camo as usize } else { 0 };
            Gun { name: here.to_ascii_uppercase(), spec: here, camo, variant: None }
        })
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

/// A weapon or asset name as the protocol carries it.
fn asset_name(s: &str) -> String {
    s.chars().take(48).filter(|c| c.is_ascii_alphanumeric() || "_:+".contains(*c)).collect()
}

// ---- Debug ----

/// Where everyone is, now and then (debug).
#[allow(clippy::type_complexity)]
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

/// Friends' controls, classes and requests; friends who left.
#[allow(clippy::too_many_arguments)]
fn receive_host(
    mut commands: Commands,
    mut online: ResMut<Online>,
    mut state: ResMut<HostState>,
    content: Res<crate::content::Content>,
    time: Res<Time>,
    mut remotes: Query<(Entity, &RemotePlayer, Option<&mut Dead>)>,
    mut pawns: Query<&mut Pawn>,
    mut inbox: ResMut<world::HostInbox>,
    mut stats: ResMut<graph::NetStats>,
) {
    let now = time.elapsed_secs();
    let mut switch = Vec::new();
    for event in online.take_game() {
        match event {
            Event::Inputs(peer, list) => {
                let Some(p) = state.peers.get_mut(&peer) else { continue };
                for c in list {
                    // Not run yet and not already waiting (a late one comes
                    // in in its place).
                    if p.applied != 0 && !cod4rw_multiplayer::security::newer(c.sequence, p.applied) || p.queue.iter().any(|q| q.sequence == c.sequence) {
                        continue;
                    }
                    p.heard += 1;
                    p.taps |= c.buttons;
                    if p.newest.is_none_or(|n| cod4rw_multiplayer::security::newer(c.sequence, n.sequence)) {
                        p.newest = Some(c);
                    }
                    let at = p.queue.iter().position(|q| cod4rw_multiplayer::security::newer(q.sequence, c.sequence)).unwrap_or(p.queue.len());
                    p.queue.insert(at, c);
                    // A friend far behind: the oldest go.
                    while p.queue.len() > 30 {
                        p.queue.pop_front();
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
                        dead.respawn_at = now;
                    }
                }
            }
            Event::Message(peer, LobbyMsg::Game(data)) => match wire::GameMsg::decode(&data) {
                Ok(m) => inbox.0.push((peer, m)),
                Err(e) => debug!("netplay: a bad message from player {peer}: {e}"),
            },
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
                state.members.retain(|m| m.peer != peer);
                state.peers.remove(&peer);
                stats.peer_ping.remove(&peer);
                stats.peer_rate.remove(&peer);
                stats.peer_loss.remove(&peer);
                for (e, r, _) in &remotes {
                    if r.peer == peer {
                        if let Ok(p) = pawns.get(e) {
                            info!("netplay: {} left; a bot takes their place", p.name);
                            state.refill.push(p.team);
                        }
                        commands.entity(e).despawn();
                    }
                }
            }
            _ => {}
        }
    }
    for e in switch {
        if let Ok(mut pawn) = pawns.get_mut(e) {
            pawn.team = if pawn.team == Team::Allies { Team::Axis } else { Team::Allies };
            info!("netplay: {} switched to {:?}", pawn.name, pawn.team);
            commands.entity(e).insert((Dead { respawn_at: f32::INFINITY, killer: None }, Frozen));
        }
    }
    // The net graph: commands a second and the share lost, per friend.
    for (peer, p) in state.peers.iter_mut() {
        if now - p.rate_from >= 1.0 {
            let span = now - p.rate_from;
            stats.peer_rate.insert(*peer, p.heard as f32 / span);
            let total = (p.heard + p.missed).max(1);
            stats.peer_loss.insert(*peer, p.missed as f32 / total as f32);
            (p.heard, p.missed, p.rate_from) = (0, 0, now);
        }
    }
}

/// Friends' soldiers, once the match is up: waiting for their class.
fn spawn_remotes(
    mut commands: Commands,
    mut state: ResMut<HostState>,
    time: Res<Time>,
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
            crate::cover::CoverTap::default(),
            WeaponInput::default(),
            PawnClass(default_class()),
            crate::movement::NetDriven,
            lagcomp::Rewind { tick: lagcomp::now_tick(&time) },
            // Spawns when the friend's class arrives (or right away if it
            // came first); unseen until then, like a player picking a class.
            Dead { respawn_at: f32::INFINITY, killer: None },
            Frozen,
            Visibility::Hidden,
        ));
        state.peers.insert(m.peer, Peer { rate_from: time.elapsed_secs(), ..default() });
        info!("netplay: {} joined the match on {team:?}", m.profile.name);
    }
}

/// Each friend's gun, grenades and buttons do what their newest controls
/// say (their movement is [`step_remotes`]').
#[allow(clippy::type_complexity)]
pub(crate) fn drive_remotes(
    time: Res<Time>,
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
            &mut crate::cover::CoverTap,
            &mut lagcomp::Rewind,
            Option<&mut crate::modes::UseObjective>,
            Option<&mut crate::killstreaks::carepackage::UseCrate>,
        ),
        Without<Dead>,
    >,
) {
    let now = lagcomp::now_tick(&time);
    for (remote, mut view, mut mv, mut wi, weapon, grenades, switch, loadout, mut visibility, mut cover_tap, mut rewind, use_objective, use_crate) in &mut remotes {
        // Spawned: seen from now on.
        visibility.set_if_neq(Visibility::Inherited);
        let Some(peer) = state.peers.get_mut(&remote.peer) else { continue };
        let Some(c) = peer.newest else { continue };
        let earlier = std::mem::take(&mut peer.taps);
        let pressed = |b: u16| c.buttons & b != 0;
        // Taps: pressed now, or in a command since the last frame.
        let tapped = |b: u16| (c.buttons | earlier) & b != 0;
        view.yaw = c.yaw;
        view.pitch = c.pitch;
        // (Movement is run command by command in [`step_remotes`]; this is
        // for the body's animations until then.)
        *mv = predict::move_input_of(&c, weapon.speed_scale(), false);
        *wi = WeaponInput { fire: pressed(button::FIRE), ads: pressed(button::AIM), reload: tapped(button::RELOAD), inspect: false };
        // Their shots meet everyone where they saw them.
        // (`COD4RW_NETTEST_NOLAGCOMP=1`: not, to compare.)
        rewind.tick = if std::env::var_os("COD4RW_NETTEST_NOLAGCOMP").is_some() { now } else { lagcomp::rewind_to(c.view_tick, now) };
        if let Some(mut g) = grenades {
            g.frag = tapped(button::FRAG);
            g.special = tapped(button::SPECIAL);
        }
        // "Use": the bomb, a crate.
        if let Some(mut u) = use_objective {
            u.0 = pressed(button::USE);
        }
        if let Some(mut u) = use_crate {
            u.0 = pressed(button::USE);
        }
        // The friend changed between primary and secondary.
        if let (Some(mut s), Some(l)) = (switch, loadout)
            && tapped(button::SWITCH)
            && l.current < 2
            && l.class.guns.len() > 1
        {
            s.to = Some(1 - l.current);
        }
        // Once per press, however many frames the same controls last.
        cover_tap.want = tapped(button::COVER) && !cover_tap.held;
        cover_tap.held = pressed(button::COVER);
    }
}

/// Each friend's soldier moves by their commands, each for as long as the
/// friend's game ran it (and no faster than the clock allows): as the
/// friend's own game predicted.
#[allow(clippy::type_complexity)]
fn step_remotes(
    time: Res<Time>,
    mut state: ResMut<HostState>,
    world: crate::movement::MoveWorld,
    objectives: Option<Res<crate::modes::Objectives>>,
    mut remotes: Query<(Entity, &RemotePlayer, &mut Transform, &mut Mover, &mut MoveInput, &mut ViewAngles, &WeaponState, Has<Frozen>, Has<Dead>)>,
    mut landed: MessageWriter<crate::movement::Landed>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs().min(0.25);
    for (e, remote, mut tf, mut mover, mut mv, mut view, weapon, frozen, dead) in &mut remotes {
        let Some(peer) = state.peers.get_mut(&remote.peer) else { continue };
        peer.budget = (peer.budget + dt * 1000.0).min(250.0);
        let held = objectives.as_ref().is_some_and(|o| o.using.iter().any(|u| u.0 == e));
        let mut pos = tf.translation;
        while let Some(c) = peer.queue.pop_front() {
            // Commands that never came (more lost in a row than the
            // redundancy covers).
            if peer.applied != 0 {
                peer.missed += c.sequence.wrapping_sub(peer.applied).saturating_sub(1).min(60);
            }
            let msec = (c.msec as f32).min(peer.budget);
            peer.budget -= msec;
            peer.applied = c.sequence;
            peer.applied_at = now;
            if frozen || dead {
                continue;
            }
            let input = predict::move_input_of(&c, weapon.speed_scale(), held);
            let angles = ViewAngles { yaw: c.yaw, pitch: c.pitch };
            if let Some(h) = world.run(&mut pos, &mut mover, &input, &angles, msec / 1000.0) {
                landed.write(crate::movement::Landed { entity: e, fall_height: h });
            }
            *mv = input;
            *view = angles;
        }
        if !(frozen || dead) {
            tf.translation = pos;
            tf.rotation = Quat::from_rotation_y(view.yaw);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pawn_state(id: u16, tf: &Transform, mover: &Mover, view: &ViewAngles, health: &Health, dead: bool, frozen: bool, weapon: &WeaponState, ads: bool, cover: Option<&crate::cover::InCover>) -> PawnState {
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
    if frozen {
        flags |= pawn_flags::FROZEN;
    }
    // In cover ([`crate::cover`]): seen in the cover poses.
    if let Some(c) = cover {
        flags |= pawn_flags::IN_COVER;
        if c.high {
            flags |= pawn_flags::COVER_HIGH;
        }
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
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn send_world(
    online: Res<Online>,
    time: Res<Time>,
    mut state: ResMut<HostState>,
    pawns: Query<(
        Entity,
        &Pawn,
        &Transform,
        &Mover,
        &ViewAngles,
        &Health,
        Has<Dead>,
        Has<Frozen>,
        &WeaponState,
        Option<&WeaponInput>,
        Option<&RemotePlayer>,
        Has<LocalPlayer>,
        Option<&crate::loadout::Loadout>,
        Option<&crate::cover::InCover>,
    )>,
    (match_state, objectives, radar): (Option<Res<crate::tdm::MatchState>>, Option<Res<crate::modes::Objectives>>, Res<crate::killstreaks::uav::Radar>),
    mirrors: streaks::Mirrors,
) {
    let now = time.elapsed_secs();
    // Details first, so a new pawn's name arrives with it.
    let mut seen = Vec::new();
    for (_, pawn, _, _, _, _, _, _, weapon, _, remote, local, loadout, _) in &pawns {
        let id = pawn.id as u16;
        seen.push(id);
        let info = PawnInfo {
            id,
            peer: remote.map_or(if local { HOST } else { BOT }, |r| r.peer),
            name: crate::online::lobby::clean_name(&pawn.name),
            team: side_of(pawn.team),
            // The gun's spec (what the guest loads it by), or the
            // equipment's name.
            weapon: asset_name(loadout.and_then(|l| l.class.guns.get(l.current)).map_or(weapon.def.name.as_str(), |g| g.spec.as_str())),
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
    // On a steady beat (not "a period from now", which rounds up to whole
    // frames: 40 Hz came out at 30), catching up after a stall.
    let period = 1.0 / snapshot_rate();
    state.next_snapshot = if now - state.next_snapshot > period { now + period } else { state.next_snapshot + period };
    let tick = (now * TICK_RATE) as u32;
    let all: Vec<PawnState> = pawns
        .iter()
        .take(cod4rw_multiplayer::protocol::MAX_PLAYERS)
        .map(|(_, pawn, tf, mover, view, health, dead, frozen, weapon, wi, _, _, _, cover)| {
            pawn_state(pawn.id as u16, tf, mover, view, health, dead, frozen, weapon, wi.is_some_and(|w| w.ads), cover)
        })
        .filter(|p| p.validate().is_ok())
        .collect();
    // (Cover, once, for the log.)
    let covered = all.iter().filter(|p| p.flags & pawn_flags::IN_COVER != 0).count();
    if covered != state.covered_logged {
        info!("netplay: {covered} pawn(s) in cover in the snapshot");
        state.covered_logged = covered;
    }
    // Everything else going on, as much as fits beside the pawns.
    let id = |e: Entity| pawns.get(e).map_or(BOT, |p| p.1.id as u16);
    let default_objectives = crate::modes::Objectives::default();
    let pings: Vec<(u16, u16)> = PINGS.lock().map(|p| p.iter().map(|(id, ms)| (*id as u16, *ms)).collect()).unwrap_or_default();
    let world = world::encode(&time, match_state.as_deref(), objectives.as_deref().unwrap_or(&default_objectives), &radar, &pings, &id);
    let room = cod4rw_multiplayer::protocol::MAX_PACKET.saturating_sub(40 + all.len() * 40 + 2 + world.len());
    let mut extra = Vec::with_capacity(2 + world.len() + room.min(400));
    extra.extend((world.len() as u16).to_le_bytes());
    extra.extend(&world);
    extra.extend(mirrors.encode(room, &id));
    let members: Vec<PeerId> = state.members.iter().map(|m| m.peer).collect();
    for peer in members {
        let Some(p) = state.peers.get_mut(&peer) else { continue };
        p.snapshots = p.snapshots.wrapping_add(1);
        let ack_age = ((now - p.applied_at) * 1000.0).clamp(0.0, 255.0) as u8;
        online.send_snapshot(peer, Snapshot { tick, sequence: p.snapshots, acknowledged_input: p.applied, ack_age, pawns: all.clone(), extra: extra.clone() });
    }
}

// ---- Guest ----

/// Snapshots, pawns' details and news from the host.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn receive_guest(
    mut commands: Commands,
    mut online: ResMut<Online>,
    net: Res<NetStart>,
    mut state: ResMut<GuestState>,
    puppets: Query<(Entity, &Puppet)>,
    (time, mut match_state, everyone): (Res<Time>, Option<ResMut<crate::tdm::MatchState>>, Query<Entity, With<Pawn>>),
    (mine, mut killed, mut damage): (Query<Entity, With<LocalPlayer>>, MessageWriter<crate::combat::Killed>, MessageWriter<crate::combat::Damage>),
    (mut pending, mut launches): (ResMut<crate::grenades::Pending>, MessageWriter<crate::explosives::Launch>),
    (mut inbox, mut stats, mut feed, names): (ResMut<world::GuestInbox>, ResMut<graph::NetStats>, ResMut<crate::combat::KillFeed>, Query<&Pawn>),
    prediction: Res<predict::Prediction>,
) {
    let now = time.elapsed_secs();
    // Another pawn's puppet, by the host's ID (not ours: we threw it).
    let puppet_of = |id: u16, puppets: &Query<(Entity, &Puppet)>| puppets.iter().find(|(_, p)| p.id == id).map(|(e, _)| e);
    for event in online.take_game() {
        match event {
            Event::Snapshot(s) => {
                // Loss and jitter, for the delay and the net graph.
                if let Some(last) = state.sequence {
                    let gap = s.sequence.wrapping_sub(last);
                    if gap > 0 && gap < 1000 {
                        state.lost = state.lost * 0.95 + (gap - 1) as f32 * 0.05;
                    }
                }
                state.sequence = Some(s.sequence);
                let fresh = state.latest.as_ref().is_none_or(|l| cod4rw_multiplayer::security::newer(s.tick, l.tick));
                if fresh {
                    if let (Some(at), Some(l)) = (state.arrived_at, state.latest.as_ref()) {
                        let gap = s.tick.wrapping_sub(l.tick) as f32;
                        if gap > 0.0 && gap < 30.0 {
                            state.interval += (gap - state.interval) * 0.1;
                        }
                        let expected = gap / TICK_RATE;
                        state.jitter = state.jitter * 0.9 + ((now - at) - expected).abs() * 0.1;
                    }
                    state.arrived_at = Some(now);
                    state.arrivals.push_back(now);
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
            Event::Message(_, LobbyMsg::Game(data)) => match wire::GameMsg::decode(&data) {
                Ok(m) => inbox.0.push(m),
                Err(e) => warn!("netplay: a bad message from the host: {e}"),
            },
            Event::Message(_, LobbyMsg::MatchOver(winner)) => {
                if let Some(state) = match_state.as_deref_mut()
                    && state.ended.is_none()
                {
                    let winner = match winner {
                        0 => Some(Team::Allies),
                        1 => Some(Team::Axis),
                        _ => None,
                    };
                    info!("netplay: the match is over: {winner:?} won");
                    state.ended = Some((winner, now));
                    for e in &everyone {
                        commands.entity(e).insert(Frozen);
                    }
                }
            }
            Event::Message(_, LobbyMsg::Kill { victim, attacker, weapon, location }) => {
                // The pawns as this game has them: puppets, and our own.
                let my_id = state.my_id;
                let entity = |id: u16| world::entity_of(id, my_id, mine.single().ok(), &puppets);
                let Some(victim_e) = entity(victim) else { continue };
                if Some(victim) == state.my_id {
                    state.killer = state.info.get(&attacker).filter(|_| attacker != victim).map(|i| i.name.clone());
                }
                let attacker_e = if attacker == BOT { None } else { entity(attacker) };
                let location = world::location_from(location);
                let weapon = intern(&weapon);
                // The finishing hit first: the kill feed's headshot icon,
                // the killcam's weapon ([`crate::ui`] reads it from damage).
                damage.write(crate::combat::Damage { target: victim_e, attacker: attacker_e, amount: 0.0, location, weapon });
                killed.write(crate::combat::Killed { victim: victim_e, attacker: attacker_e, weapon, location });
                // The kill feed's text, as the host's.
                let name = |e: Option<Entity>| e.and_then(|e| names.get(e).ok()).map(|p| p.name.clone());
                let victim_name = name(Some(victim_e)).unwrap_or_default();
                let text = match name(attacker_e) {
                    Some(a) if attacker != victim => {
                        let hs = location == crate::combat::HitLocation::Head;
                        format!("{a}  [{}{}]  {victim_name}", crate::killstreaks::feed_name(weapon), if hs { " HS" } else { "" })
                    }
                    _ => format!("{victim_name} died"),
                };
                info!("netplay: kill: {text}");
                feed.entries.push(crate::combat::KillFeedEntry { text, time: now });
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
                pending.queue_throw(kind, by, Vec3::from_array(at), Vec3::from_array(velocity), now + fuse.clamp(0.0, 60.0));
                debug!("netplay: a {kind:?} from {thrower}");
            }
            Event::Message(_, LobbyMsg::Launch { shooter, weapon, from, dir, yaw }) => {
                // A pawn's, or a helicopter's rocket (nobody's here).
                let by = if shooter == BOT { Some(Entity::PLACEHOLDER) } else { puppet_of(shooter, &puppets) };
                let Some(by) = by else { continue };
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
    // The net graph.
    while state.arrivals.front().is_some_and(|t| *t < now - 1.0) {
        state.arrivals.pop_front();
    }
    stats.snapshot_rate = state.arrivals.len() as f32;
    stats.loss = (state.lost / (1.0 + state.lost)).clamp(0.0, 1.0);
    stats.jitter_ms = state.jitter * 1000.0;
    stats.interp_ms = state.delay / TICK_RATE * 1000.0;
    // Our ping to the host, for its net graph.
    if now >= state.next_ping {
        state.next_ping = now + 2.0;
        if let Some(rtt) = prediction.rtt {
            online.send(HOST, &LobbyMsg::Game(wire::GameMsg::Ping((rtt * 1000.0).round().clamp(0.0, 9999.0) as u16).encode()));
        }
    }
}

/// Debug (`COD4RW_NETLOOK=fire`): a guest aims at the nearest enemy puppet
/// and pulls the trigger when it's close, to test shots through the host.
fn aim_at_host(
    puppets: Query<(&Transform, &Pawn), (With<Puppet>, Without<Dead>)>,
    mut me: Query<(&Transform, &Pawn, &mut ViewAngles, &mut WeaponInput), (With<LocalPlayer>, Without<Puppet>, Without<Dead>)>,
) {
    let Ok((tf, pawn, mut view, mut wi)) = me.single_mut() else { return };
    let Some((target, _)) = puppets
        .iter()
        .filter(|(_, p)| crate::combat::hostile(pawn, p))
        .min_by(|a, b| a.0.translation.distance(tf.translation).total_cmp(&b.0.translation.distance(tf.translation)))
    else {
        wi.fire = false;
        return;
    };
    let d = target.translation + Vec3::Y * 1.2 - (tf.translation + Vec3::Y * 1.5);
    view.yaw = (-d.x).atan2(-d.z);
    view.pitch = (d.y / d.xz().length().max(0.1)).atan();
    // Pulled and let go, for burst and single-shot guns.
    wi.fire = d.length() < 35.0 && (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_millis()) / 150) % 2 == 0;
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
        let by = k.attacker.and_then(|a| pawns.get(a).ok());
        let attacker = by.map_or(BOT, |p| p.id as u16);
        info!("netplay: kill: {} [{}] {}", by.map_or("-", |p| p.name.as_str()), k.weapon, victim.name);
        online.send(EVERYONE, &LobbyMsg::Kill { victim: victim.id as u16, attacker, weapon: asset_name(k.weapon), location: world::location_byte(k.location) });
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
        online.send(EVERYONE, &LobbyMsg::Launch { shooter: id(l.shooter), weapon: asset_name(&l.weapon), from: l.from.to_array(), dir: l.dir.to_array(), yaw: l.yaw });
    }
}

/// Host: the match is over, for everyone.
fn announce_end(online: Res<Online>, state: Option<Res<crate::tdm::MatchState>>, mut sent: Local<bool>) {
    let ended = state.and_then(|s| s.ended);
    match ended {
        Some((winner, _)) if !*sent => {
            *sent = true;
            let code = match winner {
                Some(Team::Allies) => 0,
                Some(Team::Axis) => 1,
                None => 2,
            };
            info!("netplay: the match is over: {winner:?} won");
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
    let shown = state.and_then(|s| s.ended).is_some_and(|(_, at)| time.elapsed_secs() - at.max(crate::killcam::finished_at()) > crate::tdm::POST_MATCH) && !crate::killcam::busy();
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

/// Our class, to the host, when it's picked. A gun from a game the host
/// hasn't got (it couldn't run it) becomes the closest CoD4 gun first, for
/// us too, and we're told ([`mixed`]).
fn send_class(online: Res<Online>, net: Res<NetStart>, content: Res<crate::content::Content>, mut mine: Query<&mut PawnClass, (With<LocalPlayer>, Changed<PawnClass>)>) {
    let host_games = net.members.iter().find(|m| m.peer == HOST).map_or(0, |m| m.profile.games);
    for mut class in &mut mine {
        let mut swapped = Vec::new();
        for gun in &mut class.bypass_change_detection().0.guns {
            let game = mixed::gun_game(&gun.spec);
            if host_games & mixed::bit(game) == 0 {
                let to = mixed::standin(&content, &gun.spec);
                swapped.push(format!("{} -> {} (the host hasn't got {})", mixed::gun_name(&content, &gun.spec), mixed::gun_name(&content, &to), game.title()));
                *gun = Gun { name: to.to_ascii_uppercase(), spec: to, camo: 0, variant: None };
            }
        }
        if !swapped.is_empty() {
            mixed::notify(format!("Class changed: {}", swapped.join(", ")));
        }
        online.send(HOST, &LobbyMsg::Class(class_to(&class.0)));
    }
}

/// Everyone else, posed from the snapshots.
#[allow(clippy::type_complexity)]
fn pose_puppets(
    mut commands: Commands,
    time: Res<Time>,
    mut state: ResMut<GuestState>,
    assets: Option<Res<PawnAssets>>,
    content: Res<crate::content::Content>,
    mut puppets: Query<(Entity, &mut Puppet, &mut Pawn, &mut Transform, &mut Mover, &mut ViewAngles, &mut Health, &mut WeaponState, &mut WeaponInput, &mut Visibility, Has<Dead>, Has<crate::cover::InCover>)>,
    mut shots: MessageWriter<ShotFired>,
) {
    let Some(latest) = state.latest.as_ref().map(|s| s.tick as f32) else { return };
    // How far behind: two snapshots, and more when they come unevenly or
    // go missing (so there's still one to move towards). Up quickly, back
    // down gently.
    let floor = (state.interval * 2.0).max(MIN_DELAY_TICKS);
    let want = (floor + state.jitter * TICK_RATE * 1.5 + state.lost.min(1.0) * state.interval * 4.0).clamp(floor.min(MAX_DELAY_TICKS), MAX_DELAY_TICKS);
    let rate = if want > state.delay { 4.0 } else { 0.5 };
    state.delay += (want - state.delay) * (time.delta_secs() * rate).min(1.0);
    // The puppets' clock: steady, that far behind the newest snapshot.
    let target = latest - state.delay;
    let clock = match state.clock {
        Some(c) if (target - c).abs() < 20.0 => {
            let c = c + time.delta_secs() * TICK_RATE;
            // Snapshots late: carry on ahead of the newest a little.
            let c = c.min(latest + EXTRAPOLATE_TICKS);
            c + (target - c) * 0.05
        }
        _ => target,
    };
    state.clock = Some(clock);
    let (tick, fraction) = (clock.floor().max(0.0) as u32, clock.fract());

    let ids: Vec<u16> = state.latest.as_ref().map_or(Vec::new(), |s| s.pawns.iter().map(|p| p.id).collect());
    let mut posed = Vec::new();
    for (e, mut puppet, mut pawn, mut tf, mut mover, mut view, mut health, mut weapon, mut wi, mut visibility, dead, in_cover) in &mut puppets {
        posed.push(puppet.id);
        let Some(mut p) = state.snapshots.sample(puppet.id, tick, fraction) else { continue };
        // Past the newest snapshot: on along their way, briefly.
        let ahead = clock - latest;
        if ahead > 0.0 && p.life == 0 {
            for i in 0..3 {
                p.position[i] += p.velocity[i] * ahead.min(EXTRAPOLATE_TICKS) / TICK_RATE * if i == 1 { 0.0 } else { 1.0 };
            }
        }
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
                if let Some(def) = crate::loadout::bot_weapon(&content, &mixed::loadable(&content, &info.weapon)) {
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
        // In cover on the host: shown in its poses ([`crate::cover`]).
        let cover = p.flags & pawn_flags::IN_COVER != 0 && p.life == 0;
        if cover && !in_cover {
            bevy::log::info!("netplay: puppet {} takes cover", puppet.id);
            commands.entity(e).insert(crate::cover::InCover::replicated(p.flags & pawn_flags::COVER_HIGH != 0, time.elapsed_secs()));
        } else if !cover && in_cover {
            bevy::log::info!("netplay: puppet {} leaves cover", puppet.id);
            commands.entity(e).remove::<crate::cover::InCover>();
        }
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
        if let Some(def) = crate::loadout::bot_weapon(&content, &mixed::loadable(&content, &info.weapon)) {
            weapon.def = def;
        }
        let lived = p.life == 0;
        let visibility = if lived { Visibility::Inherited } else { Visibility::Hidden };
        commands.entity(e).insert((Puppet { id, weapon: info.weapon.clone(), shots: p.shots, lived }, Frozen, WeaponInput::default(), weapon, visibility));
    }
}

/// Our own soldier: health, death, respawn and being held still from the
/// host (its movement is [`predict`]'s).
#[allow(clippy::type_complexity)]
fn correct_own(
    mut commands: Commands,
    state: Res<GuestState>,
    mut prediction: ResMut<predict::Prediction>,
    mut me: Query<(Entity, &mut Transform, &mut Mover, &mut ViewAngles, &mut Health, &mut Pawn, Has<Dead>, Has<crate::cover::InCover>), With<LocalPlayer>>,
    time: Res<Time>,
) {
    let (Some(id), Some(latest)) = (state.my_id, state.latest.as_ref()) else { return };
    let Some(p) = latest.pawns.iter().find(|p| p.id == id) else { return };
    let Ok((e, mut tf, mut mover, mut view, mut health, mut pawn, dead, in_cover)) = me.single_mut() else { return };
    health.current = p.health as f32;
    if let Some(info) = state.info.get(&id) {
        pawn.kills = info.kills as u32;
        pawn.deaths = info.deaths as u32;
        pawn.team = team_of(info.team);
    }
    let at = Vec3::from_array(p.position);
    // In cover on the host ([`crate::cover`]): shown in its poses, and held
    // where the host has it (it moves it along the wall).
    let covered = p.flags & pawn_flags::IN_COVER != 0 && p.life == 0;
    if covered && !in_cover {
        info!("netplay: our soldier is in cover on the host");
        commands.entity(e).insert(crate::cover::InCover::replicated(p.flags & pawn_flags::COVER_HIGH != 0, time.elapsed_secs()));
    } else if !covered && in_cover {
        info!("netplay: our soldier left cover on the host");
        commands.entity(e).remove::<crate::cover::InCover>();
    }
    if covered && !dead {
        tf.translation = tf.translation.lerp(at, 0.4);
        mover.velocity = Vec3::ZERO;
        prediction.reset();
        return;
    }
    match (p.life == 1, dead) {
        (true, false) => {
            commands.entity(e).insert((Dead { respawn_at: f32::INFINITY, killer: state.killer.clone() }, Frozen));
        }
        (false, true) => {
            // Respawned: where the host put us, facing its way.
            commands.entity(e).remove::<(Dead, Frozen, crate::loadout::AwaitingClass)>();
            tf.translation = at;
            // (Fresh, as the host's respawn leaves it.)
            let stance = mover.stance;
            *mover = Mover::default();
            mover.stance = stance;
            view.yaw = p.yaw;
            view.pitch = 0.0;
            prediction.reset();
        }
        _ => {}
    }
}
