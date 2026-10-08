//! Pawns (players and bots), health, hit locations, damage, death and
//! respawning.

use crate::collision::Layer;
use crate::movement::{Frozen, Landed, MoveInput, Mover, Stance, ViewAngles};
use crate::units::u;
use crate::weapons::WeaponState;
use crate::world::{MapInfo, SpawnKind, SpawnPoint};
use avian3d::prelude::*;
use bevy::prelude::*;
use rand::Rng;
use std::sync::atomic::{AtomicU32, Ordering};

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Damage>()
            .add_message::<Killed>()
            .init_resource::<KillFeed>()
            .add_systems(OnEnter(crate::state::GameState::InGame), setup_pawn_assets.in_set(crate::state::Setup::Content))
            .add_systems(
                Update,
                // An online guest's health and deaths are the host's ([`crate::netplay`]).
                (
                    fall_damage.run_if(crate::netplay::authority),
                    apply_damage.run_if(crate::netplay::authority),
                    regen_health.run_if(crate::netplay::authority),
                    respawn.run_if(crate::netplay::authority),
                    mark_spawned,
                    update_hitboxes,
                )
                    .chain()
                    .after(crate::movement::MovementSet)
                    .run_if(crate::state::in_game),
            );
    }
}

pub const MAX_HEALTH: f32 = 100.0;
/// Hardcore's (`scr_player_maxhealth 30`).
pub const HARDCORE_HEALTH: f32 = 30.0;

/// A pawn's full health: 100, Hardcore 30.
pub fn max_health() -> f32 {
    if crate::tdm::hardcore() { HARDCORE_HEALTH } else { MAX_HEALTH }
}
/// `_healthoverlay.gsc`: 5 s after the last hurt, health comes back: all
/// at once, or from at or below 55% (`healthOverlayCutoff`) a tenth of it
/// every 0.05 s.
const REGEN_DELAY: f32 = 5.0;
const VERY_HURT: f32 = 0.55;
const VERY_HURT_REGEN: f32 = 0.1 / 0.05;
/// `TimeUntilSpawn`: the death's 0.25 + 1.75 s, no respawn delay.
const RESPAWN_DELAY: f32 = 2.0;

/// When someone last hurt an enemy (`useStartSpawns` ends with the
/// match's first such damage), as f32 bits.
static LAST_HOSTILE_DAMAGE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Hardcore's (`scr_player_respawndelay 10`).
const HARDCORE_RESPAWN_DELAY: f32 = 10.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Team {
    #[default]
    Allies,
    Axis,
}

impl Team {
    pub fn other(self) -> Team {
        match self {
            Team::Allies => Team::Axis,
            Team::Axis => Team::Allies,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Team::Allies => "Marines",
            Team::Axis => "OpFor",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Team::Allies => Color::srgb(0.35, 0.6, 1.0),
            Team::Axis => Color::srgb(1.0, 0.35, 0.3),
        }
    }
}

#[derive(Component, Clone, Debug)]
pub struct Pawn {
    pub name: String,
    pub team: Team,
    pub kills: u32,
    pub deaths: u32,
    /// Hurt someone a teammate then killed ([`crate::ui`]'s progression).
    pub assists: u32,
    /// Unique among pawns: who's who in free-for-all.
    pub id: u32,
}

static NEXT_PAWN_ID: AtomicU32 = AtomicU32::new(1);

/// Is the match free-for-all (everyone against everyone)?
pub fn free_for_all() -> bool {
    crate::modes::current() == crate::modes::GameMode::Ffa
}

/// Are `a` and `b` enemies: on different teams, or in free-for-all, any
/// two pawns. Use this rather than comparing teams.
pub fn hostile(a: &Pawn, b: &Pawn) -> bool {
    if free_for_all() { a.id != b.id } else { a.team != b.team }
}

#[derive(Component, Clone, Copy, Debug)]
pub struct Health {
    pub current: f32,
    pub last_damage: f32,
}

impl Default for Health {
    fn default() -> Self {
        Health { current: max_health(), last_damage: -100.0 }
    }
}

/// Scales the damage a pawn takes (Juggernaut).
#[derive(Component, Clone, Copy, Debug)]
pub struct DamageScale(pub f32);

/// Present while a pawn is dead.
#[derive(Component)]
pub struct Dead {
    pub respawn_at: f32,
    pub killer: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitLocation {
    Head,
    /// CoD4's neck: snipers do more there (1.5), it isn't a headshot.
    Neck,
    /// The upper torso (the chest): CoD4's `torso_upper`.
    Torso,
    /// The lower torso (the stomach): `torso_lower`, where snipers do less
    /// (an M40A3 kills with a chest shot, not with one here).
    TorsoLower,
    Legs,
}

/// A hitbox collider, child of its pawn.
#[derive(Component, Clone, Copy)]
pub struct Hitbox {
    pub owner: Entity,
    pub location: HitLocation,
}


#[derive(Message, Clone)]
pub struct Damage {
    pub target: Entity,
    pub attacker: Option<Entity>,
    pub amount: f32,
    pub location: HitLocation,
    pub weapon: &'static str,
}

#[derive(Message, Clone)]
pub struct Killed {
    pub victim: Entity,
    pub attacker: Option<Entity>,
    /// The finishing hit, retained even if the killer has switched guns.
    pub weapon: &'static str,
    pub location: HitLocation,
}

pub struct KillFeedEntry {
    pub text: String,
    pub time: f32,
}

#[derive(Resource, Default)]
pub struct KillFeed {
    pub entries: Vec<KillFeedEntry>,
}

/// Marker resource: pawns can be spawned (character models are attached
/// separately by [`crate::thirdperson`]).
#[derive(Resource)]
pub struct PawnAssets;

fn setup_pawn_assets(mut commands: Commands) {
    commands.insert_resource(PawnAssets);
}

/// Spawn a pawn at `spawn`. Returns the pawn entity.
pub fn spawn_pawn(commands: &mut Commands, _assets: &PawnAssets, name: &str, team: Team, spawn: &SpawnPoint) -> Entity {
    let pawn = commands
        .spawn((
            Name::new(name.to_owned()),
            Pawn { name: name.to_owned(), team, kills: 0, deaths: 0, assists: 0, id: NEXT_PAWN_ID.fetch_add(1, Ordering::Relaxed) },
            Health::default(),
            Mover::default(),
            MoveInput::default(),
            ViewAngles { yaw: spawn.yaw, pitch: 0.0 },
            WeaponState::default(),
            Transform::from_translation(spawn.pos + Vec3::Y * u(1.0)).with_rotation(Quat::from_rotation_y(spawn.yaw)),
            Visibility::default(),
        ))
        .id();
    let hitbox = |loc: HitLocation, collider: Collider| {
        (
            Hitbox { owner: pawn, location: loc },
            collider,
            CollisionLayers::new(Layer::Hitbox, LayerMask::NONE),
            Transform::default(),
            ChildOf(pawn),
        )
    };
    commands.spawn(hitbox(HitLocation::Head, Collider::sphere(u(5.0))));
    commands.spawn(hitbox(HitLocation::Neck, Collider::sphere(u(3.5))));
    commands.spawn(hitbox(HitLocation::Torso, Collider::cuboid(u(20.0), u(TORSO_UPPER), u(14.0))));
    commands.spawn(hitbox(HitLocation::TorsoLower, Collider::cuboid(u(18.0), u(TORSO_LOWER), u(13.0))));
    commands.spawn(hitbox(HitLocation::Legs, Collider::cuboid(u(16.0), u(34.0), u(12.0))));
    pawn
}

/// Where the neck is between the head and the chest's middle.
const NECK_ALONG: f32 = 0.36;
/// The torso (22 units, its middle at the layout's) split in two: the
/// chest above, the stomach below (CoD4's `torso_upper`, `torso_lower`).
const TORSO_UPPER: f32 = 12.0;
const TORSO_LOWER: f32 = 10.0;

/// Hitbox and body placement for a stance. Returns (head, torso, legs) local
/// centres and whether the pawn is lying down.
fn body_layout(stance: Stance) -> (Vec3, Vec3, Vec3, bool) {
    match stance {
        Stance::Stand => (Vec3::Y * u(65.0), Vec3::Y * u(43.0), Vec3::Y * u(17.0), false),
        Stance::Crouch => (Vec3::Y * u(45.0), Vec3::Y * u(28.0), Vec3::Y * u(10.0), false),
        Stance::Prone => {
            (Vec3::new(0.0, u(10.0), -u(30.0)), Vec3::new(0.0, u(8.0), -u(10.0)), Vec3::new(0.0, u(6.0), u(20.0)), true)
        }
    }
}

fn update_hitboxes(pawns: Query<(&Mover, &Children), With<Pawn>>, mut hitboxes: Query<(&Hitbox, &mut Transform)>) {
    for (mover, children) in &pawns {
        let (mut head, mut torso, legs, prone) = body_layout(mover.stance);
        // Leaning moves the head and upper body sideways (pawn space: +X is right).
        let lean = mover.lean * crate::movement::LEAN_DISTANCE;
        head += Vec3::new(lean, -mover.lean.abs() * u(2.0), 0.0);
        torso.x += lean * 0.4;
        for child in children.iter() {
            if let Ok((hb, mut tf)) = hitboxes.get_mut(child) {
                tf.translation = match hb.location {
                    HitLocation::Head => head,
                    // A third of the way from the head to the chest.
                    HitLocation::Neck => head.lerp(torso, NECK_ALONG),
                    // Each half along the line from the torso to the head
                    // (up standing, forward lying down).
                    HitLocation::Torso => torso + (head - torso).normalize_or_zero() * u(TORSO_LOWER * 0.5),
                    HitLocation::TorsoLower => torso - (head - torso).normalize_or_zero() * u(TORSO_UPPER * 0.5),
                    HitLocation::Legs => legs,
                };
                tf.rotation = if prone && hb.location != HitLocation::Head {
                    Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)
                } else {
                    Quat::IDENTITY
                };
            }
        }
    }
}

fn fall_damage(mut landed: MessageReader<Landed>, mut damage: MessageWriter<Damage>) {
    for l in landed.read() {
        // bg_fallDamageMinHeight 128, bg_fallDamageMaxHeight 300.
        if l.fall_height > 128.0 {
            let amount = ((l.fall_height - 128.0) / (300.0 - 128.0)).min(1.0) * max_health();
            damage.write(Damage {
                target: l.entity,
                attacker: None,
                amount,
                location: HitLocation::Legs,
                weapon: "falling",
            });
        }
    }
}

/// `lastStandWait`'s invulnerability on going down (s).
const LAST_STAND_GRACE: f32 = 0.5;

fn apply_damage(
    mut commands: Commands,
    time: Res<Time>,
    mut damage: MessageReader<Damage>,
    mut killed: MessageWriter<Killed>,
    mut pawns: Query<(&mut Pawn, &mut Health, Has<Dead>)>,
    scales: Query<&DamageScale>,
    mut feed: ResMut<KillFeed>,
    last_stand: Query<(Option<&crate::loadout::Loadout>, Has<crate::perks::Downed>)>,
    downed_at: Query<&crate::perks::Downed>,
) {
    let now = time.elapsed_secs();
    for d in damage.read() {
        // Friendly fire is off, as in CoD4's default TDM rules; Hardcore's
        // `scr_team_fftype 1` hurts the teammate.
        let friendly = d
            .attacker
            .filter(|&a| a != d.target)
            .and_then(|a| Some(!hostile(&pawns.get(a).ok()?.0, &pawns.get(d.target).ok()?.0)))
            == Some(true);
        if friendly && !crate::tdm::hardcore() {
            continue;
        }
        let Ok((_, mut health, dead)) = pawns.get_mut(d.target) else { continue };
        if dead || health.current <= 0.0 {
            continue;
        }
        // Last Stand: half a second's grace on going down.
        if last_stand.get(d.target).is_ok_and(|(_, down)| down) && downed_at.get(d.target).is_ok_and(|t| t.since(now) < LAST_STAND_GRACE) {
            continue;
        }
        // Juggernaut is against players' damage, not a fall's.
        let scale = if d.attacker.is_some() { scales.get(d.target).map_or(1.0, |s| s.0) } else { 1.0 };
        health.current -= d.amount * scale;
        health.last_damage = now;
        if d.attacker.is_some_and(|a| a != d.target) && !friendly {
            LAST_HOSTILE_DAMAGE.store(now.to_bits(), std::sync::atomic::Ordering::Relaxed);
        }
        if health.current > 0.0 {
            continue;
        }
        // Last Stand: downed instead of dead ([`crate::perks`]).
        if let Ok((loadout, false)) = last_stand.get(d.target) {
            if crate::perks::may_go_down(loadout, d.weapon, d.location) {
                health.current = 1.0;
                commands.entity(d.target).insert(crate::perks::Downed::new(now, d.attacker, d.weapon));
                continue;
            }
        }
        health.current = 0.0;
        let attacker_name = d.attacker.and_then(|a| pawns.get(a).ok()).map(|(p, _, _)| p.name.clone());
        if let Ok((mut victim, _, _)) = pawns.get_mut(d.target) {
            victim.deaths += 1;
            let victim_name = victim.name.clone();
            let headshot = d.location == HitLocation::Head;
            let text = match &attacker_name {
                Some(a) if d.attacker != Some(d.target) => {
                    format!("{a}  [{}{}]  {victim_name}", crate::killstreaks::feed_name(d.weapon), if headshot { " HS" } else { "" })
                }
                _ => format!("{victim_name} died"),
            };
            feed.entries.push(KillFeedEntry { text, time: now });
            // Killing yourself (a grenade) has no killer to name.
            let killer = attacker_name.filter(|_| d.attacker != Some(d.target));
            let delay = if crate::tdm::hardcore() { HARDCORE_RESPAWN_DELAY } else { RESPAWN_DELAY };
            commands.entity(d.target).insert((Dead { respawn_at: now + delay, killer }, Frozen));
            killed.write(Killed { victim: d.target, attacker: d.attacker, weapon: d.weapon, location: d.location });
        }
        // A teammate killed (Hardcore) isn't a kill.
        if let Some(a) = d.attacker.filter(|&a| a != d.target && !friendly) {
            if let Ok((mut p, _, _)) = pawns.get_mut(a) {
                p.kills += 1;
            }
        }
    }
}

fn regen_health(time: Res<Time>, mut q: Query<&mut Health, Without<Dead>>) {
    let now = time.elapsed_secs();
    for mut h in &mut q {
        let max = max_health();
        if h.current < max && now - h.last_damage > REGEN_DELAY {
            h.current = if h.current <= max * VERY_HURT {
                (h.current + VERY_HURT_REGEN * max * time.delta_secs()).min(max)
            } else {
                max
            };
        }
    }
}

fn respawn(
    mut commands: Commands,
    time: Res<Time>,
    map: Option<Res<MapInfo>>,
    spatial: SpatialQuery,
    state: Option<Res<crate::tdm::MatchState>>,
    mut dead: Query<(Entity, &Dead, &Pawn, &mut Transform, &mut Health, &mut Mover, &mut ViewAngles, &mut WeaponState)>,
    alive: Query<(Entity, &Pawn, &Transform, &ViewAngles), Without<Dead>>,
    grenades: Query<&GlobalTransform, With<crate::grenades::LiveGrenade>>,
    objectives: Option<Res<crate::modes::Objectives>>,
    strikes: Option<Res<crate::killstreaks::airstrike::Airstrikes>>,
    mut recent: Local<Vec<(Vec3, f32, Entity)>>,
    mut last: Local<std::collections::HashMap<Entity, Vec3>>,
) {
    let Some(map) = map else { return };
    let now = time.elapsed_secs();
    recent.retain(|r| now - r.1 < SPAWN_REUSE_TIME);
    let sight = crate::collision::sight_filter();
    let sees = |from: Vec3, to: Vec3| Dir3::new(to - from).ok().is_none_or(|d| spatial.cast_ray(from, d, from.distance(to), true, &sight).is_none());
    let bombs: Vec<Vec3> = grenades.iter().map(|g| g.translation()).collect();
    // CoD4's grace period (`level.gracePeriod`): the first 15 s respawn at
    // the team's start.
    // `useStartSpawns`: the team's start points until the match's first
    // hostile damage.
    let first_hurt = f32::from_bits(LAST_HOSTILE_DAMAGE.load(std::sync::atomic::Ordering::Relaxed));
    let grace = state.as_ref().is_some_and(|s| first_hurt < s.started);
    for (e, d, pawn, mut tf, mut health, mut mover, mut view, mut weapon) in &mut dead {
        if now < d.respawn_at {
            continue;
        }
        let allies: Vec<Vec3> = alive.iter().filter(|(o, p, ..)| *o != e && !hostile(p, pawn)).map(|(_, _, t, _)| t.translation).collect();
        let enemies: Vec<(Vec3, Vec3)> =
            alive.iter().filter(|(_, p, ..)| hostile(p, pawn)).map(|(_, _, t, v)| (t.translation, v.forward())).collect();
        let occupied: Vec<Vec3> = alive.iter().map(|(_, _, t, _)| t.translation).collect();
        // Spawns enemies used lately, and how far from them they still are.
        let reused: Vec<(Vec3, f32, f32)> = recent
            .iter()
            .filter_map(|&(at, when, who)| {
                let (_, p, t, _) = alive.get(who).ok()?;
                hostile(p, pawn).then(|| (at, now - when, t.translation.distance(at)))
            })
            .collect();
        let anchors = objectives.as_ref().map(|o| o.spawn_anchors(pawn.team)).unwrap_or_default();
        let spawning = Spawning {
            team: pawn.team,
            allies,
            enemies,
            occupied,
            initial: grace,
            anchors: &anchors,
            last: last.get(&e).copied(),
            reused,
            grenades: bombs.clone(),
            airstrike: &|at| strikes.as_ref().map_or(0.0, |s| s.danger(at, now)),
            sees: &sees,
        };
        let spawn = pick_spawn(&map, &spawning);
        if std::env::var_os("COD4RW_SIM").is_some() {
            let nearest = spawning.enemies.iter().map(|(p, _)| p.distance(spawn.pos) / u(1.0)).fold(f32::MAX, f32::min);
            let seen = spawning.enemies.iter().filter(|(p, _)| sees(*p + Vec3::Y * u(50.0), spawn.pos + Vec3::Y * u(50.0))).count();
            info!("spawn: {} {:?} nearest enemy {:.0}u, seen by {seen}{}", pawn.name, spawn.kind, nearest, if grace { " (grace period)" } else { "" });
        }
        recent.push((spawn.pos, now, e));
        last.insert(e, spawn.pos);
        tf.translation = spawn.pos + Vec3::Y * u(1.0);
        *view = ViewAngles { yaw: spawn.yaw, pitch: 0.0 };
        *health = Health::default();
        *mover = Mover::default();
        *weapon = WeaponState::default();
        commands.entity(e).remove::<(Dead, Frozen)>();
    }
}

/// Where and when a pawn last spawned, for the grenade spawn protection.
#[derive(Component, Clone, Copy, Debug)]
pub struct Spawned {
    pub at: Vec3,
    pub time: f32,
}

/// Grenades (frags, C4, claymores, launched grenades) don't hurt a player
/// for 3.5 s after spawning when they go off within 250 units of the
/// spawn (`Callback_PlayerDamage`'s "spawnkill grenades" check).
const GRENADE_SPAWN_PROTECTION: f32 = 3.5;
const GRENADE_SPAWN_RADIUS: f32 = 250.0;

/// Whether a grenade going off at `at` spares a pawn that spawned so.
pub fn spawn_protected(spawned: Option<&Spawned>, at: Vec3, now: f32) -> bool {
    spawned.is_some_and(|s| now - s.time < GRENADE_SPAWN_PROTECTION && s.at.distance(at) < u(GRENADE_SPAWN_RADIUS))
}

/// Note each spawn: a pawn's first and every respawn.
fn mark_spawned(
    mut commands: Commands,
    time: Res<Time>,
    new: Query<(Entity, &Transform), Added<Pawn>>,
    mut respawned: RemovedComponents<Dead>,
    pawns: Query<&Transform, (With<Pawn>, Without<Dead>)>,
) {
    let now = time.elapsed_secs();
    for (e, tf) in &new {
        commands.entity(e).insert(Spawned { at: tf.translation, time: now });
    }
    for e in respawned.read() {
        if let Ok(tf) = pawns.get(e) {
            commands.entity(e).insert(Spawned { at: tf.translation, time: now });
        }
    }
}

/// `avoidSpawnReuse`: an enemy's spawn is avoided for 10 s while they're
/// within 800 units of it.
const SPAWN_REUSE_TIME: f32 = 10.0;
const SPAWN_REUSE_DIST: f32 = 800.0;
/// `avoidSameSpawn`, `getLosPenalty`/`avoidWeaponDamage` (per enemy that can
/// see it, per grenade within 250 units), and Domination's favoured spawns.
const SAME_SPAWN_PENALTY: f32 = 50_000.0;
const DANGER_PENALTY: f32 = 100_000.0;
const GRENADE_DANGER: f32 = 250.0;
const FAVORED: f32 = 25_000.0;
/// `getSpawnpoint_NearTeam`: teammates' distance counts this much against a
/// spawn, enemies' for it.
const ALLIED_DISTANCE_WEIGHT: f32 = 2.0;
/// `getSpawnpoint_DM`: most players about this far away; nearer than this,
/// a spawn counts against.
const DM_IDEAL_DIST: f32 = 1600.0;
const DM_BAD_DIST: f32 = 1200.0;

/// What a spawn choice weighs up.
pub struct Spawning<'a> {
    pub team: Team,
    /// Living teammates; living enemies and which way they look.
    pub allies: Vec<Vec3>,
    pub enemies: Vec<(Vec3, Vec3)>,
    /// Where anyone stands (no spawning inside them).
    pub occupied: Vec<Vec3>,
    /// The match's start (or its grace period): the team's start points.
    pub initial: bool,
    /// Pulls (weight > 0, the team's flags) or pushes (< 0) within ~1500u.
    pub anchors: &'a [(Vec3, f32)],
    /// This player's last spawn point.
    pub last: Option<Vec3>,
    /// Spawns enemies used lately: where, how long ago, how far they are now.
    pub reused: Vec<(Vec3, f32, f32)>,
    /// Live grenades.
    pub grenades: Vec<Vec3>,
    /// An airstrike under way's danger to a point (0 none, 1 full).
    pub airstrike: &'a dyn Fn(Vec3) -> f32,
    /// Whether there's a clear line between two points.
    pub sees: &'a dyn Fn(Vec3, Vec3) -> bool,
}

impl Spawning<'_> {
    /// A match's first spawns: just the team's start points.
    pub fn start(team: Team, occupied: &[Vec3]) -> Spawning<'static> {
        Spawning {
            team,
            allies: Vec::new(),
            enemies: Vec::new(),
            occupied: occupied.to_vec(),
            initial: true,
            anchors: &[],
            last: None,
            reused: Vec::new(),
            grenades: Vec::new(),
            airstrike: &|_| 0.0,
            sees: &|_, _| false,
        }
    }
}

/// Choose a spawn point as CoD4's `_spawnlogic.gsc` does. At the start (and
/// in the 15 s grace period), one of the team's start points. Otherwise the
/// game type's spawns, weighted:
///
/// - team modes (`getSpawnpoint_NearTeam`): the sum of enemies' distances
///   less twice teammates', per player, so near the team and away from the
///   enemy; Domination's spawns by the team's flags favoured (+25000);
/// - free-for-all (`getSpawnpoint_DM`): most players about 1600 units away,
///   any nearer than 1200 counting against;
///
/// then the same spawn as last time (-50000), one an enemy spawned at in the
/// last 10 s while they're still within 800 units of it, each live grenade
/// within 250 units (-100000) and each enemy that can see it (-100000; not
/// one facing away from a spawn facing away from them) count against. The
/// best wins (ties at random); nobody is spawned inside anyone. Each game
/// type has its own spawns (`mp_tdm_spawn`, `mp_dm_spawn`, `mp_dom_spawn`,
/// ...), where the map has them.
pub fn pick_spawn(map: &MapInfo, sp: &Spawning) -> SpawnPoint {
    use crate::modes::GameMode;
    let mut rng = rand::rng();
    let team = sp.team;
    let free = |s: &&SpawnPoint| sp.occupied.iter().all(|o| o.distance(s.pos) > u(40.0));
    let has = |kind: SpawnKind| map.spawns.iter().any(|s| s.kind == kind);
    let mode = crate::modes::current();
    let ffa = mode == GameMode::Ffa && has(SpawnKind::Dm);
    let dom = mode == GameMode::Dom && has(SpawnKind::Dom);
    let sab = mode == GameMode::Sab && has(SpawnKind::SabAllies);
    // Search and Destroy: always the side's own spawns (the round starts there).
    if mode == GameMode::Sd && has(SpawnKind::SdAttacker) {
        let kind = if crate::modes::sd::attacking(team) { SpawnKind::SdAttacker } else { SpawnKind::SdDefender };
        let spots: Vec<_> = map.spawns.iter().filter(|s| s.kind == kind).filter(free).collect();
        if !spots.is_empty() {
            return *spots[rng.random_range(0..spots.len())];
        }
    }
    if sp.initial && !ffa {
        let kind = match (team, dom, sab) {
            (Team::Allies, _, true) => SpawnKind::SabAlliesStart,
            (Team::Axis, _, true) => SpawnKind::SabAxisStart,
            (Team::Allies, true, _) => SpawnKind::DomAlliesStart,
            (Team::Axis, true, _) => SpawnKind::DomAxisStart,
            (Team::Allies, ..) => SpawnKind::AlliesStart,
            (Team::Axis, ..) => SpawnKind::AxisStart,
        };
        let all_starts: Vec<_> = map.spawns.iter().filter(|s| s.kind == kind).collect();
        let starts: Vec<_> = all_starts.iter().copied().filter(free).collect();
        if !starts.is_empty() {
            return *starts[rng.random_range(0..starts.len())];
        }
        // More players than the side's start spots (Bloc has 8 a side, a
        // full lobby 9): the free spawn nearest them, not one anywhere on
        // the map (which put the odd one out by the enemy's start).
        if !all_starts.is_empty() {
            let centre = all_starts.iter().map(|s| s.pos).sum::<Vec3>() / all_starts.len() as f32;
            let near = map.spawns.iter().filter(|s| matches!(s.kind, SpawnKind::Tdm | SpawnKind::Dm)).filter(free).min_by(|a, b| a.pos.distance(centre).total_cmp(&b.pos.distance(centre)));
            if let Some(s) = near {
                return SpawnPoint { yaw: all_starts[0].yaw, ..*s };
            }
        }
    }
    let sab_kind = if team == Team::Allies { SpawnKind::SabAllies } else { SpawnKind::SabAxis };
    let tdm = has(SpawnKind::Tdm);
    let wanted = |s: &&SpawnPoint| match (ffa, dom) {
        (true, _) => s.kind == SpawnKind::Dm,
        (_, true) => s.kind == SpawnKind::Dom,
        _ if sab => s.kind == sab_kind,
        _ if tdm => s.kind == SpawnKind::Tdm,
        _ => s.kind == SpawnKind::Dm,
    };
    // CoD units.
    let cod = |d: f32| d / u(1.0);
    let weight = |s: &SpawnPoint| -> f32 {
        let mut w = if ffa {
            let others: Vec<f32> = sp.enemies.iter().map(|(p, _)| cod(p.distance(s.pos))).collect();
            if others.is_empty() {
                rand::random_range(0.0..0.2)
            } else {
                let near_bad: f32 = others.iter().filter(|&&d| d < DM_BAD_DIST).map(|d| (DM_BAD_DIST - d) / DM_BAD_DIST).sum();
                let from_ideal = others.iter().map(|d| (d - DM_IDEAL_DIST).abs()).sum::<f32>() / others.len() as f32;
                (DM_IDEAL_DIST - from_ideal) / DM_IDEAL_DIST - near_bad * 2.0 + rand::random_range(0.0..0.2)
            }
        } else {
            let players = sp.allies.len() + sp.enemies.len();
            if players == 0 {
                0.0
            } else {
                let enemy_sum: f32 = sp.enemies.iter().map(|(p, _)| cod(p.distance(s.pos))).sum();
                let ally_sum: f32 = sp.allies.iter().map(|p| cod(p.distance(s.pos))).sum();
                (enemy_sum - ALLIED_DISTANCE_WEIGHT * ally_sum) / players as f32
            }
        };
        // Domination: the spawns by the team's flags.
        if dom {
            let pull: f32 = sp.anchors.iter().map(|(at, wt)| wt * (1.0 - at.distance(s.pos) / u(1500.0)).max(0.0)).sum();
            w += FAVORED * pull;
        }
        if sp.last.is_some_and(|l| l.distance(s.pos) < u(1.0)) {
            w -= SAME_SPAWN_PENALTY;
        }
        for &(at, age, dist) in &sp.reused {
            if at.distance(s.pos) < u(1.0) && cod(dist) < SPAWN_REUSE_DIST {
                let d = cod(dist) / SPAWN_REUSE_DIST;
                w -= 1000.0 * (1.0 - d * d) * (1.0 - age / SPAWN_REUSE_TIME);
            }
        }
        w -= DANGER_PENALTY * sp.grenades.iter().filter(|g| cod(g.distance(s.pos)) < GRENADE_DANGER).count() as f32;
        // An airstrike coming down there, as much as its danger.
        w -= DANGER_PENALTY * (sp.airstrike)(s.pos);
        w
    };
    // Enemies that can see it (`spawnPerFrameUpdate`'s sight checks).
    let sight = |s: &SpawnPoint| -> f32 {
        let facing = Vec3::new(-s.yaw.sin(), 0.0, -s.yaw.cos());
        let seen = sp
            .enemies
            .iter()
            .filter(|(p, look)| {
                let diff = *p - s.pos;
                !(facing.dot(diff) < 0.0 && look.dot(diff) > 0.0) && (sp.sees)(*p + Vec3::Y * u(50.0), s.pos + Vec3::Y * u(50.0))
            })
            .count();
        DANGER_PENALTY * seen as f32
    };
    let mut candidates: Vec<(f32, SpawnPoint)> = map.spawns.iter().filter(wanted).filter(free).map(|s| (weight(s), *s)).collect();
    let top = if candidates.is_empty() { map.spawns.clone() } else { best_spawns(&mut candidates, sight) };
    top[rng.random_range(0..top.len())]
}

/// The spawns with the best weight less their sight penalty, given each
/// spawn's weight without it. The penalty only ever lowers a weight, so
/// candidates are tried best first and the (raycasting) penalty is only
/// worked out while one could still win.
fn best_spawns(candidates: &mut [(f32, SpawnPoint)], penalty: impl Fn(&SpawnPoint) -> f32) -> Vec<SpawnPoint> {
    candidates.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
    let mut best = f32::MIN;
    let mut top = Vec::new();
    for (base, s) in candidates.iter() {
        if *base < best {
            break;
        }
        let w = base - penalty(s);
        if w > best {
            best = w;
            top.clear();
        }
        if w >= best {
            top.push(*s);
        }
    }
    top
}

#[cfg(test)]
mod spawn_tests {
    use super::*;

    /// The bounded search picks exactly the spawns a full evaluation does.
    #[test]
    fn best_spawns_matches_full_evaluation() {
        let mut seed = 0x9e37_79b9_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for layout in 0..200 {
            let count = 1 + (next() % 40) as usize;
            // Coarse weights so ties happen; penalties in whole sightings.
            let spawns: Vec<(f32, SpawnPoint, f32)> = (0..count)
                .map(|i| {
                    let base = (next() % 8) as f32 * 1000.0;
                    let seen = if layout % 3 == 0 { 0.0 } else { (next() % 3) as f32 * DANGER_PENALTY };
                    (base, SpawnPoint { pos: Vec3::X * i as f32, yaw: 0.0, kind: SpawnKind::Tdm }, seen)
                })
                .collect();
            let penalty = |s: &SpawnPoint| spawns[s.pos.x as usize].2;
            let full: Vec<f32> = spawns.iter().map(|(b, s, _)| b - penalty(s)).collect();
            let best = full.iter().copied().fold(f32::MIN, f32::max);
            let mut want: Vec<usize> = (0..count).filter(|&i| full[i] >= best).collect();
            let mut candidates: Vec<(f32, SpawnPoint)> = spawns.iter().map(|(b, s, _)| (*b, *s)).collect();
            let mut got: Vec<usize> = best_spawns(&mut candidates, penalty).iter().map(|s| s.pos.x as usize).collect();
            want.sort_unstable();
            got.sort_unstable();
            assert_eq!(got, want, "layout {layout}");
        }
    }
}
