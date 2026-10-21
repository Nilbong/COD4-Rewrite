//! The map editor: F7 in a match. The camera leaves the player
//! ([`crate::flycam`], hold the right mouse button to fly), a palette of the
//! loaded zones' props opens on the left, and objects are placed on the map
//! by clicking: they stand on whatever's under the pointer, solid to players
//! and bullets, and the bots route round them once the editor closes.
//! Saved per map ([`layout`]), loaded as each map loads.
//!
//! Mouse: click a picture, then click the map to place it (again and again;
//! X stops). Click an object to select it, drag it to move it along the
//! ground. Hold the right button to fly (WASD, Space/Ctrl, wheel the speed).
//! Keys (not while flying): Q/E turn (Shift: 1 degree), PageUp/PageDown
//! raise and lower, +/- scale, G grid snap, T align to slopes, Ctrl+D
//! duplicate, Delete removes, Ctrl+Z / Ctrl+Y undo and redo, Ctrl+S saves,
//! X drops the selection, F7 closes.
//!
//! Painting: with an object picked, hold Shift and drag to scatter it
//! (random turn and size) within the brush; Alt and drag rubs placed
//! objects out; [ and ] size the brush. H hides the map's own object under
//! the pointer (or shows it again): gone from the next load, as the map's
//! models are merged as it loads. Lights (C colour, +/- brightness, [ ]
//! range, V shadows), effects and spawn points come from their own tabs.
//!
//! The palette holds the loaded zones' models first, then those of every
//! other CoD4 level ([`stream`]: a level loads on its own thread when a
//! picture or a placed model needs it, then goes), and CoD4's characters,
//! placed as people idling ([`actors`]). With a person selected (or being
//! placed), the wheel or Up/Down picks the idle, Left/Right the character,
//! K the gun. / searches the palette (Enter or Esc ends); the wheel over the
//! palette turns its pages. In Headquarters it opens as anywhere else, with
//! a layout of its own ([`layout::key`]).
//!
//! Pad: Back + D-pad up opens and closes it. The sticks fly (triggers up and
//! down, bumpers the speed) and the middle of the screen is the pointer: A
//! places or selects, B drops, X removes, Y saves, D-pad left/right the
//! previous/next object of the tab (a person selected: the idle), D-pad
//! up/down turns.

pub mod actors;
pub mod catalog;
pub mod layout;
mod stream;
mod thumbs;

use crate::collision::Layer;
use crate::content::Content;
use crate::state::{GameState, Setup, in_game};
use crate::units::u;
use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use layout::{Layout, ObjectBounds, Placed, PlacedObject, Prepared};
use std::collections::HashMap;

pub struct MapEditorPlugin;

impl Plugin for MapEditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Editor>()
            .init_resource::<Layout>()
            .init_resource::<ModelCache>()
            .init_resource::<thumbs::Thumbs>()
            .init_resource::<stream::Streamer>()
            .init_resource::<actors::ActorAssets>()
            // (Says at the start whether the dev tools are on.)
            .add_systems(Startup, || {
                crate::dev::enabled();
            })
            .add_systems(OnEnter(GameState::InGame), (load_layout, apply_spawns).chain().in_set(Setup::Spawn))
            .add_systems(OnExit(GameState::InGame), leave)
            .add_systems(
                Update,
                (test, toggle, buttons, search, edit, stream::step, retry, respawn, start_fx, apply_spawns, panel, status, draw, make_thumbs, actors::idle).chain().run_if(in_game),
            );
    }
}

/// Whether the editor's open (the game's input is the editor's, and player
/// 1 can't be hurt).
pub fn open() -> bool {
    OPEN.load(std::sync::atomic::Ordering::Relaxed)
}

static OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A pad is in use: the pointer's the middle of the screen (until the mouse
/// moves).
static PAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn pad_mode() -> bool {
    PAD.load(std::sync::atomic::Ordering::Relaxed)
}

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};

/// What the pads did this frame (the first that did anything).
#[derive(Default, Clone, Copy)]
struct PadPress {
    a: bool,
    b: bool,
    x: bool,
    y: bool,
    left: bool,
    right: bool,
    up: bool,
    down: bool,
}

fn pad_press(pads: &Query<&Gamepad>) -> PadPress {
    let mut p = PadPress::default();
    for g in pads {
        // (Back + D-pad is the editors' own toggles.)
        if g.pressed(GamepadButton::Select) {
            continue;
        }
        p.a |= g.just_pressed(GamepadButton::South);
        p.b |= g.just_pressed(GamepadButton::East);
        p.x |= g.just_pressed(GamepadButton::West);
        p.y |= g.just_pressed(GamepadButton::North);
        p.left |= g.just_pressed(GamepadButton::DPadLeft);
        p.right |= g.just_pressed(GamepadButton::DPadRight);
        p.up |= g.just_pressed(GamepadButton::DPadUp);
        p.down |= g.just_pressed(GamepadButton::DPadDown);
    }
    p
}

/// The palette's width (logical pixels): clicks there aren't the map's.
const PANEL_WIDTH: f32 = 330.0;
const COLUMNS: usize = 3;
const ROWS: usize = 4;
/// Grid snap step.
const GRID: f32 = u(16.0);
const UNDO_DEPTH: usize = 200;

#[derive(Resource, Default)]
struct Editor {
    open: bool,
    /// The props to pick from (filled as it first opens in a match).
    models: Vec<String>,
    category: usize,
    page: usize,
    /// The palette needs building anew.
    dirty: bool,
    /// The model being placed, and its ghost.
    placing: Option<String>,
    ghost: Option<(String, Entity)>,
    /// The turn and size the next placed object gets.
    place_yaw: f32,
    place_scale: f32,
    selected: Option<usize>,
    /// Dragging the selection: where the press began (to ignore a click's
    /// wobble) and whether it's moved yet.
    drag: Option<(Vec2, bool)>,
    grid: bool,
    align: bool,
    undo: Vec<Vec<Placed>>,
    redo: Vec<Vec<Placed>>,
    /// The placed objects need spawning anew.
    respawn: bool,
    /// Changed since opened (the bots' routes are built anew on closing).
    changed: bool,
    /// Changed since saved.
    unsaved: bool,
    message: Option<(String, f32)>,
    /// Parent of the placed objects.
    root: Option<Entity>,
    /// Only objects from this index on are new (a paint stroke): spawn just
    /// those.
    respawn_from: Option<usize>,
    /// The map's own models (to hide), listed as the editor first opens.
    map_objects: Vec<MapObject>,
    /// The paint brush's radius (metres) and when it next drops one.
    brush: f32,
    next_dab: f32,
    /// The map's own spawn points (how many), before the placed ones.
    base_spawns: Option<usize>,
    /// The placed spawn points need applying again.
    spawns_dirty: bool,
    /// The palette's search, and whether it's being typed.
    query: String,
    typing: bool,
    /// Only these placed objects need spawning anew (a person's idle
    /// changed).
    respawn_only: Vec<usize>,
    /// Some placed objects couldn't be spawned (their level's on its way),
    /// and the streamer's count when last tried.
    missing: bool,
    tried_at: u32,
    /// The next person's idle and gun.
    actor_preset: String,
    actor_gun: String,
    /// The loaded zones' models (lower case): the rest are other levels'.
    loaded: std::collections::HashSet<String>,
}

/// One of the map's own models (a static model or a script model).
#[derive(Clone)]
struct MapObject {
    model: String,
    /// CoD space.
    origin: [f32; 3],
    tf: Transform,
    min: Vec3,
    max: Vec3,
}

impl MapObject {
    fn hidden_in(&self, layout: &Layout) -> Option<usize> {
        let at = crate::units::pos(self.origin);
        layout.objects.iter().position(|o| matches!(o.kind(), layout::Kind::Hide(m) if m == self.model) && o.pos.distance(at) < u(2.0))
    }
}

/// The map's own models, with their boxes.
fn map_objects(content: &Content) -> Vec<MapObject> {
    let mut out = Vec::new();
    let map = content.map();
    if let Some(gfx) = map.gfx_world() {
        for sm in &gfx.static_models {
            let Some(id) = sm.model else { continue };
            let (zi, id) = content.resolve_xmodel(crate::content::MAP_ZONE, id);
            let Some(xm) = content.zone(zi).xmodel(id) else { continue };
            let (a, b) = (crate::units::pos(xm.mins), crate::units::pos(xm.maxs));
            out.push(MapObject {
                model: xm.name.clone(),
                origin: sm.origin,
                tf: Transform { translation: crate::units::pos(sm.origin), rotation: crate::units::axis_rotation(sm.axis), scale: Vec3::splat(sm.scale) },
                min: a.min(b),
                max: a.max(b),
            });
        }
    }
    let ents = map.map_ents().map(|m| iw3::ents::parse(&m.entity_string)).unwrap_or_default();
    for e in ents.iter().filter(|e| e.classname() == "script_model") {
        let (Some(origin), Some(name)) = (e.origin(), e.get("model")) else { continue };
        let Some((zi, id)) = content.find(name) else { continue };
        let (zi, id) = content.resolve_xmodel(zi, id);
        let Some(xm) = content.zone(zi).xmodel(id) else { continue };
        let (a, b) = (crate::units::pos(xm.mins), crate::units::pos(xm.maxs));
        let scale = e.get("modelscale").and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
        out.push(MapObject {
            model: name.to_owned(),
            origin,
            tf: Transform::from_translation(crate::units::pos(origin)).with_rotation(crate::modes::koth::cod_rotation(e.angles())).with_scale(Vec3::splat(scale)),
            min: a.min(b),
            max: a.max(b),
        });
    }
    out
}

/// Where a ray enters a box (`min`..`max` in the space `tf` places), and
/// how far along.
fn ray_box(tf: &Transform, min: Vec3, max: Vec3, from: Vec3, dir: Dir3) -> Option<f32> {
    let m = tf.compute_affine();
    let inv = m.inverse();
    let (lf, ld) = (inv.transform_point3(from), inv.transform_vector3(*dir));
    let (mut near, mut far) = (0.0f32, f32::MAX);
    for a in 0..3 {
        let (o, d, lo, hi) = (lf[a], ld[a], min[a], max[a]);
        if d.abs() < 1e-8 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let (t1, t2) = ((lo - o) / d, (hi - o) / d);
        near = near.max(t1.min(t2));
        far = far.min(t1.max(t2));
    }
    (near <= far).then(|| m.transform_point3(lf + ld * near).distance(from))
}

impl Editor {
    fn say(&mut self, s: impl Into<String>, now: f32) {
        let s = s.into();
        info!("map editor: {s}");
        self.message = Some((s, now + 4.0));
    }

    /// Before a change: what to go back to.
    fn checkpoint(&mut self, layout: &Layout) {
        self.undo.push(layout.objects.clone());
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.changed = true;
        self.unsaved = true;
    }

    fn filtered(&self) -> Vec<&String> {
        let cat = catalog::CATEGORIES[self.category];
        self.models
            .iter()
            .filter(|m| (cat == catalog::Category::All || catalog::category(m) == cat) && (self.query.is_empty() || catalog::matches(m, &self.query)))
            .collect()
    }

    /// The person a placement makes from the palette's `@actor:<id>`.
    fn actor_spec(&self, picked: &str) -> Option<actors::Spec> {
        let mut spec = actors::Spec::parse(picked)?;
        if !self.actor_preset.is_empty() {
            spec.preset = self.actor_preset.clone();
        }
        if !self.actor_gun.is_empty() {
            spec.gun = self.actor_gun.clone();
        }
        Some(spec)
    }
}

/// Models made ready to place, by name (`None`: can't be).
#[derive(Resource, Default)]
struct ModelCache(HashMap<String, Option<Prepared>>);

impl ModelCache {
    fn get(&mut self, name: &str, content: &mut Content, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Option<Prepared> {
        self.0.entry(name.to_owned()).or_insert_with(|| layout::prepare(content, name, meshes, materials, images)).clone()
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Models<'w> {
    cache: ResMut<'w, ModelCache>,
    content: ResMut<'w, Content>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    images: ResMut<'w, Assets<Image>>,
    streamer: ResMut<'w, stream::Streamer>,
    cast: actors::Cast<'w>,
    time: Res<'w, Time<Real>>,
}

/// A palette picture's model: ready (or never), or its level's on the way.
enum ForThumb {
    Ready(Option<Prepared>),
    Wait,
}

impl Models<'_> {
    fn now(&self) -> f32 {
        self.time.elapsed_secs()
    }

    /// A light's, effect's, spawn point's or (not here yet) person's box.
    fn marker(name: &str) -> Prepared {
        let probe = Placed { model: name.to_owned(), pos: Vec3::ZERO, rot: Quat::IDENTITY, scale: 1.0, params: vec![] };
        let (min, max) = layout::marker_bounds(probe.kind());
        Prepared { parts: vec![], min, max, boxes: vec![], solid: false, surface: 0 }
    }

    /// Whether `name` is another level's model (not the map's zones').
    fn elsewhere(&self, name: &str) -> bool {
        !name.starts_with('@') && !self.cache.0.contains_key(name) && self.content.find(name).is_none() && stream::index().models.contains_key(&name.to_ascii_lowercase())
    }

    /// A model made ready (from the map's zones, or another level's once
    /// it's here: `None` until then), a person's stand-in, or a marker's
    /// box.
    fn get(&mut self, name: &str) -> Option<Prepared> {
        if let Some(spec) = actors::Spec::parse(name) {
            return Some(self.stand_in(&spec, true).unwrap_or_else(|| Self::marker(name)));
        }
        if name.starts_with('@') {
            return Some(Self::marker(name));
        }
        if !self.elsewhere(name) {
            return self.cache.get(name, &mut self.content, &mut self.meshes, &mut self.materials, &mut self.images);
        }
        if self.streamer.extra_models >= stream::MAX_EXTRA_MODELS {
            warn!("map editor: {} models from other levels already: {name} left out", stream::MAX_EXTRA_MODELS);
            self.cache.0.insert(name.to_owned(), None);
            return None;
        }
        let now = self.now();
        let Some(zone) = self.streamer.holding(name, now) else {
            if let Some(z) = self.streamer.zone_for(name) {
                self.streamer.want(&z);
            }
            return None;
        };
        let p = layout::prepare(zone, name, &mut self.meshes, &mut self.materials, &mut self.images);
        self.streamer.extra_models += 1;
        self.cache.0.insert(name.to_owned(), p.clone());
        p
    }

    /// A model for a palette picture (another level's isn't kept after).
    fn for_thumb(&mut self, name: &str) -> ForThumb {
        if let Some(spec) = actors::Spec::parse(name) {
            return match self.stand_in(&spec, false) {
                Some(p) => ForThumb::Ready(Some(p)),
                None if spec.character().is_none_or(|c| self.streamer.failed(c.zone)) => ForThumb::Ready(None),
                None => ForThumb::Wait,
            };
        }
        if !self.elsewhere(name) {
            return ForThumb::Ready(self.get(name));
        }
        let now = self.now();
        if let Some(zone) = self.streamer.holding(name, now) {
            return ForThumb::Ready(layout::prepare(zone, name, &mut self.meshes, &mut self.materials, &mut self.images));
        }
        match self.streamer.zone_for(name) {
            Some(z) if !self.streamer.failed(&z) => {
                self.streamer.want(&z);
                ForThumb::Wait
            }
            _ => ForThumb::Ready(None),
        }
    }

    /// A person drawn unskinned (as bound): kept for the ghost, or made
    /// afresh for a picture. `None` while their level's on its way.
    fn stand_in(&mut self, spec: &actors::Spec, keep: bool) -> Option<Prepared> {
        let c = spec.character()?;
        if let Some(p) = self.cast.assets.statics.get(c.id) {
            return Some(p.clone());
        }
        let now = self.now();
        let here = c.models.iter().all(|m| self.content.find(m).is_some());
        let from: &mut Content = if here {
            &mut self.content
        } else {
            match self.streamer.content(c.zone, now) {
                Some(z) => z,
                None => {
                    self.streamer.want(c.zone);
                    return None;
                }
            }
        };
        let p = actors::stand_in(c, from, &mut self.meshes, &mut self.materials, &mut self.images);
        if keep {
            self.cast.assets.statics.insert(c.id, p.clone());
        }
        Some(p)
    }
}

/// Spawns placed object `i` (as its kind wants); false if it can't be
/// (yet: its level may be on its way).
fn spawn_one(commands: &mut Commands, models: &mut Models, o: &Placed, i: usize, root: Entity) -> bool {
    match o.kind() {
        layout::Kind::Actor(token) => match actors::Spec::parse(token) {
            Some(spec) => {
                let now = models.now();
                actors::spawn(commands, models, o, &spec, i, root, now)
            }
            None => false,
        },
        layout::Kind::Model(m) => match models.get(m) {
            Some(p) => {
                layout::spawn(commands, &p, o, i, Some(root));
                true
            }
            None => false,
        },
        layout::Kind::Hide(_) => true,
        _ => layout::spawn_marker(commands, o, i, Some(root)).is_some(),
    }
}

// --- loading

fn load_layout(mut commands: Commands, mut e: ResMut<Editor>, mut layout: ResMut<Layout>, mut models: Models) {
    let map = layout::key(&crate::atmos::climate::map_name());
    layout.objects = layout::text_for(&map).map(|t| layout::parse(&t)).unwrap_or_default();
    let root = commands.spawn((Name::new("placed objects"), Transform::default(), Visibility::default())).id();
    e.root = Some(root);
    let mut missing = 0;
    for (i, o) in layout.objects.iter().enumerate() {
        if !spawn_one(&mut commands, &mut models, o, i, root) {
            missing += 1;
        }
    }
    // (Those from other levels come as their levels load: `retry`.)
    e.missing = missing > 0;
    e.tried_at = models.streamer.generation;
    e.spawns_dirty = true;
    let hides = layout.objects.iter().filter(|o| matches!(o.kind(), layout::Kind::Hide(_))).count();
    if hides > 0 {
        info!("map editor: {hides} of the map's own models hidden");
    }
    if !layout.objects.is_empty() {
        info!(
            "map editor: {} placed objects on {map}{} (layout {:016x})",
            layout.objects.len(),
            if missing > 0 { format!(", {missing} waiting for their levels") } else { String::new() },
            layout::hash(&layout.objects)
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn leave(
    mut commands: Commands,
    mut e: ResMut<Editor>,
    mut cache: ResMut<ModelCache>,
    mut thumbs: ResMut<thumbs::Thumbs>,
    mut layout: ResMut<Layout>,
    mut streamer: ResMut<stream::Streamer>,
    mut cast: ResMut<actors::ActorAssets>,
) {
    if e.open {
        OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    *e = Editor::default();
    cache.0.clear();
    streamer.reset();
    cast.reset();
    thumbs.reset(&mut commands);
    layout.objects.clear();
}

// --- opening and closing

#[allow(clippy::too_many_arguments)]
fn toggle(
    mut commands: Commands,
    mut e: ResMut<Editor>,
    keys: Res<ButtonInput<KeyCode>>,
    mut fly: ResMut<crate::flycam::Flycam>,
    content: Res<Content>,
    panels: Query<Entity, With<Panel>>,
    time: Res<Time<Real>>,
    pads: Query<&Gamepad>,
    online: Option<Res<crate::netplay::NetStart>>,
) {
    let combo = pads.iter().any(|g| g.pressed(GamepadButton::Select) && g.just_pressed(GamepadButton::DPadUp));
    if !keys.just_pressed(KeyCode::F7) && !combo || !e.open && !crate::dev::allow("the map editor (F7)") {
        return;
    }
    // Offline, on one screen only: open, player 1 can't be hurt
    // (`crate::combat`), which online would be god mode, and in splitscreen
    // the others would play on round a player who can't be.
    if !e.open && (online.is_some() || crate::splitscreen::count() > 1) {
        info!("map editor: not in {}", if online.is_some() { "an online match" } else { "splitscreen" });
        return;
    }
    if combo {
        PAD.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let open = !e.open;
    set_open(&mut commands, &mut e, open, &mut fly, &content, &panels, time.elapsed_secs());
}

fn set_open(commands: &mut Commands, e: &mut Editor, open: bool, fly: &mut crate::flycam::Flycam, content: &Content, panels: &Query<Entity, With<Panel>>, now: f32) {
    if open == e.open {
        return;
    }
    e.open = open;
    OPEN.store(open, std::sync::atomic::Ordering::Relaxed);
    if open {
        if e.models.is_empty() {
            e.models = catalog::models(content);
            e.place_scale = 1.0;
            e.brush = 2.0;
            e.actor_preset = actors::PRESETS[0].key.into();
            e.actor_gun = "auto".into();
            e.loaded = catalog::loaded(content);
            e.map_objects = map_objects(content);
            info!("map editor: {} models to place, {} of the map's own", e.models.len(), e.map_objects.len());
        }
        e.dirty = true;
        e.changed = false;
        fly.hold_to_fly = true;
        fly.enter();
        e.say("Map editor: pick an object, click to place. Hold right mouse to fly.", now);
    } else {
        fly.exit();
        for p in panels {
            commands.entity(p).try_despawn();
        }
        if let Some((_, g)) = e.ghost.take() {
            commands.entity(g).try_despawn();
        }
        e.placing = None;
        e.drag = None;
        if e.changed {
            crate::bots::request_nav_rebuild();
            e.spawns_dirty = true;
        }
        if e.unsaved {
            info!("map editor: closed with unsaved changes (they stay for this match)");
        }
    }
}

// --- the palette

#[derive(Component)]
struct Panel;

#[derive(Component, Clone)]
enum Btn {
    Category(usize),
    Item(String),
    Prev,
    Next,
    Save,
    Undo,
    Redo,
    Grid,
    Align,
    Close,
    Search,
}

#[derive(Component)]
struct Thumb(String);

#[derive(Component)]
struct Status;

fn rgba(c: [f32; 4]) -> Color {
    Color::srgba(c[0], c[1], c[2], c[3])
}

fn text(s: impl Into<String>, size: f32, c: [f32; 4]) -> impl Bundle {
    // (The menus' typeface.)
    (Text::new(s), crate::hud::menu_font(size), TextColor(rgba(c)))
}

fn button(p: &mut ChildSpawnerCommands, label: &str, b: Btn, accent: bool) {
    use crate::ui::next::theme as t;
    p.spawn((
        Button,
        b,
        Node { padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)), margin: UiRect::all(Val::Px(1.5)), ..default() },
        BackgroundColor(rgba(if accent { t::ACCENT_DIM } else { t::RAISED })),
    ))
    .with_children(|c| {
        c.spawn(text(label, 12.0, t::INK));
    });
}

fn panel(mut commands: Commands, mut e: ResMut<Editor>, panels: Query<Entity, With<Panel>>, mut thumbs: ResMut<thumbs::Thumbs>, mut shown: Query<(&Thumb, &mut ImageNode)>) {
    if !e.open {
        return;
    }
    // Pictures as they're ready.
    for (t, mut img) in &mut shown {
        if let Some(h) = thumbs.images.get(&t.0)
            && *h != Handle::default()
            && img.image != *h
        {
            img.image = h.clone();
        }
    }
    if !e.dirty {
        return;
    }
    e.dirty = false;
    for p in &panels {
        commands.entity(p).try_despawn();
    }
    use crate::ui::next::theme as t;
    let items = e.filtered();
    let per_page = COLUMNS * ROWS;
    let pages = items.len().div_ceil(per_page).max(1);
    let page = e.page.min(pages - 1);
    let shown_items: Vec<String> = items.iter().skip(page * per_page).take(per_page).map(|s| (*s).clone()).collect();
    thumbs.clear_queue();
    for m in &shown_items {
        thumbs.want(m);
    }
    let map = if crate::hq::active() { "HEADQUARTERS".to_owned() } else { crate::atmos::climate::map_name() };
    let category = e.category;
    let placing = e.placing.clone();
    let (grid, align) = (e.grid, e.align);
    let search = if e.typing { format!("Search: {}_", e.query) } else if e.query.is_empty() { "Search (/)".to_owned() } else { format!("Search: {}  (/ to change)", e.query) };
    let typing = e.typing;
    // Another level's model: which (it loads when shown or placed).
    let idx = stream::index();
    let loaded = &e.loaded;
    let elsewhere: Vec<Option<String>> = shown_items
        .iter()
        .map(|m| (!m.starts_with('@') && !loaded.contains(&m.to_ascii_lowercase())).then(|| idx.smallest(m).map(str::to_owned)).flatten())
        .collect();
    commands
        .spawn((
            Panel,
            crate::flycam::KeepUi,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(PANEL_WIDTH),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(10.0)),
                row_gap: Val::Px(4.0),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(rgba(t::PANEL)),
            GlobalZIndex(900),
        ))
        .with_children(|p| {
            p.spawn(text(format!("MAP EDITOR  -  {map}"), 19.0, t::INK));
            p.spawn(Node { flex_wrap: FlexWrap::Wrap, margin: UiRect::vertical(Val::Px(4.0)), ..default() }).with_children(|r| {
                for (i, c) in catalog::CATEGORIES.iter().enumerate() {
                    button(r, c.label(), Btn::Category(i), i == category);
                }
            });
            p.spawn(Node { margin: UiRect::bottom(Val::Px(2.0)), ..default() }).with_children(|r| button(r, &search, Btn::Search, typing));
            p.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(4.0), row_gap: Val::Px(4.0), ..default() }).with_children(|g| {
                for (m, from) in shown_items.iter().zip(&elsewhere) {
                    let picked = placing.as_deref() == Some(m.as_str());
                    g.spawn((
                        Button,
                        Btn::Item(m.clone()),
                        Node { width: Val::Px(98.0), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, padding: UiRect::all(Val::Px(2.0)), ..default() },
                        BackgroundColor(rgba(if picked { t::ACCENT_DIM } else { t::ROW })),
                    ))
                    .with_children(|b| {
                        b.spawn((Thumb(m.clone()), ImageNode::default(), Node { width: Val::Px(92.0), height: Val::Px(76.0), ..default() }, BackgroundColor(rgba(t::HAIRLINE))));
                        b.spawn((text(catalog::label(m), 10.0, t::INK_DIM), Node { max_width: Val::Px(94.0), height: Val::Px(24.0), overflow: Overflow::clip(), ..default() }));
                        if let Some(z) = from {
                            b.spawn((text(format!("from {z}"), 9.0, t::ACCENT), Node { max_width: Val::Px(94.0), height: Val::Px(12.0), overflow: Overflow::clip(), ..default() }));
                        }
                    });
                }
            });
            p.spawn(Node { align_items: AlignItems::Center, margin: UiRect::vertical(Val::Px(4.0)), ..default() }).with_children(|r| {
                button(r, "Prev", Btn::Prev, false);
                r.spawn(text(format!("  {} / {}  ({} objects)  ", page + 1, pages, items.len()), 13.0, t::INK_DIM));
                button(r, "Next", Btn::Next, false);
            });
            p.spawn(Node { flex_wrap: FlexWrap::Wrap, ..default() }).with_children(|r| {
                button(r, "Save", Btn::Save, true);
                button(r, "Undo", Btn::Undo, false);
                button(r, "Redo", Btn::Redo, false);
                button(r, if grid { "Grid: on" } else { "Grid: off" }, Btn::Grid, grid);
                button(r, if align { "Align: on" } else { "Align: off" }, Btn::Align, align);
                button(r, "Close (F7)", Btn::Close, false);
            });
            p.spawn((Status, text("", 13.0, t::WARN), Node { margin: UiRect::vertical(Val::Px(4.0)), ..default() }));
            p.spawn(text(
                "Pick an object, click the map to place it (X stops). Click an object to select, drag to move.\n\
                 Hold right mouse: fly (WASD, Space/Ctrl, wheel speed, Shift fast).\n\
                 Q/E turn (Shift fine)  PgUp/PgDn height  +/- size  G grid  T align\n\
                 Ctrl+D copy  Del remove  Ctrl+Z/Y undo/redo  Ctrl+S save  F7 close\n\
                 Shift+drag paint  Alt+drag erase  [ ] brush  H hide map object\n\
                 Light: C colour  +/- bright  [ ] range  V shadows\n\
                 Person: wheel/Up/Down idle  Left/Right character  K gun\n\
                 / search  wheel over palette: pages\n\
                 Pad: A place/select  B drop  X remove  Y save  D-pad objects/turn",
                11.0,
                t::INK_DIM,
            ));
        });
    e.page = page;
}

#[allow(clippy::too_many_arguments)]
fn buttons(
    mut commands: Commands,
    mut e: ResMut<Editor>,
    mut layout: ResMut<Layout>,
    pressed: Query<(&Interaction, &Btn), Changed<Interaction>>,
    mut fly: ResMut<crate::flycam::Flycam>,
    content: Res<Content>,
    panels: Query<Entity, With<Panel>>,
    time: Res<Time<Real>>,
) {
    if !e.open {
        return;
    }
    let now = time.elapsed_secs();
    for (i, b) in &pressed {
        if *i != Interaction::Pressed {
            continue;
        }
        match b {
            Btn::Category(c) => {
                e.category = *c;
                e.page = 0;
                e.dirty = true;
            }
            Btn::Item(m) => {
                e.placing = if e.placing.as_deref() == Some(m.as_str()) { None } else { Some(m.clone()) };
                e.selected = None;
                e.dirty = true;
            }
            Btn::Prev => {
                e.page = e.page.saturating_sub(1);
                e.dirty = true;
            }
            Btn::Next => {
                e.page += 1;
                e.dirty = true;
            }
            Btn::Save => save(&mut e, &layout, now),
            Btn::Undo => undo(&mut e, &mut layout, true, now),
            Btn::Redo => undo(&mut e, &mut layout, false, now),
            Btn::Grid => {
                e.grid = !e.grid;
                e.dirty = true;
            }
            Btn::Align => {
                e.align = !e.align;
                e.dirty = true;
            }
            Btn::Close => set_open(&mut commands, &mut e, false, &mut fly, &content, &panels, now),
            Btn::Search => {
                e.typing = !e.typing;
                e.dirty = true;
            }
        }
    }
}

/// The palette's search being typed (/ starts it; Enter, Esc or a click
/// on it ends it, Esc clearing it), and the wheel over the palette turning
/// its pages.
fn search(
    mut e: ResMut<Editor>,
    keys: Res<ButtonInput<KeyCode>>,
    mut typed: MessageReader<bevy::input::keyboard::KeyboardInput>,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    if !e.open {
        typed.clear();
        return;
    }
    if window.cursor_position().is_some_and(|c| c.x < PANEL_WIDTH) && scroll.delta.y.abs() > 0.0 {
        e.page = if scroll.delta.y < 0.0 { e.page + 1 } else { e.page.saturating_sub(1) };
        e.dirty = true;
    }
    if !e.typing {
        typed.clear();
        if keys.just_pressed(KeyCode::Slash) {
            e.typing = true;
            e.dirty = true;
        }
        return;
    }
    use bevy::input::keyboard::Key;
    let before = (e.query.clone(), e.typing);
    for k in typed.read() {
        if !k.state.is_pressed() {
            continue;
        }
        match &k.logical_key {
            Key::Enter => e.typing = false,
            Key::Escape => {
                e.typing = false;
                e.query.clear();
            }
            Key::Backspace => {
                e.query.pop();
            }
            Key::Space => e.query.push(' '),
            Key::Character(c) if e.query.len() < 40 => {
                let c: String = c.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
                e.query.push_str(&c.to_ascii_lowercase());
            }
            _ => {}
        }
    }
    if e.query != before.0 {
        e.page = 0;
    }
    if (e.query.as_str(), e.typing) != (before.0.as_str(), before.1) {
        e.dirty = true;
    }
}

fn save(e: &mut Editor, layout: &Layout, now: f32) {
    let map = layout::key(&crate::atmos::climate::map_name());
    let msg = layout::save(&map, &layout.objects);
    if msg.starts_with("Saved") {
        e.unsaved = false;
    }
    e.say(msg, now);
}

fn undo(e: &mut Editor, layout: &mut Layout, back: bool, now: f32) {
    let (from, to) = if back { (&mut e.undo, &mut e.redo) } else { (&mut e.redo, &mut e.undo) };
    let Some(state) = from.pop() else {
        e.say(if back { "Nothing to undo" } else { "Nothing to redo" }, now);
        return;
    };
    to.push(std::mem::replace(&mut layout.objects, state));
    e.selected = None;
    e.respawn = true;
    e.changed = true;
    e.unsaved = true;
}

fn status(e: Res<Editor>, layout: Res<Layout>, mut texts: Query<&mut Text, With<Status>>, time: Res<Time<Real>>, streamer: Res<stream::Streamer>) {
    if !e.open {
        return;
    }
    let Ok(mut t) = texts.single_mut() else { return };
    let now = time.elapsed_secs();
    let mut s = String::new();
    if let Some((m, until)) = &e.message
        && now < *until
    {
        s.push_str(m);
        s.push('\n');
    }
    if let Some(spec) = e.placing.as_deref().and_then(|m| e.actor_spec(m)) {
        s.push_str(&format!("Placing {} (turn {:.0})\n", spec.label(), e.place_yaw.to_degrees()));
    } else if let Some(m) = &e.placing {
        s.push_str(&format!("Placing {} (turn {:.0}, size {:.2}, brush {:.1} m)\n", catalog::label(m), e.place_yaw.to_degrees(), e.place_scale, e.brush));
    } else if let Some(o) = e.selected.and_then(|i| layout.objects.get(i)) {
        match actors::Spec::parse(&o.model) {
            Some(spec) => s.push_str(&format!("Selected {}\n", spec.label())),
            None => s.push_str(&format!("Selected {} (size {:.2})\n", catalog::label(&o.model), o.scale)),
        }
    }
    if let Some(z) = streamer.loading_zone() {
        s.push_str(&format!("Loading {z}...\n"));
    } else if streamer.low_memory && streamer.busy() {
        s.push_str("Waiting for free memory to load another level\n");
    }
    s.push_str(&format!("{} objects placed{}", layout.objects.len(), if e.unsaved { " - not saved" } else { "" }));
    if t.0 != s {
        t.0 = s;
    }
}

// --- editing

/// The pointer's ray into the world (player 1's camera), unless it's over
/// the palette.
fn pointer_ray(window: &Window, camera: &(&GlobalTransform, &Projection)) -> Option<(Vec3, Dir3, Vec2)> {
    let cursor = if pad_mode() { window.size() * 0.5 } else { window.cursor_position()? };
    if cursor.x < PANEL_WIDTH && !pad_mode() {
        return None;
    }
    let size = window.size();
    let Projection::Perspective(p) = camera.1 else { return None };
    let ndc = Vec2::new(cursor.x / size.x * 2.0 - 1.0, 1.0 - cursor.y / size.y * 2.0);
    let tan = (p.fov * 0.5).tan();
    let view = Vec3::new(ndc.x * tan * size.x / size.y.max(1.0), ndc.y * tan, -1.0);
    let dir = Dir3::new(camera.0.rotation() * view).ok()?;
    Some((camera.0.translation(), dir, cursor))
}

/// Where an object stands on the surface hit at `point` (normal `normal`).
fn stand(e: &Editor, p: &Prepared, point: Vec3, normal: Vec3, yaw: Quat, scale: f32) -> (Vec3, Quat) {
    let up = if e.align && normal.y > 0.3 { normal } else { Vec3::Y };
    let rot = Quat::from_rotation_arc(Vec3::Y, up) * yaw;
    (point - up * p.min.y * scale, rot)
}

/// The surface under the pointer (the world, the shot-only props and the
/// placed objects but `skip`), snapped to the grid if it's on.
fn surface(spatial: &SpatialQuery, e: &Editor, from: Vec3, dir: Dir3, skip: &[Entity]) -> Option<(Vec3, Vec3)> {
    let filter = SpatialQueryFilter::from_mask([Layer::World, Layer::ShotClip]).with_excluded_entities(skip.iter().copied());
    let hit = spatial.cast_ray(from, dir, 1000.0, true, &filter)?;
    let mut point = from + *dir * hit.distance;
    let mut normal = hit.normal;
    if e.grid {
        let snapped = Vec3::new((point.x / GRID).round() * GRID, point.y, (point.z / GRID).round() * GRID);
        if let Some(h) = spatial.cast_ray(snapped + Vec3::Y * 1.0, Dir3::NEG_Y, 3.0, true, &filter) {
            point = snapped + Vec3::Y * (1.0 - h.distance);
            normal = h.normal;
        }
    }
    Some((point, normal))
}

/// The nearest placed object along the ray (its box), and how far.
fn pick(objects: &Query<(Entity, &PlacedObject, &GlobalTransform, &ObjectBounds)>, from: Vec3, dir: Dir3) -> Option<(usize, f32)> {
    objects
        .iter()
        .filter_map(|(_, o, gt, b)| {
            let inv = gt.affine().inverse();
            let (lf, ld) = (inv.transform_point3(from), inv.transform_vector3(*dir));
            // Slab test in the model's own space.
            let (mut near, mut far) = (0.0f32, f32::MAX);
            for a in 0..3 {
                let (o, d, lo, hi) = (lf[a], ld[a], b.min[a], b.max[a]);
                if d.abs() < 1e-8 {
                    if o < lo || o > hi {
                        return None;
                    }
                    continue;
                }
                let (t1, t2) = ((lo - o) / d, (hi - o) / d);
                near = near.max(t1.min(t2));
                far = far.min(t1.max(t2));
            }
            (near <= far).then(|| {
                // Back to world distance.
                let at = gt.transform_point(lf + ld * near);
                (o.0, at.distance(from))
            })
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

#[allow(clippy::too_many_arguments)]
fn edit(
    mut commands: Commands,
    mut e: ResMut<Editor>,
    mut layout: ResMut<Layout>,
    mut models: Models,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Query<(&GlobalTransform, &Projection), With<crate::player::MainCamera>>,
    spatial: SpatialQuery,
    objects: Query<(Entity, &PlacedObject, &GlobalTransform, &ObjectBounds)>,
    // (Their colliders follow: `collision::sync_moved_colliders`.)
    mut moving: Query<(&PlacedObject, &mut Transform)>,
    time: Res<Time<Real>>,
    pads: Query<&Gamepad>,
    motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
) {
    if !e.open || e.typing {
        return;
    }
    let now = time.elapsed_secs();
    // Pad or mouse: whichever moved last has the pointer.
    let pad = pad_press(&pads);
    let sticks = pads.iter().any(|g| {
        [GamepadAxis::LeftStickX, GamepadAxis::LeftStickY, GamepadAxis::RightStickX, GamepadAxis::RightStickY]
            .iter()
            .any(|a| g.get(*a).is_some_and(|v| v.abs() > 0.3))
    });
    if pad.a || pad.b || pad.x || pad.y || pad.left || pad.right || pad.up || pad.down || sticks {
        PAD.store(true, std::sync::atomic::Ordering::Relaxed);
    } else if motion.delta.length() > 2.0 || mouse.get_just_pressed().next().is_some() {
        PAD.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    // A person selected (not placing): the D-pad picks their idle.
    let actor_selected = e.placing.is_none() && e.selected.and_then(|i| layout.objects.get(i)).is_some_and(|o| matches!(o.kind(), layout::Kind::Actor(_)));
    // D-pad left/right: the tab's previous or next object.
    if (pad.left || pad.right) && !actor_selected {
        let items: Vec<String> = e.filtered().into_iter().cloned().collect();
        if !items.is_empty() {
            let at = e.placing.as_ref().and_then(|m| items.iter().position(|i| i == m));
            let n = items.len();
            let next = match at {
                Some(i) if pad.right => (i + 1) % n,
                Some(i) => (i + n - 1) % n,
                None => 0,
            };
            e.placing = Some(items[next].clone());
            e.selected = None;
            e.page = next / (COLUMNS * ROWS);
            e.dirty = true;
        }
    }
    let flying = mouse.pressed(MouseButton::Right);
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let Ok(cam) = camera.single() else { return };
    let ray = pointer_ray(&window, &cam);
    let placed: Vec<Entity> = objects.iter().map(|o| o.0).collect();

    // Keys (not while flying: WASD and Ctrl are the camera's then).
    if !flying {
        if (ctrl && keys.just_pressed(KeyCode::KeyS)) || pad.y {
            save(&mut e, &layout, now);
        }
        if ctrl && keys.just_pressed(KeyCode::KeyZ) {
            undo(&mut e, &mut layout, true, now);
        }
        if ctrl && keys.just_pressed(KeyCode::KeyY) {
            undo(&mut e, &mut layout, false, now);
        }
        if keys.just_pressed(KeyCode::KeyG) {
            e.grid = !e.grid;
            e.dirty = true;
            let s = if e.grid { "Grid snap on" } else { "Grid snap off" };
            e.say(s, now);
        }
        if keys.just_pressed(KeyCode::KeyT) {
            e.align = !e.align;
            e.dirty = true;
            let s = if e.align { "Align to slopes on" } else { "Align to slopes off" };
            e.say(s, now);
        }
        if keys.just_pressed(KeyCode::KeyX) || pad.b {
            if e.placing.take().is_some() {
                e.dirty = true;
            }
            e.selected = None;
        }
        // The map's own object under the pointer: hide it (or show it again).
        if keys.just_pressed(KeyCode::KeyH)
            && let Some((from, dir, _)) = ray
        {
            let placed_hit = pick(&objects, from, dir).map(|p| p.1);
            let hit = e
                .map_objects
                .iter()
                .enumerate()
                .filter_map(|(k, m)| ray_box(&m.tf, m.min, m.max, from, dir).map(|d| (k, d)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|h| placed_hit.is_none_or(|d| h.1 < d));
            if let Some((k, _)) = hit {
                let m = e.map_objects[k].clone();
                e.checkpoint(&layout);
                match m.hidden_in(&layout) {
                    Some(i) => {
                        layout.objects.remove(i);
                        // (The selection's index moves down with the rest.)
                        e.selected = match e.selected {
                            Some(s) if s == i => None,
                            Some(s) if s > i => Some(s - 1),
                            s => s,
                        };
                        e.say(format!("{} shown again (from the next load)", catalog::label(&m.model)), now);
                    }
                    None => {
                        layout.objects.push(Placed {
                            model: format!("@hide:{}", m.model),
                            pos: crate::units::pos(m.origin),
                            rot: Quat::IDENTITY,
                            scale: 1.0,
                            params: vec![],
                        });
                        e.say(format!("{} hidden (gone from the next load: save, then reload the map)", catalog::label(&m.model)), now);
                    }
                }
                e.respawn = true;
            }
        }
        let bracket = keys.just_pressed(KeyCode::BracketRight) as i32 as f32 - keys.just_pressed(KeyCode::BracketLeft) as i32 as f32;
        let light = e.selected.filter(|&i| layout.objects.get(i).is_some_and(|o| o.kind() == layout::Kind::Light));
        if let Some(i) = light {
            let grow = keys.any_just_pressed([KeyCode::Equal, KeyCode::NumpadAdd]) as i32 as f32
                - keys.any_just_pressed([KeyCode::Minus, KeyCode::NumpadSubtract]) as i32 as f32;
            let colour = keys.just_pressed(KeyCode::KeyC);
            let shadows = keys.just_pressed(KeyCode::KeyV);
            if grow != 0.0 || bracket != 0.0 || colour || shadows {
                e.checkpoint(&layout);
                let o = &mut layout.objects[i];
                let mut p: Vec<f32> = (0..6).map(|k| o.param(k)).collect();
                p[3] = (p[3] * 1.25f32.powf(grow)).clamp(0.05, 50.0);
                p[4] = (p[4] * 1.2f32.powf(bracket)).clamp(0.5, 60.0);
                if colour {
                    const COLOURS: [[f32; 3]; 7] = [[1.0, 0.85, 0.65], [1.0, 1.0, 1.0], [0.7, 0.85, 1.0], [1.0, 0.55, 0.2], [1.0, 0.2, 0.15], [0.3, 1.0, 0.4], [0.3, 0.45, 1.0]];
                    let now_c = [p[0], p[1], p[2]];
                    let k = COLOURS.iter().position(|c| c.iter().zip(&now_c).all(|(a, b)| (a - b).abs() < 0.01)).map_or(0, |k| (k + 1) % COLOURS.len());
                    p[..3].copy_from_slice(&COLOURS[k]);
                }
                if shadows {
                    p[5] = if p[5] > 0.5 { 0.0 } else { 1.0 };
                }
                let msg = format!("Light: brightness {:.2}, range {:.1} m, shadows {}", p[3], p[4], if p[5] > 0.5 { "on" } else { "off" });
                o.params = p;
                e.respawn = true;
                e.say(msg, now);
            }
        } else if bracket != 0.0 {
            e.brush = (e.brush * 1.25f32.powf(bracket)).clamp(0.25, 20.0);
            let msg = format!("Brush {:.1} m", e.brush);
            e.say(msg, now);
        }
        // A person (placing or selected): the idle (wheel, Up/Down, D-pad),
        // the character (Left/Right), the gun (K, Shift+K back).
        let over_panel = !pad_mode() && window.cursor_position().is_some_and(|c| c.x < PANEL_WIDTH);
        let wheel = if over_panel { 0.0 } else { scroll.delta.y };
        let idle_step = keys.just_pressed(KeyCode::ArrowDown) as i32 - keys.just_pressed(KeyCode::ArrowUp) as i32
            + (wheel < 0.0) as i32
            - (wheel > 0.0) as i32
            + if actor_selected { pad.right as i32 - pad.left as i32 } else { 0 };
        let who_step = keys.just_pressed(KeyCode::ArrowRight) as i32 - keys.just_pressed(KeyCode::ArrowLeft) as i32;
        let gun_step = if keys.just_pressed(KeyCode::KeyK) { if shift { -1 } else { 1 } } else { 0 };
        if idle_step != 0 || who_step != 0 || gun_step != 0 {
            let adjust = |spec: &mut actors::Spec| {
                spec.step_preset(idle_step.signum());
                spec.step_character(who_step.signum());
                if gun_step != 0 {
                    spec.step_gun(gun_step);
                }
            };
            if let Some(picked) = e.placing.clone().filter(|m| m.starts_with(actors::PREFIX))
                && let Some(mut spec) = e.actor_spec(&picked)
            {
                adjust(&mut spec);
                e.actor_preset = spec.preset.clone();
                e.actor_gun = spec.gun.clone();
                if who_step != 0 {
                    e.placing = Some(format!("{}{}", actors::PREFIX, spec.character));
                    e.dirty = true;
                }
                e.say(format!("Next: {}", spec.label()), now);
            } else if actor_selected
                && let Some(i) = e.selected
                && let Some(mut spec) = actors::Spec::parse(&layout.objects[i].model)
            {
                adjust(&mut spec);
                e.checkpoint(&layout);
                layout.objects[i].model = spec.token();
                e.actor_preset = spec.preset.clone();
                e.actor_gun = spec.gun.clone();
                e.respawn_only.push(i);
                e.say(spec.label(), now);
            }
        }
        let step = if shift { 1f32 } else { 15.0 }.to_radians();
        let turn = (keys.just_pressed(KeyCode::KeyE) || pad.up) as i32 as f32 - (keys.just_pressed(KeyCode::KeyQ) || pad.down) as i32 as f32;
        let lift = keys.just_pressed(KeyCode::PageUp) as i32 as f32 - keys.just_pressed(KeyCode::PageDown) as i32 as f32;
        let grow = keys.any_just_pressed([KeyCode::Equal, KeyCode::NumpadAdd]) as i32 as f32
            - keys.any_just_pressed([KeyCode::Minus, KeyCode::NumpadSubtract]) as i32 as f32;
        if e.placing.is_some() {
            e.place_yaw += turn * step;
            e.place_scale = (e.place_scale * 1.1f32.powf(grow)).clamp(0.1, 10.0);
        } else if let Some(i) = e.selected.filter(|&i| i < layout.objects.len()) {
            let grow = if light.is_some() { 0.0 } else { grow };
            if turn != 0.0 || lift != 0.0 || grow != 0.0 {
                e.checkpoint(&layout);
                let o = &mut layout.objects[i];
                o.rot = (Quat::from_rotation_y(turn * step) * o.rot).normalize();
                o.pos.y += lift * if shift { u(1.0) } else { u(4.0) };
                o.scale = (o.scale * 1.1f32.powf(grow)).clamp(0.1, 10.0);
                e.respawn = true;
            }
            if keys.just_pressed(KeyCode::Delete) || pad.x {
                e.checkpoint(&layout);
                let gone = layout.objects.remove(i);
                e.selected = None;
                e.respawn = true;
                e.say(format!("Removed {}", catalog::label(&gone.model)), now);
            }
            // (Not both in a frame: the index is gone once removed.)
            else if ctrl && keys.just_pressed(KeyCode::KeyD) {
                e.checkpoint(&layout);
                let mut copy = layout.objects[i].clone();
                copy.pos += Vec3::new(0.5, 0.0, 0.5);
                layout.objects.push(copy);
                e.selected = Some(layout.objects.len() - 1);
                e.respawn = true;
            }
        }
    }

    // The ghost of what's being placed, under the pointer.
    let ghost_at = match (&e.placing, ray) {
        (Some(m), Some((from, dir, _))) if !flying => {
            let m = m.clone();
            models.get(&m).zip(surface(&spatial, &e, from, dir, &[])).map(|(p, (point, normal))| {
                let (pos, rot) = stand(&e, &p, point, normal, Quat::from_rotation_y(e.place_yaw), e.place_scale);
                (m, p, pos, rot)
            })
        }
        _ => None,
    };
    match &ghost_at {
        Some((m, p, pos, rot)) => {
            let tf = Transform::from_translation(*pos).with_rotation(*rot).with_scale(Vec3::splat(e.place_scale));
            if e.ghost.as_ref().is_none_or(|(g, _)| g != m) {
                if let Some((_, g)) = e.ghost.take() {
                    commands.entity(g).try_despawn();
                }
                let g = commands.spawn((Name::new("map editor ghost"), tf, Visibility::default())).id();
                for (material, mesh) in &p.parts {
                    commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), bevy::light::NotShadowCaster, ChildOf(g)));
                }
                e.ghost = Some((m.clone(), g));
            } else if let Some((_, g)) = &e.ghost {
                commands.entity(*g).insert(tf);
            }
        }
        None => {
            if let Some((_, g)) = e.ghost.take() {
                commands.entity(g).try_despawn();
            }
        }
    }

    let Some((from, dir, cursor)) = ray else {
        if mouse.just_released(MouseButton::Left) {
            e.drag = None;
        }
        return;
    };
    if flying {
        return;
    }
    // Click: place, or select (and start dragging).
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    // Painting (Shift) and rubbing out (Alt) take the button.
    if (shift || alt) && mouse.pressed(MouseButton::Left) {
        if now >= e.next_dab {
            e.next_dab = now + 0.06;
            paint(&mut e, &mut layout, &mut models, &spatial, from, dir, alt, mouse.just_pressed(MouseButton::Left), now);
        }
        return;
    }
    if mouse.just_pressed(MouseButton::Left) || pad.a {
        if let Some((m, _, pos, rot)) = ghost_at {
            e.checkpoint(&layout);
            let scale = e.place_scale;
            let params = if m == layout::LIGHT { layout::LIGHT_DEFAULTS.to_vec() } else { vec![] };
            // (A person: as the next one's set to be.)
            let model = e.actor_spec(&m).map_or_else(|| m.clone(), |s| s.token());
            layout.objects.push(Placed { model, pos, rot, scale, params });
            e.respawn_from = Some(e.respawn_from.unwrap_or(layout.objects.len() - 1).min(layout.objects.len() - 1));
            e.say(format!("Placed {}", catalog::label(&m)), now);
        } else {
            e.selected = pick(&objects, from, dir).map(|(i, _)| i);
            e.drag = e.selected.filter(|_| !pad.a).map(|_| (cursor, false));
        }
    }
    if mouse.just_released(MouseButton::Left) {
        e.drag = None;
    }
    // Dragging: along whatever's under the pointer.
    if let (Some((start, moved)), Some(i)) = (e.drag, e.selected)
        && mouse.pressed(MouseButton::Left)
        && i < layout.objects.len()
    {
        if !moved && cursor.distance(start) < 5.0 {
            return;
        }
        if !moved {
            e.checkpoint(&layout);
            e.drag = Some((start, true));
        }
        let me: Vec<Entity> = objects.iter().filter(|o| o.1.0 == i).map(|o| o.0).collect();
        let _ = &placed;
        let Some(p) = models.get(&layout.objects[i].model) else { return };
        let Some((point, normal)) = surface(&spatial, &e, from, dir, &me) else { return };
        let o = &mut layout.objects[i];
        let up = if e.align && normal.y > 0.3 { normal } else { Vec3::Y };
        o.pos = point - up * p.min.y * o.scale;
        for (po, mut tf) in &mut moving {
            if po.0 == i {
                tf.translation = o.pos;
            }
        }
    }
}

/// One dab of the brush at the pointer: an object dropped somewhere in it
/// (clear of others like it), or, rubbing out, the placed objects in it gone.
#[allow(clippy::too_many_arguments)]
fn paint(e: &mut Editor, layout: &mut Layout, models: &mut Models, spatial: &SpatialQuery, from: Vec3, dir: Dir3, erase: bool, first: bool, now: f32) {
    let Some((centre, _)) = surface(spatial, e, from, dir, &[]) else { return };
    if erase {
        let gone: Vec<usize> = layout
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| !matches!(o.kind(), layout::Kind::Hide(_)) && o.pos.distance(centre) < e.brush)
            .map(|(i, _)| i)
            .collect();
        if gone.is_empty() {
            return;
        }
        e.checkpoint(layout);
        for i in gone.into_iter().rev() {
            layout.objects.remove(i);
        }
        e.selected = None;
        e.respawn = true;
        return;
    }
    let Some(m) = e.placing.clone().filter(|m| !m.starts_with('@')) else {
        if first {
            e.say("Pick an object to paint with", now);
        }
        return;
    };
    let Some(p) = models.get(&m) else { return };
    // Spacing: about the object's own footprint.
    let footprint = ((p.max.x - p.min.x).max(p.max.z - p.min.z) * e.place_scale).max(0.15);
    let (a, r) = (rand::random::<f32>() * std::f32::consts::TAU, e.brush * rand::random::<f32>().sqrt());
    let at = centre + Vec3::new(a.cos() * r, 0.0, a.sin() * r);
    let filter = SpatialQueryFilter::from_mask([Layer::World, Layer::ShotClip]);
    // (From a little above the pointer's surface: onto that floor, not a
    // roof or awning over it.)
    let Some(h) = spatial.cast_ray(at + Vec3::Y * 0.6, Dir3::NEG_Y, 3.0, true, &filter) else { return };
    let point = at + Vec3::Y * (0.6 - h.distance);
    if layout.objects.iter().any(|o| o.model == m && o.pos.distance(point) < footprint * 0.8) {
        return;
    }
    if first || e.respawn_from.is_none() {
        e.checkpoint(layout);
    }
    let scale = e.place_scale * (0.85 + 0.3 * rand::random::<f32>());
    let yaw = Quat::from_rotation_y(rand::random::<f32>() * std::f32::consts::TAU);
    // (Plants follow the slope; the rest stand up.)
    let up = if catalog::category(&m) == catalog::Category::Plants && h.normal.y > 0.3 { h.normal } else { Vec3::Y };
    let rot = Quat::from_rotation_arc(Vec3::Y, up) * yaw;
    layout.objects.push(Placed { model: m, pos: point - up * p.min.y * scale, rot, scale, params: vec![] });
    let new = layout.objects.len() - 1;
    e.respawn_from = Some(e.respawn_from.unwrap_or(new).min(new));
}

/// Spawns the placed objects anew after a change.
fn respawn(mut commands: Commands, mut e: ResMut<Editor>, layout: Res<Layout>, mut models: Models, objects: Query<(Entity, &PlacedObject)>) {
    // Just these (a person's idle changed).
    if !e.respawn && !e.respawn_only.is_empty() {
        let only = std::mem::take(&mut e.respawn_only);
        let Some(root) = e.root else { return };
        for (ent, o) in &objects {
            if only.contains(&o.0) {
                commands.entity(ent).try_despawn();
            }
        }
        for &i in &only {
            if let Some(o) = layout.objects.get(i)
                && !spawn_one(&mut commands, &mut models, o, i, root)
            {
                e.missing = true;
            }
        }
    }
    e.respawn_only.clear();
    let from = match (e.respawn, e.respawn_from) {
        (true, _) => 0,
        (false, Some(f)) => f,
        _ => return,
    };
    if e.respawn {
        for (o, _) in &objects {
            commands.entity(o).try_despawn();
        }
        e.missing = false;
    }
    e.respawn = false;
    e.respawn_from = None;
    let root = match e.root {
        Some(r) => r,
        None => {
            let r = commands.spawn((Name::new("placed objects"), Transform::default(), Visibility::default())).id();
            e.root = Some(r);
            r
        }
    };
    for (i, o) in layout.objects.iter().enumerate().skip(from) {
        if !spawn_one(&mut commands, &mut models, o, i, root) {
            e.missing = true;
        }
    }
    e.tried_at = models.streamer.generation;
}

/// Placed objects waiting for their levels: tried again as each level
/// comes in (the bots' routes built anew round them, in a match).
fn retry(mut e: ResMut<Editor>, streamer: Res<stream::Streamer>) {
    if !e.missing || streamer.generation == e.tried_at {
        return;
    }
    e.tried_at = streamer.generation;
    e.respawn = true;
    if !e.open && !crate::hq::active() {
        crate::bots::request_nav_rebuild();
    }
}

/// Placed effects start as they appear (and stop as their entity goes).
fn start_fx(fx: Query<(Entity, &layout::PlacedFx), Added<layout::PlacedFx>>, effects: Option<ResMut<crate::fx::Effects>>) {
    let Some(mut effects) = effects else { return };
    for (e, f) in &fx {
        effects.play_ambient_bolted(&f.0, e);
    }
}

/// The placed spawn points join the map's own (as it loads, and as the
/// editor closes after changes).
fn apply_spawns(mut e: ResMut<Editor>, layout: Res<Layout>, info: Option<ResMut<crate::world::MapInfo>>) {
    if !e.spawns_dirty {
        return;
    }
    let Some(mut info) = info else { return };
    e.spawns_dirty = false;
    let base = *e.base_spawns.get_or_insert(info.spawns.len());
    info.spawns.truncate(base);
    for o in &layout.objects {
        let layout::Kind::Spawn(k) = o.kind() else { continue };
        let kinds: &[crate::world::SpawnKind] = match k {
            "allies" => &[crate::world::SpawnKind::AlliesStart, crate::world::SpawnKind::Tdm],
            "axis" => &[crate::world::SpawnKind::AxisStart, crate::world::SpawnKind::Tdm],
            "dm" => &[crate::world::SpawnKind::Dm],
            _ => &[crate::world::SpawnKind::Tdm, crate::world::SpawnKind::Dm],
        };
        let (yaw, _, _) = o.rot.to_euler(EulerRot::YXZ);
        for &kind in kinds {
            info.spawns.push(crate::world::SpawnPoint { pos: o.pos, yaw, kind });
        }
    }
    let added = info.spawns.len() - base;
    if added > 0 {
        info!("map editor: {added} placed spawn points");
    }
}

/// The placed objects' boxes (the selection pulsing), lights, effects and
/// spawn points, the map's hidden models, and the brush.
#[allow(clippy::too_many_arguments)]
fn draw(
    e: Res<Editor>,
    layout: Res<Layout>,
    objects: Query<(&PlacedObject, &GlobalTransform, &ObjectBounds)>,
    mut gizmos: Gizmos,
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Query<(&GlobalTransform, &Projection), With<crate::player::MainCamera>>,
    spatial: SpatialQuery,
    ghosts: Query<&GlobalTransform>,
) {
    if !e.open {
        return;
    }
    // A light's, effect's or spawn point's ghost has nothing to draw: its box.
    if let Some((m, g)) = &e.ghost
        && m.starts_with('@')
        && !m.starts_with(actors::PREFIX)
        && let Ok(gt) = ghosts.get(*g)
    {
        let probe = Placed { model: m.clone(), pos: Vec3::ZERO, rot: Quat::IDENTITY, scale: 1.0, params: vec![] };
        let (min, max) = layout::marker_bounds(probe.kind());
        let tf = gt.compute_transform();
        gizmos.cube(Transform::from_translation(tf.transform_point((min + max) * 0.5)).with_rotation(tf.rotation).with_scale(max - min), Color::srgb(0.3, 1.0, 0.4));
    }
    let pulse = 0.75 + 0.25 * (time.elapsed_secs() * 5.0).sin();
    // The map's own models hidden: red.
    for m in &e.map_objects {
        if m.hidden_in(&layout).is_some() {
            let centre = m.tf.transform_point((m.min + m.max) * 0.5);
            gizmos.cube(Transform::from_translation(centre).with_rotation(m.tf.rotation).with_scale(((m.max - m.min) * m.tf.scale).max(Vec3::splat(0.01))), Color::srgb(1.0, 0.15, 0.1));
        }
    }
    // The pad's pointer: a ring where the middle of the screen meets the map.
    if pad_mode()
        && let Ok(cam) = camera.single()
        && let Some((from, dir, _)) = pointer_ray(&window, &cam)
        && let Some((point, normal)) = surface(&spatial, &e, from, dir, &[])
    {
        gizmos.circle(Isometry3d::new(point + normal * 0.03, Quat::from_rotation_arc(Vec3::Z, normal)), 0.15, Color::WHITE);
    }
    // The brush, while painting or rubbing out.
    let painting = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight, KeyCode::AltLeft, KeyCode::AltRight]);
    if painting
        && let Ok(cam) = camera.single()
        && let Some((from, dir, _)) = pointer_ray(&window, &cam)
        && let Some((point, normal)) = surface(&spatial, &e, from, dir, &[])
    {
        let erase = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
        let colour = if erase { Color::srgb(1.0, 0.3, 0.2) } else { Color::srgb(0.3, 1.0, 0.4) };
        let iso = Isometry3d::new(point + normal * 0.05, Quat::from_rotation_arc(Vec3::Z, normal));
        gizmos.circle(iso, e.brush, colour);
    }
    for (o, gt, b) in &objects {
        let selected = e.selected == Some(o.0);
        if let Some(placed) = layout.objects.get(o.0) {
            let at = gt.translation();
            match placed.kind() {
                layout::Kind::Light => {
                    let c = Color::srgb(placed.param(0), placed.param(1), placed.param(2));
                    gizmos.sphere(Isometry3d::from_translation(at), 0.2, c);
                    if selected {
                        gizmos.sphere(Isometry3d::from_translation(at), placed.param(4), c.with_alpha(0.25));
                    }
                }
                layout::Kind::Fx(_) => {
                    gizmos.cross(Isometry3d::from_translation(at), 0.3, Color::srgb(1.0, 0.6, 0.1));
                }
                layout::Kind::Spawn(k) => {
                    let c = match k {
                        "allies" => Color::srgb(0.3, 0.5, 1.0),
                        "axis" => Color::srgb(1.0, 0.3, 0.3),
                        _ => Color::srgb(0.9, 0.9, 0.9),
                    };
                    let facing = gt.rotation() * Vec3::NEG_Z;
                    gizmos.arrow(at + Vec3::Y * 0.1, at + Vec3::Y * 0.1 + facing * 0.8, c);
                }
                _ => {}
            }
        }
        let colour = if selected { Color::srgb(1.0, 0.75 * pulse, 0.2) } else { Color::srgba(0.4, 0.8, 1.0, 0.35) };
        let tf = gt.compute_transform();
        let centre = tf.transform_point((b.min + b.max) * 0.5);
        let size = (b.max - b.min) * tf.scale;
        gizmos.cube(Transform::from_translation(centre).with_rotation(tf.rotation).with_scale(size.max(Vec3::splat(0.01))), colour);
    }
    let _ = &layout;
}

fn make_thumbs(mut commands: Commands, e: Res<Editor>, mut thumbs: ResMut<thumbs::Thumbs>, mut models: Models) {
    if !e.open {
        return;
    }
    for (slot, name) in thumbs::step(&mut commands, &mut thumbs) {
        match models.for_thumb(&name) {
            ForThumb::Ready(p) => thumbs::start(&mut commands, &mut thumbs, &mut models.images, slot, name, p),
            // (Its level's on the way: asked again later.)
            ForThumb::Wait => thumbs.later(name),
        }
    }
}

// --- test

/// `COD4RW_MAPEDIT=<dir>`: a few seconds in, opens the editor, places a few
/// objects ahead of the player, a vehicle (`COD4RW_MAPEDIT_VEHICLE`, the
/// Humvee by default: another level's on most maps) and three people in
/// different idles, waits for their levels to load, saves (into
/// `COD4RW_MAPLAYOUT_DIR` if set), closes, checks the placed objects stop
/// rays (players, bullets), with screenshots in `<dir>` (`edit.png` open,
/// `people.png` the people near, `play.png` closed), and quits.
#[allow(clippy::too_many_arguments)]
fn test(
    mut commands: Commands,
    mut e: ResMut<Editor>,
    mut layout: ResMut<Layout>,
    mut models: Models,
    mut fly: ResMut<crate::flycam::Flycam>,
    panels: Query<Entity, With<Panel>>,
    spatial: SpatialQuery,
    me: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    placed: Query<(), With<PlacedObject>>,
    mut stage: Local<u32>,
    mut t0: Local<Option<f32>>,
    mut ready: Local<f32>,
    mut close: Local<Option<(Vec3, f32, f32)>>,
    thumbs: Res<thumbs::Thumbs>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(dir) = std::env::var_os("COD4RW_MAPEDIT").map(std::path::PathBuf::from) else { return };
    let shot = |commands: &mut Commands, name: &str| {
        std::fs::create_dir_all(&dir).ok();
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(dir.join(name)));
    };
    let now = models.now();
    let start = *t0.get_or_insert(now);
    let at = now - start;
    let Ok(cam) = me.single() else { return };
    let fwd = (cam.forward().as_vec3() * Vec3::new(1.0, 0.0, 1.0)).normalize_or_zero();
    let right = fwd.cross(Vec3::Y);
    let floor_at = |over: Vec3| {
        let filter = SpatialQueryFilter::from_mask([Layer::World]);
        spatial.cast_ray(over + Vec3::Y, Dir3::NEG_Y, 5.0, true, &filter).map(|h| (over + Vec3::Y - Vec3::Y * h.distance, h.normal))
    };
    // Facing the camera (CoD models face +X).
    let facing_me = Quat::from_rotation_y((fwd.z).atan2(-fwd.x));
    match *stage {
        0 if at > 8.0 => {
            let models_list = catalog::models(&models.content);
            set_open(&mut commands, &mut e, true, &mut fly, &models.content, &panels, now);
            // Three props ahead, on the floor.
            let pickable: Vec<String> = models_list
                .iter()
                .filter(|m| e.loaded.contains(&m.to_ascii_lowercase()) && matches!(catalog::category(m), catalog::Category::Props | catalog::Category::Military))
                .take(3)
                .cloned()
                .collect();
            e.checkpoint(&layout);
            for (k, m) in pickable.iter().enumerate() {
                let Some(p) = models.get(m) else { continue };
                let Some((point, normal)) = floor_at(cam.translation() + fwd * 3.0 + right * (k as f32 - 1.0) * 1.5) else { continue };
                let (pos, rot) = stand(&e, &p, point, normal, Quat::from_rotation_y(k as f32 * 0.6), 1.0);
                layout.objects.push(Placed { model: m.clone(), pos, rot, scale: 1.0, params: vec![] });
            }
            // A light, an effect and a spawn point ahead, a dab of paint, and
            // the nearest of the map's own models ahead hidden.
            if let Some((floor, _)) = floor_at(cam.translation() + fwd * 5.0) {
                layout.objects.push(Placed { model: layout::LIGHT.into(), pos: floor + Vec3::Y * 1.5, rot: Quat::IDENTITY, scale: 1.0, params: vec![1.0, 0.3, 0.1, 3.0, 6.0, 0.0] });
                if let Some(fx) = e.models.iter().find(|m| m.starts_with("@fx:") && m.contains("fire")).cloned() {
                    layout.objects.push(Placed { model: fx, pos: floor + right * 1.5, rot: Quat::IDENTITY, scale: 1.0, params: vec![] });
                }
                layout.objects.push(Placed { model: "@spawn:allies".into(), pos: floor - right * 1.5, rot: Quat::IDENTITY, scale: 1.0, params: vec![] });
            }
            if let Some(plant) = e.models.iter().find(|m| e.loaded.contains(&m.to_ascii_lowercase()) && catalog::category(m) == catalog::Category::Plants).cloned() {
                e.placing = Some(plant);
                let down = Dir3::new(fwd * 4.0 - Vec3::Y * 1.2).unwrap_or(Dir3::NEG_Y);
                for k in 0..30 {
                    paint(&mut e, &mut layout, &mut models, &spatial, cam.translation(), down, false, k == 0, now);
                }
                e.placing = None;
            }
            let near = e
                .map_objects
                .iter()
                .filter(|m| m.tf.translation.distance(cam.translation()) < 25.0)
                .min_by(|a, b| a.tf.translation.distance(cam.translation()).total_cmp(&b.tf.translation.distance(cam.translation())))
                .cloned();
            if let Some(m) = near {
                info!("map editor test: hiding {} at {:?}", m.model, m.origin);
                layout.objects.push(Placed { model: format!("@hide:{}", m.model), pos: crate::units::pos(m.origin), rot: Quat::IDENTITY, scale: 1.0, params: vec![] });
            }
            // A vehicle off to the left, and three people, each idling his own way.
            let vehicle = std::env::var("COD4RW_MAPEDIT_VEHICLE").unwrap_or_else(|_| "vehicle_humvee_camo".into());
            if let Some((floor, _)) = floor_at(cam.translation() + fwd * 9.0 - right * 1.5) {
                layout.objects.push(Placed { model: vehicle.clone(), pos: floor, rot: Quat::from_rotation_y(0.5), scale: 1.0, params: vec![] });
            }
            let people = [("marine_assault", "bored"), ("sas_zied", "radio"), ("gaz", "gaz")];
            for (k, (who, idle)) in people.iter().enumerate() {
                let Some((floor, _)) = floor_at(cam.translation() + fwd * 4.0 + right * (1.2 + k as f32 * 1.1)) else { continue };
                let spec = actors::Spec { character: (*who).into(), gun: "auto".into(), preset: (*idle).into() };
                layout.objects.push(Placed { model: spec.token(), pos: floor, rot: facing_me, scale: 1.0, params: vec![] });
            }
            e.respawn = true;
            info!("map editor test: placed {} ({:?}, {vehicle})", layout.objects.len(), pickable);
            *stage = 1;
        }
        // Until what's placed is all here (their levels loaded), or a minute.
        1 if at > 11.0 && ((!e.missing && !models.streamer.busy()) || at > 70.0) => {
            info!("map editor test: all in at {at:.1} s (missing {}), {} models from other levels", e.missing, models.streamer.extra_models);
            save(&mut e, &layout, now);
            e.selected = Some(0);
            // Placing a person: the ghost.
            e.placing = Some(format!("{}marine_recon", actors::PREFIX));
            e.dirty = true;
            *ready = at;
            *stage = 2;
        }
        2 if at > *ready + 2.5 => {
            info!("map editor test: camera at {:?}, panel nodes {}", cam.translation(), panels.iter().count());
            shot(&mut commands, "edit.png");
            // The people up close.
            let people: Vec<Vec3> = layout.objects.iter().filter(|o| matches!(o.kind(), layout::Kind::Actor(_))).map(|o| o.pos).collect();
            if !people.is_empty() {
                let mid = people.iter().copied().sum::<Vec3>() / people.len() as f32;
                // (From the player's side, a little nearer: they face him.)
                let eye = cam.translation().with_y(mid.y + 1.6) + fwd * 1.2;
                let look = mid + Vec3::Y * 1.0 - eye;
                *close = Some((eye, (-look.x).atan2(-look.z), (look.y / look.length()).asin()));
                // (Flown there afresh: a pose is taken only as it starts.)
                fly.exit();
            }
            e.placing = None;
            e.dirty = true;
            *stage = 8;
        }
        8 if at > *ready + 2.8 => {
            if let Some((eye, yaw, pitch)) = close.take() {
                fly.hold_to_fly = true;
                fly.enter_at(eye, yaw, pitch);
            }
            *stage = 3;
        }
        // (Once the palette's pictures are all drawn.)
        3 if at > *ready + 5.0 && (thumbs.idle() || at > *ready + 25.0) => {
            shot(&mut commands, "people.png");
            // (The rest timed from here.)
            *ready = at - 5.0;
            *stage = 4;
        }
        4 if at > *ready + 6.0 => {
            set_open(&mut commands, &mut e, false, &mut fly, &models.content, &panels, now);
            *stage = 5;
        }
        5 if at > *ready + 7.0 => {
            // A ray from the camera at each object's middle: the first thing hit
            // should be it.
            let filter = crate::collision::movement_filter();
            let bullets = crate::collision::bullet_filter();
            for o in &layout.objects {
                let target = o.pos + Vec3::Y * 0.3;
                let Ok(dir) = Dir3::new(target - cam.translation()) else { continue };
                // (From clear of the player's own hitboxes.)
                let from = cam.translation() + *dir * 0.6;
                let walk = spatial.cast_ray(from, dir, 50.0, true, &filter).map(|h| placed.contains(h.entity));
                let shot = spatial.cast_ray(from, dir, 50.0, true, &bullets).map(|h| placed.contains(h.entity));
                info!("map editor test: {} blocks movement {:?}, bullets {:?}", o.model, walk, shot);
            }
            shot(&mut commands, "play.png");
            *stage = 6;
        }
        6 if at > *ready + 9.0 => {
            exit.write(AppExit::Success);
            *stage = 7;
        }
        _ => {}
    }
}
