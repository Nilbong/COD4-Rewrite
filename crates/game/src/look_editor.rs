//! The Map Look Editor: F9 (or Back + D-pad down on a pad) mid-match opens
//! a side panel of sliders for how the map looks: time and sun, sky and
//! clouds, fog and air, the grade, weather and shadows. Changes apply live
//! (the game keeps running) and can be saved as the map's look.
//!
//! Every slider is a live knob (`crate::tune`) set in the map's look layer
//! ([`crate::tune::look_set`]), over `tuning.txt`. A map's saved look is a
//! `name = value` file in `crates/game/assets/maplooks/`: `<map>.txt` for its
//! normal look, `<map>.showcase.txt` for its showcase; they're embedded at
//! build time (`build.rs`) so they ship, and a copy in
//! `%LOCALAPPDATA%/cod4rw/maplooks/` (where the editor saves outside the
//! project folder) wins over them. Loaded as each map loads; a map without
//! one is as before. Saving never touches `tuning.txt`.
//!
//! Mouse: drag a slider, click its value to reset it. Pad: D-pad up/down a
//! row, left/right the value, LB/RB the tab, X resets, Y saves, B closes,
//! hold LT for the A/B. Keys: arrows, PageUp/PageDown, Delete, Ctrl+S,
//! hold H for the A/B, F9 closes.

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use std::collections::BTreeMap;

include!(concat!(env!("OUT_DIR"), "/maplooks.rs"));

pub struct LookEditorPlugin;

impl Plugin for LookEditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Editor>()
            // (A match started from the command line skips the load screen.)
            .add_systems(OnEnter(crate::state::GameState::Loading), load_saved)
            .add_systems(OnEnter(crate::state::GameState::InGame), load_saved.in_set(crate::state::Setup::Content))
            .add_systems(OnExit(crate::state::GameState::InGame), |mut e: ResMut<Editor>| {
                e.open = false;
                crate::atmos::climate::set_original(false);
            })
            .add_systems(Update, (toggle, navigate, mouse, buttons, refresh, water_look).chain().run_if(crate::state::in_game))
            .add_systems(PostUpdate, apply_sun.before(bevy::transform::TransformSystems::Propagate).run_if(crate::state::in_game));
    }
}

/// Run condition: the editor's panel is closed (the game has the mouse).
pub fn closed(e: Option<Res<Editor>>) -> bool {
    e.is_none_or(|e| !e.open)
}

#[derive(Resource, Default)]
pub struct Editor {
    open: bool,
    tab: usize,
    row: usize,
    /// The panel needs building anew (opened, or another tab).
    dirty: bool,
    /// Saving as the map's showcase look (else its normal look).
    as_showcase: bool,
    message: Option<(String, f32)>,
    /// Pad repeat: the direction held and when it next repeats.
    repeat: (i32, f32),
}

#[derive(Clone, Copy, PartialEq)]
enum Auto {
    No,
    /// The clock's hour now.
    Hour,
    /// The sun's bearing and elevation now.
    SunAz,
    SunEl,
    /// The weather's wetness now.
    Wetness,
    /// Neutral (1).
    One,
}

#[derive(Clone, Copy, PartialEq)]
enum Unit {
    Num,
    Hour,
    Deg,
    Toggle,
}

/// A slider: its knob (`{m}` the map), range, step and the value it has
/// when unset.
struct Def {
    tab: usize,
    label: &'static str,
    key: &'static str,
    min: f32,
    max: f32,
    step: f32,
    fallback: f32,
    auto: Auto,
    unit: Unit,
}

const TABS: [&str; 7] = ["Time & sun", "Sky", "Fog & air", "Grade", "Weather", "Shadows", "Water"];
/// The Water tab (greyed out on a map without water).
const WATER_TAB: usize = 6;

const fn d(tab: usize, label: &'static str, key: &'static str, min: f32, max: f32, step: f32, fallback: f32, auto: Auto, unit: Unit) -> Def {
    Def { tab, label, key, min, max, step, fallback, auto, unit }
}

const DEFS: &[Def] = &[
    d(0, "Time of day", "tod.hour", 0.0, 24.0, 0.25, -1.0, Auto::Hour, Unit::Hour),
    d(0, "Clock speed (x)", "tod.speed", 0.0, 600.0, 10.0, 0.0, Auto::No, Unit::Num),
    d(0, "Sun bearing", "sun.azimuth.{m}", 0.0, 360.0, 2.0, -1.0, Auto::SunAz, Unit::Deg),
    d(0, "Sun elevation", "sun.elevation.{m}", 1.0, 90.0, 1.0, -1.0, Auto::SunEl, Unit::Deg),
    d(0, "Sun brightness", "sun.brightness.{m}", 0.0, 3.0, 0.05, 1.0, Auto::No, Unit::Num),
    d(0, "Sunlight colour (cool-warm)", "sun.warmth.{m}", -1.0, 1.0, 0.05, 0.0, Auto::No, Unit::Num),
    d(1, "Cloud cover", "sky.cover.{m}", 0.0, 1.0, 0.02, 0.5, Auto::No, Unit::Num),
    d(1, "Cloud darkness", "sky.density.{m}", 0.0, 1.0, 0.02, 0.3, Auto::No, Unit::Num),
    d(1, "Cloud altitude (m)", "sky.altitude.{m}", -1000.0, 4000.0, 50.0, 0.0, Auto::No, Unit::Num),
    d(1, "Cloud drift direction", "sky.wind_dir.{m}", 0.0, 360.0, 5.0, 19.3, Auto::No, Unit::Deg),
    d(1, "Cloud drift speed (x)", "sky.wind_speed.{m}", 0.0, 10.0, 0.1, 1.0, Auto::No, Unit::Num),
    d(1, "Cloud position (m)", "sky.offset.{m}", 0.0, 20000.0, 100.0, 0.0, Auto::No, Unit::Num),
    d(1, "Sky & fog colour (cool-warm)", "sky.tint.{m}", -1.0, 1.0, 0.05, 0.0, Auto::No, Unit::Num),
    d(1, "Sky brightness", "sky.brightness.{m}", 0.0, 3.0, 0.05, 1.0, Auto::No, Unit::Num),
    d(1, "Volumetric clouds (bevy_atmos)", "sky.screen_clouds.{m}", 0.0, 1.0, 1.0, 0.0, Auto::No, Unit::Toggle),
    d(1, "Physical sky", "sky.physical.{m}", 0.0, 1.0, 1.0, 0.0, Auto::No, Unit::Toggle),
    d(2, "Fog distance (x)", "fog.{m}.reach", 0.2, 20.0, 0.1, 1.0, Auto::No, Unit::Num),
    d(2, "Fog start (x)", "fog.{m}.start", 0.0, 5.0, 0.05, 1.0, Auto::No, Unit::Num),
    d(2, "Fog brightness", "fog.{m}.tone", 0.0, 2.0, 0.05, 1.0, Auto::No, Unit::Num),
    d(2, "Sun shafts", "volumetric.{m}", 0.0, 3.0, 0.05, 1.0, Auto::No, Unit::Num),
    d(2, "Shaft quality (steps)", "volumetric.steps", 8.0, 48.0, 2.0, 20.0, Auto::No, Unit::Num),
    d(2, "CoD4 fog effects (next load)", "mapfx.fog.{m}", 0.0, 1.0, 1.0, 0.0, Auto::No, Unit::Toggle),
    d(3, "Exposure (stops)", "exposure.{m}", -3.0, 3.0, 0.05, 0.0, Auto::No, Unit::Num),
    d(3, "Overall warmth (whole image)", "warmth.{m}", -0.3, 0.3, 0.01, 0.0, Auto::No, Unit::Num),
    d(3, "Overall tint (green-magenta)", "tint.{m}", -0.3, 0.3, 0.01, 0.0, Auto::No, Unit::Num),
    d(3, "Contrast", "contrast.{m}", 0.5, 2.0, 0.02, -1.0, Auto::One, Unit::Num),
    d(3, "Saturation", "saturation.{m}", 0.0, 2.0, 0.02, -1.0, Auto::One, Unit::Num),
    d(4, "Rain", "weather.rain", 0.0, 1.0, 0.02, 0.0, Auto::No, Unit::Num),
    d(4, "Rain, far layer", "rain.far", 0.0, 1.0, 0.05, 0.7, Auto::No, Unit::Num),
    d(4, "Rain, near streaks", "rain.near", 0.0, 1.0, 0.05, 1.0, Auto::No, Unit::Num),
    d(4, "Wetness", "weather.wetness", 0.0, 1.0, 0.02, -1.0, Auto::Wetness, Unit::Num),
    d(4, "Puddles without rain", "weather.puddles", 0.0, 1.0, 1.0, 0.0, Auto::No, Unit::Toggle),
    d(4, "Lightning", "weather.lightning", 0.0, 1.0, 1.0, 1.0, Auto::No, Unit::Toggle),
    d(4, "Snow", "weather.snow", 0.0, 1.0, 0.02, 0.0, Auto::No, Unit::Num),
    d(5, "Soft sun shadows", "shadows.soft", 0.0, 5.0, 0.1, 0.6, Auto::No, Unit::Num),
    d(5, "Contact shadows (m)", "shadows.contact", 0.0, 1.0, 0.05, 0.35, Auto::No, Unit::Num),
    d(5, "Lamp shadows (count)", "shadows.lamps", 0.0, 8.0, 1.0, 4.0, Auto::No, Unit::Num),
    d(5, "Lamp shadow strength", "shadows.lamp_strength", 0.0, 1.0, 0.05, 0.7, Auto::No, Unit::Num),
    d(6, "Modern water (bevy_atmos)", "water.modern.{m}", 0.0, 1.0, 1.0, 1.0, Auto::No, Unit::Toggle),
    d(6, "Water colour (blue-brown)", "water.tint.{m}", -1.0, 1.0, 0.05, 0.0, Auto::No, Unit::Num),
    d(6, "Water brightness", "water.brightness.{m}", 0.0, 2.0, 0.05, 1.0, Auto::No, Unit::Num),
    d(6, "Water clarity", "water.clarity.{m}", 0.0, 2.0, 0.05, 1.0, Auto::No, Unit::Num),
    d(6, "Wave strength", "water.waves.{m}", 0.0, 1.5, 0.05, 1.0, Auto::No, Unit::Num),
    d(6, "Reflections", "water.reflection.{m}", 0.0, 3.0, 0.05, 1.0, Auto::No, Unit::Num),
];

fn key(def: &Def) -> String {
    def.key.replace("{m}", &crate::atmos::climate::map_name())
}

fn rows(tab: usize) -> Vec<usize> {
    (0..DEFS.len()).filter(|&i| DEFS[i].tab == tab).collect()
}

/// What the world shows now, for sliders left unset.
struct Now {
    hour: f32,
    window: (f32, f32),
    sun: (f32, f32),
    wetness: f32,
}

fn range(def: &Def, now: &Now) -> (f32, f32) {
    if def.auto == Auto::Hour { now.window } else { (def.min, def.max) }
}

/// A slider's value now: set, or what the world shows.
fn value(def: &Def, now: &Now) -> (f32, bool) {
    let k = key(def);
    if let Some(v) = crate::tune::look_all().get(&k) {
        return (*v, true);
    }
    let v = match def.auto {
        Auto::Hour => now.hour,
        Auto::SunAz => now.sun.0,
        Auto::SunEl => now.sun.1,
        Auto::Wetness => now.wetness,
        Auto::One => 1.0,
        Auto::No => crate::tune::base(&k, def.fallback),
    };
    (v, false)
}

fn set(def: &Def, now: &Now, v: f32) {
    // (Nothing to set on a map without water.)
    if def.tab == WATER_TAB && !has_water() {
        return;
    }
    let (lo, hi) = range(def, now);
    let v = if def.unit == Unit::Toggle { if v >= 0.5 { 1.0 } else { 0.0 } } else { v.clamp(lo, hi) };
    crate::tune::look_set(&key(def), Some(v));
    // (The sun's bearing and elevation go together: setting one sets both.)
    for (a, b, other) in [(Auto::SunAz, Auto::SunEl, now.sun.1), (Auto::SunEl, Auto::SunAz, now.sun.0)] {
        if def.auto == a {
            if let Some(o) = DEFS.iter().find(|d| d.auto == b) {
                if !crate::tune::look_all().contains_key(&key(o)) {
                    crate::tune::look_set(&key(o), Some(other));
                }
            }
        }
    }
}

fn format(def: &Def, v: f32, set: bool) -> String {
    let s = match def.unit {
        Unit::Hour => format!("{:02}:{:02}", v.floor() as u32 % 24, ((v.fract()) * 60.0).round() as u32 % 60),
        Unit::Deg => format!("{v:.0} deg"),
        Unit::Toggle => if v >= 0.5 { "On".into() } else { "Off".into() },
        Unit::Num => {
            if def.step >= 1.0 {
                format!("{v:.0}")
            } else {
                format!("{v:.2}")
            }
        }
    };
    if set {
        s
    } else if def.auto == Auto::No || def.auto == Auto::One {
        format!("{s} (default)")
    } else {
        format!("{s} (now)")
    }
}

fn now_of(tod: &crate::atmos::climate::TimeOfDay, weather: &crate::atmos::climate::Weather, sun_dir: Option<Vec3>) -> Now {
    let sun = crate::atmos::climate::sun_override().or(if tod.enabled { Some(tod.sun_dir) } else { sun_dir }).map_or((0.0, 45.0), crate::atmos::climate::sun_angles);
    Now { hour: tod.hours, window: crate::atmos::climate::hour_window(tod), sun, wetness: weather.wetness }
}

// --- saving and loading

/// Where the editor saves: the project's `crates/game/assets/maplooks/`
/// when the game runs from the project folder, else the user's folder.
fn save_dir() -> std::path::PathBuf {
    let project = std::path::PathBuf::from("crates/game/assets/maplooks");
    if project.is_dir() {
        return project;
    }
    user_dir()
}

fn user_dir() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA").map_or_else(|| std::path::PathBuf::from("maplooks"), |d| std::path::PathBuf::from(d).join("cod4rw").join("maplooks"))
}

fn file_name(map: &str, showcase: bool) -> String {
    if showcase { format!("{map}.showcase.txt") } else { format!("{map}.txt") }
}

fn parse(text: &str) -> BTreeMap<String, f32> {
    text.lines()
        .filter_map(|l| {
            let l = l.split('#').next()?.trim();
            let (k, v) = l.split_once('=')?;
            let v: f32 = v.trim().parse().ok()?;
            v.is_finite().then(|| (k.trim().to_owned(), v))
        })
        .collect()
}

/// As a map loads: its saved look (user folder, then project folder, then
/// the one built in), or none.
fn load_saved() {
    // (Once a match: on the load screen, or as it starts if it had none.)
    let id = crate::atmos::climate::match_id();
    if LOADED_FOR.swap(id, std::sync::atomic::Ordering::Relaxed) == id {
        return;
    }
    let map = crate::atmos::climate::map_name();
    let name = file_name(&map, crate::atmos::climate::showcase());
    let text = [user_dir().join(&name), std::path::PathBuf::from("crates/game/assets/maplooks").join(&name)]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .or_else(|| MAPLOOKS.iter().find(|(n, _)| *n == name).map(|(_, t)| (*t).to_owned()));
    let values = text.as_deref().map(parse).unwrap_or_default();
    if !values.is_empty() {
        info!("map look: {} values from {name}", values.len());
    }
    crate::tune::look_replace(values);
}

static LOADED_FOR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);

fn save(as_showcase: bool) -> String {
    let map = crate::atmos::climate::map_name();
    let dir = save_dir();
    let path = dir.join(file_name(&map, as_showcase));
    let mut text = format!("# {map}'s {} look, from the Map Look Editor (F9).\n", if as_showcase { "showcase" } else { "normal" });
    for (k, v) in crate::tune::look_all() {
        text.push_str(&format!("{k} = {v}\n"));
    }
    match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text)) {
        Ok(()) => format!("Saved {}", path.display()),
        Err(e) => format!("Couldn't save: {e}"),
    }
}

// --- the panel

#[derive(Component)]
struct Panel;

#[derive(Component, Clone, Copy)]
enum Btn {
    Tab(usize),
    Reset(usize),
    Save,
    ResetMap,
    Ab,
    AsShowcase,
    Close,
    Fly,
}

/// A slider's track (its index in [`DEFS`]).
#[derive(Component)]
struct Track(usize);

#[derive(Component)]
struct Fill(usize);

#[derive(Component)]
struct ValueText(usize);

#[derive(Component)]
struct RowBg(usize);

#[derive(Component)]
struct Note;

#[derive(Component)]
struct SaveAs;

fn rgba(c: [f32; 4]) -> Color {
    Color::srgba(c[0], c[1], c[2], c[3])
}

fn text(s: impl Into<String>, size: f32, c: [f32; 4]) -> impl Bundle {
    (Text::new(s), TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }, TextColor(rgba(c)))
}

fn button(commands: &mut ChildSpawnerCommands, label: &str, b: Btn, accent: bool) {
    use crate::ui::next::theme as t;
    commands
        .spawn((
            Button,
            b,
            Node { padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)), margin: UiRect::all(Val::Px(3.0)), ..default() },
            BackgroundColor(rgba(if accent { t::ACCENT_DIM } else { t::RAISED })),
        ))
        .with_children(|c| {
            c.spawn(text(label, 14.0, t::INK));
        });
}

fn build(commands: &mut Commands, e: &Editor) {
    use crate::ui::next::theme as t;
    let map = crate::atmos::climate::map_name();
    commands
        .spawn((
            Panel,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(440.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(12.0)),
                row_gap: Val::Px(4.0),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(rgba(t::PANEL)),
            GlobalZIndex(900),
        ))
        .with_children(|p| {
            p.spawn(text(format!("MAP LOOK  -  {map}"), 20.0, t::INK));
            p.spawn(text(if crate::atmos::climate::showcase() { "Showcase on" } else { "Normal look (showcase off)" }, 14.0, t::INK_DIM));
            // Tabs.
            p.spawn(Node { flex_wrap: FlexWrap::Wrap, margin: UiRect::vertical(Val::Px(6.0)), ..default() }).with_children(|r| {
                for (i, name) in TABS.iter().enumerate() {
                    button(r, name, Btn::Tab(i), i == e.tab);
                }
            });
            // Sliders.
            for (n, i) in rows(e.tab).into_iter().enumerate() {
                let def = &DEFS[i];
                p.spawn((
                    RowBg(n),
                    Node { flex_direction: FlexDirection::Column, padding: UiRect::axes(Val::Px(6.0), Val::Px(4.0)), ..default() },
                    BackgroundColor(rgba(t::ROW)),
                ))
                .with_children(|row| {
                    let grey = def.tab == WATER_TAB && !has_water();
                    row.spawn(Node { justify_content: JustifyContent::SpaceBetween, ..default() }).with_children(|l| {
                        l.spawn(text(def.label, 15.0, if grey { t::INK_MUTE } else { t::INK }));
                        // (Its value: click to reset it.)
                        l.spawn((Button, Btn::Reset(i), Node::default())).with_children(|b| {
                            b.spawn((ValueText(i), text("", 14.0, t::INK_DIM)));
                        });
                    });
                    row.spawn((
                        Track(i),
                        Button,
                        RelativeCursorPosition::default(),
                        Node { width: Val::Percent(100.0), height: Val::Px(10.0), margin: UiRect::top(Val::Px(4.0)), ..default() },
                        BackgroundColor(rgba(t::HAIRLINE)),
                    ))
                    .with_children(|tr| {
                        tr.spawn((Fill(i), Node { width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() }, BackgroundColor(rgba(t::ACCENT))));
                    });
                });
            }
            p.spawn((Note, text("", 13.0, t::WARN), Node { margin: UiRect::vertical(Val::Px(6.0)), ..default() }));
            p.spawn(Node { flex_wrap: FlexWrap::Wrap, ..default() }).with_children(|r| {
                button(r, "Save", Btn::Save, true);
                button(r, "Hold: CoD4's look", Btn::Ab, false);
                button(r, "Reset map", Btn::ResetMap, false);
                button(r, "Fly camera (F8)", Btn::Fly, false);
                button(r, "Close (F9)", Btn::Close, false);
            });
            p.spawn((Button, Btn::AsShowcase, Node { margin: UiRect::top(Val::Px(2.0)), ..default() })).with_children(|b| {
                b.spawn((SaveAs, text("", 13.0, t::INK_DIM)));
            });
            p.spawn(text("Drag a slider, click a value to reset it. Pad: D-pad, LB/RB tabs, X reset, Y save, hold LT for A/B, B close. Keys: arrows, PgUp/PgDn, Del, Ctrl+S, hold H for A/B. F8: fly camera (WASD, mouse, wheel speed, Shift fast; F8 again to return)", 12.0, t::INK_DIM));
        });
}

fn set_cursor(cursor: &mut CursorOptions, free: bool) {
    cursor.grab_mode = if free { CursorGrabMode::None } else { CursorGrabMode::Locked };
    cursor.visible = free;
}

fn pad_combo(pads: &Query<&Gamepad>) -> bool {
    pads.iter().any(|g| g.pressed(GamepadButton::Select) && g.just_pressed(GamepadButton::DPadDown))
}

/// F9 (or the pad combo) opens and closes it; the pad's B closes.
fn toggle(
    mut commands: Commands,
    mut e: ResMut<Editor>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    panels: Query<Entity, With<Panel>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    time: Res<Time<Real>>,
    mut debug_done: Local<u32>,
) {
    // Debug (screenshots): `COD4RW_LOOKEDIT=<tab>` opens it on that tab a few
    // seconds in; `COD4RW_LOOKEDIT_SET="k=v;k=v"` sets knobs ({m} the map)
    // and `COD4RW_LOOKEDIT_SAVE=1` saves them.
    let mut flip = (keys.just_pressed(KeyCode::F9) || pad_combo(&pads)) && crate::dev::allow("the look editor (F9)");
    if *debug_done != crate::atmos::climate::match_id() && time.elapsed_secs() > 4.0 {
        *debug_done = crate::atmos::climate::match_id();
        if let Ok(set) = std::env::var("COD4RW_LOOKEDIT_SET") {
            let map = crate::atmos::climate::map_name();
            for (k, v) in parse(&set.replace(';', "\n").replace("{m}", &map)) {
                crate::tune::look_set(&k, Some(v));
            }
            if std::env::var_os("COD4RW_LOOKEDIT_SAVE").is_some() {
                info!("map look: {}", save(crate::atmos::climate::showcase()));
            }
        }
        if let Some(tab) = std::env::var("COD4RW_LOOKEDIT").ok().and_then(|t| t.parse::<usize>().ok()) {
            e.tab = tab.min(TABS.len() - 1);
            flip = !e.open;
        }
    }
    // (Not Esc: that's the game's menu.)
    let close = e.open && pads.iter().any(|g| g.just_pressed(GamepadButton::East));
    if flip || close {
        e.open = !e.open && !close;
        if e.open {
            e.as_showcase = crate::atmos::climate::showcase();
            e.dirty = true;
        } else {
            crate::atmos::climate::set_original(false);
        }
        set_cursor(&mut cursor, e.open);
    }
    if (!e.open || e.dirty) && !panels.is_empty() {
        for p in &panels {
            commands.entity(p).try_despawn();
        }
    }
    if e.open && e.dirty {
        e.dirty = false;
        build(&mut commands, &e);
    }
}

/// Pad and keys: rows, values, tabs, reset, save, A/B.
#[allow(clippy::too_many_arguments)]
fn navigate(
    mut e: ResMut<Editor>,
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    tod: Res<crate::atmos::climate::TimeOfDay>,
    weather: Res<crate::atmos::climate::Weather>,
    suns: Query<(&Name, &GlobalTransform), With<DirectionalLight>>,
) {
    if !e.open {
        return;
    }
    let pressed = |b: GamepadButton| pads.iter().any(|g| g.just_pressed(b));
    let held = |b: GamepadButton| pads.iter().any(|g| g.pressed(b));
    let list = rows(e.tab);
    if list.is_empty() {
        return;
    }
    if pressed(GamepadButton::LeftTrigger) || keys.just_pressed(KeyCode::PageUp) {
        e.tab = (e.tab + TABS.len() - 1) % TABS.len();
        e.row = 0;
        e.dirty = true;
        return;
    }
    if pressed(GamepadButton::RightTrigger) || keys.just_pressed(KeyCode::PageDown) {
        e.tab = (e.tab + 1) % TABS.len();
        e.row = 0;
        e.dirty = true;
        return;
    }
    if pressed(GamepadButton::DPadDown) && !held(GamepadButton::Select) || keys.just_pressed(KeyCode::ArrowDown) {
        e.row = (e.row + 1) % list.len();
    }
    if pressed(GamepadButton::DPadUp) || keys.just_pressed(KeyCode::ArrowUp) {
        e.row = (e.row + list.len() - 1) % list.len();
    }
    e.row = e.row.min(list.len() - 1);
    let def = &DEFS[list[e.row]];
    let sun_dir = suns.iter().find(|(n, _)| n.as_str() == "sun").map(|(_, g)| -g.forward().as_vec3());
    let now = now_of(&tod, &weather, sun_dir);
    // Left/right, repeating while held.
    let dir = (held(GamepadButton::DPadRight) || keys.pressed(KeyCode::ArrowRight)) as i32 - (held(GamepadButton::DPadLeft) || keys.pressed(KeyCode::ArrowLeft)) as i32;
    let t = time.elapsed_secs();
    let step_now = if dir == 0 {
        e.repeat = (0, 0.0);
        false
    } else if e.repeat.0 != dir {
        e.repeat = (dir, t + 0.35);
        true
    } else if t >= e.repeat.1 {
        e.repeat.1 = t + 0.06;
        true
    } else {
        false
    };
    if step_now {
        let (v, _) = value(def, &now);
        set(def, &now, if def.unit == Unit::Toggle { 1.0 - v.round() } else { v + dir as f32 * def.step });
    }
    if pressed(GamepadButton::South) && def.unit == Unit::Toggle {
        let (v, _) = value(def, &now);
        set(def, &now, 1.0 - v.round());
    }
    if pressed(GamepadButton::West) || keys.just_pressed(KeyCode::Delete) {
        crate::tune::look_set(&key(def), None);
    }
    if pressed(GamepadButton::North) || (keys.pressed(KeyCode::ControlLeft) && keys.just_pressed(KeyCode::KeyS)) {
        let m = save(e.as_showcase);
        e.message = Some((m, t + 4.0));
    }
    // A/B: CoD4's look while held (the button's handled in `buttons`).
    let ab = held(GamepadButton::LeftTrigger2) || keys.pressed(KeyCode::KeyH);
    if ab != crate::atmos::climate::original() && (ab || !AB_BUTTON.load(std::sync::atomic::Ordering::Relaxed)) {
        crate::atmos::climate::set_original(ab);
    }
}

static AB_BUTTON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Dragging a slider.
#[allow(clippy::type_complexity)]
fn mouse(
    e: Res<Editor>,
    tracks: Query<(&Track, &Interaction, &RelativeCursorPosition)>,
    tod: Res<crate::atmos::climate::TimeOfDay>,
    weather: Res<crate::atmos::climate::Weather>,
    suns: Query<(&Name, &GlobalTransform), With<DirectionalLight>>,
) {
    if !e.open {
        return;
    }
    let sun_dir = suns.iter().find(|(n, _)| n.as_str() == "sun").map(|(_, g)| -g.forward().as_vec3());
    let now = now_of(&tod, &weather, sun_dir);
    for (track, interaction, rel) in &tracks {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(p) = rel.normalized else { continue };
        let def = &DEFS[track.0];
        let (lo, hi) = range(def, &now);
        let f = (p.x + 0.5).clamp(0.0, 1.0);
        let raw = lo + (hi - lo) * f;
        let v = (raw / def.step).round() * def.step;
        set(def, &now, v);
    }
}

fn buttons(
    mut e: ResMut<Editor>,
    time: Res<Time<Real>>,
    clicked: Query<(&Interaction, &Btn), Changed<Interaction>>,
    all: Query<(&Interaction, &Btn)>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut fly: ResMut<crate::flycam::Flycam>,
) {
    if !e.open {
        return;
    }
    // (The A/B button: CoD4's look while it's held.)
    let ab = all.iter().any(|(i, b)| matches!(b, Btn::Ab) && *i == Interaction::Pressed);
    AB_BUTTON.store(ab, std::sync::atomic::Ordering::Relaxed);
    if ab {
        crate::atmos::climate::set_original(true);
    } else if clicked.iter().any(|(_, b)| matches!(b, Btn::Ab)) {
        crate::atmos::climate::set_original(false);
    }
    let t = time.elapsed_secs();
    for (interaction, b) in &clicked {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *b {
            Btn::Tab(i) => {
                e.tab = i;
                e.row = 0;
                e.dirty = true;
            }
            Btn::Reset(i) => crate::tune::look_set(&key(&DEFS[i]), None),
            Btn::Save => {
                let m = save(e.as_showcase);
                e.message = Some((m, t + 4.0));
            }
            Btn::ResetMap => {
                crate::tune::look_replace(BTreeMap::new());
                e.message = Some(("Map reset to its defaults (Save to keep it so)".into(), t + 4.0));
            }
            Btn::AsShowcase => e.as_showcase = !e.as_showcase,
            Btn::Close => {
                e.open = false;
                crate::atmos::climate::set_original(false);
                set_cursor(&mut cursor, false);
            }
            Btn::Ab => {}
            Btn::Fly => {
                // (The panel closes: the mouse is the camera's while flying.)
                e.open = false;
                crate::atmos::climate::set_original(false);
                fly.enter();
            }
        }
    }
}

/// Values, fills, the selected row and the notes, each frame.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn refresh(
    mut e: ResMut<Editor>,
    time: Res<Time<Real>>,
    tod: Res<crate::atmos::climate::TimeOfDay>,
    weather: Res<crate::atmos::climate::Weather>,
    suns: Query<(&Name, &GlobalTransform), With<DirectionalLight>>,
    mut values: Query<(&ValueText, &mut Text), (Without<Note>, Without<SaveAs>)>,
    mut fills: Query<(&Fill, &mut Node)>,
    mut rows_bg: Query<(&RowBg, &mut BackgroundColor)>,
    mut note: Query<&mut Text, (With<Note>, Without<SaveAs>, Without<ValueText>)>,
    mut save_as: Query<&mut Text, (With<SaveAs>, Without<Note>, Without<ValueText>)>,
) {
    use crate::ui::next::theme as t;
    if !e.open {
        return;
    }
    let sun_dir = suns.iter().find(|(n, _)| n.as_str() == "sun").map(|(_, g)| -g.forward().as_vec3());
    let now = now_of(&tod, &weather, sun_dir);
    for (v, mut s) in &mut values {
        let def = &DEFS[v.0];
        let (x, set) = value(def, &now);
        let want = format(def, x, set);
        if s.0 != want {
            s.0 = want;
        }
    }
    for (f, mut node) in &mut fills {
        let def = &DEFS[f.0];
        let (lo, hi) = range(def, &now);
        let (x, _) = value(def, &now);
        let pct = if hi > lo { ((x - lo) / (hi - lo)).clamp(0.0, 1.0) * 100.0 } else { 0.0 };
        node.width = Val::Percent(pct);
    }
    for (r, mut bg) in &mut rows_bg {
        bg.0 = rgba(if r.0 == e.row { t::ROW_FOCUS } else { t::ROW });
    }
    // Notes: why the clock is held close, the sun moved off its bake, a save.
    let mut lines: Vec<String> = Vec::new();
    if e.tab == 0 {
        if !crate::atmos::climate::tod_keys() {
            lines.push("No baked time-of-day lighting here: the lightmaps stay at CoD4's hour, so the clock keeps within 1.5 h of it.".into());
        }
        if !tod.enabled {
            lines.push("The clock runs with the showcase only (this map's is off).".into());
        }
        if let (Some(over), Some(map)) = (crate::atmos::climate::sun_override(), tod.map.as_ref()) {
            if !crate::atmos::climate::tod_keys() && over.angle_between(map.dir).to_degrees() > 15.0 {
                lines.push("The sun is far from where CoD4 baked it: baked light and shadows won't follow it.".into());
            }
        }
    }
    if e.tab == WATER_TAB && !has_water() {
        lines.push("No water on this map.".into());
    }
    if e.tab == 4 && !weather.enabled {
        lines.push("Weather runs with the showcase only (this map's is off).".into());
    }
    if crate::atmos::climate::original() {
        lines.push("Showing CoD4's look (A/B)".into());
    }
    if let Some((m, until)) = e.message.clone() {
        if time.elapsed_secs() < until {
            lines.push(m);
        } else {
            e.message = None;
        }
    }
    let want = lines.join("\n");
    for mut n in &mut note {
        if n.0 != want {
            n.0 = want.clone();
        }
    }
    let label = format!("Save as: {} look (click to switch)", if e.as_showcase { "showcase" } else { "normal" });
    for mut s in &mut save_as {
        if s.0 != label {
            s.0 = label.clone();
        }
    }
}

/// The sun's brightness, colour and (on maps whose clock doesn't place it)
/// position from the editor's knobs, over what the map (or the clock) set.
#[derive(Component)]
struct SunBase {
    illuminance: f32,
    color: LinearRgba,
    rotation: Quat,
}

#[allow(clippy::type_complexity)]
fn apply_sun(
    mut commands: Commands,
    tod: Res<crate::atmos::climate::TimeOfDay>,
    mut suns: Query<(Entity, &Name, &mut DirectionalLight, &mut Transform, Option<&SunBase>), Without<crate::model_lighting::ViewModelSun>>,
) {
    let map = crate::atmos::climate::map_name();
    let brightness = crate::tune::get(&format!("sun.brightness.{map}"), 1.0).clamp(0.0, 10.0);
    let warmth = crate::tune::get(&format!("sun.warmth.{map}"), 0.0).clamp(-1.0, 1.0);
    let over = crate::atmos::climate::sun_override();
    // (The showcase's clock rewrites the sun each frame; elsewhere it's
    // the map's, set once.)
    let clocked = tod.enabled && crate::atmos::climate::profile().is_some_and(|p| p.clock);
    for (e, name, mut light, mut tf, base) in &mut suns {
        if name.as_str() != "sun" {
            continue;
        }
        let base = if clocked {
            SunBase { illuminance: light.illuminance, color: light.color.to_linear(), rotation: tf.rotation }
        } else if let Some(b) = base {
            SunBase { illuminance: b.illuminance, color: b.color, rotation: b.rotation }
        } else {
            let b = SunBase { illuminance: light.illuminance, color: light.color.to_linear(), rotation: tf.rotation };
            commands.entity(e).try_insert(SunBase { illuminance: b.illuminance, color: b.color, rotation: b.rotation });
            b
        };
        let illuminance = base.illuminance * brightness;
        let c = base.color;
        let color = Color::linear_rgb(c.red * (1.0 + 0.3 * warmth), c.green, c.blue * (1.0 - 0.3 * warmth));
        if (light.illuminance - illuminance).abs() > 1e-3 {
            light.illuminance = illuminance;
        }
        if light.color != color {
            light.color = color;
        }
        let rotation = match (over, clocked) {
            (Some(d), false) => Transform::default().looking_to(-d, if d.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y }).rotation,
            _ => base.rotation,
        };
        if !clocked && tf.rotation != rotation {
            tf.rotation = rotation;
        }
    }
}

// --- water

static HAS_WATER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The map has water the Water tab drives: CoD4's water surfaces, or the
/// showcase's ocean.
fn has_water() -> bool {
    HAS_WATER.load(std::sync::atomic::Ordering::Relaxed) || crate::atmos::climate::profile().is_some_and(|p| p.ocean)
}

/// The map's water surfaces as the editor's Water knobs have them (colour,
/// brightness, clarity, waves, reflections), from their own values (kept
/// per match, so changes never compound). The showcase ocean reads the
/// same knobs itself (`crate::ocean`).
#[allow(clippy::type_complexity)]
fn water_look(
    time: Res<Time<Real>>,
    mut materials: ResMut<Assets<crate::world::WorldMaterial>>,
    mut base: Local<(u32, f32, Vec<(AssetId<crate::world::WorldMaterial>, Vec4, Vec4, Vec4)>)>,
    mut last: Local<Option<[f32; 5]>>,
) {
    let id = crate::atmos::climate::match_id();
    if base.0 != id {
        *base = (id, 0.0, Vec::new());
        *last = None;
        HAS_WATER.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    // (Found once the map's materials are there: looked for now and then.)
    if base.2.is_empty() {
        let now = time.elapsed_secs();
        if now < base.1 {
            return;
        }
        base.1 = now + 2.0;
        base.2 = materials
            .iter()
            .filter(|(_, m)| m.extension.params.water_color.w >= 1.0)
            .map(|(i, m)| (i, m.extension.params.water_color, m.extension.params.water_env, m.extension.params.sun_dir))
            .collect();
        if base.2.is_empty() {
            return;
        }
        HAS_WATER.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let map = crate::atmos::climate::map_name();
    let knob = |n: &str, d: f32| crate::tune::get(&format!("water.{n}.{map}"), d);
    // (Its waves: the muddy water's calm as the default.)
    let calm = (base.2[0].1.w - 1.0).clamp(0.0, 1.0);
    let now = [
        knob("tint", 0.0).clamp(-1.0, 1.0),
        knob("brightness", 1.0).clamp(0.0, 3.0),
        knob("clarity", 1.0).clamp(0.0, 3.0),
        knob("waves", 1.0 - calm).clamp(0.0, 1.5),
        knob("reflection", 1.0).clamp(0.0, 3.0),
    ];
    if *last == Some(now) {
        return;
    }
    *last = Some(now);
    let [tint, bright, clarity, waves, reflection] = now;
    let shade = Vec3::new(1.0 + 0.4 * tint, 1.0 + 0.1 * tint, 1.0 - 0.4 * tint) * bright;
    for (i, colour, env, sun) in base.2.clone() {
        let Some(mut m) = materials.get_mut(i) else { continue };
        let p = &mut m.extension.params;
        p.water_color = (colour.truncate() * shade).extend(1.0 + (1.0 - waves).clamp(0.0, 1.0));
        p.water_env = Vec4::new(env.x * reflection, env.y * reflection, env.z, env.w);
        p.sun_dir = sun.truncate().extend(clarity + 1.0);
    }
}
