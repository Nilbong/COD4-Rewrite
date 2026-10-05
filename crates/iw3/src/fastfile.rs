//! `.ff` fastfile container.
//!
//! PC CoD4 fastfiles are an 8 byte magic (`IWffu100`, "u" = unsigned), a
//! version number (5) and a single zlib stream holding the zone data.

use anyhow::{Context, Result, bail};
use flate2::read::ZlibDecoder;
use std::io::Read;
use std::path::Path;

pub const MAGIC_UNSIGNED: &[u8; 8] = b"IWffu100";
pub const VERSION: u32 = 5;

/// Read a fastfile from disk and return its decompressed zone data.
pub fn load(path: &Path) -> Result<Vec<u8>> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    decompress(&raw).with_context(|| format!("decompressing {}", path.display()))
}

pub fn decompress(raw: &[u8]) -> Result<Vec<u8>> {
    if raw.len() < 12 {
        bail!("file too small to be a fastfile");
    }
    if &raw[..8] != MAGIC_UNSIGNED {
        bail!("bad magic {:?} (signed/console fastfiles are not supported)", &raw[..8]);
    }
    let version = u32::from_le_bytes(raw[8..12].try_into().unwrap());
    if version != VERSION {
        bail!("unsupported fastfile version {version} (expected {VERSION}, PC CoD4)");
    }
    let mut out = Vec::with_capacity(raw.len() * 3);
    ZlibDecoder::new(&raw[12..]).read_to_end(&mut out)?;
    Ok(out)
}
