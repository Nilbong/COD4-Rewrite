//! Debug aid: `COD4RW_GALLERY=<dir>` renders every character a game's own
//! character scripts put together in pages of twelve, named, as PNGs in
//! `<dir>`, then exits. CoD4 by default; `COD4RW_GALLERY_GAME=blackops`
//! renders Black Ops' (read with the `t5` crate) instead.
//!
//! CoD4's multiplayer characters' scripts are in common_mp and their models
//! in the maps; each campaign level has its own scripts and models. Black
//! Ops' campaign scripts build characters from a body, a head, a hat and
//! gear, some picked at random from `xmodelalias` lists (the gallery takes
//! each list's first); its multiplayer factions are bodies and heads in four
//! maps, shown paired up. Its levels also use materials defined in other
//! levels (`,name` references, e.g. Khe Sanh's Marines are dressed by Hue
//! City), so those levels are loaded alongside.

use crate::content::Content;
use crate::models::{AnimPlayer, ModelsPlugin, Skeleton, SpawnModel, spawn_model};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use iw3::zone::{Asset, ParseOptions, Zone};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

#[derive(Clone, Copy, Default, PartialEq)]
enum Game {
    #[default]
    Cod4,
    BlackOps,
}

impl Game {
    /// Multiplayer maps holding every faction's models, then the campaign
    /// levels in story order.
    fn zones(self) -> &'static [&'static str] {
        match self {
            Game::Cod4 => &[
                "mp_killhouse",
                "mp_bloc",
                "mp_crash",
                "killhouse",
                "cargoship",
                "coup",
                "blackout",
                "bog_a",
                "hunted",
                "armada",
                "bog_b",
                "village_assault",
                "scoutsniper",
                "sniperescape",
                "village_defend",
                "ambush",
                "icbm",
                "launchfacility_a",
                "launchfacility_b",
                "jeepride",
                "airlift",
                "aftermath",
                "ac130",
                "airplane",
            ],
            Game::BlackOps => &[
                "mp_nuked",
                "mp_array",
                "mp_cracked",
                "mp_firingrange",
                "frontend",
                "cuba",
                "vorkuta",
                "pentagon",
                "flashpoint",
                "khe_sanh",
                "hue_city",
                "kowloon",
                "fullahead",
                "creek_1",
                "river",
                "wmd_sr71",
                "wmd",
                "pow",
                "int_escape",
                "rebirth",
                "underwaterbase",
                "outro",
            ],
        }
    }

    /// The gun in the characters' hands.
    fn gun(self) -> &'static str {
        match self {
            Game::Cod4 => "weapon_ak47",
            Game::BlackOps => "t5_weapon_ak47_world",
        }
    }
}

const PER_PAGE: usize = 12;
const COLUMNS: usize = 6;
/// Cell size in CoD units.
const CELL_W: f32 = 46.0;
const CELL_H: f32 = 92.0;

pub fn run(dir: PathBuf) -> AppExit {
    let game = match std::env::var("COD4RW_GALLERY_GAME").as_deref() {
        Ok("blackops" | "bo1" | "t5") => Game::BlackOps,
        _ => Game::Cod4,
    };
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window { title: "CoD4 Rewrite (character gallery)".into(), resolution: (1920, 1080).into(), ..default() }),
            ..default()
        }))
        .add_plugins(ModelsPlugin)
        .insert_resource(ClearColor(Color::srgb(0.32, 0.33, 0.35)))
        .insert_resource(Gallery { dir, game, ..default() })
        .add_systems(Startup, setup)
        .add_systems(Update, step)
        .run()
}

/// One character: its body, then what is attached (head, hat, gear).
#[derive(Clone)]
struct Character {
    /// `sp_sas_woodland_price`, `c_usa_cubrebel_bowman`.
    name: String,
    models: Vec<String>,
    /// Black Ops: other zones defining materials it uses.
    also: Vec<&'static str>,
}

/// CoD4: `setModel("body")` and `attach("head", "", true)`.
fn parse_script(name: &str, text: &str) -> Option<Character> {
    let quoted = |after: &str| -> Option<String> {
        let at = text.find(after)? + after.len();
        Some(text[at..].split('"').next()?.to_owned())
    };
    let body = quoted("setModel(\"")?;
    let head = text.lines().find(|l| l.contains("attach(\"") && l.contains("\"\", true")).and_then(|l| {
        let at = l.find("attach(\"")? + "attach(\"".len();
        Some(l[at..].split('"').next()?.to_owned())
    });
    let name = name.trim_start_matches("character/character_").trim_end_matches(".gsc").to_owned();
    Some(Character { name, models: std::iter::once(body).chain(head).collect(), also: Vec::new() })
}

/// People, alive.
fn wanted(c: &Character) -> bool {
    !["dead", "dog", "shepherd"].iter().any(|skip| c.name.contains(skip))
}

fn scripts(zone: &Zone) -> Vec<Character> {
    zone.assets
        .iter()
        .filter_map(|a| match a {
            Asset::RawFile(r) if r.name.starts_with("character/character_") => {
                parse_script(&r.name, &String::from_utf8_lossy(&r.data))
            }
            _ => None,
        })
        .filter(wanted)
        .collect()
}

/// A Black Ops raw file's text: script files are zlib-compressed behind
/// their two lengths.
fn bo_text(data: &[u8]) -> String {
    use std::io::Read;
    let mut out = Vec::new();
    if data.len() > 8 && data[8] == 0x78 {
        let _ = flate2::read::ZlibDecoder::new(&data[8..]).read_to_end(&mut out);
    } else {
        out = data.to_vec();
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Black Ops' campaign characters: `setModel("body")` (or
/// `setModelFromArray(xmodelalias\..)`), then the head, hat and gear its
/// script attaches, each a model name or an `xmodelalias` list.
fn bo_scripts(zone: &t5::zone::Zone) -> Vec<Character> {
    let raw: HashMap<&str, String> = zone
        .of_type(t5::zone::AssetType::RawFile)
        .filter(|(_, a)| a.name.starts_with("character/") || a.name.starts_with("xmodelalias/"))
        .map(|(_, a)| (a.name.as_str(), bo_text(a.root.bytes("buffer"))))
        .collect();
    let first_quoted = |s: &str| s.split('"').nth(1).map(str::to_owned);
    let alias = |name: &str| raw.get(format!("xmodelalias/{name}.gsc").as_str()).and_then(|t| first_quoted(t));
    // A model name or `..xmodelalias\name::main()..`.
    let value = |s: &str| -> Option<String> {
        match s.find("xmodelalias\\") {
            Some(at) => alias(s[at + "xmodelalias\\".len()..].split("::").next()?),
            None => first_quoted(s),
        }
    };
    let mut out = Vec::new();
    for (name, text) in &raw {
        let Some(short) = name.strip_prefix("character/").and_then(|n| n.strip_suffix(".gsc")) else { continue };
        if short.contains('/') {
            continue;
        }
        let body = text.lines().find(|l| l.contains("setModel")).and_then(|l| value(l.split_once("setModel")?.1));
        let Some(body) = body else { continue };
        let mut models = vec![body];
        for field in ["headModel", "hatModel", "gearModel"] {
            if !text.contains(&format!("attach(self.{field}")) {
                continue;
            }
            let assigned = text.lines().find_map(|l| l.split_once(&format!("self.{field} = ")).map(|(_, r)| r.to_owned()));
            if let Some(m) = assigned.as_deref().and_then(value) {
                models.push(m);
            }
        }
        // Models attached by name.
        for l in text.lines().filter(|l| l.contains("attach(\"")) {
            if let Some(m) = first_quoted(l.split_once("attach(").map_or("", |(_, r)| r)) {
                models.push(m);
            }
        }
        out.push(Character { name: short.to_owned(), models, also: Vec::new() });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.into_iter().filter(wanted).collect()
}

/// Black Ops' multiplayer factions in a map: `c_<faction>_mp_body_<kind>`
/// paired with `c_<faction>_mp_head_<n>` in turn.
fn bo_factions(zone: &Zone) -> Vec<Character> {
    let mut factions: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for a in &zone.assets {
        let Asset::XModel(x) = a else { continue };
        let Some((faction, part)) = x.name.strip_prefix("c_").and_then(|r| r.split_once("_mp_")) else { continue };
        let entry = factions.entry(faction.to_owned()).or_default();
        if part.starts_with("body") {
            entry.0.push(x.name.clone());
        } else if part.starts_with("head") {
            entry.1.push(x.name.clone());
        }
    }
    let mut out = Vec::new();
    for (faction, (mut bodies, mut heads)) in factions {
        bodies.sort();
        heads.sort();
        for (i, body) in bodies.iter().enumerate() {
            let kind = body.rsplit("_mp_").next().unwrap_or(body);
            let models = std::iter::once(body.clone()).chain(heads.get(i % heads.len().max(1)).cloned()).collect();
            out.push(Character { name: format!("{faction} {kind}"), models, also: Vec::new() });
        }
    }
    out
}

/// A zone parsed on a thread: its models (as CoD4 assets) and characters,
/// and other zones defining materials it only references.
struct Loaded {
    zone: Zone,
    characters: Vec<Character>,
    extras: Vec<Zone>,
}

/// Black Ops: which zone defines each material (built once, by parsing
/// common_mp and every gallery zone).
fn material_zones() -> &'static HashMap<String, &'static str> {
    static INDEX: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = HashMap::new();
        let Ok(install) = t5::Install::locate() else { return index };
        for &name in std::iter::once(&"common_mp").chain(Game::BlackOps.zones()) {
            let Ok(data) = t5::fastfile::load(&install.zone_path(name)) else { continue };
            let Ok(zone) = t5::zone::Zone::parse(&data, t5::zone::ParseOptions::default()) else { continue };
            for (_, a) in zone.of_type(t5::zone::AssetType::Material).filter(|(_, a)| !a.name.starts_with(',')) {
                index.entry(a.name.clone()).or_insert(name);
            }
        }
        info!("gallery: {} Black Ops materials indexed", index.len());
        index
    })
}

/// The levels defining the materials `characters` only reference in `zone`.
fn defining_zones(zone: &Zone, name: &str, characters: &[Character]) -> Vec<&'static str> {
    let index = material_zones();
    let mut out: Vec<&'static str> = characters
        .iter()
        .flat_map(|c| &c.models)
        .filter_map(|m| zone.xmodel(zone.find(m)?))
        .flat_map(|x| x.materials.iter().flatten().filter_map(|&m| zone.material(m)))
        .filter_map(|m| index.get(m.name.strip_prefix(',')?).copied())
        .filter(|z| *z != name)
        .collect();
    out.sort();
    out.dedup();
    out
}

fn load(game: Game, name: &'static str) -> JoinHandle<anyhow::Result<Loaded>> {
    std::thread::spawn(move || match game {
        Game::Cod4 => {
            let install = iw3::Install::locate()?;
            let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(name))?, ParseOptions::default())?;
            let characters = scripts(&zone);
            Ok(Loaded { zone, characters, extras: Vec::new() })
        }
        Game::BlackOps => {
            let install = t5::Install::locate()?;
            let parsed = t5::zone::Zone::parse(&t5::fastfile::load(&install.zone_path(name))?, t5::zone::ParseOptions::default())?;
            let zone = t5::convert::to_iw3(&parsed);
            let mut characters = if name.starts_with("mp_") { bo_factions(&zone) } else { bo_scripts(&parsed) };
            for c in &mut characters {
                c.also = defining_zones(&zone, name, std::slice::from_ref(c));
            }
            let extras = defining_zones(&zone, name, &characters)
                .into_iter()
                .filter(|z| *z != "common_mp")
                .filter_map(|z| t5::zone::Zone::parse(&t5::fastfile::load(&install.zone_path(z)).ok()?, t5::zone::ParseOptions::default()).ok())
                .map(|z| t5::convert::to_iw3(&z))
                .collect();
            Ok(Loaded { zone, characters, extras })
        }
    })
}

#[derive(Resource, Default)]
struct Gallery {
    dir: PathBuf,
    game: Game,
    vfs: Option<Arc<iw3::iwd::Vfs>>,
    /// common_mp: the animations and the AK (and CoD4's multiplayer scripts).
    common: Option<Zone>,
    mp_scripts: Vec<Character>,
    loading: Option<(usize, JoinHandle<anyhow::Result<Loaded>>)>,
    next_zone: usize,
    content: Option<Content>,
    /// Characters of the loaded zone still to show.
    queue: Vec<Character>,
    zone_name: &'static str,
    shown: HashSet<String>,
    page: usize,
    /// Frames since this page was spawned.
    frames: u32,
    /// When it was spawned.
    shown_at: f32,
    /// Model sets shown: some scripts only differ in name.
    models_shown: HashSet<Vec<String>>,
    on_page: Vec<Entity>,
    index: String,
    done: bool,
}

#[derive(Component)]
struct GalleryCamera;

fn setup(mut commands: Commands, mut gallery: ResMut<Gallery>) {
    let rows = (PER_PAGE / COLUMNS) as f32;
    commands.spawn((
        GalleryCamera,
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            scaling_mode: bevy::camera::ScalingMode::FixedVertical { viewport_height: crate::units::u(CELL_H * rows + 16.0) },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
        AmbientLight { brightness: 400.0, ..default() },
    ));
    commands.spawn((DirectionalLight { illuminance: 6000.0, ..default() }, Transform::default().looking_to(Vec3::new(-0.4, -0.5, -1.0), Vec3::Y)));
    commands.spawn((DirectionalLight { illuminance: 2000.0, ..default() }, Transform::default().looking_to(Vec3::new(0.7, -0.2, -0.6), Vec3::Y)));
    match gallery.game {
        Game::Cod4 => {
            let install = iw3::Install::locate().expect("CoD4 install");
            gallery.vfs = Some(Arc::new(iw3::iwd::Vfs::mount(&install.iwd_paths().expect("iwds")).expect("mounting iwds")));
            let common = Zone::parse(&iw3::fastfile::load(&install.zone_path("common_mp")).expect("common_mp"), ParseOptions::default())
                .expect("parsing common_mp");
            gallery.mp_scripts = scripts(&common);
            gallery.common = Some(common);
        }
        Game::BlackOps => {
            let install = t5::Install::locate().expect("Black Ops install");
            gallery.vfs = Some(Arc::new(install.vfs().expect("mounting Black Ops iwds")));
            gallery.common = Some(t5::load_iw3(&install, "common_mp").expect("Black Ops common_mp"));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    time: Res<Time>,
    mut gallery: ResMut<Gallery>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    camera: Single<(&Camera, &GlobalTransform), With<GalleryCamera>>,
    mut exit: MessageWriter<AppExit>,
) {
    let g = &mut *gallery;
    if g.done {
        g.frames += 1;
        if g.frames > 30 {
            exit.write(AppExit::Success);
        }
        return;
    }
    // A page on show: give its textures a few frames (and the first page
    // the shader compiles), then save it.
    if !g.on_page.is_empty() {
        g.frames += 1;
        let wait = if g.page == 0 { 6.0 } else { 0.5 };
        if g.frames < 1_000_000 && time.elapsed_secs() - g.shown_at >= wait {
            g.frames = 1_000_000;
            std::fs::create_dir_all(&g.dir).ok();
            let path = g.dir.join(format!("{:02}_{}.png", g.page, g.zone_name));
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        }
        if g.frames >= 1_000_010 {
            for e in g.on_page.drain(..) {
                commands.entity(e).despawn();
            }
            g.page += 1;
        }
        return;
    }
    // The next page of this zone.
    if !g.queue.is_empty() {
        let page: Vec<Character> = g.queue.drain(..g.queue.len().min(PER_PAGE)).collect();
        let gun = g.game.gun();
        let Some(content) = g.content.as_mut() else { return };
        g.index += &format!("page {:02} ({}):\n", g.page, g.zone_name);
        let (cam, cam_tf) = *camera;
        for (k, c) in page.iter().enumerate() {
            let (col, row) = ((k % COLUMNS) as f32, (k / COLUMNS) as f32);
            let feet = Vec3::new(
                crate::units::u((col - (COLUMNS as f32 - 1.0) * 0.5) * CELL_W),
                crate::units::u(CELL_H * (0.5 - row) - CELL_H * 0.5 + 6.0),
                0.0,
            );
            // CoD models face +X; turn them to the camera, a little to the side.
            let owner = commands
                .spawn((Transform::from_translation(feet).with_rotation(Quat::from_rotation_y(-1.2)), Visibility::default()))
                .id();
            let mut skeleton = Skeleton::default();
            for name in &c.models {
                if let Some(m) = content.model(name, &mut meshes, &mut materials, &mut images, &mut bindposes) {
                    spawn_model(&mut commands, &mut skeleton, SpawnModel { model: &m, owner, attach_to: None, layers: None, shadows: false });
                }
            }
            if let Some(gun) = content.model(gun, &mut meshes, &mut materials, &mut images, &mut bindposes) {
                let hand = skeleton.joint("tag_weapon_right");
                spawn_model(&mut commands, &mut skeleton, SpawnModel { model: &gun, owner, attach_to: hand, layers: None, shadows: false });
            }
            let mut player = AnimPlayer::default();
            // `COD4RW_GALLERY_POSE=<anim>` poses them otherwise.
            let pose = std::env::var("COD4RW_GALLERY_POSE").unwrap_or_else(|_| "pb_stand_alert".into());
            if let Some(a) = content.anim(&pose) {
                player.play(a, 0.0);
            }
            commands.entity(owner).insert((skeleton, player));
            g.on_page.push(owner);
            // The name under the feet.
            let label_at = feet - Vec3::Y * crate::units::u(3.0);
            if let Ok(p) = cam.world_to_viewport(cam_tf, label_at) {
                let label = commands
                    .spawn((
                        Text::new(std::iter::once(&c.name).chain(&c.models).cloned().collect::<Vec<_>>().join("\n")),
                        TextFont { font_size: FontSize::Px(12.0), ..default() },
                        TextColor(Color::WHITE),
                        TextLayout::justify(Justify::Center),
                        Node { position_type: PositionType::Absolute, left: px(p.x - 150.0), top: px(p.y), width: px(300.0), ..default() },
                    ))
                    .id();
                g.on_page.push(label);
            }
            g.index += &format!("  {}: {} | also {:?}\n", c.name, c.models.join(" "), c.also);
        }
        g.frames = 0;
        g.shown_at = time.elapsed_secs();
        return;
    }
    // The next zone.
    // `COD4RW_GALLERY_ZONES=a,b` limits it to those zones.
    let only = std::env::var("COD4RW_GALLERY_ZONES").ok();
    let zones: Vec<&str> = g.game.zones().iter().copied().filter(|z| only.as_ref().is_none_or(|o| o.split(',').any(|x| x == *z))).collect();
    match g.loading.take() {
        Some((i, task)) if task.is_finished() => {
            let name = zones[i];
            let Loaded { zone, characters, extras } = match task.join() {
                Ok(Ok(l)) => l,
                Ok(Err(e)) => {
                    warn!("gallery: {name}: {e:#}");
                    return;
                }
                Err(_) => return,
            };
            let found = if g.game == Game::Cod4 && name.starts_with("mp_") { g.mp_scripts.clone() } else { characters };
            // Characters not shown yet whose body this zone has (with
            // geometry: Black Ops' cinematic `_char` copies come without);
            // attachments it lacks are left off.
            let has_geometry =
                |m: &str| zone.find(m).and_then(|id| zone.xmodel(id)).is_some_and(|x| x.surfs.iter().any(|s| !s.verts.is_empty()));
            let new: Vec<Character> = found
                .into_iter()
                .filter(|c| !g.shown.contains(&c.name) && has_geometry(&c.models[0]))
                .map(|mut c| {
                    c.models.retain(|m| zone.find(m).is_some());
                    c
                })
                .filter(|c| g.models_shown.insert(c.models.clone()))
                .collect();
            info!("gallery: {name}: {} new characters", new.len());
            let common = g.content.take().map_or_else(|| g.common.take(), |mut c| c.zones.pop());
            g.shown.extend(new.iter().map(|c| c.name.clone()));
            g.queue = new;
            g.zone_name = name;
            let zones = std::iter::once(zone).chain(extras).chain(common).collect();
            g.content = Some(Content::new(zones, g.vfs.clone().expect("vfs")));
        }
        Some(task) => g.loading = Some(task),
        None if g.next_zone < zones.len() => {
            g.loading = Some((g.next_zone, load(g.game, zones[g.next_zone])));
            g.next_zone += 1;
        }
        None => {
            std::fs::write(g.dir.join("index.txt"), &g.index).ok();
            info!("gallery: {} pages in {}", g.page, g.dir.display());
            g.done = true;
            g.frames = 0;
        }
    }
}
