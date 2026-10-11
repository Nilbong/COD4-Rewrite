//! CoD4's in-match HUD, drawn with the game's own materials and fonts: the
//! rotating minimap (`hud.menu`'s `compass_old`), the ammo counter
//! (`weaponinfo`), the weapon's crosshair, hit markers
//! (`_damagefeedback.gsc`), damage direction arrows, the low health
//! overlay, obituaries with kill icons, and the "+10" a kill is worth
//! (`_rank.gsc`). Places, sizes and timings are those of CoD4's menus,
//! scripts and multiplayer config. CoD4's own HUD menus run on the menu
//! interpreter with the match's state ([`GameInfo`]): `scorebars` (team
//! scores and the clock, bottom left), `scorebar` (the score along the
//! top after spawning and near the end) and, with Tab, the `scoreboard`
//! banner over the player list.
//!
//! A match started with `--map` has no menus loaded; their fonts and
//! materials then load in the background as it starts.

use crate::settings_apply::Hud;
use super::assets::UiAssets;
use super::draw::{self, MINIMAP_Z, Placement};
use super::expr::{Env, Val};
use super::{Frontend, Op, OpenMenu};
use crate::bodycam::Gunplay;
use crate::combat::{Damage, Dead, Health, HitLocation, Hitbox, Killed, Pawn, Team, hostile};
use crate::content::Content;
use crate::loadout::{AwaitingClass, Loadout};
use crate::movement::{Mover, ViewAngles};
use crate::player::LocalPlayer;
use crate::splitscreen::{LocalSlot, PlayerInput, SlotCamera};
use crate::state::{GameState, in_game};
use crate::tdm::MatchState;
use crate::weapons::{HitConfirmed, ShotFired, WeaponState};
use crate::killstreaks::{Hardpoint, StreakNotice};
use crate::world::MapName;
use avian3d::prelude::SpatialQuery;
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::ui_render::prelude::{MaterialNode, UiMaterial, UiMaterialPlugin};
use bevy::window::PrimaryWindow;
use iw3::menu::Rect as VRect;
use iw3::menu::op;
use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, TAU};
use std::sync::Arc;
use std::thread::JoinHandle;

/// World units from the compass's centre to its edge: half CoD4's
/// `compassMaxRange` (2500), for a closer view.
const COMPASS_RANGE: f32 = 1250.0;
/// `compassPlayerWidth`, `compassFriendlyWidth`.
const COMPASS_ICON: f32 = 18.75;
/// `compassSoundPingFadeTime`: how long a shot shows an enemy.
const PING_TIME: f32 = 2.0;
/// `compassTickertapeStretch`: the share of the tape across the ticker.
const TICKER_SPAN: f32 = 0.5;
/// The compass's map square (`mini_map`), left/top aligned.
const MAP_RECT: [f32; 4] = [6.0, 18.0, 102.0, 102.0];
/// `cg_hudDamageIconWidth`/`Height`/`Offset`/`Time`.
const DAMAGE_ICON: Vec2 = Vec2::new(128.0, 64.0);
const DAMAGE_ICON_OFFSET: f32 = 128.0;
const DAMAGE_ICON_TIME: f32 = 2.0;
/// `con_gameMsgWindow0*`: obituary lines, how long each stays, fading in
/// and out.
const OBIT_LINES: usize = 4;
const OBIT_TIME: f32 = 5.0;
const OBIT_FADE_IN: f32 = 0.25;
const OBIT_FADE_OUT: f32 = 0.5;
/// Longer than any hit marker shows (hold + fade): a hit after this pops
/// it in afresh.
const HIT_MARKER_GONE: f32 = 0.7;
/// TDM's score for a kill, shown as XP.
const KILL_XP: u32 = 10;
/// An assist's score (`registerScoreInfo( "assist", 2 )`).
const ASSIST_SCORE: u32 = 2;
/// `updateRankScoreHUD`: shown a second, then faded over 0.75.
const XP_SHOW: f32 = 1.0;
const XP_FADE: f32 = 0.75;
/// The weapon's name after a switch or spawn, then fading.
const NAME_SHOW: f32 = 3.0;
const NAME_FADE: f32 = 0.5;

/// Obituary names: the player's team and the other.
const FRIEND: [f32; 3] = [0.6, 0.85, 1.0];
const ENEMY: [f32; 3] = [1.0, 0.45, 0.4];
const WHITE: [f32; 4] = [1.0; 4];

mod names;
mod objectives;
pub(super) mod modern;

pub(super) fn build(app: &mut App) {
    app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
        std::path::PathBuf::from(file!()).with_file_name("compass.wgsl"),
        std::path::Path::new(COMPASS_SHADER),
        include_bytes!("compass.wgsl").as_slice(),
    );
    app.add_plugins(UiMaterialPlugin::<CompassMaterial>::default())
        .init_resource::<HudState>()
        .init_resource::<ExtraHuds>()
        .add_systems(OnEnter(GameState::InGame), setup_minimap.in_set(crate::state::Setup::Spawn))
        .add_systems(Update, (collect, collect_extra, load_assets, aim_check, names::update, pick_airstrike).run_if(in_game));
    if std::env::var_os("COD4RW_HITMARKERTEST").is_some() {
        app.add_systems(Update, hit_marker_test.run_if(in_game));
    }
    if std::env::var_os("COD4RW_HUDTEST").is_some() {
        app.add_systems(
            Update,
            (hud_test.after(crate::player::InputSet).before(crate::weapons::WeaponSet), hud_test_log).run_if(in_game),
        );
    }
}

/// Debug aid: `COD4RW_HITMARKERTEST=<dir>` fakes a hit 6 s in and a kill
/// 8 s in, saving frames through each marker's pop and fade, then exits.
fn hit_marker_test(
    mut commands: Commands,
    time: Res<Time>,
    mut hud: ResMut<HudState>,
    mut step: Local<usize>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    const AT: [f32; 7] = [0.0, 0.017, 0.035, 0.06, 0.1, 0.2, 0.4];
    let t = time.elapsed_secs();
    let dir = std::path::PathBuf::from(std::env::var("COD4RW_HITMARKERTEST").unwrap_or_default());
    let (start, kill) = if *step < AT.len() { (6.0, false) } else { (8.0, true) };
    let i = *step % AT.len();
    if *step >= 2 * AT.len() {
        if t > 9.0 {
            exit.write(AppExit::Success);
        }
        return;
    }
    if i == 0 && t >= start && hud.hit < start {
        hud.hit = t;
        hud.hit_start = t;
        if kill {
            hud.hit_kill = t;
        }
    }
    if hud.hit >= start && t - hud.hit >= AT[i] {
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join(format!("{}_{i}.png", if kill { "kill" } else { "hit" }));
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        *step += 1;
    }
}

/// Debug aid: with `COD4RW_HUDTEST` (and `COD4RW_DUMMY`'s frozen lineup):
/// 5 s in the player fires at the friendly dummy (no hit marker); 5.6 s in
/// an enemy wounds the player, fires and steps into the crosshair (red),
/// and the player kills another with a headshot; 6.1 s in the player fires
/// at that body (no hit marker). The enemy leads by two kills, for the
/// losing score layout. For screenshots of the HUD's events; the player's
/// shots and hit confirms are logged.
#[allow(clippy::too_many_arguments)]
fn hud_test(
    time: Res<Time>,
    mut step: Local<(u8, Option<Entity>)>,
    mut me: Query<(Entity, &Transform, &mut ViewAngles, &mut crate::weapons::WeaponInput), With<LocalPlayer>>,
    mut pawns: Query<(Entity, &Pawn, &mut Transform), Without<LocalPlayer>>,
    mut damage: MessageWriter<Damage>,
    mut hits: MessageWriter<HitConfirmed>,
    mut shots: MessageWriter<ShotFired>,
    state: Option<ResMut<MatchState>>,
) {
    let t = time.elapsed_secs();
    let Ok((me, feet, mut view, mut input)) = me.single_mut() else { return };
    // Aim at a pawn's chest from the standing eye.
    let aim = |view: &mut ViewAngles, at: Vec3| {
        let d = (at + Vec3::Y * crate::units::u(45.0) - feet.translation - Vec3::Y * crate::units::u(60.0)).normalize_or_zero();
        view.yaw = (-d.x).atan2(-d.z);
        view.pitch = d.y.asin();
    };
    let (n, body) = &mut *step;
    match *n {
        0 if t >= 5.0 => {
            let Some(friend) = pawns.iter().find(|p| p.1.team == Team::Allies).map(|p| p.2.translation) else { return };
            aim(&mut view, friend);
            input.fire = true;
            *n = 1;
        }
        2 if t >= 5.6 => {
            let enemies: Vec<Entity> = pawns.iter().filter(|p| p.1.team == Team::Axis).map(|p| p.0).collect();
            let [shooter, target, ..] = enemies[..] else { return };
            *n = 3;
            *body = Some(target);
            if let Some(mut state) = state {
                state.axis += 3;
            }
            let at = pawns.get(shooter).map_or(Vec3::ZERO, |p| p.2.translation);
            damage.write(Damage { target: me, attacker: Some(shooter), amount: 65.0, location: HitLocation::Torso, weapon: "ak47_mp" });
            shots.write(ShotFired { shooter, weapon: None, from: at, to: at, hit_pawn: true, normal: Vec3::Y, hit_world: false });
            damage.write(Damage { target, attacker: Some(me), amount: 500.0, location: HitLocation::Head, weapon: "ak47_mp" });
            hits.write(HitConfirmed { shooter: me, headshot: true });
            let ahead = view.forward().with_y(0.0).normalize_or_zero();
            if let Ok((_, _, mut tf)) = pawns.get_mut(shooter) {
                tf.translation = feet.translation + ahead * 2.0;
            }
        }
        3 if t >= 6.1 => {
            let Some(at) = body.and_then(|b| pawns.get(b).ok()).map(|p| p.2.translation) else { return };
            aim(&mut view, at);
            input.fire = true;
            *n = 4;
        }
        1 | 4 if t >= 5.3 && (*n == 1 || t >= 6.4) => {
            input.fire = false;
            *n += 1;
        }
        1 | 4 => input.fire = true,
        _ => {}
    }
}

fn hud_test_log(mut shots: MessageReader<ShotFired>, mut hits: MessageReader<HitConfirmed>, me: Query<Entity, With<LocalPlayer>>) {
    let Ok(me) = me.single() else { return };
    for s in shots.read().filter(|s| s.shooter == me) {
        info!("hud test: player shot, hit a pawn: {}", s.hit_pawn);
    }
    for _ in hits.read().filter(|h| h.shooter == me) {
        info!("hud test: hit confirmed");
    }
}

/// Where `compass.wgsl` is registered in the `embedded://` asset source.
const COMPASS_SHADER: &str = "cod4rw/compass.wgsl";

/// The minimap's map, turned and clipped (see `compass.wgsl`).
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct CompassMaterial {
    #[uniform(0)]
    center_scale: Vec4,
    #[uniform(1)]
    dir_alpha: Vec4,
    #[texture(2)]
    #[sampler(3)]
    map: Handle<Image>,
    /// x: roundness (0 square, 1 circle), y: outline width (share of the
    /// half size), z: fill behind the map's image (alpha), w: pixels per
    /// half size (for smooth edges).
    #[uniform(4)]
    shape: Vec4,
    #[uniform(5)]
    outline_color: Vec4,
}

impl UiMaterial for CompassMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/compass.wgsl".into()
    }
}

/// A player's compass map (by their place).
#[derive(Component)]
pub(super) struct CompassMap(usize);

/// The map's minimap image and the square of the world it shows: the map
/// script's `setupMiniMap`, from its two `minimap_corner`s and its north.
#[derive(Resource, Clone, Debug)]
pub struct Minimap {
    material: String,
    /// CoD space: the image's top left corner, and its east and north.
    northwest: Vec2,
    east: Vec2,
    north: Vec2,
    /// World units across and down the image.
    size: Vec2,
}

impl Minimap {
    fn of(content: &Content, map: &str) -> Option<Minimap> {
        let ents = iw3::ents::parse(&content.map().map_ents()?.entity_string);
        let corners: Vec<Vec2> = ents
            .iter()
            .filter(|e| e.get("targetname") == Some("minimap_corner"))
            .filter_map(|e| e.origin())
            .map(|o| Vec2::new(o[0], o[1]))
            .collect();
        let [c0, c1] = corners[..] else { return None };
        let yaw = ents
            .iter()
            .find(|e| e.classname() == "worldspawn")
            .and_then(|e| e.get("northyaw")?.parse::<f32>().ok())
            // Maps without one (mp_crash, mp_backlot) have +X north: their
            // images run along it.
            .unwrap_or(0.0)
            .to_radians();
        let (northwest, east, north, size) = square(c0, c1, yaw);
        // `maps\mp\_compass::setupMiniMap("compass_map_mp_...")`.
        let script = format!("maps/mp/{map}.gsc");
        let material = content
            .zones
            .iter()
            .flat_map(|z| &z.assets)
            .find_map(|a| match a {
                iw3::zone::Asset::RawFile(r) if r.name.eq_ignore_ascii_case(&script) => {
                    let text = String::from_utf8_lossy(&r.data);
                    let at = text.find("setupMiniMap(")?;
                    text[at..].split('"').nth(1).map(str::to_owned)
                }
                _ => None,
            })
            .unwrap_or_else(|| format!("compass_map_{map}"));
        Some(Minimap { material, northwest, east, north, size })
    }

    /// A CoD-space point's place in the image (0..1, y down).
    fn uv(&self, p: Vec2) -> Vec2 {
        let d = p - self.northwest;
        Vec2::new(d.dot(self.east) / self.size.x, -d.dot(self.north) / self.size.y)
    }

    /// A CoD-space direction in image space (x east, y south).
    fn image_dir(&self, v: Vec2) -> Vec2 {
        Vec2::new(v.dot(self.east), -v.dot(self.north))
    }
}

/// `_compass.gsc`: the north-west corner, east, north and size of the
/// square around two opposite corners (`scr_requiredMapAspectRatio` 1; the
/// minimap images are square).
fn square(c0: Vec2, c1: Vec2, north_yaw: f32) -> (Vec2, Vec2, Vec2, Vec2) {
    let north = Vec2::new(north_yaw.cos(), north_yaw.sin());
    let west = Vec2::new(-north.y, north.x);
    let diff = c1 - c0;
    let side = north * diff.dot(north);
    let (mut nw, mut se) = match (diff.dot(west) > 0.0, diff.dot(north) > 0.0) {
        (true, true) => (c1, c0),
        (true, false) => (c1 - side, c0 + side),
        (false, true) => (c0 + side, c1 - side),
        (false, false) => (c0, c1),
    };
    let (north_part, west_part) = ((nw - se).dot(north), (nw - se).dot(west));
    let aspect = west_part / north_part;
    let add = if aspect < 1.0 { west * west_part * (1.0 / aspect - 1.0) * 0.5 } else { north * north_part * (aspect - 1.0) * 0.5 };
    nw += add;
    se -= add;
    let east = -west;
    (nw, east, north, Vec2::new((se - nw).dot(east), (nw - se).dot(north)))
}

fn setup_minimap(mut commands: Commands, content: Option<Res<Content>>, map: Res<MapName>, mut hud: ResMut<HudState>) {
    let menus = std::mem::take(&mut hud.menus);
    *hud = HudState { menus, ..default() };
    commands.remove_resource::<Minimap>();
    let Some(minimap) = content.and_then(|c| Minimap::of(&c, &map.0)) else {
        info!("hud: {} has no minimap", map.0);
        return;
    };
    debug!("hud: minimap {minimap:?}");
    commands.insert_resource(minimap);
}

/// What happened lately that the HUD shows, and what it draws this frame.
#[derive(Resource)]
pub(super) struct HudState {
    /// The frame time's running average (ms), for the performance overlay.
    frame_ms: f32,
    /// A dropped weapon a Use would swap for: its name's string key
    /// ([`crate::pickups`]).
    pickup: Option<String>,
    /// When the player last hurt someone.
    hit: f32,
    /// When the player last got a kill (the hit marker goes red).
    hit_kill: f32,
    /// When the hit marker last appeared from nothing (its big pop; later
    /// hits while it shows only bump it).
    hit_start: f32,
    /// The crosshair is on a live enemy.
    aim_enemy: bool,
    /// Where the player was hurt from (Bevy space), and when.
    hurt: Vec<(Vec3, f32)>,
    obits: Vec<Line>,
    /// XP being shown and when it last grew.
    xp: (u32, f32),
    /// Notifies (`_hud_message.gsc`), one after another: title, text, icon,
    /// when it shows.
    notify: Vec<(String, String, String, f32)>,
    /// Enemies heard firing: where, when.
    pings: HashMap<Entity, (Vec3, f32)>,
    /// The team UAV's last sweep: its number, and the enemies it saw (where,
    /// when).
    radar_sweep: Option<u32>,
    radar_pings: Vec<(Vec3, f32)>,
    /// A kill streak's announcement (`hardpointNotify`): the streak, what it
    /// earned and when; and a hardpoint that couldn't be used now.
    streak_notify: Option<(u32, Hardpoint, f32)>,
    unavailable: Option<(Hardpoint, f32)>,
    /// The weapon name shown, and since when.
    weapon: (String, f32),
    /// Each pawn's last wound: where, and by what.
    last_hit: HashMap<Entity, (HitLocation, &'static str)>,
    /// The compass map's node this frame, if shown.
    compass: Option<CompassView>,
    /// CoD4's HUD menus: `scorebars`, `scorebar`, `scoreboard`.
    menus: Vec<OpenMenu>,
    /// This frame's HUD, drawn under any open menu.
    pub ops: Vec<Op>,
}

/// The match as CoD4's HUD menus see it (`team("score")`, `timeLeft()`,
/// ...): see [`Frontend::game_value`].
#[derive(Default, Clone)]
pub(super) struct GameInfo {
    /// In a match.
    pub on: bool,
    /// Team scores in points (allies, axis), and the player's team.
    pub scores: [i32; 2],
    pub mine: Option<usize>,
    pub player_score: i32,
    pub player_name: String,
    pub time_left: f32,
    pub intermission: bool,
    pub scoreboard: bool,
    pub ui_active: bool,
    /// Free-for-all: the player is on `TEAM_FREE`, and the menus show
    /// players' scores, best first (`score(1)`, `score(2)`).
    pub ffa: bool,
    pub ranked: Vec<i32>,
}

impl Frontend {
    /// The game-state functions of menu expressions, in a match.
    pub(super) fn game_value(&self, f: u8, args: &[Val]) -> Option<Val> {
        let g = &self.game;
        if !g.on {
            return None;
        }
        let field = args.first().map(|v| v.text().to_ascii_lowercase()).unwrap_or_default();
        let team = |i: Option<usize>| match (field.as_str(), i) {
            ("name", Some(_)) if g.ffa => Val::Str("TEAM_FREE".into()),
            ("score", Some(i)) => Val::Int(g.scores[i]),
            ("name", Some(0)) => Val::Str("TEAM_ALLIES".into()),
            ("name", Some(_)) => Val::Str("TEAM_AXIS".into()),
            ("name", None) => Val::Str("TEAM_SPECTATOR".into()),
            _ => Val::Int(0),
        };
        Some(match f {
            op::UIACTIVE => Val::Int(g.ui_active as i32),
            op::FLASHBANGED | op::SCOPED | op::INKILLCAM | op::SELECTINGLOCATION | op::FOLLOWING | op::GAMEMSGWNDACTIVE => Val::Int(0),
            op::SCOREBOARDVISIBLE => Val::Int(g.scoreboard as i32),
            op::PLAYERFIELD => match field.as_str() {
                "score" => Val::Int(g.player_score),
                "name" => Val::Str(g.player_name.clone()),
                _ => Val::Int(0),
            },
            op::TEAM => team(g.mine),
            op::OTHERTEAM => team(g.mine.map(|m| 1 - m)),
            op::MARINES => team(Some(0)),
            op::OPFOR => team(Some(1)),
            op::TIMELEFT => Val::Int(g.time_left.ceil() as i32),
            // `score(n)`: the nth best player's.
            op::SCORE => Val::Int(field.parse::<usize>().ok().and_then(|n| g.ranked.get(n.checked_sub(1)?)).copied().unwrap_or(0)),
            op::ISINTERMISSION => Val::Int(g.intermission as i32),
            _ => return None,
        })
    }
}

/// Each team's faction for the menus: `scr_allies`/`scr_axis` and its name,
/// by the map's soldiers.
fn faction(sides: Option<&crate::audio::Sides>, team: Team) -> (&'static str, &'static str) {
    match (sides.map(|s| s.of(team).music), team) {
        (Some("sas"), _) => ("sas", "MPUI_SAS"),
        (Some("soviet"), _) => ("ussr", "MPUI_SPETSNAZ"),
        (_, Team::Axis) => ("arab", "MPUI_OPFOR"),
        _ => ("usmc", "MPUI_MARINES"),
    }
}

/// Keep [`GameInfo`] and the dvars the HUD menus read up to date.
/// `_hud.gsc`'s `showClientScoreBar(5)` raises `ui_score_bar` for five
/// seconds after spawning.
#[allow(clippy::too_many_arguments)]
pub(super) fn sync_game(
    mut fe: ResMut<Frontend>,
    time: Res<Time>,
    state: Option<Res<MatchState>>,
    sides: Option<Res<crate::audio::Sides>>,
    player: Query<(&Pawn, Has<Dead>, Has<AwaitingClass>, &PlayerInput), With<LocalPlayer>>,
    pawns: Query<&Pawn>,
    objectives: Option<Res<crate::modes::Objectives>>,
    mut score_bar: Local<(bool, f32)>,
) {
    let now = time.elapsed_secs();
    let me = player.single().ok();
    let alive = me.is_some_and(|(_, dead, waiting, _)| !dead && !waiting);
    if alive && !score_bar.0 {
        score_bar.1 = now + 5.0;
    }
    score_bar.0 = alive;
    let mode = state.as_ref().map_or_else(Default::default, |s| s.mode);
    // Search and Destroy's clock: the round's, or the bomb's.
    let timer = objectives.as_ref().and_then(|o| o.timer);
    // Points: kills are worth 10, Domination's points one.
    let scale = mode.point_scale();
    let (limit, minutes) = state.as_ref().map_or((0, 0.0), |s| (s.score_limit * scale, s.time_limit / 60.0));
    let mut ranked: Vec<i32> = pawns.iter().map(|p| (p.kills * KILL_XP) as i32).collect();
    ranked.sort_by(|a, b| b.cmp(a));
    let (allies, axis) = (faction(sides.as_deref(), Team::Allies), faction(sides.as_deref(), Team::Axis));
    let dvars = [
        ("scr_allies", allies.0.to_owned()),
        ("scr_axis", axis.0.to_owned()),
        ("g_TeamIcon_Allies", format!("faction_128_{}", allies.0)),
        ("g_TeamIcon_Axis", format!("faction_128_{}", axis.0)),
        ("g_TeamName_Allies", allies.1.to_owned()),
        ("g_TeamName_Axis", axis.1.to_owned()),
        ("ui_scorelimit", limit.to_string()),
        ("ui_timelimit", format!("{minutes}")),
        ("ui_score_bar", ((now < score_bar.1) as i32).to_string()),
        ("ui_gametype", mode.gametype().to_owned()),
        ("g_gametype", mode.gametype().to_owned()),
        // The scoreboard's title (`gametypename()`, from `mp/gametypesTable.csv`).
        ("ui_netGametypeName", mode.gametype().to_owned()),
        // The HUD's clock shows the bomb's timer once planted.
        ("ui_bomb_timer", (timer.is_some_and(|t| t.1) as i32).to_string()),
        ("ui_hud_hardcore", (crate::tdm::hardcore() as i32).to_string()),
    ];
    for (k, v) in dvars {
        if fe.dvar(k) != v {
            fe.set_dvar(k, &v);
        }
    }
    let team_index = |t: Team| if t == Team::Allies { 0 } else { 1 };
    let ended = state.as_ref().is_some_and(|s| s.ended.is_some());
    fe.game = GameInfo {
        on: true,
        scores: state.as_ref().map_or([0; 2], |s| [Team::Allies, Team::Axis].map(|t| (s.score(t) * scale) as i32)),
        mine: me.map(|(p, ..)| team_index(p.team)),
        player_score: me.map_or(0, |(p, ..)| (p.kills * KILL_XP) as i32),
        player_name: me.map(|(p, ..)| p.name.clone()).unwrap_or_default(),
        time_left: timer.map_or_else(|| state.as_ref().map_or(0.0, |s| s.time_left(now)), |(at, _)| (at - now).max(0.0)),
        intermission: ended,
        scoreboard: me.is_some_and(|m| m.3.keys.pressed(KeyCode::Tab)) || ended,
        ui_active: !fe.stack.is_empty(),
        ffa: !mode.teams(),
        ranked,
    };
}

/// Back in the menus: no match for their expressions.
pub(super) fn end_game(mut commands: Commands, mut fe: Option<ResMut<Frontend>>) {
    if let Some(fe) = fe.as_mut() {
        // Headquarters' previews go with it (the menus start theirs anew).
        fe.previews.clear(&mut commands);
        fe.game = GameInfo::default();
        fe.clear_slot_menus();
    }
}

impl Default for HudState {
    fn default() -> Self {
        HudState {
            frame_ms: 0.0,
            pickup: None,
            hit: f32::NEG_INFINITY,
            hit_kill: f32::NEG_INFINITY,
            hit_start: f32::NEG_INFINITY,
            aim_enemy: false,
            hurt: Vec::new(),
            obits: Vec::new(),
            xp: (0, f32::NEG_INFINITY),
            notify: Vec::new(),
            pings: HashMap::new(),
            radar_sweep: None,
            radar_pings: Vec::new(),
            streak_notify: None,
            unavailable: None,
            weapon: (String::new(), f32::NEG_INFINITY),
            last_hit: HashMap::new(),
            compass: None,
            menus: Vec::new(),
            ops: Vec::new(),
        }
    }
}

impl HudState {
    /// Where the minimap's map is drawn (window pixels), if it is.
    pub(super) fn minimap_rect(&self) -> Option<(Vec2, Vec2)> {
        self.compass.as_ref().map(|c| (c.pos, c.size))
    }

    /// A line of plain text among the obituaries.
    /// XP earned: the "+N" by the crosshair, adding up while it shows.
    pub fn add_xp(&mut self, amount: u32, now: f32) {
        let showing = now - self.xp.1 < XP_SHOW + XP_FADE;
        self.xp = (if showing { self.xp.0 } else { 0 } + amount, now);
    }

    /// A notify at the top of the screen ("Promoted!"), after any showing.
    pub fn notify(&mut self, title: String, text: String, icon: String, now: f32) {
        self.notify.retain(|n| n.3 + NOTIFY_TIME > now);
        let at = self.notify.last().map_or(now, |n| (n.3 + NOTIFY_TIME).max(now));
        self.notify.push((title, text, icon, at));
    }

    pub fn message(&mut self, text: String, now: f32) {
        self.obits.push(Line { time: now, parts: vec![Part::Text(text, [1.0; 3])] });
    }
}

/// An obituary: names and icons in a row.
struct Line {
    time: f32,
    parts: Vec<Part>,
}

enum Part {
    Text(String, [f32; 3]),
    /// A localized string (`&&1`... from `args`), or `fallback` where the
    /// loaded zones lack it.
    Loc { key: &'static str, fallback: &'static str, args: Vec<String> },
    /// A material, and its width over its height.
    Icon(String, f32),
}

struct CompassView {
    material: String,
    pos: Vec2,
    size: Vec2,
    center: Vec2,
    scale: Vec2,
    dir: Vec2,
    /// The map image's alpha, [`CompassMaterial::shape`] (bar its w) and
    /// its outline's colour.
    alpha: f32,
    shape: Vec4,
    outline: Vec4,
}

/// Note hits, wounds, kills and enemy gunfire as they happen.
#[allow(clippy::too_many_arguments)]
fn collect(
    time: Res<Time>,
    mut hud: ResMut<HudState>,
    mut damage: MessageReader<Damage>,
    mut killed: MessageReader<Killed>,
    mut hits: MessageReader<HitConfirmed>,
    mut shots: MessageReader<ShotFired>,
    pawns: Query<(&Pawn, &Transform, Option<&WeaponState>, Option<&Loadout>)>,
    me: Query<(Entity, &Pawn), With<LocalPlayer>>,
    mut notices: MessageReader<StreakNotice>,
    radar: Res<crate::killstreaks::uav::Radar>,
    alive: Query<(&Pawn, &Transform, Option<&Loadout>), Without<Dead>>,
    explosives: Option<Res<crate::explosives::ExplosiveDefs>>,
) {
    let now = time.elapsed_secs();
    let mine: Option<Pawn> = me.single().ok().map(|(_, p)| p.clone());
    let me = me.single().ok().map(|(e, p)| (e, p.team));
    let my_team = me.map_or(Team::Allies, |m| m.1);
    // The player's enemies: the other team, or in free-for-all everyone else.
    let enemy = |p: &Pawn| mine.as_ref().map_or(p.team != my_team, |m| hostile(p, m));
    for n in notices.read() {
        match n {
            StreakNotice::Earned { streak, item } => hud.streak_notify = Some((*streak, *item, now)),
            StreakNotice::Unavailable(item) => hud.unavailable = Some((*item, now)),
            // `MP_WAR_AIRSTRIKE_INBOUND_NEAR_YOUR_POSITION`.
            StreakNotice::AirstrikeNear => hud.message("Airstrike inbound near your position!".into(), now),
            StreakNotice::Opened { item } => hud.message(format!("Care Package: {}", item.map_or("Ammo", |h| h.name())), now),
            // `MP_WAR_RADAR_ACQUIRED` and so on: to the caller's team, and
            // the enemy's UAV to everyone else.
            StreakNotice::CalledIn { item, by, team } => {
                let friendly = if crate::combat::free_for_all() { mine.as_ref().is_some_and(|m| m.name == *by) } else { *team == my_team };
                let (key, fallback, args) = match (item, friendly) {
                    (Hardpoint::Uav, true) => ("MP_WAR_RADAR_ACQUIRED", "UAV Recon called in by &&1 for &&2 seconds", vec![by.clone(), "30".into()]),
                    (Hardpoint::Uav, false) => ("MP_WAR_RADAR_ACQUIRED_ENEMY", "Enemy acquired UAV Recon for &&1 seconds", vec!["30".into()]),
                    (Hardpoint::Airstrike, true) => ("MP_WAR_AIRSTRIKE_INBOUND", "Airstrike called in by &&1", vec![by.clone()]),
                    (Hardpoint::Helicopter, true) => ("MP_HELICOPTER_INBOUND", "Helicopter called in by &&1", vec![by.clone()]),
                    (Hardpoint::CarePackage, true) => ("MP_CAREPACKAGE_INBOUND", "Care Package called in by &&1", vec![by.clone()]),
                    (Hardpoint::Sentry, true) => ("MP_SENTRY_INBOUND", "Sentry Gun called in by &&1", vec![by.clone()]),
                    _ => continue,
                };
                hud.obits.push(Line { time: now, parts: vec![Part::Loc { key, fallback, args }] });
            }
        }
    }
    // The team UAV's sweeps: every enemy where it is, fading till the next.
    match me.and_then(|m| radar.sweep_for(my_team, m.0, now)) {
        Some((k, at)) if hud.radar_sweep != Some(k) => {
            hud.radar_sweep = Some(k);
            // UAV Jammer (`specialty_gpsjammer`) keeps them off it.
            hud.radar_pings = alive
                .iter()
                .filter(|(p, _, l)| enemy(p) && !crate::perks::has(*l, "specialty_gpsjammer"))
                .map(|(_, tf, _)| (tf.translation, at))
                .collect();
        }
        Some(_) => {}
        None => hud.radar_sweep = None,
    }
    let is_me = |e: Entity| me.is_some_and(|m| m.0 == e);
    for d in damage.read() {
        hud.last_hit.insert(d.target, (d.location, d.weapon));
        if is_me(d.target) {
            // A teammate's bullets do nothing (but in Hardcore): no arrow.
            let felt = |p: &Pawn| enemy(p) || crate::tdm::hardcore();
            let from = d.attacker.filter(|&a| !is_me(a)).and_then(|a| pawns.get(a).ok()).filter(|p| felt(p.0)).map(|p| p.1.translation);
            if let Some(from) = from {
                hud.hurt.push((from, now));
            }
        }
    }
    if hits.read().any(|h| is_me(h.shooter)) {
        if now - hud.hit > HIT_MARKER_GONE {
            hud.hit_start = now;
        }
        hud.hit = now;
    }
    for s in shots.read() {
        let Ok((p, tf, _, loadout)) = pawns.get(s.shooter) else { continue };
        let silenced = loadout.is_some_and(|l| l.gun().spec.contains("silencer"));
        if enemy(p) && !silenced {
            hud.pings.insert(s.shooter, (tf.translation, now));
        }
    }
    for k in killed.read() {
        let victim = pawns.get(k.victim).ok();
        let attacker = k.attacker.filter(|&a| a != k.victim).and_then(|a| Some((a, pawns.get(a).ok()?)));
        if attacker.as_ref().is_some_and(|(a, _)| is_me(*a)) {
            hud.hit = now;
            hud.hit_kill = now;
            hud.hit_start = now;
        }
        let (location, weapon) = hud.last_hit.remove(&k.victim).unwrap_or((HitLocation::Torso, ""));
        let color = |p: &Pawn| if enemy(p) { ENEMY } else { FRIEND };
        let mut parts = Vec::new();
        match &attacker {
            Some((_, (p, _, w, _))) => {
                parts.push(Part::Text(p.name.clone(), color(p)));
                let def = w.map_or_else(crate::weapons::weapon_def, |w| w.def);
                // `weaponIconRatio_t`: 1:1, 2:1, 4:1. A grenade's own icon
                // for its kills.
                let thrown = crate::grenades::kill_icon(weapon)
                    .or_else(|| explosives.as_ref().and_then(|x| x.kill_icon(weapon)))
                    .or((weapon == crate::melee::WEAPON).then_some(crate::melee::KILL_ICON));
                match (crate::killstreaks::kill_icon(weapon), thrown) {
                    (Some((icon, ratio)), _) => parts.push(Part::Icon(icon.into(), ratio)),
                    (None, Some(icon)) => parts.push(Part::Icon(icon.into(), 1.0)),
                    (None, None) => parts.push(Part::Icon(def.kill_icon.clone(), [1.0, 2.0, 4.0][def.kill_icon_ratio.clamp(0, 2) as usize])),
                }
                if location == HitLocation::Head {
                    parts.push(Part::Icon("killiconheadshot".into(), 1.0));
                }
            }
            None => parts.push(Part::Icon(if weapon == "falling" { "killiconfalling" } else { "killiconsuicide" }.into(), 1.0)),
        }
        if let Some((p, ..)) = victim {
            parts.push(Part::Text(p.name.clone(), color(p)));
        }
        hud.obits.push(Line { time: now, parts });
        // The "+10" comes from the XP the kill earns (`super::progression`).
    }
    hud.hurt.retain(|h| now - h.1 < DAMAGE_ICON_TIME);
    hud.pings.retain(|_, p| now - p.1 < crate::tune::get("minimap.enemy_ping_time", PING_TIME));
    hud.radar_pings.retain(|p| now - p.1 < crate::tune::get("minimap.uav_ping_time", crate::killstreaks::uav::PING_FADE));
    hud.obits.retain(|l| now - l.time < OBIT_TIME);
    let extra = hud.obits.len().saturating_sub(OBIT_LINES);
    hud.obits.drain(..extra);
}

/// The other splitscreen players' own HUD events: their wounds' directions
/// and their hits.
fn collect_extra(
    time: Res<Time>,
    mut extra: ResMut<ExtraHuds>,
    mut damage: MessageReader<Damage>,
    mut hits: MessageReader<HitConfirmed>,
    players: Query<(Entity, &LocalSlot)>,
    pawns: Query<(&Transform, &Pawn)>,
) {
    let now = time.elapsed_secs();
    let slot_of = |e: Entity| players.get(e).ok().map(|(_, s)| s.0).filter(|&s| s > 0);
    for d in damage.read() {
        let Some(slot) = slot_of(d.target) else { continue };
        // A teammate's bullets do nothing (but in Hardcore): no arrow.
        let me = pawns.get(d.target).ok().map(|p| p.1);
        let felt = |p: &Pawn| me.is_some_and(|m| hostile(p, m)) || crate::tdm::hardcore();
        let from = d.attacker.filter(|&a| a != d.target).and_then(|a| pawns.get(a).ok()).filter(|p| felt(p.1)).map(|p| p.0.translation);
        if let Some(from) = from {
            extra.slot(slot).hurt.push((from, now));
        }
    }
    for h in hits.read() {
        if let Some(slot) = slot_of(h.shooter) {
            extra.slot(slot).hit = now;
        }
    }
    for h in &mut extra.0 {
        h.hurt.retain(|x| now - x.1 < DAMAGE_ICON_TIME);
    }
}

/// `cg_crosshairEnemyColor`: trace each player's view like a bullet to see
/// whether their crosshair is on a live enemy.
fn aim_check(
    spatial: SpatialQuery,
    mut hud: ResMut<HudState>,
    mut extra: ResMut<ExtraHuds>,
    cameras: Query<(&GlobalTransform, &SlotCamera)>,
    players: Query<(Entity, &Pawn, &LocalSlot)>,
    hitboxes: Query<&Hitbox>,
    pawns: Query<(&Pawn, Has<Dead>)>,
) {
    for (cam, slot) in &cameras {
        let Some((me, mine, _)) = players.iter().find(|p| p.2.0 == slot.0) else { continue };
        let own = |e: Entity| hitboxes.get(e).map_or(true, |h| h.owner != me);
        let filter = crate::collision::bullet_filter();
        let hit = spatial.cast_ray_predicate(cam.translation(), cam.forward(), crate::units::u(8192.0), true, &filter, &own);
        let enemy = hit
            .and_then(|h| hitboxes.get(h.entity).ok())
            .and_then(|hb| pawns.get(hb.owner).ok())
            .is_some_and(|(p, dead)| !dead && hostile(p, mine));
        let h = if slot.0 == 0 { &mut *hud } else { extra.slot(slot.0) };
        if h.aim_enemy != enemy {
            h.aim_enemy = enemy;
        }
    }
}

/// A match started without the menus: load their fonts and materials for
/// the HUD.
fn load_assets(
    mut commands: Commands,
    fe: Option<Res<Frontend>>,
    mut task: Local<Option<JoinHandle<anyhow::Result<UiAssets>>>>,
    mut failed: Local<bool>,
) {
    if fe.is_some() || *failed {
        return;
    }
    match task.take() {
        None => *task = Some(std::thread::spawn(UiAssets::load)),
        Some(t) if t.is_finished() => match t.join() {
            Ok(Ok(assets)) => commands.insert_resource(Frontend::for_match(assets)),
            Ok(Err(e)) => {
                warn!("hud: can't load the HUD's fonts and materials: {e:#}");
                *failed = true;
            }
            Err(_) => *failed = true,
        },
        Some(t) => *task = Some(t),
    }
}

/// Once per map: make the match's zones' materials drawable (the compass,
/// the overlays), and the team icons the menus name.
pub(super) fn prepare(
    mut fe: ResMut<Frontend>,
    mut hud: ResMut<HudState>,
    content: Res<Content>,
    map: Res<MapName>,
    mut done: Local<Option<String>>,
) {
    if hud.menus.is_empty() {
        // The in-game menus live in common_mp, which the match has loaded.
        if !fe.ingame {
            fe.ingame = true;
            let t0 = std::time::Instant::now();
            let mut added = 0;
            // Only the menus the front end lacks are parsed.
            for zone in &content.zones {
                let menus = iw3::menu::UiData::menus_from_zone(zone, |name| !fe.assets.menus.contains_key(&name.to_ascii_lowercase()));
                for m in menus {
                    let key = m.window.name.to_ascii_lowercase();
                    if !fe.assets.menus.contains_key(&key) {
                        fe.assets.menus.insert(key, Arc::new(m));
                        added += 1;
                    }
                }
            }
            info!("ui: {added} in-game menus in {:.2?}", t0.elapsed());
        }
        // (`scorebars`, the team scores and clock, is [`score_panel`] now.)
        hud.menus = ["scoreboard"]
            .into_iter()
            .filter_map(|name| {
                let menu = fe.assets.menu(name)?;
                let shown = menu.items.iter().map(|it| it.window.dynamic_flags & iw3::menu::flags::VISIBLE != 0).collect();
                Some(OpenMenu { name: name.to_owned(), rows: vec![None; menu.items.len()], shown, menu })
            })
            .collect();
    }
    if done.as_deref() == Some(map.0.as_str()) {
        return;
    }
    *done = Some(map.0.clone());
    for zone in &content.zones {
        fe.assets.add_materials(zone);
    }
    // `localized_common_mp`'s team icons, airstrike selector and night
    // vision goggles.
    for (material, image) in [
        ("faction_128_usmc", "faction_128_usmc_silver"),
        ("faction_128_sas", "faction_128_sas_black"),
        ("faction_128_arab", "faction_128_arab_gold"),
        ("faction_128_ussr", "faction_128_russia_red"),
        ("map_artillery_selector", "artilleryarea"),
        ("nightvision_overlay_goggles", "nightvision_overlay_goggles"),
    ] {
        fe.assets.add_material(material, image);
    }
}

/// Each local player's HUD state past Player 1's ([`HudState`], which
/// also keeps what everyone shares: the obituaries, enemy gunfire on the
/// compass, the UAV's sweeps).
#[derive(Resource, Default)]
pub struct ExtraHuds(pub Vec<HudState>);

impl ExtraHuds {
    fn slot(&mut self, slot: usize) -> &mut HudState {
        let i = slot - 1;
        while self.0.len() <= i {
            self.0.push(HudState::default());
        }
        &mut self.0[i]
    }
}

/// Lay out this frame's HUD: each local player's in their part of the
/// window (the whole of it without splitscreen).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn paint(
    mut fe: ResMut<Frontend>,
    mut hud: ResMut<HudState>,
    mut extra: ResMut<ExtraHuds>,
    time: Res<Time>,
    window: Single<&Window, With<PrimaryWindow>>,
    gunplay: Res<Gunplay>,
    state: Option<Res<MatchState>>,
    minimap: Option<Res<Minimap>>,
    sides: Option<Res<crate::audio::Sides>>,
    players: Query<(
        (Entity, &Pawn, &Health, &WeaponState, &Mover, &Transform, &ViewAngles, Option<&Dead>, Option<&Loadout>, Has<AwaitingClass>),
        (&LocalSlot, &PlayerInput, Option<&crate::grenades::Grenades>, Option<&crate::grenades::Flashed>, Option<&crate::grenades::Stunned>),
        Option<&crate::killstreaks::Killstreak>,
    )>,
    pawns: Query<(Entity, &Pawn, &Transform, &ViewAngles, Has<Dead>, Option<&crate::bots::Bot>)>,
    live: Query<(&crate::grenades::LiveGrenade, &Transform)>,
    selecting: Option<Res<crate::killstreaks::airstrike::Selecting>>,
    (cameras, explosives, killcam, night_vision, objectives, radar, pickup_hints): (
        Query<(&SlotCamera, &Camera, &GlobalTransform, &Projection)>,
        Query<(&crate::explosives::Explosive, &GlobalTransform)>,
        Option<Res<crate::killcam::Killcam>>,
        Res<crate::vision::NightVisions>,
        Option<Res<crate::modes::Objectives>>,
        Option<Res<crate::killstreaks::uav::Radar>>,
        Query<(&LocalSlot, &crate::pickups::PickupHint)>,
    ),
) {
    let now = time.elapsed_secs();
    let ms = time.delta_secs() * 1000.0;
    hud.frame_ms = if hud.frame_ms <= 0.0 { ms } else { hud.frame_ms + (ms - hud.frame_ms) * 0.05 };
    // Hardcore (`ui_hud_hardcore`, `cg_drawCrosshair 0`): no compass but
    // while the team's UAV is up, crosshair, ammo, equipment, XP or
    // hardpoint icons; the HUD menus hide their own parts by the dvar.
    let hardcore = crate::tdm::hardcore();
    hud.compass = None;
    for h in &mut extra.0 {
        h.compass = None;
    }
    // Photo mode ([`crate::photo`]): no HUD at all.
    if crate::photo::active() {
        hud.ops.clear();
        for h in &mut extra.0 {
            h.ops.clear();
        }
        return;
    }
    // CoD4 hides the HUD under its menus (`ui_active`).
    // Headquarters has no combat HUD: its stations' names and prompt.
    if !fe.stack.is_empty() || crate::hq::active() {
        hud.ops.clear();
        if fe.stack.is_empty() {
            let mut p = Painter { fe: &fe, pl: Placement::new(window.width().max(1.0), window.height().max(1.0)), ops: Vec::new() };
            let camera = cameras.iter().find(|c| c.0.0 == 0).map(|c| (c.1, c.2));
            let input = players.iter().find(|(_, (s, ..), _)| s.0 == 0).map(|(_, (_, i, ..), _)| i);
            hq_overlay(&mut p, camera, input);
            hud.ops = p.ops;
        }
        return;
    }
    let count = crate::splitscreen::count();
    let killcam_on = killcam.as_ref().is_some_and(|k| k.showing());
    let ended = state.as_ref().and_then(|s| s.ended);
    // Player 1's own match info, put back after the others' are painted.
    let game = fe.game.clone();
    let mut all_ops = Vec::new();
    for slot in 0..count.max(1) {
        let (origin, size) = if count > 1 {
            crate::splitscreen::logical_rect(slot, count, &window)
        } else {
            (Vec2::ZERO, Vec2::new(window.width(), window.height()))
        };
        // A player with a menu up sees it, not their HUD.
        if fe.slot_menu_open(slot) {
            continue;
        }
        let me = players.iter().find(|(_, (s, ..), _)| s.0 == slot);
        {
            let own = if slot == 0 { &mut *hud } else { extra.slot(slot) };
            own.pickup = pickup_hints.iter().find(|h| h.0.0 == slot).and_then(|h| h.1.0.clone());
        }
        // This player's weapon name, shown a while after it changes.
        if let Some(((_, _, _, weapon, _, _, _, _, loadout, _), ..)) = me {
            let name = match loadout {
                Some(l) => l.gun().name.clone(),
                None => fe.assets.localize(&format!("@{}", weapon.def.display_key)),
            };
            let own = if slot == 0 { &mut *hud } else { extra.slot(slot) };
            if own.weapon.0 != name {
                own.weapon = (name, now);
            }
        }
        // The HUD menus' expressions read this player's score and name, and
        // whether they hold the scoreboard up.
        if let Some(((_, pawn, ..), (_, input, ..), _)) = me.filter(|_| slot > 0) {
            fe.game.mine = Some(if pawn.team == Team::Allies { 0 } else { 1 });
            fe.game.player_score = (pawn.kills * KILL_XP) as i32;
            fe.game.player_name = pawn.name.clone();
            fe.game.scoreboard = input.keys.pressed(KeyCode::Tab) || ended.is_some();
        }
        let shared: &HudState = &hud;
        let own: &HudState = if slot == 0 { &hud } else { extra.0.get(slot - 1).unwrap_or(shared) };
        let mut p = Painter { fe: &fe, pl: Placement::new(size.x.max(1.0), size.y.max(1.0)), ops: Vec::new() };
        let camera = cameras.iter().find(|c| c.0.0 == slot);
        let compass_view = paint_player(
            &mut p,
            (own, shared, slot),
            me,
            (&pawns, &live, &explosives, objectives.as_deref(), minimap.as_deref(), radar.as_deref(), sides.as_deref()),
            camera.map(|c| (c.1, c.2, c.3)),
            (&gunplay, &night_vision, killcam.as_deref(), state.as_deref(), selecting.is_some(), &window),
            hardcore,
            now,
        );
        if count > 1 {
            for op in &mut p.ops {
                op.offset(origin);
            }
        }
        all_ops.extend(p.ops);
        let compass_view = compass_view.map(|mut v| {
            v.pos += origin;
            v
        });
        if slot == 0 {
            hud.compass = compass_view;
        } else {
            extra.slot(slot).compass = compass_view;
        }
    }
    let _ = killcam_on;
    fe.game = game;
    hud.ops = all_ops;
}

/// One player's HUD, in their part of the window; returns where the
/// compass's map goes.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn paint_player(
    p: &mut Painter,
    (hud, shared, slot): (&HudState, &HudState, usize),
    me: Option<(
        (Entity, &Pawn, &Health, &WeaponState, &Mover, &Transform, &ViewAngles, Option<&Dead>, Option<&Loadout>, bool),
        (&LocalSlot, &PlayerInput, Option<&crate::grenades::Grenades>, Option<&crate::grenades::Flashed>, Option<&crate::grenades::Stunned>),
        Option<&crate::killstreaks::Killstreak>,
    )>,
    (pawns, live, explosives, objectives, minimap, radar, sides): (
        &Query<(Entity, &Pawn, &Transform, &ViewAngles, Has<Dead>, Option<&crate::bots::Bot>)>,
        &Query<(&crate::grenades::LiveGrenade, &Transform)>,
        &Query<(&crate::explosives::Explosive, &GlobalTransform)>,
        Option<&crate::modes::Objectives>,
        Option<&Minimap>,
        Option<&crate::killstreaks::uav::Radar>,
        Option<&crate::audio::Sides>,
    ),
    camera: Option<(&Camera, &GlobalTransform, &Projection)>,
    (gunplay, night_vision, killcam, state, selecting, window): (
        &Gunplay,
        &crate::vision::NightVisions,
        Option<&crate::killcam::Killcam>,
        Option<&MatchState>,
        bool,
        &Window,
    ),
    hardcore: bool,
    now: f32,
) -> Option<CompassView> {
    let fe = p.fe;
    let mut compass_view = None;
    let my_team = me.map_or(Team::Allies, |m| m.0.1.team);
    let ended = state.and_then(|s| s.ended);
    let killcam_on = killcam.is_some_and(|k| k.showing());
    let size = (p.pl.w, p.pl.h);
    if let Some(((e, _, health, weapon, mover, tf, view, dead, loadout, awaiting), (_, input, grenades, flashed, stunned), _)) = me {
        let alive = dead.is_none() && !awaiting && ended.is_none();
        // Night vision's goggles (`CG_DrawNightVisionOverlay`) and the fade
        // through black switching them, under the rest. The goggles are
        // drawn twice: blending in linear light, black at alpha a leaves
        // (1-a) of the colour where CoD4's gamma-space blend leaves about
        // (1-a)^2.2.
        {
            let night_vision = night_vision.get(slot);
            if night_vision.showing(now) {
                for _ in 0..2 {
                    p.image("nightvision_overlay_goggles", vr(0.0, 0.0, 640.0, 480.0, 4, 4), WHITE, 1);
                }
            }
            let black = night_vision.black(now);
            if black > 0.0 {
                p.image("white", vr(0.0, 0.0, 640.0, 480.0, 4, 4), [0.0, 0.0, 0.0, black], 1);
            }
        }
        if alive {
            // A stun's haze, a flashbang's white-out.
            if grenades.is_some() {
                if let Some(s) = stunned.map(|s| s.strength(now)).filter(|s| *s > 0.0) {
                    p.image("white", vr(0.0, 0.0, 640.0, 480.0, 4, 4), [0.55, 0.55, 0.6, 0.22 * s], 1);
                }
                if let Some(f) = flashed.map(|f| f.strength(now)).filter(|f| *f > 0.0) {
                    p.image("white", vr(0.0, 0.0, 640.0, 480.0, 4, 4), [1.0, 1.0, 1.0, f], 1);
                }
            }
            low_health(p, health.current, now);
            let uav = radar.is_some_and(|r| r.sweep_for(my_team, e, now).is_some());
            if let Some(m) = minimap.filter(|_| !hardcore || uav) {
                compass_view = compass(p, shared, m, (e, my_team, tf.translation, view.yaw), pawns, objectives, now);
            }
            if !hardcore {
                top_compass(p, shared, minimap.map_or(Vec2::X, |m| m.north), (e, my_team, tf.translation, view.yaw), pawns, now);
            }
            if !hardcore && modern_hud() {
                let mut held: Vec<(&'static str, u32)> = Vec::new();
                if let Some(g) = grenades {
                    held.push(("frag_grenade", g.frags));
                    if g.special.is_some() {
                        held.push(("tactical_grenade", g.specials));
                    }
                }
                let equipment = loadout.and_then(|l| l.equipment(weapon)).map(|(def, left)| (def.kill_icon.as_str(), left));
                modern::ammo(p, weapon, &hud.weapon.0, &held, equipment);
            } else if !hardcore && crate::tune::get("ammo.style", 1.0) > 0.5 {
                ammo_panel(p, weapon, &hud.weapon, now, grenades, loadout.and_then(|l| l.equipment(weapon)), me.and_then(|m| m.1.1.pad_kind));
            } else if !hardcore {
                ammo(p, weapon, &hud.weapon, now);
                if let Some(g) = grenades {
                    offhand(p, weapon, g, loadout.and_then(|l| l.equipment(weapon)));
                }
            }
            if !gunplay.is_bodycam() {
                if let Some((_, _, Projection::Perspective(proj))) = camera {
                    if !mover.sprinting && !hardcore && show(Hud::Crosshair) {
                        let color = if hud.aim_enemy { [1.0, 0.0, 0.0] } else { [1.0; 3] };
                        crosshair(p, weapon, mover, proj.fov, color);
                    }
                }
                // The hit marker: drawn crisp at any resolution, popping in,
                // red for a kill.
                if show(Hud::HitMarkers) {
                    hit_marker(p, now - hud.hit, now - hud.hit_start, now - hud.hit_kill, weapon.ads.clamp(0.0, 1.0));
                }
            }
            if show(Hud::DamageDirection) {
                damage_direction(p, hud, tf.translation, view.yaw, now);
            }
            // Objectives in the world, unless the killcam has the camera.
            if let (Some(o), Some((cam, cam_tf, _))) = (objectives.filter(|_| !killcam_on), camera) {
                objectives::waypoints(p, o, my_team, size, (cam, cam_tf));
            }
            grenade_danger(p, tf.translation, view.yaw, live);
            // Teammates' names over their heads, an enemy's under the crosshair.
            // (The campaign has none: its soldiers aren't players.)
            if let Some((cam, cam_tf, _)) = camera.filter(|_| !killcam_on && !crate::campaign::active()) {
                names::paint(p, slot, (cam, cam_tf));
            }
            if crate::perks::has(loadout, "specialty_detectexplosive") {
                if let Some((cam, cam_tf, _)) = camera {
                    bomb_squad(p, size, (e, my_team), tf.translation, (cam, cam_tf), explosives, pawns);
                }
            }
        } else if let Some(d) = dead.filter(|_| !awaiting && ended.is_none() && !killcam_on && !crate::campaign::active()) {
            // The center message (`centerobituary`).
            let height = 0.4583 * 48.0;
            if let Some(killer) = &d.killer {
                let text = loc(fe, "CGAME_YOUWEREKILLED", "Killed by &&1").replace("&&1", killer);
                p.text(&text, 0.0, 150.0, 2, 2, height, 0, WHITE, 0.5, true);
            }
            let left = (d.respawn_at - now).max(0.0).ceil();
            if left.is_finite() {
                p.text(&format!("Respawning in {left:.0}"), 0.0, 150.0 + height, 2, 2, height * 0.75, 0, WHITE, 0.5, true);
            } else if !crate::combat::free_for_all() && crate::netplay::authority() {
                // Out for the round, watching a teammate.
                let line = match crate::player::watching(slot) {
                    Some(name) => format!("Waiting for next round  -  watching {name} (Fire: next)"),
                    None => "Waiting for next round".to_owned(),
                };
                p.text(&line, 0.0, 150.0 + height, 2, 2, height * 0.75, 0, WHITE, 0.5, true);
            }
        }
        // The player's part in the objectives, and a round's result.
        if let Some(o) = objectives.filter(|_| !killcam_on) {
            objectives::status(p, o, e, my_team, tf.translation, alive, &input.use_key(), now);
        }
        // A dropped weapon to swap for (`PLATFORM_SWAPWEAPONS` and its name;
        // hardcore hides the hints).
        if let Some(key) = hud.pickup.as_ref().filter(|_| alive && !killcam_on && !hardcore) {
            let text = loc(fe, "PLATFORM_SWAPWEAPONS", "Press [{+activate}] to swap for")
                .replace("[{+activate}]", &input.use_key())
                .replace("&&1", &input.use_key());
            let name = fe.assets.localize(&format!("@{key}"));
            p.text(&format!("{text} {name}"), 0.0, 100.0, 2, 2, 0.4 * 48.0, 0, WHITE, 0.5, true);
        }
        // The kill streaks' asks (placing the sentry, opening a crate), and
        // a bar while a crate opens.
        if let Some((text, progress)) = crate::killstreaks::prompt(slot).filter(|_| alive && !killcam_on) {
            p.text(&text, 0.0, 70.0, 2, 2, 0.4 * 48.0, 0, WHITE, 0.5, true);
            if let Some(f) = progress {
                let (w, h) = (120.0, 6.0);
                p.outlined_box(vr(-w * 0.5, 80.0, w, h, 2, 2), [0.0, 0.0, 0.0, 0.5], 1.0, [1.0, 1.0, 1.0, 0.8]);
                p.outlined_box(vr(-w * 0.5, 80.0, w * f.clamp(0.0, 1.0), h, 2, 2), [1.0, 1.0, 1.0, 0.9], 0.0, [0.0; 4]);
            }
        }
    }
    // The campaign: its own HUD ([`crate::campaign`]), no match's.
    let campaign = crate::campaign::active();
    if show(Hud::KillFeed) && !campaign {
        obituaries(p, shared, now);
    }
    performance(p, shared);
    // The Modern HUD's scores and clock stand in for `scorebars` and
    // `scorebar`.
    let modern = modern_hud();
    if modern && !hardcore && !killcam_on && !campaign {
        let g = &fe.game;
        let (ours, theirs, labels) = if g.ffa {
            // The best score that isn't ours (or ours, when tied).
            let others: Vec<i32> = g.ranked.iter().copied().collect();
            let leader = others.iter().copied().filter(|&s| s != g.player_score).max().unwrap_or_else(|| others.first().copied().unwrap_or(0));
            (g.player_score, leader, ("YOU", "LEADER"))
        } else {
            let mine = g.mine.unwrap_or(0).min(1);
            (g.scores[mine], g.scores[1 - mine], ("US", "THEM"))
        };
        modern::scores(p, ours, theirs, labels, g.time_left);
    } else if !modern && !hardcore && !killcam_on && !campaign {
        score_panel(p, fe);
    }
    // The scoreboard waits for the final killcam.
    for om in shared.menus.iter().filter(|_| !campaign).filter(|m| !modern || m.name == "scoreboard").filter(|m| m.name != "scoreboard" || (fe.game.scoreboard && !killcam_on)) {
        fe.paint_menu(om, &p.pl, None, &mut p.ops);
    }
    if fe.game.scoreboard && !killcam_on && !campaign {
        scoreboard(p, pawns, me.map(|m| m.0.0), my_team, sides);
    }
    // The killcam's banner (`_killcam.gsc`).
    if let Some(b) = killcam.and_then(|k| k.banner()).filter(|_| slot == 0) {
        let height = 0.4583 * 48.0;
        p.text(b.title, 0.0, 80.0, 2, 1, 36.0, 0, [1.0, 0.85, 0.3, 1.0], 0.5, true);
        let line = format!("{}  [{}{}]  {}", b.killer, b.weapon, if b.headshot { " HS" } else { "" }, b.victim);
        p.text(&line, 0.0, -112.0, 2, 3, height, 0, WHITE, 0.5, true);
        if b.skippable {
            // [Use]: F, or the pad's X / Square (CoD4's console layout).
            let key = me.map_or_else(|| "F".to_owned(), |m| m.1.1.use_key());
            let prompt = loc(fe, "PLATFORM_PRESS_TO_SKIP", "Press [{+activate}] to skip").replace("[{+activate}]", &key);
            p.text(&prompt, 0.0, -90.0, 2, 3, height * 0.75, 0, WHITE, 0.5, true);
        }
    }
    if state.is_some() && !killcam_on && ended.is_some() {
        let outcome = state.zip(me).and_then(|(s, m)| s.outcome(m.0.1));
        let (key, fallback) = match outcome {
            Some(true) => ("MP_VICTORY", "Victory!"),
            Some(false) => ("MP_DEFEAT", "Defeat!"),
            None => ("MP_DRAW", "Draw"),
        };
        p.text(&loc(fe, key, fallback), 0.0, -40.0, 2, 2, 36.0, 0, WHITE, 0.5, true);
    }
    // XP, notifies and kill streak notices are Player 1's.
    if slot == 0 {
        if !hardcore {
            xp(p, hud, now);
        }
        notify(p, hud, now);
        streak_notice(p, hud, now);
        if let (Some(m), Some(((_, pawn, _, _, _, tf, view, ..), ..)), true) = (minimap, me, selecting) {
            location_map(p, m, (pawn.team, tf.translation, view.yaw), pawns, window.cursor_position());
        }
    }
    if let Some((streak, m)) = me.and_then(|m| Some((m.2?, m))).filter(|(_, m)| m.0.7.is_none() && !hardcore) {
        killstreak_panel(p, streak, m.1.1.pad_kind.is_some());
    }
    compass_view
}

/// Show, place and turn each player's compass map for this frame.
pub(super) fn update_compass(
    mut commands: Commands,
    mut fe: ResMut<Frontend>,
    hud: Res<HudState>,
    extra: Res<ExtraHuds>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<CompassMaterial>>,
    mut nodes: Query<(&CompassMap, &mut Node, &MaterialNode<CompassMaterial>, &mut Visibility)>,
) {
    let views: Vec<Option<&CompassView>> = std::iter::once(hud.compass.as_ref()).chain(extra.0.iter().map(|h| h.compass.as_ref())).collect();
    for (slot, view) in views.into_iter().enumerate() {
        let shown = view.and_then(|v| Some((v, fe.assets.material(&v.material, &mut images)?)));
        let Some((view, map)) = shown else {
            for (_, _, _, mut v) in nodes.iter_mut().filter(|n| n.0.0 == slot) {
                v.set_if_neq(Visibility::Hidden);
            }
            continue;
        };
        let node = Node {
            position_type: PositionType::Absolute,
            left: px(view.pos.x),
            top: px(view.pos.y),
            width: px(view.size.x),
            height: px(view.size.y),
            ..default()
        };
        let material = CompassMaterial {
            center_scale: Vec4::new(view.center.x, view.center.y, view.scale.x, view.scale.y),
            dir_alpha: Vec4::new(view.dir.x, view.dir.y, view.alpha, 0.0),
            map: map.handle,
            shape: view.shape.with_w(view.size.x * 0.5),
            outline_color: view.outline,
        };
        match nodes.iter_mut().find(|n| n.0.0 == slot) {
            Some((_, mut n, m, mut v)) => {
                n.set_if_neq(node);
                v.set_if_neq(Visibility::Inherited);
                if let Some(mut m) = materials.get_mut(&m.0) {
                    *m = material;
                }
            }
            None => {
                commands.spawn((Name::new("compass map"), CompassMap(slot), node, MaterialNode(materials.add(material)), GlobalZIndex(MINIMAP_Z)));
            }
        }
    }
}

/// A line over a player's menus naming whose they are (splitscreen), top
/// right of their part of the window (`pl`).
pub(super) fn menu_banner_ops(fe: &Frontend, pl: &Placement, text: &str) -> Vec<Op> {
    let mut p = Painter { fe, pl: *pl, ops: Vec::new() };
    p.image("white", vr(-112.0, 4.0, 104.0, 22.0, 3, 1), [0.0, 0.0, 0.0, 0.6], 1);
    p.text(text, -60.0, 21.0, 3, 1, 0.4 * 48.0, 0, [1.0, 0.85, 0.45, 1.0], 0.5, true);
    p.ops
}

/// A localized string, or `fallback` where the loaded zones lack it.
fn loc(fe: &Frontend, key: &str, fallback: &str) -> String {
    let s = fe.assets.localize(&format!("@{key}"));
    if s == key { fallback.to_owned() } else { s }
}

fn vr(x: f32, y: f32, w: f32, h: f32, horz_align: u8, vert_align: u8) -> VRect {
    VRect { x, y, w, h, horz_align, vert_align }
}

/// Builds the HUD's draw ops in CoD's virtual coordinates (aligns: 1
/// left/top, 2 centre, 3 right/bottom, 4 stretched).
struct Painter<'a> {
    fe: &'a Frontend,
    pl: Placement,
    ops: Vec<Op>,
}

impl Painter<'_> {
    fn image(&mut self, material: &str, r: VRect, color: [f32; 4], layer: u8) {
        self.image_uv(material, r, color, None, 0.0, layer);
    }

    /// With a source rectangle (0..1) and a clockwise turn.
    fn image_uv(&mut self, material: &str, r: VRect, color: [f32; 4], uv: Option<Rect>, rot: f32, layer: u8) {
        if color[3] <= 0.0 {
            return;
        }
        let (pos, size) = self.pl.rect(&r);
        self.ops.push(Op::Image { pos, size, material: material.to_owned(), color, uv, rot, layer });
    }

    /// A solid bar snapped to whole pixels (at least one), so thin lines
    /// stay sharp.
    fn bar(&mut self, r: VRect, color: [f32; 4]) {
        if color[3] <= 0.0 {
            return;
        }
        let (pos, size) = self.pl.rect(&r);
        let size = size.round().max(Vec2::ONE);
        let pos = (pos + (self.pl.rect(&r).1 - size) * 0.5).round();
        self.ops.push(Op::Image { pos, size, material: "white".to_owned(), color, uv: None, rot: 0.0, layer: 1 });
    }

    /// A box snapped to whole pixels: `fill` inside, an `edge` (virtual
    /// units, at least a pixel) outline of `line` round it, the outline's
    /// sides between its top and bottom so no pixel is drawn twice.
    fn outlined_box(&mut self, r: VRect, fill: [f32; 4], edge: f32, line: [f32; 4]) {
        let (pos, size) = self.pl.rect(&r);
        let (x0, y0) = (pos.x.round(), pos.y.round());
        let (x1, y1) = ((pos.x + size.x).round().max(x0 + 1.0), (pos.y + size.y).round().max(y0 + 1.0));
        let px = self.pl.rect(&vr(0.0, 0.0, edge, edge, 1, 1)).1.y;
        let e = if edge > 0.0 { px.round().clamp(1.0, ((x1 - x0) * 0.5).min((y1 - y0) * 0.5).floor().max(1.0)) } else { 0.0 };
        let mut quad = |x: f32, y: f32, w: f32, h: f32, color: [f32; 4]| {
            if color[3] > 0.0 && w > 0.0 && h > 0.0 {
                self.ops.push(Op::Image { pos: Vec2::new(x, y), size: Vec2::new(w, h), material: "white".to_owned(), color, uv: None, rot: 0.0, layer: 1 });
            }
        };
        quad(x0 + e, y0 + e, x1 - x0 - 2.0 * e, y1 - y0 - 2.0 * e, fill);
        if e > 0.0 {
            quad(x0, y0, x1 - x0, e, line);
            quad(x0, y1 - e, x1 - x0, e, line);
            quad(x0, y0 + e, e, y1 - y0 - 2.0 * e, line);
            quad(x1 - e, y0 + e, e, y1 - y0 - 2.0 * e, line);
        }
    }

    /// One line in the menus' typeface (Bahnschrift, drawn at its exact
    /// pixel size so it's crisp), its capitals centred on `y`, `height`
    /// virtual units tall; `align` as [`Self::text`]. CoD4's objective font
    /// where the PC lacks the face.
    #[allow(clippy::too_many_arguments)]
    fn text_hd(&mut self, text: &str, x: f32, y: f32, horz: u8, vert: u8, height: f32, cut: super::next::font::Cut, color: [f32; 4], align: f32, shadow: bool) -> f32 {
        if color[3] <= 0.0 || text.is_empty() {
            return 0.0;
        }
        let (pos, _) = self.pl.rect(&vr(x, y, 0.0, 0.0, horz, vert));
        let px = height * self.pl.sy(vert);
        let (font, k, cap) = match super::next::font::pick(cut, px) {
            Some((m, k)) => (m.font, k, m.cap * k),
            None => {
                let f = self.fe.font_for(6, px);
                (f, px / self.fe.assets.fonts[f].pixel_height as f32, px * 0.7)
            }
        };
        let Some(f) = self.fe.assets.fonts.get(font) else { return 0.0 };
        let w = draw::text_width(f, text, k);
        let shadow = if shadow { (self.pl.scale * 0.6).max(1.0) } else { 0.0 };
        self.ops.push(Op::Text { text: text.to_owned(), x: (pos.x - w * align).round(), y: (pos.y + cap * 0.5).round(), font, k, color, shadow });
        w / self.pl.sx(horz)
    }

    /// One line with its baseline at `y`, `height` virtual units tall; `align`
    /// 0 puts its left at `x`, 0.5 its middle, 1 its right. Returns its width
    /// in virtual units.
    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, text: &str, x: f32, y: f32, horz: u8, vert: u8, height: f32, font: i32, color: [f32; 4], align: f32, shadow: bool) -> f32 {
        let (pos, _) = self.pl.rect(&vr(x, y, 0.0, 0.0, horz, vert));
        let px = height * self.pl.sy(vert);
        let fi = self.fe.font_for(font, px);
        let Some(f) = self.fe.assets.fonts.get(fi) else { return 0.0 };
        let k = px / f.pixel_height as f32;
        let w = draw::text_width(f, text, k);
        if color[3] > 0.0 && !text.is_empty() {
            let shadow = if shadow { self.pl.scale } else { 0.0 };
            self.ops.push(Op::Text { text: text.to_owned(), x: pos.x - w * align, y: pos.y, font: fi, k, color, shadow });
        }
        w / self.pl.sx(horz)
    }
}

/// The hit marker, `since` seconds after the hit (`since_kill` after the
/// player's last kill; `ads` how far the player is aimed in): four tapered
/// ticks in an X round the crosshair, a smooth image made to the tuned shape
/// ([`hit_marker_image`]) with a dark outline for contrast. It pops in
/// (over-sized, then settling with a slight undershoot) and fades out; red
/// for a kill, which holds a little longer. Aimed in, it sits on the iron
/// sight's tip rather than the exact centre. Shape and timing are
/// `hitmarker.*` knobs in tuning.txt.
fn hit_marker(p: &mut Painter, since: f32, since_start: f32, since_kill: f32, ads: f32) {
    use crate::tune::get;
    let kill = since_kill <= since + 1e-4;
    let hold = get(if kill { "hitmarker.kill_hold" } else { "hitmarker.hold" }, if kill { 0.35 } else { 0.18 });
    let fade = get("hitmarker.fade", 0.3);
    if !(0.0..hold + fade).contains(&since) {
        return;
    }
    let alpha = if since < hold { 1.0 } else { 1.0 - (since - hold) / fade };
    // The pop: appearing, it starts `pop` bigger and springs back through a
    // small dip; each further hit while it shows (automatic fire) only
    // bumps it a little (`repop`), so it doesn't keep blowing up.
    let pop = get("hitmarker.pop", 0.12);
    let pop_time = get("hitmarker.pop_time", 0.11);
    let spring = |t: f32, amount: f32| {
        let x = (t / pop_time).min(1.0);
        amount * (1.0 - x).powi(3) - 0.06 * (x * std::f32::consts::PI).sin() * (1.0 - x) * amount / pop.max(1e-3)
    };
    let first = spring(since_start, pop);
    let bump = if since_start - since > 1e-4 { spring(since, get("hitmarker.repop", 0.05)) } else { 0.0 };
    let scale = 1.0 + first.max(bump);
    let (len, width, gap) = (get("hitmarker.length", 4.0), get("hitmarker.width", 1.2), get("hitmarker.gap", 5.0));
    let (outline, taper) = (get("hitmarker.outline", 1.1), get("hitmarker.taper", 0.35));
    let name = format!(
        "cod4rw_hitmarker_{}_{}_{}_{}_{}",
        (len * 100.0) as i32,
        (width * 100.0) as i32,
        (gap * 100.0) as i32,
        (outline * 100.0) as i32,
        (taper * 100.0) as i32
    );
    let r = (gap + len + width + outline + 1.0) * scale;
    let dy = get("hitmarker.ads_offset_y", 2.2) * ads;
    let color = if kill { [1.0, 0.13, 0.1, alpha] } else { [1.0, 1.0, 1.0, alpha] };
    p.image(&name, vr(-r, -r + dy, 2.0 * r, 2.0 * r, 2, 2), color, 1);
}

/// The hit marker's picture: white tapered ticks (`taper` of their width at
/// the inner end) on the diagonals, `gap` from the centre and `len` long,
/// with a black outline `outline` wide, smooth edged, on a transparent
/// square spanning `gap + len + width + outline + 1` virtual units each way
/// (the size [`hit_marker`] draws it at). Tinted, the ticks take the tint
/// and the outline stays dark.
pub(super) fn hit_marker_image(len: f32, width: f32, gap: f32, outline: f32, taper: f32) -> Image {
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    const N: u32 = 256;
    let r = gap + len + width + outline + 1.0;
    let px = 2.0 * r / N as f32;
    let mut data = vec![0u8; (N * N * 4) as usize];
    let (ri, ro) = (width * 0.5 * taper, width * 0.5);
    let ticks: Vec<(Vec2, Vec2)> = [Vec2::new(1.0, 1.0), Vec2::new(1.0, -1.0), Vec2::new(-1.0, 1.0), Vec2::new(-1.0, -1.0)]
        .iter()
        .map(|d| {
            let d = d.normalize();
            (d * gap, d * (gap + len))
        })
        .collect();
    // Signed distance to the nearest tick, a capsule tapering from `ri` at
    // its inner end to `ro` at its outer.
    let dist = |p: Vec2| {
        ticks
            .iter()
            .map(|&(a, b)| {
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
                (p - (a + ab * t)).length() - (ri + (ro - ri) * t)
            })
            .fold(f32::MAX, f32::min)
    };
    let cover = |d: f32| (0.5 - d / px).clamp(0.0, 1.0);
    for y in 0..N {
        for x in 0..N {
            let p = Vec2::new((x as f32 + 0.5) * px - r, (y as f32 + 0.5) * px - r);
            let d = dist(p);
            let fill = cover(d);
            let edge = cover(d - outline);
            let a = fill.max(edge);
            let i = ((y * N + x) * 4) as usize;
            let white = if a > 0.0 { fill / a } else { 0.0 };
            let v = (white * 255.0) as u8;
            data[i..i + 4].copy_from_slice(&[v, v, v, (a * 255.0) as u8]);
        }
    }
    let mut image = Image::new(
        Extent3d { width: N, height: N, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::linear();
    image
}

/// `overlay_low_health`, pulsing faster when critical
/// (`hud_health_pulserate_injured` 1 s, `_critical` 0.5 s below 33%).
fn low_health(p: &mut Painter, health: f32, now: f32) {
    let frac = (health / crate::combat::max_health()).clamp(0.0, 1.0);
    if frac >= 1.0 {
        return;
    }
    let rate = if frac < 0.33 { 0.5 } else { 1.0 };
    let pulse = 0.5 + 0.5 * (now * TAU / rate).cos();
    let alpha = (1.0 - frac) * (0.7 + 0.3 * pulse);
    p.image("overlay_low_health", vr(0.0, 0.0, 640.0, 480.0, 4, 4), [1.0, 1.0, 1.0, alpha], 0);
}

/// CoD space, flat.
fn flat(v: Vec3) -> Vec2 {
    let c = crate::units::to_cod(v);
    Vec2::new(c[0], c[1])
}

/// Bevy yaw -> the CoD-space direction faced.
fn facing(yaw: f32) -> Vec2 {
    let cod = yaw + FRAC_PI_2;
    Vec2::new(cod.cos(), cod.sin())
}

/// `compass_old`: the background, the heading tickertape, friendlies,
/// enemies heard firing and the player, around the map (drawn by its own
/// node from what this returns).
fn compass(
    p: &mut Painter,
    hud: &HudState,
    m: &Minimap,
    (me, team, at, yaw): (Entity, Team, Vec3, f32),
    pawns: &Query<(Entity, &Pawn, &Transform, &ViewAngles, Has<Dead>, Option<&crate::bots::Bot>)>,
    goals: Option<&crate::modes::Objectives>,
    now: f32,
) -> Option<CompassView> {
    use crate::tune::get;
    // Every knob listed in `tuning.txt` from the start, used or not yet
    // (no teammates or enemies around).
    for kind in ["player", "friendly", "enemy"] {
        get(&format!("minimap.{kind}_size"), COMPASS_ICON);
        for c in ["red", "green", "blue"] {
            get(&format!("minimap.{kind}_{c}"), 1.0);
        }
    }
    for (knob, default) in [("friendlies", 1.0), ("show_enemies", 0.0), ("enemy_ping_time", PING_TIME), ("uav_ping_time", crate::killstreaks::uav::PING_FADE), ("glow", 1.0), ("glow_size", 1.0), ("hd_icons", 1.0)] {
        get(&format!("minimap.{knob}"), default);
    }
    let range = get("minimap.range", COMPASS_RANGE).max(50.0);
    let round = get("minimap.round", 0.0).clamp(0.0, 1.0);
    let pos = flat(at);
    // The player's facing in the image, screen up on the compass; or
    // north, with the player's arrow turning (the settings' North Up).
    let rotating = show(Hud::MinimapRotates);
    let d = if rotating { m.image_dir(facing(yaw)) } else { m.image_dir(m.north) };
    let r = Vec2::new(-d.y, d.x);
    let to_compass = |world: Vec3| {
        let o = m.image_dir(flat(world) - pos) / range;
        Vec2::new(r.dot(o), -d.dot(o))
    };
    let turn = |yaw: f32| {
        let f = m.image_dir(facing(yaw));
        let s = Vec2::new(r.dot(f), -d.dot(f));
        s.x.atan2(-s.y)
    };
    // Heading: turns clockwise from north.
    let north = m.north.y.atan2(m.north.x);
    let f = facing(yaw);
    let heading = ((north - f.y.atan2(f.x)) / TAU).rem_euclid(1.0);
    let modern = modern_hud();
    // The map square (`minimap.x`/`y`/`size`, virtual units from the top
    // left); CoD4's frame and heading tape follow it.
    let [mx, my, mw, mh] = if modern {
        modern::map_rect()
    } else {
        let size = get("minimap.size", MAP_RECT[2]).max(10.0);
        [get("minimap.x", MAP_RECT[0]), get("minimap.y", MAP_RECT[1]), size, size]
    };
    if modern {
        modern::minimap_frame(p);
        modern::compass_rail(p, heading);
    } else {
        let k = mw / MAP_RECT[2];
        let frame = get("minimap.frame", 1.0).clamp(0.0, 1.0);
        if frame > 0.0 {
            p.image("minimap_background", vr(mx - 14.0 * k, my - 6.0 * k, 125.0 * k, 125.0 * k, 1, 1), [1.0, 1.0, 1.0, frame], 0);
        }
        let tape = get("minimap.heading_tape", 1.0).clamp(0.0, 1.0);
        if tape > 0.0 {
            p.image("minimap_tickertape_background", vr(mx, my - 15.0 * k, mw, 14.0 * k, 1, 1), [1.0, 1.0, 1.0, tape], 1);
            ticker(p, heading, (mx, my - 12.0 * k, mw, 9.0 * k), tape);
        }
    }
    let half = Vec2::new(mw, mh) * 0.5;
    let center = Vec2::new(mx, my) + half;
    // Whether a compass point (-1..1) is inside the map's shape.
    let inside = |s: Vec2| {
        let q = s.abs() - Vec2::splat(1.0 - round);
        q.max(Vec2::ZERO).length() + q.max_element().min(0.0) - round <= 0.0
    };
    let tint = |name: &str| [get(&format!("minimap.{name}_red"), 1.0), get(&format!("minimap.{name}_green"), 1.0), get(&format!("minimap.{name}_blue"), 1.0)];
    let icon = |p: &mut Painter, material: &str, s: Vec2, size: f32, rot: f32, alpha: f32| {
        let c = center + s * half;
        // The tuned size and colour of the player's, friendlies' and
        // enemies' marks (sizes grow with the map).
        let k = mw / MAP_RECT[2];
        let ([r, g, b], size) = match material {
            "compassping_player" => (tint("player"), get("minimap.player_size", size) * k),
            "compassping_friendly" => (tint("friendly"), get("minimap.friendly_size", size) * k),
            "compassping_enemy" => (tint("enemy"), get("minimap.enemy_size", size) * k),
            _ => ([1.0; 3], size * k),
        };
        let color = [r, g, b, alpha];
        // The Modern HUD's own player, friendly and enemy marks.
        let (own, size) = match material {
            "compassping_player" if modern => (Some("player_arrow"), modern::PLAYER_ICON),
            "compassping_friendly" if modern => (Some("friendly_arrow"), modern::FRIENDLY_ICON),
            "compassping_enemy" if modern => (Some("enemy_blip"), modern::ENEMY_ICON),
            _ => (None, size),
        };
        // Sharp drawn marks in place of CoD4's small textures
        // (`minimap.hd_icons 0` for those).
        let hd = match material {
            "compassping_player" | "compassping_friendly" | "compassping_enemy" if own.is_none() && get("minimap.hd_icons", 1.0) > 0.5 => {
                Some(format!("cod4rw_mmicon_{}_{}_{}", &material["compassping_".len()..], (get("minimap.glow", 1.0).clamp(0.0, 3.0) * 100.0) as i32, (get("minimap.glow_size", 1.0).clamp(0.0, 1.2) * 100.0) as i32))
            }
            _ => None,
        };
        let material = hd.as_deref().unwrap_or(material);
        let r = vr(c.x - size * 0.5, c.y - size * 0.5, size, size, 1, 1);
        match own {
            Some(name) => modern::sprite(p, name, r, color, rot, 1),
            None => p.image_uv(material, r, color, None, rot, 1),
        }
    };
    for (e, pawn, tf, view, dead, _) in pawns {
        let s = to_compass(tf.translation);
        if e == me || dead || !inside(s) {
            continue;
        }
        // `minimap.show_enemies 1`, and target practice's targets: every
        // enemy, always (a UAV that never ends).
        if pawn.team != team || crate::combat::free_for_all() {
            if crate::target_practice::active() || get("minimap.show_enemies", 0.0) > 0.5 {
                icon(p, "compassping_enemy", s, COMPASS_ICON, 0.0, 1.0);
            }
            continue;
        }
        if get("minimap.friendlies", 1.0) > 0.5 {
            icon(p, "compassping_friendly", s, COMPASS_ICON, turn(view.yaw), 1.0);
        }
    }
    for (&e, &(from, t)) in &hud.pings {
        let s = to_compass(from);
        if !inside(s) || pawns.get(e).is_ok_and(|x| x.4) {
            continue;
        }
        icon(p, "compassping_enemy", s, COMPASS_ICON, 0.0, 1.0 - (now - t) / get("minimap.enemy_ping_time", PING_TIME));
    }
    for &(at, t) in &hud.radar_pings {
        let s = to_compass(at);
        if inside(s) {
            icon(p, "compassping_enemy", s, COMPASS_ICON, 0.0, 1.0 - (now - t) / get("minimap.uav_ping_time", crate::killstreaks::uav::PING_FADE));
        }
    }
    if let Some(o) = goals {
        objectives::compass_icons(p, o, team, &to_compass, &icon);
    }
    icon(p, "compassping_player", Vec2::ZERO, COMPASS_ICON, if rotating { 0.0 } else { turn(yaw) }, 1.0);

    let (map_pos, map_size) = p.pl.rect(&vr(mx, my, mw, mh, 1, 1));
    Some(CompassView {
        material: m.material.clone(),
        pos: map_pos,
        size: map_size,
        center: m.uv(pos),
        scale: Vec2::splat(range) / m.size,
        dir: d,
        alpha: get("minimap.opacity", 1.0).clamp(0.0, 1.0),
        shape: Vec4::new(round, get("minimap.outline", 0.0).max(0.0) / (mw * 0.5), get("minimap.fill", 0.0).clamp(0.0, 1.0), 0.0),
        outline: Vec4::new(get("minimap.outline_red", 0.0), get("minimap.outline_green", 0.0), get("minimap.outline_blue", 0.0), get("minimap.outline_alpha", 1.0)),
    })
}

/// The compass bar along the top centre of the screen (`compass.*` in
/// `tuning.txt`): ticks every `tick_step` degrees across `span` degrees of
/// view, N/NE/E... at the eighths and degrees between, the heading under the
/// centre mark, and enemies heard firing or seen by a UAV (with
/// `show_enemies`, or in target practice, every enemy) as dots along it.
/// Fades out towards its ends.
fn top_compass(
    p: &mut Painter,
    hud: &HudState,
    north: Vec2,
    (me, team, at, yaw): (Entity, Team, Vec3, f32),
    pawns: &Query<(Entity, &Pawn, &Transform, &ViewAngles, Has<Dead>, Option<&crate::bots::Bot>)>,
    now: f32,
) {
    use crate::tune::get;
    if get("compass.enabled", 1.0) < 0.5 {
        return;
    }
    let width = get("compass.width", 240.0).max(20.0);
    let y = get("compass.y", 6.0);
    let height = get("compass.height", 16.0).max(2.0);
    let span = get("compass.span", 140.0).clamp(10.0, 360.0);
    let opacity = get("compass.opacity", 1.0).clamp(0.0, 1.0);
    let back = get("compass.background", 0.35).clamp(0.0, 1.0);
    let step = get("compass.tick_step", 15.0).max(1.0);
    let tick_h = get("compass.tick_height", 4.0);
    let major_h = get("compass.major_tick_height", 7.0);
    let tick_w = get("compass.tick_width", 0.8).max(0.1);
    let text = get("compass.text_size", 9.0).max(2.0);
    let degrees = get("compass.degrees", 1.0) > 0.5;
    let fade = get("compass.edge_fade", 0.3).clamp(0.0, 1.0);
    let color = [get("compass.red", 1.0), get("compass.green", 1.0), get("compass.blue", 1.0)];
    let accent = [get("compass.north_red", 1.0), get("compass.north_green", 0.82), get("compass.north_blue", 0.3)];
    let half = width * 0.5;
    // Bearings: turns clockwise from north.
    let north_a = north.y.atan2(north.x);
    let bearing = |v: Vec2| ((north_a - v.y.atan2(v.x)) / TAU).rem_euclid(1.0) * 360.0;
    let heading = bearing(facing(yaw));
    // A bearing's place along the bar (None past its ends), and how faded.
    let place = |deg: f32| {
        let rel = (deg - heading + 540.0).rem_euclid(360.0) - 180.0;
        let x = rel / span * width;
        (x.abs() <= half).then(|| {
            let edge = 1.0 - x.abs() / half;
            (x, if fade > 0.0 { (edge / fade).min(1.0) } else { 1.0 })
        })
    };
    let rgba = |c: [f32; 3], a: f32| [c[0], c[1], c[2], a * opacity];
    use super::next::font::Cut;
    let label_cut = if get("compass.bold", 1.0) > 0.5 { Cut::Semi } else { Cut::Regular };
    if back > 0.0 {
        // The backing fades out towards the ends with the ticks.
        let pieces = 24;
        for i in 0..pieces {
            let x0 = -half + width * i as f32 / pieces as f32;
            let mid = (x0 + width / pieces as f32 * 0.5).abs();
            let edge = 1.0 - mid / half;
            let a = if fade > 0.0 { (edge / fade).min(1.0) } else { 1.0 };
            p.image("white", vr(x0, y, width / pieces as f32, height, 2, 1), [0.0, 0.0, 0.0, back * opacity * a], 1);
        }
    }
    let names = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let count = (360.0 / step).round().max(1.0) as i32;
    for i in 0..count {
        let deg = i as f32 * 360.0 / count as f32;
        let Some((x, a)) = place(deg) else { continue };
        let eighth = (deg / 45.0).round();
        let major = (deg - eighth * 45.0).abs() < 0.01;
        let c = if major && eighth as i32 % 8 == 0 { accent } else { color };
        let h = if major { major_h } else { tick_h };
        p.bar(vr(x - tick_w * 0.5, y + height - h, tick_w, h, 2, 1), rgba(c, a));
        let label = if major { Some(names[eighth as usize % 8].to_owned()) } else { degrees.then(|| format!("{}", deg.round() as i32)) };
        if let Some(label) = label {
            let size = if major { text } else { text * 0.75 };
            let cut = if major { label_cut } else { Cut::Regular };
            p.text_hd(&label, x, y + (height - major_h) * 0.5, 2, 1, size, cut, rgba(c, a * if major { 1.0 } else { 0.7 }), 0.5, true);
        }
    }
    // The centre mark, and the heading under it.
    let mark = get("compass.marker", 1.0).clamp(0.0, 1.0);
    if mark > 0.0 {
        p.bar(vr(-tick_w, y + height, tick_w * 2.0, 4.0, 2, 1), rgba(accent, mark));
    }
    if get("compass.heading_number", 1.0) > 0.5 {
        let size = text * 0.8;
        p.text_hd(&format!("{}", heading.round() as i32 % 360), 0.0, y + height + 5.0 + size * 0.5, 2, 1, size, label_cut, rgba(color, 0.9), 0.5, true);
    }
    // Enemies: dots under the ticks.
    if get("compass.enemies", 1.0) < 0.5 {
        return;
    }
    let pos = flat(at);
    let dot = get("compass.enemy_size", 4.0).max(0.5);
    let red = [get("compass.enemy_red", 1.0), get("compass.enemy_green", 1.0), get("compass.enemy_blue", 1.0)];
    let mut show = |p: &mut Painter, world: Vec3, alpha: f32| {
        let d = flat(world) - pos;
        if d.length_squared() < 1.0 {
            return;
        }
        if let Some((x, a)) = place(bearing(d)) {
            p.image("cod4rw_mmicon_enemy_100_100", vr(x - dot, y + height - dot * 1.5 - 1.0, dot * 2.0, dot * 2.0, 2, 1), rgba(red, alpha * a), 1);
        }
    };
    let ping_time = get("minimap.enemy_ping_time", PING_TIME);
    for (&e, &(from, t)) in &hud.pings {
        if !pawns.get(e).is_ok_and(|x| x.4) {
            show(p, from, 1.0 - (now - t) / ping_time);
        }
    }
    let uav_time = get("minimap.uav_ping_time", crate::killstreaks::uav::PING_FADE);
    for &(at, t) in &hud.radar_pings {
        show(p, at, 1.0 - (now - t) / uav_time);
    }
    if crate::target_practice::active() || get("compass.show_enemies", 0.0) > 0.5 {
        for (e, pawn, tf, _, dead, _) in pawns {
            if e != me && !dead && (pawn.team != team || crate::combat::free_for_all()) {
                show(p, tf.translation, 1.0);
            }
        }
    }
}

/// The team scores and the clock (`score.*` in `tuning.txt`), in place of
/// CoD4's `scorebars`: a row each for us and them (free-for-all: the
/// player and the leader) with the side's icon, a bar filling towards the
/// score limit and the score, and the time left under them. `score.h` /
/// `score.v` pick the screen edge it's placed from (1 left/top, 2 centre,
/// 3 right/bottom), `score.x` / `score.y` the offset from there.
fn score_panel(p: &mut Painter, fe: &Frontend) {
    use super::next::font::Cut;
    use crate::tune::get;
    if get("score.enabled", 1.0) < 0.5 {
        return;
    }
    let g = &fe.game;
    let (horz, vert) = (get("score.h", 1.0).round().clamp(1.0, 3.0) as u8, get("score.v", 3.0).round().clamp(1.0, 3.0) as u8);
    let width = get("score.width", 120.0).max(20.0);
    let row = get("score.row_height", 11.0).max(3.0);
    let gap = get("score.row_gap", 2.0);
    let opacity = get("score.opacity", 1.0).clamp(0.0, 1.0);
    let back = get("score.background", 0.45).clamp(0.0, 1.0);
    let text = get("score.text_size", 10.0).max(3.0);
    let show_icons = get("score.icons", 1.0) > 0.5;
    let show_timer = get("score.timer", 1.0) > 0.5;
    let timer_size = get("score.timer_size", 12.0).max(3.0);
    let rgb = |name: &str, d: [f32; 3]| [get(&format!("score.{name}_red"), d[0]), get(&format!("score.{name}_green"), d[1]), get(&format!("score.{name}_blue"), d[2])];
    let ours_c = rgb("ours", [0.45, 0.85, 0.3]);
    let theirs_c = rgb("theirs", [0.95, 0.3, 0.22]);
    let text_c = rgb("text", [1.0, 1.0, 1.0]);
    // The whole panel's size, so it can sit against any edge.
    let rows_h = row * 2.0 + gap;
    let total_h = rows_h + if show_timer { gap * 2.0 + timer_size } else { 0.0 };
    let (x0, y0) = (
        get("score.x", 6.0) - match horz { 2 => width * 0.5, 3 => width, _ => 0.0 },
        get("score.y", -8.0) - match vert { 2 => total_h * 0.5, 3 => total_h, _ => 0.0 },
    );
    let at = |x: f32, y: f32, w: f32, h: f32| vr(x0 + x, y0 + y, w, h, horz, vert);
    let limit: f32 = fe.dvar("ui_scorelimit").parse().unwrap_or(0.0);
    let (rows, icons): ([(i32, [f32; 3], &str); 2], [String; 2]) = if g.ffa {
        let leader = g.ranked.iter().copied().filter(|&s| s != g.player_score).max().unwrap_or_else(|| g.ranked.first().copied().unwrap_or(0));
        ([(g.player_score, ours_c, "YOU"), (leader, theirs_c, "LEADER")], [String::new(), String::new()])
    } else {
        let mine = g.mine.unwrap_or(0).min(1);
        let icon = |i: usize| fe.dvar(if i == 0 { "g_TeamIcon_Allies" } else { "g_TeamIcon_Axis" }).to_owned();
        ([(g.scores[mine], ours_c, ""), (g.scores[1 - mine], theirs_c, "")], [icon(mine), icon(1 - mine)])
    };
    let most = rows[0].0.max(rows[1].0).max(1) as f32;
    let full = if limit > 0.0 { limit } else { most };
    for (i, ((score, color, label), icon)) in rows.iter().zip(&icons).enumerate() {
        let y = i as f32 * (row + gap);
        let mut x = 0.0;
        if show_icons && !icon.is_empty() {
            p.image(icon, at(0.0, y, row, row), [1.0, 1.0, 1.0, opacity], 1);
            x = row + gap;
        }
        let bar_w = width - x;
        if back > 0.0 {
            p.image("white", at(x, y, bar_w, row), [0.0, 0.0, 0.0, back * opacity], 1);
        }
        // The fill: how near the score limit.
        let frac = (*score as f32 / full).clamp(0.0, 1.0);
        let fill = get("score.fill", 0.75).clamp(0.0, 1.0);
        if frac > 0.0 && fill > 0.0 {
            p.image("white", at(x, y, bar_w * frac, row), [color[0], color[1], color[2], fill * opacity], 1);
        }
        // A thin edge of the side's colour at the bar's start.
        p.bar(at(x, y, 1.5, row), [color[0], color[1], color[2], opacity]);
        if !label.is_empty() {
            p.text_hd(label, x0 + x + 4.0, y0 + y + row * 0.5, horz, vert, text * 0.8, Cut::Semi, [text_c[0], text_c[1], text_c[2], 0.85 * opacity], 0.0, true);
        }
        p.text_hd(&score.to_string(), x0 + width - 3.0, y0 + y + row * 0.5, horz, vert, text, Cut::Semi, [text_c[0], text_c[1], text_c[2], opacity], 1.0, true);
    }
    if show_timer && g.time_left.is_finite() {
        let secs = g.time_left.max(0.0).ceil() as i32;
        let low = secs <= get("score.timer_warn", 30.0) as i32;
        let c = if low { theirs_c } else { text_c };
        let y = rows_h + gap * 2.0 + timer_size * 0.5;
        let align = match horz { 2 => 0.5, 3 => 1.0, _ => 0.0 };
        p.text_hd(&format!("{}:{:02}", secs / 60, secs % 60), x0 + width * align, y0 + y, horz, vert, timer_size, Cut::Semi, [c[0], c[1], c[2], opacity], align, true);
    }
}

/// The kill streaks, Advanced Warfare style (`killstreaks.*` in
/// `tuning.txt`; placed as [`score_panel`] is): all three always stacked up
/// the right side above the ammo, the best on top, each its icon in a box.
/// Not yet earned: greyed. Earned: lit, a green outline, and its key beside
/// it (on a pad, the d-pad; the picked one outlined in the picker's colour
/// while it's open). Beside the boxes, MW3's ladder: a pip per kill up to
/// the last streak's, lit for the kills of the streak so far, the pips that
/// earn a streak level with its box.
fn killstreak_panel(p: &mut Painter, streak: &crate::killstreaks::Killstreak, pad: bool) {
    use super::next::font::Cut;
    use crate::killstreaks::Hardpoint;
    use crate::tune::get;
    if get("killstreaks.enabled", 1.0) < 0.5 {
        return;
    }
    let (horz, vert) = (get("killstreaks.h", 3.0).round().clamp(1.0, 3.0) as u8, get("killstreaks.v", 3.0).round().clamp(1.0, 3.0) as u8);
    let size = get("killstreaks.box_size", 24.0).max(4.0);
    let gap = get("killstreaks.gap", 5.0);
    let icon = size * get("killstreaks.icon_scale", 0.8).clamp(0.1, 1.5);
    let text = get("killstreaks.text_size", 10.0).max(3.0);
    let opacity = get("killstreaks.opacity", 1.0).clamp(0.0, 1.0);
    let dim = get("killstreaks.dim", 0.3).clamp(0.0, 1.0);
    let back = get("killstreaks.background", 0.35).clamp(0.0, 1.0);
    let line = get("killstreaks.outline", 1.0).max(0.0);
    let glow = get("killstreaks.fill", 0.15).clamp(0.0, 1.0);
    let rgb = |name: &str, d: [f32; 3]| [get(&format!("killstreaks.{name}_red"), d[0]), get(&format!("killstreaks.{name}_green"), d[1]), get(&format!("killstreaks.{name}_blue"), d[2])];
    let ready_c = rgb("ready", [0.35, 1.0, 0.4]);
    let pick_c = rgb("picked", [1.0, 0.82, 0.3]);
    let text_c = rgb("text", [1.0, 1.0, 1.0]);
    let icon_c = rgb("icon", [1.0, 1.0, 1.0]);
    let rgba = |c: [f32; 3], a: f32| [c[0], c[1], c[2], a * opacity];
    let list = streak.held.list();
    let picked = streak.picker.and_then(|(i, _)| list.get(i).copied());
    // Best on top.
    let items: Vec<Hardpoint> = Hardpoint::ALL.into_iter().rev().collect();
    let total = items.len() as f32 * size + (items.len() as f32 - 1.0) * gap;
    let items = items.as_slice();
    // The boxes' right edge at `killstreaks.x`, the stack's bottom at `y`.
    // The ladder on the outer side of the boxes.
    let pips = get("killstreaks.pips", 1.0) > 0.5;
    let pip_w = get("killstreaks.pip_width", 5.0).max(0.5);
    let pip_space = get("killstreaks.pip_spacing", 3.0);
    let ladder = if pips { pip_w + pip_space } else { 0.0 };
    let x0 = get("killstreaks.x", -10.0) - match horz { 1 => -size - ladder, 2 => size * 0.5, _ => 0.0 } - size - if horz == 3 { ladder } else { 0.0 };
    let y0 = get("killstreaks.y", -58.0) - match vert { 2 => total * 0.5, 3 => total, _ => 0.0 };
    let at = |x: f32, y: f32, w: f32, h: f32| vr(x0 + x, y0 + y, w, h, horz, vert);
    // Labels sit left of the boxes (right of them when the stack is on the
    // left edge).
    let (label_x, align) = if horz == 1 { (x0 + size + 4.0, 0.0) } else { (x0 - 4.0, 1.0) };
    for (n, item) in items.iter().copied().enumerate() {
        let y = n as f32 * (size + gap);
        let ready = streak.held.has(item);
        let outline = if picked == Some(item) { Some(pick_c) } else { ready.then_some(ready_c) };
        if back > 0.0 {
            p.outlined_box(at(0.0, y, size, size), [0.0, 0.0, 0.0, back * opacity], 0.0, [0.0; 4]);
        }
        if let Some(c) = outline {
            p.outlined_box(at(0.0, y, size, size), rgba(c, glow), line, rgba(c, 1.0));
        }
        let pic = if get("killstreaks.hd_icons", 1.0) > 0.5 { item.hud_icon() } else { item.icon() };
        let a = if ready { 1.0 } else { dim };
        p.image(pic, at((size - icon) * 0.5, y + (size - icon) * 0.5, icon, icon), rgba(icon_c, a), 1);
        let cy = y0 + y + size * 0.5;
        if ready {
            let key = if pad { "D-PAD".to_owned() } else { crate::killstreaks::key_name(item) };
            let c = outline.unwrap_or(ready_c);
            p.text_hd(&key, label_x, cy, horz, vert, text, Cut::Semi, rgba(c, 1.0), align, true);
        } else if get("killstreaks.show_kills", 0.0) > 0.5 {
            p.text_hd(&item.kills().to_string(), label_x, cy, horz, vert, text * 0.85, Cut::Regular, rgba(text_c, dim + 0.15), align, true);
        }
    }
    if !pips {
        return;
    }
    // The ladder: beside each box a pip per kill it takes on from the
    // streak below (3 for the UAV, 1 more each for the rest), spread over
    // the box's height, kill 1 at the bottom; lit
    // for the kills of the streak so far, each pip outlined.
    let lx = if horz == 1 { -ladder } else { size + pip_space };
    let on_c = rgb("pip", [1.0, 0.85, 0.2]);
    let line_c = rgb("pip_outline", [0.9, 0.9, 0.9]);
    let off = get("killstreaks.pip_off", 0.35).clamp(0.0, 1.0);
    let pip_gap = get("killstreaks.pip_gap", 1.5).max(0.0);
    let edge = get("killstreaks.pip_outline", 0.75).max(0.0);
    let edge_a = get("killstreaks.pip_outline_alpha", 0.7).clamp(0.0, 1.0);
    for (n, item) in items.iter().enumerate() {
        let below = items.get(n + 1).map_or(0, |h| h.kills());
        let count = item.kills() - below;
        let each = size / count as f32;
        for j in 0..count {
            let k = below + j + 1;
            let y = n as f32 * (size + gap) + size - (j + 1) as f32 * each + pip_gap * 0.5;
            let h = (each - pip_gap).max(0.5);
            let lit = streak.kills >= k;
            let fill = if lit { rgba(on_c, 1.0) } else { [0.05, 0.05, 0.05, off * opacity] };
            // Lit pips edged dark, empty ones light.
            let line = if lit { [0.0, 0.0, 0.0, 0.6 * opacity] } else { rgba(line_c, edge_a) };
            p.outlined_box(at(lx, y, pip_w, h), fill, edge, line);
        }
    }
}

/// `minimapTicker`: the part of the heading tape around `heading` (turns
/// from north), wrapping round.
fn ticker(p: &mut Painter, heading: f32, (x, y, w, h): (f32, f32, f32, f32), alpha: f32) {
    let start = heading - TICKER_SPAN * 0.5;
    let end = start + TICKER_SPAN;
    let mut u = start;
    while u < end - 1e-5 {
        let next = (u.floor() + 1.0).min(end);
        let (x0, x1) = (x + (u - start) / TICKER_SPAN * w, x + (next - start) / TICKER_SPAN * w);
        let uv = Rect::new(u - u.floor(), 0.0, next - u.floor(), 1.0);
        p.image_uv("minimap_tickertape_mp", vr(x0, y, x1 - x0, h, 1, 1), [1.0, 1.0, 1.0, alpha], Some(uv), 0.0, 1);
        u = next;
    }
}

/// The ammo panel, MW2 (2022) style (`ammo.*` in `tuning.txt`,
/// `ammo.style 0` for CoD4's [`ammo`] and [`offhand`]; placed as
/// [`score_panel`] is, from its bottom right): left to right the gun's
/// silhouette, the rounds in the magazine big over the reserve (the
/// magazine's number turning `low_*` at a quarter left), a divider, then a
/// column each for the equipment, the frag and the special grenade: how
/// many, its icon, and the button that throws it.
#[allow(clippy::too_many_arguments)]
fn ammo_panel(
    p: &mut Painter,
    w: &WeaponState,
    (name, since): &(String, f32),
    now: f32,
    grenades: Option<&crate::grenades::Grenades>,
    equipment: Option<(&'static crate::weapons::WeaponDef, u32)>,
    pad: Option<crate::gamepad::PadKind>,
) {
    use super::next::font::Cut;
    use crate::bindings::Action;
    use crate::grenades::Kind;
    use crate::tune::get;
    let (horz, vert) = (get("ammo.h", 3.0).round().clamp(1.0, 3.0) as u8, get("ammo.v", 3.0).round().clamp(1.0, 3.0) as u8);
    let opacity = get("ammo.opacity", 1.0).clamp(0.0, 1.0);
    let height = get("ammo.height", 34.0).max(8.0);
    let clip_size = get("ammo.clip_text", 22.0).max(3.0);
    let reserve_size = get("ammo.reserve_text", 9.0).max(3.0);
    let icon = get("ammo.icon_size", 15.0).max(2.0);
    let count_size = get("ammo.count_text", 8.0).max(3.0);
    let key_size = get("ammo.key_text", 7.0).max(3.0);
    let column = get("ammo.column_width", 18.0).max(4.0);
    let gun_h = get("ammo.gun_height", 15.0).max(0.0);
    let gap = get("ammo.gap", 7.0);
    let rgb = |name: &str, d: [f32; 3]| [get(&format!("ammo.{name}_red"), d[0]), get(&format!("ammo.{name}_green"), d[1]), get(&format!("ammo.{name}_blue"), d[2])];
    let text_c = rgb("text", [1.0, 1.0, 1.0]);
    let low_c = rgb("low", [1.0, 0.35, 0.25]);
    let key_c = rgb("key", [0.8, 0.8, 0.8]);
    let rgba = |c: [f32; 3], a: f32| [c[0], c[1], c[2], a * opacity];
    // The columns, right to left: what each is, its count, its button.
    let pad_key = |a: Action| -> Option<&'static str> {
        let ps = pad == Some(crate::gamepad::PadKind::PlayStation);
        Some(match a {
            Action::Frag => if ps { "R1" } else { "RB" },
            Action::Special => if ps { "L1" } else { "LB" },
            Action::Equipment => "D-PAD",
            _ => return None,
        })
    };
    let key = |a: Action| if pad.is_some() { pad_key(a).unwrap_or("").to_owned() } else { crate::bindings::key_name(a) };
    let kind_icon = |k: Kind| match k {
        Kind::Frag => "oh:frag",
        Kind::Flash => "oh:flash",
        Kind::Stun => "oh:stun",
        Kind::Smoke => "oh:smoke",
    };
    let equipment_icon = |def: &crate::weapons::WeaponDef| {
        let n = def.name.to_ascii_lowercase();
        if n.contains("claymore") {
            "oh:claymore".to_owned()
        } else if n.contains("c4") {
            "oh:c4".to_owned()
        } else if n.contains("rpg") {
            "oh:rpg".to_owned()
        } else if n.starts_with("gl_") || n.contains("_gl") || n.contains("m203") || n.contains("gp25") {
            "oh:gl".to_owned()
        } else {
            def.kill_icon.clone()
        }
    };
    let mut columns: Vec<(String, u32, String)> = Vec::new();
    if let Some((def, left)) = equipment {
        columns.push((equipment_icon(def), left, key(Action::Equipment)));
    }
    if let Some(g) = grenades {
        columns.push((kind_icon(Kind::Frag).to_owned(), g.frags, key(Action::Frag)));
        if let Some(k) = g.special {
            columns.push((kind_icon(k).to_owned(), g.specials, key(Action::Special)));
        }
    }
    // The ammo block's width: the magazine's number (or the reserve's, if
    // wider), measured by drawing them where they go below.
    let (rx, by) = (get("ammo.x", -8.0), get("ammo.y", -6.0));
    let total_guess = columns.len() as f32 * column + gap * 2.0 + 40.0 + gun_h * 4.0;
    let right = match horz {
        1 => rx + total_guess,
        2 => rx + total_guess * 0.5,
        _ => rx,
    };
    let bottom = match vert {
        1 => by + height,
        2 => by + height * 0.5,
        _ => by,
    };
    let top = bottom - height;
    // Columns from the right.
    let mut x = right;
    for (pic, count, k) in columns.iter().rev() {
        let cx = x - column * 0.5;
        let have = *count > 0;
        p.text_hd(&count.to_string(), cx, top + count_size * 0.5, horz, vert, count_size, Cut::Semi, rgba(text_c, if have { 0.95 } else { 0.4 }), 0.5, true);
        let iy = top + count_size + 2.0;
        p.image(pic, vr(cx - icon * 0.5, iy, icon, icon, horz, vert), [1.0, 1.0, 1.0, if have { 0.95 * opacity } else { 0.3 * opacity }], 1);
        if get("ammo.keys", 1.0) > 0.5 && !k.is_empty() {
            p.text_hd(k, cx, bottom - key_size * 0.5, horz, vert, key_size, Cut::Semi, rgba(key_c, 0.85), 0.5, true);
        }
        x -= column;
    }
    // The divider.
    if !columns.is_empty() {
        x -= gap;
        let d = get("ammo.divider", 0.5).clamp(0.0, 1.0);
        if d > 0.0 {
            p.bar(vr(x - 0.5, top + height * 0.12, 1.0, height * 0.76, horz, vert), rgba(text_c, d));
        }
        x -= gap;
    }
    // The magazine over the reserve, right-aligned.
    let n = w.def.clip_size.max(1);
    let low = (w.clip as f32) <= n as f32 * get("ammo.low_at", 0.25);
    let clip_cy = top + clip_size * 0.55;
    let clip_w = p.text_hd(&w.clip.to_string(), x, clip_cy, horz, vert, clip_size, Cut::Display, rgba(if low { low_c } else { text_c }, 1.0), 1.0, true);
    let reserve_w = p.text_hd(&w.reserve.to_string(), x, bottom - reserve_size * 0.6, horz, vert, reserve_size, Cut::Semi, rgba(text_c, 0.7), 1.0, true);
    x -= clip_w.max(reserve_w) + gap;
    // The gun's silhouette (its kill feed icon), and its name over it after
    // a switch (`ammo.name_always 1`: always).
    if gun_h > 0.0 && !w.def.kill_icon.is_empty() {
        let ratio = [1.0, 2.0, 4.0][w.def.kill_icon_ratio.clamp(0, 2) as usize];
        let gw = gun_h * ratio;
        p.image(&w.def.kill_icon, vr(x - gw, top + (height - gun_h) * 0.5, gw, gun_h, horz, vert), [1.0, 1.0, 1.0, get("ammo.gun_alpha", 0.9) * opacity], 1);
        let t = now - since;
        let alpha = if get("ammo.name_always", 0.0) > 0.5 || t < NAME_SHOW { 1.0 } else { 1.0 - (t - NAME_SHOW) / NAME_FADE };
        if alpha > 0.0 && get("ammo.name", 1.0) > 0.5 {
            let size = get("ammo.name_text", 9.0).max(3.0);
            p.text_hd(name, x, top - size * 0.6, horz, vert, size, Cut::Semi, rgba(text_c, 0.85 * alpha), 1.0, true);
        }
    }
}

/// `weaponinfo`: the magazine's rounds (`ammoCounterClip`'s style), the
/// rounds in reserve and, after a switch, the weapon's name.
fn ammo(p: &mut Painter, w: &WeaponState, (name, since): &(String, f32), now: f32) {
    // Material, size, rounds per line, and whether lines are columns.
    let (material, size, per_line, columns) = match w.def.ammo_counter {
        2 => ("ammo_counter_riflebullet", Vec2::new(16.0, 4.0), 10, true),
        3 => ("ammo_counter_shotgunshell", Vec2::new(12.0, 6.0), 10, true),
        4 => ("ammo_counter_rocket", Vec2::new(32.0, 8.0), 5, true),
        5 => ("ammo_counter_beltbullet", Vec2::new(6.0, 3.0), 10, true),
        _ => ("ammo_counter_bullet", Vec2::new(3.0, 8.0), 30, false),
    };
    let n = w.def.clip_size as usize;
    let per = n.div_ceil(n.div_ceil(per_line).max(1)).max(1);
    // Rightmost first, from beside the reserve count; spent ones stay faint.
    let (right, bottom) = (-36.0, -4.0);
    for i in 0..n {
        let (line, k) = ((i / per) as f32, (i % per) as f32);
        let (x, y) = if columns {
            (right - (size.x + 1.0) * (line + 1.0), bottom - size.y * (k + 1.0))
        } else {
            (right - size.x * (k + 1.0), bottom - (size.y + 1.0) * (line + 1.0))
        };
        let alpha = if i < w.clip as usize { 0.65 } else { 0.15 };
        p.image(material, vr(x, y, size.x, size.y, 3, 3), [1.0, 1.0, 1.0, alpha], 1);
    }
    let text = 0.375 * 48.0;
    p.text(&w.reserve.to_string(), -34.0, -2.0, 3, 3, text, 6, [1.0, 1.0, 1.0, 0.75], 0.0, true);
    let t = now - since;
    let alpha = if t < NAME_SHOW { 1.0 } else { 1.0 - (t - NAME_SHOW) / NAME_FADE };
    if alpha > 0.0 {
        let y = bottom - (size.y + 1.0) * if columns { per as f32 } else { (n.div_ceil(per)) as f32 } - 4.0;
        p.text(name, -36.0, y.min(-16.0), 3, 3, text, 6, [1.0, 1.0, 1.0, 0.75 * alpha], 1.0, true);
    }
}

/// The grenades carried, an icon each, left of the ammo counter (CoD4's
/// offhand icons).
fn offhand(p: &mut Painter, w: &WeaponState, g: &crate::grenades::Grenades, equipment: Option<(&'static crate::weapons::WeaponDef, u32)>) {
    // Past the magazine's rounds (see [`ammo`]).
    let n = w.def.clip_size as f32;
    let width = match w.def.ammo_counter {
        2 | 3 => 2.0 * 17.0,
        4 => 2.0 * 33.0,
        5 => 10.0 * 6.0,
        _ => n.min(30.0) * 3.0,
    };
    let size = 22.0;
    let mut x = -36.0 - width - 10.0;
    let specials = g.special.map(|k| (k, g.specials));
    for (kind, count) in [(crate::grenades::Kind::Frag, g.frags)].into_iter().chain(specials) {
        for _ in 0..count {
            x -= size;
            p.image(kind.icon(), vr(x, -4.0 - size, size, size, 3, 3), [1.0, 1.0, 1.0, 0.8], 1);
        }
        x -= 6.0;
    }
    // The equipment (or launcher) on 5: its icon and what's left.
    if let Some((def, left)) = equipment {
        let text = 0.375 * 48.0 * 0.8;
        x -= 14.0;
        p.text(&left.to_string(), x + 12.0, -6.0, 3, 3, text, 6, [1.0, 1.0, 1.0, 0.75], 0.0, true);
        x -= size;
        p.image(&def.kill_icon, vr(x, -4.0 - size, size, size, 3, 3), [1.0, 1.0, 1.0, if left > 0 { 0.8 } else { 0.3 }], 1);
    }
}

/// The weapon's `reticleSide` pieces around the centre, as far out as the
/// spread reaches, fading as the sights come up; red over an enemy.
fn crosshair(p: &mut Painter, w: &WeaponState, mover: &Mover, fov: f32, [cr, cg, cb]: [f32; 3]) {
    let r = &w.def.reticle;
    let alpha = (1.0 - w.ads * 2.0).clamp(0.0, 1.0);
    let color = [cr, cg, cb, alpha];
    // 3rd Person TDM, the gun in a wall's way ([`crate::cover`]): the
    // reticle's pieces turned into an X, in red.
    if crate::cover::blocked() && !r.side.is_empty() {
        let side = r.side_size.max(1.0);
        for i in 0..4 {
            let a = FRAC_PI_4 + i as f32 * FRAC_PI_2;
            let c = Vec2::new(a.sin(), -a.cos()) * (side * 0.7);
            p.image_uv(&r.side, vr(c.x - side * 0.5, c.y - side * 0.5, side, side, 2, 2), [1.0, 0.15, 0.1, 1.0], None, a, 1);
        }
        return;
    }
    if r.side.is_empty() || alpha <= 0.0 {
        return;
    }
    // The spread cone's edge on the 480-unit-high screen.
    let spread = w.spread(mover).to_radians().tan() / (fov * 0.5).tan() * 240.0;
    tuned_crosshair(p, r.min_ofs + spread, color);
    if !r.center.is_empty() && r.center_size > 0.0 {
        let s = r.center_size;
        p.image(&r.center, vr(-s * 0.5, -s * 0.5, s, s, 2, 2), color, 1);
    }
}

/// The crosshair (both HUD styles): four ticks drawn to the shape in
/// `tuning.txt` (`crosshair.*`), as far out as the spread reaches (`out`,
/// virtual units), with an optional centre dot. The defaults match CoD4's
/// `reticle_side` ticks.
fn tuned_crosshair(p: &mut Painter, out: f32, [cr, cg, cb, ca]: [f32; 4]) {
    use crate::tune::get;
    let len = get("crosshair.length", 5.5).max(0.5);
    let width = get("crosshair.width", 1.4).max(0.2);
    let outline = get("crosshair.outline", 0.6).max(0.0);
    let round = get("crosshair.round", 0.4).clamp(0.0, 1.0);
    // How far out the ticks start: a fixed gap plus a share of the spread.
    let inner = get("crosshair.gap", -3.0) + get("crosshair.spread", 1.0) * out;
    let alpha = ca * get("crosshair.alpha", 0.9).clamp(0.0, 1.0);
    // Over an enemy the caller's red; otherwise the tuned colour.
    let color = if cg > 0.9 && cb > 0.9 {
        [get("crosshair.red", 1.0), get("crosshair.green", 1.0), get("crosshair.blue", 1.0), alpha]
    } else {
        [cr, cg, cb, alpha]
    };
    let name = format!("cod4rw_xhairtick_{}_{}_{}_{}", (len * 100.0) as i32, (width * 100.0) as i32, (outline * 100.0) as i32, (round * 100.0) as i32);
    let (w, h) = (width + 2.0 * outline + 2.0, len + 2.0 * outline + 2.0);
    for i in 0..4 {
        let a = i as f32 * FRAC_PI_2;
        let c = Vec2::new(a.sin(), -a.cos()) * (inner + len * 0.5);
        p.image_uv(&name, vr(c.x - w * 0.5, c.y - h * 0.5, w, h, 2, 2), color, None, a, 1);
    }
    let dot = get("crosshair.dot", 0.0);
    if dot > 0.0 {
        let name = format!("cod4rw_xhairtick_{}_{}_{}_100", (dot * 100.0) as i32, (dot * 100.0) as i32, (outline * 100.0) as i32);
        let s = dot + 2.0 * outline + 2.0;
        p.image(&name, vr(-s * 0.5, -s * 0.5, s, s, 2, 2), color, 1);
    }
}

/// One crosshair tick, upright: a `width` by `len` bar (virtual units) with
/// corners rounded by `round` (1: fully round ends), white with a black
/// `outline`, a unit of clear margin around it.
pub(super) fn crosshair_tick_image(len: f32, width: f32, outline: f32, round: f32) -> Image {
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    // Pixels per virtual unit: sharp up to 4K (4.5 px per unit).
    const RES: f32 = 8.0;
    let (w, h) = (width + 2.0 * outline + 2.0, len + 2.0 * outline + 2.0);
    let (nx, ny) = ((w * RES).ceil().max(2.0) as u32, (h * RES).ceil().max(2.0) as u32);
    let px = 1.0 / RES;
    let half = Vec2::new(width, len) * 0.5;
    let radius = round * width.min(len) * 0.5;
    // Signed distance to the rounded bar.
    let dist = |p: Vec2| {
        let q = p.abs() - half + Vec2::splat(radius);
        q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - radius
    };
    let cover = |d: f32| (0.5 - d / px).clamp(0.0, 1.0);
    let mut data = vec![0u8; (nx * ny * 4) as usize];
    for y in 0..ny {
        for x in 0..nx {
            let p = Vec2::new((x as f32 + 0.5) / nx as f32 * w - w * 0.5, (y as f32 + 0.5) / ny as f32 * h - h * 0.5);
            let d = dist(p);
            let fill = cover(d);
            let a = fill.max(cover(d - outline));
            let v = if a > 0.0 { (fill / a * 255.0) as u8 } else { 0 };
            let i = ((y * nx + x) * 4) as usize;
            data[i..i + 4].copy_from_slice(&[v, v, v, (a * 255.0) as u8]);
        }
    }
    let mut image = Image::new(
        Extent3d { width: nx, height: ny, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::linear();
    image
}

/// A minimap mark, drawn smooth at high resolution to the user's design:
/// `player` a lime arrow with a yellow edge, `friendly` a dark green arrow
/// with a bright green edge, `enemy` a red dot with a light rim; each with a
/// soft glow of its edge's colour, `glow_amount` strong (1: as designed)
/// and `glow_size` wide (1: as designed, 1.2 at most: the icon's edge).
pub(super) fn minimap_icon_image(kind: &str, glow_amount: f32, glow_size: f32) -> Image {
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    const N: u32 = 128;
    let px = 2.0 / N as f32;
    // Signed distance to a polygon (`pts` in order), in icon space -1..1.
    let polygon = |p: Vec2, pts: &[Vec2]| {
        let mut d = (p - pts[0]).length_squared();
        let mut inside = false;
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + pts.len() - 1) % pts.len()]);
            let e = b - a;
            let w = p - a;
            let t = (w.dot(e) / e.length_squared()).clamp(0.0, 1.0);
            d = d.min((w - e * t).length_squared());
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
        }
        if inside { -d.sqrt() } else { d.sqrt() }
    };
    // The arrow: a tip, two wide feet and a notch between them.
    let arrow = [Vec2::new(0.0, -0.64), Vec2::new(0.52, 0.64), Vec2::new(0.0, 0.3), Vec2::new(-0.52, 0.64)];
    let srgb = |r: u8, g: u8, b: u8| Vec3::new(r as f32, g as f32, b as f32) / 255.0;
    // Fill, edge and glow colours; the edge's width.
    let (fill, edge, glow, rim) = match kind {
        "enemy" => (srgb(253, 0, 0), srgb(255, 90, 85), srgb(255, 20, 10), 0.025),
        "friendly" => (srgb(29, 108, 3), srgb(85, 225, 45), srgb(60, 220, 30), 0.035),
        _ => (srgb(165, 250, 12), srgb(245, 252, 42), srgb(215, 250, 25), 0.035),
    };
    let reach = (0.3 * glow_size).max(1e-3);
    let mut data = vec![0u8; (N * N * 4) as usize];
    for y in 0..N {
        for x in 0..N {
            let p = Vec2::new((x as f32 + 0.5) * px - 1.0, (y as f32 + 0.5) * px - 1.0);
            let d = if kind == "enemy" { p.length() - 0.45 } else { polygon(p, &arrow) - 0.015 };
            let cover = |d: f32| (0.5 - d / px).clamp(0.0, 1.0);
            let shape = cover(d);
            let inner = cover(d + rim);
            // The glow: strongest at the edge, fading out over `GLOW`.
            let halo = ((1.0 - d.max(0.0) / reach).clamp(0.0, 1.0).powf(2.2) * 0.75 * glow_amount).min(1.0);
            let a = shape + halo * (1.0 - shape);
            let body = fill * inner + edge * (shape - inner);
            let rgb = if a > 0.0 { (body + glow * halo * (1.0 - shape)) / a } else { Vec3::ZERO };
            let i = ((y * N + x) * 4) as usize;
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            data[i..i + 4].copy_from_slice(&[byte(rgb.x), byte(rgb.y), byte(rgb.z), byte(a)]);
        }
    }
    let mut image = Image::new(
        Extent3d { width: N, height: N, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::linear();
    image
}

/// `hit_direction` arcs around the centre, towards whoever hurt the player.
fn damage_direction(p: &mut Painter, hud: &HudState, at: Vec3, yaw: f32, now: f32) {
    let f = facing(yaw);
    let right = Vec2::new(f.y, -f.x);
    for &(from, t) in &hud.hurt {
        let delta = flat(from) - flat(at);
        let a = delta.dot(right).atan2(delta.dot(f));
        let c = Vec2::new(a.sin(), -a.cos()) * DAMAGE_ICON_OFFSET;
        let alpha = ((DAMAGE_ICON_TIME - (now - t)) / 0.5).min(1.0);
        let r = vr(c.x - DAMAGE_ICON.x * 0.5, c.y - DAMAGE_ICON.y * 0.5, DAMAGE_ICON.x, DAMAGE_ICON.y, 2, 2);
        p.image_uv("hit_direction", r, [1.0, 1.0, 1.0, alpha], None, a, 1);
    }
}

/// Live grenades nearby (`CG_DrawGrenadeIndicators`): a frag within
/// `cg_hudGrenadeIconMaxRangeFrag` (250), a flashbang within
/// `cg_hudGrenadeIconMaxRangeFlash` (500): its icon
/// `cg_hudGrenadeIconOffset` (50) from the centre towards it, and a pointer
/// beyond it.
fn grenade_danger(p: &mut Painter, at: Vec3, yaw: f32, live: &Query<(&crate::grenades::LiveGrenade, &Transform)>) {
    use crate::grenades::Kind;
    let f = facing(yaw);
    let right = Vec2::new(f.y, -f.x);
    for (g, tf) in live {
        let (range, icon) = match g.kind {
            // The engine's `hud_grenadeicon` materials aren't in the
            // zones; their images are in the iwds.
            // Hardcore: none for frags (`cg_hudGrenadeIconMaxRangeFrag 0`).
            Kind::Frag if !crate::tdm::hardcore() => (250.0, "grenadeicon"),
            Kind::Flash => (500.0, "flashbangicon"),
            _ => continue,
        };
        if g.popped() || (tf.translation - at).length() / crate::units::INCH > range {
            continue;
        }
        let delta = flat(tf.translation) - flat(at);
        let a = delta.dot(right).atan2(delta.dot(f));
        let dir = Vec2::new(a.sin(), -a.cos());
        let (icon_size, offset) = (25.0, 50.0);
        let c = dir * offset;
        p.image(icon, vr(c.x - icon_size * 0.5, c.y - icon_size * 0.5, icon_size, icon_size, 2, 2), [1.0; 4], 1);
        // `cg_hudGrenadePointerWidth` x `Height` (25 x 12), out past the icon.
        let c = dir * (offset + icon_size * 0.5 + 8.0);
        p.image_uv("grenadepointer", vr(c.x - 12.5, c.y - 6.0, 25.0, 12.0, 2, 2), [1.0; 4], None, a, 1);
    }
}

/// Bomb Squad (`specialty_detectexplosive`, `_weapons.gsc`'s
/// `claymoreDetectionTrigger`): within 512 units across and 128 up or down
/// of an enemy's C4 or claymore, its `waypoint_bombsquad` icon 24 units over
/// it, through walls; the nearest four.
fn bomb_squad(
    p: &mut Painter,
    (w, h): (f32, f32),
    (me, my_team): (Entity, Team),
    at: Vec3,
    (cam, cam_tf): (&Camera, &GlobalTransform),
    explosives: &Query<(&crate::explosives::Explosive, &GlobalTransform)>,
    pawns: &Query<(Entity, &Pawn, &Transform, &ViewAngles, Has<Dead>, Option<&crate::bots::Bot>)>,
) {
    const ACROSS: f32 = 512.0;
    const UP_DOWN: f32 = 128.0;
    const MOST: usize = 4;
    let (w, h) = (w.max(1.0), h.max(1.0));
    let mut near: Vec<(f32, Vec3)> = explosives
        .iter()
        .filter(|(x, _)| matches!(x.weapon.as_str(), "c4_mp" | "claymore_mp"))
        .filter(|(x, _)| x.owner != me && pawns.get(x.owner).is_ok_and(|(_, owner, ..)| owner.team != my_team || crate::combat::free_for_all()))
        .filter_map(|(_, tf)| {
            let d = (tf.translation() - at) / crate::units::INCH;
            let across = Vec2::new(d.x, d.z).length();
            (across <= ACROSS && d.y.abs() <= UP_DOWN).then_some((across, tf.translation()))
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, x) in near.into_iter().take(MOST) {
        let Ok(px) = cam.world_to_viewport(cam_tf, x + Vec3::Y * crate::units::u(24.0)) else { continue };
        // 14 by 14, kept square on screen.
        let (sw, sh) = (14.0 / w * 640.0 * (h / 480.0), 14.0);
        let (vx, vy) = (px.x / w * 640.0, px.y / h * 480.0);
        p.image("waypoint_bombsquad", vr(vx - sw * 0.5, vy - sh * 0.5, sw, sh, 4, 4), WHITE, 1);
    }
}

/// Headquarters' stations ([`crate::hq::STATIONS`]): each one's name over it
/// while it's in view and not far, and "Press F to ..." at the one in reach.
fn hq_overlay(p: &mut Painter, camera: Option<(&Camera, &GlobalTransform)>, input: Option<&crate::splitscreen::PlayerInput>) {
    const SHOWN_WITHIN: f32 = 1000.0;
    if let Some((camera, view)) = camera {
        for s in crate::hq::STATIONS.iter().filter(|_| !crate::hq::revealing()) {
            let at = crate::units::pos(s.at);
            let far = view.translation().distance(at) / crate::units::u(1.0);
            if far > SHOWN_WITHIN {
                continue;
            }
            let Ok(px) = camera.world_to_viewport(view, at) else { continue };
            let alpha = (1.0 - (far - SHOWN_WITHIN * 0.6) / (SHOWN_WITHIN * 0.4)).clamp(0.0, 1.0) * 0.9;
            p.text(&crate::hq::label_of(s), px.x, px.y, 5, 5, 0.3 * 48.0 * p.pl.scale, 0, [1.0, 0.85, 0.45, alpha], 0.5, true);
        }
    }
    // Opening a drop: what came out, a line each in its rarity's colour,
    // and the prompt under them.
    // Opening a drop: its cards, dealt over the crate (the Supply Drops
    // menu's), with the prompt under them.
    let cards = crate::hq::drop_cards();
    if !cards.is_empty() {
        let fe = p.fe;
        fe.paint_reveal_cards(&p.pl, &mut p.ops, &cards, crate::hq::reveal_since());
    }
    if let Some(input) = input {
        let prompt_y = if cards.is_empty() { 100.0 } else { 225.0 };
        if let Some(text) = crate::hq::prompt(input) {
            p.text(&text, 0.0, prompt_y, 2, 2, 0.4 * 48.0, 0, WHITE, 0.5, true);
        }
        // How to call one in, with the hold filling a bar under it.
        if let Some((text, held)) = crate::hq::call_hint(input) {
            p.text(&text, 0.0, -34.0, 2, 3, 0.32 * 48.0, 0, [1.0, 0.85, 0.45, 0.9], 0.5, true);
            if held > 0.0 {
                p.image("white", vr(-60.0, -26.0, 120.0, 3.0, 2, 3), [0.0, 0.0, 0.0, 0.5], 1);
                p.image("white", vr(-60.0, -26.0, 120.0 * held, 3.0, 2, 3), [1.0, 0.85, 0.45, 0.95], 1);
            }
        }
    }
}

/// The settings' HUD Style: Modern ([`modern`]) or CoD4's own.
fn modern_hud() -> bool {
    crate::settings_apply::modern_hud()
}

/// A HUD switch from the settings.
fn show(which: Hud) -> bool {
    crate::settings_apply::hud(which)
}

/// The settings' performance overlay: frames a second (and the frame
/// time), top right, from a running average.
fn performance(p: &mut Painter, hud: &HudState) {
    let mode = crate::settings_apply::overlay();
    if mode == 0 || hud.frame_ms <= 0.0 {
        return;
    }
    let text = match mode {
        1 => format!("{:.0} FPS", 1000.0 / hud.frame_ms),
        _ => format!("{:.0} FPS  {:.1} ms", 1000.0 / hud.frame_ms, hud.frame_ms),
    };
    p.text(&text, -6.0, 16.0, 3, 1, 12.0, 0, [1.0, 1.0, 0.6, 0.9], 1.0, true);
}

/// The game message window: obituaries, newest at the bottom.
fn obituaries(p: &mut Painter, hud: &HudState, now: f32) {
    use crate::tune::get;
    let modern = modern_hud();
    // `killfeed.*` in `tuning.txt`: the lines' size and spacing, and where
    // the newest sits (from the bottom left).
    let (row, height) = if modern { modern::FEED_ROW } else { (get("killfeed.row", 15.0), get("killfeed.text_size", 11.0)) };
    let mut y = if modern { -38.0 } else { get("killfeed.y", -64.0) };
    for line in hud.obits.iter().rev() {
        let age = now - line.time;
        let alpha = ((OBIT_TIME - age) / OBIT_FADE_OUT).min(age / OBIT_FADE_IN).clamp(0.0, 1.0);
        let mut x = if modern { 6.0 } else { get("killfeed.x", 6.0) };
        // The Modern HUD's rows: a plate behind each line, as wide as it.
        if modern {
            let width = obituary_line(p, &line.parts, 0.0, y, height, 0.0);
            modern::feed_row(p, x, y - row * 0.74, width + 16.0, alpha);
            x += 8.0;
        }
        obituary_line(p, &line.parts, x, y, height, alpha);
        y -= row + if modern { 3.0 } else { 0.0 };
    }
}

/// One obituary from `x`, its baseline at `y` (bottom aligned); returns its
/// width. At alpha 0 it only measures.
fn obituary_line(p: &mut Painter, parts: &[Part], mut x: f32, y: f32, height: f32, alpha: f32) -> f32 {
    // The menus' typeface (`killfeed.hd 0`: CoD4's font), its capitals
    // centred where CoD4's sit on the baseline.
    let hd = crate::tune::get("killfeed.hd", 1.0) > 0.5;
    let mut text = |p: &mut Painter, s: &str, x: f32, color: [f32; 4]| {
        if hd {
            // (`text_hd` measures nothing at alpha 0: a trace of alpha keeps
            // the Modern HUD's measuring pass right.)
            let c = [color[0], color[1], color[2], color[3].max(0.0001)];
            p.text_hd(s, x, y - height * 0.36, 1, 3, height, super::next::font::Cut::Semi, c, 0.0, true)
        } else {
            p.text(s, x, y, 1, 3, height, 0, color, 0.0, true)
        }
    };
    let start = x;
    for part in parts {
        match part {
            Part::Text(s, [r, g, b]) => x += text(p, s, x, [*r, *g, *b, alpha]) + 3.0,
            Part::Loc { key, fallback, args } => {
                let mut s = loc(p.fe, key, fallback);
                for (k, a) in args.iter().enumerate() {
                    s = s.replace(&format!("&&{}", k + 1), a);
                }
                x += text(p, &s, x, [1.0, 1.0, 1.0, alpha]) + 3.0;
            }
            Part::Icon(material, ratio) => {
                let h = height * 1.25;
                p.image(material, vr(x, y - h + 2.0, h * ratio, h, 1, 3), [1.0, 1.0, 1.0, alpha], 1);
                x += h * ratio + 3.0;
            }
        }
    }
    x - start
}

/// The scoreboard's player list (drawn by CoD4's code under the
/// `scoreboard` menu's banner; `cg_scoreboardWidth` 400, banners 35 and
/// rows 18 high): per team, its icon, name, score and the column titles,
/// then a row per player by score: rank, name, score, kills, assists,
/// deaths and ping, a skull while dead, the player's own row lit.
fn scoreboard(
    p: &mut Painter,
    pawns: &Query<(Entity, &Pawn, &Transform, &ViewAngles, Has<Dead>, Option<&crate::bots::Bot>)>,
    me: Option<Entity>,
    mine: Team,
    sides: Option<&crate::audio::Sides>,
) {
    const LEFT: f32 = -200.0;
    const WIDTH: f32 = 400.0;
    const BANNER: f32 = 35.0;
    const ROW: f32 = 18.0;
    // Right edges of the number columns.
    const COLUMNS: [(&str, &str, f32); 5] = [
        ("CGAME_SB_SCORE", "Score", 236.0),
        ("CGAME_SB_KILLS", "Kills", 272.0),
        ("CGAME_SB_ASSISTS", "Assists", 316.0),
        ("CGAME_SB_DEATHS", "Deaths", 358.0),
        ("CGAME_SB_PING", "Ping", 396.0),
    ];
    let (rank, icon) = player_rank(p.fe);
    let mut y = 44.0;
    // Free-for-all: everyone in one list.
    let groups = if crate::combat::free_for_all() { vec![None] } else { vec![Some(mine), Some(mine.other())] };
    for group in groups {
        let (name, team_key) = group.map_or(("", ""), |team| faction(sides, team));
        let color = match name {
            "sas" => [0.5, 0.5, 0.5],
            "ussr" => [0.52, 0.28, 0.28],
            "arab" => [0.65, 0.57, 0.41],
            _ => [0.6, 0.64, 0.69],
        };
        let mut rows: Vec<_> = pawns.iter().filter(|r| group.is_none_or(|team| r.1.team == team)).collect();
        // By the Score shown, then kills, then fewest deaths.
        let points = |p: &Pawn| p.kills * KILL_XP + p.assists * ASSIST_SCORE;
        rows.sort_by(|a, b| points(b.1).cmp(&points(a.1)).then(b.1.kills.cmp(&a.1.kills)).then(a.1.deaths.cmp(&b.1.deaths)).then(a.1.name.cmp(&b.1.name)));
        p.image("line_horizontal_scorebar", vr(LEFT, y, WIDTH, BANNER, 2, 1), [color[0], color[1], color[2], 0.8], 1);
        match group {
            Some(team) => {
                // The team's score as the score bars have it (flags, rounds and
                // HQ points in the objective modes, not just kills).
                let score = p.fe.game.scores[(team == Team::Axis) as usize];
                p.image(&format!("faction_128_{name}"), vr(LEFT, y, BANNER, BANNER, 2, 1), WHITE, 1);
                let team_name = loc(p.fe, &format!("{team_key}_SHORT"), team.name());
                let w = p.text(&team_name, LEFT + BANNER + 4.0, y + 24.0, 2, 1, 0.4 * 48.0, 0, WHITE, 0.0, true);
                p.text(&score.to_string(), LEFT + BANNER + 14.0 + w, y + 24.0, 2, 1, 0.4 * 48.0, 0, WHITE, 0.0, true);
            }
            None => {
                let title = loc(p.fe, "MPUI_DEATHMATCH", "Free-for-all");
                p.text(&title, LEFT + 8.0, y + 24.0, 2, 1, 0.4 * 48.0, 0, WHITE, 0.0, true);
            }
        }
        for (key, fallback, right) in COLUMNS {
            let title = loc(p.fe, key, fallback);
            p.text(&title, LEFT + right, y + 31.0, 2, 1, 0.3 * 48.0, 0, WHITE, 1.0, true);
        }
        y += BANNER;
        for (k, (e, pawn, _, _, dead, bot)) in rows.into_iter().enumerate() {
            let own = Some(e) == me;
            let shade = if own {
                [0.9, 0.8, 0.3, 0.35]
            } else if k % 2 == 0 {
                [0.0, 0.0, 0.0, 0.5]
            } else {
                [0.12, 0.12, 0.12, 0.5]
            };
            p.image("white", vr(LEFT, y, WIDTH, ROW, 2, 1), shade, 1);
            // Bots: the rank that goes with their skill.
            let (r, rank_icon) = match bot {
                Some(b) if !own => (b.rank.0, rank_icon(p.fe, b.rank.0, b.rank.1)),
                _ if own => (rank, icon.clone()),
                _ => (0, "rank_pvt1".into()),
            };
            p.image(&rank_icon, vr(LEFT + 2.0, y + 1.0, 16.0, 16.0, 2, 1), WHITE, 1);
            p.text(&(r + 1).to_string(), LEFT + 32.0, y + 14.0, 2, 1, 0.25 * 48.0, 0, WHITE, 1.0, true);
            if dead {
                p.image("hud_status_dead", vr(LEFT + 35.0, y + 2.0, 14.0, 14.0, 2, 1), WHITE, 1);
            }
            let text = [1.0, 1.0, 1.0, if dead { 0.6 } else { 1.0 }];
            p.text(&pawn.name, LEFT + 52.0, y + 14.0, 2, 1, 0.3 * 48.0, 0, text, 0.0, true);
            let ping = if bot.is_some() { "BOT" } else { "0" };
            let values = [(pawn.kills * KILL_XP + pawn.assists * ASSIST_SCORE).to_string(), pawn.kills.to_string(), pawn.assists.to_string(), pawn.deaths.to_string(), ping.into()];
            for ((_, _, right), v) in COLUMNS.iter().zip(values) {
                p.text(&v, LEFT + right, y + 14.0, 2, 1, 0.3 * 48.0, 0, text, 1.0, true);
            }
            y += ROW;
        }
        y += 6.0;
    }
}

/// The player's rank (0-based) and its icon: `mp/rankIconTable.csv` by the
/// rank and prestige stats (`mp/playerStatsTable.csv`'s `rank`, `plevel`).
fn player_rank(fe: &Frontend) -> (i32, String) {
    let stat = |name: &str| fe.table_lookup("mp/playerStatsTable.csv", 1, name, 0).parse::<i32>().ok().map_or(0, |i| fe.stat(i));
    let (rank, prestige) = (stat("rank").max(0), stat("plevel").max(0));
    (rank, rank_icon(fe, rank, prestige))
}

/// A rank's icon (`mp/rankIconTable.csv`, a column per prestige).
pub(super) fn rank_icon(fe: &Frontend, rank: i32, prestige: i32) -> String {
    let icon = fe.table_lookup("mp/rankIconTable.csv", 0, &rank.to_string(), prestige + 1);
    if icon.is_empty() { "rank_pvt1".into() } else { icon }
}

/// The map an airstrike's spot is picked on (`fullscreenmap`): its items
/// sit at the menu's -100 -100 400 400 (centred) offset by the menu's own
/// rect, so 400 units square in the middle of the screen; `map_border`'s
/// frame takes the middle of that, and the map fills the frame's hole
/// (texels 18 to 46 of 64), under the instruction at y -160.
const LOCATION_BORDER: [f32; 4] = [-200.0, -200.0, 400.0, 400.0];
const LOCATION_MAP: [f32; 4] = [-200.0 + 400.0 * 18.0 / 64.0, -200.0 + 400.0 * 18.0 / 64.0, 400.0 * 28.0 / 64.0, 400.0 * 28.0 / 64.0];
/// `beginLocationSelection( "map_artillery_selector", artilleryDangerMaxRadius * 1.2 )`.
const SELECTOR_RADIUS: f32 = 450.0 * 1.2;

fn location_rect(pl: &Placement) -> (Vec2, Vec2) {
    let [x, y, w, h] = LOCATION_MAP;
    pl.rect(&vr(x, y, w, h, 2, 2))
}

/// CoD4's location selection, in `fullscreenmap`'s order: the minimap's
/// image, north up; the airstrike's reach (`map_artillery_selector`) under
/// the mouse; friendlies; the player; the border; the instruction.
fn location_map(
    p: &mut Painter,
    m: &Minimap,
    (team, at, yaw): (Team, Vec3, f32),
    pawns: &Query<(Entity, &Pawn, &Transform, &ViewAngles, Has<Dead>, Option<&crate::bots::Bot>)>,
    cursor: Option<Vec2>,
) {
    let [x, y, w, h] = LOCATION_MAP;
    p.image(&m.material.clone(), vr(x, y, w, h, 2, 2), WHITE, 1);
    let (pos, size) = location_rect(&p.pl);
    if let Some(c) = cursor.filter(|c| c.cmpge(pos).all() && c.cmple(pos + size).all()) {
        let r = SELECTOR_RADIUS / m.size.x * size.x;
        p.ops.push(Op::Image {
            pos: c - Vec2::splat(r),
            size: Vec2::splat(2.0 * r),
            material: "map_artillery_selector".into(),
            color: WHITE,
            uv: None,
            rot: 0.0,
            layer: 1,
        });
    }
    let north = m.north.y.atan2(m.north.x);
    // A pawn's place on the map, and its facing clockwise from north.
    let place = |pos: Vec3, view_yaw: f32| {
        let uv = m.uv(flat(pos));
        let f = facing(view_yaw);
        (Vec2::new(x + uv.x * w, y + uv.y * h), north - f.y.atan2(f.x))
    };
    for (_, pawn, tf, view, dead, _) in pawns {
        let (c, turn) = place(tf.translation, view.yaw);
        if pawn.team == team && !crate::combat::free_for_all() && !dead && (tf.translation - at).length() > 0.01 {
            p.image_uv("compassping_friendly", vr(c.x - 8.0, c.y - 8.0, 16.0, 16.0, 2, 2), WHITE, None, turn, 1);
        }
    }
    let (c, turn) = place(at, yaw);
    p.image_uv("compassping_player", vr(c.x - 9.0, c.y - 9.0, 18.0, 18.0, 2, 2), WHITE, None, turn, 1);
    let [bx, by, bw, bh] = LOCATION_BORDER;
    p.image("map_border", vr(bx, by, bw, bh, 2, 2), WHITE, 1);
    let text = loc(p.fe, "PLATFORM_PRESS_TO_SET_AIRSTRIKE", "Press Fire to set Air Strike location");
    p.text(&text, 0.0, -160.0, 2, 2, 0.4 * 48.0, 0, WHITE, 0.5, true);
}

/// Picking the spot: a click on the map (or its border, taken to the
/// map's edge) calls the airstrike on the ground there
/// ([`crate::killstreaks::airstrike::ground_below`]); a right click puts
/// the map away.
pub(super) fn pick_airstrike(
    mut commands: Commands,
    selecting: Option<Res<crate::killstreaks::airstrike::Selecting>>,
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    minimap: Option<Res<Minimap>>,
    spatial: SpatialQuery,
    mut inputs: Query<(&mut crate::killstreaks::HardpointInput, &Transform)>,
) {
    let (Some(sel), Some(m)) = (selecting, minimap) else { return };
    if mouse.just_pressed(MouseButton::Right) {
        commands.remove_resource::<crate::killstreaks::airstrike::Selecting>();
        return;
    }
    let Some(c) = window.cursor_position().filter(|_| mouse.just_pressed(MouseButton::Left)) else { return };
    let (pos, size) = location_rect(&Placement::new(window.width(), window.height()));
    let [bx, by, bw, bh] = LOCATION_BORDER;
    let (border_pos, border_size) = Placement::new(window.width(), window.height()).rect(&vr(bx, by, bw, bh, 2, 2));
    if !(c.cmpge(border_pos).all() && c.cmple(border_pos + border_size).all()) {
        return;
    }
    let uv = ((c - pos) / size).clamp(Vec2::ZERO, Vec2::ONE);
    let xy = m.northwest + m.east * uv.x * m.size.x - m.north * uv.y * m.size.y;
    if let Ok((mut input, tf)) = inputs.get_mut(sel.owner) {
        input.use_now = true;
        input.item = Some(crate::killstreaks::Hardpoint::Airstrike);
        input.target = Some(crate::killstreaks::airstrike::ground_below(&spatial, crate::units::pos([xy.x, xy.y, 0.0]), tf.translation.y));
    }
    commands.remove_resource::<crate::killstreaks::airstrike::Selecting>();
}

/// `_hud_message.gsc`'s `notifyMessage` for a hardpoint earned: "3 Kill
/// Streak!" over "Press [6] for RADAR.", top centre (objective font at 2.5
/// and 1.75), typed in a letter every 100 ms, shown 4 s and faded over one.
/// A hardpoint that can't be used now gets its `*_NOT_AVAILABLE` line.
fn streak_notice(p: &mut Painter, hud: &HudState, now: f32) {
    if let Some((streak, item, at)) = hud.streak_notify {
        let t = now - at;
        if t < 5.0 {
            let alpha = if t < 4.0 { 1.0 } else { 5.0 - t };
            let typed = |s: &str| s.chars().take((t / 0.1) as usize + 1).collect::<String>();
            let title = loc(p.fe, "MP_KILLSTREAK_N", "&&1 Kill Streak!").replace("&&1", &streak.to_string());
            let fallback = match item {
                Hardpoint::Uav => "Press [{+actionslot 4}] for RADAR.",
                Hardpoint::Airstrike => "Press [{+actionslot 4}] for AIRSTRIKE.",
                Hardpoint::Helicopter => "Press [{+actionslot 4}] for HELICOPTER.",
                Hardpoint::CarePackage => "Press [{+actionslot 4}] for CARE PACKAGE.",
                Hardpoint::Sentry => "Press [{+actionslot 4}] for SENTRY GUN.",
            };
            let text = loc(p.fe, item.hint(), fallback).replace("{+actionslot 4}", &crate::killstreaks::key_name(item));
            let (title_h, text_h) = (2.5 * 12.0, 1.75 * 12.0);
            p.text(&typed(&title), 0.0, 30.0 + title_h, 2, 1, title_h, 6, [1.0, 1.0, 1.0, alpha], 0.5, true);
            p.text(&typed(&text), 0.0, 30.0 + title_h + text_h, 2, 1, text_h, 6, [1.0, 1.0, 1.0, alpha], 0.5, true);
        }
    }
    if let Some((item, at)) = hud.unavailable.filter(|(_, at)| now - at < 3.0) {
        let (key, fallback) = match item {
            Hardpoint::Uav => ("MP_RADAR_NOT_AVAILABLE", "Radar not available."),
            Hardpoint::Airstrike => ("MP_AIRSTRIKE_NOT_AVAILABLE", "Airstrike not available."),
            Hardpoint::Helicopter => ("MP_HELICOPTER_NOT_AVAILABLE", "Helicopter not available."),
            Hardpoint::CarePackage => ("MP_CAREPACKAGE_NOT_AVAILABLE", "Care Package not available."),
            Hardpoint::Sentry => ("MP_SENTRY_NOT_AVAILABLE", "Sentry Gun not available."),
        };
        let alpha = (3.0 - (now - at)).min(1.0);
        p.text(&loc(p.fe, key, fallback), 0.0, -60.0, 2, 2, 0.4583 * 48.0, 0, [1.0, 1.0, 1.0, alpha], 0.5, true);
    }
}

/// A notify (`_hud_message.gsc`'s `notifyMessage`): the icon, a big title
/// and a line under it, for four seconds, fading.
fn notify(p: &mut Painter, hud: &HudState, now: f32) {
    let Some((title, text, icon, at)) = hud.notify.iter().find(|n| n.3 <= now && now - n.3 <= NOTIFY_TIME) else { return };
    let t = now - at;
    let alpha = (t / 0.25).min((NOTIFY_TIME - t) / 0.75).clamp(0.0, 1.0);
    if !icon.is_empty() {
        p.image(icon, vr(-32.0, 70.0, 64.0, 64.0, 2, 1), [1.0, 1.0, 1.0, alpha], 1);
    }
    p.text(title, 0.0, 160.0, 2, 1, 0.75 * 48.0, 0, [1.0, 0.85, 0.3, alpha], 0.5, true);
    p.text(text, 0.0, 182.0, 2, 1, 0.4583 * 48.0, 0, [1.0, 1.0, 1.0, alpha], 0.5, true);
}

/// How long a notify shows.
const NOTIFY_TIME: f32 = 4.0;

/// `_rank.gsc`'s score popup: "+10" pulsing to twice its size over three
/// frames and back over five, then fading.
fn xp(p: &mut Painter, hud: &HudState, now: f32) {
    let t = now - hud.xp.1;
    if hud.xp.0 == 0 || t > XP_SHOW + XP_FADE || !show(Hud::ScorePopups) {
        return;
    }
    let alpha = 0.85 * if t < XP_SHOW { 1.0 } else { 1.0 - (t - XP_SHOW) / XP_FADE };
    let pulse = if t < 0.15 { 1.0 + t / 0.15 } else if t < 0.4 { 2.0 - (t - 0.15) / 0.25 } else { 1.0 };
    // fontScale 2 (twelve units a unit of scale), centred on y -60.
    let height = 2.0 * pulse * 12.0;
    p.text(&format!("+{}", hud.xp.0), 0.0, -60.0 + height * 0.35, 2, 2, height, 0, [1.0, 1.0, 0.5, alpha], 0.5, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn killhouse_minimap_square() {
        let (nw, east, north, size) = square(Vec2::new(-504.0, 2936.0), Vec2::new(1648.0, -160.0), 90f32.to_radians());
        assert!((north - Vec2::Y).length() < 1e-5 && (east - Vec2::X).length() < 1e-5);
        // 2152 x 3096 widened to 3096 square about its middle.
        assert!((size - Vec2::splat(3096.0)).abs().max_element() < 0.01);
        assert!((nw - Vec2::new(572.0 - 1548.0, 2936.0)).abs().max_element() < 0.01);
    }
}
