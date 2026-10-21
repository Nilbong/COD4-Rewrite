//! Soldiers' surfaces: the worn characters (third-person bodies, the
//! first-person arms and body, the killcam's arms) get
//! [`CharacterMaterial`], their CoD4 material with CoD4's own model
//! specular (its specular map times `envMapParms`' fresnel times the
//! reflection probe, and the sun's glint: the shine on skin, eyes and sweat
//! that makes its soldiers read as detailed) and skin, cloth and hard gear
//! shaded as such (`character_look.wgsl`). Every map; not with ray-traced
//! lighting (its own materials).
//!
//! CoD4's characters have colour, normal and specular maps (the specular
//! maps generated, `~name~hash`: colour in rgb, gloss in alpha), and one
//! texture holds a soldier's hands next to his vest, pouches and
//! helmet, so skin is found per pixel by its colour (CoD4's skin: hue
//! 11-28 degrees, saturation 0.35-0.6; its cloth, leather and paint fall
//! outside, red keffiyehs below, khaki and tan above), within materials
//! whose names allow it (not gloves or sleeves). Helmets, goggles and kit
//! (by material name) are hard gear; the rest is cloth.
//!
//! - Skin: wrapped, red-tinted light past the terminator (light bleeding
//!   through thin skin), a broad second highlight over Bevy's, and a little
//!   of its light kept in the shade, so faces don't go dead grey.
//! - Cloth: rough, a sheen lobe (Charlie) towards grazing light.
//! - Gear: smoother, the light grid's light reflected round it.
//!
//! Live knobs (`crate::tune`): `charlook.scatter` (strength),
//! `charlook.scatter_r/g/b` (its colour), `charlook.wrap`, `charlook.skin_spec`,
//! `charlook.shade_lift`, `charlook.sheen`, `charlook.sheen_rough`,
//! `charlook.gear_spec`, `charlook.cod4_spec` (CoD4's specular), `charlook.flat_ambient`
//! (Bevy's flat ambient light kept: CoD4 had none on models),
//! `charlook.fresnel_max` (cap on CoD4's grazing fresnel). Debug: `COD4RW_CHARLOOK=off` leaves CoD4's
//! material on (A/B); `=mask` draws skin red, cloth green, gear blue;
//! `=added` shows only what this look adds (x8); `=sun` the sun's shadow
//! at the surface (r), 3 m above (g) and 30 m toward the sun (b); `=wipe` keeps CoD4's shading on the screen's left half; `=ab` switches between CoD4's shading (even shots) and this (odd) in
//! step with `COD4RW_SHOT_VIEWS` (list each view twice) for same-pose pairs;
//! `COD4RW_CHARLOOK_LOG` logs each material's class.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use std::collections::HashMap;
use iw3::iwd::Vfs;
use std::sync::{Arc, LazyLock, Mutex};

pub type CharacterMaterial = ExtendedMaterial<StandardMaterial, CharacterLook>;

const SHADER: &str = "cod4rw/character_look.wgsl";

pub struct CharacterLookPlugin;

impl Plugin for CharacterLookPlugin {
    fn build(&self, app: &mut App) {
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("character_look.wgsl"),
            std::path::Path::new(SHADER),
            include_bytes!("character_look.wgsl").as_slice(),
        );
        // (Each match's materials are new: those made for the last let go.)
        app.add_plugins(MaterialPlugin::<CharacterMaterial>::default())
            .init_resource::<Looks>()
            .add_systems(OnEnter(crate::state::GameState::InGame), |mut looks: ResMut<Looks>| *looks = Looks::default())
            .add_systems(
            PostUpdate,
            (wear, knobs, follow_reflection_probe, ab, prune_sources).chain().run_if(crate::state::in_game).run_if(active),
        );
    }
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct CharacterLook {
    /// How much of the material may be each class: x skin (still found by
    /// colour), y gear; w the debug view (1: the classes as colours).
    #[uniform(100)]
    pub class: Vec4,
    /// Skin: the scattered light's colour (rgb), its strength (w).
    #[uniform(101)]
    pub skin: Vec4,
    /// x: cloth's sheen, y: its roughness, z: gear's reflection, w: the
    /// skin's second highlight.
    #[uniform(102)]
    pub cloth: Vec4,
    /// x: how far skin's light wraps past the terminator, y: how much the
    /// light grid lifts skin in the shade.
    #[uniform(103)]
    pub extra: Vec4,
    /// CoD4's specular map: specular colour (rgb), gloss (a); `class.z` 1
    /// with one.
    #[texture(104)]
    #[sampler(105)]
    pub specular: Option<Handle<Image>>,
    /// `envMapParms`: fresnel min, max and power, sun glint strength.
    #[uniform(106)]
    pub env: Vec4,
    /// x: has a probe, y: the probe's last mip, z: the probe's exposure, w:
    /// the open air's light (the light grid's).
    #[uniform(107)]
    pub shine: Vec4,
    /// The reflection probe nearest the camera.
    #[texture(108, dimension = "cube")]
    #[sampler(109)]
    pub probe: Option<Handle<Image>>,
    /// x: how much of Bevy's flat ambient light they keep (CoD4 lit models
    /// by the light grid and sun alone).
    #[uniform(110)]
    pub more: Vec4,
}

impl MaterialExtension for CharacterLook {
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/character_look.wgsl".into()
    }
}

/// Worn character surfaces: given [`CharacterMaterial`].
#[derive(Component)]
pub struct Worn;

/// Insert [`Worn`] on spawned surfaces (nothing when the look is off).
pub fn mark(commands: &mut Commands, surfaces: &[Entity]) {
    if !active() {
        return;
    }
    for &e in surfaces {
        commands.entity(e).try_insert(Worn);
    }
}

/// Whether a model is a person's (`body_*`, `head_*`, `character_*`,
/// `viewhands_*`), for the look's other callers (campaign actors).
pub fn is_character(model: &str) -> bool {
    let m = model.trim_start_matches(',').to_ascii_lowercase();
    ["body", "head", "character", "viewhands", "viewmodel_hands"].iter().any(|p| m.starts_with(p))
}

/// Model materials' CoD4 names and technique sets, by their Bevy material
/// (registered as [`crate::content::Content`] and the wardrobe make them:
/// characters from other zones are made by contents that are then dropped).
static SOURCES: LazyLock<Mutex<HashMap<AssetId<StandardMaterial>, Source>>> = LazyLock::new(Default::default);

#[derive(Clone)]
struct Source {
    name: String,
    techset: String,
    /// The specular map's image, to load from these archives when worn.
    specular: Option<(String, Arc<Vfs>)>,
    /// `envMapParms`.
    env: Vec4,
}

/// IW3's `envMapParms` when a material sets none.
const DEFAULT_ENV: Vec4 = Vec4::new(0.5, 1.0, 2.0, 0.625);

/// A model material made from CoD4's `name` (technique set `techset`,
/// specular map image `specular` in `vfs`, `envMapParms` `env`).
pub fn register(id: AssetId<StandardMaterial>, name: &str, techset: &str, specular: Option<String>, env: Option<[f32; 4]>, vfs: &Arc<Vfs>) {
    let specular = specular.filter(|n| !n.trim_start_matches(',').starts_with('$')).map(|n| (n, vfs.clone()));
    let env = env.map_or(DEFAULT_ENV, Vec4::from_array);
    if let Ok(mut s) = SOURCES.lock() {
        s.insert(id, Source { name: name.to_ascii_lowercase(), techset: techset.to_ascii_lowercase(), specular, env });
    }
}

/// `new` is `old` with something added (its normal map).
pub fn register_copy(old: AssetId<StandardMaterial>, new: AssetId<StandardMaterial>) {
    if let Ok(mut s) = SOURCES.lock() {
        if let Some(v) = s.get(&old).cloned() {
            s.insert(new, v);
        }
    }
}

/// How much of a material may be skin and gear, by its name; `None`: left
/// as it is (eyes, glass).
fn classes(name: &str, techset: &str) -> Option<(f32, f32)> {
    let has = |parts: &[&str]| parts.iter().any(|p| name.contains(p));
    if has(&["cornea", "glass", "lens", "visor"]) || techset.contains("cornea") {
        return None;
    }
    // (CoD4's viewhands named for gloves often show skin: `mtl_marine_glove`
    // is a skin technique.)
    let skin = if has(&["sleeve"]) && !techset.contains("skin") { 0.0 } else { 1.0 };
    // Gear is also only where the specular map is glossy (CoD4's helmets
    // ~0.28, skin 0.14-0.21, cloth under 0.1). Not by gloss alone: other
    // games' maps are glossier (Black Ops' uniforms read as gear).
    let gear = if has(&["goggle", "headset", "accessor", "radio", "nvg", "kevlar", "helmet", "mich", "gear"]) { 1.0 } else { 0.0 };
    Some((skin, gear))
}

/// Whether worn surfaces take this look: not switched off, baked lighting.
pub fn active() -> bool {
    mode() != "off" && crate::rtgi::lighting() == crate::rtgi::Lighting::Baked
}

/// Where `COD4RW_CHARLOOK=wipe` splits the screen (0..1 across):
/// `COD4RW_CHARLOOK_WIPE`, else the middle.
fn wipe_at() -> f32 {
    std::env::var("COD4RW_CHARLOOK_WIPE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5)
}

/// Debug view and A/B switch, `COD4RW_CHARLOOK`.
fn mode() -> &'static str {
    static MODE: LazyLock<String> = LazyLock::new(|| std::env::var("COD4RW_CHARLOOK").unwrap_or_default().to_ascii_lowercase());
    MODE.as_str()
}

/// The materials made, by the CoD4 material they're made from (`None`:
/// left as it is).
#[derive(Resource, Default)]
struct Looks {
    made: HashMap<AssetId<StandardMaterial>, Option<Handle<CharacterMaterial>>>,
    knobs: Option<[f32; 14]>,
    /// The probe and its light as last given out.
    shine: Option<(AssetId<Image>, Vec4)>,
}

fn knob_values() -> [f32; 14] {
    use crate::tune::get;
    [
        get("charlook.scatter", 1.0),
        get("charlook.scatter_r", 1.0),
        get("charlook.scatter_g", 0.32),
        get("charlook.scatter_b", 0.18),
        get("charlook.wrap", 0.5),
        get("charlook.skin_spec", 0.25),
        get("charlook.shade_lift", 0.12),
        get("charlook.sheen", 0.35),
        get("charlook.sheen_rough", 0.5),
        get("charlook.gear_spec", 1.0),
        get("charlook.cod4_spec", 0.5),
        get("charlook.flat_ambient", 0.0),
        get("charlook.fresnel_max", 2.0),
        match mode() {
            "mask" => 1.0,
            "wipe" => 3.0,
            "bands" => 4.0,
            "added" => 5.0,
            "sun" => 6.0,
            _ => 0.0,
        },
    ]
}

fn apply_knobs(look: &mut CharacterLook, k: &[f32; 14]) {
    look.class.w = k[13];
    look.more.x = k[11];
    look.more.z = k[12];
    look.skin = Vec4::new(k[1], k[2], k[3], k[0]);
    look.cloth = Vec4::new(k[7], k[8], k[9], k[5]);
    look.extra = Vec4::new(k[4], k[6], wipe_at(), k[10]);
}

/// Worn surfaces still on CoD4's material: on to theirs.
fn wear(
    mut commands: Commands,
    mut looks: ResMut<Looks>,
    surfaces: Query<(Entity, &MeshMaterial3d<StandardMaterial>), With<Worn>>,
    standard: Res<Assets<StandardMaterial>>,
    mut materials: ResMut<Assets<CharacterMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut textures: Local<crate::textures::TextureCache>,
) {
    if surfaces.is_empty() {
        return;
    }
    let knobs = *looks.knobs.get_or_insert_with(knob_values);
    for (e, m) in &surfaces {
        let id = m.0.id();
        let made = looks.made.entry(id).or_insert_with(|| {
            let base = standard.get(id)?;
            let Source { name, techset, specular, env } = SOURCES.lock().ok()?.get(&id).cloned()?;
            // An eye's highlight shell: in CoD4 only its specular shows (its
            // colour map is flat grey; drawn as such the eyes were milky).
            let eye = name.contains("eye_highlight");
            let blended = matches!(base.alpha_mode, AlphaMode::Blend | AlphaMode::Add | AlphaMode::Multiply | AlphaMode::Premultiplied);
            let classes = if eye { Some((0.0, 0.0)) } else if blended || base.unlit { None } else { classes(&name, &techset) };
            if std::env::var_os("COD4RW_CHARLOOK_LOG").is_some() {
                info!("charlook: {name} [{techset}] -> {classes:?}, specular {:?}", specular.as_ref().map(|s| &s.0));
            }
            let (skin, gear) = classes?;
            let mut base = base.clone();
            base.opaque_render_method = bevy::material::OpaqueRendererMethod::Forward;
            if eye {
                base.alpha_mode = AlphaMode::Add;
                base.unlit = false;
            }
            let specular = specular.and_then(|(n, vfs)| textures.get(&n, false, &vfs, &mut images));
            let mut look = CharacterLook { class: Vec4::new(skin, gear, specular.is_some() as u32 as f32, 0.0), specular, env, ..default() };
            apply_knobs(&mut look, &knobs);
            look.more.y = eye as u32 as f32;
            Some(materials.add(CharacterMaterial { base, extension: look }))
        });
        // Unknown or left as it is: no longer looked at.
        // (`try_`: a body can go the same frame, as a bot's stand-in models
        // give way to its own.)
        let mut entity = commands.entity(e);
        entity.try_remove::<Worn>();
        if let Some(h) = made.clone() {
            entity.try_remove::<MeshMaterial3d<StandardMaterial>>().try_insert(MeshMaterial3d(h));
        }
    }
}

/// The live knobs onto every character material, when they change.
fn knobs(mut looks: ResMut<Looks>, mut materials: ResMut<Assets<CharacterMaterial>>, mut wait: Local<f32>, time: Res<Time<Real>>) {
    *wait -= time.delta_secs();
    if *wait > 0.0 {
        return;
    }
    *wait = 0.5;
    let k = knob_values();
    if looks.knobs == Some(k) {
        return;
    }
    looks.knobs = Some(k);
    for h in looks.made.values().flatten() {
        if let Some(mut m) = materials.get_mut(h) {
            apply_knobs(&mut m.extension, &k);
        }
    }
}

/// `COD4RW_CHARLOOK=ab`: CoD4's shading while the autoshot's step is even
/// (`crate::autoshot`'s views: one per 1.5 s from 5 s, after
/// `COD4RW_SHOT_LATE`), this look while it's odd.
fn ab(looks: Res<Looks>, mut materials: ResMut<Assets<CharacterMaterial>>, time: Res<Time>, mut last: Local<Option<bool>>) {
    if mode() != "ab" {
        return;
    }
    let late: f32 = std::env::var("COD4RW_SHOT_LATE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let step = ((time.elapsed_secs() - late - 5.0) / 1.5).floor().max(0.0) as u32;
    let plain = step % 2 == 0;
    if *last == Some(plain) && !looks.is_changed() {
        return;
    }
    *last = Some(plain);
    for h in looks.made.values().flatten() {
        if let Some(mut m) = materials.get_mut(h) {
            m.extension.class.w = if plain { 2.0 } else { 0.0 };
        }
    }
}

/// Characters reflect the probe nearest the camera, as the guns do
/// (`crate::gunmodel`), at the probe's exposure (the showcase's sky and
/// rain dim it) and with the open air's light for dimming it in the shade.
fn follow_reflection_probe(
    probes: Option<Res<crate::world::ReflectionProbes>>,
    grid: Option<Res<crate::model_lighting::LightGridLookup>>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    mut looks: ResMut<Looks>,
    mut materials: ResMut<Assets<CharacterMaterial>>,
    mut known: Local<usize>,
) {
    let (Some(probes), Ok(camera)) = (probes, camera.single()) else { return };
    let at = camera.translation();
    let Some((_, probe, mip)) = probes.0.iter().min_by(|a, b| a.0.distance(at).total_cmp(&b.0.distance(at))) else { return };
    let exposure = crate::lightmaps::LIGHTMAP_EXPOSURE * crate::atmos::climate::light_share() * crate::model_lighting::soak_share();
    let open = grid.map_or(0.0, |g| g.open);
    let shine = Vec4::new(1.0, *mip, exposure, open);
    let same = |a: Vec4, b: Vec4| (a - b).abs().max_element() < 0.01 * b.abs().max_element().max(1e-3);
    // Only when it changes, or for new materials.
    let made = looks.made.values().flatten().count();
    if *known == made && looks.shine.as_ref().is_some_and(|(id, s)| *id == probe.id() && same(*s, shine)) {
        return;
    }
    *known = made;
    looks.shine = Some((probe.id(), shine));
    for h in looks.made.values().flatten() {
        if let Some(mut m) = materials.get_mut(h) {
            m.extension.probe = Some(probe.clone());
            m.extension.shine = shine;
        }
    }
}

/// [`SOURCES`] kept to materials that still exist (every 10 s): each map's
/// content registers its own, and they'd otherwise pile up across matches,
/// holding their archives (`Vfs`) open.
fn prune_sources(standard: Res<Assets<StandardMaterial>>, time: Res<Time<Real>>, mut wait: Local<f32>) {
    *wait -= time.delta_secs();
    if *wait > 0.0 {
        return;
    }
    *wait = 10.0;
    if let Ok(mut s) = SOURCES.lock() {
        s.retain(|id, _| standard.contains(*id));
    }
}
