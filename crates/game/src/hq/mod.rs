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

mod ambient;
mod life;
mod reveal;
pub use reveal::{DEAL, FACE_DOWN, reveal_since, revealing};

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
            .add_systems(Update, (third_person, overcast, outdoor_exposure).run_if(|| active()))
            // Nothing hurts in Headquarters (a fall, say): through the
            // frame the player's health is out of reach of any damage, and
            // full again after.
            .add_systems(PreUpdate, (|mut h: Query<&mut crate::combat::Health, With<crate::player::LocalPlayer>>| {
                for mut h in &mut h {
                    h.current = UNHURT;
                }
            })
            .run_if(|| active()))
            .add_systems(PostUpdate, (|mut h: Query<&mut crate::combat::Health, With<crate::player::LocalPlayer>>| {
                for mut h in &mut h {
                    h.current = crate::combat::max_health();
                }
            })
            .run_if(|| active()))
            .add_systems(Update, stations.after(crate::splitscreen::InputGathered).run_if(|| active()));
        reveal::build(app);
        ambient::build(app);
        life::build(app);
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
                HqFigure,
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

/// One of the hangar's SAS ([`spawn_figures`]).
#[derive(Component)]
pub struct HqFigure;

fn leave(mut commands: Commands, mut view: ResMut<crate::wardrobe::ThirdPerson>) {
    NEAR.store(usize::MAX, Ordering::Relaxed);
    EXPOSURE.store(0f32.to_bits(), Ordering::Relaxed);
    if ACTIVE.swap(false, Ordering::Relaxed) {
        view.0[0] = false;
    }
    commands.remove_resource::<Headquarters>();
}

/// F.N.G.'s day is overcast: its sun as our pipeline renders the level's
/// sunlight value (the same 1.5 as mp_killhouse, which is all indoors)
/// blows the snowy tarmac out to white. Outdoors in Headquarters the sun is
/// this much of that (the hangar's inside, lit by its lightmaps, is
/// unchanged). Debug: `COD4RW_HQSUN=<scale>`.
const SUN_SCALE: f32 = 0.7;

fn overcast(mut suns: Query<&mut DirectionalLight, Added<DirectionalLight>>) {
    let scale = std::env::var("COD4RW_HQSUN").ok().and_then(|s| s.parse().ok()).unwrap_or(SUN_SCALE);
    for mut sun in &mut suns {
        sun.illuminance *= scale;
    }
}

/// Outdoors (no roof over the camera within this many units) the frame is
/// this many stops darker, eased in and out over half a second: the snowy,
/// wet ground and the hangars' siding under F.N.G.'s overcast sky otherwise
/// glare. Indoors, as it was.
const ROOF_WITHIN: f32 = 1200.0;
const OUTDOOR_EV: f32 = -0.75;
static EXPOSURE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Headquarters' exposure offset now (stops): [`crate::settings_apply`]'s
/// brightness adds it.
pub fn exposure_offset() -> f32 {
    if active() { f32::from_bits(EXPOSURE.load(Ordering::Relaxed)) } else { 0.0 }
}

fn outdoor_exposure(time: Res<Time>, spatial: avian3d::prelude::SpatialQuery, cameras: Query<(&crate::splitscreen::SlotCamera, &GlobalTransform)>) {
    let Some((_, cam)) = cameras.iter().find(|c| c.0.0 == 0) else { return };
    let roofed = spatial
        .cast_ray(cam.translation(), Dir3::Y, crate::units::u(ROOF_WITHIN), true, &crate::collision::sight_filter())
        .is_some();
    let target = if roofed { 0.0 } else { OUTDOOR_EV };
    let now = f32::from_bits(EXPOSURE.load(Ordering::Relaxed));
    let next = now + (target - now) * (1.0 - (-4.0 * time.delta_secs()).exp());
    EXPOSURE.store(next.to_bits(), Ordering::Relaxed);
}

/// Health while Headquarters' frame runs: no damage reaches it.
const UNHURT: f32 = 1.0e6;

/// Always behind the player.
fn third_person(mut view: ResMut<crate::wardrobe::ThirdPerson>) {
    if !view.0[0] {
        view.0[0] = true;
    }
}

/// A place in the hangar to walk up to: Use there opens a menu.
pub struct Station {
    /// Where its label floats, CoD units.
    pub at: [f32; 3],
    /// How near (CoD units, across the floor) the player must be.
    pub reach: f32,
    /// Its name over it, and what Use does ("Press F to ...").
    pub label: &'static str,
    pub action: &'static str,
    /// The menu script it runs.
    script: &'static str,
}

/// The hangar's stations: Price's briefing corner and the door. (Supply
/// drops are called in anywhere: [`reveal`].)
pub const STATIONS: [Station; 5] = [
    Station {
        at: [-508.0, -1025.0, 110.0],
        reach: 110.0,
        label: "Loadouts",
        action: "edit your classes",
        script: "\"open\" \"pc_cac_popup\"",
    },
    Station {
        at: [-310.0, -985.0, 85.0],
        reach: 95.0,
        label: "Combat Record",
        action: "view your Combat Record",
        script: "\"uiScript\" \"combatOpen\"",
    },
    Station {
        at: [-388.0, -830.0, 70.0],
        reach: 100.0,
        label: "Character",
        action: "choose your character",
        script: "\"open\" \"supply_character\"",
    },
    Station {
        at: [66.0, -976.0, 85.0],
        reach: 90.0,
        label: "Quartermaster",
        action: "open Supply Drops",
        script: "\"open\" \"supply_drops\"",
    },
    Station {
        at: [160.0, -1190.0, 110.0],
        reach: 130.0,
        label: "Find a Game",
        action: "set up a match",
        script: "\"open\" \"private_match\"",
    },
];

/// The station the player stands at ([`STATIONS`]' index), and Use pressed
/// (taken from the input in [`strip`]).
static NEAR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);
static USE: AtomicBool = AtomicBool::new(false);
/// The pad's X last frame (Use is its going down).
static PAD_USE: AtomicBool = AtomicBool::new(false);

/// The station within reach.
pub fn near() -> Option<&'static Station> {
    STATIONS.get(NEAR.load(Ordering::Relaxed))
}

/// A station's name over it.
pub fn label_of(s: &Station) -> String {
    s.label.to_owned()
}

/// What Use does here: "Press F to ...", in the middle of the screen.
pub fn prompt(input: &PlayerInput) -> Option<String> {
    let key = input.use_key();
    match reveal::phase() {
        reveal::Phase::Done => return Some(format!("Press {key} to put it away")),
        reveal::Phase::Waiting if reveal::near_crate() => return Some(format!("Press {key} to open the Supply Drop")),
        reveal::Phase::None | reveal::Phase::Waiting => {}
        _ => return None,
    }
    near().map(|s| format!("Press {key} to {}", s.action))
}

/// At the bottom, while there are drops to open and none is out: how to
/// call one in, and how far into the hold.
pub fn call_hint(input: &PlayerInput) -> Option<(String, f32)> {
    let n = crate::supply::inventory().unopened;
    if n == 0 || reveal::phase() != reveal::Phase::None {
        return None;
    }
    let key = match input.pad_kind {
        Some(crate::gamepad::PadKind::PlayStation) => "R1".to_owned(),
        Some(_) => "RB".to_owned(),
        None => CALL_KEY.lock().map(|k| k.clone()).unwrap_or_default(),
    };
    let key = if key.is_empty() { "G".to_owned() } else { key };
    let drops = if n == 1 { "1 to open".to_owned() } else { format!("{n} to open") };
    Some((format!("Hold {key} to call in a Supply Drop ({drops})"), reveal::hold()))
}

/// The keyboard's Frag key's name (it calls drops in): set from the
/// settings ([`crate::settings_apply`]).
pub static CALL_KEY: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The cards of a drop being opened ([`reveal::cards`]).
pub fn drop_cards() -> Vec<(crate::supply::Item, bool, Option<f32>)> {
    reveal::cards()
}

/// Which station is in reach, and Use there opens its menu.
fn stations(player: Query<&Transform, With<crate::player::LocalPlayer>>, fe: Option<ResMut<crate::ui::Frontend>>) {
    // A drop out (or by a crate to open): Use is the drop's.
    if revealing() || reveal::near_crate() {
        NEAR.store(usize::MAX, Ordering::Relaxed);
        return;
    }
    let used = USE.swap(false, Ordering::Relaxed);
    let Ok(tf) = player.single() else { return };
    let me = crate::units::to_cod(tf.translation);
    let nearest = STATIONS
        .iter()
        .enumerate()
        .map(|(i, s)| (i, ((me[0] - s.at[0]).powi(2) + (me[1] - s.at[1]).powi(2)).sqrt()))
        .filter(|&(i, d)| d <= STATIONS[i].reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i);
    NEAR.store(nearest.unwrap_or(usize::MAX), Ordering::Relaxed);
    let (Some(i), true, Some(mut fe)) = (nearest, used, fe) else { return };
    if crate::ui::menu_open(Some(&*fe)) {
        return;
    }
    fe.run_station(STATIONS[i].script);
}

/// No fighting: the fire, aim, reload, grenade, melee, equipment and
/// weapon switch inputs never reach the game, nor does the view toggle.
pub fn strip(input: &mut PlayerInput) {
    if !active() {
        return;
    }
    // Use is for the stations and the drops (F, or a pad's X going down);
    // the Frag key calls drops in.
    let pad_use = input.pad.interact;
    let pad_pressed = pad_use && !PAD_USE.swap(pad_use, Ordering::Relaxed);
    if !pad_use {
        PAD_USE.store(false, Ordering::Relaxed);
    }
    if input.keys.just_pressed(KeyCode::KeyF) || pad_pressed {
        USE.store(true, Ordering::Relaxed);
    }
    reveal::set_call_held(input.keys.pressed(KeyCode::KeyG));
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
