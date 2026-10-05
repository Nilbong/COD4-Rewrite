//! Zone (decompressed fastfile) parser.
//!
//! The asset stream is parsed in order. Parsing must understand every asset
//! type it meets, because the stream carries no sizes; [`Zone::parse`] stops
//! cleanly at the first unsupported top-level asset and reports how far it got.

pub mod generic;
mod load;
pub mod reader;
pub mod schema;
pub mod types;

pub use types::*;

use anyhow::{Result, bail};
use reader::Reader;

/// Size of the `XFile` header: data size, external size and nine block sizes.
const XFILE_HEADER_SIZE: usize = 4 + 4 + 4 * reader::BLOCK_COUNT;

#[derive(Debug, Default)]
pub struct ZoneStats {
    pub top_level_total: usize,
    pub top_level_parsed: usize,
    /// Set if parsing stopped early.
    pub stopped_at: Option<String>,
    pub unresolved_aliases: usize,
    pub unresolved_strings: usize,
    /// Block positions when parsing stopped (see `Reader::block_positions`).
    pub block_positions: [u32; reader::BLOCK_COUNT],
}

pub struct Zone {
    pub script_strings: Vec<String>,
    /// Every asset loaded, including ones nested inside other assets.
    pub assets: Vec<Asset>,
    /// Top-level asset list entries, in stream order.
    pub top_level: Vec<(u32, Option<AssetId>)>,
    pub stats: ZoneStats,
}

#[derive(Clone, Copy)]
pub struct ParseOptions {
    /// Stop after the first top-level asset of this type has been parsed.
    /// Map zones put everything the renderer needs before `clipMap`.
    pub stop_after: Option<AssetType>,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions { stop_after: None }
    }
}

/// Read just the top-level asset type list (no asset data is parsed).
pub fn asset_list(data: &[u8]) -> Result<(Vec<u32>, Vec<String>)> {
    let mut r = Reader::new(data, XFILE_HEADER_SIZE);
    let list = r.read_unblocked(16)?;
    let string_count = list.u32(0) as usize;
    let mut strings = Vec::new();
    if list.ptr(4).is_inline() {
        let (_, ptrs) = r.array(4, 4, string_count)?;
        for i in 0..string_count {
            strings.push(r.xstring_or_empty(ptrs.ptr(i * 4))?);
        }
    }
    let count = list.u32(8) as usize;
    let (_, entries) = r.array(4, 8, count)?;
    Ok(((0..count).map(|i| entries.u32(i * 8)).collect(), strings))
}

impl Zone {
    pub fn parse(data: &[u8], opts: ParseOptions) -> Result<Zone> {
        if data.len() < XFILE_HEADER_SIZE + 16 {
            bail!("zone too small");
        }
        let mut l = load::Loader::new(Reader::new(data, XFILE_HEADER_SIZE));

        // XAssetList { int scriptStringCount; char** scriptStrings; int assetCount; XAsset* assets; }
        // The game reads the list header into a global, so it takes no space
        // in any block.
        let list = l.r.read_unblocked(16)?;
        let string_count = list.u32(0) as usize;
        if list.ptr(4).is_inline() {
            let (_, ptrs) = l.r.array(4, 4, string_count)?;
            for i in 0..string_count {
                let s = l.r.xstring_or_empty(ptrs.ptr(i * 4))?;
                l.script_strings.push(s);
            }
        }

        let asset_count = list.u32(8) as usize;
        let (entries_loc, entries) = l.r.array(4, 8, asset_count)?;
        let mut top_level = Vec::with_capacity(asset_count);
        let mut stats = ZoneStats { top_level_total: asset_count, ..Default::default() };

        let trace = std::env::var_os("IW3_TRACE").is_some();
        for i in 0..asset_count {
            let e = entries.elem(i, 8);
            let raw_type = e.u32(0);
            let Some(ty) = AssetType::from_u32(raw_type) else {
                stats.stopped_at = Some(format!("asset #{i}: invalid type {raw_type}"));
                break;
            };
            if !load::Loader::supports(ty) {
                stats.stopped_at = Some(format!("asset #{i}: unsupported type {ty:?}"));
                break;
            }
            let pos = l.r.file_pos();
            let slot = Some(((entries_loc.block as u32) << 28) | (entries_loc.offset + i as u32 * 8 + 4));
            match l.handle(e.ptr(4), ty, slot) {
                Ok(id) => {
                    if trace {
                        let name = id.map(|id| l.assets[id].name().to_owned()).unwrap_or_default();
                        eprintln!("#{i:4} {ty:?} @ {pos:#x}..{:#x} {name} unresolved {}/{} virt {:#x}", l.r.file_pos(), l.r.unresolved_aliases, l.r.unresolved_strings, l.r.block_positions()[4]);
                    }
                    top_level.push((raw_type, id))
                }
                Err(err) => {
                    return Err(err.context(format!("loading top-level asset #{i} ({ty:?}) at zone offset {pos:#x}")));
                }
            }
            stats.top_level_parsed += 1;
            if opts.stop_after == Some(ty) {
                break;
            }
        }
        stats.block_positions = l.r.block_positions();
        stats.unresolved_aliases = l.r.unresolved_aliases;
        stats.unresolved_strings = l.r.unresolved_strings;
        Ok(Zone { script_strings: l.script_strings, assets: l.assets, top_level, stats })
    }

    pub fn get(&self, id: AssetId) -> &Asset {
        &self.assets[id]
    }

    pub fn material(&self, id: AssetId) -> Option<&Material> {
        match &self.assets[id] {
            Asset::Material(m) => Some(m),
            _ => None,
        }
    }

    pub fn image(&self, id: AssetId) -> Option<&Image> {
        match &self.assets[id] {
            Asset::Image(m) => Some(m),
            _ => None,
        }
    }

    pub fn xmodel(&self, id: AssetId) -> Option<&XModel> {
        match &self.assets[id] {
            Asset::XModel(m) => Some(m),
            _ => None,
        }
    }

    pub fn technique_set(&self, id: AssetId) -> Option<&TechniqueSet> {
        match &self.assets[id] {
            Asset::TechniqueSet(m) => Some(m),
            _ => None,
        }
    }

    pub fn gfx_world(&self) -> Option<&GfxWorld> {
        self.assets.iter().find_map(|a| match a {
            Asset::GfxWorld(w) => Some(&**w),
            _ => None,
        })
    }

    pub fn clip_map(&self) -> Option<&ClipMap> {
        self.assets.iter().find_map(|a| match a {
            Asset::ClipMap(c) => Some(&**c),
            _ => None,
        })
    }

    pub fn com_world(&self) -> Option<&ComWorld> {
        self.assets.iter().find_map(|a| match a {
            Asset::ComWorld(c) => Some(c),
            _ => None,
        })
    }

    pub fn map_ents(&self) -> Option<&MapEnts> {
        self.assets.iter().find_map(|a| match a {
            Asset::MapEnts(c) => Some(c),
            _ => None,
        })
    }

    pub fn find(&self, name: &str) -> Option<AssetId> {
        self.assets.iter().position(|a| a.name() == name)
    }
}
