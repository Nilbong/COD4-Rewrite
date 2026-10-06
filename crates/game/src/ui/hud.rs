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
use std::f32::consts::{FRAC_PI_2, TAU};
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

mod objectives;

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
        .add_systems(Update, (collect, collect_extra, load_assets, aim_check, pick_airstrike).run_if(in_game));
    if std::env::var_os("COD4RW_HUDTEST").is_some() {
        app.add_systems(
            Update,
            (hud_test.after(crate::player::InputSet).before(crate::weapons::WeaponSet), hud_test_log).run_if(in_game),
        );
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
pub(super) fn end_game(mut fe: Option<ResMut<Frontend>>) {
    if let Some(fe) = fe.as_mut() {
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
            // `MP_WAR_RADAR_ACQUIRED` and so on: to the caller's team, and
            // the enemy's UAV to everyone else.
            StreakNotice::CalledIn { item, by, team } => {
                let friendly = if crate::combat::free_for_all() { mine.as_ref().is_some_and(|m| m.name == *by) } else { *team == my_team };
                let (key, fallback, args) = match (item, friendly) {
                    (Hardpoint::Uav, true) => ("MP_WAR_RADAR_ACQUIRED", "UAV Recon called in by &&1 for &&2 seconds", vec![by.clone(), "30".into()]),
                    (Hardpoint::Uav, false) => ("MP_WAR_RADAR_ACQUIRED_ENEMY", "Enemy acquired UAV Recon for &&1 seconds", vec!["30".into()]),
                    (Hardpoint::Airstrike, true) => ("MP_WAR_AIRSTRIKE_INBOUND", "Airstrike called in by &&1", vec![by.clone()]),
                    (Hardpoint::Helicopter, true) => ("MP_HELICOPTER_INBOUND", "Helicopter called in by &&1", vec![by.clone()]),
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
    hud.pings.retain(|_, p| now - p.1 < PING_TIME);
    hud.radar_pings.retain(|p| now - p.1 < crate::killstreaks::uav::PING_FADE);
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
        hud.menus = ["scorebars", "scorebar", "scoreboard"]
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
    // CoD4 hides the HUD under its menus (`ui_active`).
    // Headquarters has no combat HUD.
    if !fe.stack.is_empty() || crate::hq::active() {
        hud.ops.clear();
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
                // `_damagefeedback.gsc`: shown, then faded over a second.
                let t = now - hud.hit;
                if t < 1.0 && show(Hud::HitMarkers) {
                    p.image("damage_feedback", vr(-12.0, -12.0, 24.0, 48.0, 2, 2), [1.0, 1.0, 1.0, 1.0 - t], 1);
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
            if crate::perks::has(loadout, "specialty_detectexplosive") {
                if let Some((cam, cam_tf, _)) = camera {
                    bomb_squad(p, size, (e, my_team), tf.translation, (cam, cam_tf), explosives, pawns);
                }
            }
        } else if let Some(d) = dead.filter(|_| !awaiting && ended.is_none() && !killcam_on) {
            // The center message (`centerobituary`).
            let height = 0.4583 * 48.0;
            if let Some(killer) = &d.killer {
                let text = loc(fe, "CGAME_YOUWEREKILLED", "Killed by &&1").replace("&&1", killer);
                p.text(&text, 0.0, 150.0, 2, 2, height, 0, WHITE, 0.5, true);
            }
            let left = (d.respawn_at - now).max(0.0).ceil();
            if left.is_finite() {
                p.text(&format!("Respawning in {left:.0}"), 0.0, 150.0 + height, 2, 2, height * 0.75, 0, WHITE, 0.5, true);
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
    }
    if show(Hud::KillFeed) {
        obituaries(p, shared, now);
    }
    performance(p, shared);
    // The scoreboard waits for the final killcam.
    for om in shared.menus.iter().filter(|m| m.name != "scoreboard" || (fe.game.scoreboard && !killcam_on)) {
        fe.paint_menu(om, &p.pl, None, &mut p.ops);
    }
    if fe.game.scoreboard && !killcam_on {
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
    if let Some(item) = me.and_then(|m| m.2).and_then(|s| s.held).filter(|_| me.is_some_and(|m| m.0.7.is_none()) && !hardcore) {
        // `dpad`'s `slot4` (`+actionslot 4`): the hardpoint's icon.
        p.image(item.icon(), vr(60.0, -43.0, 28.0, 28.0, 2, 3), [1.0, 1.0, 1.0, 0.65], 1);
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
            dir_alpha: Vec4::new(view.dir.x, view.dir.y, 1.0, 0.0),
            map: map.handle,
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
    let pos = flat(at);
    // The player's facing in the image, screen up on the compass; or
    // north, with the player's arrow turning (the settings' North Up).
    let rotating = show(Hud::MinimapRotates);
    let d = if rotating { m.image_dir(facing(yaw)) } else { m.image_dir(m.north) };
    let r = Vec2::new(-d.y, d.x);
    let to_compass = |world: Vec3| {
        let o = m.image_dir(flat(world) - pos) / COMPASS_RANGE;
        Vec2::new(r.dot(o), -d.dot(o))
    };
    let turn = |yaw: f32| {
        let f = m.image_dir(facing(yaw));
        let s = Vec2::new(r.dot(f), -d.dot(f));
        s.x.atan2(-s.y)
    };
    p.image("minimap_background", vr(-8.0, 12.0, 125.0, 125.0, 1, 1), WHITE, 0);
    p.image("minimap_tickertape_background", vr(6.0, 3.0, 102.0, 14.0, 1, 1), WHITE, 1);
    // Heading: turns clockwise from north.
    let north = m.north.y.atan2(m.north.x);
    let f = facing(yaw);
    ticker(p, ((north - f.y.atan2(f.x)) / TAU).rem_euclid(1.0));

    let [mx, my, mw, mh] = MAP_RECT;
    let half = Vec2::new(mw, mh) * 0.5;
    let center = Vec2::new(mx, my) + half;
    let icon = |p: &mut Painter, material: &str, s: Vec2, size: f32, rot: f32, alpha: f32| {
        let c = center + s * half;
        p.image_uv(material, vr(c.x - size * 0.5, c.y - size * 0.5, size, size, 1, 1), [1.0, 1.0, 1.0, alpha], None, rot, 1);
    };
    for (e, pawn, tf, view, dead, _) in pawns {
        let s = to_compass(tf.translation);
        if e == me || dead || pawn.team != team || crate::combat::free_for_all() || s.abs().max_element() > 1.0 {
            continue;
        }
        icon(p, "compassping_friendly", s, COMPASS_ICON, turn(view.yaw), 1.0);
    }
    for (&e, &(from, t)) in &hud.pings {
        let s = to_compass(from);
        if s.abs().max_element() > 1.0 || pawns.get(e).is_ok_and(|x| x.4) {
            continue;
        }
        icon(p, "compassping_enemy", s, COMPASS_ICON, 0.0, 1.0 - (now - t) / PING_TIME);
    }
    for &(at, t) in &hud.radar_pings {
        let s = to_compass(at);
        if s.abs().max_element() <= 1.0 {
            icon(p, "compassping_enemy", s, COMPASS_ICON, 0.0, 1.0 - (now - t) / crate::killstreaks::uav::PING_FADE);
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
        scale: Vec2::splat(COMPASS_RANGE) / m.size,
        dir: d,
    })
}

/// `minimapTicker`: the part of the heading tape around `heading` (turns
/// from north), wrapping round.
fn ticker(p: &mut Painter, heading: f32) {
    let (x, y, w, h) = (6.0, 6.0, 102.0, 9.0);
    let start = heading - TICKER_SPAN * 0.5;
    let end = start + TICKER_SPAN;
    let mut u = start;
    while u < end - 1e-5 {
        let next = (u.floor() + 1.0).min(end);
        let (x0, x1) = (x + (u - start) / TICKER_SPAN * w, x + (next - start) / TICKER_SPAN * w);
        let uv = Rect::new(u - u.floor(), 0.0, next - u.floor(), 1.0);
        p.image_uv("minimap_tickertape_mp", vr(x0, y, x1 - x0, h, 1, 1), WHITE, Some(uv), 0.0, 1);
        u = next;
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
    if r.side.is_empty() || alpha <= 0.0 {
        return;
    }
    // The spread cone's edge on the 480-unit-high screen.
    let spread = w.spread(mover).to_radians().tan() / (fov * 0.5).tan() * 240.0;
    let side = r.side_size.max(1.0);
    let out = r.min_ofs + spread + side * 0.5;
    for i in 0..4 {
        let a = i as f32 * FRAC_PI_2;
        let c = Vec2::new(a.sin(), -a.cos()) * out;
        p.image_uv(&r.side, vr(c.x - side * 0.5, c.y - side * 0.5, side, side, 2, 2), color, None, a, 1);
    }
    if !r.center.is_empty() && r.center_size > 0.0 {
        let s = r.center_size;
        p.image(&r.center, vr(-s * 0.5, -s * 0.5, s, s, 2, 2), color, 1);
    }
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
    let height = 11.0;
    let mut y = -64.0;
    for line in hud.obits.iter().rev() {
        let age = now - line.time;
        let alpha = ((OBIT_TIME - age) / OBIT_FADE_OUT).min(age / OBIT_FADE_IN).clamp(0.0, 1.0);
        let mut x = 6.0;
        for part in &line.parts {
            match part {
                Part::Text(s, [r, g, b]) => x += p.text(s, x, y, 1, 3, height, 0, [*r, *g, *b, alpha], 0.0, true) + 3.0,
                Part::Loc { key, fallback, args } => {
                    let mut s = loc(p.fe, key, fallback);
                    for (k, a) in args.iter().enumerate() {
                        s = s.replace(&format!("&&{}", k + 1), a);
                    }
                    x += p.text(&s, x, y, 1, 3, height, 0, [1.0, 1.0, 1.0, alpha], 0.0, true) + 3.0;
                }
                Part::Icon(material, ratio) => {
                    let h = height * 1.25;
                    p.image(material, vr(x, y - h + 2.0, h * ratio, h, 1, 3), [1.0, 1.0, 1.0, alpha], 1);
                    x += h * ratio + 3.0;
                }
            }
        }
        y -= height + 4.0;
    }
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
        rows.sort_by(|a, b| b.1.kills.cmp(&a.1.kills).then(a.1.deaths.cmp(&b.1.deaths)).then(a.1.name.cmp(&b.1.name)));
        p.image("line_horizontal_scorebar", vr(LEFT, y, WIDTH, BANNER, 2, 1), [color[0], color[1], color[2], 0.8], 1);
        match group {
            Some(team) => {
                let score: u32 = rows.iter().map(|r| r.1.kills).sum::<u32>() * KILL_XP;
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
fn rank_icon(fe: &Frontend, rank: i32, prestige: i32) -> String {
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

/// Picking the spot: a click on the map calls the airstrike on the ground
/// there; a right click puts the map away.
pub(super) fn pick_airstrike(
    mut commands: Commands,
    selecting: Option<Res<crate::killstreaks::airstrike::Selecting>>,
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    minimap: Option<Res<Minimap>>,
    spatial: SpatialQuery,
    mut inputs: Query<&mut crate::killstreaks::HardpointInput>,
) {
    let (Some(sel), Some(m)) = (selecting, minimap) else { return };
    if mouse.just_pressed(MouseButton::Right) {
        commands.remove_resource::<crate::killstreaks::airstrike::Selecting>();
        return;
    }
    let Some(c) = window.cursor_position().filter(|_| mouse.just_pressed(MouseButton::Left)) else { return };
    let (pos, size) = location_rect(&Placement::new(window.width(), window.height()));
    let uv = (c - pos) / size;
    if !(uv.cmpge(Vec2::ZERO).all() && uv.cmple(Vec2::ONE).all()) {
        return;
    }
    let xy = m.northwest + m.east * uv.x * m.size.x - m.north * uv.y * m.size.y;
    let top = crate::units::pos([xy.x, xy.y, 20000.0]);
    let ground = spatial.cast_ray(top, Dir3::NEG_Y, crate::units::u(40000.0), true, &crate::collision::sight_filter());
    let Some(hit) = ground else { return };
    if let Ok(mut input) = inputs.get_mut(sel.owner) {
        input.use_now = true;
        input.target = Some(top - Vec3::Y * hit.distance);
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
            };
            let text = loc(p.fe, item.hint(), fallback).replace("{+actionslot 4}", "6");
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
