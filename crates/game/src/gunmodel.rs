//! A weapon's gun model with any mix of attachments and its camo, put
//! together from CoD4's single-attachment weapon variants. Used by the
//! Create a Class previews and the viewmodel.
//!
//! Sights and grips are parts of the base model, shown or hidden by tag the
//! way each single-attachment variant (`m16_reflex_mp`) does it; a silencer
//! is borrowed from the silencer variant's model (its suppressor has a
//! material of its own); the grenade launcher variant is a model of its own.
//! Camos are model variants, indexed by the camo stat, whose materials add a
//! tiled camo texture onto the colour map (see `ui/camo.wgsl`). A red dot
//! sight's dot is drawn the way IW3's `mc_reflexsight` does it (see
//! `ui/reflex.wgsl`).
//!
//! A gun is named `weapon:attachment+attachment` (`ak47:reflex+silencer`).
//! Black Ops guns (`t5_ak47:...`, see [`crate::bo1`]) come from their own
//! [`Content`]: every attachment is a tagged part of the one model, and the
//! camo is BO1's colour detail layer. World at War guns (`t4_thompson:...`,
//! see [`crate::waw`]) likewise, with no camos (WaW has none).

use crate::content::{Content, PreparedModel};
use crate::models::{Skeleton, SpawnModel, spawn_model};
use crate::textures::TextureCache;
use bevy::camera::visibility::RenderLayers;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use iw3::zone::AssetType;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

mod custom;
mod diamond;
#[cfg(test)]
mod coverage_tests;
pub(crate) mod platinum;

pub struct GunModelPlugin;

impl Plugin for GunModelPlugin {
    fn build(&self, app: &mut App) {
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("ui/camo.wgsl"),
            std::path::Path::new(CAMO_SHADER),
            include_bytes!("ui/camo.wgsl").as_slice(),
        );
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("ui/reflex.wgsl"),
            std::path::Path::new(REFLEX_SHADER),
            include_bytes!("ui/reflex.wgsl").as_slice(),
        );
        app.add_plugins((MaterialPlugin::<CamoMaterial>::default(), MaterialPlugin::<ReflexMaterial>::default()))
            .add_systems(PostUpdate, (light_reflex_dots, follow_reflection_probe));
    }
}

/// Where `reflex.wgsl` is registered in the `embedded://` asset source.
const REFLEX_SHADER: &str = "cod4rw/reflex.wgsl";

/// IW3's `mc_reflexsight` (from its shaders): a red dot sight's dot, added
/// as light and projected along the lens's normal, so it sits on the sight
/// line and the rest of the lens stays clear. The dot texture is laid over
/// the view directions around the sight line, `detailScale` texture widths
/// per unit of their offset (25 on CoD4's sights: the texture spans about a
/// degree either side), and multiplied by a grain texture on the lens.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct ReflexMaterial {
    /// xy: `detailScale`.
    #[uniform(0)]
    scale: Vec4,
    #[texture(1)]
    #[sampler(2)]
    dot: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    grain: Handle<Image>,
    /// The reticle ([`crate::reticles`]): x its shape (0: CoD4's dot), y
    /// its size; and its colour.
    #[uniform(5)]
    style: Vec4,
    #[uniform(6)]
    tint: Vec4,
}

impl Material for ReflexMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/reflex.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }
}

/// A red dot sight's dot as its material describes it.
#[derive(Clone, Debug)]
struct ReflexInfo {
    dot: String,
    /// The lens grain (`detailMap`), if any.
    grain: Option<String>,
    scale: Vec2,
}

/// `detailScale` when a dot material has none.
const REFLEX_SCALE: f32 = 25.0;

/// A red dot surface just spawned, waiting for its [`ReflexMaterial`]: the
/// dot's and grain's textures and scale, and the reticle picked.
#[derive(Component)]
struct ReflexDot(Handle<Image>, Option<Handle<Image>>, Vec2, crate::reticles::Reticle);

fn light_reflex_dots(
    mut commands: Commands,
    dots: Query<(Entity, &ReflexDot)>,
    mut materials: ResMut<Assets<ReflexMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut made: Local<HashMap<(AssetId<Image>, Option<AssetId<Image>>, u16), Handle<ReflexMaterial>>>,
    mut white: Local<Option<Handle<Image>>>,
) {
    for (e, ReflexDot(dot, grain, scale, reticle)) in &dots {
        let grain = grain.clone().unwrap_or_else(|| white.get_or_insert_with(|| images.add(Image::default())).clone());
        let (style, tint) = reticle.uniforms();
        let material = made
            .entry((dot.id(), Some(grain.id()), reticle.code()))
            .or_insert_with(|| {
                materials.add(ReflexMaterial { scale: scale.extend(0.0).extend(0.0), dot: dot.clone(), grain, style, tint })
            })
            .clone();
        commands.entity(e).remove::<(ReflexDot, MeshMaterial3d<StandardMaterial>)>().insert(MeshMaterial3d(material));
    }
}

/// Name hash of a material's `detailMap` texture (the camo pattern).
const DETAIL_MAP: u32 = 3948059469;

/// CoD4 gun surfaces: the standard material with a camo detail texture
/// added onto its colour map, and IW3's model specular on top (from the
/// `lp_*_r0c0s0` shaders): the specular map's colour (and gloss) times a
/// fresnel term from the material's `envMapParms`, times the reflection
/// probe (sharper the glossier). That's what makes guns, and gold ones
/// above all, shine.
pub type CamoMaterial = ExtendedMaterial<StandardMaterial, CamoDetail>;

/// Where `camo.wgsl` is registered in the `embedded://` asset source.
const CAMO_SHADER: &str = "cod4rw/camo.wgsl";

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct CamoDetail {
    /// `detailScale`: the camo's tiling over the gun's UVs (xy); z = 1 masks
    /// it by the colour map's alpha (Black Ops' `colorDetailMap`), z = 2 is
    /// Black Ops' gold (the camo replaces the colour and shines like CoD4's
    /// gold guns); z = 3 platinum, 4 universal gold, 5 raised diamond;
    /// w = 1 with a camo.
    #[uniform(100)]
    pub scale: Vec4,
    #[texture(101)]
    #[sampler(102)]
    pub detail: Option<Handle<Image>>,
    /// IW3 specular map: specular colour in rgb, gloss in alpha.
    #[texture(103)]
    #[sampler(104)]
    pub specular: Option<Handle<Image>>,
    /// The reflection probe nearest the camera (see `follow_reflection_probe`).
    #[texture(105, dimension = "cube")]
    #[sampler(106)]
    pub probe: Option<Handle<Image>>,
    /// `envMapParms`: fresnel min, max and power, sun glint strength.
    #[uniform(107)]
    pub env: Vec4,
    /// x: has a specular map, y: has a probe, z: the probe's last mip, w:
    /// the lightmap exposure (IW3's gamma-space light to ours).
    #[uniform(108)]
    pub shine: Vec4,
    /// Beyond CoD4 ([`GunLook`]): x: the camo's contrast, y: its
    /// saturation, z: how much of it is painted on, w: the reflection's
    /// strength (over 1 also tints polished metal's by its colour). Not
    /// tried again: the specular map's gloss lowering Bevy's roughness, as
    /// Bevy's untinted ambient reflection then washed colours out (gold went
    /// grey).
    #[uniform(109)]
    pub look: Vec4,
    /// The colour map, for its local average under painted camo.
    #[texture(110)]
    #[sampler(111)]
    pub color: Option<Handle<Image>>,
}

/// How gun surfaces look: `COD4RW_GUNLOOK=cod4` keeps CoD4's (no normal
/// maps, its camo blend, its highlights); otherwise normal-mapped, with
/// camos stronger and the specular map's gloss in the lights' highlights.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GunLook {
    Cod4,
    Improved,
}

impl GunLook {
    fn get() -> GunLook {
        static LOOK: std::sync::OnceLock<GunLook> = std::sync::OnceLock::new();
        *LOOK.get_or_init(|| match std::env::var("COD4RW_GUNLOOK") {
            Ok(v) if v.eq_ignore_ascii_case("cod4") => GunLook::Cod4,
            _ => GunLook::Improved,
        })
    }

    fn uniform(self) -> Vec4 {
        match self {
            GunLook::Cod4 => Vec4::new(1.0, 1.0, 0.0, 1.0),
            GunLook::Improved => Vec4::new(CAMO_CONTRAST, CAMO_SATURATION, CAMO_PAINT, SHEEN),
        }
    }
}

/// The camo layer's contrast and saturation over CoD4's.
const CAMO_CONTRAST: f32 = 1.35;
const CAMO_SATURATION: f32 = 1.35;
/// How much of the camo is painted on rather than added.
const CAMO_PAINT: f32 = 0.6;
/// The specular map's reflection over CoD4's, past its 8-bit white.
const SHEEN: f32 = 1.6;

impl MaterialExtension for CamoDetail {
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/camo.wgsl".into()
    }
}

/// `ak47:reflex+silencer` -> ("ak47", ["reflex", "silencer"]).
pub fn parse(spec: &str) -> (&str, Vec<&str>) {
    let (weapon, list) = spec.split_once(':').unwrap_or((spec, ""));
    (weapon, list.split('+').filter(|a| !a.is_empty()).collect())
}

/// The camo cache third-person guns share: held ([`crate::thirdperson`])
/// and dropped ([`crate::pickups`]). It must be one: it's built from the
/// models' meshes while they're still readable, before they go to the GPU.
#[derive(Resource, Default)]
pub struct WorldCamos(pub CamoCache);

/// Camo materials made so far, by base material and camo texture.
#[derive(Default)]
pub struct CamoCache {
    textures: TextureCache,
    camos: HashMap<(AssetId<StandardMaterial>, String), Handle<CamoMaterial>>,
    platinum: Option<Handle<Image>>,
    /// Diamond geometry, cached by source mesh. The original remains intact.
    diamond_meshes: HashMap<AssetId<Mesh>, Handle<Mesh>>,
    /// Gun models with their normal maps (the shared model materials leave
    /// them out), by the plain model's first mesh: guns come from more than
    /// one content.
    dressing: crate::wardrobe::Dressing,
    dressed: HashMap<AssetId<Mesh>, Arc<PreparedModel>>,
}

impl CamoCache {
    /// Editing a custom camo makes materials for each version: keep only
    /// the versions in use once there are many.
    fn drop_stale_custom(&mut self) {
        let custom = |k: &str| k.strip_prefix("cod4rw/custom/").and_then(|r| u64::from_str_radix(r.split('/').next()?, 16).ok());
        if self.camos.keys().filter(|(_, k)| custom(k).is_some()).count() < 64 {
            return;
        }
        let live = crate::custom_camos::live_material_keys();
        self.camos.retain(|(_, k), _| custom(k).is_none_or(|v| live.contains(&v)));
    }

    /// `m` (model `name`) normal-mapped, its surfaces in the same order.
    fn dress(
        &mut self,
        content: &Content,
        name: &str,
        m: &Arc<PreparedModel>,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
    ) -> Arc<PreparedModel> {
        let Some(key) = m.surfaces.first().map(|s| s.0.id()).filter(|_| GunLook::get() == GunLook::Improved) else {
            return m.clone();
        };
        let dressing = &mut self.dressing;
        self.dressed
            .entry(key)
            .or_insert_with(|| {
                dressing.dressed(content, name, m, meshes, materials, images).map_or_else(|| m.clone(), Arc::new)
            })
            .clone()
    }
}

pub struct GunAssets<'a> {
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<StandardMaterial>,
    pub images: &'a mut Assets<Image>,
    pub bindposes: &'a mut Assets<SkinnedMeshInverseBindposes>,
    pub camo_materials: &'a mut Assets<CamoMaterial>,
}

/// Where a gun goes.
pub struct GunTarget {
    /// The entity holding the skeleton.
    pub owner: Entity,
    /// The joint the gun hangs from (the hands' `tag_weapon`).
    pub attach_to: Option<Entity>,
    pub layers: Option<RenderLayers>,
}

/// Spawn gun `spec`'s first-person model with `camo` into `skeleton`, with
/// the tags of the attachments it doesn't have hidden. Returns the bind-pose
/// bounds of the surfaces on show.
#[allow(clippy::too_many_arguments)]
pub fn spawn_gun(
    commands: &mut Commands,
    content: &mut Content,
    camos: &mut CamoCache,
    a: &mut GunAssets,
    skeleton: &mut Skeleton,
    spec: &str,
    camo: usize,
    target: GunTarget,
) -> Option<(Vec3, Vec3)> {
    spawn_model_of(commands, content, camos, a, skeleton, spec, camo, target, GUN_MODEL).map(|(b, _)| b)
}

/// The same with the gun's world model, the one characters hold (on
/// `tag_weapon_right`).
#[allow(clippy::too_many_arguments)]
pub fn spawn_world_gun(
    commands: &mut Commands,
    content: &mut Content,
    camos: &mut CamoCache,
    a: &mut GunAssets,
    skeleton: &mut Skeleton,
    spec: &str,
    camo: usize,
    target: GunTarget,
) -> Option<(Vec3, Vec3)> {
    spawn_model_of(commands, content, camos, a, skeleton, spec, camo, target, WORLD_MODEL).map(|(b, _)| b)
}

/// The world model in a character's hand (third person), returning its
/// surfaces (the player's own body switches them between being seen and
/// only casting a shadow).
#[allow(clippy::too_many_arguments)]
pub fn spawn_held_gun(
    commands: &mut Commands,
    content: &mut Content,
    camos: &mut CamoCache,
    a: &mut GunAssets,
    skeleton: &mut Skeleton,
    spec: &str,
    camo: usize,
    target: GunTarget,
) -> Option<Vec<Entity>> {
    spawn_model_of(commands, content, camos, a, skeleton, spec, camo, target, WORLD_MODEL).map(|(_, s)| s)
}

/// Weapon def fields listing a gun's models, one per camo.
const GUN_MODEL: &str = "gunXModel";
const WORLD_MODEL: &str = "worldModel";

#[allow(clippy::too_many_arguments)]
fn spawn_model_of(
    commands: &mut Commands,
    content: &mut Content,
    camos: &mut CamoCache,
    a: &mut GunAssets,
    skeleton: &mut Skeleton,
    spec: &str,
    camo: usize,
    target: GunTarget,
    field: &str,
) -> Option<((Vec3, Vec3), Vec<Entity>)> {
    let (meshes, materials, images) = (&mut *a.meshes, &mut *a.materials, &mut *a.images);
    let (weapon, attachments) = parse(spec);
    // The red dot's reticle rides on the camo number.
    let (camo, reticle) = crate::reticles::split(camo);
    // A custom camo is painted on the plain gun; one not defined here
    // (another player's) shows none.
    let custom = crate::custom_camos::get(camo);
    let camo = match &custom {
        Some(_) => custom::model_camo(crate::bo1::is_bo1(weapon), crate::waw::is_waw(weapon)),
        None if crate::custom_camos::is_custom(camo) => 0,
        None => camo,
    };
    let finish = platinum::Finish::selected(weapon, camo);
    let camo = finish.map_or(camo, |f| f.model_camo(weapon));
    // Keep the native gold models and materials where CoD4 supplies them.
    let finish = finish.filter(|f| *f != platinum::Finish::Gold || !platinum::Finish::native_gold(weapon));
    let bo1 = crate::bo1::is_bo1(weapon);
    let waw = crate::waw::is_waw(weapon);
    let gl = !bo1 && !waw && attachments.contains(&"gl");

    // The base model with the variant's hidden tags; each sight or grip
    // shows the tags its single-attachment variant shows (and hides what it
    // hides, like the iron sight under a red dot).
    let (model_name, hide) = if bo1 {
        bo1_model(weapon, &attachments, field == WORLD_MODEL)?
    } else if waw {
        let (name, hide) = crate::waw::data()?.model(weapon, &attachments, field == WORLD_MODEL)?;
        (crate::waw::model_name(content, &name)?, hide)
    } else {
        let (model_name, base_hide) =
            gun_model(content, &format!("{weapon}_{}mp", if gl { "gl_" } else { "" }), camo, field)?;
        let mut hide: BTreeSet<String> = base_hide.clone();
        if !gl {
            let (mut shown, mut hidden) = (BTreeSet::new(), BTreeSet::new());
            for att in attachments.iter().filter(|a| matches!(**a, "reflex" | "acog" | "grip")) {
                if let Some((_, variant_hide)) = gun_model(content, &format!("{weapon}_{att}_mp"), camo, field) {
                    shown.extend(base_hide.difference(&variant_hide).cloned());
                    hidden.extend(variant_hide.difference(&base_hide).cloned());
                }
            }
            hide = base_hide.difference(&shown).cloned().collect::<BTreeSet<_>>().union(&hidden).cloned().collect();
        }
        (model_name, hide)
    };
    let prepared = content.model(&model_name, meshes, materials, images, a.bindposes)?;
    // What's spawned; `prepared`'s materials still key the tables below.
    let mut worn = camos.dress(content, &model_name, &prepared, meshes, materials, images);
    let camo_surfaces = if bo1 {
        bo1_camo_surfaces(content, &model_name, weapon, camo, materials, images)
    } else if waw {
        HashMap::new()
    } else {
        camo_surfaces(content, &model_name, materials, images)
    };
    let paint_surfaces = match &custom {
        Some(_) => custom::surfaces(content, &model_name, weapon, (bo1, waw), materials, images),
        None => HashMap::new(),
    };
    let finish_surfaces: HashSet<_> = if finish.is_some() {
        platinum::surfaces(content, &model_name, materials, images)
    } else {
        HashSet::new()
    };
    if finish == Some(platinum::Finish::Diamond) {
        worn =
            diamond_model(content, &model_name, &prepared, &worn, &finish_surfaces, meshes, &mut camos.diamond_meshes);
    }
    let reflex_surfaces = reflex_surfaces(content, &model_name, materials, images);
    let lens_surfaces = lens_surfaces(content, &model_name, materials, images);
    let mut shine_surfaces = shine_surfaces(content, &model_name, materials, images);
    if waw {
        // World at War's guns give negative fresnel powers (`envMapParms.z`,
        // -3.4 on most), which as CoD4's power make every surface a mirror;
        // their size as CoD4's power gives the usual sheen.
        for (_, env) in shine_surfaces.values_mut() {
            env.z = env.z.abs();
        }
    }
    // A silencer: the suppressor surfaces of the silencer variant's model.
    let suppressor = (!bo1 && !waw && !gl && attachments.contains(&"silencer"))
        .then(|| gun_model(content, &format!("{weapon}_silencer_mp"), camo, field))
        .flatten()
        .and_then(|(name, _)| {
            let keep = own_materials(content, &name, &model_name, materials, images);
            content.model(&name, meshes, materials, images, a.bindposes).map(|m| (m, keep))
        });

    // The bind-pose bounds of the gun (and its suppressor), as worked out
    // when they were prepared: once a mesh is on the GPU its vertices can't
    // be read back (a gun spawned again, dropped on death, would panic).
    let (mut lo, mut hi) = prepared.bounds;
    if let Some((m, _)) = &suppressor
        && m.bounds.0.x <= m.bounds.1.x
    {
        lo = lo.min(m.bounds.0);
        hi = hi.max(m.bounds.1);
    }
    if lo.x > hi.x || prepared.surfaces.is_empty() {
        return None;
    }

    let GunTarget { owner, attach_to, layers } = target;
    // Held guns cast the sun's shadow with their holder (the viewmodel's
    // layer has no shadowing sun, so first-person guns are unaffected).
    let surfaces = spawn_model(
        commands,
        skeleton,
        SpawnModel { model: &worn, owner, attach_to, layers: layers.clone(), shadows: true },
    );
    let mut spawned = surfaces.clone();
    if let Some((model, keep)) = &suppressor {
        // Joints shared with the base model are reused; drop all but the suppressor.
        let parts = spawn_model(commands, skeleton, SpawnModel { model, owner, attach_to, layers, shadows: true });
        for (entity, (_, material)) in parts.into_iter().zip(&model.surfaces) {
            if !keep.contains(&material.id()) {
                commands.entity(entity).despawn();
            } else {
                spawned.push(entity);
            }
        }
    }
    let vfs = content.vfs.clone();
    for (entity, ((_, material), (_, worn))) in surfaces.into_iter().zip(prepared.surfaces.iter().zip(&worn.surfaces)) {
        // A magnifying scope's eyepiece: a 3D scope draws its view there.
        // (Kept as it is meanwhile: no camo or shine of its own.)
        if lens_surfaces.contains(&material.id()) {
            commands.entity(entity).insert(ScopeLens);
            continue;
        }
        if let Some(info) = reflex_surfaces.get(&material.id()) {
            if let Some(dot) = camos.textures.get(&info.dot, true, &vfs, images) {
                let grain = info.grain.as_ref().and_then(|g| camos.textures.get(g, true, &vfs, images));
                commands.entity(entity).insert(ReflexDot(dot, grain, info.scale, reticle));
                continue;
            }
        }
        if let (Some(def), Some(&surface)) = (&custom, paint_surfaces.get(&material.id())) {
            let key = (worn.id(), format!("cod4rw/custom/{:x}/{}", def.key(), surface.1));
            let handle = camos.camos.get(&key).cloned().or_else(|| {
                let base = materials.get(worn)?.clone();
                let paint = crate::custom_camos::texture(def, images);
                let specular = shine_surfaces
                    .get(&material.id())
                    .and_then(|(s, env)| Some((camos.textures.get(s, false, &vfs, images)?, *env)));
                let h = a.camo_materials.add(custom::material(base, def, paint, specular, surface));
                camos.drop_stale_custom();
                camos.camos.insert(key, h.clone());
                Some(h)
            });
            if let Some(handle) = handle {
                commands.entity(entity).remove::<MeshMaterial3d<StandardMaterial>>().insert(MeshMaterial3d(handle));
                continue;
            }
        }
        if let Some(finish) = finish.filter(|_| finish_surfaces.contains(&material.id())) {
            let key = (worn.id(), finish.cache_key().to_owned());
            let handle = camos.camos.get(&key).cloned().or_else(|| {
                let detail = camos.platinum.clone().or_else(|| {
                    let h = platinum::texture(images)?;
                    camos.platinum = Some(h.clone());
                    Some(h)
                })?;
                let base = materials.get(worn)?.clone();
                let specular =
                    shine_surfaces.get(&material.id()).and_then(|(s, _)| camos.textures.get(s, false, &vfs, images));
                let h = a.camo_materials.add(platinum::material(base, detail, specular, finish));
                camos.camos.insert(key, h.clone());
                Some(h)
            });
            if let Some(handle) = handle {
                commands.entity(entity).remove::<MeshMaterial3d<StandardMaterial>>().insert(MeshMaterial3d(handle));
                continue;
            }
        }
        let camo_detail = camo_surfaces.get(&material.id());
        let shine = shine_surfaces.get(&material.id());
        if camo_detail.is_none() && shine.is_none() {
            continue;
        }
        let key = (worn.id(), camo_detail.map_or_else(String::new, |(d, _)| d.clone()));
        let camo = match camos.camos.get(&key) {
            Some(h) => h.clone(),
            None => {
                let Some(base) = materials.get(worn).cloned() else { continue };
                let detail = camo_detail.and_then(|(d, _)| camos.textures.get(d, true, &vfs, images));
                let scale = camo_detail.map_or(Vec4::ZERO, |(_, s)| Vec4::new(s.x, s.y, s.z, 1.0));
                let gold = scale.z > 1.5;
                // Black Ops' gold is a dark colour under a gold specular map.
                let specular = match gold {
                    true => camos.textures.get(crate::bo1::GOLD_CAMO_SPECULAR, false, &vfs, images),
                    false => shine.and_then(|(spec, _)| camos.textures.get(spec, false, &vfs, images)),
                };
                let env = if gold { GOLD_ENV } else { shine.map_or(Vec4::ZERO, |(_, env)| *env) };
                let on = (specular.is_some() || gold) && std::env::var_os("COD4RW_NOSHINE").is_none();
                let flags = Vec4::new(on as u32 as f32, 0.0, 0.0, crate::lightmaps::LIGHTMAP_EXPOSURE);
                let color = base.base_color_texture.clone();
                let h = a.camo_materials.add(CamoMaterial {
                    base,
                    extension: CamoDetail {
                        scale,
                        detail,
                        specular,
                        probe: None,
                        env,
                        shine: flags,
                        look: GunLook::get().uniform(),
                        color,
                    },
                });
                camos.camos.insert(key, h.clone());
                h
            }
        };
        commands.entity(entity).remove::<MeshMaterial3d<StandardMaterial>>().insert(MeshMaterial3d(camo));
    }
    // Scaled to nothing: now for still models, and by the animation player
    // for animated ones.
    for tag in &hide {
        if let Some(j) = skeleton.joints.iter().find(|j| j.name.eq_ignore_ascii_case(tag)) {
            commands.entity(j.entity).insert(Transform { scale: Vec3::ZERO, ..j.bind });
            let name = j.name.clone();
            skeleton.hide(&name);
        }
    }
    Some(((lo, hi), spawned))
}

/// Diamond geometry is made from the native CPU model, so switching to it
/// after Bevy has extracted the original mesh still works. The studs share
/// the gun's skinning and draw calls, and cached meshes are reused.
fn diamond_model(
    content: &Content,
    name: &str,
    prepared: &PreparedModel,
    worn: &Arc<PreparedModel>,
    covered: &HashSet<AssetId<StandardMaterial>>,
    meshes: &mut Assets<Mesh>,
    cache: &mut HashMap<AssetId<Mesh>, Handle<Mesh>>,
) -> Arc<PreparedModel> {
    let Some((zi, id)) = content.find(name) else { return worn.clone() };
    let Some(xm) = content.zones[zi].xmodel(id) else { return worn.clone() };
    let Some(lod) = xm.lods.first() else { return worn.clone() };
    let kept: Vec<_> = (lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize)
        .filter(|&s| xm.materials.get(s).copied().flatten().is_some())
        .filter(|&s| xm.surfs.get(s).is_some_and(|s| !s.verts.is_empty() && !s.tris.is_empty()))
        .collect();
    if kept.len() != prepared.surfaces.len() {
        return worn.clone();
    }
    let coated_count = prepared.surfaces.iter().filter(|(_, m)| covered.contains(&m.id())).count().max(1);
    let stud_budget = (2400 / coated_count).min(diamond::MAX_STUDS);
    let surfaces = kept
        .iter()
        .zip(prepared.surfaces.iter().zip(&worn.surfaces))
        .map(|(&surf, ((_, original), (mesh, material)))| {
            if !covered.contains(&original.id()) {
                return (mesh.clone(), material.clone());
            }
            let made = cache.get(&mesh.id()).cloned().or_else(|| {
                let mut source = crate::content::surface_mesh(xm, surf, true)?;
                let tangents: Vec<_> = xm.surfs[surf]
                    .verts
                    .iter()
                    .map(|v| {
                        let t = crate::units::dir(iw3::unpack::unit_vec(v.tangent)).normalize_or(Vec3::X);
                        [t.x, t.y, t.z, if v.binormal_sign < 0.0 { -1.0 } else { 1.0 }]
                    })
                    .collect();
                source.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
                let h = crate::mesh_bounds::add(meshes, diamond::studded_with_budget(&source, stud_budget)?);
                cache.insert(mesh.id(), h.clone());
                Some(h)
            });
            (made.unwrap_or_else(|| mesh.clone()), material.clone())
        })
        .collect();
    let bones = worn
        .bones
        .iter()
        .map(|b| crate::content::PreparedBone {
            name: b.name.clone(),
            parent: b.parent,
            bind_local: b.bind_local,
            anim_base: b.anim_base,
        })
        .collect();
    Arc::new(PreparedModel {
        name: worn.name.clone(),
        bones,
        surfaces,
        inverse_bindposes: worn.inverse_bindposes.clone(),
        extent: worn.extent,
        bounds: worn.bounds,
    })
}

/// The surfaces of a model whose material has a camo: its standard material
/// -> (camo texture, [`CamoDetail::scale`]).
fn camo_surfaces(
    content: &mut Content,
    model: &str,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> HashMap<AssetId<StandardMaterial>, (String, Vec4)> {
    let mut out = HashMap::new();
    let Some((zi, id)) = content.find(model) else { return out };
    let zone = &content.zones[zi];
    let Some(xm) = zone.xmodel(id) else { return out };
    let Some(lod) = xm.lods.first() else { return out };
    let range = lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize;
    let mut found = Vec::new();
    for mat_id in range.filter_map(|s| xm.materials.get(s).copied().flatten()) {
        let Some(mat) = zone.material(mat_id) else { continue };
        let detail =
            mat.textures.iter().find(|t| t.name_hash == DETAIL_MAP).and_then(|t| t.image).and_then(|i| zone.image(i));
        let scale = mat
            .constants
            .iter()
            .find(|c| c.name == "detailScale")
            .map(|c| Vec4::new(c.literal[0], c.literal[1], 0.0, 0.0));
        if let (Some(detail), Some(scale)) = (detail, scale) {
            found.push((mat_id, detail.name.clone(), scale));
        }
    }
    for (mat_id, detail, scale) in found {
        if let Some(info) = content.material(zi, mat_id, materials, images) {
            out.insert(info.handle.id(), (detail, scale));
        }
    }
    out
}

/// CoD4's gold guns' reflection settings (`mtl_weapon_*_gold`), for Black
/// Ops' gold camo.
const GOLD_ENV: Vec4 = Vec4::new(4.0, 4.0, 6.0, 0.625);

/// IW3 materials' reflection settings when they have none.
const DEFAULT_ENV: Vec4 = Vec4::new(0.5, 1.0, 2.0, 0.625);

/// The surfaces of a model with a specular map: its standard material ->
/// (specular map, `envMapParms`).
fn shine_surfaces(
    content: &mut Content,
    model: &str,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> HashMap<AssetId<StandardMaterial>, (String, Vec4)> {
    let mut out = HashMap::new();
    let Some((zi, id)) = content.find(model) else { return out };
    let zone = &content.zones[zi];
    let Some(xm) = zone.xmodel(id) else { return out };
    let Some(lod) = xm.lods.first() else { return out };
    let range = lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize;
    let mut found = Vec::new();
    for mat_id in range.filter_map(|s| xm.materials.get(s).copied().flatten()) {
        let Some(mat) = zone.material(mat_id) else { continue };
        let spec = mat
            .textures
            .iter()
            .find(|t| t.semantic == iw3::zone::TextureSemantic::Specular)
            .and_then(|t| t.image)
            .and_then(|i| zone.image(i))
            .map(|i| i.name.clone())
            .filter(|n| !n.starts_with('$'));
        let env =
            mat.constants.iter().find(|c| c.name == "envMapParms").map_or(DEFAULT_ENV, |c| Vec4::from_array(c.literal));
        if let Some(spec) = spec {
            found.push((mat_id, spec, env));
        }
    }
    for (mat_id, spec, env) in found {
        if let Some(info) = content.material(zi, mat_id, materials, images) {
            out.insert(info.handle.id(), (spec, env));
        }
    }
    out
}

/// Guns reflect the probe nearest the camera, as IW3 lights models with the
/// probe where they are: switched on every gun material when the camera
/// moves to another probe's area, and given to new ones as they come.
fn follow_reflection_probe(
    probes: Option<Res<crate::world::ReflectionProbes>>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    mut current: Local<Option<usize>>,
    mut added: MessageReader<AssetEvent<CamoMaterial>>,
    mut materials: ResMut<Assets<CamoMaterial>>,
) {
    let new: Vec<AssetId<CamoMaterial>> =
        added.read().filter_map(|e| if let AssetEvent::Added { id } = e { Some(*id) } else { None }).collect();
    let (Some(probes), Ok(camera)) = (probes, camera.single()) else { return };
    let at = camera.translation();
    let nearest =
        probes.0.iter().enumerate().min_by(|a, b| a.1.0.distance(at).total_cmp(&b.1.0.distance(at))).map(|(i, _)| i);
    let changed = nearest != *current;
    *current = nearest;
    let Some((_, probe, mip)) = nearest.and_then(|i| probes.0.get(i)) else { return };
    let ids: Vec<AssetId<CamoMaterial>> = if changed { materials.ids().collect() } else { new };
    for id in ids {
        let Some(m) = materials.get(id) else { continue };
        if m.extension.shine.x < 0.5 || m.extension.probe.as_ref() == Some(probe) {
            continue;
        }
        if let Some(mut m) = materials.get_mut(id) {
            m.extension.probe = Some(probe.clone());
            m.extension.shine.y = 1.0;
            m.extension.shine.z = *mip;
        }
    }
}

/// Black Ops' `colorDetailMap` texture semantic (see [`crate::bo1`]).
const COLOR_DETAIL_SEMANTIC: u8 = 19;

/// A Black Ops gun's model and the tags it hides: its base weapon file's,
/// less the parts each attachment's variant shows, plus what they hide (the
/// iron sight under a scope, the standard magazine under an extended one).
/// Every Black Ops attachment is a part of the one model.
fn bo1_model(weapon: &str, attachments: &[&str], world: bool) -> Option<(String, BTreeSet<String>)> {
    let data = crate::bo1::data()?;
    let gun = data.gun(weapon)?;
    let base = data.weapon(&format!("{}_mp", gun.name))?;
    let tags = |w: &t5::weapons::WeaponFile| -> BTreeSet<String> {
        w.hide_tags().iter().map(|t| t.to_ascii_lowercase()).collect()
    };
    let base_hide = tags(base);
    let (mut shown, mut hidden) = (BTreeSet::new(), BTreeSet::new());
    for att in attachments {
        if let Some(variant) = data.weapon(&format!("{}_{att}_mp", gun.name)) {
            let variant_hide = tags(variant);
            shown.extend(base_hide.difference(&variant_hide).cloned());
            hidden.extend(variant_hide.difference(&base_hide).cloned());
        }
    }
    let hide = base_hide.difference(&shown).cloned().collect::<BTreeSet<_>>().union(&hidden).cloned().collect();
    Some((if world { base.world_model() } else { base.gun_model() }.to_owned(), hide))
}

/// A Black Ops gun's surfaces and their colour detail textures with camo
/// stat `camo` (see [`crate::bo1::camo_stat`]; none still has the guns'
/// metal and wood textures).
fn bo1_camo_surfaces(
    content: &mut Content,
    model: &str,
    weapon: &str,
    camo: usize,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> HashMap<AssetId<StandardMaterial>, (String, Vec4)> {
    let mut out = HashMap::new();
    let Some(gun) = crate::bo1::data().and_then(|d| Some((d, d.gun(weapon)?))) else { return out };
    let Some((zi, id)) = content.find(model) else { return out };
    let zone = &content.zones[zi];
    let Some(xm) = zone.xmodel(id) else { return out };
    let Some(lod) = xm.lods.first() else { return out };
    let range = lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize;
    let mut found = Vec::new();
    for mat_id in range.filter_map(|s| xm.materials.get(s).copied().flatten()) {
        let Some(mat) = zone.material(mat_id) else { continue };
        let own = mat
            .textures
            .iter()
            .find(|t| t.semantic == iw3::zone::TextureSemantic::Other(COLOR_DETAIL_SEMANTIC))
            .and_then(|t| t.image)
            .and_then(|i| zone.image(i))
            .map(|i| i.name.clone());
        let Some(detail) = gun.0.camo_detail(&gun.1.name, &mat.name, crate::bo1::camo_of_stat(camo), own.as_deref())
        else {
            continue;
        };
        // `weapon_camo_off` and `weapon_camo_neutral` are flat grey.
        if detail.starts_with("weapon_camo_") {
            continue;
        }
        // Constant names are cut to 12 characters (`colorDetailS`).
        let scale = mat
            .constants
            .iter()
            .find(|c| c.name.starts_with("colorDetail"))
            .map_or([1.0, 1.0], |c| [c.literal[0], c.literal[1]]);
        // Gold replaces the colour rather than adding to it.
        let mode = if detail == crate::bo1::GOLD_CAMO_TEXTURE { 2.0 } else { 1.0 };
        found.push((mat_id, detail, Vec4::new(scale[0], scale[1], mode, 0.0)));
    }
    for (mat_id, detail, scale) in found {
        if let Some(info) = content.material(zi, mat_id, materials, images) {
            out.insert(info.handle.id(), (detail, scale));
        }
    }
    out
}

/// A magnifying scope's eyepiece lens (a sniper scope's, an ACOG's): what
/// `ui::scope3d` draws the scope's view on.
#[derive(Component)]
pub struct ScopeLens;

/// The surfaces of a model that are a scope's eyepiece: CoD4's
/// `mtl_weapon_*_scope_lens` and `mtl_weapon_acog_lens` (not red dots'
/// `reflex_lens`, nor the front lenses).
fn lens_surfaces(
    content: &mut Content,
    model: &str,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> HashSet<AssetId<StandardMaterial>> {
    let Some((zi, id)) = content.find(model) else { return HashSet::new() };
    let zone = &content.zones[zi];
    let Some(xm) = zone.xmodel(id) else { return HashSet::new() };
    let Some(lod) = xm.lods.first() else { return HashSet::new() };
    let ids: Vec<_> = (lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize)
        .filter_map(|i| xm.materials.get(i).copied().flatten())
        .filter(|&i| {
            zone.material(i).is_some_and(|m| {
                let name = m.name.trim_start_matches(',').to_ascii_lowercase();
                name.ends_with("scope_lens") || name.ends_with("acog_lens")
            })
        })
        .collect();
    ids.into_iter().filter_map(|id| content.material(zi, id, materials, images)).map(|m| m.handle.id()).collect()
}

/// The surfaces of a model drawn with `mc_reflexsight` (a red dot sight's
/// dot): its standard material -> the dot.
fn reflex_surfaces(
    content: &mut Content,
    model: &str,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> HashMap<AssetId<StandardMaterial>, ReflexInfo> {
    let mut out = HashMap::new();
    let Some((zi, id)) = content.find(model) else { return out };
    let zone = &content.zones[zi];
    let Some(xm) = zone.xmodel(id) else { return out };
    let Some(lod) = xm.lods.first() else { return out };
    let range = lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize;
    let mut found = Vec::new();
    for mat_id in range.filter_map(|s| xm.materials.get(s).copied().flatten()) {
        let Some(mat) = zone.material(mat_id) else { continue };
        let techset = mat.technique_set.and_then(|t| zone.technique_set(t)).map_or("", |t| t.name.as_str());
        if !techset.ends_with("reflexsight") {
            continue;
        }
        let image = |t: &iw3::zone::MaterialTexture| t.image.and_then(|i| zone.image(i)).map(|i| i.name.clone());
        let grain = mat.textures.iter().find(|t| t.name_hash == DETAIL_MAP).and_then(image);
        let dot = mat.textures.iter().find(|t| t.name_hash != DETAIL_MAP).and_then(image);
        let scale = mat
            .constants
            .iter()
            .find(|c| c.name == "detailScale")
            .map_or(Vec2::splat(REFLEX_SCALE), |c| Vec2::new(c.literal[0], c.literal[1]));
        if let Some(dot) = dot {
            found.push((mat_id, ReflexInfo { dot, grain, scale }));
        }
    }
    for (mat_id, dot) in found {
        if let Some(info) = content.material(zi, mat_id, materials, images) {
            out.insert(info.handle.id(), dot);
        }
    }
    out
}

/// A weapon def's gun (or world) model for a camo (its first one if the
/// camo has none) and the tags it hides.
fn gun_model(content: &Content, def: &str, camo: usize, field: &str) -> Option<(String, BTreeSet<String>)> {
    let (zi, def) = content.generic(AssetType::Weapon, def)?;
    let zone = &content.zones[zi];
    let models: Vec<String> =
        def.assets(field).into_iter().flatten().map(|id| zone.get(id).name().to_owned()).collect();
    let model = models.get(camo).or(models.first())?.clone();
    let hide = (0..8)
        .map(|i| def.int(&format!("hideTags[{i}]")) as usize)
        .filter(|&i| i != 0)
        .filter_map(|i| zone.script_strings.get(i).map(|s| s.to_ascii_lowercase()))
        .collect();
    Some((model, hide))
}

/// Materials of `model` that `base` doesn't use (a silencer variant's
/// suppressor), as their standard materials.
fn own_materials(
    content: &mut Content,
    model: &str,
    base: &str,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> HashSet<AssetId<StandardMaterial>> {
    let lod0 = |content: &Content, name: &str| -> Option<(usize, Vec<iw3::zone::AssetId>)> {
        let (zi, id) = content.find(name)?;
        let xm = content.zones[zi].xmodel(id)?;
        let lod = xm.lods.first()?;
        let range = lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize;
        Some((zi, range.filter_map(|s| xm.materials.get(s).copied().flatten()).collect()))
    };
    let (Some((zi, own)), Some((_, used))) = (lod0(content, model), lod0(content, base)) else { return HashSet::new() };
    let names = |ids: &[iw3::zone::AssetId]| -> HashSet<String> {
        ids.iter().map(|&m| content.zones[zi].get(m).name().to_owned()).collect()
    };
    let used = names(&used);
    let own: Vec<_> = own.into_iter().filter(|&m| !used.contains(content.zones[zi].get(m).name())).collect();
    own.into_iter().filter_map(|m| content.material(zi, m, materials, images)).map(|info| info.handle.id()).collect()
}
