//! Modern Warfare 2 zone (decompressed fastfile) parser.
//!
//! The same design as `iw3::zone`: the asset stream is parsed in order, and
//! since it carries no sizes, parsing must understand every asset type it
//! meets. Every asset is loaded by the schema-driven loader ([`generic`]);
//! [`crate::convert`] turns models, materials and images into `iw3` types.

pub mod generic;
pub mod reader;
pub mod schema;

pub use generic::{GNode, GVal};

use anyhow::{Result, bail};
use reader::{Ptr, Reader};

/// Size of the `XFile` header: data size, external size and eight block sizes.
const XFILE_HEADER_SIZE: usize = 4 + 4 + 4 * reader::BLOCK_COUNT;

pub type AssetId = usize;

/// `XAssetType` in Modern Warfare 2's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetType {
    PhysPreset,
    PhysCollmap,
    XAnimParts,
    XModelSurfs,
    XModel,
    Material,
    PixelShader,
    VertexShader,
    VertexDecl,
    TechniqueSet,
    Image,
    Sound,
    SoundCurve,
    LoadedSound,
    ClipMapSp,
    ClipMapMp,
    ComWorld,
    GameWorldSp,
    GameWorldMp,
    MapEnts,
    FxWorld,
    GfxWorld,
    LightDef,
    UiMap,
    Font,
    MenuList,
    Menu,
    LocalizeEntry,
    Weapon,
    SndDriverGlobals,
    Fx,
    ImpactFx,
    AiType,
    MpType,
    Character,
    XModelAlias,
    RawFile,
    StringTable,
    Leaderboard,
    StructuredDataDef,
    Tracer,
    Vehicle,
    AddonMapEnts,
}

impl AssetType {
    const ALL: [AssetType; 43] = {
        use AssetType::*;
        [
            PhysPreset, PhysCollmap, XAnimParts, XModelSurfs, XModel, Material, PixelShader, VertexShader, VertexDecl, TechniqueSet, Image, Sound, SoundCurve, LoadedSound, ClipMapSp, ClipMapMp, ComWorld, GameWorldSp, GameWorldMp, MapEnts, FxWorld, GfxWorld, LightDef, UiMap, Font, MenuList, Menu, LocalizeEntry, Weapon, SndDriverGlobals, Fx, ImpactFx, AiType, MpType, Character, XModelAlias, RawFile, StringTable, Leaderboard, StructuredDataDef, Tracer, Vehicle, AddonMapEnts
        ]
    };

    pub fn from_u32(v: u32) -> Option<AssetType> {
        Self::ALL.get(v as usize).copied()
    }
}

/// An asset and everything hanging off it.
#[derive(Debug)]
pub struct Asset {
    pub ty: AssetType,
    pub name: String,
    pub root: GNode,
}

#[derive(Debug, Default)]
pub struct ZoneStats {
    pub top_level_total: usize,
    pub top_level_parsed: usize,
    /// Set if parsing stopped early.
    pub stopped_at: Option<String>,
    pub unresolved_aliases: usize,
    pub unresolved_strings: usize,
    /// Block positions when parsing stopped; after a full parse they equal
    /// the block sizes in the XFile header ([`Zone::block_sizes`]).
    pub block_positions: [u32; reader::BLOCK_COUNT],
}

pub struct Zone {
    pub script_strings: Vec<String>,
    /// Every asset loaded, including ones nested inside other assets.
    pub assets: Vec<Asset>,
    /// Top-level asset list entries, in stream order.
    pub top_level: Vec<(u32, Option<AssetId>)>,
    pub stats: ZoneStats,
    /// Block sizes from the XFile header.
    pub block_sizes: [u32; reader::BLOCK_COUNT],
}

pub(crate) struct Loader<'a> {
    pub r: Reader<'a>,
    pub assets: Vec<Asset>,
}

impl<'a> Loader<'a> {
    /// Load an asset through a handle (`XModel*`, `Material*`, ...).
    /// `slot` is where the pointer field itself lives: once an asset has been
    /// loaded inline, later references to it point at that field.
    pub fn handle(&mut self, ptr: Ptr, ty: AssetType, slot: Option<u32>) -> Result<Option<AssetId>> {
        match ptr {
            Ptr::Null => Ok(None),
            Ptr::Ref { .. } => Ok(self.r.alias(ptr)),
            Ptr::Inline | Ptr::Insert => {
                let inserted = (ptr == Ptr::Insert).then(|| self.r.insert_alias_slot());
                let Some(st) = generic::struct_for_asset_type(ty) else {
                    bail!("asset type {ty:?} has no schema");
                };
                let root = self.generic_asset(st)?;
                let name = asset_name(&root);
                let id = self.assets.len();
                let name_for_trace = name.clone();
                self.assets.push(Asset { ty, name, root });
                for key in [inserted, slot].into_iter().flatten() {
                    self.r.aliases.insert(key, id);
                }
                if std::env::var_os("IW4_TRACE_ALIAS").is_some() {
                    eprintln!("alias {ty:?} {name_for_trace:?} inserted {:?} slot {:?}", inserted.map(|k| format!("{k:#x}")), slot.map(|k| format!("{k:#x}")));
                }
                Ok(Some(id))
            }
        }
    }
}

fn asset_name(root: &GNode) -> String {
    ["name", "aliasName", "szInternalName", "fontName", "filename"]
        .iter()
        .find_map(|f| root.string(f).map(str::to_owned))
        // Materials keep their name in `info`, menus in `window`.
        .or_else(|| root.node("info").and_then(|i| i.string("name")).map(str::to_owned))
        .or_else(|| root.node("window").and_then(|w| w.string("name")).map(str::to_owned))
        .unwrap_or_default()
}

#[derive(Clone, Copy, Default)]
pub struct ParseOptions {
    /// Stop after the first top-level asset of this type has been parsed.
    pub stop_after: Option<AssetType>,
}

impl Zone {
    pub fn parse(data: &[u8], opts: ParseOptions) -> Result<Zone> {
        if data.len() < XFILE_HEADER_SIZE + 16 {
            bail!("zone too small");
        }
        let header = reader::S(&data[..XFILE_HEADER_SIZE]);
        let block_sizes = std::array::from_fn(|i| header.u32(8 + i * 4));
        let mut l = Loader { r: Reader::new(data, XFILE_HEADER_SIZE), assets: Vec::new() };

        // XAssetList { ScriptStringList { int count; char** strings; }; int assetCount; XAsset* assets; }
        // The game reads the list header into a global, so it takes no space
        // in any block.
        let list = l.r.read_unblocked(16)?;
        let string_count = list.u32(0) as usize;
        let mut script_strings = Vec::new();
        if list.ptr(4).is_inline() {
            let (_, ptrs) = l.r.array(4, 4, string_count)?;
            for i in 0..string_count {
                script_strings.push(l.r.xstring_or_empty(ptrs.ptr(i * 4))?);
            }
        }

        let asset_count = list.u32(8) as usize;
        let (entries_loc, entries) = l.r.array(4, 8, asset_count)?;
        let mut top_level = Vec::with_capacity(asset_count);
        let mut stats = ZoneStats { top_level_total: asset_count, ..Default::default() };
        let trace = std::env::var_os("IW4_TRACE").is_some();
        // Debug: `IW4_TIME` reports the time spent per asset type.
        let timing = std::env::var_os("IW4_TIME").is_some();
        let mut times: std::collections::HashMap<AssetType, f32> = std::collections::HashMap::new();
        for i in 0..asset_count {
            let e = entries.elem(i, 8);
            let raw_type = e.u32(0);
            let Some(ty) = AssetType::from_u32(raw_type) else {
                stats.stopped_at = Some(format!("asset #{i}: invalid type {raw_type}"));
                break;
            };
            if generic::struct_for_asset_type(ty).is_none() {
                // Nothing is streamed for these (OpenAssetTools skips them too).
                top_level.push((raw_type, None));
                stats.top_level_parsed += 1;
                continue;
            }
            let pos = l.r.file_pos();
            let slot = Some(reader::key(entries_loc.block, entries_loc.offset + i as u32 * 8 + 4));
            let started = std::time::Instant::now();
            let handled = l.handle(e.ptr(4), ty, slot);
            if timing {
                *times.entry(ty).or_insert(0.0) += started.elapsed().as_secs_f32();
            }
            match handled {
                Ok(id) => {
                    if trace {
                        let name = id.map(|id| l.assets[id].name.clone()).unwrap_or_default();
                        eprintln!(
                            "#{i:5} {ty:?} @ {pos:#x}..{:#x} {name} unresolved {}/{}",
                            l.r.file_pos(),
                            l.r.unresolved_aliases,
                            l.r.unresolved_strings
                        );
                    }
                    top_level.push((raw_type, id));
                }
                Err(err) => {
                    stats.stopped_at = Some(format!("asset #{i} ({ty:?}) at {pos:#x}: {err:#}"));
                    break;
                }
            }
            stats.top_level_parsed += 1;
            if opts.stop_after == Some(ty) {
                break;
            }
        }
        if timing {
            let mut t: Vec<_> = times.into_iter().collect();
            t.sort_by(|a, b| b.1.total_cmp(&a.1));
            eprintln!("parse time by type: {:?}", t.iter().take(8).map(|(k, v)| format!("{k:?} {v:.2}s")).collect::<Vec<_>>());
        }
        stats.block_positions = l.r.block_positions();
        if std::env::var_os("IW4_TRACE_BLOCKS").is_some() {
            eprintln!("read per block {:?}; file pos {:#x} of {:#x}", l.r.read_per_block, l.r.file_pos(), data.len());
        }
        stats.unresolved_aliases = l.r.unresolved_aliases;
        stats.unresolved_strings = l.r.unresolved_strings;
        Ok(Zone { script_strings, assets: l.assets, top_level, stats, block_sizes })
    }

    pub fn find(&self, ty: AssetType, name: &str) -> Option<AssetId> {
        self.assets.iter().position(|a| a.ty == ty && a.name == name)
    }

    pub fn of_type(&self, ty: AssetType) -> impl Iterator<Item = (AssetId, &Asset)> {
        self.assets.iter().enumerate().filter(move |(_, a)| a.ty == ty)
    }
}
