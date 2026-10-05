//! Loading a CoD4 map zone into Bevy: world geometry, static models, sun and
//! spawn points. Collision is built by [`crate::collision`].

use crate::content::{Content, MAP_ZONE};
use crate::units;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
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
        app.add_plugins(MaterialPlugin::<WorldMaterial>::default()).add_systems(OnEnter(crate::state::GameState::InGame), load_map.in_set(crate::state::Setup::Content));
    }
}

/// World surfaces: the standard material plus IW3's surface model (see
/// `shaders/world.wgsl`): normal and specular maps, the directional
/// lightmap, and view-angle falloff.
pub type WorldMaterial = ExtendedMaterial<StandardMaterial, WorldLighting>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct WorldLighting {
    #[uniform(100)]
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

    let lightmaps =
        crate::lightmaps::load(content.map(), content.map().gfx_world().expect("zone has no GfxWorld"), &mut images);
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
    );
    spawn_static_models(&mut commands, &mut content, &mut meshes, &mut std_materials, &mut world_materials, &mut images);
    let world = content.map().gfx_world().expect("zone has no GfxWorld");
    let light_grid = crate::model_lighting::spawn_irradiance_volume(&mut commands, world, &mut images);
    spawn_sun(&mut commands, world, light_grid);
    if let Some(sky) = content.sky_cube(&mut images) {
        commands.insert_resource(MapSky(sky));
    }
    info!("built render data in {:?}", t0.elapsed());

    if let Some(clip) = content.map().clip_map() {
        crate::collision::spawn_collision(&mut commands, clip);
    }
    let spawns =
        content.map().map_ents().map(|e| read_spawns(&iw3::ents::parse(&e.entity_string))).unwrap_or_default();
    info!("{} spawn points; map ready in {:?}", spawns.len(), t0.elapsed());
    commands.insert_resource(MapInfo { spawns });
    commands.insert_resource(content);
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
) {
    // Group surfaces by material, lightmap and reflection probe so each group
    // is one draw.
    let mut groups: HashMap<(AssetId, u8, u8), Vec<usize>> = HashMap::new();
    for (i, s) in content.map().gfx_world().expect("gfxworld").surfaces.iter().enumerate() {
        if let Some(m) = s.material {
            groups.entry((m, s.lightmap_index, s.reflection_probe_index)).or_default().push(i);
        }
    }
    let mut world_mats: HashMap<(AssetId, u8, u8), Handle<WorldMaterial>> = HashMap::new();

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
        let Some(mat) = content.material(MAP_ZONE, mat_id, std_materials, images) else { continue };
        if mat.sky {
            continue;
        }
        let lightmap = lightmaps.get(lightmap_index as usize).cloned().flatten();
        // Probe 0 is the engine's placeholder (the same reddish cube on every
        // map): no probe.
        let reflection_probe = probes.get(probe_index as usize).filter(|_| probe_index != 0).cloned().flatten();
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
                        reflection_probe.is_some() as u32 as f32,
                        reflection_probe
                            .as_ref()
                            .and_then(|h| images.get(h))
                            .map_or(0.0, |i| i.texture_descriptor.mip_level_count.saturating_sub(1) as f32),
                        0.0,
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
                if technique_set.starts_with("wc_water") {
                    WorldLighting::water(zone_mat, content.map().gfx_world().expect("gfxworld"), &mut params);
                }
                let extension =
                    WorldLighting { params, normal_map, specular_map, lightmap: lightmap.clone(), reflection_probe };
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
                for g in [global[0], global[2], global[1]] {
                    let local = *remap.entry(g).or_insert_with(|| {
                        let v = &world.vertices[g as usize];
                        positions.push(units::pos(v.xyz).to_array());
                        normals.push(units::dir(iw3::unpack::unit_vec(v.normal)).normalize_or(Vec3::Y).to_array());
                        // IW3's binormal is cross(normal, tangent) * sign, as in Bevy.
                        let t = units::dir(iw3::unpack::unit_vec(v.tangent)).normalize_or(Vec3::X);
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
        if !casts_itself {
            e.insert(NotShadowCaster);
        }
        if let Some(image) = lightmap {
            e.insert(Lightmap { image, uv_rect: Rect::new(0.0, 0.0, 1.0, 1.0), bicubic_sampling: false });
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
        let parent = commands.spawn((Name::new(name.clone()), transform, Visibility::default(), ChildOf(root))).id();
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
                miss("material failed", &mut missed);
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
            match falloff {
                Some(material) => commands.spawn((Mesh3d(mesh), MeshMaterial3d(material), NotShadowCaster, ChildOf(parent))),
                None => commands.spawn((Mesh3d(mesh), MeshMaterial3d(mat.handle), ChildOf(parent))),
            };
        }
    }
    for (name, (instances, surfaces, why)) in &missed {
        warn!("static model {name}: {surfaces} surfaces not drawn ({why}){}", if *instances > 0 { format!(", {instances} placed not drawn") } else { String::new() });
    }
}

/// How much of the map's flat ambient light models still get when the light
/// grid lights them: a floor for anything outside the grid's volume.
const AMBIENT_WITH_LIGHT_GRID: f32 = 0.15;

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
            illuminance: 12_000.0 * sun.sun_light.max(0.5),
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder { maximum_distance: 120.0, first_cascade_far_bound: 12.0, ..default() }.build(),
        Transform::default().looking_to(-to_sun, Vec3::Y),
        // The default layer plus the world's shadow proxy, and the
        // splitscreen players' bodies (for their shadows).
        RenderLayers::from_layers(
            &[0, SHADOW_PROXY_LAYER].into_iter().chain((0..crate::splitscreen::MAX_PLAYERS).map(crate::splitscreen::body_layer)).collect::<Vec<_>>(),
        ),
    ));
    let a = sun.ambient_color;
    commands.insert_resource(GlobalAmbientLight {
        color: Color::linear_rgb(a[0], a[1], a[2]),
        brightness: 2500.0 * sun.ambient_scale.max(0.05) * 4.0 * if light_grid { AMBIENT_WITH_LIGHT_GRID } else { 1.0 },
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
