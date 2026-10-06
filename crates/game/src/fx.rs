//! CoD4's effects, played from the game's own definitions ([`iw3::fx`]):
//! muzzle flashes on the guns, bullet impacts by surface, blood where
//! players are hit, and the marks bullets leave.
//!
//! As in the engine, a particle's place and look each frame follow in closed
//! form from what it spawned with and its element's graphs (velocity, colour,
//! size and spin over its life); the only collision is models stopping on
//! the floor. Sprites are batched:
//! every live sprite with one material (per render layer) goes into one mesh,
//! rebuilt each frame and sorted back to front, so the cost stays a handful
//! of draws however many bots fire. Omni lights come from a small pool of
//! point lights, nearest first. Impact marks are lit quads on the surface,
//! the oldest giving way to new ones.
//!
//! [`Effects::play`] plays an effect at a fixed place or bolted to an entity
//! (a model's tag, followed as it moves). Billboard and oriented sprites,
//! tails, omni lights, models (spent cases, which stop on the floor under
//! where they're thrown), runners (effects spawning effects), decals,
//! clouds (IW3's particle clouds: specks scattered through a growing ball,
//! like glass glints or blood mist) and trails (a cross-section swept along
//! the points a moving effect lays down, like a rocket's smoke) are drawn.
//! Particles play their child effects: along their path every so far
//! (`effectEmitted`, the dust behind flying debris), where they hit
//! something (`effectOnImpact`: a case coming to rest, a blood splat) and
//! where they die. Spot lights (which no multiplayer effect uses) and
//! sounds are skipped (gun and impact sounds come from [`crate::audio`]).

mod test;

use crate::collision::{self, SURFACE_NAMES, Surfaces};
use crate::combat::{HitLocation, Hitbox, Killed};
use crate::content::Content;
use crate::models::Skeleton;
use crate::player::MainCamera;
use crate::thirdperson::Body;
use crate::units::{self, INCH, u};
use crate::viewmodel::ViewModelRoot;
use crate::weapons::{ShotFired, WeaponDef, WeaponState};
use avian3d::prelude::*;
use bevy::asset::{AssetEventSystems, RenderAssetUsages};
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers, VisibilitySystems};
use bevy::ecs::system::SystemParam;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use iw3::fx::{ElemType, FleshImpact, FxEffectDef, FxElemDef, ImpactTable, Spawn, Visual, flags};
use iw3::zone::{AssetType, TextureSemantic};
use std::collections::{HashMap, VecDeque};

pub struct FxPlugin;

impl Plugin for FxPlugin {
    fn build(&self, app: &mut App) {
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("shaders/fx.wgsl"),
            std::path::Path::new(FX_SHADER),
            include_bytes!("shaders/fx.wgsl").as_slice(),
        );
        app.add_plugins(MaterialPlugin::<FxMaterial>::default())
            .add_systems(OnEnter(crate::state::GameState::InGame), reset.in_set(crate::state::Setup::Spawn))
            .add_message::<BulletImpact>()
            .add_systems(Update, (weapon_effects, bullet_impacts).chain().after(crate::weapons::WeaponSet).run_if(crate::state::in_game))
            .add_systems(
                PostUpdate,
                // Bolted effects follow this frame's poses; the meshes built
                // here are extracted this frame.
                run_effects
                    .after(TransformSystems::Propagate)
                    .before(VisibilitySystems::VisibilityPropagate)
                    .before(AssetEventSystems)
                    .run_if(crate::state::in_game),
            );
        test::register(app);
    }
}

/// Where `shaders/fx.wgsl` is registered in the `embedded://` asset source.
const FX_SHADER: &str = "cod4rw/fx.wgsl";

/// Effect sprites' brightness, times the camera's exposure: about one over
/// Bevy's default exposure (EV100 9.7), so a white sprite comes out as
/// bright as an unlit white world surface. Additive ones add what IW3's
/// gamma-space blending would have (see `shaders/fx.wgsl`).
const FX_BRIGHTNESS: f32 = 1000.0;

/// Live particles at most; new ones are dropped past this.
const MAX_PARTICLES: usize = 4096;
/// Point lights for omni light elements, given to the nearest.
const MAX_LIGHTS: usize = 6;
/// Model particles (spent cases) at most; the oldest make way.
const MAX_MODELS: usize = 96;
/// Left on the wall or floor behind someone killed by a bullet.
const BLOOD_SPLAT: &str = "impacts/flesh_hit_splat_large";

/// Impact marks kept (CoD4's `FX_MARKS_LIMIT`): they stay until newer ones
/// need the room, oldest first.
const MAX_DECALS: usize = 512;
/// How far below a mark with nothing in front or behind finds the floor
/// (CoD units).
const DECAL_FLOOR_REACH: f32 = 72.0;
/// Looping elements that would loop forever stop after this (ms).
const MAX_LOOPING_MSEC: f64 = 10_000.0;
/// How far back a map's ambient effect with a negative delay starts.
const AMBIENT_PREWARM_MSEC: f64 = 4_000.0;

/// How long an effect's looping elements go on (ms): its own looping life,
/// else (CoD4: until stopped) briefly, or for good for a map's ambient one.
fn looping_life(def: &FxEffectDef, forever: bool) -> f64 {
    match def.msec_looping_life {
        l if l > 0 => l as f64,
        _ if forever => f64::INFINITY,
        _ => MAX_LOOPING_MSEC,
    }
}
/// Point light intensity (lumens) per square metre of radius: about 1000 lux
/// a quarter of the radius out, a third of a well-lit wall's light.
const LIGHT_LUMENS_PER_M2: f32 = 800.0;

/// CoD's world gravity, units/s².
const GRAVITY: f32 = 800.0;

/// Specks in a particle cloud.
const CLOUD_POINTS: u32 = 48;

/// The engine's per-particle random numbers (`FXRAND_*`).
mod key {
    pub const VELOCITY: u32 = 0;
    pub const ANGULAR_VELOCITY: u32 = 3;
    pub const ORIGIN: u32 = 6;
    pub const OFFSET_YAW: u32 = 9;
    pub const OFFSET_HEIGHT: u32 = 10;
    pub const OFFSET_RADIUS: u32 = 11;
    pub const ANGLES: u32 = 12;
    pub const GRAVITY: u32 = 15;
    pub const LIFE_SPAN: u32 = 17;
    pub const SPAWN_DELAY: u32 = 18;
    pub const SPAWN_COUNT: u32 = 19;
    pub const VISUAL: u32 = 21;
    pub const TILE_START: u32 = 22;
    pub const COLOR: u32 = 23;
    pub const ROTATION: u32 = 24;
    pub const ROTATION_DELTA: u32 = 25;
    pub const SIZE_0: u32 = 26;
    pub const SIZE_1: u32 = 27;
    pub const SCALE: u32 = 28;
    pub const EMIT_DIST: u32 = 29;
    pub const EMIT_VARIANCE: u32 = 30;
}

/// A particle's random number for `key`, in [0, 1).
fn rand(seed: u32, key: u32) -> f32 {
    let mut x = seed ^ key.wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Which camera draws an effect: the world's, a local player's viewmodel
/// camera (their first-person gun's own flash), or in splitscreen the other
/// players' (a player's third-person gun's flash, seen by everyone else).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FxLayer {
    World,
    ViewModel(u8),
    Body(u8),
}

impl FxLayer {
    fn render_layers(self) -> RenderLayers {
        match self {
            FxLayer::World => RenderLayers::layer(0),
            FxLayer::ViewModel(slot) => RenderLayers::layer(crate::splitscreen::viewmodel_layer(slot as usize)),
            FxLayer::Body(slot) => RenderLayers::layer(crate::splitscreen::body_layer(slot as usize)),
        }
    }
}

/// A place and orientation in CoD's terms: the origin, and CoD's forward,
/// left and up axes (x, y, z) as Bevy directions.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub origin: Vec3,
    pub axis: Mat3,
}

impl Frame {
    /// At `origin` with CoD's world axes.
    fn world(origin: Vec3) -> Frame {
        Frame { origin, axis: units::basis() }
    }

    /// Facing `forward`, turned `roll` radians about it (impacts face out
    /// of the surface at a random roll, like `CG_RandomEffectAxis`).
    pub fn facing(origin: Vec3, forward: Vec3, roll: f32) -> Frame {
        let f = forward.try_normalize().unwrap_or(Vec3::Y);
        let hint = if f.y.abs() < 0.99 { Vec3::Y } else { Vec3::X };
        let left = hint.cross(f).normalize();
        let up = f.cross(left);
        let (s, c) = roll.sin_cos();
        Frame { origin, axis: Mat3::from_cols(f, left * c + up * s, up * c - left * s) }
    }

    /// A model joint's frame: models are loaded with CoD's x forward along
    /// Bevy +X, y left along -Z and z up along +Y.
    fn of(g: &GlobalTransform) -> Frame {
        let (_, r, t) = g.to_scale_rotation_translation();
        Frame { origin: t, axis: Mat3::from_cols(r * Vec3::X, r * Vec3::NEG_Z, r * Vec3::Y) }
    }

    /// A point given in CoD units along this frame's axes.
    fn point(&self, local: Vec3) -> Vec3 {
        self.origin + self.axis * local * INCH
    }

    /// The other way: CoD units along this frame's axes.
    fn local(&self, p: Vec3) -> Vec3 {
        self.axis.transpose() * ((p - self.origin) / INCH)
    }
}

/// Where an effect plays.
#[derive(Clone, Copy, Debug)]
pub enum Anchor {
    Fixed(Frame),
    /// Follows an entity's global transform (a model's tag).
    Bolted(Entity),
}

/// A loaded effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FxId(u32);

/// Effects playing, and everything loaded to play them.
#[derive(Resource, Default)]
pub struct Effects {
    defs: Vec<Loaded>,
    names: HashMap<String, Option<FxId>>,
    materials: Vec<Handle<FxMaterial>>,
    material_names: HashMap<String, Option<u16>>,
    /// Models' surfaces, and models by name.
    models: Vec<Vec<(Handle<Mesh>, Handle<StandardMaterial>)>>,
    model_names: HashMap<String, Option<u16>>,
    /// Live model particles.
    model_count: usize,
    decal_materials: HashMap<(String, [u8; 4]), Option<Handle<StandardMaterial>>>,
    impacts: Option<Option<ImpactTable>>,
    weapons: HashMap<String, WeaponFx>,
    queue: Vec<Play>,
    instances: Vec<Option<Instance>>,
    free: Vec<u32>,
    particles: Vec<Particle>,
    new_decals: Vec<DecalSpawn>,
    batches: HashMap<(u16, FxLayer), Batch>,
    lights: Vec<Entity>,
    decals: VecDeque<Entity>,
    decal_mesh: Option<Handle<Mesh>>,
    fatal: Vec<PendingFatal>,
    /// The effects clock, ms.
    now: f64,
    /// The camera: where it is and looks (for CoD4's spawn culling).
    eye: Option<(Vec3, Vec3)>,
    seed: u32,
    warmed: bool,
    /// Effects to load before they're needed, and the impact types done.
    preloads: Vec<(String, FxLayer)>,
    preloaded_impacts: Vec<i32>,
    /// Time spent updating and drawing, and frames (for `COD4RW_FXLOG`).
    busy: (std::time::Duration, u32),
}

impl Effects {
    /// Play the effect named `name` (as CoD names it,
    /// `impacts/large_concrete_1`) now.
    pub fn play(&mut self, name: &str, anchor: Anchor, layer: FxLayer) {
        let name = name.trim_start_matches(',');
        if name.is_empty() {
            return;
        }
        self.queue.push(Play { effect: EffectRef::Name(name.to_owned()), anchor, layer, at: None, forever: false });
    }

    /// Start a map's ambient effect (its createfx placements) at `frame`,
    /// `delay_ms` from now: CoD4's negative delays start it that long ago,
    /// so smoke is already up (a few seconds' worth here). Its looping
    /// elements go on for as long as the match.
    pub fn play_ambient(&mut self, name: &str, frame: Frame, delay_ms: f64) {
        let name = name.trim_start_matches(',');
        if name.is_empty() {
            return;
        }
        let at = self.now + delay_ms.max(-AMBIENT_PREWARM_MSEC);
        self.queue.push(Play { effect: EffectRef::Name(name.to_owned()), anchor: Anchor::Fixed(frame), layer: FxLayer::World, at: Some(at), forever: true });
    }

    fn next_seed(&mut self) -> u32 {
        self.seed = self.seed.wrapping_add(0x9e37_79b9);
        let mut x = self.seed;
        x ^= x >> 15;
        x.wrapping_mul(0x2c1b_3c6d)
    }
}

enum EffectRef {
    Name(String),
    Id(FxId),
}

struct Play {
    effect: EffectRef,
    anchor: Anchor,
    layer: FxLayer,
    /// A map's ambient effect: its looping elements never stop.
    forever: bool,
    /// When it starts (ms on the effects clock); `None` for this frame.
    at: Option<f64>,
}

impl Play {
    /// A particle's child effect, from now.
    fn child(name: &str, frame: Frame, layer: FxLayer) -> Play {
        Play { effect: EffectRef::Name(name.trim_start_matches(',').to_owned()), anchor: Anchor::Fixed(frame), layer, at: None, forever: false }
    }
}

/// An effect definition and what its elements' visuals resolved to.
struct Loaded {
    def: FxEffectDef,
    visuals: Vec<Vec<Vis>>,
}

#[derive(Clone)]
enum Vis {
    /// A sprite material, by index into [`Effects::materials`].
    Sprite(u16),
    /// A model, by index into [`Effects::models`].
    Model(u16),
    Effect(FxId),
    /// A decal's world material.
    Decal(String),
    /// Nothing to draw (lights, skipped materials, missing assets).
    None,
}

/// A playing effect: what its particles move with.
struct Instance {
    def: FxId,
    bolt: Option<Entity>,
    /// Where it is now (bolted ones move).
    frame: Frame,
    layer: FxLayer,
    start: f64,
    /// Looping elements: (element, spawned so far).
    looping: Vec<(u16, u32)>,
    live: u32,
    /// Looping for good (a map's ambient effect).
    forever: bool,
}

struct Particle {
    inst: u32,
    elem: u16,
    visual: u16,
    sequence: u16,
    seed: u32,
    /// Birth (ms) and life span (ms).
    begin: f64,
    life: f32,
    /// Where it started, CoD units along `frame`.
    origin: Vec3,
    /// The frame it runs in; [`Particle::follow`] ones use the effect's.
    frame: Frame,
    follow: bool,
    /// Drawn at least once: every particle shows for a frame, however short
    /// its life.
    drawn: bool,
    /// A model's entities (one per surface) and, for one that collides, the
    /// floor under where it was thrown (Bevy y) and where it came to rest.
    model: Option<ModelState>,
    /// Where it was last frame, how far it has come since it last emitted
    /// its `effectEmitted` and how far it goes between them (units), and
    /// whether it has hit something.
    last: Option<Vec3>,
    travel: f32,
    emit_every: f32,
    impacted: bool,
}

struct ModelState {
    entities: Vec<Entity>,
    floor: Option<f32>,
    rest: Option<(Vec3, Quat)>,
}

/// A model entity of an effect.
#[derive(Component)]
struct FxModel;

struct DecalSpawn {
    material: String,
    color: [u8; 4],
    frame: Frame,
    /// Half size in CoD units, and spin in radians.
    size: f32,
    rotation: f32,
}

/// One material's sprites on one layer: a mesh rebuilt every frame.
struct Batch {
    entity: Entity,
    mesh: Handle<Mesh>,
    /// Its mesh draws nothing (and needn't be rebuilt while that lasts).
    empty: bool,
}

#[derive(Component)]
pub(crate) struct FxBatch;

#[derive(Component)]
struct FxLight;

/// A hit that may yet turn out to have killed (the kill comes after the
/// shot): its exit wound plays then.
struct PendingFatal {
    victim: Entity,
    attacker: Entity,
    point: Vec3,
    dir: Vec3,
    head: bool,
    impact_type: i32,
    until: f64,
}

/// A weapon's effects, by name.
#[derive(Clone, Default, Debug)]
struct WeaponFx {
    view_flash: Option<String>,
    world_flash: Option<String>,
    /// Spent cases thrown out at `tag_brass`.
    view_shell: Option<String>,
    world_shell: Option<String>,
    /// Its first-person flash is loaded (only the player's gun needs it).
    preloaded_view: bool,
}

/// Effect sprites: a texture times the vertex colour, added or alpha
/// blended (see `shaders/fx.wgsl`).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct FxMaterial {
    /// x: 1 additive, 0 alpha blended; y: brightness; z: soft particle fade
    /// distance in metres (0: none).
    #[uniform(0)]
    params: Vec4,
    #[texture(1)]
    #[sampler(2)]
    texture: Handle<Image>,
}

impl Material for FxMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/fx.wgsl".into()
    }

    /// Both kinds go out premultiplied; additive ones with alpha 0.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    /// Sprites are seen from either side.
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// Asset collections loading and drawing effects needs.
#[derive(SystemParam)]
struct FxAssets<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    fx_materials: ResMut<'w, Assets<FxMaterial>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    images: ResMut<'w, Assets<Image>>,
}

fn reset(mut commands: Commands) {
    commands.insert_resource(Effects::default());
}

// ---------------------------------------------------------------------------
// Loading

impl Effects {
    /// Load an effect (and the effects and materials it uses) by name.
    fn load(&mut self, content: &mut Content, a: &mut FxAssets, name: &str) -> Option<FxId> {
        let name = name.trim_start_matches(',');
        if let Some(id) = self.names.get(name) {
            return *id;
        }
        // No cycles: an effect is missing while its own parts load.
        self.names.insert(name.to_owned(), None);
        let Some((zi, node)) = content.generic(AssetType::Fx, name) else {
            warn!("effect {name} not found");
            return None;
        };
        let def = FxEffectDef::from_node(&content.zones[zi], node);
        let visuals =
            def.elems.iter().map(|e| e.visuals.iter().map(|v| self.visual(content, a, e, v)).collect()).collect();
        let id = FxId(self.defs.len() as u32);
        debug!("effect {name}: {} elements", def.elems.len());
        self.defs.push(Loaded { def, visuals });
        self.names.insert(name.to_owned(), Some(id));
        Some(id)
    }

    fn visual(&mut self, content: &mut Content, a: &mut FxAssets, e: &FxElemDef, v: &Visual) -> Vis {
        match (e.elem_type, v) {
            (t, Visual::Material(m)) if t.is_sprite() || matches!(t, ElemType::Cloud | ElemType::Trail) => {
                self.material(content, a, m).map_or(Vis::None, Vis::Sprite)
            }
            (ElemType::Runner, Visual::Effect(name)) => self.load(content, a, name).map_or(Vis::None, Vis::Effect),
            (ElemType::Decal, Visual::Decal([_, world])) => world.clone().map_or(Vis::None, Vis::Decal),
            (ElemType::Model, Visual::Model(name)) => self.model(content, a, name).map_or(Vis::None, Vis::Model),
            _ => Vis::None,
        }
    }

    /// A sprite material: its colour map, and how it blends (from the
    /// material's emissive technique state bits, IW3's `GFXS_BLEND_*`).
    fn material(&mut self, content: &mut Content, a: &mut FxAssets, name: &str) -> Option<u16> {
        let name = name.trim_start_matches(',');
        if let Some(m) = self.material_names.get(name) {
            return *m;
        }
        let made = make_material(content, a, name).map(|m| {
            self.materials.push(a.fx_materials.add(m));
            (self.materials.len() - 1) as u16
        });
        if made.is_none() {
            debug!("effect material {name} skipped");
        }
        self.material_names.insert(name.to_owned(), made);
        made
    }

    /// A model's surfaces (its first LOD), unskinned.
    fn model(&mut self, content: &mut Content, a: &mut FxAssets, name: &str) -> Option<u16> {
        let name = name.trim_start_matches(',');
        if let Some(m) = self.model_names.get(name) {
            return *m;
        }
        let found = content
            .zones
            .iter()
            .enumerate()
            .find_map(|(zi, z)| z.find(name).filter(|&id| z.xmodel(id).is_some()).map(|id| (zi, id)));
        let parts = found.and_then(|(zi, id)| {
            let xm = content.zones[zi].xmodel(id)?;
            let lod = xm.lods.first()?;
            let surfs: Vec<(usize, Option<iw3::zone::AssetId>)> = (lod.surf_index as usize
                ..(lod.surf_index + lod.num_surfs) as usize)
                .map(|s| (s, xm.materials.get(s).copied().flatten()))
                .collect();
            let parts: Vec<_> = surfs
                .into_iter()
                .filter_map(|(surf, mat)| {
                    let mesh = content.static_mesh(zi, id, surf, &mut a.meshes)?;
                    let mat = content.material(zi, mat?, &mut a.materials, &mut a.images)?;
                    Some((mesh, mat.handle))
                })
                .collect();
            (!parts.is_empty()).then_some(parts)
        });
        let made = parts.map(|p| {
            self.models.push(p);
            (self.models.len() - 1) as u16
        });
        if made.is_none() {
            warn!("effect model {name} not found");
        }
        self.model_names.insert(name.to_owned(), made);
        made
    }

    /// An impact mark's material: the world one, tinted.
    fn decal_material(
        &mut self,
        content: &mut Content,
        a: &mut FxAssets,
        name: &str,
        color: [u8; 4],
    ) -> Option<Handle<StandardMaterial>> {
        let key = (name.to_owned(), color);
        if let Some(m) = self.decal_materials.get(&key) {
            return m.clone();
        }
        let name = name.trim_start_matches(',');
        let made = find_material(content, name)
            .and_then(|(zi, id)| content.material(zi, id, &mut a.materials, &mut a.images))
            .map(|info| {
                if color == [255; 4] {
                    return info.handle;
                }
                let mut m = a.materials.get(&info.handle).cloned().unwrap_or_default();
                let [r, g, b, al] = color.map(|c| c as f32 / 255.0);
                m.base_color = Color::srgba(r, g, b, al);
                a.materials.add(m)
            });
        self.decal_materials.insert(key, made.clone());
        made
    }
}

/// A material asset by name, in whichever zone defines it.
fn find_material(content: &Content, name: &str) -> Option<(usize, iw3::zone::AssetId)> {
    content
        .zones
        .iter()
        .enumerate()
        .find_map(|(zi, z)| z.find(name).filter(|&id| z.material(id).is_some()).map(|id| (zi, id)))
}

fn make_material(content: &mut Content, a: &mut FxAssets, name: &str) -> Option<FxMaterial> {
    let (zi, id) = find_material(content, name)?;
    let zone = &content.zones[zi];
    let mat = zone.material(id)?;
    let techset = mat.technique_set.and_then(|t| zone.technique_set(t)).map_or("", |t| t.name.as_str());
    // Heat haze needs the frame behind it; there's no such pass here.
    if techset.contains("distortion") {
        return None;
    }
    let bits = [iw3::zone::TECHNIQUE_EMISSIVE, iw3::zone::TECHNIQUE_UNLIT]
        .into_iter()
        .find_map(|t| mat.state_bits_for(t))
        .or_else(|| mat.state_bits.first().copied())
        .unwrap_or([0, 0]);
    let (src, dst) = (bits[0] & 0xf, (bits[0] >> 4) & 0xf);
    // ONE/ONE and INVDESTCOLOR/ONE (a screen) add; SRCALPHA/INVSRCALPHA and
    // alpha-tested ones blend.
    let additive = dst == 2 && matches!(src, 2 | 5 | 10);
    // `featherParms.x` is one over the soft particle fade distance.
    let feather = mat
        .constants
        .iter()
        .find(|c| c.name == "featherParms")
        .filter(|c| c.literal[0] > 0.0)
        .map_or(0.0, |c| u(1.0 / c.literal[0]));
    let texture = content.material_texture(zi, id, TextureSemantic::Color, true, &mut a.images).or_else(|| {
        // Some effects only have a "2D" map: the standard material finds it.
        let info = content.material(zi, id, &mut a.materials, &mut a.images)?;
        a.materials.get(&info.handle)?.base_color_texture.clone()
    })?;
    Some(FxMaterial { params: Vec4::new(if additive { 1.0 } else { 0.0 }, FX_BRIGHTNESS, feather, 0.0), texture })
}

// ---------------------------------------------------------------------------
// Spawning

impl Effects {
    /// Start an effect: spawn its one-shot elements and set up its looping
    /// ones.
    fn start(&mut self, id: FxId, frame: Frame, bolt: Option<Entity>, layer: FxLayer, at: f64, forever: bool) {
        let seed = self.next_seed();
        let def = &self.defs[id.0 as usize].def;
        let looping = def.looping().map(|i| (i as u16, 0)).collect();
        let instance = Instance { def: id, bolt, frame, layer, start: at, looping, live: 0, forever };
        let slot = match self.free.pop() {
            Some(s) => {
                self.instances[s as usize] = Some(instance);
                s
            }
            None => {
                self.instances.push(Some(instance));
                (self.instances.len() - 1) as u32
            }
        };
        for elem in self.defs[id.0 as usize].def.one_shot() {
            let e = &self.defs[id.0 as usize].def.elems[elem];
            let count = match e.spawn {
                // One count for the whole effect, as in the engine.
                Spawn::OneShot { count } => count.at(rand(seed, key::SPAWN_COUNT)),
                Spawn::Looping { .. } => 1,
            };
            // The settings' density: fewer particles from the big bursts.
            let count = if count > 2 { ((count as f32 * crate::settings_apply::fx_density()).round() as i32).max(1) } else { count };
            for sequence in 0..count.clamp(0, 256) {
                self.spawn_elem(slot, elem, frame, at, sequence as u16);
            }
        }
        self.spawn_looping(slot);
        self.release_if_done(slot);
    }

    /// Spawn the looping elements due by now.
    fn spawn_looping(&mut self, slot: u32) {
        let Some(inst) = self.instances[slot as usize].as_mut() else { return };
        let def = &self.defs[inst.def.0 as usize].def;
        let life = looping_life(def, inst.forever);
        let (start, frame) = (inst.start, inst.frame);
        let mut due = Vec::new();
        for (elem, spawned) in inst.looping.iter_mut() {
            let Spawn::Looping { interval_msec, count } = def.elems[*elem as usize].spawn else { continue };
            let interval = interval_msec.max(1) as f64;
            loop {
                let t = *spawned as f64 * interval;
                if *spawned >= count.max(0) as u32 || t > life || start + t > self.now {
                    break;
                }
                due.push((*elem as usize, start + t, *spawned as u16));
                *spawned += 1;
            }
        }
        // The settings' density: some of the stream's particles left out.
        let keep = crate::settings_apply::fx_density();
        for (elem, at, sequence) in due {
            if keep < 1.0 && (sequence as f32 * keep).floor() == ((sequence as f32 + 1.0) * keep).floor() {
                continue;
            }
            self.spawn_elem(slot, elem, frame, at, sequence);
        }
    }

    /// Whether an instance's looping elements have all spawned.
    fn looping_done(&self, inst: &Instance) -> bool {
        let def = &self.defs[inst.def.0 as usize].def;
        let life = looping_life(def, inst.forever);
        inst.looping.iter().all(|&(elem, spawned)| match def.elems[elem as usize].spawn {
            Spawn::Looping { interval_msec, count } => {
                spawned >= count.max(0) as u32 || spawned as f64 * interval_msec.max(1) as f64 > life
            }
            Spawn::OneShot { .. } => true,
        })
    }

    fn release_if_done(&mut self, slot: u32) {
        let done = self.instances[slot as usize].as_ref().is_some_and(|i| i.live == 0 && self.looping_done(i));
        if done {
            self.instances[slot as usize] = None;
            self.free.push(slot);
        }
    }

    /// Spawn one particle of an element (`FX_SpawnElem`): runners start
    /// their effect, decals leave a mark, the rest become particles.
    fn spawn_elem(&mut self, slot: u32, elem: usize, frame: Frame, when: f64, sequence: u16) {
        let seed = self.next_seed();
        let r = |k: u32| rand(seed, k);
        let Some(inst) = self.instances[slot as usize].as_ref() else { return };
        let (layer, bolt) = (inst.layer, inst.bolt);
        let loaded = &self.defs[inst.def.0 as usize];
        let e = &loaded.def.elems[elem];
        if self.eye.is_some_and(|eye| culled_for_spawn(e, frame.origin, eye)) {
            return;
        }
        let visuals = &loaded.visuals[elem];
        let visual = if visuals.len() > 1 {
            ((r(key::VISUAL) * visuals.len() as f32) as usize).min(visuals.len() - 1)
        } else {
            0
        };
        let vis = visuals.get(visual).cloned().unwrap_or(Vis::None);
        let begin = when + e.spawn_delay_msec.at(r(key::SPAWN_DELAY)) as f64;
        let life = e.life_span_msec.at(r(key::LIFE_SPAN));
        match e.elem_type {
            ElemType::Runner => {
                let Vis::Effect(id) = vis else { return };
                let mut child = Frame { origin: spawn_point(e, &frame, &r), axis: frame.axis };
                if e.has(flags::RUNNER_USES_RAND_ROT) {
                    let roll = r(key::ROTATION) * std::f32::consts::TAU;
                    child = Frame::facing(child.origin, frame.axis.col(0), roll);
                }
                let anchor = match bolt {
                    Some(b) => Anchor::Bolted(b),
                    None => Anchor::Fixed(child),
                };
                self.queue.push(Play { effect: EffectRef::Id(id), anchor, layer, at: Some(begin), forever: false });
            }
            ElemType::Decal => {
                let Vis::Decal(material) = vis else { return };
                let Some(s) = e.vis_samples.first() else { return };
                let rc = r(key::COLOR);
                let color = std::array::from_fn(|c| {
                    (s.base.color[c] as f32 * (1.0 - rc) + s.amplitude.color[c] as f32 * rc).round() as u8
                });
                let size = s.base.size[0] + s.amplitude.size[0] * r(key::SIZE_0);
                let rotation = e.initial_rotation.at(r(key::COLOR));
                self.new_decals.push(DecalSpawn { material, color, frame, size, rotation });
            }
            ElemType::BillboardSprite
            | ElemType::OrientedSprite
            | ElemType::Tail
            | ElemType::Cloud
            | ElemType::Trail
            | ElemType::OmniLight
            | ElemType::Model => {
                // Its whole life over before it began (`FX_SpawnElem`).
                if begin + life as f64 <= self.now || self.particles.len() >= MAX_PARTICLES {
                    return;
                }
                let drawable = match e.elem_type {
                    ElemType::OmniLight => true,
                    ElemType::Model => matches!(vis, Vis::Model(_)),
                    _ => matches!(vis, Vis::Sprite(_)),
                };
                if !drawable {
                    return;
                }
                // A trail lays a new point only once it has moved its split
                // distance from the last.
                if e.elem_type == ElemType::Trail {
                    let split = u(e.trail.as_ref().map_or(0.0, |t| t.split_dist));
                    let here = spawn_point(e, &frame, &r);
                    let last = self.particles.iter().rev().find(|p| p.inst == slot && p.elem == elem as u16);
                    if last.is_some_and(|p| p.frame.point(p.origin).distance(here) < split) {
                        return;
                    }
                }
                if e.elem_type == ElemType::Model {
                    if self.model_count >= MAX_MODELS {
                        let oldest = self
                            .particles
                            .iter_mut()
                            .filter(|p| p.model.is_some() && p.life > 0.0)
                            .min_by(|a, b| a.begin.total_cmp(&b.begin));
                        if let Some(p) = oldest {
                            p.life = 0.0;
                        }
                    }
                    self.model_count += 1;
                }
                let model = (e.elem_type == ElemType::Model).then(|| ModelState {
                    entities: Vec::new(),
                    floor: None,
                    rest: None,
                });
                let (frame, origin, follow) = run_frame(e, &frame, &r);
                let emit_every = e.emit_dist.at(r(key::EMIT_DIST)) + e.emit_dist_variance.at(r(key::EMIT_VARIANCE));
                self.particles.push(Particle {
                    inst: slot,
                    elem: elem as u16,
                    visual: visual as u16,
                    sequence,
                    seed,
                    begin,
                    life: life.max(1) as f32,
                    origin,
                    frame,
                    follow,
                    drawn: false,
                    model,
                    last: None,
                    travel: 0.0,
                    emit_every,
                    impacted: false,
                });
                if let Some(inst) = self.instances[slot as usize].as_mut() {
                    inst.live += 1;
                }
            }
            _ => {}
        }
    }
}

/// Where an element spawns (`FX_GetSpawnOrigin` + `FX_OffsetSpawnOrigin`),
/// in Bevy space.
fn spawn_point(e: &FxElemDef, frame: &Frame, r: &impl Fn(u32) -> f32) -> Vec3 {
    spawn_origin(e, frame, r) + spawn_offset(e, frame, r)
}

fn spawn_origin(e: &FxElemDef, frame: &Frame, r: &impl Fn(u32) -> f32) -> Vec3 {
    let o = Vec3::from_array(std::array::from_fn(|i| e.spawn_origin[i].at(r(key::ORIGIN + i as u32))));
    if e.has(flags::SPAWN_RELATIVE_TO_EFFECT) { frame.point(o) } else { frame.origin + units::basis() * o * INCH }
}

/// The spawn offset: a random direction (world axes) for spheres, a random
/// point on a circle round the effect's forward axis for cylinders.
fn spawn_offset(e: &FxElemDef, frame: &Frame, r: &impl Fn(u32) -> f32) -> Vec3 {
    let radius = e.spawn_offset_radius.at(r(key::OFFSET_RADIUS));
    let yaw = r(key::OFFSET_YAW) * std::f32::consts::TAU;
    match e.flags & flags::SPAWN_OFFSET_MASK {
        flags::SPAWN_OFFSET_SPHERE => {
            let h = 2.0 * r(key::OFFSET_HEIGHT) - 1.0;
            let s = (1.0 - h * h).max(0.0).sqrt();
            units::basis() * Vec3::new(s * yaw.cos(), s * yaw.sin(), h) * radius * INCH
        }
        flags::SPAWN_OFFSET_CYLINDER => {
            let height = e.spawn_offset_height.at(r(key::OFFSET_HEIGHT));
            (frame.axis.col(1) * radius * yaw.cos()
                + frame.axis.col(2) * radius * yaw.sin()
                + frame.axis.col(0) * height)
                * INCH
        }
        _ => Vec3::ZERO,
    }
}

/// The frame a particle runs in and its start along it (`FX_GetOrientation`,
/// `FX_GetOriginForElem`); whether it follows the effect as it moves.
fn run_frame(e: &FxElemDef, frame: &Frame, r: &impl Fn(u32) -> f32) -> (Frame, Vec3, bool) {
    match e.run_frame() {
        flags::RUN_RELATIVE_TO_SPAWN => (*frame, frame.local(spawn_point(e, frame, r)), false),
        flags::RUN_RELATIVE_TO_EFFECT => (*frame, frame.local(spawn_point(e, frame, r)), true),
        flags::RUN_RELATIVE_TO_OFFSET => {
            // Facing out along the offset from the spawn origin.
            let offset = spawn_offset(e, frame, r);
            let origin = spawn_origin(e, frame, r) + offset;
            let fwd = offset.try_normalize().unwrap_or(frame.axis.col(0));
            let up = if e.flags & flags::SPAWN_OFFSET_MASK == flags::SPAWN_OFFSET_CYLINDER && fwd.y.abs() >= 0.999 {
                Vec3::NEG_Z
            } else {
                Vec3::Y
            };
            let left = up.cross(fwd).try_normalize().unwrap_or(fwd.any_orthonormal_vector());
            (Frame { origin, axis: Mat3::from_cols(fwd, left, fwd.cross(left)) }, Vec3::ZERO, false)
        }
        // World: CoD's world axes (local velocities are world ones too).
        _ => {
            let world = Frame::world(frame.origin);
            (world, world.local(spawn_point(e, frame, r)), false)
        }
    }
}

// ---------------------------------------------------------------------------
// Evaluating

/// Where along a graph of `n + 1` samples normalised time `t` falls: the
/// interval and how far into it.
fn interval(samples: usize, t: f32) -> (usize, f32) {
    let n = samples.saturating_sub(1).max(1);
    let p = t.clamp(0.0, 1.0) * n as f32;
    let k = (p.floor() as usize).min(n - 1);
    (k, (p - k as f32).min(1.0))
}

/// Distance moved by `t` (CoD units, local and world graphs): the engine's
/// `FX_IntegrateVelocity`, from the samples' running totals and the
/// piecewise-linear velocity within the interval.
fn displacement(e: &FxElemDef, rv: [f32; 3], t: f32, life: f32) -> (Vec3, Vec3) {
    let s = &e.vel_samples;
    if s.len() < 2 {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let (k, x) = interval(s.len(), t);
    let (w1, w0) = (0.5 * x * x, x - 0.5 * x * x);
    let at = |r: &iw3::fx::Vec3Range| Vec3::from_array(r.at(rv));
    let local = if e.has(flags::HAS_VELOCITY_GRAPH_LOCAL) {
        (at(&s[k].local_total) + at(&s[k].local_velocity) * w0 + at(&s[k + 1].local_velocity) * w1) * life
    } else {
        Vec3::ZERO
    };
    let world = if e.has(flags::HAS_VELOCITY_GRAPH_WORLD) {
        (at(&s[k].world_total) + at(&s[k].world_velocity) * w0 + at(&s[k + 1].world_velocity) * w1) * life
    } else {
        Vec3::ZERO
    };
    (local, world)
}

/// Velocity at `t` as a Bevy direction (units/s), for tails
/// (`FX_GetVelocityAtTime`): the graphs plus gravity so far.
fn velocity(e: &FxElemDef, rv: [f32; 3], t: f32, frame: &Frame, gravity: f32, age_s: f32) -> Vec3 {
    let s = &e.vel_samples;
    let mut v = Vec3::ZERO;
    if s.len() >= 2 {
        let (k, x) = interval(s.len(), t);
        let n = (s.len() - 1) as f32;
        let at = |r: &iw3::fx::Vec3Range| Vec3::from_array(r.at(rv));
        if e.has(flags::HAS_VELOCITY_GRAPH_LOCAL) {
            v += frame.axis * at(&s[k].local_velocity).lerp(at(&s[k + 1].local_velocity), x) * n * 1000.0;
        }
        if e.has(flags::HAS_VELOCITY_GRAPH_WORLD) {
            v += units::basis() * at(&s[k].world_velocity).lerp(at(&s[k + 1].world_velocity), x) * n * 1000.0;
        }
    }
    v - Vec3::Y * gravity * GRAVITY * age_s
}

/// A particle's look at one moment.
struct Look {
    /// Linear RGB and alpha.
    color: [f32; 4],
    size: [f32; 2],
    rotation: f32,
    /// Models' scale.
    scale: f32,
}

/// `FX_EvaluateVisualState`: colours blend between the sample's base and
/// amplitude colours by one random number; sizes and spin are base plus
/// amplitude times their own.
fn look(e: &FxElemDef, seed: u32, t: f32, life: f32) -> Look {
    let s = &e.vis_samples;
    if s.is_empty() {
        return Look { color: [1.0; 4], size: [0.0; 2], rotation: 0.0, scale: 1.0 };
    }
    let (k, x) = interval(s.len(), t);
    let (a, b) = (&s[k], &s[(k + 1).min(s.len() - 1)]);
    let rc = rand(seed, key::COLOR);
    let channel = |c: usize| {
        let from = a.base.color[c] as f32 * (1.0 - rc) + a.amplitude.color[c] as f32 * rc;
        let to = b.base.color[c] as f32 * (1.0 - rc) + b.amplitude.color[c] as f32 * rc;
        (from + (to - from) * x) / 255.0
    };
    let size = |c: usize, key: u32| {
        let r = rand(seed, key);
        let from = a.base.size[c] + a.amplitude.size[c] * r;
        let to = b.base.size[c] + b.amplitude.size[c] * r;
        from + (to - from) * x
    };
    let w = size(0, key::SIZE_0);
    let h = if e.has(flags::NONUNIFORM_SCALE) { size(1, key::SIZE_1) } else { w };
    // Spin integrates like velocity: totals plus the rate within the interval.
    let rr = rand(seed, key::ROTATION_DELTA);
    let (w1, w0) = (0.5 * x * x, x - 0.5 * x * x);
    let spin = (a.base.rotation_total + a.amplitude.rotation_total * rr)
        + (a.base.rotation_delta + a.amplitude.rotation_delta * rr) * w0
        + (b.base.rotation_delta + b.amplitude.rotation_delta * rr) * w1;
    let rs = rand(seed, key::SCALE);
    let scale = (a.base.scale + a.amplitude.scale * rs) * (1.0 - x) + (b.base.scale + b.amplitude.scale * rs) * x;
    let srgb = |v: f32| Color::srgb(v, v, v).to_linear().red;
    Look {
        color: [srgb(channel(0)), srgb(channel(1)), srgb(channel(2)), channel(3)],
        size: [w, h],
        rotation: e.initial_rotation.at(rc) + spin * life,
        scale,
    }
}

/// A model's rotation (`FX_GetElemAxis`): its spawn angles (pitch, yaw,
/// roll) turning at their rates, in its frame. Models are loaded with CoD's
/// axes as Bevy's (x, -z, y).
fn model_rotation(e: &FxElemDef, seed: u32, frame: &Frame, age_ms: f32) -> Quat {
    let angle = |i: usize| {
        e.spawn_angles[i].at(rand(seed, key::ANGLES + i as u32))
            + e.angular_velocity[i].at(rand(seed, key::ANGULAR_VELOCITY + i as u32)) * age_ms
    };
    let (sp, cp) = angle(0).sin_cos();
    let (sy, cy) = angle(1).sin_cos();
    let (sr, cr) = angle(2).sin_cos();
    let forward = Vec3::new(cp * cy, cp * sy, -sp);
    let left = Vec3::new(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, sr * cp);
    let up = Vec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp);
    let axes = frame.axis * Mat3::from_cols(forward, left, up);
    Quat::from_mat3(&(axes * units::basis().transpose())).normalize()
}

/// `FX_CullElemForSpawn`: an element isn't spawned with the camera outside
/// its `spawnRange` (units), nor (flag 4) while a sphere of its
/// `spawnFrustumCullRadius` round the effect is out of view (here: behind
/// the camera, or well off to the side).
fn culled_for_spawn(e: &FxElemDef, origin: Vec3, (eye, forward): (Vec3, Vec3)) -> bool {
    let to = origin - eye;
    if e.spawn_range.amplitude != 0.0 {
        let d = to.length() / units::INCH - e.spawn_range.base;
        if d < 0.0 || d > e.spawn_range.amplitude {
            return true;
        }
    }
    if e.flags & 4 != 0 {
        let radius = u(e.spawn_frustum_cull_radius.max(0.0));
        let ahead = to.dot(forward);
        if ahead < -radius {
            return true;
        }
        // Wider than any field of view (about 70 degrees each way).
        let side = (to - forward * ahead).length();
        if side - radius > ahead.max(0.0) * 2.8 {
            return true;
        }
    }
    false
}

/// `FX_EvaluateDistanceFade`: fading in and out with distance (CoD units).
fn distance_fade(e: &FxElemDef, dist: f32) -> f32 {
    let ramp = |r: &iw3::fx::FloatRange| {
        let d = dist - r.base;
        if d < 0.0 {
            1.0
        } else if r.amplitude > d {
            1.0 - d / r.amplitude
        } else {
            0.0
        }
    };
    let fade_in = if e.fade_in_range.amplitude != 0.0 { ramp(&e.fade_in_range) } else { 1.0 };
    let fade_out = if e.fade_out_range.amplitude != 0.0 { 1.0 - ramp(&e.fade_out_range) } else { 1.0 };
    fade_in.min(fade_out)
}

/// The atlas frame's texture rectangle (`FX_GetSpriteTexCoords`): (s0, t0,
/// ds, dt).
fn atlas_rect(e: &FxElemDef, seed: u32, sequence: u16, t: f32, age_ms: f32) -> [f32; 4] {
    let at = &e.atlas;
    let count = at.entry_count.max(1) as i32;
    if count == 1 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let mut index = match at.behavior & iw3::fx::atlas::START_MASK {
        iw3::fx::atlas::START_RANDOM => (count as f32 * rand(seed, key::TILE_START)) as i32,
        iw3::fx::atlas::START_INDEXED => sequence as i32 & (count - 1),
        _ => at.index as i32,
    };
    if at.behavior & iw3::fx::atlas::PLAY_OVER_LIFE != 0 {
        index += (count as f32 * t) as i32;
    } else if at.fps != 0 {
        index += at.fps as i32 * age_ms as i32 / 1000;
    }
    if at.behavior & iw3::fx::atlas::LOOP_ONLY_N_TIMES != 0 && index >= count * at.loop_count as i32 {
        index = count - 1;
    }
    let index = index & (count - 1);
    let (cols, rows) = (1 << at.col_index_bits, 1 << at.row_index_bits);
    let (ds, dt) = (1.0 / cols as f32, 1.0 / rows as f32);
    [(index & (cols - 1)) as f32 * ds, (index >> at.col_index_bits) as f32 * dt, ds, dt]
}

/// A trail's points this frame, oldest first once sorted.
struct TrailRun {
    material: u16,
    layer: FxLayer,
    def: FxId,
    elem: u16,
    points: Vec<TrailPoint>,
}

struct TrailPoint {
    begin: f64,
    pos: Vec3,
    /// Half width, CoD units.
    size: f32,
    color: [f32; 4],
}

/// Sweep a trail's cross-section along its points (`FX_DrawElem_Trail`):
/// each pair of indices is a strip, stretched between consecutive points
/// across the path (sized by each point's size), its texture repeating
/// every `repeat_dist` units along and scrolling with time.
fn sweep_trail(
    trail: &iw3::fx::TrailDef,
    mut run: TrailRun,
    now: f64,
    view: &View,
    quads: &mut HashMap<(u16, FxLayer), Vec<Quad>>,
) {
    run.points.sort_by(|a, b| a.begin.total_cmp(&b.begin));
    let scroll = if trail.scroll_time_msec != 0 { (now / trail.scroll_time_msec as f64).rem_euclid(1.0) as f32 } else { 0.0 };
    let repeat = trail.repeat_dist.max(1.0);
    let list = quads.entry((run.material, run.layer)).or_default();
    let mut along = 0.0;
    for w in run.points.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let seg = b.pos - a.pos;
        let len = seg.length() / INCH;
        let Some(dir) = seg.try_normalize() else { continue };
        let right = dir.cross(Vec3::Y).try_normalize().unwrap_or(view.right);
        let up = right.cross(dir);
        let at = |p: &TrailPoint, v: &iw3::fx::TrailVertex| p.pos + (right * v.pos[0] + up * v.pos[1]) * p.size * INCH;
        let (va, vb) = (along / repeat + scroll, (along + len) / repeat + scroll);
        // Each end its own point's colour, so the ribbon fades along it (and
        // in from the newest point, born clear).
        let colors = [a.color, a.color, b.color, b.color];
        if colors.iter().all(|c| c[3] <= 0.0) {
            along += len;
            continue;
        }
        for pair in trail.inds.chunks_exact(2) {
            let (Some(v0), Some(v1)) = (trail.verts.get(pair[0] as usize), trail.verts.get(pair[1] as usize)) else { continue };
            let verts = [at(a, v0), at(a, v1), at(b, v1), at(b, v0)];
            let uv = [Vec2::new(v0.tex_coord, va), Vec2::new(v1.tex_coord, va), Vec2::new(v1.tex_coord, vb), Vec2::new(v0.tex_coord, vb)];
            let depth = ((a.pos + b.pos) * 0.5 - view.pos).dot(view.forward);
            list.push(Quad { verts, uv, colors, depth });
        }
        along += len;
    }
}

/// One sprite quad in a batch.
struct Quad {
    verts: [Vec3; 4],
    uv: [Vec2; 4],
    /// Per corner (a sprite's are all one).
    colors: [[f32; 4]; 4],
    /// Distance along the view, for sorting.
    depth: f32,
}

/// A point light wanted this frame.
struct LightWant {
    pos: Vec3,
    radius: f32,
    color: Vec3,
    layer: FxLayer,
    dist: f32,
}

/// The camera's place and axes.
struct View {
    pos: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
}

/// `FX_GenSpriteVerts`: a quad of half size `size` about `pos` along
/// tangent and binormal, turned by `rotation`.
fn sprite_quad(
    pos: Vec3,
    tangent: Vec3,
    binormal: Vec3,
    size: [f32; 2],
    rotation: f32,
    rect: [f32; 4],
) -> ([Vec3; 4], [Vec2; 4]) {
    let (sin, cos) = rotation.sin_cos();
    let rt = tangent * cos + binormal * sin;
    let rb = tangent * sin - binormal * cos;
    let (left, up) = (rt * size[0] * INCH, rb * size[1] * INCH);
    let [s0, t0, ds, dt] = rect;
    (
        [pos - left + up, pos - left - up, pos + left - up, pos + left + up],
        [Vec2::new(s0, t0 + dt), Vec2::new(s0, t0), Vec2::new(s0 + ds, t0), Vec2::new(s0 + ds, t0 + dt)],
    )
}

// ---------------------------------------------------------------------------
// The frame's update

#[allow(clippy::too_many_arguments)]
fn run_effects(
    mut commands: Commands,
    time: Res<Time>,
    fx: Option<ResMut<Effects>>,
    content: Option<ResMut<Content>>,
    mut assets: FxAssets,
    globals: Query<&GlobalTransform, (Without<FxBatch>, Without<FxLight>, Without<FxModel>)>,
    camera: Query<Entity, With<MainCamera>>,
    mut batches: Query<(&mut Transform, &mut GlobalTransform), (With<FxBatch>, Without<FxLight>, Without<FxModel>)>,
    mut lights: Query<
        (&mut PointLight, &mut Transform, &mut GlobalTransform, &mut Visibility, &mut RenderLayers),
        (With<FxLight>, Without<FxBatch>, Without<FxModel>),
    >,
    mut models: Query<(&mut Transform, &mut GlobalTransform), (With<FxModel>, Without<FxBatch>, Without<FxLight>)>,
    spatial: SpatialQuery,
) {
    let (Some(mut fx), Some(mut content)) = (fx, content) else { return };
    let started = std::time::Instant::now();
    let fx = &mut *fx;
    fx.now = time.elapsed_secs_f64() * 1000.0;
    let now = fx.now;
    let Some(view) = camera.single().ok().and_then(|c| globals.get(c).ok()).map(|g| View {
        pos: g.translation(),
        right: g.right().as_vec3(),
        up: g.up().as_vec3(),
        forward: g.forward().as_vec3(),
    }) else {
        return;
    };

    fx.eye = Some((view.pos, view.forward));
    fx.warm_up(&mut commands, &mut assets);
    let preloads = std::mem::take(&mut fx.preloads);
    if !preloads.is_empty() {
        let started = std::time::Instant::now();
        let count = preloads.len();
        for (name, layer) in preloads {
            fx.preload(&mut commands, &mut content, &mut assets, &name, layer);
        }
        debug!("fx: {count} effects preloaded in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
    }

    // Effects played since last frame, and runners' children now due
    // (starting runners queues their children: those due go this frame,
    // nested a few deep at most).
    for _ in 0..8 {
        let queue = std::mem::take(&mut fx.queue);
        let mut started = false;
        for play in queue {
            let at = play.at.unwrap_or(now);
            if at > now {
                fx.queue.push(play);
                continue;
            }
            let id = match play.effect {
                EffectRef::Id(id) => Some(id),
                EffectRef::Name(name) => fx.load(&mut content, &mut assets, &name),
            };
            let Some(id) = id else { continue };
            let (frame, bolt) = match play.anchor {
                Anchor::Fixed(f) => (f, None),
                Anchor::Bolted(e) => match globals.get(e) {
                    Ok(g) => (Frame::of(g), Some(e)),
                    Err(_) => continue,
                },
            };
            fx.start(id, frame, bolt, play.layer, at, play.forever);
            started = true;
        }
        if !started {
            break;
        }
    }

    // Bolted effects follow their entity; looping elements keep spawning.
    // Once the entity is gone (a rocket that went off) the effect stops
    // spawning, as IW3 stops an effect on a freed entity, and what it has
    // spawned lives out its life: else a rocket's trail kept pouring out
    // where it hit.
    for slot in 0..fx.instances.len() as u32 {
        let Some(inst) = fx.instances[slot as usize].as_mut() else { continue };
        if let Some(b) = inst.bolt {
            match globals.get(b) {
                Ok(g) => inst.frame = Frame::of(g),
                Err(_) => {
                    inst.bolt = None;
                    inst.looping.clear();
                }
            }
        }
        if !inst.looping.is_empty() {
            fx.spawn_looping(slot);
        }
    }

    // Particles: drawn, then dropped once their life is over.
    let mut quads: HashMap<(u16, FxLayer), Vec<Quad>> = HashMap::new();
    let mut trails: HashMap<(u32, u16), TrailRun> = HashMap::new();
    let mut wanted_lights = Vec::new();
    let mut ended = Vec::new();
    let Effects { defs, instances, particles, models: model_parts, model_count, queue, .. } = &mut *fx;
    let mut despawn = |p: &Particle, commands: &mut Commands| {
        if let Some(m) = &p.model {
            *model_count = model_count.saturating_sub(1);
            for &e in &m.entities {
                commands.entity(e).try_despawn();
            }
        }
    };
    particles.retain_mut(|p| {
        let Some(inst) = instances[p.inst as usize].as_ref() else {
            despawn(p, &mut commands);
            return false;
        };
        let age = (now - p.begin) as f32;
        if age < 0.0 {
            return true;
        }
        let loaded = &defs[inst.def.0 as usize];
        let e = &loaded.def.elems[p.elem as usize];
        if age >= p.life && (p.drawn || p.model.is_some()) {
            if let (Some(name), Some(at)) = (&e.effect_on_death, p.last) {
                queue.push(Play::child(name, Frame::facing(at, Vec3::Y, 0.0), inst.layer));
            }
            ended.push(p.inst);
            despawn(p, &mut commands);
            return false;
        }
        p.drawn = true;
        let t = (age / p.life).min(1.0);
        let frame = if p.follow { inst.frame } else { p.frame };
        let rv = std::array::from_fn(|i| rand(p.seed, key::VELOCITY + i as u32));
        let (local, world) = displacement(e, rv, t, p.life);
        let mut pos = frame.point(p.origin + local) + units::basis() * world * INCH;
        let gravity = e.gravity.at(rand(p.seed, key::GRAVITY));
        let age_s = age.min(p.life) / 1000.0;
        if e.flags & (flags::HAS_GRAVITY | flags::HAS_VELOCITY_GRAPH_WORLD) != 0 {
            pos.y -= u(0.5 * gravity * GRAVITY * age_s * age_s);
        }
        // Child effects: every so far along its path, and where it first
        // hits something (sprites that collide; models when they land).
        if let Some(last) = p.last {
            let step = pos - last;
            if let Some(name) = e.effect_emitted.as_ref().filter(|_| p.emit_every > 0.0) {
                p.travel += step.length() / INCH;
                while p.travel >= p.emit_every {
                    p.travel -= p.emit_every;
                    queue.push(Play::child(name, Frame::facing(pos, step, 0.0), inst.layer));
                }
            }
            if e.has(flags::USE_COLLISION) && p.model.is_none() && !p.impacted {
                let hit = Dir3::new(step).ok().and_then(|d| {
                    spatial.cast_ray(last, d, step.length(), true, &collision::sight_filter())
                });
                if let Some(hit) = hit {
                    p.impacted = true;
                    if let Some(name) = &e.effect_on_impact {
                        let at = last + step.normalize() * hit.distance;
                        queue.push(Play::child(name, Frame::facing(at, hit.normal, 0.0), inst.layer));
                    }
                    if e.has(flags::DIE_ON_TOUCH) {
                        p.life = age;
                    }
                }
            }
        }
        p.last = Some(pos);
        let mut look = look(e, p.seed, t, p.life);
        let dist = pos.distance(view.pos) / INCH;
        look.color[3] *= distance_fade(e, dist);
        if e.elem_type == ElemType::OmniLight {
            let c = Vec3::new(look.color[0], look.color[1], look.color[2]);
            wanted_lights.push(LightWant { pos, radius: look.size[0], color: c, layer: inst.layer, dist });
            return true;
        }
        let visual = loaded.visuals[p.elem as usize].get(p.visual as usize);
        if let (Some(state), Some(Vis::Model(model))) = (p.model.as_mut(), visual) {
            let mut rotation = model_rotation(e, p.seed, &frame, age.min(p.life));
            // Colliding ones stop on the floor under where they were thrown
            // (or vanish there, if they die on touching it).
            if e.has(flags::USE_COLLISION) {
                let floor = *state.floor.get_or_insert_with(|| {
                    let filter = collision::sight_filter();
                    spatial
                        .cast_ray(pos, Dir3::NEG_Y, u(256.0), true, &filter)
                        .map_or(f32::NEG_INFINITY, |h| pos.y - h.distance)
                });
                if let Some((at, r)) = state.rest {
                    (pos, rotation) = (at, r);
                } else if pos.y < floor + u(0.5) {
                    pos.y = floor + u(0.5);
                    if e.has(flags::DIE_ON_TOUCH) {
                        p.life = age;
                    }
                    state.rest = Some((pos, rotation));
                    // A case landing: the one that lies there (`*_resting`).
                    if let Some(name) = &e.effect_on_impact {
                        queue.push(Play::child(name, Frame::facing(pos, Vec3::Y, 0.0), inst.layer));
                    }
                }
            }
            let tf = Transform { translation: pos, rotation, scale: Vec3::splat(look.scale.max(0.0)) };
            if state.entities.is_empty() {
                for (mesh, material) in &model_parts[*model as usize] {
                    let e = commands
                        .spawn((
                            FxModel,
                            Name::new("fx model"),
                            Mesh3d(mesh.clone()),
                            MeshMaterial3d(material.clone()),
                            tf,
                            GlobalTransform::from(tf),
                            NotShadowCaster,
                            inst.layer.render_layers(),
                        ))
                        .id();
                    state.entities.push(e);
                }
            } else {
                for &e in &state.entities {
                    if let Ok((mut t, mut g)) = models.get_mut(e) {
                        *t = tf;
                        *g = GlobalTransform::from(tf);
                    }
                }
            }
            return true;
        }
        let Some(Vis::Sprite(material)) = visual else { return true };
        // A trail's clear points still end its ribbon, which fades to them.
        let clear = look.color[3] <= 0.0 && e.elem_type != ElemType::Trail;
        if clear || look.size[0] <= 0.0 && look.size[1] <= 0.0 {
            return true;
        }
        let rect = atlas_rect(e, p.seed, p.sequence, t, age.min(p.life));
        match e.elem_type {
            // A trail's points are swept into ribbons once all are placed.
            ElemType::Trail => {
                let run = trails.entry((p.inst, p.elem)).or_insert_with(|| TrailRun {
                    material: *material,
                    layer: inst.layer,
                    def: inst.def,
                    elem: p.elem,
                    points: Vec::new(),
                });
                run.points.push(TrailPoint { begin: p.begin, pos, size: look.size[0], color: look.color });
                return true;
            }
            // `CLOUD_POINTS` specks through a ball of radius `scale` (the
            // cloud's size is each speck's), scattered by its seed.
            ElemType::Cloud => {
                let radius = look.scale.max(0.0) * INCH;
                let list = quads.entry((*material, inst.layer)).or_default();
                for k in 0..CLOUD_POINTS {
                    let s = p.seed ^ (k + 1).wrapping_mul(0x85eb_ca6b);
                    let z = 2.0 * rand(s, 0) - 1.0;
                    let a = rand(s, 1) * std::f32::consts::TAU;
                    let ring = (1.0 - z * z).max(0.0).sqrt();
                    let center = pos + Vec3::new(ring * a.cos(), z, ring * a.sin()) * rand(s, 2).cbrt() * radius;
                    let spin = rand(s, 3) * std::f32::consts::TAU;
                    let (verts, uv) = sprite_quad(center, view.right, view.up, look.size, spin, rect);
                    list.push(Quad { verts, uv, colors: [look.color; 4], depth: (center - view.pos).dot(view.forward) });
                }
                return true;
            }
            _ => {}
        }
        let (tangent, binormal, center) = match e.elem_type {
            ElemType::OrientedSprite => (frame.axis.col(1), frame.axis.col(2), pos),
            ElemType::Tail => {
                // Stretched back along its velocity; none, no tail.
                let Some(dir) = velocity(e, rv, t, &frame, gravity, age_s).try_normalize() else { return true };
                let back = pos - dir * look.size[1] * INCH;
                let Some(tangent) = dir.cross(view.pos - back).try_normalize() else { return true };
                (tangent, dir, back)
            }
            _ => (view.right, view.up, pos),
        };
        let (verts, uv) = sprite_quad(center, tangent, binormal, look.size, look.rotation, rect);
        let depth = (center - view.pos).dot(view.forward);
        quads.entry((*material, inst.layer)).or_default().push(Quad { verts, uv, colors: [look.color; 4], depth });
        true
    });
    for slot in ended {
        if let Some(inst) = fx.instances[slot as usize].as_mut() {
            inst.live = inst.live.saturating_sub(1);
        }
        fx.release_if_done(slot);
    }
    for run in trails.into_values() {
        let e = &fx.defs[run.def.0 as usize].def.elems[run.elem as usize];
        if let Some(trail) = &e.trail {
            sweep_trail(trail, run, now, &view, &mut quads);
        }
    }

    draw_batches(&mut commands, fx, &mut assets, quads, &mut batches);
    place_lights(&mut commands, fx, wanted_lights, &mut lights);
    place_decals(&mut commands, fx, &mut content, &mut assets, &spatial);
    fx.busy.0 += started.elapsed();
    fx.busy.1 += 1;
}

impl Effects {
    /// Draw nothing on both layers from the start: Bevy compiles a
    /// material's pipelines (per camera) the first time something uses
    /// them, which takes longer than a muzzle flash lasts.
    fn warm_up(&mut self, commands: &mut Commands, assets: &mut FxAssets) {
        if self.warmed {
            return;
        }
        self.warmed = true;
        let material = assets.fx_materials.add(FxMaterial { params: Vec4::ZERO, texture: Handle::default() });
        let layers = std::iter::once(FxLayer::World).chain((0..crate::splitscreen::MAX_PLAYERS as u8).flat_map(|s| [FxLayer::ViewModel(s), FxLayer::Body(s)]));
        for layer in layers {
            commands.spawn((
                Name::new("fx warm-up"),
                Mesh3d(assets.meshes.add(empty_mesh())),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                NoFrustumCulling,
                NotShadowCaster,
                layer.render_layers(),
            ));
        }
    }
}

/// Build each batch's mesh from its quads, back to front, about their
/// centre (so the batch sorts against other transparent things there).
fn draw_batches(
    commands: &mut Commands,
    fx: &mut Effects,
    assets: &mut FxAssets,
    mut quads: HashMap<(u16, FxLayer), Vec<Quad>>,
    batches: &mut Query<(&mut Transform, &mut GlobalTransform), (With<FxBatch>, Without<FxLight>, Without<FxModel>)>,
) {
    for &(material, layer) in quads.keys() {
        fx.batch(commands, assets, material, layer);
    }
    for (key, batch) in fx.batches.iter_mut() {
        let mut list = quads.remove(key).unwrap_or_default();
        if list.is_empty() && batch.empty {
            continue;
        }
        batch.empty = list.is_empty();
        let mesh = if list.is_empty() {
            empty_mesh()
        } else {
            list.sort_by(|a, b| b.depth.total_cmp(&a.depth));
            let center = list.iter().map(|q| (q.verts[0] + q.verts[2]) * 0.5).sum::<Vec3>() / list.len() as f32;
            if let Ok((mut tf, mut g)) = batches.get_mut(batch.entity) {
                *tf = Transform::from_translation(center);
                *g = GlobalTransform::from_translation(center);
            }
            quads_mesh(&list, center)
        };
        let _ = assets.meshes.insert(batch.mesh.id(), mesh);
    }
}

impl Effects {
    /// The batch for a material on a layer, made if need be.
    fn batch(&mut self, commands: &mut Commands, assets: &mut FxAssets, material: u16, layer: FxLayer) {
        if self.batches.contains_key(&(material, layer)) {
            return;
        }
        let mesh = assets.meshes.add(empty_mesh());
        let entity = commands
            .spawn((
                FxBatch,
                Name::new("fx batch"),
                Mesh3d(mesh.clone()),
                MeshMaterial3d(self.materials[material as usize].clone()),
                Transform::default(),
                GlobalTransform::default(),
                Visibility::default(),
                NoFrustumCulling,
                NotShadowCaster,
                layer.render_layers(),
            ))
            .id();
        self.batches.insert((material, layer), Batch { entity, mesh, empty: true });
    }

    /// Load an effect ahead of its first use, with batches for its
    /// sprites' materials (and its runners' effects'), so it shows from
    /// the first frame it plays: a new material or batch would otherwise
    /// only be ready to draw a frame later, after a muzzle flash is over.
    fn preload(
        &mut self,
        commands: &mut Commands,
        content: &mut Content,
        assets: &mut FxAssets,
        name: &str,
        layer: FxLayer,
    ) {
        let Some(id) = self.load(content, assets, name) else { return };
        let mut todo = vec![id];
        let mut seen = vec![id];
        while let Some(id) = todo.pop() {
            let visuals: Vec<Vis> = self.defs[id.0 as usize].visuals.iter().flatten().cloned().collect();
            for v in visuals {
                match v {
                    Vis::Sprite(m) => self.batch(commands, assets, m, layer),
                    Vis::Effect(child) if !seen.contains(&child) => {
                        seen.push(child);
                        todo.push(child);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// A mesh with nothing to draw (one degenerate triangle).
fn empty_mesh() -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32; 2]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0f32; 4]; 3]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2]));
    mesh
}

fn quads_mesh(quads: &[Quad], center: Vec3) -> Mesh {
    let n = quads.len();
    let (mut pos, mut uv, mut color, mut idx) =
        (Vec::with_capacity(n * 4), Vec::with_capacity(n * 4), Vec::with_capacity(n * 4), Vec::with_capacity(n * 6));
    for (i, q) in quads.iter().enumerate() {
        let base = (i * 4) as u32;
        for k in 0..4 {
            pos.push((q.verts[k] - center).to_array());
            uv.push(q.uv[k].to_array());
            color.push(q.colors[k]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, color);
    mesh.insert_indices(Indices::U32(idx));
    mesh
}

/// Give the pool's point lights to the nearest omni light elements.
fn place_lights(
    commands: &mut Commands,
    fx: &mut Effects,
    mut wanted: Vec<LightWant>,
    lights: &mut Query<
        (&mut PointLight, &mut Transform, &mut GlobalTransform, &mut Visibility, &mut RenderLayers),
        (With<FxLight>, Without<FxBatch>, Without<FxModel>),
    >,
) {
    wanted.retain(|w| w.radius > 1.0);
    wanted.sort_by(|a, b| a.dist.total_cmp(&b.dist));
    wanted.truncate(MAX_LIGHTS);
    while fx.lights.len() < wanted.len() {
        let e = commands
            .spawn((
                FxLight,
                Name::new("fx light"),
                PointLight { shadow_maps_enabled: false, intensity: 0.0, ..default() },
                Transform::default(),
                GlobalTransform::default(),
                Visibility::Hidden,
                RenderLayers::layer(0),
            ))
            .id();
        fx.lights.push(e);
    }
    for (i, &e) in fx.lights.iter().enumerate() {
        let Ok((mut light, mut tf, mut g, mut vis, mut layers)) = lights.get_mut(e) else { continue };
        let Some(w) = wanted.get(i) else {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
            continue;
        };
        let range = u(w.radius);
        light.range = range;
        light.color = Color::linear_rgb(w.color.x, w.color.y, w.color.z);
        light.intensity = LIGHT_LUMENS_PER_M2 * range * range;
        *tf = Transform::from_translation(w.pos);
        *g = GlobalTransform::from_translation(w.pos);
        *vis = Visibility::Visible;
        // A first-person flash lights the gun and the world around it.
        let want = match w.layer {
            FxLayer::World | FxLayer::Body(_) => RenderLayers::layer(0),
            FxLayer::ViewModel(slot) => RenderLayers::from_layers(&[0, crate::splitscreen::viewmodel_layer(slot as usize)]),
        };
        if *layers != want {
            *layers = want;
        }
    }
}

/// Lay new impact marks on their surfaces and clear old ones.
/// Marks go onto the world (`R_MarkFragments`): CoD4 clips them to the
/// surfaces within their radius either way along the effect's forward, so a
/// mark is laid flat on the nearest surface there (behind an impact; behind
/// someone shot, a wall within reach of the exit wound), or not at all.
fn place_decals(commands: &mut Commands, fx: &mut Effects, content: &mut Content, assets: &mut FxAssets, spatial: &SpatialQuery) {
    for d in std::mem::take(&mut fx.new_decals) {
        let forward = d.frame.axis.col(0).normalize_or(Vec3::Y);
        let reach = u(d.size.max(4.0));
        let filter = SpatialQueryFilter::from_mask(collision::Layer::World);
        let cast = |dir: Vec3, reach: f32| {
            let d3 = Dir3::new(dir).ok()?;
            // From just in front, so a mark on the surface it starts on
            // finds it; a ray starting inside something (the other way into
            // that surface) finds nothing.
            spatial.cast_ray(d.frame.origin - dir * u(1.0), d3, reach + u(1.0), true, &filter).filter(|h| h.distance > 0.0).map(|h| (h, dir))
        };
        let surface = [-forward, forward]
            .into_iter()
            .filter_map(|dir| cast(dir, reach))
            .min_by(|a, b| a.0.distance.total_cmp(&b.0.distance))
            // Nothing that way: the floor below (blood under someone shot).
            .or_else(|| cast(Vec3::NEG_Y, u(DECAL_FLOOR_REACH)));
        let Some((hit, dir)) = surface else {
            debug!("fx: mark {} found no surface", d.material);
            continue;
        };
        let start = d.frame.origin - dir * u(1.0);
        let at = start + dir * hit.distance;
        // Facing the side the ray came from.
        let n = if hit.normal.dot(start - at) < 0.0 { -hit.normal } else { hit.normal };
        let Some(material) = fx.decal_material(content, assets, &d.material, d.color) else {
            debug!("fx: mark material {} not found", d.material);
            continue;
        };
        debug!(
            "fx: mark {} at {at} size {} colour {:?} {:?}",
            d.material,
            d.size,
            d.color,
            assets.materials.get(&material).map(|m| (m.alpha_mode, m.unlit, m.depth_bias, m.base_color_texture.is_some()))
        );
        let mesh = fx.decal_mesh.get_or_insert_with(|| assets.meshes.add(Rectangle::new(1.0, 1.0))).clone();
        // The quad's +Z faces out of the surface, spun as the effect says.
        let (sin, cos) = d.rotation.sin_cos();
        let t = d.frame.axis.col(1).reject_from_normalized(n).normalize_or(n.any_orthonormal_vector());
        let b = n.cross(t);
        let x = t * cos + b * sin;
        let rotation = Quat::from_mat3(&Mat3::from_cols(x, n.cross(x), n));
        let tf = Transform { translation: at + n * u(0.1), rotation, scale: Vec3::splat(u(d.size * 2.0)) };
        let e = commands
            .spawn((Name::new("impact mark"), Mesh3d(mesh), MeshMaterial3d(material), tf, NotShadowCaster))
            .id();
        fx.decals.push_back(e);
        if fx.decals.len() > MAX_DECALS {
            if let Some(old) = fx.decals.pop_front() {
                commands.entity(old).try_despawn();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Guns and bullets

impl Effects {
    /// The impact table (`FxImpactTable`), from whichever zone has one.
    fn impact_table(&mut self, content: &Content) -> Option<&ImpactTable> {
        self.impacts
            .get_or_insert_with(|| {
                content.zones.iter().find_map(|z| {
                    z.assets.iter().find_map(|a| match a {
                        iw3::zone::Asset::Generic(g) if g.ty == AssetType::ImpactFx => {
                            Some(ImpactTable::from_node(z, &g.root))
                        }
                        _ => None,
                    })
                })
            })
            .as_ref()
    }

    /// A weapon's flash effects. Guns put together from attachments
    /// (`ak47_reflex_silencer_mp`) take their base gun's, or its silenced
    /// variant's; guns from other games a CoD4 gun's of their class.
    fn weapon(&mut self, content: &Content, def: &WeaponDef) -> WeaponFx {
        if let Some(w) = self.weapons.get(&def.name) {
            return w.clone();
        }
        let base = def.name.split('_').next().unwrap_or("");
        let by_class = match def.class {
            1 => "rpd_mp",
            2 => "mp5_mp",
            3 => "winchester1200_mp",
            4 => "colt45_mp",
            _ => "ak47_mp",
        };
        let mut candidates = vec![def.name.clone()];
        if def.name.contains("_silencer") {
            candidates.push(format!("{base}_silencer_mp"));
        }
        candidates.push(format!("{base}_mp"));
        candidates.push(by_class.to_owned());
        let found = candidates.iter().find_map(|n| content.generic(AssetType::Weapon, n)).map(|(zi, node)| {
            let zone = &content.zones[zi];
            let fx = |f: &str| node.asset(f).map(|i| zone.get(i).name().trim_start_matches(',').to_owned());
            WeaponFx {
                view_flash: fx("viewFlashEffect"),
                world_flash: fx("worldFlashEffect"),
                view_shell: fx("viewShellEjectEffect"),
                world_shell: fx("worldShellEjectEffect"),
                preloaded_view: false,
            }
        });
        let w = found.unwrap_or_default();
        self.weapons.insert(def.name.clone(), w.clone());
        w
    }
}

impl Effects {
    /// Queue a weapon's flashes, cases and its bullets' impact effects to
    /// load.
    fn preload_weapon(&mut self, content: &Content, def: &WeaponDef, local: bool) {
        let w = self.weapon(content, def);
        for name in [&w.world_flash, &w.world_shell].into_iter().flatten() {
            self.preloads.push((name.clone(), FxLayer::World));
        }
        if local {
            for name in [&w.view_flash, &w.view_shell].into_iter().flatten() {
                for slot in 0..crate::splitscreen::count() as u8 {
                    self.preloads.push((name.clone(), FxLayer::ViewModel(slot)));
                }
            }
            if let Some(w) = self.weapons.get_mut(&def.name) {
                w.preloaded_view = true;
            }
        }
        if self.preloaded_impacts.contains(&def.impact_type) {
            return;
        }
        self.preloaded_impacts.push(def.impact_type);
        let rows = [false, true].map(|exit| ImpactTable::row(def.impact_type, exit));
        let Some(table) = self.impact_table(content) else { return };
        let mut names: Vec<String> = Vec::new();
        for row in rows.into_iter().flatten() {
            if let Some(entry) = table.entries.get(row) {
                names.extend(entry.nonflesh.iter().chain(&entry.flesh).flatten().cloned());
            }
        }
        names.sort();
        names.dedup();
        self.preloads.extend(names.into_iter().map(|n| (n, FxLayer::World)));
    }
}

/// The surface type a bullet hit: a short ray onto the hit point finds the
/// brush face. Terrain has no surface type here (`None`).
fn hit_surface(spatial: &SpatialQuery, surfaces: &Query<&Surfaces>, s: &ShotFired) -> Option<usize> {
    surface_at(spatial, surfaces, s.from, s.to)
}

/// The surface type (an index into [`SURFACE_NAMES`]) a bullet from `from`
/// hit at `to`: the brush face under a short ray onto the point.
fn surface_at(spatial: &SpatialQuery, surfaces: &Query<&Surfaces>, from: Vec3, to: Vec3) -> Option<usize> {
    let dir = (to - from).try_normalize()?;
    let hit = spatial.cast_ray(to - dir * u(4.0), Dir3::new(dir).ok()?, u(8.0), true, &collision::sight_filter())?;
    // Brushes know their faces' surfaces; terrain and patches, by place.
    let name = match surfaces.get(hit.entity) {
        Ok(sf) => sf.facing(hit.normal),
        Err(_) => crate::terrain::surface_at(to)?,
    };
    SURFACE_NAMES.iter().position(|&n| n == name)
}

/// What a bullet hit: the world, a pawn, or something else with a hitbox
/// (a helicopter), which is metal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulletHit {
    World,
    Flesh { head: bool },
    Metal,
}

/// A bullet from something without a gun of its own (a helicopter's
/// turret): where it landed, for its impact effect and sound.
#[derive(Message, Clone, Copy, Debug)]
pub struct BulletImpact {
    pub from: Vec3,
    pub to: Vec3,
    pub normal: Vec3,
    /// The weapon's `impactType`.
    pub impact_type: i32,
    pub hit: BulletHit,
}

/// [`BulletImpact`]s' effects, from the impact table as for a gun's.
fn bullet_impacts(
    fx: Option<ResMut<Effects>>,
    content: Option<Res<Content>>,
    mut impacts: MessageReader<BulletImpact>,
    spatial: SpatialQuery,
    surfaces: Query<&Surfaces>,
) {
    let (Some(mut fx), Some(content)) = (fx, content) else {
        impacts.clear();
        return;
    };
    for b in impacts.read() {
        let Some(table) = fx.impact_table(&content) else { return };
        let Some(row) = ImpactTable::row(b.impact_type, false) else { continue };
        let name = match b.hit {
            BulletHit::World => {
                let surface = surface_at(&spatial, &surfaces, b.from, b.to);
                surface.and_then(|i| table.surface(row, i)).or_else(|| table.surface(row, if surface.is_none() { 6 } else { 5 }))
            }
            BulletHit::Metal => table.surface(row, metal()),
            BulletHit::Flesh { head } => table.flesh(row, if head { FleshImpact::Head } else { FleshImpact::Body }),
        };
        let normal = b.normal.try_normalize().unwrap_or(Vec3::Y);
        if let Some(name) = name.map(str::to_owned) {
            fx.play(&name, Anchor::Fixed(Frame::facing(b.to, normal, roll())), FxLayer::World);
        }
    }
}

/// [`SURFACE_NAMES`]' metal.
fn metal() -> usize {
    SURFACE_NAMES.iter().position(|&n| n == "metal").unwrap_or(0)
}

/// Muzzle flashes and spent cases for every shot, and where bullets land,
/// impacts and blood (`CG_BulletHitEvent`): effects from the impact table by
/// the weapon's impact type and the surface, facing out of it.
#[allow(clippy::too_many_arguments)]
fn weapon_effects(
    fx: Option<ResMut<Effects>>,
    content: Option<Res<Content>>,
    mut shots: MessageReader<ShotFired>,
    mut killed: MessageReader<Killed>,
    shooters: Query<(&WeaponState, Option<&Body>, Option<&crate::splitscreen::LocalSlot>)>,
    skeletons: Query<&Skeleton>,
    viewmodel: Query<(&Skeleton, &InheritedVisibility, &crate::viewmodel::ViewModelSlot), With<ViewModelRoot>>,
    third_person: Option<Res<crate::wardrobe::ThirdPerson>>,
    spatial: SpatialQuery,
    surfaces: Query<&Surfaces>,
    hitboxes: Query<&Hitbox>,
    teams: Query<&crate::combat::Pawn>,
) {
    let (Some(mut fx), Some(content)) = (fx, content) else {
        shots.clear();
        killed.clear();
        return;
    };
    let now = fx.now;
    // Every gun in play has its effects ready before it first fires.
    for (w, _, local) in &shooters {
        let local = local.is_some();
        if !fx.weapons.contains_key(&w.def.name) || local && !fx.weapons[&w.def.name].preloaded_view {
            fx.preload_weapon(&content, w.def, local);
        }
    }
    for s in shots.read() {
        let Ok((w, body, local)) = shooters.get(s.shooter) else { continue };
        let wfx = fx.weapon(&content, w.def);

        // The flash and the spent case, bolted to the gun's `tag_flash` and
        // `tag_brass` (`WeaponFlash`, `CG_EjectWeaponBrass`): the
        // first-person gun's own in first person, else the world model's.
        let slot = local.map(|s| s.0);
        let first_person = slot.is_some_and(|s| !third_person.as_ref().is_some_and(|t| t.on(s)));
        let world_gun = || body.and_then(|b| skeletons.get(b.0).ok());
        let (gun, layer, flash, shell) = if first_person {
            let slot = slot.unwrap_or(0);
            let gun = viewmodel.iter().find(|(_, v, s)| v.get() && s.0 == slot).map(|(sk, ..)| sk);
            (gun, FxLayer::ViewModel(slot as u8), &wfx.view_flash, &wfx.view_shell)
        } else {
            (world_gun(), FxLayer::World, &wfx.world_flash, &wfx.world_shell)
        };
        for (tag, name) in [("tag_flash", flash), ("tag_brass", shell)] {
            if let (Some(tag), Some(name)) = (gun.and_then(|sk| sk.joint(tag)), name) {
                fx.play(name, Anchor::Bolted(tag), layer);
            }
        }
        // Splitscreen: the other players see a player's third-person gun
        // flash, as they see any other.
        if let (true, Some(slot), Some(flash)) = (first_person && crate::splitscreen::active(), slot, &wfx.world_flash) {
            if let Some(tag) = world_gun().and_then(|sk| sk.joint("tag_flash")) {
                fx.play(flash, Anchor::Bolted(tag), FxLayer::Body(slot as u8));
            }
        }

        let dir = (s.to - s.from).try_normalize();
        let normal = s.normal.try_normalize().or(dir.map(|d| -d)).unwrap_or(Vec3::Y);
        let Some(table) = fx.impact_table(&content) else { continue };
        let Some(row) = ImpactTable::row(w.def.impact_type, false) else { continue };
        if s.hit_world {
            // Terrain is mostly dirt; unlisted brush surfaces look like concrete.
            let surface = hit_surface(&spatial, &surfaces, s);
            let name = surface
                .and_then(|i| table.surface(row, i))
                .or_else(|| table.surface(row, if surface.is_none() { 6 } else { 5 }))
                .map(str::to_owned);
            if let Some(name) = name {
                fx.play(&name, Anchor::Fixed(Frame::facing(s.to, normal, roll())), FxLayer::World);
            }
        } else if s.hit_pawn {
            // Which body part: the hitbox along the shot.
            let hit = dir.and_then(|d| {
                let filter = SpatialQueryFilter::from_mask(collision::Layer::Hitbox);
                let own = |e: Entity| hitboxes.get(e).is_ok_and(|h| h.owner != s.shooter);
                let h = spatial.cast_ray_predicate(
                    s.from,
                    Dir3::new(d).ok()?,
                    s.from.distance(s.to) + u(4.0),
                    true,
                    &filter,
                    &own,
                )?;
                hitboxes.get(h.entity).ok().copied()
            });
            // A hitbox that isn't a pawn's (a helicopter's) is metal.
            if hit.is_some_and(|h| teams.get(h.owner).is_err()) {
                if let Some(name) = table.surface(row, metal()).map(str::to_owned) {
                    fx.play(&name, Anchor::Fixed(Frame::facing(s.to, normal, roll())), FxLayer::World);
                }
                continue;
            }
            // Not on the player's own body seen from inside it.
            let own_view = |e: Entity| {
                shooters.get(e).ok().and_then(|(_, _, local)| local).is_some_and(|s| !third_person.as_ref().is_some_and(|t| t.on(s.0)))
                    && !crate::splitscreen::active()
            };
            if hit.is_some_and(|h| own_view(h.owner)) {
                continue;
            }
            // Friendly fire is off: no blood on teammates.
            let team = |e: Entity| teams.get(e).ok().map(|p| p.team);
            if hit.is_some_and(|h| team(h.owner).is_some() && team(h.owner) == team(s.shooter)) {
                continue;
            }
            let head = hit.is_some_and(|h| h.location == HitLocation::Head);
            let kind = if head { FleshImpact::Head } else { FleshImpact::Body };
            if let Some(name) = table.flesh(row, kind).map(str::to_owned) {
                fx.play(&name, Anchor::Fixed(Frame::facing(s.to, normal, roll())), FxLayer::World);
            }
            if let (Some(h), Some(d)) = (hit, dir) {
                let impact_type = w.def.impact_type;
                fx.fatal.push(PendingFatal {
                    victim: h.owner,
                    attacker: s.shooter,
                    point: s.to,
                    dir: d,
                    head,
                    impact_type,
                    until: now + 300.0,
                });
            }
        }
    }

    // A killing shot also comes out the other side (the exit row's fatal
    // wound), as bullets go through players.
    for k in killed.read() {
        let Some(attacker) = k.attacker else { continue };
        let Some(i) = fx.fatal.iter().rposition(|p| p.victim == k.victim && p.attacker == attacker) else { continue };
        let p = fx.fatal.swap_remove(i);
        let kind = if p.head { FleshImpact::HeadFatal } else { FleshImpact::BodyFatal };
        let name = ImpactTable::row(p.impact_type, true)
            .and_then(|row| fx.impact_table(&content)?.flesh(row, kind))
            .map(str::to_owned);
        let exit = p.point + p.dir * u(8.0);
        if let Some(name) = name {
            fx.play(&name, Anchor::Fixed(Frame::facing(exit, p.dir, roll())), FxLayer::World);
        }
        // Beyond CoD4's multiplayer (its blood splat effect goes unused):
        // blood on the wall behind, or the floor.
        fx.play(BLOOD_SPLAT, Anchor::Fixed(Frame::facing(exit, p.dir, roll())), FxLayer::World);
    }
    fx.fatal.retain(|p| p.until > now);
}

/// A random roll about an impact's normal (`CG_RandomEffectAxis`).
fn roll() -> f32 {
    use rand::Rng;
    rand::rng().random_range(0.0..std::f32::consts::TAU)
}
