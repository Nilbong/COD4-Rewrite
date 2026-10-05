//! World at War `.ff` fastfiles: the same container as CoD4's (`IWffu100`, a
//! version number and one zlib stream), version 387.

use anyhow::{Context, Result, bail};
use flate2::read::ZlibDecoder;
use std::io::Read;
use std::path::Path;

pub const VERSION: u32 = 387;

/// Read a fastfile from disk and return its decompressed zone data.
pub fn load(path: &Path) -> Result<Vec<u8>> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    decompress(&raw).with_context(|| format!("decompressing {}", path.display()))
}

pub fn decompress(raw: &[u8]) -> Result<Vec<u8>> {
    if raw.len() < 12 || &raw[..8] != iw3::fastfile::MAGIC_UNSIGNED {
        bail!("not an unsigned fastfile");
    }
    let version = u32::from_le_bytes(raw[8..12].try_into().unwrap());
    if version != VERSION {
        bail!("unsupported fastfile version {version} (expected {VERSION}, PC World at War)");
    }
    let mut out = Vec::with_capacity(raw.len() * 2);
    ZlibDecoder::new(&raw[12..]).read_to_end(&mut out)?;
    Ok(out)
}
