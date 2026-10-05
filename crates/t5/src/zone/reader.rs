//! Low-level zone stream reader (Black Ops), adapted from `iw3::zone::reader`.
//!
//! A zone is the in-memory image of a set of assets, serialised depth-first.
//! Black Ops streams it into seven memory "blocks" (CoD4 has nine: no vertex
//! or index blocks here). Every pointer in the stream is one of:
//!
//! * `0` – null
//! * `-1` – the pointee follows inline in the stream
//! * `-2` – the pointee follows inline, and a 4 byte alias slot is first
//!   reserved in the virtual block so later references can find the asset
//! * anything else – `((block << 29) | offset) + 1` (three block bits, where
//!   CoD4 has four), a reference to data that was already loaded
//!
//! The file itself carries no padding. Alignment is only applied to the
//! *block* positions, which we track exactly so references can be resolved.

use anyhow::{Result, bail};
use std::collections::HashMap;

pub const BLOCK_TEMP: usize = 0;
pub const BLOCK_RUNTIME: usize = 1;
pub const BLOCK_LARGE_RUNTIME: usize = 2;
pub const BLOCK_PHYSICAL_RUNTIME: usize = 3;
pub const BLOCK_VIRTUAL: usize = 4;
pub const BLOCK_LARGE: usize = 5;
pub const BLOCK_PHYSICAL: usize = 6;
pub const BLOCK_COUNT: usize = 7;

/// Bits of a pointer that hold the block offset.
const OFFSET_BITS: u32 = 29;

fn is_runtime(block: usize) -> bool {
    matches!(block, BLOCK_RUNTIME | BLOCK_LARGE_RUNTIME | BLOCK_PHYSICAL_RUNTIME)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ptr {
    Null,
    Inline,
    Insert,
    Ref { block: usize, offset: u32 },
}

impl Ptr {
    pub fn from_raw(v: u32) -> Ptr {
        match v {
            0 => Ptr::Null,
            0xFFFF_FFFF => Ptr::Inline,
            0xFFFF_FFFE => Ptr::Insert,
            v => {
                let x = v - 1;
                Ptr::Ref { block: (x >> OFFSET_BITS) as usize, offset: x & ((1 << OFFSET_BITS) - 1) }
            }
        }
    }

    pub fn is_inline(self) -> bool {
        matches!(self, Ptr::Inline | Ptr::Insert)
    }
}

/// Little-endian field access into a struct image read from the stream.
#[derive(Clone, Copy)]
pub struct S<'a>(pub &'a [u8]);

impl<'a> S<'a> {
    pub fn u8(&self, o: usize) -> u8 {
        self.0[o]
    }
    pub fn i8(&self, o: usize) -> i8 {
        self.0[o] as i8
    }
    pub fn u16(&self, o: usize) -> u16 {
        u16::from_le_bytes([self.0[o], self.0[o + 1]])
    }
    pub fn i16(&self, o: usize) -> i16 {
        self.u16(o) as i16
    }
    pub fn u32(&self, o: usize) -> u32 {
        u32::from_le_bytes(self.0[o..o + 4].try_into().unwrap())
    }
    pub fn i32(&self, o: usize) -> i32 {
        self.u32(o) as i32
    }
    pub fn f32(&self, o: usize) -> f32 {
        f32::from_bits(self.u32(o))
    }
    pub fn vec3(&self, o: usize) -> [f32; 3] {
        [self.f32(o), self.f32(o + 4), self.f32(o + 8)]
    }
    pub fn vec4(&self, o: usize) -> [f32; 4] {
        [self.f32(o), self.f32(o + 4), self.f32(o + 8), self.f32(o + 12)]
    }
    pub fn ptr(&self, o: usize) -> Ptr {
        Ptr::from_raw(self.u32(o))
    }
    /// The `i`th element of an array of `size` byte records.
    pub fn elem(&self, i: usize, size: usize) -> S<'a> {
        S(&self.0[i * size..(i + 1) * size])
    }
    pub fn cstr(&self, o: usize, len: usize) -> String {
        let b = &self.0[o..o + len];
        let end = b.iter().position(|&c| c == 0).unwrap_or(len);
        String::from_utf8_lossy(&b[..end]).into_owned()
    }
}

/// Where an inline allocation landed: its block position and file position.
#[derive(Clone, Copy, Debug)]
pub struct Loc {
    pub block: usize,
    pub offset: u32,
    pub file_pos: usize,
}

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    block_pos: [u32; BLOCK_COUNT],
    cur: usize,
    stack: Vec<usize>,
    temp_saves: Vec<u32>,
    /// Block offset (packed as `block << 29 | offset`) -> file position for
    /// every inline allocation in a persistent block.
    allocs: HashMap<u32, usize>,
    /// Packed block offset of a pointer field (or `-2` alias slot) that holds
    /// an asset -> asset index.
    pub(crate) aliases: HashMap<u32, usize>,
    pub unresolved_aliases: usize,
    pub unresolved_strings: usize,
}

pub fn key(block: usize, offset: u32) -> u32 {
    ((block as u32) << OFFSET_BITS) | offset
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], start: usize) -> Self {
        Reader {
            data,
            pos: start,
            block_pos: [0; BLOCK_COUNT],
            cur: BLOCK_VIRTUAL,
            stack: Vec::new(),
            temp_saves: Vec::new(),
            allocs: HashMap::new(),
            aliases: HashMap::new(),
            unresolved_aliases: 0,
            unresolved_strings: 0,
        }
    }

    /// Current position in each block; after a full parse these equal the
    /// block sizes recorded in the XFile header.
    pub fn block_positions(&self) -> [u32; BLOCK_COUNT] {
        self.block_pos
    }

    pub fn file_pos(&self) -> usize {
        self.pos
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn push(&mut self, block: usize) {
        self.stack.push(self.cur);
        if block == BLOCK_TEMP {
            self.temp_saves.push(self.block_pos[BLOCK_TEMP]);
        }
        self.cur = block;
    }

    /// Leaving the temp block rewinds it: its contents are discarded.
    pub fn pop(&mut self) {
        let popped = self.cur;
        self.cur = self.stack.pop().expect("block stack underflow");
        if popped == BLOCK_TEMP {
            self.block_pos[BLOCK_TEMP] = self.temp_saves.pop().expect("temp stack underflow");
        }
    }

    /// Align the current block for a new allocation and remember where it is.
    pub fn alloc(&mut self, align: u32) -> Loc {
        let p = &mut self.block_pos[self.cur];
        *p = (*p + align - 1) & !(align - 1);
        let loc = Loc { block: self.cur, offset: *p, file_pos: self.pos };
        if self.cur != BLOCK_TEMP && !is_runtime(self.cur) {
            self.allocs.insert(key(loc.block, loc.offset), loc.file_pos);
        }
        loc
    }

    /// Consume `n` bytes in the current block. Runtime blocks have no backing
    /// file data, so they return an empty slice.
    pub fn read(&mut self, n: usize) -> Result<&'a [u8]> {
        self.block_pos[self.cur] += n as u32;
        if is_runtime(self.cur) {
            return Ok(&[]);
        }
        self.read_unblocked(n).map(|s| s.0)
    }

    /// Whether the current block is a runtime block (no file data).
    pub fn in_runtime_block(&self) -> bool {
        is_runtime(self.cur)
    }

    /// Read file data that is not placed in any block.
    pub fn read_unblocked(&mut self, n: usize) -> Result<S<'a>> {
        let end = self.pos + n;
        if end > self.data.len() {
            bail!("read past end of zone (pos {:#x}, len {n})", self.pos);
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(S(s))
    }

    /// `alloc` + `read` for an inline array of `count` records.
    pub fn array(&mut self, align: u32, size: usize, count: usize) -> Result<(Loc, S<'a>)> {
        if !is_runtime(self.cur) && size.saturating_mul(count) > self.data.len() - self.pos {
            bail!("array of {count} x {size} bytes at {:#x} runs past the end of the zone", self.pos);
        }
        let loc = self.alloc(align);
        let bytes = self.read(size * count)?;
        Ok((loc, S(bytes)))
    }

    fn read_cstring(&mut self) -> Result<String> {
        self.alloc(1);
        let rest = &self.data[self.pos..];
        let Some(len) = rest.iter().position(|&c| c == 0) else {
            bail!("unterminated string at {:#x}", self.pos);
        };
        let s = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.read(len + 1)?;
        Ok(s)
    }

    /// Resolve a reference pointer to the file position of its data.
    pub fn deref(&self, ptr: Ptr) -> Option<usize> {
        match ptr {
            Ptr::Ref { block, offset } => self.allocs.get(&key(block, offset)).copied(),
            _ => None,
        }
    }

    /// `const char*` field.
    pub fn xstring(&mut self, ptr: Ptr) -> Result<Option<String>> {
        Ok(match ptr {
            Ptr::Null => None,
            Ptr::Inline | Ptr::Insert => Some(self.read_cstring()?),
            Ptr::Ref { .. } => match self.deref(ptr) {
                Some(fp) => {
                    let rest = &self.data[fp..];
                    let len = rest.iter().position(|&c| c == 0).unwrap_or(0);
                    Some(String::from_utf8_lossy(&rest[..len]).into_owned())
                }
                None => {
                    self.unresolved_strings += 1;
                    if std::env::var_os("T5_TRACE").is_some() && self.unresolved_strings < 5 {
                        if let Ptr::Ref { block, offset } = ptr {
                            eprintln!("unresolved string -> block {block} offset {offset:#x}");
                        }
                    }
                    Some(String::from("<unresolved>"))
                }
            },
        })
    }

    pub fn xstring_or_empty(&mut self, ptr: Ptr) -> Result<String> {
        Ok(self.xstring(ptr)?.unwrap_or_default())
    }

    /// Reserve the 4 byte alias slot that precedes data behind a `-2` pointer.
    pub fn insert_alias_slot(&mut self) -> u32 {
        self.push(BLOCK_VIRTUAL);
        let loc = self.alloc(4);
        self.block_pos[BLOCK_VIRTUAL] += 4;
        self.pop();
        key(BLOCK_VIRTUAL, loc.offset)
    }

    /// Resolve a reference to an already loaded asset.
    pub fn alias(&mut self, ptr: Ptr) -> Option<usize> {
        let Ptr::Ref { block, offset } = ptr else { return None };
        let found = self.aliases.get(&key(block, offset)).copied();
        if found.is_none() {
            self.unresolved_aliases += 1;
        }
        found
    }
}
