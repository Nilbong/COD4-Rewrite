//! Owned, engine-agnostic versions of the IW3 assets we use.
//!
//! These keep the fields the rewrite needs and drop runtime/D3D-only state.

pub type AssetId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum AssetType {
    XModelPieces = 0,
    PhysPreset = 1,
    XAnimParts = 2,
    XModel = 3,
    Material = 4,
    TechniqueSet = 5,
    Image = 6,
    Sound = 7,
    SoundCurve = 8,
    LoadedSound = 9,
    ClipMap = 10,
    ClipMapPvs = 11,
    ComWorld = 12,
    GameWorldSp = 13,
    GameWorldMp = 14,
    MapEnts = 15,
    GfxWorld = 16,
    LightDef = 17,
    UiMap = 18,
    Font = 19,
    MenuList = 20,
    Menu = 21,
    LocalizeEntry = 22,
    Weapon = 23,
    SndDriverGlobals = 24,
    Fx = 25,
    ImpactFx = 26,
    AiType = 27,
    MpType = 28,
    Character = 29,
    XModelAlias = 30,
    RawFile = 31,
    StringTable = 32,
}

impl AssetType {
    pub fn from_u32(v: u32) -> Option<Self> {
        if v <= 32 {
            // SAFETY: repr(u32) enum with contiguous discriminants 0..=32.
            Some(unsafe { std::mem::transmute::<u32, AssetType>(v) })
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub enum Asset {
    TechniqueSet(TechniqueSet),
    Material(Material),
    Image(Image),
    XModel(XModel),
    XModelPieces(XModelPieces),
    PhysPreset(PhysPreset),
    LightDef(LightDef),
    ComWorld(ComWorld),
    GameWorldMp(String),
    MapEnts(MapEnts),
    GfxWorld(Box<GfxWorld>),
    ClipMap(Box<ClipMap>),
    RawFile(RawFile),
    /// Loaded by the schema-driven loader; see [`super::generic::GNode`].
    Generic(GenericAsset),
}

#[derive(Debug)]
pub struct GenericAsset {
    pub ty: AssetType,
    pub name: String,
    pub root: super::generic::GNode,
}

impl Asset {
    pub fn name(&self) -> &str {
        match self {
            Asset::TechniqueSet(a) => &a.name,
            Asset::Material(a) => &a.name,
            Asset::Image(a) => &a.name,
            Asset::XModel(a) => &a.name,
            Asset::XModelPieces(a) => &a.name,
            Asset::PhysPreset(a) => &a.name,
            Asset::LightDef(a) => &a.name,
            Asset::ComWorld(a) => &a.name,
            Asset::GameWorldMp(n) => n,
            Asset::MapEnts(a) => &a.name,
            Asset::GfxWorld(a) => &a.name,
            Asset::ClipMap(a) => &a.name,
            Asset::RawFile(a) => &a.name,
            Asset::Generic(a) => &a.name,
        }
    }
}

#[derive(Debug)]
pub struct TechniqueSet {
    pub name: String,
    pub world_vert_format: u8,
    /// Techniques by `MaterialTechniqueType`, where present inline.
    pub techniques: Vec<Option<Technique>>,
}

#[derive(Debug)]
pub struct Technique {
    pub name: String,
    pub passes: Vec<TechniquePass>,
}

#[derive(Debug, Default)]
pub struct TechniquePass {
    /// Shaders stored inline in this pass (shared shaders are references).
    pub vertex_shader: Option<ShaderProgram>,
    pub pixel_shader: Option<ShaderProgram>,
    /// `MaterialShaderArgument`s: (type, dest register, value). The value is
    /// the code constant/sampler index or the material name hash.
    pub args: Vec<(u16, u16, u32)>,
}

/// A D3D9 shader: name and bytecode.
#[derive(Debug, Clone)]
pub struct ShaderProgram {
    pub name: String,
    pub program: Vec<u8>,
}

pub const TECHNIQUE_LIT: usize = 7;
pub const TECHNIQUE_UNLIT: usize = 4;
pub const TECHNIQUE_EMISSIVE: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureSemantic {
    Function,
    Color,
    Normal,
    Specular,
    Water,
    Other(u8),
}

impl TextureSemantic {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Function,
            2 => Self::Color,
            5 => Self::Normal,
            8 => Self::Specular,
            11 => Self::Water,
            o => Self::Other(o),
        }
    }
}

#[derive(Debug)]
pub struct MaterialTexture {
    pub name_hash: u32,
    pub name_start: u8,
    pub name_end: u8,
    pub sampler_state: u8,
    pub semantic: TextureSemantic,
    pub image: Option<AssetId>,
}

#[derive(Debug)]
pub struct Material {
    pub name: String,
    pub game_flags: u8,
    pub sort_key: u8,
    pub surface_type_bits: u32,
    pub state_bits_entry: [u8; 34],
    pub state_flags: u8,
    pub camera_region: u8,
    pub technique_set: Option<AssetId>,
    pub textures: Vec<MaterialTexture>,
    pub constants: Vec<MaterialConstant>,
    pub state_bits: Vec<[u32; 2]>,
}

impl Material {
    /// State bits for a technique slot (e.g. [`TECHNIQUE_LIT`]).
    pub fn state_bits_for(&self, technique: usize) -> Option<[u32; 2]> {
        let idx = *self.state_bits_entry.get(technique)?;
        if idx == 0xff {
            return None;
        }
        self.state_bits.get(idx as usize).copied()
    }
}

#[derive(Debug)]
pub struct MaterialConstant {
    pub name_hash: u32,
    pub name: String,
    pub literal: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapType {
    TwoD,
    ThreeD,
    Cube,
    Other(u32),
}

#[derive(Debug)]
pub struct Image {
    pub name: String,
    pub map_type: MapType,
    pub semantic: u8,
    pub category: u8,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    /// Pixel data embedded in the zone (lightmaps, reflection probes, ...).
    /// Most images instead live in `images/<name>.iwi` inside an iwd.
    pub load_def: Option<ImageLoadDef>,
}

#[derive(Debug)]
pub struct ImageLoadDef {
    pub level_count: u8,
    pub flags: u8,
    pub dimensions: [u16; 3],
    /// D3DFORMAT four-cc or enum value.
    pub format: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct PackedVertex {
    pub xyz: [f32; 3],
    pub binormal_sign: f32,
    pub color: u32,
    pub tex_coord: u32,
    pub normal: u32,
    pub tangent: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct RigidVertList {
    /// Byte offset into the bone matrix array (divide by 64 for the index).
    pub bone_offset: u16,
    pub vert_count: u16,
    pub tri_offset: u16,
    pub tri_count: u16,
}

#[derive(Debug, Default)]
pub struct XSurface {
    pub tile_mode: u8,
    pub deformed: bool,
    pub vert_count: u16,
    pub tri_count: u16,
    pub base_tri_index: u16,
    pub base_vert_index: u16,
    /// Number of vertices influenced by 1, 2, 3 and 4 bones.
    pub blend_counts: [i16; 4],
    pub verts_blend: Vec<u16>,
    pub verts: Vec<PackedVertex>,
    pub tris: Vec<[u16; 3]>,
    pub vert_lists: Vec<RigidVertList>,
}

#[derive(Debug, Clone, Copy)]
pub struct XModelLod {
    pub dist: f32,
    pub num_surfs: u16,
    pub surf_index: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct BoneMat {
    pub quat: [f32; 4],
    pub trans: [f32; 3],
}

#[derive(Debug)]
pub struct XModel {
    pub name: String,
    pub num_bones: u8,
    pub num_root_bones: u8,
    pub bone_names: Vec<String>,
    pub parent_list: Vec<u8>,
    /// Local rotations of non-root bones, `i16 / 32767`.
    pub quats: Vec<[i16; 4]>,
    pub trans: Vec<[f32; 3]>,
    /// Base pose matrices in model space, one per bone.
    pub base_mat: Vec<BoneMat>,
    pub surfs: Vec<XSurface>,
    pub materials: Vec<Option<AssetId>>,
    pub lods: Vec<XModelLod>,
    pub radius: f32,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub contents: i32,
}

#[derive(Debug)]
pub struct XModelPieces {
    pub name: String,
    pub pieces: Vec<(Option<AssetId>, [f32; 3])>,
}

#[derive(Debug)]
pub struct PhysPreset {
    pub name: String,
    pub mass: f32,
    pub bounce: f32,
    pub friction: f32,
    /// How hard bullets and blasts push it, over the usual.
    pub bullet_force_scale: f32,
    pub explosive_force_scale: f32,
    /// Its broken pieces: how far they fly apart, and up (units/s).
    pub pieces_spread_fraction: f32,
    pub pieces_upward_velocity: f32,
    /// Its impact sounds: `{prefix}_{surface}`, else `{prefix}_default`.
    pub sound_prefix: String,
}

#[derive(Debug)]
pub struct LightDef {
    pub name: String,
    pub attenuation: Option<AssetId>,
}

#[derive(Debug)]
pub struct PrimaryLight {
    pub kind: u8,
    pub color: [f32; 3],
    pub dir: [f32; 3],
    pub origin: [f32; 3],
    pub radius: f32,
    pub cos_half_fov_outer: f32,
    pub cos_half_fov_inner: f32,
    pub def_name: Option<String>,
}

#[derive(Debug)]
pub struct ComWorld {
    pub name: String,
    pub primary_lights: Vec<PrimaryLight>,
}

#[derive(Debug)]
pub struct MapEnts {
    pub name: String,
    pub entity_string: String,
}

#[derive(Debug)]
pub struct RawFile {
    pub name: String,
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------- GfxWorld

#[derive(Debug, Clone, Copy)]
pub struct WorldVertex {
    pub xyz: [f32; 3],
    pub binormal_sign: f32,
    pub color: u32,
    pub tex_coord: [f32; 2],
    pub lmap_coord: [f32; 2],
    pub normal: u32,
    pub tangent: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct GfxSurface {
    pub first_vertex: i32,
    pub vertex_count: u16,
    pub tri_count: u16,
    pub base_index: i32,
    pub material: Option<AssetId>,
    pub lightmap_index: u8,
    pub reflection_probe_index: u8,
    pub primary_light_index: u8,
    pub flags: u8,
    pub bounds: [[f32; 3]; 2],
}

#[derive(Debug, Clone, Copy)]
pub struct StaticModel {
    pub model: Option<AssetId>,
    pub origin: [f32; 3],
    /// Rows are the model's forward/left/up axes in world space.
    pub axis: [[f32; 3]; 3],
    pub scale: f32,
    pub cull_dist: f32,
    pub flags: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct LightmapPair {
    pub primary: Option<AssetId>,
    pub secondary: Option<AssetId>,
}

#[derive(Debug)]
pub struct SunParse {
    pub name: String,
    pub ambient_scale: f32,
    pub ambient_color: [f32; 3],
    pub diffuse_fraction: f32,
    pub sun_light: f32,
    pub sun_color: [f32; 3],
    pub diffuse_color: [f32; 3],
    pub angles: [f32; 3],
}

/// `sunflare_t` (GfxWorld @380): the sun's sprite and lens flare, and how
/// looking at it blinds (darkens) and glares (lightens) the screen. Each
/// effect runs from its min to its max as the dot product of the view and
/// the sun direction goes from `*_min_dot` to `*_max_dot`; times are ms.
#[derive(Debug, Clone, Default)]
pub struct SunFlare {
    pub valid: bool,
    pub sprite: Option<AssetId>,
    pub flare: Option<AssetId>,
    pub sprite_size: f32,
    pub flare_min_size: f32,
    pub flare_min_dot: f32,
    pub flare_max_size: f32,
    pub flare_max_dot: f32,
    pub flare_max_alpha: f32,
    pub flare_fade_in: i32,
    pub flare_fade_out: i32,
    pub blind_min_dot: f32,
    pub blind_max_dot: f32,
    pub blind_max_darken: f32,
    pub blind_fade_in: i32,
    pub blind_fade_out: i32,
    pub glare_min_dot: f32,
    pub glare_max_dot: f32,
    pub glare_max_lighten: f32,
    pub glare_fade_in: i32,
    pub glare_fade_out: i32,
    /// Where the sun's effects are placed (CoD units).
    pub fx_position: [f32; 3],
}

#[derive(Debug)]
pub struct GfxWorld {
    pub name: String,
    pub base_name: String,
    pub indices: Vec<u16>,
    pub vertices: Vec<WorldVertex>,
    pub surfaces: Vec<GfxSurface>,
    pub static_models: Vec<StaticModel>,
    pub sky_start_surfs: Vec<i32>,
    pub sky_image: Option<AssetId>,
    pub sun: SunParse,
    pub sun_flare: SunFlare,
    pub sun_color_from_bsp: [f32; 3],
    pub lightmaps: Vec<LightmapPair>,
    /// Baked reflection cubemaps; surfaces pick one by `reflection_probe_index`.
    pub reflection_probes: Vec<ReflectionProbe>,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// Surface index ranges in `surfaces`, from `GfxWorldDpvsStatic`.
    pub lit_surfs: std::ops::Range<u32>,
    pub decal_surfs: std::ops::Range<u32>,
    pub emissive_surfs: std::ops::Range<u32>,
    /// Brush models (doors, script_brushmodels); entry 0 is the world.
    pub models: Vec<GfxBrushModel>,
    /// Baked light at points through the map, for lighting models.
    pub light_grid: LightGrid,
}

/// IW3's light grid: baked light at points 32 units apart horizontally and
/// 64 vertically, wherever models can go. Each point stores the light
/// arriving from 56 directions; the sun's direct light is not included (the
/// engine adds it at runtime).
///
/// Points are stored sparsely: one run-length encoded row per grid line
/// along `row_axis`, listing which columns (`col_axis`) and heights have
/// points. See [`LightGrid::points`].
#[derive(Debug, Default)]
pub struct LightGrid {
    /// Grid coordinates of the corners of the grid's bounds (see
    /// [`LightGrid::world_pos`]).
    pub mins: [u16; 3],
    pub maxs: [u16; 3],
    pub row_axis: u32,
    pub col_axis: u32,
    /// Per row, its offset in `raw_row_data` in 4-byte units; 0xFFFF if empty.
    pub row_data_start: Vec<u16>,
    pub raw_row_data: Vec<u8>,
    pub entries: Vec<LightGridEntry>,
    pub colors: Vec<LightGridColors>,
}

#[derive(Debug, Clone, Copy)]
pub struct LightGridEntry {
    pub colors_index: u16,
    pub primary_light_index: u8,
    pub needs_trace: u8,
}

/// Light from each of [`LightGrid::directions`], as 8-bit gamma-space RGB.
#[derive(Debug, Clone, Copy)]
pub struct LightGridColors(pub [[u8; 3]; 56]);

impl LightGrid {
    /// Grid spacing in world units along x, y and z.
    pub const SPACING: [f32; 3] = [32.0, 32.0, 64.0];

    /// The world-space position of grid point `p`.
    pub fn world_pos(p: [u32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| p[i] as f32 * Self::SPACING[i] - 131_072.0)
    }

    /// The 56 sample directions, in the order of [`LightGridColors`]: the
    /// points on the surface of a 4×4×4 lattice spanning the cube [-1, 1]³
    /// (x fastest, then y, then z), not normalised.
    pub fn directions() -> Vec<[f32; 3]> {
        let mut out = Vec::with_capacity(56);
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    let inside = (1..3).contains(&x) && (1..3).contains(&y) && (1..3).contains(&z);
                    if !inside {
                        out.push([x, y, z].map(|c| c as f32 * (2.0 / 3.0) - 1.0));
                    }
                }
            }
        }
        out
    }

    /// Every stored grid point with its entry.
    pub fn points(&self) -> Vec<([u32; 3], LightGridEntry)> {
        let (ra, ca) = (self.row_axis as usize, self.col_axis as usize);
        let mut out = Vec::with_capacity(self.entries.len());
        if ra > 1 || ca > 1 || ra == ca {
            return out;
        }
        let d = &self.raw_row_data;
        let u16_at = |o: usize| d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        for (row, &start) in self.row_data_start.iter().enumerate() {
            if start == 0xFFFF {
                continue;
            }
            let h = start as usize * 4;
            let (Some(col_start), Some(col_count), Some(z_start), Some(z_count)) =
                (u16_at(h), u16_at(h + 2), u16_at(h + 4), u16_at(h + 6))
            else {
                continue;
            };
            let Some(first) = d.get(h + 8..h + 12).map(|b| u32::from_le_bytes(b.try_into().unwrap())) else {
                continue;
            };
            let wide_z = z_count > 255;
            let (mut o, mut col, mut entry) = (h + 12, 0u32, first as usize);
            // Blocks of `run` columns, each column with `zn` points from `base` up.
            while col < col_count as u32 {
                let (Some(&run), Some(&zn)) = (d.get(o), d.get(o + 1)) else { break };
                if run == 0 {
                    break;
                }
                if zn == 0 {
                    o += 2;
                } else {
                    let Some(&lo) = d.get(o + 2) else { break };
                    let hi = if wide_z { d.get(o + 3).copied().unwrap_or(0) } else { 0 };
                    let base = lo as u32 | (hi as u32) << 8;
                    o += if wide_z { 4 } else { 3 };
                    for c in 0..run as u32 {
                        for k in 0..zn as u32 {
                            let Some(&e) = self.entries.get(entry + (c * zn as u32 + k) as usize) else { continue };
                            let mut p = [0u32; 3];
                            p[ra] = self.mins[ra] as u32 + row as u32;
                            p[ca] = col_start as u32 + col + c;
                            p[2] = z_start as u32 + base + k;
                            out.push((p, e));
                        }
                    }
                    entry += run as usize * zn as usize;
                }
                col += run as u32;
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReflectionProbe {
    pub origin: [f32; 3],
    pub image: Option<AssetId>,
}

#[derive(Debug, Clone, Copy)]
pub struct GfxBrushModel {
    pub bounds: [[f32; 3]; 2],
    pub surface_count: u16,
    pub start_surf_index: u16,
}

// ---------------------------------------------------------------- clipMap

#[derive(Debug, Clone, Copy)]
pub struct Plane {
    pub normal: [f32; 3],
    pub dist: f32,
}

#[derive(Debug, Clone)]
pub struct ClipMaterial {
    pub name: String,
    pub surface_flags: i32,
    pub content_flags: i32,
}

/// A convex collision brush: its AABB plus any non-axial bounding planes.
#[derive(Debug, Clone)]
pub struct Brush {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub contents: i32,
    /// Indices into `ClipMap::planes`.
    pub side_planes: Vec<u32>,
    /// Material index per non-axial side.
    pub side_materials: Vec<u32>,
    /// Material per axial side: [min x, min y, min z], [max x, max y, max z].
    pub axial_materials: [[i16; 3]; 2],
}

/// A dynamic entity (`DynEntityDef`): a model (or brush model) the world
/// places, that bullets and blasts can push or break.
#[derive(Debug, Clone, Copy)]
pub struct DynEntDef {
    /// `DynEntityType`: 1 clutter, 2 destructible.
    pub kind: i32,
    /// Rotation as a quaternion (x, y, z, w) and position, CoD space.
    pub quat: [f32; 4],
    pub origin: [f32; 3],
    pub model: Option<AssetId>,
    /// A brush model instead (`*n`), 0 for none.
    pub brush_model: u16,
    pub physics_brush_model: u16,
    pub destroy_fx: Option<AssetId>,
    pub destroy_pieces: Option<AssetId>,
    pub phys_preset: Option<AssetId>,
    pub health: i32,
    pub contents: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct ClipStaticModel {
    pub model: Option<AssetId>,
    pub origin: [f32; 3],
    pub inv_scaled_axis: [[f32; 3]; 3],
    pub absmin: [f32; 3],
    pub absmax: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
pub struct CModel {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub radius: f32,
    pub leaf_brush_node: i32,
    pub first_coll_aabb: u16,
    pub coll_aabb_count: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct CollisionAabbTree {
    pub origin: [f32; 3],
    pub half_size: [f32; 3],
    pub material_index: u16,
    pub child_count: u16,
    /// First child when `child_count > 0`, otherwise a partition index.
    pub index: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct CollisionPartition {
    pub tri_count: u8,
    pub first_tri: i32,
}

#[derive(Debug)]
pub struct ClipMap {
    pub name: String,
    pub planes: Vec<Plane>,
    pub static_models: Vec<ClipStaticModel>,
    pub materials: Vec<ClipMaterial>,
    pub brushes: Vec<Brush>,
    /// Triangle soup used for terrain and patch collision.
    pub verts: Vec<[f32; 3]>,
    pub tri_indices: Vec<u16>,
    pub partitions: Vec<CollisionPartition>,
    pub aabb_trees: Vec<CollisionAabbTree>,
    /// Sub-models; index 0 is the world, others are brush entities (`*1`, ...).
    pub cmodels: Vec<CModel>,
    pub map_ents: Option<AssetId>,
    pub dyn_ent_counts: [u16; 2],
    /// The map's dynamic entities (`dynEntDefList`): clutter (cinder blocks,
    /// boxes, cans, bottles) and destructibles, each list in turn.
    pub dyn_ents: Vec<DynEntDef>,
    /// The brush tree each model's leaf points into (`CModel::leaf_brush_node`).
    pub leaf_brush_nodes: Vec<LeafBrushNode>,
}

/// A node of a model's brush tree: a leaf listing brushes, or a split with
/// two children (at the given offsets from this node).
#[derive(Debug, Clone)]
pub struct LeafBrushNode {
    pub leaf_brush_count: i16,
    /// Indices into `ClipMap::brushes`, for a leaf.
    pub brushes: Vec<u16>,
    pub child_offsets: [u16; 2],
}

impl ClipMap {
    /// The brushes of the brush entities (doors, script brush models,
    /// triggers): `cmodels[1..]`. They move with their entities or aren't
    /// solid to players at all, so they aren't part of the static world.
    pub fn entity_brushes(&self) -> std::collections::HashSet<u32> {
        let mut out = std::collections::HashSet::new();
        for m in self.cmodels.iter().skip(1) {
            let mut stack = vec![(m.leaf_brush_node, 0)];
            while let Some((i, depth)) = stack.pop() {
                let Some(node) = usize::try_from(i).ok().and_then(|i| self.leaf_brush_nodes.get(i)) else { continue };
                if node.leaf_brush_count > 0 {
                    out.extend(node.brushes.iter().map(|&b| b as u32));
                } else if depth < 64 {
                    for off in node.child_offsets.iter().filter(|&&o| o > 0) {
                        stack.push((i + *off as i32, depth + 1));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_grid_has_56_directions_on_the_cube_surface() {
        let dirs = LightGrid::directions();
        assert_eq!(dirs.len(), 56);
        assert!(dirs.iter().all(|d| d.iter().any(|c| (c.abs() - 1.0).abs() < 1e-5)));
    }

    #[test]
    fn light_grid_rows_decode() {
        // One row (x = 10) with columns y = 20..23: a block of two columns
        // with three points from z = 5 + 1, an empty column, then one column
        // with one point at z = 5.
        let mut raw = Vec::new();
        for v in [20u16, 4, 5, 3] {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        raw.extend_from_slice(&0u32.to_le_bytes());
        raw.extend_from_slice(&[2, 3, 1, 1, 0, 1, 1, 0]);
        let entry = |i: u16| LightGridEntry { colors_index: i, primary_light_index: 0, needs_trace: 0 };
        let grid = LightGrid {
            mins: [10, 20, 5],
            maxs: [10, 23, 8],
            row_axis: 0,
            col_axis: 1,
            row_data_start: vec![0],
            raw_row_data: raw,
            entries: (0..7).map(entry).collect(),
            colors: Vec::new(),
        };
        let points: Vec<([u32; 3], u16)> = grid.points().into_iter().map(|(p, e)| (p, e.colors_index)).collect();
        assert_eq!(
            points,
            vec![
                ([10, 20, 6], 0),
                ([10, 20, 7], 1),
                ([10, 20, 8], 2),
                ([10, 21, 6], 3),
                ([10, 21, 7], 4),
                ([10, 21, 8], 5),
                ([10, 23, 5], 6),
            ]
        );
    }
}
