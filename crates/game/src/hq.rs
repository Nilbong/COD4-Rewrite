//! Headquarters: somewhere to be between matches (CoD WWII's HQ), for now
//! the local player alone. It's the single-player F.N.G. level
//! (`killhouse`), from the hangar where Price briefs the SAS, walked in
//! third person: no weapons, combat HUD, bots, game mode or time limit.
//! Escape's menu has Create a Class, Supply Drops and Leave Headquarters
//! (`crate::ui`'s `hq_pause`).
//!
//! It runs as a match ([`GameState::InGame`]) with [`Headquarters`] set:
//! the main menu's entry starts it ([`config`]), and the parts of a match
//! that don't belong check [`active`].

use crate::combat::Team;
use crate::modes::GameMode;
use crate::splitscreen::PlayerInput;
use crate::state::GameState;
use crate::tdm::MatchConfig;
use crate::world::{MapInfo, SpawnKind, SpawnPoint};
use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct HqPlugin;

impl Plugin for HqPlugin {
    fn build(&self, app: &mut App) {
        // Debug aid: `COD4RW_HQ` goes straight to Headquarters (`=x,y,z,yaw`
        // in CoD units and degrees starts there instead).
        if let Ok(at) = std::env::var("COD4RW_HQ") {
            app.insert_resource(crate::world::MapName(MAP.into())).insert_resource(config()).insert_resource(Headquarters);
            let v: Vec<f32> = at.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if let [x, y, z, yaw] = v[..] {
                app.insert_resource(SpawnAt([x, y, z], yaw));
            }
        }
        app.add_systems(OnEnter(GameState::InGame), enter.after(crate::world::load_map).in_set(crate::state::Setup::Content))
            .add_systems(OnEnter(GameState::InGame), spawn_figures.in_set(crate::state::Setup::Spawn))
            .add_systems(OnExit(GameState::InGame), leave)
            .add_systems(Update, third_person.run_if(|| active()));
    }
}

/// The level: CoD4's single-player training (its zone, not mp_killhouse).
pub const MAP: &str = "killhouse";

/// Where the player starts (CoD units, and degrees): inside the hangar's
/// door, facing the corner where Price briefs the SAS (chalkboard, TVs).
const SPAWN: [f32; 3] = [-40.0, -1040.0, 16.0];
const SPAWN_YAW: f32 = 144.0;

/// `COD4RW_HQ`'s start instead of [`SPAWN`].
#[derive(Resource)]
struct SpawnAt([f32; 3], f32);

/// Set for a visit to Headquarters (inserted with its [`config`]).
#[derive(Resource)]
pub struct Headquarters;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Is the player in Headquarters?
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// The "match": nobody else, no game mode's ends.
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

/// The level has only the single-player start: every kind of spawn is ours.
fn enter(hq: Option<Res<Headquarters>>, at: Option<Res<SpawnAt>>, mut map: ResMut<MapInfo>) {
    ACTIVE.store(hq.is_some(), Ordering::Relaxed);
    if hq.is_none() {
        return;
    }
    let (pos, yaw) = at.map_or((SPAWN, SPAWN_YAW), |a| (a.0, a.1));
    let spawn = |kind| SpawnPoint { pos: crate::units::pos(pos), yaw: crate::units::yaw_from_cod_degrees(yaw), kind };
    use SpawnKind::*;
    map.spawns = [Tdm, AlliesStart, AxisStart, Dm, Dom, DomAlliesStart, DomAxisStart, SdAttacker, SdDefender, SabAllies, SabAxis, SabAlliesStart, SabAxisStart]
        .into_iter()
        .map(spawn)
        .collect();
    info!("headquarters: in {MAP}");
}

/// The SAS in black kit about the hangar (the level's actor spawners whose
/// models are whole bodies: Price, his team, a lookout on the gantry),
/// standing in their briefing idles with their guns. Not solid; they
/// don't move or react.
#[allow(clippy::too_many_arguments)]
fn spawn_figures(
    mut commands: Commands,
    hq: Option<Res<Headquarters>>,
    mut content: ResMut<crate::content::Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut camo_materials: ResMut<Assets<crate::gunmodel::CamoMaterial>>,
    mut camos: ResMut<crate::gunmodel::WorldCamos>,
) {
    use crate::models::{AnimPlayer, Skeleton, SpawnModel, spawn_model};
    if hq.is_none() {
        return;
    }
    let ents = content.map().map_ents().map(|e| iw3::ents::parse(&e.entity_string)).unwrap_or_default();
    let mut count = 0;
    for e in &ents {
        let class = e.classname();
        let (Some(model), Some(origin)) = (e.get("model"), e.origin()) else { continue };
        if !class.starts_with("actor_ally") || !class.contains("blackkit") || !model.starts_with("body_complete") {
            continue;
        }
        let Some(body) = content.model(model, &mut meshes, &mut materials, &mut images, &mut bindposes) else { continue };
        // Each in their own idle from the briefing.
        let idle = match e.get("script_noteworthy").unwrap_or_default() {
            "price" => "killhouse_sas_price_idle",
            "sas2" => "killhouse_sas_2_idle",
            "sas3" => "killhouse_sas_3_idle",
            _ => "killhouse_sas_1_idle",
        };
        // Their spawners' weapons, as the multiplayer guns.
        let gun = match () {
            _ if class.contains("winchester") => "winchester1200:",
            _ if class.contains("m4grenadier") => "m4:gl",
            _ if class.contains("mp5sd") => "mp5:silencer",
            _ => "m4:silencer",
        };
        // CoD models face +X, turned by the spawner's yaw.
        let owner = commands
            .spawn((
                Name::new(format!("hq figure {model}")),
                Transform::from_translation(crate::units::pos(origin)).with_rotation(Quat::from_rotation_y(e.angles()[1].to_radians())),
                Visibility::default(),
            ))
            .id();
        let mut skeleton = Skeleton::default();
        spawn_model(&mut commands, &mut skeleton, SpawnModel { model: &body, owner, attach_to: None, layers: None, shadows: true });
        let hand = skeleton.joint("tag_weapon_right");
        let mut assets = crate::gunmodel::GunAssets {
            meshes: &mut meshes,
            materials: &mut materials,
            images: &mut images,
            bindposes: &mut bindposes,
            camo_materials: &mut camo_materials,
        };
        let target = crate::gunmodel::GunTarget { owner, attach_to: hand, layers: None };
        crate::gunmodel::spawn_held_gun(&mut commands, &mut content, &mut camos.0, &mut assets, &mut skeleton, gun, 0, target);
        let mut player = AnimPlayer::default();
        if let Some(a) = content.anim(idle) {
            player.play(a, 0.0);
            // Not all in step.
            player.time = count as f32 * 0.7;
        }
        commands.entity(owner).insert((skeleton, player));
        count += 1;
    }
    info!("headquarters: {count} figures");
}

fn leave(mut commands: Commands, mut view: ResMut<crate::wardrobe::ThirdPerson>) {
    if ACTIVE.swap(false, Ordering::Relaxed) {
        view.0[0] = false;
    }
    commands.remove_resource::<Headquarters>();
}

/// Always behind the player.
fn third_person(mut view: ResMut<crate::wardrobe::ThirdPerson>) {
    if !view.0[0] {
        view.0[0] = true;
    }
}

/// No fighting: the fire, aim, reload, grenade, melee, equipment and
/// weapon switch inputs never reach the game, nor does the view toggle.
pub fn strip(input: &mut PlayerInput) {
    if !active() {
        return;
    }
    for key in [
        KeyCode::KeyG,
        KeyCode::KeyR,
        KeyCode::KeyV,
        KeyCode::KeyF,
        KeyCode::KeyB,
        KeyCode::KeyN,
        KeyCode::KeyI,
        KeyCode::F5,
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
    ] {
        input.keys.reset(key);
    }
    for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        input.mouse.reset(button);
    }
    input.scroll = 0.0;
}
