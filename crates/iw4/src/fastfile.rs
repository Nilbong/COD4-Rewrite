//! Modern Warfare 2 `.ff` fastfiles.
//!
//! `IWff0100`, version 276, a flag byte and an 8 byte creation time, then
//! the signed part: `IWffs100`, a reserved word, a 256 byte signature, the
//! zone's name and the hashes of its blocks (the auth header is 0x2000
//! bytes from `IWffs100`). The data follows in 2 MB pieces, each after an
//! 8 KB block of its pieces' hashes; joined, they are one zlib stream.
//! (The hashes and the signature are checked by the game, not here.)

use anyhow::{Context, Result, bail};
use flate2::read::ZlibDecoder;
use std::io::Read;
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"IWff0100";
pub const SIGNED: &[u8; 8] = b"IWffs100";
pub const VERSION: u32 = 276;
/// Where `IWffs100` starts, and the auth header's size from there.
const AUTH_AT: usize = 0x15;
const AUTH_SIZE: usize = 0x2000;
/// Each piece of data, and the hash block before it.
const PIECE: usize = 0x20_0000;
const HASHES: usize = 0x2000;

/// Read a fastfile from disk and return its decompressed zone data.
pub fn load(path: &Path) -> Result<Vec<u8>> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    decompress(&raw).with_context(|| format!("decompressing {}", path.display()))
}

pub fn decompress(raw: &[u8]) -> Result<Vec<u8>> {
    if raw.len() < AUTH_AT + AUTH_SIZE || &raw[..8] != MAGIC {
        bail!("not a Modern Warfare 2 fastfile");
    }
    let version = u32::from_le_bytes(raw[8..12].try_into().unwrap());
    if version != VERSION {
        bail!("unsupported fastfile version {version} (expected {VERSION}, PC Modern Warfare 2)");
    }
    if &raw[AUTH_AT..AUTH_AT + 8] != SIGNED {
        bail!("not a signed (IWffs100) fastfile");
    }
    let mut compressed = Vec::with_capacity(raw.len());
    let mut at = AUTH_AT + AUTH_SIZE;
    while at < raw.len() {
        at += HASHES;
        let end = (at + PIECE).min(raw.len());
        if at < end {
            compressed.extend_from_slice(&raw[at..end]);
        }
        at = end;
    }
    let mut out = Vec::with_capacity(compressed.len() * 2);
    ZlibDecoder::new(&compressed[..]).read_to_end(&mut out)?;
    Ok(out)
}
