//! Effects: `FxEffectDef` (muzzle flashes, impacts, explosions, ...) and the
//! `FxImpactTable` that picks a bullet impact effect by surface.
//!
//! An effect is a list of element definitions. Each element spawns one or
//! more particles (sprites, tails, models, lights, sounds, decals, or other
//! effects); a particle's motion and look over its life come from sampled
//! graphs, each sample a `base` and an `amplitude` that a per-particle random
//! number blends between. These types keep the graphs as stored and say how
//! the engine reads them; [`crate::zone::generic`] loads the raw assets.
//!
//! Semantics (units, the meaning of each graph) follow the engine's effects
//! code as reconstructed by the KisakCOD project (`EffectsCore/fx_update.cpp`,
//! `fx_draw.cpp`); layouts follow OpenAssetTools' IW3 definitions.

use crate::zone::generic::GNode;
use crate::zone::{AssetType, Zone};

/// `FxElemDef::elemType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElemType {
    /// A quad facing the camera.
    BillboardSprite,
    /// A quad facing along the element's frame (its run orientation).
    OrientedSprite,
    /// A quad stretched back along the velocity, turned to face the camera.
    Tail,
    /// A ribbon of segments (`FxTrailDef`).
    Trail,
    /// A particle cloud (many points drawn by one shader).
    Cloud,
    Model,
    OmniLight,
    SpotLight,
    Sound,
    /// A mark projected onto the surface hit.
    Decal,
    /// Spawns another effect.
    Runner,
    Unknown(u8),
}

impl ElemType {
    fn from_u8(v: u8) -> ElemType {
        use ElemType::*;
        match v {
            0 => BillboardSprite,
            1 => OrientedSprite,
            2 => Tail,
            3 => Trail,
            4 => Cloud,
            5 => Model,
            6 => OmniLight,
            7 => SpotLight,
            8 => Sound,
            9 => Decal,
            10 => Runner,
            o => Unknown(o),
        }
    }

    /// Drawn with a material as camera-facing or oriented quads.
    pub fn is_sprite(self) -> bool {
        matches!(self, ElemType::BillboardSprite | ElemType::OrientedSprite | ElemType::Tail)
    }
}

/// `FxElemDef::flags` bits.
pub mod flags {
    /// `spawnOrigin` is in the effect's axes (else world axes).
    pub const SPAWN_RELATIVE_TO_EFFECT: u32 = 0x2;
    /// Runners pick a random rotation for the effect they spawn.
    pub const RUNNER_USES_RAND_ROT: u32 = 0x8;
    pub const SPAWN_OFFSET_MASK: u32 = 0x30;
    pub const SPAWN_OFFSET_SPHERE: u32 = 0x10;
    pub const SPAWN_OFFSET_CYLINDER: u32 = 0x20;
    /// Which frame the particle moves in: the world, the effect where it
    /// spawned, the effect as it moves now (bolted to a gun), or the spawn
    /// offset direction.
    pub const RUN_MASK: u32 = 0xC0;
    pub const RUN_RELATIVE_TO_WORLD: u32 = 0x0;
    pub const RUN_RELATIVE_TO_SPAWN: u32 = 0x40;
    pub const RUN_RELATIVE_TO_EFFECT: u32 = 0x80;
    pub const RUN_RELATIVE_TO_OFFSET: u32 = 0xC0;
    pub const USE_COLLISION: u32 = 0x100;
    pub const DIE_ON_TOUCH: u32 = 0x200;
    pub const DRAW_PAST_FOG: u32 = 0x400;
    pub const DRAW_WITH_VIEWMODEL: u32 = 0x800;
    pub const BLOCK_SIGHT: u32 = 0x1000;
    pub const HAS_VELOCITY_GRAPH_LOCAL: u32 = 0x100_0000;
    pub const HAS_VELOCITY_GRAPH_WORLD: u32 = 0x200_0000;
    pub const HAS_GRAVITY: u32 = 0x400_0000;
    pub const USE_MODEL_PHYSICS: u32 = 0x800_0000;
    /// Width and height have their own graphs (else height = width).
    pub const NONUNIFORM_SCALE: u32 = 0x1000_0000;
}

/// `FxElemAtlas::behavior` bits.
pub mod atlas {
    pub const START_MASK: u8 = 0x3;
    pub const START_FIXED: u8 = 0x0;
    pub const START_RANDOM: u8 = 0x1;
    /// Successive particles of one spawn take successive frames.
    pub const START_INDEXED: u8 = 0x2;
    /// Run through every frame once over the particle's life.
    pub const PLAY_OVER_LIFE: u8 = 0x4;
    pub const LOOP_ONLY_N_TIMES: u8 = 0x8;
}

/// `base + amplitude * r` for a random `r` in [0, 1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FloatRange {
    pub base: f32,
    pub amplitude: f32,
}

impl FloatRange {
    pub fn at(&self, r: f32) -> f32 {
        self.base + self.amplitude * r
    }
}

/// `base + floor((amplitude + 1) * r)`: integers from `base` to `base + amplitude`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct IntRange {
    pub base: i32,
    pub amplitude: i32,
}

impl IntRange {
    pub fn at(&self, r: f32) -> i32 {
        if self.amplitude == 0 { self.base } else { self.base + ((self.amplitude + 1) as f32 * r) as i32 }
    }
}

/// A per-axis [`FloatRange`], each axis with its own random number.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec3Range {
    pub base: [f32; 3],
    pub amplitude: [f32; 3],
}

impl Vec3Range {
    pub fn at(&self, r: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| self.base[i] + self.amplitude[i] * r[i])
    }
}

/// How many particles an element spawns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spawn {
    /// All at once when the effect plays.
    OneShot { count: IntRange },
    /// One every `interval_msec`, `count` in all (`i32::MAX`: until the
    /// effect is stopped).
    Looping { interval_msec: i32, count: i32 },
}

/// One velocity sample, in CoD units per millisecond per velocity interval:
/// the velocity at sample `k` is `velocity * interval_count` units/ms. Local
/// velocities are in the element's run frame, world ones in world axes.
/// `total` is the displacement up to the sample, in units per millisecond of
/// life (multiply by the life span).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VelSample {
    pub local_velocity: Vec3Range,
    pub local_total: Vec3Range,
    pub world_velocity: Vec3Range,
    pub world_total: Vec3Range,
}

/// The look of a particle at one sample. Colours are RGBA bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VisualState {
    pub color: [u8; 4],
    /// Spin rate in radians per millisecond of life (times the life span).
    pub rotation_delta: f32,
    /// Spin so far at this sample, in the same units.
    pub rotation_total: f32,
    /// Half width and half height in CoD units (a light's radius is
    /// `size[0]`; a tail's length is `size[1]`).
    pub size: [f32; 2],
    /// Model and cloud scale.
    pub scale: f32,
}

/// A visual state sample: colours are blended between `base` and
/// `amplitude` by the particle's random number; every other field is
/// `base + amplitude * r`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VisSample {
    pub base: VisualState,
    pub amplitude: VisualState,
}

/// A texture atlas (flipbook) on a sprite's material.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Atlas {
    pub behavior: u8,
    /// The first frame for [`atlas::START_FIXED`].
    pub index: u8,
    /// Frames per second, unless [`atlas::PLAY_OVER_LIFE`].
    pub fps: u8,
    pub loop_count: u8,
    /// log2 of the columns and rows.
    pub col_index_bits: u8,
    pub row_index_bits: u8,
    /// Frames in all (a power of two; 1 for no atlas).
    pub entry_count: u16,
}

/// What an element draws or does. Materials and models are named as their
/// zone names them (a `,name` is defined in another zone).
#[derive(Debug, Clone, PartialEq)]
pub enum Visual {
    Material(String),
    Model(String),
    Effect(String),
    Sound(String),
    /// A decal's materials: the mark, and (optionally) its displacement.
    Decal([Option<String>; 2]),
}

/// `FxTrailDef`: a trail's cross-section, swept along the points its
/// element lays down. Each pair of indices is one strip across it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrailDef {
    /// The texture scrolls along the trail once every this long (ms; 0: no
    /// scrolling, negative: backwards).
    pub scroll_time_msec: i32,
    /// The texture repeats every this many units along the trail.
    pub repeat_dist: f32,
    /// A new point is laid once the effect has moved this far (units).
    pub split_dist: f32,
    pub verts: Vec<TrailVertex>,
    pub inds: Vec<u16>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrailVertex {
    /// Across the trail, in units of the particle's size: (right, up).
    pub pos: [f32; 2],
    pub normal: [f32; 2],
    /// The texture's u across the strip.
    pub tex_coord: f32,
}

#[derive(Debug, Clone)]
pub struct FxElemDef {
    pub flags: u32,
    pub elem_type: ElemType,
    pub spawn: Spawn,
    /// Distance fades (`spawnRange` culls spawning; the others fade in
    /// between `base` and `base + amplitude` units from the camera).
    pub spawn_range: FloatRange,
    /// With flag 4: not spawned while a sphere this big (units) around the
    /// effect is out of view.
    pub spawn_frustum_cull_radius: f32,
    pub fade_in_range: FloatRange,
    pub fade_out_range: FloatRange,
    pub spawn_delay_msec: IntRange,
    pub life_span_msec: IntRange,
    /// Spawn position offset (x forward, y left, z up), in the effect's axes
    /// with [`flags::SPAWN_RELATIVE_TO_EFFECT`], else in world axes.
    pub spawn_origin: [FloatRange; 3],
    pub spawn_offset_radius: FloatRange,
    pub spawn_offset_height: FloatRange,
    /// Model orientation (pitch, yaw, roll) in radians, and its rate of
    /// change in radians per millisecond.
    pub spawn_angles: [FloatRange; 3],
    pub angular_velocity: [FloatRange; 3],
    /// Starting sprite rotation in radians.
    pub initial_rotation: FloatRange,
    /// Fraction of world gravity (800 units/s²).
    pub gravity: FloatRange,
    pub reflection_factor: FloatRange,
    pub atlas: Atlas,
    /// `interval_count + 1` samples spread evenly over the particle's life.
    pub vel_samples: Vec<VelSample>,
    pub vis_samples: Vec<VisSample>,
    /// One is picked at random per particle.
    pub visuals: Vec<Visual>,
    pub coll_mins: [f32; 3],
    pub coll_maxs: [f32; 3],
    pub effect_on_impact: Option<String>,
    pub effect_on_death: Option<String>,
    pub effect_emitted: Option<String>,
    pub emit_dist: FloatRange,
    pub emit_dist_variance: FloatRange,
    pub sort_order: u8,
    /// How much the world's lighting tints the particle (0-255).
    pub lighting_frac: u8,
    /// Trails' cross-section.
    pub trail: Option<TrailDef>,
}

impl FxElemDef {
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    pub fn run_frame(&self) -> u32 {
        self.flags & flags::RUN_MASK
    }
}

#[derive(Debug, Clone)]
pub struct FxEffectDef {
    pub name: String,
    pub flags: u32,
    /// How long looping elements keep spawning (0: until stopped).
    pub msec_looping_life: i32,
    /// Looping elements, then one-shot ones, then emitted ones (spawned along
    /// other particles' paths by `effect_emitted`).
    pub elems: Vec<FxElemDef>,
    pub looping_count: usize,
    pub one_shot_count: usize,
    pub emission_count: usize,
}

impl FxEffectDef {
    /// Read a loaded `FxEffectDef` (asset references resolve in `zone`).
    pub fn from_node(zone: &Zone, node: &GNode) -> FxEffectDef {
        let count = |f: &str| node.int(f).max(0) as usize;
        let mut def = FxEffectDef {
            name: node.string("name").unwrap_or_default().to_owned(),
            flags: node.int("flags") as u32,
            msec_looping_life: node.int("msecLoopingLife") as i32,
            elems: node.nodes("elemDefs").iter().map(|e| elem_def(zone, e)).collect(),
            looping_count: count("elemDefCountLooping"),
            one_shot_count: count("elemDefCountOneShot"),
            emission_count: count("elemDefCountEmission"),
        };
        // `FxSpawnDef` is a union: one-shot elements keep their count range
        // where looping ones keep (interval, count).
        for i in def.one_shot() {
            if let Spawn::Looping { interval_msec, count } = def.elems[i].spawn {
                def.elems[i].spawn = Spawn::OneShot { count: IntRange { base: interval_msec, amplitude: count } };
            }
        }
        def
    }

    /// The looping elements (indices into [`Self::elems`]).
    pub fn looping(&self) -> std::ops::Range<usize> {
        0..self.looping_count.min(self.elems.len())
    }

    pub fn one_shot(&self) -> std::ops::Range<usize> {
        let start = self.looping().end;
        start..(start + self.one_shot_count).min(self.elems.len())
    }

    pub fn emission(&self) -> std::ops::Range<usize> {
        let start = self.one_shot().end;
        start..(start + self.emission_count).min(self.elems.len())
    }

    /// Find and read an effect by name in `zone`.
    pub fn find(zone: &Zone, name: &str) -> Option<FxEffectDef> {
        zone.assets.iter().find_map(|a| match a {
            crate::zone::Asset::Generic(g) if g.ty == AssetType::Fx && g.name == name => {
                Some(FxEffectDef::from_node(zone, &g.root))
            }
            _ => None,
        })
    }
}

fn float_range(n: &GNode, path: &str) -> FloatRange {
    FloatRange { base: n.float(&format!("{path}::base")), amplitude: n.float(&format!("{path}::amplitude")) }
}

fn int_range(n: &GNode, path: &str) -> IntRange {
    IntRange { base: n.int(&format!("{path}::base")) as i32, amplitude: n.int(&format!("{path}::amplitude")) as i32 }
}

fn vec3_range(n: &GNode, path: &str) -> Vec3Range {
    Vec3Range {
        base: std::array::from_fn(|i| n.float(&format!("{path}::base[{i}]"))),
        amplitude: std::array::from_fn(|i| n.float(&format!("{path}::amplitude[{i}]"))),
    }
}

/// Unsigned byte fields are declared `char`.
fn byte(n: &GNode, path: &str) -> u8 {
    (n.int(path) & 0xff) as u8
}

fn visual_state(n: &GNode, path: &str) -> VisualState {
    // Stored the way the engine packs it into a D3DCOLOR vertex: BGRA.
    let [b, g, r, a] = std::array::from_fn(|i| byte(n, &format!("{path}::color[{i}]")));
    VisualState {
        color: [r, g, b, a],
        rotation_delta: n.float(&format!("{path}::rotationDelta")),
        rotation_total: n.float(&format!("{path}::rotationTotal")),
        size: [n.float(&format!("{path}::size[0]")), n.float(&format!("{path}::size[1]"))],
        scale: n.float(&format!("{path}::scale")),
    }
}

/// An `FxEffectDefRef` (effects reference each other by name).
fn effect_ref(n: &GNode, field: &str) -> Option<String> {
    n.node(field).and_then(|r| r.string("name")).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn elem_def(zone: &Zone, e: &GNode) -> FxElemDef {
    let elem_type = ElemType::from_u8(byte(e, "elemType"));
    let asset_name = |id: Option<usize>| id.map(|i| zone.get(i).name().to_owned());
    // One `FxElemVisuals` per visual, read by the element type.
    let visual = |v: &GNode| -> Option<Visual> {
        match elem_type {
            ElemType::Model => asset_name(v.asset("model")).map(Visual::Model),
            ElemType::Runner => effect_ref(v, "effectDef").map(Visual::Effect),
            ElemType::Sound => v.string("soundName").map(|s| Visual::Sound(s.to_owned())),
            ElemType::Decal => None,
            _ => asset_name(v.asset("material")).map(Visual::Material),
        }
    };
    let visuals = match e.node("visuals") {
        Some(vis) if elem_type == ElemType::Decal => vis
            .nodes("markArray")
            .iter()
            .map(|m| {
                let mats = m.assets("materials");
                Visual::Decal([asset_name(mats.first().copied().flatten()), asset_name(mats.get(1).copied().flatten())])
            })
            .collect(),
        Some(vis) => {
            let list = vis.nodes("array");
            if list.is_empty() {
                vis.nodes("instance").iter().filter_map(visual).collect()
            } else {
                list.iter().filter_map(visual).collect()
            }
        }
        None => Vec::new(),
    };
    // Read as looping; [`FxEffectDef::from_node`] reinterprets one-shot ones.
    let spawn_raw = (e.int("spawn::looping::intervalMsec") as i32, e.int("spawn::looping::count") as i32);
    FxElemDef {
        flags: e.int("flags") as u32,
        elem_type,
        spawn: Spawn::Looping { interval_msec: spawn_raw.0, count: spawn_raw.1 },
        spawn_range: float_range(e, "spawnRange"),
        spawn_frustum_cull_radius: e.float("spawnFrustumCullRadius"),
        fade_in_range: float_range(e, "fadeInRange"),
        fade_out_range: float_range(e, "fadeOutRange"),
        spawn_delay_msec: int_range(e, "spawnDelayMsec"),
        life_span_msec: int_range(e, "lifeSpanMsec"),
        spawn_origin: std::array::from_fn(|i| float_range(e, &format!("spawnOrigin[{i}]"))),
        spawn_offset_radius: float_range(e, "spawnOffsetRadius"),
        spawn_offset_height: float_range(e, "spawnOffsetHeight"),
        spawn_angles: std::array::from_fn(|i| float_range(e, &format!("spawnAngles[{i}]"))),
        angular_velocity: std::array::from_fn(|i| float_range(e, &format!("angularVelocity[{i}]"))),
        initial_rotation: float_range(e, "initialRotation"),
        gravity: float_range(e, "gravity"),
        reflection_factor: float_range(e, "reflectionFactor"),
        atlas: Atlas {
            behavior: byte(e, "atlas::behavior"),
            index: byte(e, "atlas::index"),
            fps: byte(e, "atlas::fps"),
            loop_count: byte(e, "atlas::loopCount"),
            col_index_bits: byte(e, "atlas::colIndexBits"),
            row_index_bits: byte(e, "atlas::rowIndexBits"),
            entry_count: e.int("atlas::entryCount").max(1) as u16,
        },
        vel_samples: e
            .nodes("velSamples")
            .iter()
            .map(|s| VelSample {
                local_velocity: vec3_range(s, "local::velocity"),
                local_total: vec3_range(s, "local::totalDelta"),
                world_velocity: vec3_range(s, "world::velocity"),
                world_total: vec3_range(s, "world::totalDelta"),
            })
            .collect(),
        vis_samples: e
            .nodes("visSamples")
            .iter()
            .map(|s| VisSample { base: visual_state(s, "base"), amplitude: visual_state(s, "amplitude") })
            .collect(),
        visuals,
        coll_mins: std::array::from_fn(|i| e.float(&format!("collMins[{i}]"))),
        coll_maxs: std::array::from_fn(|i| e.float(&format!("collMaxs[{i}]"))),
        effect_on_impact: effect_ref(e, "effectOnImpact"),
        effect_on_death: effect_ref(e, "effectOnDeath"),
        effect_emitted: effect_ref(e, "effectEmitted"),
        emit_dist: float_range(e, "emitDist"),
        emit_dist_variance: float_range(e, "emitDistVariance"),
        sort_order: byte(e, "sortOrder"),
        lighting_frac: byte(e, "lightingFrac"),
        trail: e.node("trailDef").map(|t| TrailDef {
            scroll_time_msec: t.int("scrollTimeMsec") as i32,
            repeat_dist: t.int("repeatDist") as f32,
            split_dist: t.int("splitDist") as f32,
            verts: t
                .nodes("verts")
                .iter()
                .map(|v| TrailVertex {
                    pos: [v.float("pos[0]"), v.float("pos[1]")],
                    normal: [v.float("normal[0]"), v.float("normal[1]")],
                    tex_coord: v.float("texCoord"),
                })
                .collect(),
            inds: t.bytes("inds").chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect(),
        }),
    }
}

/// Surface types an impact table has an effect for (`SURF_TYPE_NUM`).
pub const SURFACE_TYPES: usize = 29;
/// Flesh impacts: body and head, each non-fatal and fatal (exit wounds).
pub const FLESH_TYPES: usize = 4;

/// `FxImpactTable`: bullet and grenade impact effects by impact type and
/// the surface hit. Its 12 rows are, in order: small bullets, large,
/// shotgun and armour piercing, each followed by its row for where the
/// bullet comes out, then grenade bounce, grenade explosion, rocket
/// explosion and dud (see [`Self::row`]; the order is the engine's
/// `CG_ImpactEffectForWeapon`).
/// Effects are named as the zone names them (a `,name` is defined in another
/// zone).
#[derive(Debug, Clone, Default)]
pub struct ImpactTable {
    pub name: String,
    pub entries: Vec<ImpactEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct ImpactEntry {
    /// By surface type (`SURF_TYPE_*`: 0 default, 5 concrete, 13 metal, ...).
    pub nonflesh: Vec<Option<String>>,
    /// By [`FleshImpact`].
    pub flesh: Vec<Option<String>>,
}

/// Which flesh effect a hit on a character uses (the order of
/// [`ImpactEntry::flesh`]: CoD4's tables put the fatal exit wounds at 1 and 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleshImpact {
    Body = 0,
    BodyFatal = 1,
    Head = 2,
    HeadFatal = 3,
}

impl ImpactTable {
    pub fn from_node(zone: &Zone, node: &GNode) -> ImpactTable {
        let names =
            |ids: Vec<Option<usize>>| ids.into_iter().map(|id| id.map(|i| zone.get(i).name().to_owned())).collect();
        ImpactTable {
            name: node.string("name").unwrap_or_default().to_owned(),
            entries: node
                .nodes("table")
                .iter()
                .map(|e| ImpactEntry { nonflesh: names(e.assets("nonflesh")), flesh: names(e.assets("flesh")) })
                .collect(),
        }
    }

    /// The row for `WeaponDef::impactType` (1 small bullets, 2 large, 3
    /// armour piercing, 4 shotgun, 5 grenade bounce, 6 grenade explosion, 7
    /// rocket explosion, 8 dud); bullets have a second row for where they
    /// come out (`exit`).
    pub fn row(impact_type: i32, exit: bool) -> Option<usize> {
        let bullet = |row: usize| Some(row + exit as usize);
        match impact_type {
            1 => bullet(0),
            2 => bullet(2),
            3 => bullet(6),
            4 => bullet(4),
            5..=8 => Some(impact_type as usize + 3),
            _ => None,
        }
    }

    /// The effect for a non-flesh surface (a `SURF_TYPE_*`).
    pub fn surface(&self, row: usize, surface: usize) -> Option<&str> {
        self.entries.get(row)?.nonflesh.get(surface)?.as_deref()
    }

    pub fn flesh(&self, row: usize, kind: FleshImpact) -> Option<&str> {
        self.entries.get(row)?.flesh.get(kind as usize)?.as_deref()
    }
}
