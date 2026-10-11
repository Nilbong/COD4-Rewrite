//! The single-player campaign: CoD4's missions, from their own zones
//! (`cargoship.ff`, ...), read from the install like everything else.
//!
//! This is the first, native slice: a mission runs as a match
//! ([`GameState::InGame`]) with [`Campaign`] set, as Headquarters does. The
//! level's actor spawners become soldiers standing at their posts; one
//! wakes when it sees or hears the player (or a squadmate wakes) and fights
//! with the bot brain. The objectives follow the waypoints the level's
//! script gives them; reaching the last completes the mission, dying fails
//! it. Later the level's own GSC scripts drive all of this (see the plan in
//! the campaign notes); the mission table here is the stand-in.
//!
//! Debug: `COD4RW_CAMPAIGN=cargoship` (or `=cargoship,x,y,z,yaw` to start
//! elsewhere, CoD units and degrees) goes straight into a mission.

use crate::combat::{Dead, Health, Killed, Pawn, Team};
use crate::modes::GameMode;
use crate::player::LocalPlayer;
use crate::state::GameState;
use crate::tdm::MatchConfig;
use crate::world::{MapInfo, SpawnKind, SpawnPoint};
use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

mod hud;
mod missions;
mod script;
pub use missions::{MISSIONS, Mission};

pub struct CampaignPlugin;

impl Plugin for CampaignPlugin {
    fn build(&self, app: &mut App) {
        if let Ok(arg) = std::env::var("COD4RW_CAMPAIGN") {
            let mut parts = arg.split(',');
            let name = parts.next().unwrap_or_default().trim().to_owned();
            let mission = missions::find(&name).unwrap_or(&MISSIONS[0]);
            let v: Vec<f32> = parts.filter_map(|x| x.trim().parse().ok()).collect();
            app.insert_resource(crate::world::MapName(mission.map.into())).insert_resource(config()).insert_resource(Campaign::new(mission));
            if let [x, y, z, yaw] = v[..] {
                app.insert_resource(StartAt([x, y, z], yaw));
            }
        }
        app.add_systems(OnEnter(GameState::InGame), enter.after(crate::world::load_map).in_set(crate::state::Setup::Content))
            .add_systems(OnEnter(GameState::InGame), spawn_actors.in_set(crate::state::Setup::Spawn))
            .add_systems(OnExit(GameState::InGame), leave)
            .add_systems(Update, (wake, the_dead, objectives, finish, shots).chain().run_if(active));
        hud::build(app);
        script::build(app);
        // Debug: `COD4RW_CAMPAIGN_GOD=1`: the player can't be hurt (shots
        // of a fight that last).
        if std::env::var_os("COD4RW_CAMPAIGN_GOD").is_some() {
            app.add_systems(PreUpdate, (|mut h: Query<&mut Health, With<LocalPlayer>>| {
                for mut h in &mut h {
                    h.current = 1.0e6;
                }
            })
            .run_if(active))
            .add_systems(PostUpdate, (|mut h: Query<&mut Health, With<LocalPlayer>>| {
                for mut h in &mut h {
                    h.current = crate::combat::max_health();
                }
            })
            .run_if(active));
        }
    }
}

/// A mission being played: which, and how it's going.
#[derive(Resource)]
pub struct Campaign {
    pub mission: &'static Mission,
    /// The objective being worked on (index into the mission's), and its
    /// waypoint.
    pub objective: usize,
    pub waypoint: usize,
    pub started: Option<f32>,
    pub kills: u32,
    pub ended: Option<(Outcome, f32)>,
    /// The level's own script runs the mission ([`script`]): its
    /// objectives (by number: state, localized text, where), the current
    /// one, and its messages (with when they came).
    pub scripted: bool,
    pub script_objectives: std::collections::BTreeMap<i32, (String, String, Option<[f32; 3]>)>,
    pub current_objective: i32,
    pub messages: Vec<(String, f32)>,
    /// The use trigger the player stands in: its hint.
    pub use_hint: Option<String>,
}

impl Campaign {
    pub fn new(mission: &'static Mission) -> Self {
        Campaign {
            mission,
            objective: 0,
            waypoint: 0,
            started: None,
            kills: 0,
            ended: None,
            scripted: std::env::var("COD4RW_CAMPAIGN_SCRIPT").is_ok_and(|v| v != "0"),
            script_objectives: Default::default(),
            current_objective: -1,
            messages: Vec::new(),
            use_hint: None,
        }
    }

    /// The objective being worked on: its localized text (or key) and where
    /// its marker is, CoD units.
    pub fn current(&self) -> Option<(&str, &str, Option<[f32; 3]>)> {
        if self.scripted {
            let active = |s: &str| s == "active" || s == "current";
            let (_, (_, text, at)) = self
                .script_objectives
                .iter()
                .find(|(i, o)| **i == self.current_objective && active(&o.0))
                .or_else(|| self.script_objectives.iter().find(|(_, o)| active(&o.0)))?;
            return Some((text.as_str(), "", *at));
        }
        let o = self.mission.objectives.get(self.objective)?;
        Some((o.text, o.english, o.waypoints.get(self.waypoint).copied()))
    }

    /// Where the current objective's marker is, CoD units.
    pub fn marker(&self) -> Option<[f32; 3]> {
        self.ended.is_none().then(|| self.current()?.2).flatten()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    Failed,
}

/// `COD4RW_CAMPAIGN`'s start instead of the mission's.
#[derive(Resource)]
struct StartAt([f32; 3], f32);

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Is a mission being played?
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// The "match" a mission runs as: the player alone on the Allies (the
/// level's soldiers come from its spawners), no limits.
pub fn config() -> MatchConfig {
    MatchConfig {
        bots: [Vec::new(), Vec::new()],
        player_team: Team::Allies,
        time_limit: f32::INFINITY,
        score_limit: u32::MAX,
        bot_skill: 0.5,
        mode: GameMode::Tdm,
        hardcore: false,
    }
}

/// A campaign soldier's own models (body, head), worn instead of a
/// multiplayer character ([`crate::thirdperson`]).
#[derive(Component, Clone)]
pub struct BodyModels(pub Vec<String>);

/// One of the level's soldiers, from its actor spawner.
#[derive(Component)]
pub struct Actor {
    /// The spawner's `targetname`: the squad it wakes with.
    pub(super) group: String,
    /// Fighting (the bot brain has it), or still at its post.
    pub(super) awake: bool,
}

/// The mission's start is every kind of spawn (SP levels have no MP
/// spawns), and the player's weapons are the mission's.
fn enter(
    campaign: Option<Res<Campaign>>,
    at: Option<Res<StartAt>>,
    content: Option<Res<crate::content::Content>>,
    mut map: ResMut<MapInfo>,
    mut choice: ResMut<crate::loadout::ClassChoice>,
) {
    ACTIVE.store(campaign.is_some(), Ordering::Relaxed);
    let Some(campaign) = campaign else { return };
    let m = campaign.mission;
    let (pos, yaw) = at.map_or((m.start, m.yaw), |a| (a.0, a.1));
    let spawn = |kind| SpawnPoint { pos: crate::units::pos(pos), yaw: crate::units::yaw_from_cod_degrees(yaw), kind };
    use SpawnKind::*;
    map.spawns = [Tdm, AlliesStart, AxisStart, Dm, Dom, DomAlliesStart, DomAxisStart, SdAttacker, SdDefender, SabAllies, SabAxis, SabAlliesStart, SabAxisStart]
        .into_iter()
        .map(spawn)
        .collect();
    // The soldiers' posts too, as Sabotage's spawns (never played here):
    // the bots' nav graph floods from the spawns, and the ship's insides
    // are behind doors.
    let ents = content.as_ref().and_then(|c| c.map().map_ents()).map(|e| iw3::ents::parse(&e.entity_string)).unwrap_or_default();
    map.spawns.extend(ents.iter().filter(|e| e.classname().to_ascii_lowercase().starts_with("actor_")).filter_map(|e| {
        Some(SpawnPoint { pos: crate::units::pos(e.origin()?), yaw: 0.0, kind: SabAxis })
    }));
    choice.next[0] = Some(m.class());
    info!("campaign: {} ({})", m.title, m.map);
}

/// Every enemy actor spawner becomes a soldier at its post, armed with the
/// spawner's weapon, frozen until it wakes ([`wake`]).
fn spawn_actors(
    mut commands: Commands,
    campaign: Option<Res<Campaign>>,
    content: Res<crate::content::Content>,
    assets: Res<crate::combat::PawnAssets>,
) {
    if campaign.as_ref().is_none_or(|c| c.scripted) {
        return;
    }
    let ents = content.map().map_ents().map(|e| iw3::ents::parse(&e.entity_string)).unwrap_or_default();
    // The enemies' characters, from the level's character scripts (their
    // `setModel` body and `attach`ed head).
    let characters: Vec<Vec<String>> = content
        .map()
        .assets
        .iter()
        .filter_map(|a| match a {
            iw3::zone::Asset::RawFile(r) if r.name.starts_with("character/") && !r.name.contains("_ally") => Some(String::from_utf8_lossy(&r.data).into_owned()),
            _ => None,
        })
        .map(|src| character_models(&src))
        .filter(|m| m.first().is_some_and(|b| b.contains("spetsnaz") || b.contains("merc") || b.contains("opforce") || b.contains("arab") || b.contains("ru_")))
        .collect();
    let mut count = 0;
    for e in &ents {
        let class = e.classname().to_ascii_lowercase();
        let Some(rest) = class.strip_prefix("actor_enemy_") else { continue };
        let Some(origin) = e.origin() else { continue };
        let group = e.get("targetname").unwrap_or_default().to_owned();
        // Left-over spawners the level doesn't use any more.
        if group.ends_with("_old") {
            continue;
        }
        let gun = missions::actor_gun(rest);
        let spawn = SpawnPoint {
            pos: crate::units::pos(origin),
            yaw: crate::units::yaw_from_cod_degrees(e.angles()[1]),
            kind: SpawnKind::Tdm,
        };
        let name = format!("{} {}", missions::rank_name(count), count + 1);
        let pawn = crate::combat::spawn_pawn(&mut commands, &assets, &name, Team::Axis, &spawn);
        let mut mover = crate::movement::Mover::default();
        mover.on_ground = true;
        commands.entity(pawn).insert((
            Actor { group, awake: false },
            crate::movement::Frozen,
            mover,
            crate::loadout::PawnClass(missions::actor_class(gun)),
        ));
        if !characters.is_empty() {
            commands.entity(pawn).insert(BodyModels(characters[count % characters.len()].clone()));
        }
        count += 1;
    }
    info!("campaign: {count} soldiers at their posts");
}

/// A character script's models: its `setModel` and `attach`es.
pub fn character_models(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for call in ["setmodel(", "attach("] {
        let lower = src.to_ascii_lowercase();
        let mut from = 0;
        while let Some(i) = lower[from..].find(call) {
            let at = from + i + call.len();
            if let Some(q) = src[at..].find('"') {
                let start = at + q + 1;
                if let Some(end) = src[start..].find('"') {
                    out.push(src[start..start + end].to_owned());
                }
            }
            from = at;
        }
    }
    out
}

/// Within this (CoD units) a soldier who can see the player wakes.
const SIGHT: f32 = 1400.0;
/// Within this one wakes whatever (hearing the player move about).
const HEAR: f32 = 300.0;
/// A waking soldier wakes his squad within this, and anyone at all this near.
const SQUAD: f32 = 1800.0;
const NEAR: f32 = 500.0;
/// Gunfire (the player's) is heard this far.
const GUNFIRE: f32 = 1200.0;

/// Soldiers at their posts wake when the player comes into sight, makes
/// noise nearby, or hurts them; their squad wakes with them. Awake, the
/// bot brain fights.
#[allow(clippy::type_complexity)]
fn wake(
    mut commands: Commands,
    spatial: avian3d::prelude::SpatialQuery,
    player: Query<(&Transform, &crate::weapons::WeaponState), (With<LocalPlayer>, Without<Dead>)>,
    mut actors: Query<(Entity, &mut Actor, &Transform, &Health), Without<Dead>>,
    mut shots: Local<u32>,
) {
    let Ok((ptf, weapon)) = player.single() else { return };
    let me = ptf.translation;
    let eye = me + Vec3::Y * crate::units::u(60.0);
    let fired = weapon.shots_fired_total != std::mem::replace(&mut *shots, weapon.shots_fired_total);
    // Debug: `COD4RW_CAMPAIGN_WAKE=1` wakes everyone at once.
    let all = std::env::var_os("COD4RW_CAMPAIGN_WAKE").is_some();
    let skill = crate::tune::get("campaign.skill", 0.45);
    let sight = crate::units::u(crate::tune::get("campaign.wake_sight", SIGHT));
    let filter = crate::collision::sight_filter();
    let mut woken: Vec<(Vec3, String)> = Vec::new();
    for (_, actor, tf, health) in &actors {
        if actor.awake {
            continue;
        }
        let at = tf.translation;
        let d = at.distance(me);
        let hurt = health.current < crate::combat::max_health();
        let heard = d < crate::units::u(HEAR) || (fired && d < crate::units::u(GUNFIRE));
        let seen = d < sight && {
            let from = at + Vec3::Y * crate::units::u(60.0);
            Dir3::new(eye - from).is_ok_and(|dir| spatial.cast_ray(from, dir, from.distance(eye), true, &filter).is_none())
        };
        if hurt || heard || seen || all {
            woken.push((at, actor.group.clone()));
        }
    }
    if woken.is_empty() {
        return;
    }
    let mut count = 0;
    for (e, mut actor, tf, _) in &mut actors {
        if actor.awake {
            continue;
        }
        let at = tf.translation;
        let wakes = woken.iter().any(|(w, g)| {
            let d = w.distance(at);
            d < crate::units::u(NEAR) || (!g.is_empty() && *g == actor.group && d < crate::units::u(SQUAD))
        });
        if wakes {
            actor.awake = true;
            count += 1;
            commands
                .entity(e)
                .remove::<crate::movement::Frozen>()
                .insert((crate::bots::Bot::new(skill, None), crate::weapons::WeaponInput::default()));
        }
    }
    if count > 0 {
        info!("campaign: {count} soldiers woke ({:?})", woken.first().map(|w| w.1.as_str()).unwrap_or_default());
    }
}

/// The dead stay dead: the level's soldiers, and the player (that fails
/// the mission).
fn the_dead(
    time: Res<Time>,
    mut campaign: ResMut<Campaign>,
    mut killed: MessageReader<Killed>,
    mut dead: Query<(&mut Dead, Has<LocalPlayer>), (Added<Dead>, Without<crate::loadout::AwaitingClass>)>,
    local: Query<Entity, With<LocalPlayer>>,
    pawns: Query<&Pawn>,
) {
    let me = local.single().ok();
    for k in killed.read() {
        if k.attacker.is_some() && k.attacker == me && k.victim != k.attacker.unwrap() && pawns.get(k.victim).is_ok_and(|p| p.team == Team::Axis) {
            campaign.kills += 1;
        }
    }
    for (mut d, local) in &mut dead {
        d.respawn_at = f32::INFINITY;
        if local && campaign.ended.is_none() {
            campaign.ended = Some((Outcome::Failed, time.elapsed_secs()));
            info!("campaign: mission failed");
        }
    }
}

/// The objective's marker moves on as the player reaches each waypoint (or
/// any later one); past an objective's last, the next objective; past the
/// last objective, the mission is complete.
fn objectives(time: Res<Time>, mut campaign: ResMut<Campaign>, player: Query<&Transform, (With<LocalPlayer>, Without<Dead>)>) {
    let now = time.elapsed_secs();
    campaign.started.get_or_insert(now);
    if campaign.ended.is_some() {
        return;
    }
    let Ok(tf) = player.single() else { return };
    if campaign.scripted {
        return;
    }
    let me = crate::units::to_cod(tf.translation);
    let m = campaign.mission;
    let Some(obj) = m.objectives.get(campaign.objective) else { return };
    let reach = crate::tune::get("campaign.waypoint_reach", 220.0);
    let reached = obj.waypoints.iter().enumerate().skip(campaign.waypoint).filter(|(_, w)| {
        let flat = ((me[0] - w[0]).powi(2) + (me[1] - w[1]).powi(2)).sqrt();
        flat < reach && (me[2] - w[2]).abs() < 160.0
    });
    let Some((i, _)) = reached.last() else { return };
    campaign.waypoint = i + 1;
    if campaign.waypoint >= obj.waypoints.len() {
        info!("campaign: objective {} done", campaign.objective + 1);
        campaign.objective += 1;
        campaign.waypoint = 0;
        if campaign.objective >= m.objectives.len() {
            campaign.ended = Some((Outcome::Complete, now));
            info!("campaign: mission complete");
        }
    }
}

/// Once it's over everyone stands still; a key after a moment leaves for
/// the menus (or, run from the command line, plays the mission again).
#[allow(clippy::too_many_arguments)]
fn finish(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    fe: Option<Res<crate::ui::Frontend>>,
    campaign: Res<Campaign>,
    pawns: Query<Entity, (With<Pawn>, Without<crate::movement::Frozen>)>,
    mut left: Local<bool>,
) {
    let Some((_, at)) = campaign.ended else {
        *left = false;
        return;
    };
    for e in &pawns {
        commands.entity(e).insert(crate::movement::Frozen);
    }
    let pressed = keys.any_just_pressed([KeyCode::Enter, KeyCode::Space, KeyCode::Escape])
        || pads.iter().any(|p| p.just_pressed(GamepadButton::South) || p.just_pressed(GamepadButton::Start));
    if time.elapsed_secs() - at < hud::END_DELAY || !pressed || std::mem::replace(&mut *left, true) {
        return;
    }
    if fe.is_some_and(|f| f.from_menus()) {
        info!("campaign: back to the menus");
        commands.insert_resource(crate::tdm::MatchOver);
    } else {
        *left = false;
    }
}

fn leave(mut commands: Commands) {
    ACTIVE.store(false, Ordering::Relaxed);
    commands.remove_resource::<Campaign>();
}

/// Debug: `COD4RW_CAMPAIGN_SHOTS=<dir>` saves a frame every
/// `COD4RW_CAMPAIGN_EVERY` seconds (3) of the mission, then the end screen
/// (`end.png`) once it's up, and exits; or exits at 90 s.
fn shots(
    mut commands: Commands,
    time: Res<Time>,
    campaign: Res<Campaign>,
    mut next: Local<f32>,
    mut taken: Local<u32>,
    mut done: Local<bool>,
    mut exit: MessageWriter<AppExit>,
    mut player: Query<(&Transform, &mut crate::movement::ViewAngles), With<LocalPlayer>>,
    soldiers: Query<&Transform, (With<Pawn>, Without<LocalPlayer>, Without<Dead>)>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let Ok(dir) = std::env::var("COD4RW_CAMPAIGN_SHOTS") else { return };
    // Looking at the nearest soldier in sight of nothing in particular:
    // the nearest one.
    if let Ok((me, mut view)) = player.single_mut()
        && let Some(t) = soldiers.iter().min_by(|a, b| a.translation.distance(me.translation).total_cmp(&b.translation.distance(me.translation)))
    {
        let d = t.translation + Vec3::Y * crate::units::u(45.0) - (me.translation + Vec3::Y * crate::units::u(60.0));
        view.yaw = (-d.x).atan2(-d.z);
        view.pitch = (d.y / Vec2::new(d.x, d.z).length().max(0.01)).atan();
    }
    let dir = std::path::PathBuf::from(dir);
    let Some(t0) = campaign.started else { return };
    let now = time.elapsed_secs();
    let every = std::env::var("COD4RW_CAMPAIGN_EVERY").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let mut shot = |name: String| {
        std::fs::create_dir_all(&dir).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.join(name)));
    };
    match campaign.ended {
        Some((_, at)) if now - at > hud::END_DELAY + 1.0 => {
            if !std::mem::replace(&mut *done, true) {
                shot("end.png".into());
            } else if now - at > hud::END_DELAY + 2.0 {
                exit.write(AppExit::Success);
            }
        }
        _ if now - t0 > 90.0 => {
            exit.write(AppExit::Success);
        }
        Some(_) => {}
        None if now - t0 >= 0.5 && now >= *next => {
            *next = now + every;
            shot(format!("t{:03}.png", *taken));
            *taken += 1;
        }
        None => {}
    }
}
