//! CoD4's hardpoints (`_hardpoints.gsc`): 3, 5 and 7 kills since your last
//! death earn a UAV, an airstrike and a helicopter (none on maps without
//! helicopter paths); and Modern Warfare 2's care package at 4 and sentry
//! gun at 6 ([`carepackage`], [`sentry`]). Unlike CoD4 (one at a time, a
//! better one replacing a worse) every one earned is held, each kept through
//! death until it's used: [`HardpointInput`], which the player's keys set (5
//! UAV, 6 airstrike, 7 helicopter, 8 care package, 9 sentry), or a pad's
//! d-pad picker (right opens it, up and down choose, right calls in, left
//! closes), and bots set themselves. Each hardpoint lives in its own module;
//! what the local player should hear about comes out as [`StreakNotice`]s
//! for the HUD, and what they're asked to do (place the sentry, open a
//! crate) as [`prompt`]s.

pub mod airstrike;
pub mod carepackage;
pub mod hands;
pub mod helicopter;
mod models;
pub mod sentry;
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
                    publish_keys,
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
        hands::build(app);
        carepackage::build(app);
        sentry::build(app);
        test::register(app);
    }
}

/// A hardpoint, worst first. The care package and the sentry gun are
/// Modern Warfare 2's ([`carepackage`], [`sentry`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hardpoint {
    Uav,
    CarePackage,
    Airstrike,
    Sentry,
    Helicopter,
}

impl Hardpoint {
    /// The kill streak that earns it.
    pub fn kills(self) -> u32 {
        match self {
            Hardpoint::Uav => 3,
            Hardpoint::CarePackage => 4,
            Hardpoint::Airstrike => 5,
            Hardpoint::Sentry => 6,
            Hardpoint::Helicopter => 7,
        }
    }

    /// Its weapon's `hudIcon` (`radar_mp`, `airstrike_mp`, `helicopter_mp`;
    /// CoD4 has none for MW2's two, so they keep the HD ones).
    pub fn icon(self) -> &'static str {
        match self {
            Hardpoint::Uav => "compass_objpoint_satallite",
            Hardpoint::Airstrike => "compass_objpoint_airstrike",
            Hardpoint::Helicopter => "compass_objpoint_helicopter",
            Hardpoint::CarePackage | Hardpoint::Sentry => self.hud_icon(),
        }
    }

    /// The HUD's icon (`crate::ui`'s embedded `ks:` pictures).
    pub fn hud_icon(self) -> &'static str {
        match self {
            Hardpoint::Uav => "ks:uav",
            Hardpoint::CarePackage => "ks:care_package",
            Hardpoint::Airstrike => "ks:airstrike",
            Hardpoint::Sentry => "ks:sentry_gun",
            Hardpoint::Helicopter => "ks:helicopter",
        }
    }

    /// `level.hardpointHints`: "Press [{+actionslot 4}] for RADAR.".
    pub fn hint(self) -> &'static str {
        match self {
            Hardpoint::Uav => "MP_EARNED_RADAR",
            Hardpoint::CarePackage => "MP_EARNED_CAREPACKAGE",
            Hardpoint::Airstrike => "MP_EARNED_AIRSTRIKE",
            Hardpoint::Sentry => "MP_EARNED_SENTRY",
            Hardpoint::Helicopter => "MP_EARNED_HELICOPTER",
        }
    }

    /// `level.hardpointInforms`: the sound on earning it (MW2's for its
    /// two, from its sounds under `iw4/`).
    fn inform(self) -> &'static str {
        match self {
            Hardpoint::Uav => "mp_killstreak_radar",
            Hardpoint::CarePackage => "iw4/mp_killstreak_carepackage",
            Hardpoint::Airstrike => "mp_killstreak_jet",
            Hardpoint::Sentry => "iw4/mp_killstreak_sentrygun",
            Hardpoint::Helicopter => "mp_killstreak_heli",
        }
    }

    /// `game["dialog"][hardpoint]`: the announcer's line on earning it, in
    /// the side's `voice` (MW2's lines for its two, where its announcers
    /// share CoD4's prefix).
    fn leader(self, voice: &str) -> String {
        match self {
            Hardpoint::Uav => format!("{voice}_1mc_uavrecon"),
            Hardpoint::CarePackage => format!("iw4/{}_1mc_achieve_carepackage", voice.to_ascii_lowercase()),
            Hardpoint::Airstrike => format!("{voice}_1mc_airstrike"),
            Hardpoint::Sentry => format!("iw4/{}_1mc_achieve_sentrygun", voice.to_ascii_lowercase()),
            Hardpoint::Helicopter => format!("{voice}_1mc_helisupport"),
        }
    }

    fn for_streak(kills: u32) -> Option<Hardpoint> {
        Hardpoint::ALL.into_iter().find(|h| h.kills() == kills)
    }
}

impl Hardpoint {
    pub const ALL: [Hardpoint; 5] = [Hardpoint::Uav, Hardpoint::CarePackage, Hardpoint::Airstrike, Hardpoint::Sentry, Hardpoint::Helicopter];

    /// How the HUD names it.
    pub fn name(self) -> &'static str {
        match self {
            Hardpoint::Uav => "UAV",
            Hardpoint::CarePackage => "Care Package",
            Hardpoint::Airstrike => "Airstrike",
            Hardpoint::Sentry => "Sentry Gun",
            Hardpoint::Helicopter => "Helicopter",
        }
    }

    /// Its key's place among the kill streak keys: 5 UAV, 6 airstrike, 7
    /// helicopter (as before MW2's two came), 8 care package, 9 sentry.
    fn slot(self) -> usize {
        match self {
            Hardpoint::Uav => 0,
            Hardpoint::Airstrike => 1,
            Hardpoint::Helicopter => 2,
            Hardpoint::CarePackage => 3,
            Hardpoint::Sentry => 4,
        }
    }

    /// The key (as the game reads it) that calls it in.
    fn key(self) -> KeyCode {
        [KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9][self.slot()]
    }
}

/// The hardpoints a pawn holds.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Held(u8);

impl Held {
    pub fn has(self, h: Hardpoint) -> bool {
        self.0 & (1 << h as u8) != 0
    }
    pub fn add(&mut self, h: Hardpoint) {
        self.0 |= 1 << h as u8;
    }
    pub fn remove(&mut self, h: Hardpoint) {
        self.0 &= !(1 << h as u8);
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    /// Those held, worst first.
    pub fn list(self) -> Vec<Hardpoint> {
        Hardpoint::ALL.into_iter().filter(|&h| self.has(h)).collect()
    }
    pub fn best(self) -> Option<Hardpoint> {
        self.list().pop()
    }
}

/// A pawn's kills since it last died, the hardpoints it holds, and a pad
/// player's picker: open on one of [`Held::list`] (by index), since when.
#[derive(Component, Default, Debug)]
pub struct Killstreak {
    pub kills: u32,
    pub held: Held,
    pub picker: Option<(usize, f32)>,
}

/// Use a held hardpoint now: `item`, or the best held; an airstrike also
/// wants where (Bevy space; the player's is picked on the map, bots give
/// one).
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct HardpointInput {
    pub use_now: bool,
    pub item: Option<Hardpoint>,
    pub target: Option<Vec3>,
}

/// The keys a pad presses for the picker ([`crate::gamepad`]): keys no
/// keyboard binding uses.
pub const PAD_PICK: KeyCode = KeyCode::F13;
pub const PAD_UP: KeyCode = KeyCode::F14;
pub const PAD_DOWN: KeyCode = KeyCode::F15;
pub const PAD_CLOSE: KeyCode = KeyCode::F16;
/// The picker closes by itself after this long untouched.
const PICKER_TIMEOUT: f32 = 6.0;

static PICKER_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether a player's picker is open (the d-pad's up, down and left work
/// it then).
pub fn picker_open() -> bool {
    PICKER_OPEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// The keyboard's kill streak keys' names (`5`, `6`, `7` unless rebound),
/// for the HUD.
static KEYS: std::sync::Mutex<[String; 5]> = std::sync::Mutex::new([const { String::new() }; 5]);

pub fn key_name(h: Hardpoint) -> String {
    KEYS.lock().ok().map(|k| k[h.slot()].clone()).filter(|k| !k.is_empty()).unwrap_or_else(|| (5 + h.slot()).to_string())
}

fn publish_keys(bindings: Option<Res<crate::bindings::Bindings>>) {
    use crate::bindings::Action;
    let Some(b) = bindings.filter(|b| b.is_changed()) else { return };
    crate::bindings::publish_key_names(&b);
    if let Ok(mut k) = KEYS.lock() {
        *k = [Action::Killstreak1, Action::Killstreak2, Action::Killstreak3, Action::Killstreak4, Action::Killstreak5].map(|a| b.key_name(a));
    }
}

/// A line under the crosshair for each local player (by slot), and how
/// far along what it asks for is: placing the sentry, opening a crate.
pub(crate) struct SlotPrompts(std::sync::Mutex<[Option<(String, Option<f32>)>; 4]>);

impl SlotPrompts {
    const fn new() -> SlotPrompts {
        SlotPrompts(std::sync::Mutex::new([None, None, None, None]))
    }
    fn set(&self, slot: usize, prompt: Option<(String, Option<f32>)>) {
        if let (Ok(mut p), true) = (self.0.lock(), slot < 4) {
            p[slot] = prompt;
        }
    }
    fn get(&self, slot: usize) -> Option<(String, Option<f32>)> {
        self.0.lock().ok().and_then(|p| p.get(slot).cloned().flatten())
    }
}

static PROMPTS: SlotPrompts = SlotPrompts::new();
static CRATE_PROMPTS: SlotPrompts = SlotPrompts::new();

/// What a local player's kill streaks ask of them now ("Hold [F] for
/// Sentry Gun"), and how far along it is (0 to 1), for the HUD.
pub fn prompt(slot: usize) -> Option<(String, Option<f32>)> {
    PROMPTS.get(slot).or_else(|| CRATE_PROMPTS.get(slot))
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
    /// The player's team called an airstrike on them.
    AirstrikeNear,
    /// The player opened a care package: the kill streak in it, or ammo.
    Opened { item: Option<Hardpoint> },
}

fn reset(
    mut commands: Commands,
    mut radar: ResMut<uav::Radar>,
    mut strikes: ResMut<airstrike::Airstrikes>,
    mut calls: ResMut<helicopter::Calls>,
    helis: Query<Entity, With<helicopter::Helicopter>>,
    (mut drops, mut sentries): (ResMut<carepackage::Calls>, ResMut<sentry::Calls>),
) {
    *radar = uav::Radar::default();
    *strikes = airstrike::Airstrikes::default();
    calls.0.clear();
    drops.0.clear();
    sentries.0.clear();
    for e in &helis {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<airstrike::Selecting>();
}

/// A hardpoint weapon's kill icon and its width over height, for the kill
/// feed.
/// A kill streak's weapon as the kill feed names it ("Airstrike",
/// "Helicopter"); other weapons as they are.
pub fn feed_name(weapon: &str) -> &str {
    if weapon == airstrike::WEAPON {
        "Airstrike"
    } else if weapon == helicopter::WEAPON || helicopter::ROCKETS.contains(&weapon) {
        "Helicopter"
    } else if weapon == sentry::WEAPON {
        "Sentry Gun"
    } else if weapon == carepackage::CRUSH {
        "Care Package"
    } else {
        weapon
    }
}

pub fn kill_icon(weapon: &str) -> Option<(&'static str, f32)> {
    match weapon {
        airstrike::WEAPON => Some(("death_airstrike", 4.0)),
        w if w == helicopter::WEAPON || helicopter::ROCKETS.contains(&w) => Some(("death_helicopter", 4.0)),
        sentry::WEAPON => Some(("ks:sentry_gun", 1.0)),
        carepackage::CRUSH => Some(("ks:care_package", 1.0)),
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

/// Every pawn gets a streak and a hardpoint input. Debug aid:
/// `COD4RW_STREAKS_GIVE=1` hands the player all of them to start with, or
/// those named (`uav,carepackage,sentrygun`).
fn equip(mut commands: Commands, pawns: Query<(Entity, Has<LocalPlayer>), (With<Pawn>, Without<Killstreak>)>) {
    let give = std::env::var("COD4RW_STREAKS_GIVE").ok();
    for (e, local) in &pawns {
        let mut streak = Killstreak::default();
        if let Some(give) = give.as_deref().filter(|_| local) {
            Hardpoint::ALL.into_iter().filter(|h| give == "1" || give.split(',').any(|n| n.eq_ignore_ascii_case(&h.name().replace(' ', "")) || n.eq_ignore_ascii_case(h.name()))).for_each(|h| streak.held.add(h));
        }
        commands.entity(e).insert((streak, HardpointInput::default()));
    }
}

/// Count kills of enemies by the living; dying ends a streak. A streak of
/// 3, 5 or 7 gives its hardpoint (`giveHardpointItemForStreak`), unless
/// it's already held.
#[allow(clippy::too_many_arguments)]
fn count_streaks(
    time: Res<Time>,
    mut killed: MessageReader<Killed>,
    mut pawns: Query<(&Pawn, &mut Killstreak, Has<LocalPlayer>, Option<Ref<Dead>>)>,
    mut notices: MessageWriter<StreakNotice>,
    mut sfx: ResMut<Sfx>,
    sides: Option<Res<Sides>>,
    heli_paths: Res<helicopter::HeliPaths>,
) {
    // Dying, or a new round's respawn (the script's `map_restart` keeps
    // only the held hardpoint), ends a streak.
    for (_, mut s, _, dead) in &mut pawns {
        if dead.is_some_and(|d| d.is_added()) {
            s.kills = 0;
        }
    }
    for k in killed.read() {
        let victim_team = pawns.get(k.victim).ok().map(|(p, ..)| p.team);
        if let Ok((_, mut s, ..)) = pawns.get_mut(k.victim) {
            s.kills = 0;
        }
        let Some(attacker) = k.attacker.filter(|&a| a != k.victim) else { continue };
        let Ok((pawn, mut streak, local, dead)) = pawns.get_mut(attacker) else { continue };
        // Kills by a kill streak (the airstrike, the helicopter, the sentry,
        // a crate) don't count towards the next, as in CoD4.
        if victim_team.is_none_or(|t| t == pawn.team) || dead.is_some() || kill_icon(k.weapon).is_some() {
            continue;
        }
        streak.kills += 1;
        let Some(item) = Hardpoint::for_streak(streak.kills) else { continue };
        if streak.held.has(item) || item == Hardpoint::Helicopter && !heli_paths.available() {
            continue;
        }
        streak.held.add(item);
        if local {
            notices.write(StreakNotice::Earned { streak: streak.kills, item });
            sfx.play(item.inform(), None);
            if let Some(sides) = &sides {
                let now = time.elapsed_secs();
                sfx.play_later(item.leader(&sides.of(pawn.team).voice), None, now, 1.0);
            }
        }
    }
}

/// The player's kill streak keys: 5 UAV, 6 airstrike, 7 helicopter; or a
/// pad's picker ([`PAD_PICK`] opens it on the best held, [`PAD_UP`] and
/// [`PAD_DOWN`] move along, [`PAD_PICK`] again calls that one in,
/// [`PAD_CLOSE`] or a few idle seconds put it away). While an airstrike's
/// spot is being picked, its key or [`PAD_PICK`] puts the map away.
#[allow(clippy::type_complexity)]
fn player_input(
    time: Res<Time>,
    spatial: avian3d::prelude::SpatialQuery,
    selecting: Option<Res<airstrike::Selecting>>,
    mut players: Query<(
        Entity,
        &crate::splitscreen::PlayerInput,
        &mut HardpointInput,
        &mut Killstreak,
        &Transform,
        &crate::movement::Mover,
        &crate::movement::ViewAngles,
    )>,
) {
    let now = time.elapsed_secs();
    let mut open = false;
    for (entity, player, mut input, mut streak, tf, mover, view) in &mut players {
        if !player.live {
            streak.picker = None;
            continue;
        }
        let keys = &player.keys;
        let list = streak.held.list();
        let mut want = None;
        // A pad has no cursor on the map: its button calls it where the
        // player aims.
        let mut aimed = crate::splitscreen::active();
        for item in Hardpoint::ALL {
            if keys.just_pressed(item.key()) {
                want = Some(item);
                streak.picker = None;
            }
        }
        if keys.just_pressed(PAD_PICK) {
            if selecting.as_ref().is_some_and(|s| s.owner == entity) {
                want = Some(Hardpoint::Airstrike);
                aimed = true;
            } else if let Some((i, _)) = streak.picker.take() {
                want = list.get(i).copied();
            } else if !list.is_empty() {
                streak.picker = Some((list.len() - 1, now));
            }
        }
        if let Some((i, at)) = streak.picker {
            let n = list.len();
            if n == 0 || keys.just_pressed(PAD_CLOSE) || now - at > PICKER_TIMEOUT {
                streak.picker = None;
            } else {
                let mut i = i.min(n - 1);
                let mut at = at;
                if keys.just_pressed(PAD_UP) {
                    i = (i + 1).min(n - 1);
                    at = now;
                }
                if keys.just_pressed(PAD_DOWN) {
                    i = i.saturating_sub(1);
                    at = now;
                }
                streak.picker = Some((i, at));
            }
        }
        open |= streak.picker.is_some();
        let Some(item) = want.filter(|&h| streak.held.has(h)) else { continue };
        input.use_now = true;
        input.item = Some(item);
        // Splitscreen has no map to pick on (nor a pad a cursor on it): the
        // airstrike goes where the player aims (as far as 6000 units, onto
        // the ground below).
        if aimed && item == Hardpoint::Airstrike {
            let eye = mover.eye(tf.translation);
            let filter = crate::collision::sight_filter();
            let reach = crate::units::u(6000.0);
            let Ok(dir) = Dir3::new(view.forward()) else { continue };
            let aim = spatial.cast_ray(eye, dir, reach, true, &filter).map_or(eye + dir * reach, |h| eye + dir * h.distance);
            input.target = Some(airstrike::ground_below(&spatial, aim, tf.translation.y));
        }
    }
    PICKER_OPEN.store(open, std::sync::atomic::Ordering::Relaxed);
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
    (mut heli_calls, mut drops, mut sentries): (ResMut<helicopter::Calls>, ResMut<carepackage::Calls>, ResMut<sentry::Calls>),
    helis: Query<(), With<helicopter::Helicopter>>,
    mut awards: MessageWriter<crate::ui::progression::Award>,
    locals: Query<(&Transform, &Pawn), (With<LocalPlayer>, Without<Dead>)>,
) {
    let now = time.elapsed_secs();
    let me = local.single().ok().cloned();
    let me = me.as_ref();
    for (entity, pawn, mut streak, mut input, dead, is_local) in &mut pawns {
        let wanted = std::mem::take(&mut input.use_now);
        let target = input.target.take();
        let asked = input.item.take();
        let item = asked.filter(|&h| streak.held.has(h)).or_else(|| streak.held.best());
        let (Some(item), false, true) = (item, dead, wanted) else { continue };
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
            Hardpoint::Airstrike => match target {
                Some(at) => {
                    strikes.call(entity, at, now);
                    // (Called from the map: it's done.)
                    if selecting.as_ref().is_some_and(|s| s.owner == entity) {
                        commands.remove_resource::<airstrike::Selecting>();
                    }
                    // `doArtillery`: the caller's team within 562.5 units
                    // of the spot are told (not in hardcore).
                    if !crate::tdm::hardcore() {
                        for (tf, p) in &locals {
                            let d = (tf.translation - at).with_y(0.0).length();
                            if !hostile(p, pawn) && d <= crate::units::u(450.0 * 1.25) {
                                notices.write(StreakNotice::AirstrikeNear);
                            }
                        }
                    }
                    // `leaderDialog( "airstrike_inbound", team )`.
                    if let (Some(sides), Some(me)) = (&sides, me.filter(|me| !hostile(me, pawn))) {
                        sfx.play(format!("{}_1mc_friendlyair", sides.of(me.team).voice), None);
                    }
                    true
                }
                // The player picks the spot on the map; its key again puts
                // it away.
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
            // One at a time: a marker still to throw, or a sentry carried.
            Hardpoint::CarePackage if drops.0.contains(&entity) => false,
            Hardpoint::CarePackage => {
                drops.0.push(entity);
                true
            }
            Hardpoint::Sentry if sentries.0.contains(&entity) => false,
            Hardpoint::Sentry => {
                sentries.0.push(entity);
                true
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
            streak.held.remove(item);
            awards.write(crate::ui::progression::Award { pawn: entity, kind: crate::ui::progression::AwardKind::Hardpoint });
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
