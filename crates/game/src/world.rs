//! Loading a CoD4 map zone into Bevy: world geometry, static models, sun and
//! spawn points. Collision is built by [`crate::collision`].

use crate::content::{Content, MAP_ZONE};
use crate::units;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{RenderLayers, VisibilityRange};
use bevy::light::{CascadeShadowConfigBuilder, NotShadowCaster};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, Lightmap, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use iw3::zone::{self, AssetId, ParseOptions, TextureSemantic, Zone};
use std::collections::HashMap;
use std::sync::Arc;

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("shaders/world.wgsl"),
            std::path::Path::new(WORLD_SHADER),
            include_bytes!("shaders/world.wgsl").as_slice(),
        );
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("shaders/world_prepass.wgsl"),
            std::path::Path::new("cod4rw/world_prepass.wgsl"),
            include_bytes!("shaders/world_prepass.wgsl").as_slice(),
        );
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("shaders/world_deferred.wgsl"),
            std::path::Path::new("cod4rw/world_deferred.wgsl"),
            include_bytes!("shaders/world_deferred.wgsl").as_slice(),
        );
        app.add_plugins(MaterialPlugin::<WorldMaterial>::default()).add_systems(OnEnter(crate::state::GameState::InGame), load_map.in_set(crate::state::Setup::Content));
        if std::env::var("COD4RW_WORLDLIGHTS").is_ok_and(|v| v == "1") && std::env::var_os("COD4RW_FIRE_AB").is_none() && std::env::var_os("COD4RW_SHOT").is_some() {
            app.add_systems(Update, world_lights_ab);
        }
    }
}

/// World surfaces: the standard material plus IW3's surface model (see
/// `shaders/world.wgsl`): normal and specular maps, the directional
/// lightmap, and view-angle falloff.
pub type WorldMaterial = ExtendedMaterial<StandardMaterial, WorldLighting>;

/// Bindless where the device allows (one bind group for many materials, so
/// their draws batch: `shaders/world.wgsl`'s `BINDLESS` path).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
#[data(100, WorldParams, binding_array(121))]
#[bindless(index_table(range(100..118), binding(120)))]
pub struct WorldLighting {
    pub params: WorldParams,
    /// IW3 normal map: tangent-space x in alpha, y in green.
    #[texture(101)]
    #[sampler(102)]
    pub normal_map: Option<Handle<Image>>,
    /// IW3 specular map: specular colour in rgb, gloss in alpha.
    #[texture(103)]
    #[sampler(104)]
    pub specular_map: Option<Handle<Image>>,
    /// The raw `lightmapN_secondary` image (see `crate::lightmaps`).
    #[texture(105)]
    #[sampler(106)]
    pub lightmap: Option<Handle<Image>>,
    /// The surface's baked reflection probe cube map.
    #[texture(107, dimension = "cube")]
    #[sampler(108)]
    pub reflection_probe: Option<Handle<Image>>,
    /// IW3 detail map: fine grit tiled over the colour map (asphalt,
    /// sidewalks, plaster), raw gamma values.
    #[texture(109)]
    #[sampler(110)]
    pub detail_map: Option<Handle<Image>>,
    /// Where rain reaches, from above, round the camera (`crate::wet`):
    /// the showcase's wet surfaces stay dry under cover.
    #[texture(111)]
    pub wet_map: Option<Handle<Image>>,
    /// The world camera's last image, for the showcase's screen-space
    /// reflections (`crate::ssr`).
    #[texture(112)]
    #[sampler(113)]
    pub ssr_history: Option<Handle<Image>>,
    /// Heights worked out from the normal map, for the showcase's
    /// parallax occlusion mapping (`crate::pom`).
    #[texture(114)]
    #[sampler(115)]
    pub height_map: Option<Handle<Image>>,
    /// The raw `lightmapN_primary` image: the baked visibility of each
    /// surface's primary light (`COD4RW_WORLDLIGHTS`).
    #[texture(116)]
    #[sampler(117)]
    pub primary_map: Option<Handle<Image>>,
}

#[derive(ShaderType, Reflect, Debug, Clone, Default)]
pub struct WorldParams {
    /// IW3 `*_falloff_*` materials (fake light beams) fade as they turn
    /// edge-on: `t = saturate(cos^2 * z + w)`, colour scaled by
    /// `t * mix(end, begin, t)`. All zero for other materials.
    pub falloff_parms: Vec4,
    pub falloff_begin: Vec4,
    pub falloff_end: Vec4,
    /// x: has a normal map, y: has a specular map, z: has a lightmap,
    /// w: lightmap exposure.
    pub flags: Vec4,
    /// x: has a reflection probe, y: its last mip level.
    pub probe: Vec4,
    /// IW3 water (`wc_water`): `waterColor`, w = 1 for water.
    pub water_color: Vec4,
    /// Water's `envMapParms`: fresnel min, max and power, sun glint.
    pub water_env: Vec4,
    /// Direction to the sun (Bevy space) for the water's glint.
    pub sun_dir: Vec4,
    /// The sun's colour times its intensity (IW3's `sunDiffuse`).
    pub sun_diffuse: Vec4,
    /// IW3 `*_distfalloff` materials (HDR portals in doorways, brightening
    /// or darkening what's seen through them): a colour from `falloff_end`
    /// near to `falloff_begin` far, `t = saturate(metres * x + y)`, that
    /// multiplies the scene behind, scaled by z (2 for DESTCOLOR/SRCCOLOR);
    /// w = 1 for these.
    pub dist_falloff: Vec4,
    /// Ray-traced lighting (`crate::rtgi`, which draws world surfaces
    /// deferred, `shaders/world_deferred.wgsl`): x is how much of the
    /// lightmap the traced light sits on. (y: CoD4's sun trial, z: the
    /// lightmap is re-baked, `crate::lightmaps::Rebaked`.)
    pub traced: Vec4,
    /// Detail map: x, y its tiling (`detailScale`), z: has one, w: its
    /// last mip level (whose texel is its average).
    pub detail: Vec4,
    /// Showcase reflections (`crate::probe_parallax`): the reflection
    /// probe's position (w = 1 when set) and the box around it the probe's
    /// view is projected onto (min w = 1 once fitted).
    pub probe_center: Vec4,
    pub probe_box_min: Vec4,
    pub probe_box_max: Vec4,
    /// The surfaces' primary light (`COD4RW_WORLDLIGHTS`; CoD4 adds it to
    /// the lightmap at run time, which leaves it out): position (Bevy) and
    /// 1 / radius (m).
    pub plight_pos: Vec4,
    /// Its colour times intensity (gamma space, as `sun_diffuse`); w 1 on.
    pub plight_color: Vec4,
    /// Its direction (where it shines) and the spot's exponent.
    pub plight_dir: Vec4,
    /// Spot: x `1 / (cos inner - cos outer)`, y `-x * cos outer`, z 1 for a
    /// spot; w 1 with a primary lightmap (baked visibility) to multiply by.
    pub plight_spot: Vec4,
    /// The material-class layer (`crate::material_class`): x the CoD4
    /// surface type, y 1 when on, z detail strength, w anti-tiling amount.
    pub surface: Vec4,
    /// Probe normalisation (`crate::world::probe_norm`): x strength, y on,
    /// z calibration, w the most it may brighten.
    pub probe_norm: Vec4,
}

impl From<&WorldLighting> for WorldParams {
    fn from(lighting: &WorldLighting) -> Self {
        lighting.params.clone()
    }
}

impl WorldLighting {
    fn falloff(mat: Option<&zone::Material>, technique_set: &str, params: &mut WorldParams) {
        let constant = |prefix: &str| {
            mat.and_then(|m| m.constants.iter().find(|c| c.name.starts_with(prefix))).map(|c| Vec4::from_array(c.literal))
        };
        let Some(parms) = constant("falloffParms").filter(|_| technique_set.contains("falloff")) else { return };
        params.falloff_begin = constant("falloffBegin").unwrap_or(Vec4::ONE);
        params.falloff_end = constant("falloffEndCo").unwrap_or(Vec4::ZERO);
        if technique_set.contains("distfalloff") {
            // `falloffParms.xy` fade by distance (per inch); zw are the
            // unused view-angle ones.
            let doubled = mat.and_then(crate::content::state_bits).is_some_and(|[b0, _]| (b0 & 0xf, (b0 >> 4) & 0xf) == (9, 3));
            params.dist_falloff = Vec4::new(parms.x / units::INCH, parms.y, if doubled { 2.0 } else { 1.0 }, 1.0);
        } else {
            params.falloff_parms = parms;
        }
    }
}

impl WorldLighting {
    /// IW3 water: its colour map is a placeholder (`case64blue`); the
    /// surface is the engine's animated waves reflecting the probe, coloured
    /// by the material's `waterColor` and `envMapParms`, with a sun glint.
    fn water(mat: Option<&zone::Material>, world: &zone::GfxWorld, params: &mut WorldParams) {
        let constant = |name: &str| mat.and_then(|m| m.constants.iter().find(|c| c.name == name)).map(|c| Vec4::from_array(c.literal));
        let colour = constant("waterColor").unwrap_or(Vec4::new(0.3, 0.3, 0.24, 1.0));
        // (w: 1 for water, plus how muddy the map's look makes it,
        // `crate::atmos::climate::MapLook`.)
        let muddy = crate::atmos::climate::look().map_or(0.0, |l| l.muddy_water);
        params.water_color = colour.truncate().extend(1.0 + muddy);
        params.water_env = constant("envMapParms").unwrap_or(Vec4::new(0.2, 0.5, 2.5, 2.5));
        let sun = &world.sun;
        let (pitch, yaw) = (sun.angles[0].to_radians(), sun.angles[1].to_radians());
        let to_sun = units::dir([pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), -pitch.sin()]).normalize();
        params.sun_dir = to_sun.extend(0.0);
        let c = sun.sun_color;
        params.sun_diffuse = (Vec3::new(c[0], c[1], c[2]) * sun.sun_light.max(0.5)).extend(0.0);
    }
}

/// Where `shaders/world.wgsl` is registered in the `embedded://` asset source.
const WORLD_SHADER: &str = "cod4rw/world.wgsl";

impl MaterialExtension for WorldLighting {
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/world.wgsl".into()
    }

    /// The prepass drops what the main pass does (vertex alpha included).
    fn prepass_fragment_shader() -> ShaderRef {
        "embedded://cod4rw/world_prepass.wgsl".into()
    }

    fn deferred_fragment_shader() -> ShaderRef {
        "embedded://cod4rw/world_deferred.wgsl".into()
    }
}

#[derive(Resource, Clone)]
pub struct MapName(pub String);

/// The map's reflection probes: where each is, its cube map and last mip.
#[derive(Resource, Clone, Default)]
pub struct ReflectionProbes(pub Vec<(Vec3, Handle<Image>, f32)>);

/// Render layer of the world's merged shadow caster: only the sun sees it.
pub const SHADOW_PROXY_LAYER: usize = 2;

/// The map's sky cube map, drawn as the main camera's skybox.
#[derive(Resource, Clone)]
pub struct MapSky(pub Handle<Image>);

/// The map's own water surfaces (`wc_water`), with their extent (Bevy
/// space), worked out as they were built.
#[derive(Component, Clone, Copy, Debug)]
pub struct MapWater {
    pub min: Vec3,
    pub max: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnKind {
    /// Any team, mid-match (`mp_tdm_spawn`).
    Tdm,
    AlliesStart,
    AxisStart,
    /// Free-for-all (`mp_dm_spawn`).
    Dm,
    /// Domination: mid-match, and each team's at the start.
    Dom,
    DomAlliesStart,
    DomAxisStart,
    /// Search and Destroy, by side (the teams swap sides).
    SdAttacker,
    SdDefender,
    /// Sabotage, each team's own, and at the start.
    SabAllies,
    SabAxis,
    SabAlliesStart,
    SabAxisStart,
}

#[derive(Clone, Copy, Debug)]
pub struct SpawnPoint {
    pub pos: Vec3,
    /// Bevy yaw in radians.
    pub yaw: f32,
    pub kind: SpawnKind,
}

#[derive(Resource)]
pub struct MapInfo {
    pub spawns: Vec<SpawnPoint>,
}

pub fn load_map(
    mut commands: Commands,
    map: Res<MapName>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    mut world_materials: ResMut<Assets<WorldMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let t0 = std::time::Instant::now();
    let install = iw3::Install::locate().expect("CoD4 install not found (set COD4_PATH)");
    // (A custom map's own archives too; a Modern Warfare 2 map's, under
    // CoD4's: see `crate::mw2`.)
    let mw2 = crate::mw2::zone(&map.0);
    crate::mw2::set_active(mw2.is_some());
    let mut iwds = if mw2.is_some() { crate::mw2::iwd_paths() } else { Vec::new() };
    iwds.extend(install.map_iwd_paths(&map.0).expect("listing iwds"));
    let vfs = Arc::new(iw3::iwd::Vfs::mount(&iwds).expect("mounting iwds"));
    info!("mounted iwds in {:?}", t0.elapsed());

    let load_zone = |name: &str| {
        let data = iw3::fastfile::load(&install.zone_path(name)).unwrap_or_else(|e| panic!("loading {name}: {e:#}"));
        let zone = Zone::parse(&data, ParseOptions::default()).unwrap_or_else(|e| panic!("parsing {name}: {e:#}"));
        info!(
            "parsed {name}: {} assets, {} unresolved refs ({:?})",
            zone.assets.len(),
            zone.stats.unresolved_aliases,
            t0.elapsed()
        );
        zone
    };
    let map_zone = match &mw2 {
        Some(zone) => {
            let z = crate::mw2::load(zone).unwrap_or_else(|e| panic!("loading MW2 map {zone}: {e:#}"));
            info!("converted MW2 {zone}: {} assets ({:?})", z.assets.len(), t0.elapsed());
            z
        }
        // (A campaign mission's, read during its briefing: `crate::campaign`.)
        None => crate::campaign::take_zone(&map.0).unwrap_or_else(|| load_zone(&map.0)),
    };
    // (Parsed ahead on a background thread, `crate::common_zone`.)
    let common = crate::common_zone::take().unwrap_or_else(|| load_zone("common_mp"));
    let mut content = Content::new(vec![map_zone, common], vfs);
    info!("load: zones ready ({:?})", t0.elapsed());

    // Re-baked lighting (`crate::bake`) when wanted and cached, else CoD4's.
    let zone_path = mw2.as_deref().and_then(crate::mw2::zone_path).unwrap_or_else(|| install.zone_path(&map.0));
    let rebaked = crate::lightmaps::load_rebaked(&mut commands, &map.0, &zone_path, &mut images);
    let lightmaps = match &rebaked {
        Some(r) => r.lightmaps.clone(),
        None => crate::lightmaps::load(content.map(), content.map().gfx_world().expect("zone has no GfxWorld"), &mut images),
    };
    info!("load: lightmaps ({:?})", t0.elapsed());
    let probes =
        crate::lightmaps::load_probes(content.map(), content.map().gfx_world().expect("zone has no GfxWorld"), &mut images);
    // Models (guns) reflect the nearest probe.
    let gfx = content.map().gfx_world().expect("zone has no GfxWorld");
    let placed = gfx.reflection_probes.iter().zip(&probes).skip(1).filter_map(|(p, h)| {
        let h = h.clone()?;
        let mip = images.get(&h).map_or(0.0, |i| i.texture_descriptor.mip_level_count.saturating_sub(1) as f32);
        Some((units::pos(p.origin), h, mip))
    });
    let placed: Vec<(Vec3, Handle<Image>, f32)> = placed.collect();
    info!("load: probes ({:?})", t0.elapsed());
    commands.insert_resource(ReflectionProbes(placed.clone()));
    spawn_world_geometry(
        &mut commands,
        &mut content,
        &lightmaps,
        &probes,
        &mut meshes,
        &mut std_materials,
        &mut world_materials,
        &mut images,
        rebaked.is_some(),
    );
    spawn_static_models(&mut commands, &mut content, &mut meshes, &mut std_materials, &mut world_materials, &mut images, &placed);
    let world = content.map().gfx_world().expect("zone has no GfxWorld");
    let light_grid = crate::model_lighting::spawn_irradiance_volume(&mut commands, world, &mut images, rebaked.as_ref().map(|r| &r.grid));
    spawn_sun(&mut commands, world, light_grid);
    if let Some(sky) = content.sky_cube(&mut images) {
        commands.insert_resource(MapSky(sky));
    }
    info!("built render data in {:?}", t0.elapsed());

    if let Some(clip) = content.map().clip_map() {
        crate::collision::spawn_collision(&mut commands, clip);
        // Solid brush entities, where they stand (`script_brushmodel`).
        let ents = content.map().map_ents().map(|e| iw3::ents::parse(&e.entity_string)).unwrap_or_default();
        let places: Vec<(usize, Vec3, Quat)> = ents
            .iter()
            // Not game-mode objects (`script_gameobjectname`: Sabotage's
            // bomb site box, Headquarters' radio boxes): CoD4's scripts
            // delete them outside their mode, and the modes place their own.
            // Kept, they stood invisible and solid on Wet Work's deck.
            .filter(|e| e.classname() == "script_brushmodel" && brush_entity_shown(e))
            .filter_map(|e| {
                let n = e.get("model")?.strip_prefix('*')?.parse::<usize>().ok()?;
                Some((n, units::pos(e.origin().unwrap_or([0.0; 3])), crate::modes::koth::cod_rotation(e.angles())))
            })
            .chain(dyn_brush_places(&content))
            .collect();
        let movable = movable_brush_models(&content);
        crate::collision::spawn_brush_entity_collision(&mut commands, clip, &places, &movable);
        crate::collision::spawn_static_model_collision(&mut commands, content.map(), clip);
    }
    let spawns =
        content.map().map_ents().map(|e| read_spawns(&iw3::ents::parse(&e.entity_string))).unwrap_or_default();
    info!("{} spawn points; map ready in {:?}", spawns.len(), t0.elapsed());
    commands.insert_resource(MapInfo { spawns });
    commands.insert_resource(content);
}

/// Whether a brush entity is there in a match, as CoD4's scripts leave it:
/// not a game mode's object (`script_gameobjectname`: deleted outside its
/// mode, and the modes place their own), nor an exploder's after-piece
/// (`_load.gsc`'s `setupExploders`: `script_exploder` /
/// `script_prefab_exploder` with targetname `exploder` or `exploderchunk`,
/// hidden and not solid until it goes off).
fn brush_entity_shown(e: &iw3::ents::Entity) -> bool {
    let exploder = e.get("script_exploder").is_some() || e.get("script_prefab_exploder").is_some();
    e.get("script_gameobjectname").is_none() && !(exploder && matches!(e.get("targetname"), Some("exploder" | "exploderchunk")))
}

/// A script brush model a mission's scripts move: its surfaces drawn about
/// its own origin under this entity, its colliders tagged so too; the stage
/// sets their place from the level (`crate::campaign`).
#[derive(Component, Clone, Copy, Debug)]
pub struct ScriptBrushPart(pub usize);

/// The brush models of a mission's `script_brushmodel`s (none outside the
/// campaign: multiplayer's stay merged into the world).
pub fn movable_brush_models(content: &Content) -> std::collections::HashSet<usize> {
    // (Debug: `COD4RW_STATIC_BRUSHES=1` keeps them in the world, as before.)
    let hq = crate::hq::active();
    if !(crate::campaign::active() || hq) || std::env::var_os("COD4RW_STATIC_BRUSHES").is_some() {
        return Default::default();
    }
    let ents = content.map().map_ents().map(|e| iw3::ents::parse(&e.entity_string)).unwrap_or_default();
    // Headquarters moves only F.N.G.'s doors and pop-up targets.
    if hq {
        return crate::hq::movable_brushes(&ents);
    }
    ents.iter()
        .filter(|e| e.classname() == "script_brushmodel" && brush_entity_shown(e))
        .filter_map(|e| e.get("model")?.strip_prefix('*')?.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .collect()
}

/// Where each brush model is drawn (index = model; 0 the world, in place):
/// the origin and turn of the entity that uses it (`model "*N"`), `None`
/// for one no entity uses.
fn brush_model_places(content: &Content) -> Vec<Option<(Vec3, Quat)>> {
    let count = content.map().gfx_world().map_or(0, |w| w.models.len());
    let mut out = vec![None; count];
    if let Some(first) = out.first_mut() {
        *first = Some((Vec3::ZERO, Quat::IDENTITY));
    }
    let ents = content.map().map_ents().map(|e| iw3::ents::parse(&e.entity_string)).unwrap_or_default();
    for e in ents.iter().filter(|e| brush_entity_shown(e)) {
        let Some(n) = e.get("model").and_then(|m| m.strip_prefix('*')).and_then(|n| n.parse::<usize>().ok()) else { continue };
        if n == 0 || n >= count || out[n].is_some() {
            continue;
        }
        let origin = e.origin().unwrap_or([0.0; 3]);
        out[n] = Some((units::pos(origin), crate::modes::koth::cod_rotation(e.angles())));
    }
    for (n, at, turn) in dyn_brush_places(content) {
        if n < count && out[n].is_none() {
            out[n] = Some((at, turn));
        }
    }
    out
}

/// Brush models a dynamic entity stands for (MW2's destructible brush
/// pieces: no entity places them), where it stands. Drawn and solid; they
/// don't break yet.
fn dyn_brush_places(content: &Content) -> Vec<(usize, Vec3, Quat)> {
    content
        .map()
        .clip_map()
        .map(|c| {
            c.dyn_ents
                .iter()
                .filter(|d| d.model.is_none() && d.brush_model > 0)
                .map(|d| (d.brush_model as usize, units::pos(d.origin), units::axis_rotation(crate::clutter::quat_axes(d.quat))))
                .collect()
        })
        .unwrap_or_default()
}

/// World vertex colours are gamma-space multipliers (IW3 lit everything in
/// gamma space); decode them so they scale linear light the same way.
fn vertex_color(packed: u32) -> [f32; 4] {
    let [r, g, b, a] = iw3::unpack::color(packed);
    [r.powf(2.2), g.powf(2.2), b.powf(2.2), a]
}

fn spawn_world_geometry(
    commands: &mut Commands,
    content: &mut Content,
    lightmaps: &[Option<Handle<Image>>],
    probes: &[Option<Handle<Image>>],
    meshes: &mut Assets<Mesh>,
    std_materials: &mut Assets<StandardMaterial>,
    world_materials: &mut Assets<WorldMaterial>,
    images: &mut Assets<Image>,
    rebaked: bool,
) {
    // The showcase's wet surfaces reflect the sky where there's no probe.
    let sky = crate::atmos::climate::showcase().then(|| content.sky_cube(images)).flatten();
    // Group surfaces by material, lightmap and reflection probe so each group
    // is one draw.
    // Brush models (`*N`: Wet Work's water tank on its trailer, its radar,
    // the hold's panels) keep their surfaces about their own origin: each
    // is drawn where its entity stands, turned as it's turned. One no
    // entity places (a trigger's) isn't drawn. Model 0 is the world.
    let placed = brush_model_places(content);
    let surf_model = {
        let w = content.map().gfx_world().expect("gfxworld");
        let mut of = vec![0u16; w.surfaces.len()];
        for (n, m) in w.models.iter().enumerate().skip(1) {
            let start = m.start_surf_index as usize;
            for s in of.iter_mut().skip(start).take(m.surface_count as usize) {
                *s = n as u16;
            }
        }
        of
    };
    // A mission's script brush models move (doors, the killhouse's
    // targets): each drawn on its own, about its own origin, under an
    // entity the stage moves ([`ScriptBrushPart`]).
    let movable = movable_brush_models(content);
    let mut moving: HashMap<(AssetId, u8, u8, u8, usize), Vec<usize>> = HashMap::new();
    let mut groups: HashMap<(AssetId, u8, u8, u8), Vec<usize>> = HashMap::new();
    // (With `COD4RW_WORLDLIGHTS`, split by primary light too: each group's
    // material carries its light.)
    let lights_on = world_lights();
    let primaries = if lights_on { crate::lightmaps::load_primary(content.map(), content.map().gfx_world().expect("gfxworld"), images) } else { Vec::new() };
    for (i, s) in content.map().gfx_world().expect("gfxworld").surfaces.iter().enumerate() {
        if placed.get(surf_model[i] as usize).copied().flatten().is_none() {
            continue;
        }
        if movable.contains(&(surf_model[i] as usize)) {
            if let Some(m) = s.material {
                moving.entry((m, s.lightmap_index, s.reflection_probe_index, if lights_on { s.primary_light_index } else { 0 }, surf_model[i] as usize)).or_default().push(i);
            }
            continue;
        }
        if let Some(m) = s.material {
            groups.entry((m, s.lightmap_index, s.reflection_probe_index, if lights_on { s.primary_light_index } else { 0 })).or_default().push(i);
        }
    }
    // One group per material and lightmap, with the probe most of its
    // triangles use: split per probe the world was five times as many
    // materials (and draws in every view), which the render thread was
    // bound by; the probe only gives reflections, which barely differ
    // between neighbouring probes. `COD4RW_PROBESPLIT` keeps them split.
    if std::env::var_os("COD4RW_PROBESPLIT").is_none() {
        let surfaces = &content.map().gfx_world().expect("gfxworld").surfaces;
        let mut merged: HashMap<(AssetId, u8, u8), (HashMap<u8, usize>, Vec<usize>)> = HashMap::new();
        for ((m, lm, probe, pl), list) in groups.drain() {
            let entry = merged.entry((m, lm, pl)).or_default();
            *entry.0.entry(probe).or_default() += list.iter().map(|&i| surfaces[i].tri_count as usize).sum::<usize>();
            entry.1.extend(list);
        }
        for ((m, lm, pl), (probes, list)) in merged {
            let probe = probes.into_iter().max_by_key(|&(p, n)| (n, p)).map_or(0, |(p, _)| p);
            groups.insert((m, lm, probe, pl), list);
        }
    }
    let mut world_mats: HashMap<(AssetId, u8, u8, u8), Handle<WorldMaterial>> = HashMap::new();
    {
        let by_lightmap: std::collections::HashSet<(AssetId, u8)> = groups.keys().map(|k| (k.0, k.1)).collect();
        let by_material: std::collections::HashSet<AssetId> = groups.keys().map(|k| k.0).collect();
        info!("world groups: {} (material, lightmap, probe), {} (material, lightmap), {} materials", groups.len(), by_lightmap.len(), by_material.len());
    }

    let root = commands.spawn((Name::new("world"), Transform::default(), Visibility::default())).id();
    let mut total_tris = 0;
    let mut draws = 0;
    // Opaque world geometry is merged into one double-sided mesh that only
    // the sun renders (cascaded shadow maps draw every caster once per
    // cascade, and hundreds of world draws made that the frame's main cost).
    // Double-sided also lets single-sided roofs and walls block the sun.
    let mut proxy_positions: Vec<[f32; 3]> = Vec::new();
    let mut proxy_indices: Vec<u32> = Vec::new();
    let mut parts: HashMap<usize, Entity> = HashMap::new();
    for &n in &movable {
        let (at, turn) = placed.get(n).copied().flatten().unwrap_or((Vec3::ZERO, Quat::IDENTITY));
        let e = commands.spawn((Name::new(format!("brush model *{n}")), Transform::from_translation(at).with_rotation(turn), Visibility::default(), ScriptBrushPart(n), ChildOf(root))).id();
        parts.insert(n, e);
    }
    let all = groups.into_iter().map(|(k, v)| (k, 0usize, v)).chain(moving.into_iter().map(|((m, l, p, pl, n), v)| ((m, l, p, pl), n, v)));
    for ((mat_id, lightmap_index, probe_index, plight), model, surfs) in all {
        let Some(mat) = content.material(MAP_ZONE, mat_id, std_materials, images) else {
            warn!("world material {:?} not drawn: ({}), {} surfaces", content.zone(MAP_ZONE).material(mat_id).map(|m| m.name.clone()), content.technique_set(MAP_ZONE, mat_id), surfs.len());
            continue;
        };
        if mat.sky {
            continue;
        }
        // The showcase's rain-struck water (`shaders/world.wgsl`'s `water`):
        // Downpour's opaque brown "murkiness" layer over its water left it a
        // flat sheet; the water's own colour carries the mud.
        // ... nor CoD4's HDR portals (sheets in doorways faking the eye
        // adjusting, light or dark): auto-exposure does it, and they read as
        // translucent slabs in Downpour's barn doors.
        if crate::atmos::climate::showcase() && content.zone(MAP_ZONE).material(mat_id).is_some_and(|m| m.name.contains("water_mud_murkiness") || m.name.contains("hdrportal")) {
            continue;
        }
        let lightmap = lightmaps.get(lightmap_index as usize).cloned().flatten();
        // Probe 0 is the engine's placeholder (the same reddish cube on every
        // map): no probe.
        let reflection_probe = probes.get(probe_index as usize).filter(|_| probe_index != 0).cloned().flatten();
        // Debug aid (as `crate::content`'s): `COD4RW_MATLOG=<part>` logs
        // matching world surfaces' lightmap and probe.
        if let Ok(part) = std::env::var("COD4RW_MATLOG")
            && !part.is_empty()
            && content.zone(MAP_ZONE).material(mat_id).is_some_and(|m| m.name.contains(&part))
        {
            info!(
                "matlog world: {:?} lightmap {lightmap_index} ({}) probe {probe_index} ({}), {} surfaces",
                content.zone(MAP_ZONE).material(mat_id).map(|m| m.name.clone()),
                lightmap.is_some(),
                reflection_probe.is_some(),
                surfs.len()
            );
        }
        let material = match world_mats.get(&(mat_id, lightmap_index, probe_index, plight)) {
            Some(m) => m.clone(),
            None => {
                let base = std_materials.get(&mat.handle).cloned().unwrap_or_default();
                // `$identitynormalmap` and friends are flat; skip them.
                // (Also `,$identitynormalmap`: a reference to the one in another
                // zone. Taken for a real normal map, it bent the normal far
                // over: lit surfaces' directional light went black, Carentan's
                // walls.)
                let real = |h: Option<Handle<Image>>, name: Option<String>| h.filter(|_| !name.is_some_and(|n| n.trim_start_matches(',').starts_with('$')));
                let mut normal_map = real(
                    content.material_texture(MAP_ZONE, mat_id, TextureSemantic::Normal, false, images),
                    content.material_texture_name(MAP_ZONE, mat_id, TextureSemantic::Normal),
                );
                // Remastered textures (`crate::upscaled`): a normal map made
                // from the colour where CoD4 has none, and heights for
                // parallax on big flat opaque surfaces.
                let upscaled = crate::upscaled::on();
                let colour_name = content.material_texture_name(MAP_ZONE, mat_id, TextureSemantic::Color).map(|n| n.trim_start_matches(',').to_owned());
                let normal_name = normal_map
                    .as_ref()
                    .and(content.material_texture_name(MAP_ZONE, mat_id, TextureSemantic::Normal))
                    .map(|n| n.trim_start_matches(',').to_owned());
                if upscaled && normal_map.is_none() {
                    normal_map = colour_name.as_ref().and_then(|c| content.image(&format!("{c}.gen_n"), false, images));
                }
                let flat = upscaled
                    && normal_map.is_some()
                    && base.alpha_mode == AlphaMode::Opaque
                    && crate::upscaled::parallax_depth() > 0.0
                    && crate::upscaled::large_flat(content.map().gfx_world().expect("gfxworld"), &surfs);
                let upscaled_height = if flat { normal_name.as_ref().or(colour_name.as_ref()).and_then(|n| content.image(&format!("{n}.height"), false, images)) } else { None };
                let relief = if upscaled_height.is_some() { crate::upscaled::parallax_depth() } else { 0.0 };
                // Opaque surfaces only: the prepass doesn't shift the UVs,
                // so an alpha-tested edge would disagree between the passes
                // (the sky colour showed along cables' edges).
                let height_map = upscaled_height.or_else(|| {
                    (normal_map.is_some() && base.alpha_mode == AlphaMode::Opaque && crate::atmos::climate::showcase())
                        .then(|| content.material_texture_name(MAP_ZONE, mat_id, TextureSemantic::Normal))
                        .flatten()
                        .and_then(|name| crate::pom::height_map(&content.vfs.clone(), &name, images))
                });
                let specular_map = real(
                    content.material_texture(MAP_ZONE, mat_id, TextureSemantic::Specular, true, images),
                    content.material_texture_name(MAP_ZONE, mat_id, TextureSemantic::Specular),
                );
                let mut params = WorldParams {
                    flags: Vec4::new(
                        normal_map.is_some() as u32 as f32,
                        specular_map.is_some() as u32 as f32,
                        lightmap.is_some() as u32 as f32,
                        crate::lightmaps::LIGHTMAP_EXPOSURE,
                    ),
                    probe: Vec4::new(
                        // Debug aid: `COD4RW_NOPROBE` leaves the probes' reflections out.
                        (reflection_probe.is_some() && std::env::var_os("COD4RW_NOPROBE").is_none()) as u32 as f32,
                        reflection_probe
                            .as_ref()
                            .and_then(|h| images.get(h))
                            .map_or(0.0, |i| i.texture_descriptor.mip_level_count.saturating_sub(1) as f32),
                        // Wet surfaces' sky (`sky` below) in place of a probe.
                        (reflection_probe.is_none() && sky.is_some()) as u32 as f32,
                        // MW2's normal maps (`crate::mw2`) are unit vectors' x
                        // and y, not CoD4's slopes.
                        crate::mw2::active() as u32 as f32,
                    ),
                    ..default()
                };
                let zone_mat = content.map().material(mat_id);
                let technique_set = zone_mat
                    .and_then(|m| m.technique_set)
                    .and_then(|t| content.map().technique_set(t))
                    .map(|t| t.name.as_str())
                    .unwrap_or("");
                WorldLighting::falloff(zone_mat, technique_set, &mut params);
                // The material-class layer (`crate::material_class`), not on
                // fake beams, portals or water.
                if params.falloff_parms.z == 0.0 && !technique_set.starts_with("wc_water") && base.alpha_mode != AlphaMode::Add {
                    params.surface = crate::material_class::surface(zone_mat.map_or(0, |m| m.surface_type_bits));
                }
                params.probe_norm = probe_norm();
                // Re-baked lightmaps are linear, their direction a unit vector.
                params.traced.z = (rebaked && lightmap.is_some()) as u32 as f32;
                // Remastered parallax depth (`relief_uv`; 0: the showcase's).
                params.traced.w = relief;
                if reflection_probe.is_some() && crate::atmos::climate::showcase() {
                    if let Some(p) = content.map().gfx_world().and_then(|w| w.reflection_probes.get(probe_index as usize)) {
                        params.probe_center = units::pos(p.origin).extend(1.0);
                    }
                }
                if cod4_sun() {
                    let sun = &content.map().gfx_world().expect("gfxworld").sun;
                    params.sun_dir = to_sun(sun).extend(0.0);
                    params.sun_diffuse = live_sun(sun).extend(0.0);
                    params.traced.y = 1.0;
                }
                // With the whole sun live (`crate::lightmaps::full_sun`), no
                // rolloff on lit surfaces (knob `gi.linear`): it scaled a
                // sunlit floor by 0.66, so doubling the sun showed as +25%.
                if crate::tune::get("gi.linear", if crate::lightmaps::full_sun() { 1.0 } else { 0.0 }) > 0.5 {
                    // (Negative: ray-traced lighting's deferred path uses
                    // `traced.x` for its floor, always positive.)
                    params.traced.x = -1.0;
                }
                if technique_set.starts_with("wc_water") {
                    WorldLighting::water(zone_mat, content.map().gfx_world().expect("gfxworld"), &mut params);
                    // Water's screen-space reflections, where `crate::ssr`
                    // fills their history (the showcase). (Water leaves the
                    // shader early, so `probe.z` is free for this there.)
                    params.probe.z = crate::atmos::climate::on() as u32 as f32;
                }
                let detail_map = content.material_detail(MAP_ZONE, mat_id, images).map(|(h, scale)| {
                    let mips = images.get(&h).map_or(1, |i| i.texture_descriptor.mip_level_count);
                    params.detail = Vec4::new(scale.x, scale.y, 1.0, mips.saturating_sub(1) as f32);
                    h
                });
                // The surfaces' primary light (spot or omni; the sun's
                // handled above), CoD4's run-time light on its lightmap.
                let mut primary_map = None;
                if lights_on && lightmap.is_some() {
                    let light = content.map().com_world().and_then(|w| w.primary_lights.get(plight as usize)).filter(|l| matches!(l.kind, 2 | 3) && l.radius > 0.0);
                    if let Some(l) = light {
                        primary_map = primaries.get(lightmap_index as usize).cloned().flatten();
                        let k = crate::tune::get("worldlights.scale", 1.0);
                        params.plight_pos = units::pos(l.origin).extend(1.0 / units::u(l.radius).max(0.01));
                        params.plight_color = (Vec3::from(l.color) * k).extend(1.0);
                        // (Its light def's falloff, by name: 1 none, 2 linear, 3
                        // quadratic, 0 a smooth curve; with the exponent.)
                        let def = l.def_name.as_deref().unwrap_or("");
                        let falloff = if def.contains("no_falloff") { 1.0 } else if def.contains("linear") { 2.0 } else if def.contains("quadratic") { 3.0 } else { 0.0 };
                        params.plight_dir = (-units::dir(l.dir)).normalize_or(Vec3::NEG_Y).extend(l.exponent as f32 + 100.0 * falloff);
                        let span = (l.cos_half_fov_inner - l.cos_half_fov_outer).max(1e-3);
                        params.plight_spot = Vec4::new(1.0 / span, -l.cos_half_fov_outer / span, (l.kind == 2) as u32 as f32, primary_map.is_some() as u32 as f32);
                    }
                }
                let extension = WorldLighting {
                    params,
                    normal_map,
                    specular_map,
                    lightmap: lightmap.clone(),
                    reflection_probe: reflection_probe.or_else(|| sky.clone()),
                    detail_map,
                    wet_map: crate::wet::wet_map(),
                    ssr_history: crate::ssr::history(),
                    height_map,
                    primary_map,
                };
                let handle = world_materials.add(WorldMaterial { base, extension });
                world_mats.insert((mat_id, lightmap_index, probe_index, plight), handle.clone());
                handle
            }
        };
        let world = content.map().gfx_world().expect("gfxworld");
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut tangents = Vec::new();
        let mut uvs = Vec::new();
        let mut lightmap_uvs = Vec::new();
        let mut colors = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        // Surfaces index into shared vertex ranges, so copy only the vertices
        // actually referenced and remap the indices.
        let mut remap: HashMap<u32, u32> = HashMap::new();
        for &si in &surfs {
            let s = &world.surfaces[si];
            let first = s.first_vertex.max(0) as u32;
            let start = s.base_index.max(0) as usize;
            let end = start + s.tri_count as usize * 3;
            if end > world.indices.len() {
                continue;
            }
            for tri in world.indices[start..end].chunks_exact(3) {
                let global = [first + tri[0] as u32, first + tri[1] as u32, first + tri[2] as u32];
                if global.iter().any(|&g| g as usize >= world.vertices.len()) {
                    continue;
                }
                // CoD winds front faces clockwise; Bevy expects counter-clockwise.
                // (A moving one's about its own origin: its entity places it.)
                let (at, turn) = if model != 0 { (Vec3::ZERO, Quat::IDENTITY) } else { placed[surf_model[si] as usize].unwrap_or((Vec3::ZERO, Quat::IDENTITY)) };
                for g in [global[0], global[2], global[1]] {
                    let local = *remap.entry(g).or_insert_with(|| {
                        let v = &world.vertices[g as usize];
                        positions.push((at + turn * units::pos(v.xyz)).to_array());
                        normals.push((turn * units::dir(iw3::unpack::unit_vec(v.normal))).normalize_or(Vec3::Y).to_array());
                        // IW3's binormal is cross(normal, tangent) * sign, as in Bevy.
                        let t = (turn * units::dir(iw3::unpack::unit_vec(v.tangent))).normalize_or(Vec3::X);
                        tangents.push([t.x, t.y, t.z, if v.binormal_sign < 0.0 { -1.0 } else { 1.0 }]);
                        uvs.push(v.tex_coord);
                        lightmap_uvs.push(v.lmap_coord);
                        colors.push(vertex_color(v.color));
                        positions.len() as u32 - 1
                    });
                    indices.push(local);
                }
            }
        }
        if indices.is_empty() {
            continue;
        }
        total_tris += indices.len() / 3;
        draws += 1;
        // The map's sea (`wc_water`): where it lies, for the showcase ocean
        // that takes its place (`crate::ocean`).
        let water = content.technique_set(MAP_ZONE, mat_id).starts_with("wc_water").then(|| {
            let (lo, hi) = positions.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(Vec3::from(*p)), hi.max(Vec3::from(*p))));
            MapWater { min: lo, max: hi }
        });
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, lightmap_uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_indices(Indices::U32(indices));
        let name = content.map().material(mat_id).map(|m| m.name.clone()).unwrap_or_default();
        // Opaque surfaces cast through the proxy; translucent ones (glass,
        // decals, light beams) don't block the sun; alpha-tested ones keep
        // casting themselves for their cutouts.
        let alpha_mode = world_materials.get(&material).map_or(AlphaMode::Opaque, |m| m.base.alpha_mode);
        let casts_itself = matches!(alpha_mode, AlphaMode::Mask(_));
        if alpha_mode == AlphaMode::Opaque && model == 0 {
            let base = proxy_positions.len() as u32;
            if let (Some(bevy::mesh::VertexAttributeValues::Float32x3(p)), Some(Indices::U32(idx))) =
                (mesh.attribute(Mesh::ATTRIBUTE_POSITION), mesh.indices())
            {
                proxy_positions.extend_from_slice(p);
                proxy_indices.extend(idx.iter().map(|i| base + i));
            }
        }
        let parent = parts.get(&model).copied().unwrap_or(root);
        let mut e = commands.spawn((Name::new(name), Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), ChildOf(parent)));
        if let Some(w) = water {
            e.insert(w);
        }
        if !casts_itself && model == 0 {
            e.insert(NotShadowCaster);
        }
        if let Some(image) = lightmap {
            // Bevy's lightmap only where its sun must leave the world alone
            // (the cod4 sun trial) or for comparing (`COD4RW_BEVYLIGHTMAP`):
            // the world shader samples its lightmap itself, and Bevy's
            // rebuilt a bind group per lightmap and phase every frame.
            if cod4_sun() || std::env::var_os("COD4RW_BEVYLIGHTMAP").is_some() {
                e.insert(Lightmap { image, uv_rect: Rect::new(0.0, 0.0, 1.0, 1.0), bicubic_sampling: false });
            }
        }
    }
    if !proxy_indices.is_empty() {
        let material =
            std_materials.add(StandardMaterial { unlit: true, cull_mode: None, ..default() });
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
        let normals = vec![[0.0, 1.0, 0.0]; proxy_positions.len()];
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, proxy_positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_indices(Indices::U32(proxy_indices));
        commands.spawn((
            Name::new("world shadow caster"),
            Mesh3d(crate::mesh_bounds::add(meshes, mesh)),
            MeshMaterial3d(material),
            RenderLayers::layer(SHADOW_PROXY_LAYER),
            ChildOf(root),
        ));
    }
    info!("world geometry: {total_tris} triangles in {draws} draws");
}

/// Alpha-tested static models (foliage) on the world shader for its
/// distance cutoff; `COD4RW_FOLIAGEWORLD=0` puts them back on their plain
/// material (to time it).
fn foliage_world() -> bool {
    !std::env::var("COD4RW_FOLIAGEWORLD").is_ok_and(|v| v == "0")
}

/// Reflection probe normalisation (`world.wgsl`), read at map load:
/// `COD4RW_PROBENORM=0` off; knobs `probe.normalize` (strength 0..1),
/// `probe.norm_k` (calibration), `probe.norm_max` (the most it may
/// brighten: 1, it only dims where less light arrives than the probe saw).
pub fn probe_norm() -> Vec4 {
    let on = !std::env::var("COD4RW_PROBENORM").is_ok_and(|v| v == "0");
    Vec4::new(crate::tune::get("probe.normalize", 1.0), on as u32 as f32, crate::tune::get("probe.norm_k", 1.0), crate::tune::get("probe.norm_max", 1.0))
}

/// Static models drawn as the world's surfaces are (CoD4's normal and
/// specular maps, gloss to roughness, the nearest probe's reflection)
/// rather than plain colour. `COD4RW_PROPMATS=0` for the flat look.
pub fn prop_materials() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("COD4RW_PROPMATS").is_ok_and(|v| v == "0"))
}

/// A static model material on the world shader: its CoD4 normal and
/// specular maps, reflecting `probe` (with a specular map, as IW3's model
/// techniques did), and the showcase's rain and sky as `crate::wet`'s.
pub(crate) fn prop_material(
    base: &StandardMaterial,
    (normal_map, specular_map): (Option<Handle<Image>>, Option<Handle<Image>>),
    surface_bits: u32,
    probe: Option<&(Vec3, Handle<Image>, f32)>,
    sky: Option<Handle<Image>>,
    world_materials: &mut Assets<WorldMaterial>,
) -> Handle<WorldMaterial> {
    let params = WorldParams {
        flags: Vec4::new(normal_map.is_some() as u32 as f32, specular_map.is_some() as u32 as f32, 0.0, crate::lightmaps::LIGHTMAP_EXPOSURE),
        probe: Vec4::new(probe.is_some() as u32 as f32, probe.map_or(0.0, |p| p.2), (probe.is_none() && sky.is_some()) as u32 as f32, crate::mw2::active() as u32 as f32),
        surface: crate::material_class::surface(surface_bits),
        probe_norm: probe_norm(),
        ..default()
    };
    world_materials.add(WorldMaterial {
        base: base.clone(),
        extension: WorldLighting {
            params,
            normal_map,
            specular_map,
            reflection_probe: probe.map(|p| p.1.clone()).or(sky),
            wet_map: crate::wet::wet_map(),
            ssr_history: crate::ssr::history(),
            ..default()
        },
    })
}

fn spawn_static_models(
    commands: &mut Commands,
    content: &mut Content,
    meshes: &mut Assets<Mesh>,
    std_materials: &mut Assets<StandardMaterial>,
    world_materials: &mut Assets<WorldMaterial>,
    images: &mut Assets<Image>,
    probes: &[(Vec3, Handle<Image>, f32)],
) {
    let root = commands.spawn((Name::new("static models"), Transform::default(), Visibility::default())).id();
    // Fake light beams and flares (`*_falloff_*`) fade as they turn edge-on,
    // as the world's do: the world material, with nothing else of it.
    let mut falloff_mats: HashMap<(usize, AssetId), Option<Handle<WorldMaterial>>> = HashMap::new();
    let instances: Vec<_> = content.map().gfx_world().expect("gfxworld").static_models.clone();
    let mut batches: HashMap<(BatchMaterial, IVec3, u32), Vec<(Handle<Mesh>, Transform)>> = HashMap::new();
    // Each plain batch material's CoD4 one, for its maps (`prop_material`).
    let mut zone_mats: HashMap<bevy::asset::AssetId<StandardMaterial>, (usize, AssetId)> = HashMap::new();
    let mut logged = std::collections::HashSet::new();
    // Models drawn with surfaces missing, or not at all (their collision
    // still stands): name -> (instances, surfaces missed, why).
    let mut missed: std::collections::BTreeMap<String, (usize, usize, &'static str)> = Default::default();
    for sm in &instances {
        let Some(model_id) = sm.model else { continue };
        let (zi, model_id) = content.resolve_xmodel(MAP_ZONE, model_id);
        let Some(xm) = content.zone(zi).xmodel(model_id) else {
            missed.entry(format!("{model_id:?}")).or_insert((0, 0, "no model")).0 += 1;
            continue;
        };
        let Some(lod) = xm.lods.first().copied() else {
            missed.entry(xm.name.clone()).or_insert((0, 0, "no lod")).0 += 1;
            continue;
        };
        let name = xm.name.clone();
        // (Left out with the map editor, `crate::map_editor`.)
        if crate::map_editor::layout::hidden(&name, sm.origin) {
            continue;
        }
        let mats: Vec<Option<AssetId>> = xm.materials.clone();
        let transform = Transform {
            translation: units::pos(sm.origin),
            rotation: units::axis_rotation(sm.axis),
            scale: Vec3::splat(sm.scale),
        };
        // CoD4's own draw distance for the model (0: always), bucketed so
        // models with near distances share batches.
        let cull = if sm.cull_dist > 0.0 { (units::u(sm.cull_dist) / CULL_STEP).ceil() as u32 } else { 0 };
        let cell = (transform.translation / batch_cell()).floor().as_ivec3();
        let first = lod.surf_index as usize;
        let miss = |why: &'static str, missed: &mut std::collections::BTreeMap<String, (usize, usize, &'static str)>| {
            missed.entry(name.clone()).or_insert((0, 0, why)).1 += 1;
        };
        for surf in first..first + lod.num_surfs as usize {
            let Some(mat_id) = mats.get(surf).copied().flatten() else {
                miss("no material id", &mut missed);
                continue;
            };
            let Some(mat) = content.material(zi, mat_id, std_materials, images) else {
                // Shadow-only planes (`*_shadowcaster`) are left out on purpose.
                if !content.technique_set(zi, mat_id).ends_with("shadowcaster") {
                    miss("material failed", &mut missed);
                }
                continue;
            };
            let Some(mesh) = content.static_mesh(zi, model_id, surf, meshes) else {
                miss("mesh failed", &mut missed);
                continue;
            };
            let falloff = falloff_mats
                .entry((zi, mat_id))
                .or_insert_with(|| {
                    let zone_mat = content.zone(zi).material(mat_id);
                    let mut params = WorldParams::default();
                    WorldLighting::falloff(zone_mat, content.technique_set(zi, mat_id), &mut params);
                    let base = std_materials.get(&mat.handle).cloned()?;
                    (params.falloff_parms.z != 0.0)
                        .then(|| world_materials.add(WorldMaterial { base, extension: WorldLighting { params, ..default() } }))
                })
                .clone();
            let material = match falloff {
                Some(m) => BatchMaterial::World(m),
                None => BatchMaterial::Standard(mat.handle.clone()),
            };
            zone_mats.entry(mat.handle.id()).or_insert((zi, mat_id));
            if std::env::var("COD4RW_BATCHLOG").is_ok_and(|v| !v.is_empty() && name.contains(&v)) {
                info!("batchlog: {name} surf {surf} -> cell {cell} cull {cull} material {:?}", match &material { BatchMaterial::World(m) => m.id().to_string(), BatchMaterial::Standard(m) => m.id().to_string() });
                logged.insert((material.clone(), cell, cull));
            }
            batches.entry((material, cell, cull)).or_default().push((mesh, transform));
        }
    }
    // Static models merged into one mesh per material, cell and draw
    // distance: thousands of entities became a few hundred draws, which the
    // render thread (extraction, bind groups, instance buffers) was bound by.
    // They're lit per pixel from the light grid, so merging changes nothing.
    let mut entities = 0;
    // Debug aid: `COD4RW_NOBATCH` spawns each model surface on its own, as
    // before batching, for comparing.
    if std::env::var_os("COD4RW_NOBATCH").is_some() {
        for ((material, _, _), parts) in batches {
            for (mesh, transform) in parts {
                let mut e = commands.spawn((Mesh3d(mesh), transform, ChildOf(root)));
                match &material {
                    BatchMaterial::World(m) => e.insert((MeshMaterial3d(m.clone()), NotShadowCaster)),
                    BatchMaterial::Standard(m) => e.insert(MeshMaterial3d(m.clone())),
                };
                entities += 1;
            }
        }
        info!("static models: {} placed, {entities} surfaces unbatched", instances.len());
        return;
    }
    // Opaque ones cast the sun's shadow through one merged mesh, as the
    // world does (each batch drawn again in every cascade cost the render
    // thread more than the models themselves); alpha-tested ones (foliage)
    // keep casting their own for their cutouts.
    let mut proxy: Vec<(Handle<Mesh>, Transform)> = Vec::new();
    let mut wet_materials: std::collections::HashMap<(bevy::asset::AssetId<StandardMaterial>, Option<usize>), Handle<WorldMaterial>> = Default::default();
    let mut prop_mats: std::collections::HashMap<(bevy::asset::AssetId<StandardMaterial>, Option<usize>), Handle<WorldMaterial>> = Default::default();
    let sky = (crate::atmos::climate::showcase() || prop_materials()).then(|| content.sky_cube(images)).flatten();
    for ((material, _cell, cull), parts) in batches {
        // The showcase reflects each batch's nearest probe (box-corrected,
        // `crate::probe_parallax`), not just the sky.
        let middle = parts.iter().map(|(_, tf)| tf.translation).sum::<Vec3>() / parts.len().max(1) as f32;
        let probe = (crate::atmos::climate::wet_showcase() || prop_materials())
            .then(|| probes.iter().enumerate().min_by(|a, b| a.1.0.distance_squared(middle).total_cmp(&b.1.0.distance_squared(middle))))
            .flatten();
        let opaque = matches!(&material, BatchMaterial::Standard(m) if std_materials.get(m).is_some_and(|m| m.alpha_mode == AlphaMode::Opaque));
        if opaque && std::env::var_os("COD4RW_NOBATCH").is_none() {
            proxy.extend(parts.iter().cloned());
        }
        // Meshes already handed to the renderer (a model the props or the
        // clutter drew first) can't be read to merge: each its own then.
        // (Skipped, they stood invisible but cast the proxy's shadow: MW2
        // Terminal's tables and plane seats.)
        let merged = merge_meshes(&parts, meshes);
        if logged.contains(&(material.clone(), _cell, cull)) {
            info!("batchlog: batch of {} parts, alpha {:?}, merged {:?}", parts.len(), match &material { BatchMaterial::Standard(m) => std_materials.get(m).map(|m| m.alpha_mode), _ => None }, merged.as_ref().map(|m| bevy::camera::primitives::MeshAabb::compute_aabb(m)));
        }
        let spawned: Vec<Entity> = match merged {
            // (Bounds noted while the vertices are here: worked out later,
            // after the mesh went to the GPU, there were none, and the draw
            // distance check hid the batch: MW2 Terminal's tables.)
            Some(mesh) => {
                let (mesh, centre) = recentre(mesh);
                vec![commands.spawn((Name::new("static models batch"), Mesh3d(crate::mesh_bounds::add(meshes, mesh)), Transform::from_translation(centre), ChildOf(root))).id()]
            }
            None => parts.iter().map(|(mesh, tf)| commands.spawn((Name::new("static model"), Mesh3d(mesh.clone()), *tf, ChildOf(root))).id()).collect(),
        };
        for id in spawned {
        let mut e = commands.entity(id);
        match material.clone() {
            BatchMaterial::World(m) => e.insert((MeshMaterial3d(m), NotShadowCaster)),
            // The showcase's rain wets them too (`crate::wet`).
            // (Alpha-tested ones always take the world shader: its cutoff
            // drops with distance, so grass and leaves keep their cover far
            // off rather than thinning to see-through cards.)
            // CoD4's maps on props (`prop_material`), the rain's wetness too.
            BatchMaterial::Standard(m) if prop_materials() && zone_mats.contains_key(&m.id()) && std_materials.get(&m).is_some() => {
                let probe_key = probe.map(|(i, _)| i);
                let prop = match prop_mats.get(&(m.id(), probe_key)) {
                    Some(h) => h.clone(),
                    None => {
                        let (zi, id) = zone_mats[&m.id()];
                        let maps = content.model_maps(zi, id, images);
                        let bits = content.material_surface_bits(zi, id);
                        let h = prop_material(std_materials.get(&m).expect("checked"), maps, bits, probe.map(|(_, p)| p), sky.clone(), world_materials);
                        prop_mats.insert((m.id(), probe_key), h.clone());
                        h
                    }
                };
                e.insert(MeshMaterial3d(prop))
            }
            BatchMaterial::Standard(m)
                if crate::atmos::climate::wet_showcase()
                    || (foliage_world() && std_materials.get(&m).is_some_and(|m| matches!(m.alpha_mode, AlphaMode::Mask(_)))) =>
            {
                let probe_key = probe.map(|(i, _)| i);
                let wet = wet_materials
                    .entry((m.id(), probe_key))
                    .or_insert_with(|| crate::wet::wet_material(std_materials.get(&m), sky.clone(), probe.map(|(_, p)| p), world_materials))
                    .clone();
                e.insert(MeshMaterial3d(wet))
            }
            BatchMaterial::Standard(m) => e.insert(MeshMaterial3d(m)),
        };
        if opaque {
            e.insert((NotShadowCaster, ShadowViaProxy));
        }
        if cull > 0 {
            let end = cull as f32 * CULL_STEP;
            e.insert(VisibilityRange { start_margin: 0.0..0.0, end_margin: end..end + CULL_STEP, use_aabb: true });
        }
        entities += 1;
        }
    }
    // (Only what can be read: one model handed to the renderer already
    // would otherwise cost every static model its shadow.)
    proxy.retain(|(h, _)| {
        meshes.get(h).is_some_and(|m| {
            m.try_attribute_option(Mesh::ATTRIBUTE_POSITION).is_ok_and(|p| p.is_some())
                && m.attribute(Mesh::ATTRIBUTE_NORMAL).is_some()
                && m.attribute(Mesh::ATTRIBUTE_UV_0).is_some()
        })
    });
    if let Some(mesh) = merge_meshes(&proxy, meshes) {
        let material = std_materials.add(StandardMaterial { unlit: true, cull_mode: None, ..default() });
        commands.spawn((
            Name::new("static models shadow caster"),
            Mesh3d(crate::mesh_bounds::add(meshes, mesh)),
            MeshMaterial3d(material),
            Transform::default(),
            RenderLayers::layer(SHADOW_PROXY_LAYER),
            ChildOf(root),
        ));
    }
    info!("static models: {} placed, drawn in {entities} batches", instances.len());
    for (name, (instances, surfaces, why)) in &missed {
        warn!("static model {name}: {surfaces} surfaces not drawn ({why}){}", if *instances > 0 { format!(", {instances} placed not drawn") } else { String::new() });
    }
}

/// An opaque static models batch: the sun's shadow is the merged "static
/// models shadow caster"'s, so it never casts its own (the model shadows
/// setting turns that proxy on and off instead).
#[derive(Component)]
pub struct ShadowViaProxy;

/// Static models are batched per this many metres each way.
const BATCH_CELL: f32 = 32.0;

/// [`BATCH_CELL`], or `COD4RW_BATCHCELL` (metres) for comparing.
fn batch_cell() -> f32 {
    std::env::var("COD4RW_BATCHCELL").ok().and_then(|v| v.parse().ok()).unwrap_or(BATCH_CELL)
}

/// Static models' draw distances are rounded up to this many metres.
const CULL_STEP: f32 = 16.0;

#[derive(Clone, PartialEq, Eq, Hash)]
enum BatchMaterial {
    Standard(Handle<StandardMaterial>),
    World(Handle<WorldMaterial>),
}

/// The meshes, placed by their transforms, as one: positions, normals, UVs
/// and (if all have them) colours. `None` if none could be read.
pub(crate) fn merge_meshes(parts: &[(Handle<Mesh>, Transform)], meshes: &Assets<Mesh>) -> Option<Mesh> {
    // Meshes already extracted to the render world can't be read: left out.
    let read: Vec<(&Mesh, &Transform)> =
        parts.iter().filter_map(|(h, t)| meshes.get(h).filter(|m| m.try_attribute_option(Mesh::ATTRIBUTE_POSITION).is_ok()).map(|m| (m, t))).collect();
    if read.len() < parts.len() {
        return None;
    }
    merge_mesh_data(&read)
}

/// `mesh` moved to stand around its own origin, and where that was.
///
/// Bevy's draw distance fade measures from a mesh's origin, not its bounds:
/// a merged batch left at the world's origin faded out wherever that was
/// further off than its draw distance (MW2 Terminal's escape slide).
fn recentre(mut mesh: Mesh) -> (Mesh, Vec3) {
    use bevy::mesh::VertexAttributeValues as V;
    let Some(V::Float32x3(pos)) = mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION) else { return (mesh, Vec3::ZERO) };
    let (lo, hi) = pos.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(Vec3::from(*p)), hi.max(Vec3::from(*p))));
    if lo.x > hi.x {
        return (mesh, Vec3::ZERO);
    }
    let centre = (lo + hi) * 0.5;
    for p in pos.iter_mut() {
        *p = (Vec3::from(*p) - centre).to_array();
    }
    (mesh, centre)
}

/// [`merge_meshes`] of meshes in hand.
pub(crate) fn merge_mesh_data(read: &[(&Mesh, &Transform)]) -> Option<Mesh> {
    use bevy::mesh::VertexAttributeValues as V;
    let read = read.to_vec();
    let colours = read.iter().all(|(m, _)| m.attribute(Mesh::ATTRIBUTE_COLOR).is_some());
    // (Tangents for the props' normal maps, `prop_material`.)
    let tangents = read.iter().all(|(m, _)| m.attribute(Mesh::ATTRIBUTE_TANGENT).is_some());
    let mut tan: Vec<[f32; 4]> = Vec::new();
    let (mut pos, mut nrm, mut uv, mut col, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::<u32>::new());
    for (m, t) in read {
        let (Some(V::Float32x3(p)), Some(V::Float32x3(n)), Some(V::Float32x2(u))) =
            (m.attribute(Mesh::ATTRIBUTE_POSITION), m.attribute(Mesh::ATTRIBUTE_NORMAL), m.attribute(Mesh::ATTRIBUTE_UV_0))
        else {
            // (Not merged without them: the callers draw the parts one by
            // one instead, rather than lose this one.)
            return None;
        };
        let base = pos.len() as u32;
        let matrix = t.to_matrix();
        pos.extend(p.iter().map(|v| matrix.transform_point3(Vec3::from(*v)).to_array()));
        nrm.extend(n.iter().map(|v| (t.rotation * Vec3::from(*v)).to_array()));
        uv.extend_from_slice(u);
        if tangents {
            if let Some(V::Float32x4(tv)) = m.attribute(Mesh::ATTRIBUTE_TANGENT) {
                tan.extend(tv.iter().map(|v| (t.rotation * Vec3::new(v[0], v[1], v[2])).extend(v[3]).to_array()));
            }
        }
        if colours {
            if let Some(V::Float32x4(c)) = m.attribute(Mesh::ATTRIBUTE_COLOR) {
                col.extend_from_slice(c);
            }
        }
        match m.indices() {
            Some(i) => idx.extend(i.iter().map(|i| base + i as u32)),
            None => idx.extend(base..pos.len() as u32),
        }
    }
    if idx.is_empty() {
        return None;
    }
    let mut out = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    out.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    out.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    out.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    if colours && !col.is_empty() {
        out.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    }
    if tangents && tan.len() == out.count_vertices() {
        out.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tan);
    }
    out.insert_indices(Indices::U32(idx));
    Some(out)
}

/// How much of the map's flat ambient light models still get when the light
/// grid lights them: a floor for anything outside the grid's volume.
const AMBIENT_WITH_LIGHT_GRID: f32 = 0.15;

/// The sun's illuminance (lux) for a `sunlight` of 1.
pub const SUN_ILLUMINANCE: f32 = 8_000.0;

/// Trial (`COD4RW_SUNMODEL=cod4`): light the world as CoD4's `lm_sm_sun`
/// shaders do. The live sun is `sunColor * (sunLight - ambientScale) *
/// (1 - diffuseFraction)` (the rest is baked into the lightmaps), added to
/// the lightmap's light in gamma space and shadowed by the sun's shadow map,
/// before the sum is linearised (`shaders/world.wgsl`); Bevy's sun then
/// lights only models, at the matching strength.
pub fn cod4_sun() -> bool {
    std::env::var("COD4RW_SUNMODEL").is_ok_and(|v| v.eq_ignore_ascii_case("cod4"))
}

/// The sun as strong as CoD4's live sun ([`live_sun`]), not its
/// `sunLight`, which includes the part already baked into the lightmaps
/// (with that, shade came out too dark beside the sunlit ground). The
/// default; `COD4RW_SUNMODEL=now` for the older, stronger sun.
fn live_sun_model() -> bool {
    !std::env::var("COD4RW_SUNMODEL").is_ok_and(|v| v.eq_ignore_ascii_case("now"))
}

/// The sun's illuminance (lux) for a live sun ([`live_sun`]) of 1.
const LIVE_SUN_ILLUMINANCE: f32 = 12_000.0;

/// CoD4's live sun (gamma-space colour, see [`cod4_sun`]).
fn live_sun(sun: &zone::SunParse) -> Vec3 {
    Vec3::from(sun.sun_color) * ((sun.sun_light - sun.ambient_scale) * (1.0 - sun.diffuse_fraction)).max(0.0)
}

/// The direction to the sun, Bevy space.
fn to_sun(sun: &zone::SunParse) -> Vec3 {
    let (pitch, yaw) = (sun.angles[0].to_radians(), sun.angles[1].to_radians());
    units::dir([pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), -pitch.sin()]).normalize()
}

fn spawn_sun(commands: &mut Commands, world: &zone::GfxWorld, light_grid: bool) {
    let sun = &world.sun;
    // `sundirection` is pitch/yaw of the direction towards the sun.
    let (pitch, yaw) = (sun.angles[0].to_radians(), sun.angles[1].to_radians());
    let to_sun_cod = [pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), -pitch.sin()];
    let to_sun = units::dir(to_sun_cod).normalize();
    let c = sun.sun_color;
    commands.spawn((
        Name::new("sun"),
        DirectionalLight {
            color: Color::linear_rgb(c[0], c[1], c[2]),
            // Against the lightmaps' 3000 (`crate::lightmaps`): at 12000 shade
            // came out half as bright as CoD4's against sunlit ground, which
            // on snow and overcast maps (Bloc) left black shade and white snow.
            illuminance: if cod4_sun() {
                // For models: what the world's gamma-space sum adds over a
                // typical baked 0.3, in the lightmaps' units (lux = pi * nits).
                let live = live_sun(sun).max_element();
                std::f32::consts::PI * crate::lightmaps::LIGHTMAP_EXPOSURE * ((0.3 + live).powf(2.2) - 0.3f32.powf(2.2)) / live.max(1e-3) * live
            } else if live_sun_model() && crate::lightmaps::full_sun() {
                // A live-sun bake (`crate::bake::livesun`): all of the sun.
                LIVE_SUN_ILLUMINANCE * (Vec3::from(sun.sun_color) * (sun.sun_light - sun.ambient_scale).max(0.0)).max_element()
            } else if live_sun_model() {
                LIVE_SUN_ILLUMINANCE * live_sun(sun).max_element()
            } else {
                SUN_ILLUMINANCE * sun.sun_light.max(0.5)
            },
            affects_lightmapped_mesh_diffuse: !cod4_sun(),
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder { maximum_distance: 120.0, first_cascade_far_bound: 12.0, ..default() }.build(),
        Transform::default().looking_to(-to_sun, Vec3::Y),
        // The default layer plus the world's shadow proxy, and the
        // splitscreen players' bodies (for their shadows).
        RenderLayers::from_layers(
            &[0, SHADOW_PROXY_LAYER]
                .into_iter()
                .chain((0..crate::splitscreen::MAX_PLAYERS).map(crate::splitscreen::body_layer))
                .chain((0..crate::splitscreen::MAX_PLAYERS).map(crate::first_person::body::layer))
                .collect::<Vec<_>>(),
        ),
    ));
    // District's is [0.74, 0.68, 56]: a typo CoD4 never shows (its models
    // take their light from the grid), which here turned every model blue.
    // A colour past 1 is the sun's instead.
    let a = if sun.ambient_color.iter().all(|v| (0.0..=1.0).contains(v)) { sun.ambient_color } else { c };
    commands.insert_resource(GlobalAmbientLight {
        color: Color::linear_rgb(a[0], a[1], a[2]),
        // At least 0.1: overcast maps (Bloc, Farm) set 0, which left models
        // outside the light grid (tree crowns) black.
        brightness: 2500.0 * sun.ambient_scale.max(0.1) * 4.0 * if light_grid { AMBIENT_WITH_LIGHT_GRID } else { 1.0 },
        // Lightmapped world surfaces get their indirect light from the lightmap.
        affects_lightmapped_meshes: false,
        ..default()
    });
}

fn read_spawns(ents: &[iw3::ents::Entity]) -> Vec<SpawnPoint> {
    ents.iter()
        .filter_map(|e| {
            let kind = match e.classname() {
                "mp_tdm_spawn" => SpawnKind::Tdm,
                "mp_tdm_spawn_allies_start" => SpawnKind::AlliesStart,
                "mp_tdm_spawn_axis_start" => SpawnKind::AxisStart,
                "mp_dm_spawn" => SpawnKind::Dm,
                "mp_dom_spawn" => SpawnKind::Dom,
                "mp_dom_spawn_allies_start" => SpawnKind::DomAlliesStart,
                "mp_dom_spawn_axis_start" => SpawnKind::DomAxisStart,
                "mp_sd_spawn_attacker" => SpawnKind::SdAttacker,
                "mp_sd_spawn_defender" => SpawnKind::SdDefender,
                "mp_sab_spawn_allies" => SpawnKind::SabAllies,
                "mp_sab_spawn_axis" => SpawnKind::SabAxis,
                "mp_sab_spawn_allies_start" => SpawnKind::SabAlliesStart,
                "mp_sab_spawn_axis_start" => SpawnKind::SabAxisStart,
                _ => return None,
            };
            Some(SpawnPoint { pos: units::pos(e.origin()?), yaw: units::yaw_from_cod_degrees(e.angles()[1]), kind })
        })
        .collect()
}

/// Test switch `COD4RW_WORLDLIGHTS=1`: each world surface's primary light
/// (the map's spot and omni lights) added to its lightmap as CoD4 does at
/// run time, times its baked visibility (`lightmapN_primary`). Off by
/// default: an A/B for the decision.
pub fn world_lights() -> bool {
    // (And under the night grade, `crate::fire`: the practical lights
    // carry the scene while the baked light is turned down.)
    std::env::var("COD4RW_WORLDLIGHTS").is_ok_and(|v| v == "1") || crate::fire::night_on()
}

/// With `COD4RW_WORLDLIGHTS=1` and `COD4RW_SHOT_VIEWS` (each view listed
/// twice): the primary lights off for the even shots, on for the odd, so
/// one launch makes before/after pairs; logs each half's frame time.
fn world_lights_ab(
    time: Res<Time>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut exposures: Query<&mut bevy::post_process::auto_exposure::AutoExposure>,
    mut state: Local<Option<bool>>,
) {
    let late: f32 = std::env::var("COD4RW_SHOT_LATE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let t = time.elapsed_secs() - late;
    // (As `crate::autoshot`: view k from 5 s, 1.5 s each, shot at 6 + 1.5 k;
    // without views, its pairs at 6, 7, 8, 9 s.)
    let k = if std::env::var_os("COD4RW_SHOT_VIEWS").is_some() { (((t - 5.0) / 1.5).floor().max(0.0)) as usize } else { ((t - 5.5).floor().max(0.0)) as usize };
    let on = k % 2 == 1;
    if *state == Some(on) {
        return;
    }
    let ms = diagnostics.get(&bevy::diagnostic::FrameTimeDiagnosticsPlugin::FRAME_TIME).and_then(|d| d.average()).unwrap_or(0.0);
    if let Some(was) = *state {
        info!("worldlights: view {} {} frame {:.2} ms", k.saturating_sub(1), if was { "on" } else { "off" }, ms);
    }
    *state = Some(on);
    // (The pair at one exposure: it settles fast on the view while the
    // lights are off, and holds while they're on.)
    for mut e in &mut exposures {
        let speed = if on { 0.0 } else { 30.0 };
        e.speed_brighten = speed;
        e.speed_darken = speed;
    }
    let ids: Vec<_> = materials.ids().collect();
    for id in ids {
        let Some(m) = materials.get(id) else { continue };
        if m.extension.params.plight_spot.w == 0.0 && m.extension.params.plight_pos.w == 0.0 {
            continue;
        }
        if let Some(mut m) = materials.get_mut(id) {
            m.extension.params.plight_color.w = on as u32 as f32;
        }
    }
}
