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
#[bindless(index_table(range(100..116), binding(120)))]
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
        params.water_color = colour.truncate().extend(1.0);
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
    let vfs = Arc::new(iw3::iwd::Vfs::mount(&install.iwd_paths().expect("listing iwds")).expect("mounting iwds"));
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
    let mut content = Content::new(vec![load_zone(&map.0), load_zone("common_mp")], vfs);

    // Re-baked lighting (`crate::bake`) when wanted and cached, else CoD4's.
    let rebaked = crate::lightmaps::load_rebaked(&mut commands, &map.0, &install.zone_path(&map.0), &mut images);
    let lightmaps = match &rebaked {
        Some(r) => r.lightmaps.clone(),
        None => crate::lightmaps::load(content.map(), content.map().gfx_world().expect("zone has no GfxWorld"), &mut images),
    };
    let probes =
        crate::lightmaps::load_probes(content.map(), content.map().gfx_world().expect("zone has no GfxWorld"), &mut images);
    // Models (guns) reflect the nearest probe.
    let gfx = content.map().gfx_world().expect("zone has no GfxWorld");
    let placed = gfx.reflection_probes.iter().zip(&probes).skip(1).filter_map(|(p, h)| {
        let h = h.clone()?;
        let mip = images.get(&h).map_or(0.0, |i| i.texture_descriptor.mip_level_count.saturating_sub(1) as f32);
        Some((units::pos(p.origin), h, mip))
    });
    commands.insert_resource(ReflectionProbes(placed.collect()));
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
    spawn_static_models(&mut commands, &mut content, &mut meshes, &mut std_materials, &mut world_materials, &mut images);
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
            .filter(|e| e.classname() == "script_brushmodel" && e.get("script_gameobjectname").is_none())
            .filter_map(|e| {
                let n = e.get("model")?.strip_prefix('*')?.parse::<usize>().ok()?;
                Some((n, units::pos(e.origin().unwrap_or([0.0; 3])), crate::modes::koth::cod_rotation(e.angles())))
            })
            .collect();
        crate::collision::spawn_brush_entity_collision(&mut commands, clip, &places);
        crate::collision::spawn_static_model_collision(&mut commands, content.map(), clip);
    }
    let spawns =
        content.map().map_ents().map(|e| read_spawns(&iw3::ents::parse(&e.entity_string))).unwrap_or_default();
    info!("{} spawn points; map ready in {:?}", spawns.len(), t0.elapsed());
    commands.insert_resource(MapInfo { spawns });
    commands.insert_resource(content);
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
    for e in ents.iter().filter(|e| e.get("script_gameobjectname").is_none()) {
        let Some(n) = e.get("model").and_then(|m| m.strip_prefix('*')).and_then(|n| n.parse::<usize>().ok()) else { continue };
        if n == 0 || n >= count || out[n].is_some() {
            continue;
        }
        let origin = e.origin().unwrap_or([0.0; 3]);
        out[n] = Some((units::pos(origin), crate::modes::koth::cod_rotation(e.angles())));
    }
    out
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
    let mut groups: HashMap<(AssetId, u8, u8), Vec<usize>> = HashMap::new();
    for (i, s) in content.map().gfx_world().expect("gfxworld").surfaces.iter().enumerate() {
        if placed.get(surf_model[i] as usize).copied().flatten().is_none() {
            continue;
        }
        if let Some(m) = s.material {
            groups.entry((m, s.lightmap_index, s.reflection_probe_index)).or_default().push(i);
        }
    }
    // One group per material and lightmap, with the probe most of its
    // triangles use: split per probe the world was five times as many
    // materials (and draws in every view), which the render thread was
    // bound by; the probe only gives reflections, which barely differ
    // between neighbouring probes. `COD4RW_PROBESPLIT` keeps them split.
    if std::env::var_os("COD4RW_PROBESPLIT").is_none() {
        let surfaces = &content.map().gfx_world().expect("gfxworld").surfaces;
        let mut merged: HashMap<(AssetId, u8), (HashMap<u8, usize>, Vec<usize>)> = HashMap::new();
        for ((m, lm, probe), list) in groups.drain() {
            let entry = merged.entry((m, lm)).or_default();
            *entry.0.entry(probe).or_default() += list.iter().map(|&i| surfaces[i].tri_count as usize).sum::<usize>();
            entry.1.extend(list);
        }
        for ((m, lm), (probes, list)) in merged {
            let probe = probes.into_iter().max_by_key(|&(p, n)| (n, p)).map_or(0, |(p, _)| p);
            groups.insert((m, lm, probe), list);
        }
    }
    let mut world_mats: HashMap<(AssetId, u8, u8), Handle<WorldMaterial>> = HashMap::new();
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
    for ((mat_id, lightmap_index, probe_index), surfs) in groups {
        let Some(mat) = content.material(MAP_ZONE, mat_id, std_materials, images) else {
            warn!("world material {:?} not drawn: ({}), {} surfaces", content.zone(MAP_ZONE).material(mat_id).map(|m| m.name.clone()), content.technique_set(MAP_ZONE, mat_id), surfs.len());
            continue;
        };
        if mat.sky {
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
        let material = match world_mats.get(&(mat_id, lightmap_index, probe_index)) {
            Some(m) => m.clone(),
            None => {
                let base = std_materials.get(&mat.handle).cloned().unwrap_or_default();
                // `$identitynormalmap` and friends are flat; skip them.
                let real = |h: Option<Handle<Image>>, name: Option<String>| h.filter(|_| !name.is_some_and(|n| n.starts_with('$')));
                let normal_map = real(
                    content.material_texture(MAP_ZONE, mat_id, TextureSemantic::Normal, false, images),
                    content.material_texture_name(MAP_ZONE, mat_id, TextureSemantic::Normal),
                );
                // Opaque surfaces only: the prepass doesn't shift the UVs,
                // so an alpha-tested edge would disagree between the passes
                // (the sky colour showed along cables' edges).
                let height_map = (normal_map.is_some() && base.alpha_mode == AlphaMode::Opaque && crate::atmos::climate::showcase())
                    .then(|| content.material_texture_name(MAP_ZONE, mat_id, TextureSemantic::Normal))
                    .flatten()
                    .and_then(|name| crate::pom::height_map(&content.vfs.clone(), &name, images));
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
                        0.0,
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
                // Re-baked lightmaps are linear, their direction a unit vector.
                params.traced.z = (rebaked && lightmap.is_some()) as u32 as f32;
                if cod4_sun() {
                    let sun = &content.map().gfx_world().expect("gfxworld").sun;
                    params.sun_dir = to_sun(sun).extend(0.0);
                    params.sun_diffuse = live_sun(sun).extend(0.0);
                    params.traced.y = 1.0;
                }
                if technique_set.starts_with("wc_water") {
                    WorldLighting::water(zone_mat, content.map().gfx_world().expect("gfxworld"), &mut params);
                }
                let detail_map = content.material_detail(MAP_ZONE, mat_id, images).map(|(h, scale)| {
                    let mips = images.get(&h).map_or(1, |i| i.texture_descriptor.mip_level_count);
                    params.detail = Vec4::new(scale.x, scale.y, 1.0, mips.saturating_sub(1) as f32);
                    h
                });
                let extension =
                    WorldLighting { params, normal_map, specular_map, lightmap: lightmap.clone(), reflection_probe: reflection_probe.or_else(|| sky.clone()), detail_map, wet_map: crate::wet::wet_map(), ssr_history: crate::ssr::history(), height_map };
                let handle = world_materials.add(WorldMaterial { base, extension });
                world_mats.insert((mat_id, lightmap_index, probe_index), handle.clone());
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
                let (at, turn) = placed[surf_model[si] as usize].unwrap_or((Vec3::ZERO, Quat::IDENTITY));
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
        if alpha_mode == AlphaMode::Opaque {
            let base = proxy_positions.len() as u32;
            if let (Some(bevy::mesh::VertexAttributeValues::Float32x3(p)), Some(Indices::U32(idx))) =
                (mesh.attribute(Mesh::ATTRIBUTE_POSITION), mesh.indices())
            {
                proxy_positions.extend_from_slice(p);
                proxy_indices.extend(idx.iter().map(|i| base + i));
            }
        }
        let mut e = commands.spawn((Name::new(name), Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), ChildOf(root)));
        if let Some(w) = water {
            e.insert(w);
        }
        if !casts_itself {
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
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
        let normals = vec![[0.0, 1.0, 0.0]; proxy_positions.len()];
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, proxy_positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_indices(Indices::U32(proxy_indices));
        let material =
            std_materials.add(StandardMaterial { unlit: true, cull_mode: None, ..default() });
        commands.spawn((
            Name::new("world shadow caster"),
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            RenderLayers::layer(SHADOW_PROXY_LAYER),
            ChildOf(root),
        ));
    }
    info!("world geometry: {total_tris} triangles in {draws} draws");
}

fn spawn_static_models(
    commands: &mut Commands,
    content: &mut Content,
    meshes: &mut Assets<Mesh>,
    std_materials: &mut Assets<StandardMaterial>,
    world_materials: &mut Assets<WorldMaterial>,
    images: &mut Assets<Image>,
) {
    let root = commands.spawn((Name::new("static models"), Transform::default(), Visibility::default())).id();
    // Fake light beams and flares (`*_falloff_*`) fade as they turn edge-on,
    // as the world's do: the world material, with nothing else of it.
    let mut falloff_mats: HashMap<(usize, AssetId), Option<Handle<WorldMaterial>>> = HashMap::new();
    let instances: Vec<_> = content.map().gfx_world().expect("gfxworld").static_models.clone();
    let mut batches: HashMap<(BatchMaterial, IVec3, u32), Vec<(Handle<Mesh>, Transform)>> = HashMap::new();
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
    let mut wet_materials: std::collections::HashMap<bevy::asset::AssetId<StandardMaterial>, Handle<WorldMaterial>> = Default::default();
    let sky = crate::atmos::climate::showcase().then(|| content.sky_cube(images)).flatten();
    for ((material, _, cull), parts) in batches {
        let opaque = matches!(&material, BatchMaterial::Standard(m) if std_materials.get(m).is_some_and(|m| m.alpha_mode == AlphaMode::Opaque));
        if opaque && std::env::var_os("COD4RW_NOBATCH").is_none() {
            proxy.extend(parts.iter().cloned());
        }
        let Some(mesh) = merge_meshes(&parts, meshes) else { continue };
        let mut e = commands.spawn((Name::new("static models batch"), Mesh3d(meshes.add(mesh)), Transform::default(), ChildOf(root)));
        match material {
            BatchMaterial::World(m) => e.insert((MeshMaterial3d(m), NotShadowCaster)),
            // The showcase's rain wets them too (`crate::wet`).
            BatchMaterial::Standard(m) if crate::atmos::climate::showcase() => {
                let wet = wet_materials.entry(m.id()).or_insert_with(|| crate::wet::wet_material(std_materials.get(&m), sky.clone(), world_materials)).clone();
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
    if let Some(mesh) = merge_meshes(&proxy, meshes) {
        let material = std_materials.add(StandardMaterial { unlit: true, cull_mode: None, ..default() });
        commands.spawn((
            Name::new("static models shadow caster"),
            Mesh3d(meshes.add(mesh)),
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

/// [`merge_meshes`] of meshes in hand.
pub(crate) fn merge_mesh_data(read: &[(&Mesh, &Transform)]) -> Option<Mesh> {
    use bevy::mesh::VertexAttributeValues as V;
    let read = read.to_vec();
    let colours = read.iter().all(|(m, _)| m.attribute(Mesh::ATTRIBUTE_COLOR).is_some());
    let (mut pos, mut nrm, mut uv, mut col, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::<u32>::new());
    for (m, t) in read {
        let (Some(V::Float32x3(p)), Some(V::Float32x3(n)), Some(V::Float32x2(u))) =
            (m.attribute(Mesh::ATTRIBUTE_POSITION), m.attribute(Mesh::ATTRIBUTE_NORMAL), m.attribute(Mesh::ATTRIBUTE_UV_0))
        else {
            continue;
        };
        let base = pos.len() as u32;
        let matrix = t.to_matrix();
        pos.extend(p.iter().map(|v| matrix.transform_point3(Vec3::from(*v)).to_array()));
        nrm.extend(n.iter().map(|v| (t.rotation * Vec3::from(*v)).to_array()));
        uv.extend_from_slice(u);
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
