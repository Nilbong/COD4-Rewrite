//! The frontend: CoD4's own menus from `ui_mp`, drawn with the game's fonts
//! and materials by an interpreter of the IW3 menu system. What is visible
//! and what it says comes from the compiled expressions ([`expr`]); what
//! clicking does comes from the menu scripts ([`script`]). Player stats
//! (custom classes, unlocks) persist in [`stats`]; Create a Class shows the
//! guns in 3D ([`preview`]). During a match the in-game class menus run on
//! the same interpreter ([`ingame`]), over CoD4's HUD ([`hud`]).

mod assets;
mod attachments;
mod bo1;
mod browser;
mod draw;
mod expr;
mod figures;
mod hud;
mod ingame;
mod lobby;
mod pad;
mod preview;
pub(crate) mod challenges;
pub(crate) mod progression;
mod options;
pub use options::{film_tint, lighting};
mod split;
mod scope;
pub use scope::{LENS_OUTER_FOV, ScopeCamera, lens_scopes};
mod script;
mod sound;
mod stats;
mod supply;
mod waw;

use crate::state::{GameState, in_game};
use crate::world::MapName;
use assets::UiAssets;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use draw::{DrawList, Placement, Quad, SpritePool};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use expr::{Env, Val, eval};
use iw3::menu::{Item, ItemData, Menu, Multi, Rect as VRect, Statement, Token, Window as MenuWindow, flags, item_type, op};
use preview::GunPreviews;
use script::Tok;
use stats::Stats;
use std::collections::HashMap;
use std::sync::Arc;

pub use ingame::{menu_open, no_ingame_menu};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DrawList>()
            .init_resource::<SpritePool>()
            .add_systems(OnEnter(GameState::Frontend), enter_frontend)
            .add_systems(
                Update,
                (browser::sync_servers, sync_devices, menu_input.in_set(MenuInput), paint_menus, update_previews).chain().run_if(in_state(GameState::Frontend)),
            )
            .add_systems(Update, loading_screen.run_if(in_state(GameState::Loading)))
            .add_systems(PostUpdate, draw::sync_sprites.run_if(not(in_game)))
            .add_systems(OnEnter(GameState::InGame), leave_frontend)
            .init_resource::<draw::NodePool>()
            .add_systems(
                Update,
                (
                    hud::prepare,
                    ingame::start,
                    menu_input.in_set(MenuInput),
                    ingame::update,
                    split::update,
                    hud::sync_game,
                    hud::paint,
                    hud::update_compass,
                    ingame::paint,
                )
                    .chain()
                    .run_if(in_game.and_then(resource_exists::<Frontend>)),
            )
            .add_systems(PostUpdate, draw::sync_nodes.run_if(in_game))
            // The match's UI nodes go with it (`crate::session`).
            .add_systems(OnExit(GameState::InGame), (draw::reset_nodes, hud::end_game))
            .add_systems(Update, sound::play.after(MenuInput).run_if(resource_exists::<Frontend>));
        // XP, ranks and promotions.
        progression::setup(app);
        challenges::setup(app);
        hud::build(app);
        pad::build(app);
        scope::build(app);
        options::build(app);
        if let Ok(dir) = std::env::var("COD4RW_UISHOT") {
            // Debug runs stay quiet.
            app.insert_resource(GlobalVolume::new(bevy::audio::Volume::SILENT));
            // Its key presses land as a real one would: after the input is
            // read, before the players' input is gathered from it.
            app.insert_resource(UiShotDir(dir.into())).init_resource::<SlotPresses>().add_systems(
                PreUpdate,
                (
                    ui_shots.after(bevy::input::InputSystems).before(crate::gamepad::PadSet).before(crate::splitscreen::InputGathered),
                    slot_presses.after(crate::splitscreen::InputGathered),
                ),
            );
        }
    }
}

/// Dvars the menus read, with values for a local player who has a profile.
const DEFAULT_DVARS: &[(&str, &str)] = &[
    ("com_playerProfile", "Player"),
    ("ui_playerProfileCount", "1"),
    ("ui_hint_text", "@MP_NULL"),
    ("ui_netGametypeName", "war"),
    ("g_gametype", "war"),
    ("ui_mapname", "mp_killhouse"),
    ("sv_hostname", "CoD4Host"),
    ("ui_dedicated", "0"),
    ("sv_maxclients", "18"),
    ("sv_minping", "0"),
    ("sv_maxping", "0"),
    ("sv_voice", "1"),
    ("scr_teambalance", "1"),
    ("g_allowvote", "1"),
    ("sv_punkbuster", "0"),
    ("customclass1", "@CLASS_SLOT1"),
    ("customclass2", "@CLASS_SLOT2"),
    ("customclass3", "@CLASS_SLOT3"),
    ("customclass4", "@CLASS_SLOT4"),
    ("customclass5", "@CLASS_SLOT5"),
];

#[derive(Component)]
struct FrontendCamera;

/// Mouse and keyboard on the menus, in the frontend and in a match.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
struct MenuInput;

struct OpenMenu {
    /// Lower-cased.
    name: String,
    menu: Arc<Menu>,
    /// Per-item `show`/`hide` state (`WINDOW_FLAG_VISIBLE`).
    shown: Vec<bool>,
    /// Attachment popup rows: (attachment stat, attachment) per item.
    rows: Vec<Option<(i32, String)>>,
}

/// The menu system's state: open menus, dvars, local vars and stats.
#[derive(Resource)]
pub struct Frontend {
    assets: UiAssets,
    dvars: HashMap<String, String>,
    locals: HashMap<String, String>,
    stats: Stats,
    previews: GunPreviews,
    /// Open menus, bottom first.
    stack: Vec<OpenMenu>,
    /// (menu, item) under the mouse.
    focus: Option<(String, usize)>,
    /// The text field being typed into.
    editing: Option<(String, usize)>,
    clock: std::time::Instant,
    /// Set by `uiScript StartServer`: the map to load.
    start: Option<String>,
    quit: bool,
    /// `disconnect` ran (Leave Game): back to the main menu.
    pub(super) leave: bool,
    /// The Private Match lobby's settings ([`lobby`]).
    lobby: lobby::Lobby,
    /// The local player's side in a match (for the menus' `team()`).
    player_team: crate::combat::Team,
    /// Splitscreen: whose the in-game menu is (picking their class), by
    /// their place.
    menu_slot: usize,
    /// The devices players can pick ([`crate::gamepad::Devices`]).
    devices: Vec<(crate::splitscreen::Device, String)>,
    /// Splitscreen: each local player's in-game menus ([`split`]), and
    /// whether the keyboard and mouse player has one up (the mouse is theirs).
    slot_menus: Vec<split::SlotMenu>,
    kbm_menu: bool,
    /// The match as the HUD menus' expressions see it.
    game: hud::GameInfo,
    /// `scriptMenuResponse`s for the game: (menu, response).
    responses: Vec<(String, String)>,
    /// A menu used Escape this frame.
    esc_used: bool,
    /// The in-game menus have been added.
    ingame: bool,
    /// Only for a match's HUD (started without the menus): no menus open.
    match_only: bool,
    /// Sounds and music for [`sound::play`].
    audio: sound::MenuAudio,
    /// CoD4's weapon popup the game choice just sent the player to, opened
    /// without asking again ([`bo1`]).
    bo1_bypass: Option<String>,
    /// A controller is in use: no mouse cursor is drawn ([`pad`]).
    pad: bool,
    /// The server browser's list ([`browser`]).
    servers: browser::ServerList,
    /// Where the mouse is (window pixels), for clicks inside list boxes.
    cursor: Option<Vec2>,
}

impl Env for Frontend {
    fn dvar(&self, name: &str) -> String {
        self.dvars.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default()
    }

    fn stat(&self, index: i32) -> i32 {
        self.stats.get(index)
    }

    fn local(&self, name: &str) -> String {
        self.locals.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default()
    }

    fn menu_open(&self, name: &str) -> bool {
        self.stack.iter().any(|m| m.name.eq_ignore_ascii_case(name)) || (self.game.scoreboard && name.eq_ignore_ascii_case("scoreboard"))
    }

    fn game(&self, f: u8, args: &[Val]) -> Option<Val> {
        self.game_value(f, args)
    }

    fn table_lookup(&self, table: &str, key_col: i32, key: &str, val_col: i32) -> String {
        let Some(t) = self.assets.table(table) else { return String::new() };
        let value = (0..t.rows)
            .find(|&r| t.get(r, key_col as usize).is_some_and(|v| v.eq_ignore_ascii_case(key)))
            .and_then(|r| t.get(r, val_col as usize))
            .unwrap_or("")
            .to_owned();
        // Create a Class's attribute bars show a class weapon's variant.
        if key_col == 1 && table.eq_ignore_ascii_case("mp/attributestable.csv") && !value.is_empty() {
            let delta = self.variant_attribute(key, val_col);
            if delta != 0 {
                return (num(&value) + delta).clamp(0, 100).to_string();
            }
        }
        value
    }

    fn localize(&self, text: &str) -> String {
        self.assets.localize(text)
    }

    fn millis(&self) -> i64 {
        self.clock.elapsed().as_millis() as i64
    }

    fn team(&self) -> String {
        match (self.ingame, self.player_team) {
            (false, _) => "TEAM_FREE",
            (true, crate::combat::Team::Allies) => "TEAM_ALLIES",
            (true, crate::combat::Team::Axis) => "TEAM_AXIS",
        }
        .into()
    }
}

/// Things to draw, resolved to sprites once materials are loaded.
enum Op {
    Fill { pos: Vec2, size: Vec2, color: [f32; 4] },
    Pic { pos: Vec2, size: Vec2, material: String, color: [f32; 4] },
    /// One line; `y` is the baseline.
    Text { text: String, x: f32, y: f32, font: usize, k: f32, color: [f32; 4], shadow: f32 },
    /// A material with a source rectangle (0..1 of the image), a clockwise
    /// turn and a layer (see [`draw::Quad`]).
    Image { pos: Vec2, size: Vec2, material: String, color: [f32; 4], uv: Option<Rect>, rot: f32, layer: u8 },
    /// A weapon's picture shown as a 3D preview (`key` is its weapon stat),
    /// with the 2D `picture` until the preview is ready.
    Gun { pos: Vec2, size: Vec2, key: i32, weapon: String, camo: usize, picture: String },
}

impl Op {
    /// Move it by `d` (window pixels): a splitscreen player's HUD into
    /// their part of the window.
    fn offset(&mut self, d: Vec2) {
        match self {
            Op::Fill { pos, .. } | Op::Pic { pos, .. } | Op::Image { pos, .. } | Op::Gun { pos, .. } => *pos += d,
            Op::Text { x, y, .. } => {
                *x += d.x;
                *y += d.y;
            }
        }
    }
}

fn num(s: &str) -> i32 {
    s.trim().parse::<f32>().map_or(0, |f| f as i32)
}

/// Which weapon a weapon picture expression shows: a class weapon stat N
/// for `tableLookup("mp/statstable.csv", 0, stat(N), 6)`, or
/// [`HIGHLIGHTED_WEAPON`] for the popups' picture of the weapon under the
/// mouse, `tableLookup("mp/statsTable.csv", 4, dvarString("ui_primary_highlighted"), 6)`.
fn weapon_picture_stat(exp: &Statement) -> Option<i32> {
    let from_stats_table = exp.iter().any(|t| matches!(t, Token::Str(s) if s.eq_ignore_ascii_case("mp/statstable.csv")));
    let image_column = exp.iter().rev().find_map(|t| match t {
        Token::Int(i) => Some(*i),
        _ => None,
    }) == Some(6);
    if !from_stats_table || !image_column {
        return None;
    }
    exp.windows(2).find_map(|w| match w {
        // Class weapon stats: 200 + 10 * class + 1 (primary) or 3 (secondary).
        [Token::Op(op::STAT), Token::Int(n)] if (201..250).contains(n) && matches!(n % 10, 1 | 3) => Some(*n),
        [Token::Op(op::DVARSTRING), Token::Str(d)] if d.eq_ignore_ascii_case("ui_primary_highlighted") => {
            Some(HIGHLIGHTED_WEAPON)
        }
        _ => None,
    })
}

/// Preview key for the weapon under the mouse in a weapon list popup.
const HIGHLIGHTED_WEAPON: i32 = 0;

/// Preview key for the character standing in Create a Class.
const CLASS_FIGURE: i32 = 2000;
/// Where it stands, in centre-aligned virtual units: the gap between the
/// option list and the class panel (both centre-aligned), above the bar.
const FIGURE_LEFT: f32 = -205.0;
const FIGURE_RIGHT: f32 = 68.0;
const FIGURE_TOP: f32 = 30.0;
const FIGURE_BOTTOM: f32 = 445.0;

/// The character the player wears (picked on the Character screen).
fn worn_character() -> &'static str {
    crate::supply::inventory().wearing().id
}


/// Can the mouse focus and click this item?
fn interactive(item: &Item) -> bool {
    item.window.static_flags & flags::DECORATION == 0 && !(item.ty == item_type::TEXT && item.action.is_empty())
}

fn multi_index(m: &Multi, current: &str) -> usize {
    let found = if m.str_def {
        m.strings.iter().position(|s| s.eq_ignore_ascii_case(current))
    } else {
        let v: f32 = current.trim().parse().unwrap_or(0.0);
        m.values.iter().position(|x| (x - v).abs() < 1e-4)
    };
    found.unwrap_or(0)
}

fn format_number(v: f32) -> String {
    if v.fract() == 0.0 { (v as i64).to_string() } else { v.to_string() }
}

impl Frontend {
    fn new(assets: UiAssets) -> Frontend {
        let mut fe = Frontend::bare(assets);
        fe.previews.start(fe.assets.vfs());
        fe.open("main");
        fe
    }

    /// For the HUD of a match started without the menus.
    fn for_match(assets: UiAssets) -> Frontend {
        Frontend { match_only: true, ..Frontend::bare(assets) }
    }

    fn bare(assets: UiAssets) -> Frontend {
        let previews = GunPreviews::default();
        // Debug runs (any `COD4RW_*` variable but the player's choice of
        // unlocks) don't keep stats: tests earn XP and kills that aren't
        // the player's.
        let debug = std::env::vars().any(|(k, _)| k.starts_with("COD4RW_") && k != "COD4RW_UNLOCKS" && !crate::net::setting(&k));
        let stats = Stats::load(&assets, !debug);
        let fe = Frontend {
            dvars: DEFAULT_DVARS
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), (*v).to_owned()))
                .chain(stats.dvars.clone())
                .collect(),
            locals: HashMap::new(),
            stats,
            editing: None,
            previews,
            assets,
            stack: Vec::new(),
            focus: None,
            clock: std::time::Instant::now(),
            start: None,
            quit: false,
            leave: false,
            lobby: lobby::Lobby::default(),
            player_team: crate::combat::Team::Allies,
            menu_slot: 0,
            devices: Vec::new(),
            slot_menus: Vec::new(),
            kbm_menu: false,
            game: hud::GameInfo::default(),
            responses: Vec::new(),
            esc_used: false,
            ingame: false,
            match_only: false,
            audio: sound::MenuAudio::default(),
            bo1_bypass: None,
            pad: false,
            servers: browser::ServerList::default(),
            cursor: None,
        };
        fe
    }

    fn set_dvar(&mut self, name: &str, value: &str) {
        let key = name.to_ascii_lowercase();
        if Stats::keeps(&key) {
            self.stats.set_dvar(&key, value);
        }
        self.dvars.insert(key, value.to_owned());
    }

    // --- scripts

    /// Run a menu script. Commands take a known number of arguments (see
    /// [`script::arity`]); `statSetUsingTable` takes a call expression.
    fn run(&mut self, script: &str, owner: &str) {
        let toks = script::tokenize(script);
        let mut i = 0;
        while i < toks.len() {
            let Tok::Word(cmd) = &toks[i] else {
                i += 1;
                continue;
            };
            i += 1;
            let lower = cmd.to_ascii_lowercase();
            if lower == "statsetusingtable" {
                let args = self.call_args(&toks, &mut i);
                if let [stat, value, ..] = &args[..] {
                    self.stats.set(num(stat), num(value));
                }
                continue;
            }
            let mut args = vec![cmd.clone()];
            let take = script::arity(&lower);
            while take.is_none_or(|n| args.len() <= n) {
                match toks.get(i) {
                    Some(Tok::Word(w)) => {
                        args.push(w.clone());
                        i += 1;
                    }
                    _ => break,
                }
            }
            self.command(&args, owner);
        }
    }

    /// Arguments of `( a , f ( b , c ) )`, running `tableLookup` calls.
    fn call_args(&self, toks: &[Tok], i: &mut usize) -> Vec<String> {
        let mut out = Vec::new();
        if toks.get(*i) != Some(&Tok::Punct('(')) {
            return out;
        }
        *i += 1;
        while let Some(t) = toks.get(*i) {
            *i += 1;
            match t {
                Tok::Punct(')') => break,
                Tok::Punct(_) => {}
                Tok::Word(w) if toks.get(*i) == Some(&Tok::Punct('(')) => {
                    let inner = self.call_args(toks, i);
                    out.push(if w.eq_ignore_ascii_case("tablelookup") && inner.len() >= 4 {
                        self.table_lookup(&inner[0], num(&inner[1]), &inner[2], num(&inner[3]))
                    } else {
                        inner.into_iter().next().unwrap_or_default()
                    });
                }
                Tok::Word(w) => out.push(w.clone()),
            }
        }
        out
    }

    fn exec(&mut self, console: &str) {
        for c in script::parse(console) {
            self.console(&c);
        }
    }

    fn command(&mut self, args: &[String], owner: &str) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        match a(0).to_ascii_lowercase().as_str() {
            "open" => self.open(a(1)),
            "close" => {
                let name = if a(1).eq_ignore_ascii_case("self") { owner } else { a(1) };
                self.close(&name.to_owned());
            }
            "setlocalvarint" | "setlocalvarbool" | "setlocalvarfloat" | "setlocalvarstring" => {
                self.locals.insert(a(1).to_ascii_lowercase(), a(2).to_owned());
            }
            "setdvar" => self.set_dvar(a(1), a(2)),
            "exec" | "execnow" => self.exec(a(1)),
            "execondvarstringvalue" | "execnowondvarstringvalue" => {
                if self.dvar(a(1)).eq_ignore_ascii_case(a(2)) {
                    self.exec(a(3));
                }
            }
            "execondvarintvalue" | "execnowondvarintvalue" | "execondvarfloatvalue" | "execnowondvarfloatvalue" => {
                if Val::Str(self.dvar(a(1))).num() == Val::Str(a(2).to_owned()).num() {
                    self.exec(a(3));
                }
            }
            // Clear or set the bits named by one dvar in the stat named by another.
            "statclearbitmask" | "statsetbitmask" => {
                let (stat, mask) = (num(&self.dvar(a(1))), num(&self.dvar(a(2))));
                let v = self.stats.get(stat);
                self.stats.set(stat, if a(0).eq_ignore_ascii_case("statclearbitmask") { v & !mask } else { v | mask });
            }
            "uiscript" => self.ui_script(&args[1..]),
            "show" | "fadein" => self.show_item(owner, a(1), true),
            "hide" | "fadeout" => self.show_item(owner, a(1), false),
            "setfocus" => {
                // Focusing a text field starts typing into it.
                let field = self.menu_item(&owner.to_ascii_lowercase()).and_then(|m| {
                    m.items.iter().position(|it| it.window.name.eq_ignore_ascii_case(a(1)) && it.ty == item_type::EDITFIELD)
                });
                if let Some(i) = field {
                    self.start_edit(&owner.to_ascii_lowercase(), i);
                }
            }
            "scriptmenuresponse" => self.responses.push((owner.to_ascii_lowercase(), a(1).to_owned())),
            "play" | "playlooped" => self.audio.queue.push(a(1).to_owned()),
            "showmenu" | "hidemenu" => {}
            _ => debug!("ui: unhandled command {args:?}"),
        }
    }

    /// Console commands run with `exec`.
    fn console(&mut self, args: &[String]) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        match a(0).to_ascii_lowercase().as_str() {
            "set" | "seta" | "sets" | "setu" => self.set_dvar(a(1), &args.get(2..).map(|r| r.join(" ")).unwrap_or_default()),
            "setfromdvar" => {
                let v = self.dvar(a(2));
                self.set_dvar(a(1), &v);
            }
            "toggle" => {
                let on = Val::Str(self.dvar(a(1))).truthy();
                self.set_dvar(a(1), if on { "0" } else { "1" });
            }
            "statset" => self.stats.set(num(a(1)), num(a(2))),
            "statgetindvar" => {
                let v = self.stats.get(num(a(1)));
                self.set_dvar(a(2), &v.to_string());
            }
            "uploadstats" => self.stats.save_if_changed(),
            "setfromlocstring" => {
                let text = self.assets.localize(a(2));
                self.set_dvar(a(1), &text);
            }
            // A random row of a string table, e.g. the "Did you know?" tips.
            "selectstringtableentryindvar" => {
                let col = a(2).parse::<usize>().unwrap_or(0);
                let pick = self.assets.table(a(1)).and_then(|t| {
                    let rows: Vec<&str> = (0..t.rows).filter_map(|r| t.get(r, col)).filter(|v| !v.is_empty()).collect();
                    (!rows.is_empty()).then(|| rows[rand::random_range(0..rows.len())].to_owned())
                });
                if let Some(v) = pick {
                    self.set_dvar(a(3), &v);
                }
            }
            "quit" => self.quit = true,
            // Leave Game's "yes".
            "disconnect" => self.leave = true,
            _ => debug!("ui: unhandled console command {args:?}"),
        }
    }

    fn ui_script(&mut self, args: &[String]) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        match a(0).to_ascii_lowercase().as_str() {
            "openmenuondvar" | "openmenuondvarnot" => {
                let matches = self.dvar(a(1)).eq_ignore_ascii_case(a(2));
                if matches == a(0).eq_ignore_ascii_case("openmenuondvar") {
                    self.open(a(3));
                }
            }
            "startserver" => self.start = Some(self.dvar("ui_mapname")),
            "quit" => self.quit = true,
            s if s.starts_with("supply") => self.supply_script(args),
            s if s.starts_with("t5") => self.bo1_script(args),
            s if s.starts_with("t4") => self.waw_script(args),
            s if s.starts_with("lobby") => self.lobby_script(args),
            _ if self.browser_script(args) => {}
            _ => debug!("ui: unhandled uiScript {args:?}"),
        }
    }

    fn open(&mut self, name: &str) {
        let key = name.to_ascii_lowercase();
        // Already open: to the front, and its onOpen runs again, as CoD4's
        // `Menus_Activate` does (Join Server's Back reopens `main`, whose
        // onOpen brings `main_text` back over it).
        if let Some(i) = self.stack.iter().position(|m| m.name == key) {
            let m = self.stack.remove(i);
            let on_open = m.menu.on_open.clone();
            self.stack.push(m);
            self.run(&on_open, &key);
            return;
        }
        // Weapon popups ask which game's weapons first (Black Ops' guns).
        if let Some(choice) = self.bo1_redirect(&key) {
            return self.open(&choice);
        }
        let base = self.supply_menu(&key, self.assets.menu(&key).or_else(|| self.bo1_menu(&key)).or_else(|| self.waw_menu(&key)));
        let Some(mut menu) = self.lobby_menu(&key, base) else {
            warn!("ui: no menu {name}");
            return;
        };
        // Attachment rows toggle (several per weapon), so the list needs a
        // way to move on.
        if key.contains("attachment_popup") {
            if let Some(m) = attachments::with_accept_row(&menu) {
                menu = Arc::new(m);
            }
        }
        let shown = menu.items.iter().map(|it| it.window.dynamic_flags & flags::VISIBLE != 0).collect();
        let rows = menu
            .items
            .iter()
            .map(|it| if it.ty == item_type::BUTTON { attachments::row_selects(&it.action) } else { None })
            .collect();
        self.stack.push(OpenMenu { name: key.clone(), menu: menu.clone(), shown, rows });
        // A menu's music plays on until another menu brings its own.
        if !menu.sound_name.is_empty() {
            self.audio.music = Some(menu.sound_name.clone());
        }
        self.run(&menu.on_open, &key);
    }

    fn close(&mut self, name: &str) {
        let key = name.to_ascii_lowercase();
        let Some(i) = self.stack.iter().position(|m| m.name == key) else { return };
        let m = self.stack.remove(i);
        if self.focus.as_ref().is_some_and(|(f, _)| *f == key) {
            self.focus = None;
        }
        if self.editing.as_ref().is_some_and(|(f, _)| *f == key) {
            self.editing = None;
        }
        self.run(&m.menu.on_close, &key);
    }

    fn show_item(&mut self, menu: &str, item: &str, on: bool) {
        let Some(om) = self.stack.iter_mut().find(|m| m.name.eq_ignore_ascii_case(menu)) else { return };
        for (i, it) in om.menu.items.iter().enumerate() {
            if it.window.name.eq_ignore_ascii_case(item) || it.window.group.eq_ignore_ascii_case(item) {
                om.shown[i] = on;
            }
        }
    }

    // --- items

    /// `dvarTest` with `showDvar`/`hideDvar` (mask 4|8) or
    /// `enableDvar`/`disableDvar` (mask 1|2).
    fn dvar_test(&self, item: &Item, mask: i32) -> bool {
        let f = item.dvar_flags & mask;
        if f == 0 || item.dvar_test.is_empty() {
            return true;
        }
        let value = self.dvar(&item.dvar_test);
        let hit = script::parse(&item.enable_dvar).into_iter().flatten().any(|v| v.eq_ignore_ascii_case(&value));
        if f & (1 | 4) != 0 { hit } else { !hit }
    }

    fn item_visible(&self, om: &OpenMenu, i: usize) -> bool {
        let item = &om.menu.items[i];
        om.shown[i]
            && (item.visible_exp.is_empty() || eval(&item.visible_exp, self).truthy())
            && self.dvar_test(item, 4 | 8)
            && self.browser_shows(item.window.owner_draw_flags)
    }

    fn item_rect(&self, item: &Item) -> VRect {
        let mut r = item.window.rect;
        let num = |e: &Statement, v: &mut f32| {
            if !e.is_empty() {
                *v = eval(e, self).num();
            }
        };
        num(&item.rect_x_exp, &mut r.x);
        num(&item.rect_y_exp, &mut r.y);
        num(&item.rect_w_exp, &mut r.w);
        num(&item.rect_h_exp, &mut r.h);
        r
    }

    /// The dvar-backed value shown by settings items.
    fn item_value(&self, item: &Item) -> Option<String> {
        let current = || self.dvar(&item.dvar);
        match item.ty {
            item_type::YESNO => {
                Some(self.assets.localize(if Val::Str(current()).truthy() { "@MENU_YES" } else { "@MENU_NO" }))
            }
            item_type::MULTI => {
                let ItemData::Multi(m) = &item.data else { return None };
                m.labels.get(multi_index(m, &current())).map(|l| self.assets.localize(l))
            }
            item_type::EDITFIELD | item_type::NUMERICFIELD | item_type::DVARENUM => Some(self.assets.localize(&current())),
            item_type::OWNERDRAW => self.browser_owner_draw(item.window.owner_draw),
            _ => None,
        }
    }

    fn menu_item(&self, menu: &str) -> Option<Arc<Menu>> {
        self.stack.iter().find(|m| m.name == menu).map(|m| m.menu.clone())
    }

    fn set_focus(&mut self, new: Option<(String, usize)>) {
        if let Some((m, i)) = self.focus.take() {
            if let Some(menu) = self.menu_item(&m) {
                let it = &menu.items[i];
                self.run(&format!("{} ; {}", it.leave_focus, it.mouse_exit), &m);
            }
        }
        self.focus = new.clone();
        if let Some((m, i)) = new {
            if let Some(menu) = self.menu_item(&m) {
                let it = &menu.items[i];
                self.run(&format!("{} ; {}", it.on_focus, it.mouse_enter), &m);
            }
        }
    }

    fn activate(&mut self, menu: &str, i: usize) {
        let Some(m) = self.menu_item(menu) else { return };
        let item = &m.items[i];
        // Attachment rows add or remove that attachment and stay open; "None"
        // and the grenade launcher keep CoD's own single pick (Black Ops' and
        // World at War's guns toggle every attachment, and "None" clears
        // them).
        let row = self.stack.iter().find(|om| om.name == menu).and_then(|om| om.rows[i].clone());
        if let Some((stat, name)) = row {
            let weapon = stat - 1;
            let bo1 = self.stat(weapon) >= crate::bo1::FIRST_INDEX;
            if bo1 && name == "none" {
                self.set_attachments(weapon, 0);
            } else if bo1 || (name != "none" && name != "gl") {
                self.run("\"play\" \"mouse_click\"", menu);
                let set = attachments::set_of(self, weapon);
                let set = match bo1 {
                    true if crate::waw::is_index(self.stat(weapon)) => waw::toggle(set, &name),
                    true => bo1::toggle(set, &name),
                    false => attachments::toggle(set, &name),
                };
                self.set_attachments(weapon, set);
                return;
            }
        }
        if item.ty == item_type::LISTBOX && item.special == browser::FEEDER_SERVERS {
            let item = item.clone();
            self.browser_click(&item, self.cursor);
            return;
        }
        if item.ty == item_type::OWNERDRAW {
            self.browser_owner_draw_click(item.window.owner_draw);
        }
        match (&item.data, item.ty) {
            (_, item_type::YESNO) => {
                let on = Val::Str(self.dvar(&item.dvar)).truthy();
                self.set_dvar(&item.dvar, if on { "0" } else { "1" });
            }
            (_, item_type::EDITFIELD | item_type::NUMERICFIELD) => self.start_edit(menu, i),
            (ItemData::Multi(multi), item_type::MULTI) if !multi.labels.is_empty() => {
                let next = (multi_index(multi, &self.dvar(&item.dvar)) + 1) % multi.labels.len();
                let v = if multi.str_def { multi.strings[next].clone() } else { format_number(multi.values[next]) };
                self.set_dvar(&item.dvar, &v);
            }
            _ => {}
        }
        self.run(&item.action, menu);
    }

    /// Store a weapon's attachment set (and its main attachment in CoD's own
    /// stat), then sort out Perk 1.
    fn set_attachments(&mut self, weapon: i32, set: i32) {
        // Black Ops' and World at War's guns keep the whole set; CoD's stat
        // holds none of it.
        if self.stat(weapon) >= crate::bo1::FIRST_INDEX {
            self.stats.set(weapon + 1, 0);
            self.stats.set(attachments::SET_STATS + weapon, set);
            return;
        }
        let main = attachments::main_attachment(set);
        let index = num(&self.table_lookup(attachments::TABLE, 4, main, 9));
        self.stats.set(weapon + 1, index);
        self.stats.set(attachments::SET_STATS + weapon, set);
        // Like CoD4, a grenade launcher or grip takes the Perk 1 slot (and
        // either on the second weapon too); otherwise free the slot.
        let class = weapon - weapon % 10;
        let (primary, second) = (attachments::set_of(self, class + 1), attachments::set_of(self, class + 3));
        let perk1 = self.stats.get(class + 5);
        let perk1 = if attachments::has(primary, "gl") {
            191
        } else if attachments::has(primary, "grip") {
            192
        } else if attachments::has(second, "gl") || attachments::has(second, "grip") {
            193
        } else if (191..=193).contains(&perk1) {
            190
        } else {
            perk1
        };
        self.stats.set(class + 5, perk1);
    }

    /// "Silencer + Red Dot Sight".
    fn attachment_list(&self, weapon: i32) -> String {
        let bo1 = crate::bo1::is_index(self.stat(weapon)).then(crate::bo1::data).flatten();
        let waw = crate::waw::is_index(self.stat(weapon)).then(crate::waw::data).flatten();
        let names: Vec<String> = attachments::names(attachments::set_of(self, weapon))
            .into_iter()
            .map(|n| match (bo1.and_then(|d| d.attachment(n)), waw.and_then(|d| d.attachment(n))) {
                // Black Ops' and World at War's own names (Black Ops'
                // `reflex` is CoD4's red dot's, WaW's `gl` a rifle grenade).
                (Some(a), _) => a.display.clone(),
                (_, Some(a)) => a.display.clone(),
                _ => self.assets.localize(&format!("@{}", self.table_lookup(attachments::TABLE, 4, n, 3))),
            })
            .collect();
        names.join(" + ")
    }

    fn start_edit(&mut self, menu: &str, i: usize) {
        let Some(m) = self.menu_item(menu) else { return };
        let dvar = &m.items[i].dvar;
        // Start from the text shown, not a localization key.
        let text = self.assets.localize(&self.dvar(dvar));
        self.set_dvar(dvar, &text);
        self.editing = Some((menu.to_owned(), i));
    }

    /// Typing into the text field being edited.
    fn edit_key(&mut self, key: &bevy::input::keyboard::KeyboardInput) {
        use bevy::input::keyboard::Key;
        let Some((menu, i)) = self.editing.clone() else { return };
        let Some(m) = self.menu_item(&menu) else {
            self.editing = None;
            return;
        };
        let item = &m.items[i];
        let mut text = self.dvar(&item.dvar);
        let max = match &item.data {
            ItemData::EditField(e) if e.max_chars > 0 => e.max_chars as usize,
            _ => 32,
        };
        match &key.logical_key {
            Key::Backspace => {
                text.pop();
            }
            Key::Enter => {
                self.editing = None;
                self.run(&item.on_accept, &menu);
                return;
            }
            Key::Escape => {
                self.editing = None;
                return;
            }
            _ => {
                for c in key.text.as_deref().unwrap_or("").chars().filter(|c| !c.is_control()) {
                    if text.chars().count() < max {
                        text.push(c);
                    }
                }
            }
        }
        self.set_dvar(&item.dvar, &text);
    }

    /// The topmost item under `p` in the top menu.
    fn hit(&self, pl: &Placement, p: Vec2) -> Option<(String, usize)> {
        let om = self.stack.last()?;
        (0..om.menu.items.len())
            .rev()
            .find(|&i| {
                let item = &om.menu.items[i];
                if !interactive(item) || !self.item_visible(om, i) {
                    return false;
                }
                let (pos, size) = pl.rect(&self.item_rect(item));
                p.x >= pos.x && p.y >= pos.y && p.x < pos.x + size.x && p.y < pos.y + size.y
            })
            .map(|i| (om.name.clone(), i))
    }

    // --- painting

    fn font_for(&self, font_enum: i32, px: f32) -> usize {
        let name = match font_enum {
            2 => "fonts/bigFont",
            3 => "fonts/smallFont",
            4 => "fonts/boldFont",
            5 => "fonts/consoleFont",
            6 => "fonts/objectiveFont",
            // Default and normal text use the size closest to how big it is drawn.
            _ if px <= 12.0 => "fonts/smallFont",
            _ if px <= 20.0 => "fonts/normalFont",
            _ if px <= 34.0 => "fonts/bigFont",
            _ => "fonts/extraBigFont",
        };
        self.assets.fonts.iter().position(|f| f.name.eq_ignore_ascii_case(name)).unwrap_or(0)
    }

    fn paint(&self, pl: &Placement, ops: &mut Vec<Op>) {
        // Fullscreen menus hide the ones beneath them.
        let first = self.stack.iter().rposition(|m| m.menu.full_screen).unwrap_or(0);
        // Like Black Ops, the class's character stands in Create a Class:
        // over the class menu's full-screen backdrop, under its panels and
        // popups.
        let class_menu = self.stack[first..].iter().position(|om| om.name.starts_with("menu_cac_"));
        for (k, om) in self.stack[first..].iter().enumerate() {
            let figure_after = (Some(k) == class_menu).then(|| {
                om.menu.items.iter().rposition(|it| {
                    let (_, size) = pl.rect(&self.item_rect(it));
                    size.x >= pl.w * 0.9 && size.y >= pl.h * 0.9
                })
            });
            if figure_after == Some(None) {
                self.paint_figure(om, pl, ops);
            }
            if !self.paint_menu(om, pl, figure_after.flatten().map(|b| b + 1), ops) {
                continue;
            }
            self.paint_supply(om, pl, ops);
            self.paint_lobby(om, pl, ops);
        }
    }

    /// One menu, if visible (returns whether it was), with the class figure
    /// before item `figure_at` (or after the last).
    fn paint_menu(&self, om: &OpenMenu, pl: &Placement, figure_at: Option<usize>, ops: &mut Vec<Op>) -> bool {
        let menu = &om.menu;
        if !menu.visible_exp.is_empty() && !eval(&menu.visible_exp, self).truthy() {
            return false;
        }
        let (pos, size) = pl.rect(&menu.window.rect);
        paint_window(&menu.window, pos, size, pl.scale, None, menu.window.fore_color, ops);
        for (i, item) in menu.items.iter().enumerate() {
            if figure_at == Some(i) {
                self.paint_figure(om, pl, ops);
            }
            if !self.item_visible(om, i) {
                continue;
            }
            let r = self.item_rect(item);
            let (pos, size) = pl.rect(&r);
            let mut fore = item.window.fore_color;
            if !item.forecolor_a_exp.is_empty() {
                fore[3] = eval(&item.forecolor_a_exp, self).num();
            }
            let material = (!item.material_exp.is_empty()).then(|| eval(&item.material_exp, self).text());
            if let Some((key, (weapon, camo))) = weapon_picture_stat(&item.material_exp).and_then(|k| Some((k, self.gun_for(k)?))) {
                ops.push(Op::Gun { pos, size, key, weapon, camo, picture: material.unwrap_or_default() });
                continue;
            }
            paint_window(&item.window, pos, size, pl.scale, material, fore, ops);
            let focused = self.focus.as_ref().is_some_and(|(m, f)| *m == om.name && *f == i);
            self.paint_text(menu, item, &r, pl, fore, focused, ops);
            if item.ty == item_type::LISTBOX && item.special == browser::FEEDER_SERVERS {
                self.paint_servers(item, pl, ops);
            }
            if let Some((stat, name)) = om.rows[i].as_ref().filter(|(_, n)| n != "none") {
                let on = attachments::has(attachments::set_of(self, stat - 1), name);
                let box_size = Vec2::splat(12.0 * pl.scale);
                ops.push(Op::Pic {
                    pos: Vec2::new(pos.x + 4.0 * pl.scale, pos.y + (size.y - box_size.y) * 0.5),
                    size: box_size,
                    material: if on { "hud_checkbox_checked" } else { "hud_checkbox_clear" }.into(),
                    color: [1.0, 1.0, 1.0, fore[3]],
                });
            }
        }
        // The backdrop was the last item.
        if figure_at == Some(menu.items.len()) {
            self.paint_figure(om, pl, ops);
        }
        true
    }

    /// The worn character holding the class's primary weapon, with its
    /// attachments and camo (and any the open popup is previewing).
    fn paint_figure(&self, class_menu: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
        // The class's primary: the weapon stat its picture shows.
        let primary = class_menu.menu.items.iter().filter_map(|it| weapon_picture_stat(&it.material_exp)).find(|k| k % 10 == 1);
        let Some((gun, camo)) = primary.and_then(|k| self.gun_for(k)) else { return };
        let (left, right) = (pl.w * 0.5 + FIGURE_LEFT * pl.scale, pl.w * 0.5 + FIGURE_RIGHT * pl.scale);
        let (top, bottom) = (FIGURE_TOP * pl.scale, FIGURE_BOTTOM * pl.scale);
        ops.push(Op::Gun {
            pos: Vec2::new(left, top),
            size: Vec2::new(right - left, bottom - top),
            key: CLASS_FIGURE,
            weapon: format!("char:{}|gun:{gun}|camo:{camo}", worn_character()),
            camo,
            picture: String::new(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_text(&self, menu: &Menu, item: &Item, r: &VRect, pl: &Placement, fore: [f32; 4], focused: bool, ops: &mut Vec<Op>) {
        let several = attachments::name_text_stat(&item.text_exp)
            .or_else(|| attachments::highlighted_name_text_stat(&item.text_exp, self))
            .filter(|w| {
                // Black Ops' and World at War's guns' sets aren't in CoD's
                // stat at all.
                let n = attachments::names(attachments::set_of(self, *w)).len();
                n > 1 || (n == 1 && self.stat(*w) >= crate::bo1::FIRST_INDEX)
            });
        let raw = match several {
            Some(w) => self.attachment_list(w),
            None if item.text_exp.is_empty() => item.text.clone(),
            None => eval(&item.text_exp, self).text(),
        };
        let mut text = self.assets.localize(&raw);
        if let Some(v) = self.item_value(item) {
            text = if text.trim().is_empty() { v } else { format!("{text} {v}") };
        }
        // A blinking cursor in the field being typed into.
        let editing = self.editing.as_ref().is_some_and(|(m, f)| self.menu_item(m).is_some_and(|mm| std::ptr::eq(&mm.items[*f], item)));
        if editing && (self.millis() / 300) % 2 == 0 {
            text.push('_');
        }
        if text.is_empty() || item.text_scale <= 0.0 {
            return;
        }
        let mut color = fore;
        if focused && interactive(item) {
            color = menu.focus_color;
        }
        if !self.dvar_test(item, 1 | 2) {
            color = menu.disable_color;
        }
        let (sx, sy) = (pl.sx(r.horz_align), pl.sy(r.vert_align));
        // Text is `textscale * 48` virtual units tall whatever the font.
        let height = item.text_scale * 48.0 * sy;
        let font = self.font_for(item.font_enum, height);
        let f = &self.assets.fonts[font];
        let k = height / f.pixel_height as f32;
        let (pos, size) = pl.rect(r);
        let (ax, ay) = (item.text_align_x * sx, item.text_align_y * sy);
        let lines = if item.window.static_flags & flags::AUTO_WRAPPED != 0 && size.x > 0.0 {
            draw::wrap(f, &text, k, size.x)
        } else {
            vec![text]
        };
        // ITEM_ALIGN_*: bits 0-1 horizontal, bits 2-3 vertical (0 = legacy).
        let (horz, vert) = (item.text_align_mode & 3, item.text_align_mode & 12);
        let mut y = match vert {
            0 => pos.y + ay,
            4 => pos.y + height + ay,
            8 => pos.y + (size.y + height) * 0.5 + ay,
            _ => pos.y + size.y + ay,
        };
        let shadow = match item.text_style {
            3 => pl.scale,
            5 | 6 => 2.0 * pl.scale,
            _ => 0.0,
        };
        for line in lines {
            let w = draw::text_width(f, &line, k);
            let x = pos.x + ax + [0.0, (size.x - w) * 0.5, size.x - w, 0.0][horz as usize];
            ops.push(Op::Text { text: line, x, y, font, k, color, shadow });
            y += height;
        }
    }

    /// The weapon def and camo a class weapon stat shows: the class's own
    /// choice, with the attachment or camo under the mouse in an open popup
    /// (hovered weapons show in the weapon popup's own preview).
    fn gun_for(&self, key: i32) -> Option<(String, usize)> {
        if key == HIGHLIGHTED_WEAPON {
            let w = self.dvar("ui_primary_highlighted");
            return (!w.is_empty() && w != "0").then(|| (format!("{w}:"), 0));
        }
        let primary = key % 10 == 1;
        let weapon = self.table_lookup("mp/statstable.csv", 0, &self.stat(key).to_string(), 4);
        let mut set = attachments::set_of(self, key);
        let mut camo = if primary { self.stat(key - key % 10 + 9) } else { 0 };
        let valid = |v: &String| !v.is_empty() && v != "0";
        if let Some(top) = self.stack.last().map(|m| m.name.as_str()) {
            // `...2` popups are for the second weapon (Overkill), `pistol` and
            // `secondary` for the sidearm; Black Ops' popups name their stat.
            let for_secondary = match bo1::key_stat(top).or_else(|| waw::key_stat(top)) {
                Some(stat) => stat % 10 == 3,
                None => top.ends_with('2') || top.ends_with("secondary") || top.ends_with("pistol"),
            };
            if for_secondary != primary {
                if top.contains("attachment_popup") {
                    // Show what clicking the hovered row would add.
                    let a = self.dvar("ui_attachment_highlighted");
                    if valid(&a) && !attachments::has(set, &a) {
                        set = if crate::bo1::is_bo1(&weapon) {
                            bo1::toggle(set, &a)
                        } else if crate::waw::is_waw(&weapon) {
                            waw::toggle(set, &a)
                        } else {
                            attachments::toggle(set, &a)
                        };
                    }
                } else if top.contains("popup_cac_camo") {
                    let c = self.table_lookup("mp/attachmenttable.csv", 4, &self.dvar("ui_camo_highlighted"), 11);
                    if let Ok(c) = c.parse() {
                        camo = c;
                    }
                }
            }
        }
        if weapon.is_empty() {
            return None;
        }
        let camo = self.variant_camo(key, &weapon, camo);
        Some((format!("{weapon}:{}", attachments::names(set).join("+")), camo.max(0) as usize))
    }

    /// Resolve materials and fonts and append the sprites to draw.
    fn emit(&mut self, ops: Vec<Op>, images: &mut Assets<Image>, out: &mut Vec<Quad>) {
        let white = self.assets.material("white", images);
        for op in ops {
            match op {
                Op::Fill { pos, size, color } => {
                    if let Some(w) = &white {
                        out.push(Quad::new(pos, size, w.handle.clone(), None, draw::color(color)));
                    }
                }
                Op::Pic { pos, size, material, color } => {
                    if let Some(img) = self.assets.material(&material, images) {
                        out.push(Quad::new(pos, size, img.handle, None, draw::color(color)));
                    }
                }
                Op::Image { pos, size, material, color, uv, rot, layer } => {
                    if let Some(img) = self.assets.material(&material, images) {
                        let uv = uv.map(|r| Rect::from_corners(r.min * img.size, r.max * img.size));
                        out.push(Quad { rot, layer, ..Quad::new(pos, size, img.handle, uv, draw::color(color)) });
                    }
                }
                Op::Text { text, x, y, font, k, color, shadow } => {
                    let name = self.assets.fonts[font].material.clone();
                    if let Some(img) = self.assets.material(&name, images) {
                        draw::draw_text(out, &self.assets.fonts[font], &img, &text, x, y, k, color, shadow);
                    }
                }
                Op::Gun { pos, size, key, weapon, camo, picture } => {
                    let image = self.previews.request(key, &weapon, camo, pos, size);
                    let image = image.or_else(|| self.assets.material(&picture, images).map(|i| i.handle));
                    if let Some(image) = image {
                        out.push(Quad::new(pos, size, image, None, Color::WHITE));
                    }
                }
            }
        }
    }
}

/// `Window_Paint`: background fill or material, then the border.
#[allow(clippy::too_many_arguments)]
fn paint_window(w: &MenuWindow, pos: Vec2, size: Vec2, scale: f32, material: Option<String>, fore: [f32; 4], ops: &mut Vec<Op>) {
    match w.style {
        // Filled and gradient.
        1 | 2 if w.back_color[3] > 0.0 => ops.push(Op::Fill { pos, size, color: w.back_color }),
        // Shader: the background material tinted by the fore colour.
        3 => {
            if let Some(m) = material.or_else(|| w.background.clone()).filter(|m| !m.is_empty()) {
                ops.push(Op::Pic { pos, size, material: m, color: fore });
            }
        }
        _ => {}
    }
    if w.border != 0 && w.border_size > 0.0 && w.border_color[3] > 0.0 {
        let (b, c) = (w.border_size * scale, w.border_color);
        let mut edge = |pos: Vec2, size: Vec2| ops.push(Op::Fill { pos, size, color: c });
        // 1 full, 2 horizontal, 3 vertical; the gradient/raised/sunken styles as full.
        if w.border != 3 {
            edge(pos, Vec2::new(size.x, b));
            edge(Vec2::new(pos.x, pos.y + size.y - b), Vec2::new(size.x, b));
        }
        if w.border != 2 {
            edge(pos, Vec2::new(b, size.y));
            edge(Vec2::new(pos.x + size.x - b, pos.y), Vec2::new(b, size.y));
        }
    }
}

fn enter_frontend(
    mut commands: Commands,
    fe: Option<ResMut<Frontend>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut next: ResMut<NextState<GameState>>,
) {
    commands.spawn((
        FrontendCamera,
        Camera2d,
        Camera { order: 10, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
    ));
    // The menus draw CoD's own cursor.
    cursor.visible = false;
    cursor.grab_mode = CursorGrabMode::None;
    match fe {
        Some(mut fe) => {
            fe.stack.clear();
            fe.focus = None;
            // Back from a match: the previews' content went when it started.
            let vfs = fe.assets.vfs();
            fe.previews.start(vfs);
            fe.open("main");
        }
        None => match UiAssets::load() {
            Ok(assets) => commands.insert_resource(Frontend::new(assets)),
            Err(e) => {
                error!("ui: can't load the menus ({e:#}); starting a match instead");
                next.set(GameState::Loading);
            }
        },
    }
}

fn leave_frontend(
    mut commands: Commands,
    fe: Option<ResMut<Frontend>>,
    cameras: Query<Entity, With<FrontendCamera>>,
    mut pool: ResMut<SpritePool>,
    mut list: ResMut<DrawList>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
) {
    for e in &cameras {
        commands.entity(e).despawn();
    }
    if let Some(mut fe) = fe {
        fe.previews.clear(&mut commands);
        fe.stats.save_if_changed();
    }
    for e in pool.0.drain(..) {
        commands.entity(e).despawn();
    }
    list.0.clear();
    cursor.visible = true;
}

#[allow(clippy::too_many_arguments)]
/// The devices players can pick, for the lobby.
fn sync_devices(devices: Res<crate::gamepad::Devices>, mut fe: ResMut<Frontend>) {
    if fe.devices != devices.0 {
        fe.devices_changed(devices.0.clone());
    }
}

fn menu_input(
    mut commands: Commands,
    mut fe: ResMut<Frontend>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut typed: MessageReader<bevy::input::keyboard::KeyboardInput>,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut next: ResMut<NextState<GameState>>,
    mut exit: MessageWriter<AppExit>,
) {
    fe.esc_used = false;
    let pl = Placement::new(window.width(), window.height());
    let cursor = window.cursor_position();
    fe.cursor = cursor;
    // The mouse wheel scrolls the server list it's over.
    if scroll.delta.y != 0.0 {
        let list = fe.focus.clone().and_then(|(m, i)| fe.menu_item(&m).map(|menu| menu.items[i].clone()));
        if let Some(item) = list.filter(|it| it.ty == item_type::LISTBOX && it.special == browser::FEEDER_SERVERS) {
            fe.browser_scroll(&item, scroll.delta.y.signum() as i32 * 3);
        }
    }
    // Popups put a do-nothing button behind their contents; dragging over it
    // still turns a preview.
    let over_item = fe.focus.as_ref().is_some_and(|(m, i)| {
        fe.menu_item(m).is_some_and(|menu| script::tokenize(&menu.items[*i].action).iter().any(|t| matches!(t, Tok::Word(_))))
    });
    let dragging = fe.previews.drag(cursor, mouse.pressed(MouseButton::Left), mouse.just_pressed(MouseButton::Left), over_item);
    if let Some(p) = cursor.filter(|_| !dragging) {
        let hover = fe.hit(&pl, p);
        if hover != fe.focus {
            fe.set_focus(hover);
        }
    }
    if (mouse.just_pressed(MouseButton::Left) && !dragging) || keys.just_pressed(KeyCode::Enter) {
        if let Some((menu, i)) = fe.focus.clone() {
            fe.activate(&menu, i);
        }
    }
    // Typing goes to the text field being edited; Enter and Escape end it.
    let editing = fe.editing.is_some();
    for key in typed.read() {
        if key.state.is_pressed() && fe.editing.is_some() {
            fe.edit_key(key);
        }
    }
    if editing {
        if mouse.just_pressed(MouseButton::Left) && fe.editing.is_some() && fe.focus != fe.editing {
            fe.editing = None;
        }
    } else if keys.just_pressed(KeyCode::Escape) {
        if let Some(top) = fe.stack.last() {
            let (name, esc) = (top.name.clone(), top.menu.on_esc.clone());
            fe.esc_used = true;
            fe.run(&esc, &name);
        }
    }
    // Started from the Private Match lobby: its teams, limits and bots, and
    // its splitscreen players.
    if let Some((config, players)) = fe.lobby.take_start() {
        commands.insert_resource(config);
        commands.insert_resource(players);
    }
    if let Some(map) = fe.start.take() {
        info!("ui: starting {map}");
        commands.insert_resource(MapName(map));
        next.set(GameState::Loading);
    }
    fe.stats.save_if_changed();
    if fe.quit {
        exit.write(AppExit::Success);
    }
}

#[allow(clippy::too_many_arguments)]
fn update_previews(
    mut commands: Commands,
    mut fe: ResMut<Frontend>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut camo_materials: ResMut<Assets<crate::gunmodel::CamoMaterial>>,
    mut cameras: Query<&mut Camera>,
    mut transforms: Query<&mut Transform>,
) {
    preview::update(
        &mut commands,
        &mut fe.previews,
        &mut meshes,
        &mut materials,
        &mut images,
        &mut bindposes,
        &mut camo_materials,
        &mut cameras,
        &mut transforms,
    );
}

fn paint_menus(
    mut fe: ResMut<Frontend>,
    mut list: ResMut<DrawList>,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    list.0.clear();
    draw_menus(&mut fe, &mut list.0, &mut images, &window);
}

/// The open menus and the cursor, as quads added to `out`.
fn draw_menus(fe: &mut Frontend, out: &mut Vec<Quad>, images: &mut Assets<Image>, window: &Window) {
    let pl = Placement::new(window.width(), window.height());
    let mut ops = Vec::new();
    fe.paint(&pl, &mut ops);
    if let Some(p) = window.cursor_position().filter(|_| !fe.pad) {
        // The arrow's tip is at the centre of CoD's cursor image.
        let size = Vec2::splat(32.0 * pl.scale);
        ops.push(Op::Pic { pos: p - size * 0.5, size, material: "ui_cursor".into(), color: [1.0; 4] });
    }
    fe.emit(ops, images, out);
}

/// The map's load screen; the match starts (and the map loads) once it has
/// been shown for a few frames.
fn loading_screen(
    fe: Option<ResMut<Frontend>>,
    map: Res<MapName>,
    mut list: ResMut<DrawList>,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut frames: Local<u32>,
    mut next: ResMut<NextState<GameState>>,
) {
    list.0.clear();
    if let Some(mut fe) = fe {
        let size = Vec2::new(window.width(), window.height());
        let ops = vec![
            Op::Fill { pos: Vec2::ZERO, size, color: [0.0, 0.0, 0.0, 1.0] },
            Op::Pic { pos: Vec2::ZERO, size, material: format!("loadscreen_{}", map.0), color: [1.0; 4] },
        ];
        fe.emit(ops, &mut images, &mut list.0);
    }
    *frames += 1;
    if *frames >= 3 {
        *frames = 0;
        next.set(GameState::InGame);
    }
}

/// Debug aid: with `COD4RW_UISHOT=<dir>`, screenshot the main menu, the
/// main menu with "Start New Server" focused, each menu named in
/// `COD4RW_UIMENUS` (comma separated; `menu@dvar=value;dvar=value` also sets
/// dvars, `menu#Label` clicks the item labelled so, the menu can be left
/// empty, and `~pixels` drags the first gun preview), then start a match and
/// screenshot it. In the match, each step of `COD4RW_UIGAME` (`#Label`
/// clicks, `key:Digit2` presses a key, anything else just waits) is followed
/// by a screenshot; `hold:Tab` keeps the key down through its screenshot
/// (`hold:MouseRight` aims down the sights).
#[derive(Resource)]
struct UiShotDir(std::path::PathBuf);

/// `COD4RW_UIGAME`'s `p2:key:Enter` / `p2:dir:down` steps: a splitscreen
/// player's presses, as their controller's would reach their menus.
#[derive(Resource, Default)]
struct SlotPresses(Vec<(usize, Option<KeyCode>, Option<IVec2>)>);

fn slot_presses(mut presses: ResMut<SlotPresses>, mut players: Query<(&crate::splitscreen::LocalSlot, &mut crate::splitscreen::PlayerInput)>) {
    for (slot, key, dir) in presses.0.drain(..) {
        for (s, mut input) in &mut players {
            if s.0 == slot {
                if let Some(k) = key {
                    input.keys.press(k);
                }
                if dir.is_some() {
                    input.pad.menu_dir = dir;
                }
            }
        }
    }
}

fn ui_shots(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<UiShotDir>,
    fe: Option<ResMut<Frontend>>,
    state: Res<State<GameState>>,
    mut step: Local<usize>,
    mut at: Local<f32>,
    mut exit: MessageWriter<AppExit>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut presses: ResMut<SlotPresses>,
) {
    let t = time.elapsed_secs();
    if t < at.max(3.0) {
        return;
    }
    let extra: Vec<String> =
        std::env::var("COD4RW_UIMENUS").unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect();
    let shot = |commands: &mut Commands, name: &str| {
        std::fs::create_dir_all(&dir.0).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.0.join(format!("{name}.png"))));
    };
    let Some(mut fe) = fe else { return };
    let n = *step;
    *step += 1;
    *at = t + 1.0;
    match n {
        0 => shot(&mut commands, "0_main"),
        1 => {
            // Focus the button that opens Start New Server.
            let top = fe.stack.last().map(|m| (m.name.clone(), m.menu.clone()));
            if let Some((name, menu)) = top {
                if let Some(i) = menu.items.iter().position(|it| it.action.contains("createserver")) {
                    fe.set_focus(Some((name, i)));
                }
            }
        }
        2 => shot(&mut commands, "1_main_focus"),
        _ if n - 3 < extra.len() * 2 => {
            let (k, take) = ((n - 3) / 2, (n - 3) % 2 == 1);
            // `menu@dvar=value,...` sets dvars after opening (to fake a hover).
            let mut parts = extra[k].split('@');
            let (menu, click) = parts.next().unwrap_or_default().split_once('#').unwrap_or((&extra[k], ""));
            let menu = menu.split('@').next().unwrap_or_default();
            if take {
                // Give the weapon previews time to load.
                if fe.previews.busy() {
                    *step -= 1;
                    return;
                }
                shot(&mut commands, &format!("{}_{}", k + 2, extra[k].replace(['@', '=', ':', '#', ' ', '<', '>'], "_")));
            } else {
                fe.set_focus(None);
                if !menu.is_empty() && !menu.starts_with('~') {
                    fe.open(menu);
                }
                for kv in parts.next().unwrap_or_default().split(';').filter(|s| !s.is_empty()) {
                    let (key, value) = kv.split_once('=').unwrap_or((kv, ""));
                    fe.set_dvar(key, value);
                }
                // `~pixels` drags the first preview sideways.
                if let Some(dx) = menu.strip_prefix('~').and_then(|d| d.parse::<f32>().ok()) {
                    if let Some((pos, size)) = fe.previews.first_rect() {
                        let c = pos + size * 0.5;
                        fe.previews.drag(Some(c), true, true, false);
                        fe.previews.drag(Some(c + Vec2::new(dx, dx * 0.3)), true, false, false);
                        fe.previews.drag(Some(c), false, false, false);
                    }
                }
                if !click.is_empty() {
                    click_label(&mut fe, click);
                }
            }
        }
        _ if *state.get() == GameState::Frontend && n == 3 + extra.len() * 2 => fe.start = Some(fe.dvar("ui_mapname")),
        _ if *state.get() == GameState::InGame => {
            let game: Vec<String> =
                std::env::var("COD4RW_UIGAME").unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect();
            let first = 3 + extra.len() * 2 + 5;
            if n < first {
                return;
            }
            let k = n - first;
            // `key:` presses until the screenshot, `hold:` through it.
            let key = |s: &str| match s.strip_prefix("key:").or_else(|| s.strip_prefix("hold:"))? {
                "Digit1" => Some(KeyCode::Digit1),
                "Digit2" => Some(KeyCode::Digit2),
                "Digit4" => Some(KeyCode::Digit4),
                "Digit5" => Some(KeyCode::Digit5),
                "KeyG" => Some(KeyCode::KeyG),
                "Escape" => Some(KeyCode::Escape),
                "Enter" => Some(KeyCode::Enter),
                "KeyR" => Some(KeyCode::KeyR),
                "Tab" => Some(KeyCode::Tab),
                _ => None,
            };
            if k % 2 == 0 && k > 0 {
                if let Some(code) = game.get(k / 2 - 1).filter(|s| s.starts_with("hold:")).and_then(|s| key(s)) {
                    keys.release(code);
                }
                mouse.release(MouseButton::Right);
            }
            if k < game.len() * 2 {
                let s = &game[k / 2];
                // `p2#Label`: splitscreen Player 2's menus.
                let slot_click = s.strip_prefix('p').and_then(|r| r.split_once('#')).and_then(|(n, l)| Some((n.parse::<usize>().ok()?.checked_sub(1)?, l)));
                if k % 2 == 1 {
                    if let Some(code) = key(s).filter(|_| !s.starts_with("hold:")) {
                        keys.release(code);
                    }
                    shot(&mut commands, &format!("g{}_{}", k / 2, s.replace(['#', ':', ' '], "_")));
                } else if let Some((slot, label)) = slot_click {
                    fe.with_slot(slot, |fe| click_label(fe, label));
                } else if let Some((slot, what)) = s.strip_prefix('p').and_then(|r| r.split_once(':')).and_then(|(n, w)| Some((n.parse::<usize>().ok()?.checked_sub(1)?, w))) {
                    let dir = match what {
                        "dir:up" => Some(IVec2::NEG_Y),
                        "dir:down" => Some(IVec2::Y),
                        "dir:left" => Some(IVec2::NEG_X),
                        "dir:right" => Some(IVec2::X),
                        _ => None,
                    };
                    presses.0.push((slot, key(what), dir));
                } else if let Some(label) = s.strip_prefix('#') {
                    click_label(&mut fe, label);
                } else if s == "hold:MouseRight" {
                    mouse.press(MouseButton::Right);
                } else if let Some(code) = key(s) {
                    keys.press(code);
                }
                return;
            }
            shot(&mut commands, "9_ingame");
            *at = t + 1.5;
            if k > game.len() * 2 {
                exit.write(AppExit::Success);
            }
        }
        _ => {}
    }
}

/// Debug aid: click the top menu's item showing `label`.
fn click_label(fe: &mut Frontend, label: &str) {
    let target = fe.stack.last().and_then(|om| {
        (0..om.menu.items.len()).rev().find_map(|i| {
            let it = &om.menu.items[i];
            let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, &*fe).text() };
            let shown = interactive(it) && fe.item_visible(om, i);
            (shown && fe.assets.localize(&raw).eq_ignore_ascii_case(label)).then(|| (om.name.clone(), i))
        })
    });
    match target {
        Some((name, i)) => {
            fe.set_focus(Some((name.clone(), i)));
            fe.activate(&name, i);
        }
        None => warn!("ui shot: nothing labelled {label}"),
    }
}
