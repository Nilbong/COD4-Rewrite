//! Bink 1 video: each plane is cut into 8x8 blocks (16x16 for the "scaled"
//! kind), coded with nine bundles of values read a block row at a time:
//! block types, colours, patterns, motion, DC values and runs.

use crate::bits::Bits;
use crate::tables;
use anyhow::{Result, bail};

/// The bundles (sources of values).
const BLOCK_TYPES: usize = 0;
const SUB_BLOCK_TYPES: usize = 1;
const COLORS: usize = 2;
const PATTERN: usize = 3;
const X_OFF: usize = 4;
const Y_OFF: usize = 5;
const INTRA_DC: usize = 6;
const INTER_DC: usize = 7;
const RUN: usize = 8;
const SOURCES: usize = 9;

// Block types.
const SKIP: i32 = 0;
const SCALED: i32 = 1;
const MOTION: i32 = 2;
const RUNS: i32 = 3;
const RESIDUE: i32 = 4;
const INTRA: i32 = 5;
const FILL: i32 = 6;
const INTER: i32 = 7;
const PATTERNED: i32 = 8;
const RAW: i32 = 9;

/// A Huffman tree: one of the 16 fixed ones, and how its symbols map.
#[derive(Clone, Copy, Default)]
struct Tree {
    vlc: usize,
    syms: [u8; 16],
}

/// A fixed tree as a lookup on its next 7 bits: (symbol index, length).
struct Lookup([(u8, u8); 128]);

#[derive(Default)]
struct Bundle {
    /// Bits in a value count.
    len: u32,
    tree: Tree,
    data: Vec<i32>,
    /// Values decoded so far (`None` once the plane's are all in), and read.
    dec: Option<usize>,
    ptr: usize,
}

pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub data: Vec<u8>,
}

impl Plane {
    fn new(width: usize, height: usize, block: usize) -> Plane {
        // Room for whole blocks (16x16 ones on the right and bottom edges).
        let stride = width.div_ceil(block) * block + 16;
        let rows = height.div_ceil(block) * block + 16;
        Plane { width, height, stride, data: vec![0; stride * rows] }
    }
}

pub struct Video {
    pub width: usize,
    pub height: usize,
    pub has_alpha: bool,
    /// This frame's planes (Y, U, V, alpha) and the last frame's.
    pub cur: Vec<Plane>,
    prev: Vec<Plane>,
    lookups: Vec<Lookup>,
    paths: [[u8; 64]; 16],
    intra_q: Vec<[i32; 64]>,
    inter_q: Vec<[i32; 64]>,
    bundles: Vec<Bundle>,
    col_high: [Tree; 16],
    col_last: i32,
}

impl Video {
    pub fn new(width: usize, height: usize, has_alpha: bool) -> Video {
        let planes = || {
            let mut v = vec![Plane::new(width, height, 8), Plane::new(width / 2, height / 2, 8), Plane::new(width / 2, height / 2, 8)];
            if has_alpha {
                v.push(Plane::new(width, height, 8));
            }
            v
        };
        let lookups = tables::TREE_CODES
            .iter()
            .map(|codes| {
                let lens = tables::tree_lengths(codes);
                let mut t = [(0u8, 1u8); 128];
                for (i, (&c, &l)) in codes.iter().zip(&lens).enumerate() {
                    for x in 0..128usize {
                        if x & ((1 << l) - 1) == c as usize {
                            t[x] = (i as u8, l);
                        }
                    }
                }
                Lookup(t)
            })
            .collect();
        Video {
            width,
            height,
            has_alpha,
            cur: planes(),
            prev: planes(),
            lookups,
            paths: std::array::from_fn(tables::path),
            intra_q: tables::quant(true),
            inter_q: tables::quant(false),
            bundles: (0..SOURCES).map(|_| Bundle::default()).collect(),
            col_high: [Tree::default(); 16],
            col_last: 0,
        }
    }

    /// Decode a frame's video data.
    pub fn decode(&mut self, data: &[u8]) -> Result<()> {
        std::mem::swap(&mut self.cur, &mut self.prev);
        let mut b = Bits::new(data);
        let bits = data.len() * 8;
        if self.has_alpha {
            b.skip(32);
            self.plane(&mut b, 3, false)?;
        }
        b.skip(32);
        // Luma, then the chroma planes stored V first.
        for (k, idx) in [0usize, 2, 1].into_iter().enumerate() {
            self.plane(&mut b, idx, k > 0)?;
            if b.pos() >= bits {
                break;
            }
        }
        Ok(())
    }

    fn huff(&self, b: &mut Bits, t: &Tree) -> i32 {
        let (i, l) = self.lookups[t.vlc].0[b.peek(7) as usize];
        b.skip(l as usize);
        t.syms[i as usize] as i32
    }

    fn read_tree(b: &mut Bits) -> Tree {
        let mut t = Tree { vlc: b.read(4) as usize, syms: std::array::from_fn(|i| i as u8) };
        if t.vlc == 0 {
            return t;
        }
        if b.bit() {
            let mut len = b.read(3) as usize;
            let mut used = [false; 16];
            for i in 0..=len {
                t.syms[i] = b.read(4) as u8;
                used[t.syms[i] as usize] = true;
            }
            for i in 0..16 {
                if len >= 15 {
                    break;
                }
                if !used[i] {
                    len += 1;
                    t.syms[len] = i as u8;
                }
            }
        } else {
            // Merge sorts by the bits.
            let len = b.read(2) as usize;
            let mut a: [u8; 16] = std::array::from_fn(|i| i as u8);
            let mut out = [0u8; 16];
            for i in 0..=len {
                let size = 1 << i;
                for start in (0..16).step_by(size * 2) {
                    let (mut p, mut q) = (start, start + size);
                    let (pe, qe) = (start + size, start + 2 * size);
                    let mut o = start;
                    while p < pe && q < qe {
                        if !b.bit() {
                            out[o] = a[p];
                            p += 1;
                        } else {
                            out[o] = a[q];
                            q += 1;
                        }
                        o += 1;
                    }
                    while p < pe {
                        out[o] = a[p];
                        p += 1;
                        o += 1;
                    }
                    while q < qe {
                        out[o] = a[q];
                        q += 1;
                        o += 1;
                    }
                }
                a = out;
            }
            t.syms = a;
        }
        t
    }

    /// Whether to read this bundle's next block row, and its count.
    fn count(&mut self, b: &mut Bits, k: usize) -> Option<usize> {
        let bu = &mut self.bundles[k];
        let dec = bu.dec?;
        if dec > bu.ptr {
            return None;
        }
        let t = b.read(bu.len) as usize;
        if t == 0 {
            bu.dec = None;
            return None;
        }
        Some(t)
    }

    fn push(&mut self, k: usize, v: i32) {
        let bu = &mut self.bundles[k];
        let dec = bu.dec.unwrap_or(0);
        if bu.data.len() <= dec {
            bu.data.resize(dec + 1, 0);
        }
        bu.data[dec] = v;
        bu.dec = Some(dec + 1);
    }

    fn value(&mut self, k: usize) -> i32 {
        let bu = &mut self.bundles[k];
        let v = bu.data.get(bu.ptr).copied().unwrap_or(0);
        bu.ptr += 1;
        v
    }

    fn read_runs(&mut self, b: &mut Bits) {
        let Some(t) = self.count(b, RUN) else { return };
        if b.bit() {
            let v = b.read(4) as i32;
            for _ in 0..t {
                self.push(RUN, v);
            }
        } else {
            let tree = self.bundles[RUN].tree;
            for _ in 0..t {
                let v = self.huff(b, &tree);
                self.push(RUN, v);
            }
        }
    }

    fn read_motion(&mut self, b: &mut Bits, k: usize) {
        let Some(t) = self.count(b, k) else { return };
        let signed = |b: &mut Bits, v: i32| if v != 0 && b.bit() { -v } else { v };
        if b.bit() {
            let v = b.read(4) as i32;
            let v = signed(b, v);
            for _ in 0..t {
                self.push(k, v);
            }
        } else {
            let tree = self.bundles[k].tree;
            for _ in 0..t {
                let v = self.huff(b, &tree);
                let v = signed(b, v);
                self.push(k, v);
            }
        }
    }

    fn read_block_types(&mut self, b: &mut Bits, k: usize) -> Result<()> {
        const RLE: [usize; 4] = [4, 8, 12, 32];
        let Some(t) = self.count(b, k) else { return Ok(()) };
        if b.bit() {
            let v = b.read(4) as i32;
            for _ in 0..t {
                self.push(k, v);
            }
        } else {
            let tree = self.bundles[k].tree;
            let mut last = 0;
            let mut n = 0;
            while n < t {
                let v = self.huff(b, &tree);
                if v < 12 {
                    last = v;
                    self.push(k, v);
                    n += 1;
                } else {
                    let run = RLE[(v - 12) as usize];
                    if t - n < run {
                        bail!("block type run past the row");
                    }
                    for _ in 0..run {
                        self.push(k, last);
                    }
                    n += run;
                }
            }
        }
        Ok(())
    }

    fn read_patterns(&mut self, b: &mut Bits) {
        let Some(t) = self.count(b, PATTERN) else { return };
        let tree = self.bundles[PATTERN].tree;
        for _ in 0..t {
            let lo = self.huff(b, &tree);
            let hi = self.huff(b, &tree);
            self.push(PATTERN, lo | (hi << 4));
        }
    }

    fn read_colors(&mut self, b: &mut Bits) {
        let Some(t) = self.count(b, COLORS) else { return };
        let tree = self.bundles[COLORS].tree;
        let mut one = |s: &mut Self, b: &mut Bits| {
            let hi = s.col_high[s.col_last as usize];
            s.col_last = s.huff(b, &hi);
            let lo = s.huff(b, &tree);
            (s.col_last << 4) | lo
        };
        if b.bit() {
            let v = one(self, b);
            for _ in 0..t {
                self.push(COLORS, v);
            }
        } else {
            for _ in 0..t {
                let v = one(self, b);
                self.push(COLORS, v);
            }
        }
    }

    fn read_dcs(&mut self, b: &mut Bits, k: usize, signed: bool) -> Result<()> {
        let Some(len) = self.count(b, k) else { return Ok(()) };
        let sign = |b: &mut Bits, v: i32| if v != 0 && b.bit() { -v } else { v };
        let mut v = b.read(11 - signed as u32) as i32;
        if signed {
            v = sign(b, v);
        }
        self.push(k, v);
        let len = len - 1;
        let mut i = 0;
        while i < len {
            let n = (len - i).min(8);
            let bsize = b.read(4);
            for _ in 0..n {
                if bsize != 0 {
                    let d = b.read(bsize) as i32;
                    v += sign(b, d);
                    if !(-32768..=32767).contains(&v) {
                        bail!("DC out of range");
                    }
                }
                self.push(k, v);
            }
            i += 8;
        }
        Ok(())
    }

    /// The DCT coefficients' bits: fills `block` (by position) and returns
    /// the coded coefficients' indices and the quantiser.
    fn read_dct(b: &mut Bits, block: &mut [i32; 64], idx: &mut Vec<usize>) -> Result<usize> {
        let mut coef = [0i32; 128];
        let mut mode = [0u8; 128];
        let (mut start, mut end) = (64usize, 64usize);
        for (c, m) in [(4, 0), (24, 0), (44, 0), (1, 3), (2, 3), (3, 3)] {
            coef[end] = c;
            mode[end] = m;
            end += 1;
        }
        let value = |b: &mut Bits, bits: i32| -> i32 {
            if bits == 0 {
                1 - ((b.bit() as i32) << 1)
            } else {
                let t = b.read(bits as u32) as i32 | (1 << bits);
                if b.bit() { -t } else { t }
            }
        };
        let mut bits = b.read(4) as i32 - 1;
        while bits >= 0 {
            let mut pos = start;
            while pos < end {
                if (mode[pos] == 0 && coef[pos] == 0) || !b.bit() {
                    pos += 1;
                    continue;
                }
                let mut c = coef[pos];
                let m = mode[pos];
                match m {
                    0 | 2 => {
                        if m == 0 {
                            coef[pos] = c + 4;
                            mode[pos] = 1;
                        } else {
                            coef[pos] = 0;
                            mode[pos] = 0;
                            pos += 1;
                        }
                        for _ in 0..4 {
                            if b.bit() {
                                start -= 1;
                                coef[start] = c;
                                mode[start] = 3;
                            } else {
                                if c >= 64 {
                                    bail!("coefficient past the block");
                                }
                                block[tables::SCAN[c as usize] as usize] = value(b, bits);
                                idx.push(c as usize);
                            }
                            c += 1;
                        }
                    }
                    1 => {
                        mode[pos] = 2;
                        for _ in 0..3 {
                            c += 4;
                            coef[end] = c;
                            mode[end] = 2;
                            end += 1;
                        }
                    }
                    _ => {
                        if c >= 64 {
                            bail!("coefficient past the block");
                        }
                        block[tables::SCAN[c as usize] as usize] = value(b, bits);
                        idx.push(c as usize);
                        coef[pos] = 0;
                        mode[pos] = 0;
                        pos += 1;
                    }
                }
            }
            bits -= 1;
        }
        Ok(b.read(4) as usize)
    }

    fn read_residue(b: &mut Bits, block: &mut [i32; 64], mut masks: i32) {
        let mut coef = [0i32; 128];
        let mut mode = [0u8; 128];
        let (mut start, mut end) = (64usize, 64usize);
        for (c, m) in [(4, 0), (24, 0), (44, 0), (0, 2)] {
            coef[end] = c;
            mode[end] = m;
            end += 1;
        }
        let mut nz: Vec<usize> = Vec::with_capacity(64);
        let mut mask = 1i32 << b.read(3);
        while mask != 0 {
            for &p in &nz {
                if !b.bit() {
                    continue;
                }
                if block[p] < 0 {
                    block[p] -= mask;
                } else {
                    block[p] += mask;
                }
                masks -= 1;
                if masks < 0 {
                    return;
                }
            }
            let mut pos = start;
            while pos < end {
                if (coef[pos] == 0 && mode[pos] == 0) || !b.bit() {
                    pos += 1;
                    continue;
                }
                let mut c = coef[pos];
                let m = mode[pos];
                match m {
                    0 | 2 => {
                        if m == 0 {
                            coef[pos] = c + 4;
                            mode[pos] = 1;
                        } else {
                            coef[pos] = 0;
                            mode[pos] = 0;
                            pos += 1;
                        }
                        for _ in 0..4 {
                            if b.bit() {
                                start -= 1;
                                coef[start] = c;
                                mode[start] = 3;
                            } else {
                                let p = tables::SCAN[(c as usize).min(63)] as usize;
                                nz.push(p);
                                block[p] = if b.bit() { -mask } else { mask };
                                masks -= 1;
                                if masks < 0 {
                                    return;
                                }
                            }
                            c += 1;
                        }
                    }
                    1 => {
                        mode[pos] = 2;
                        for _ in 0..3 {
                            c += 4;
                            coef[end] = c;
                            mode[end] = 2;
                            end += 1;
                        }
                    }
                    _ => {
                        let p = tables::SCAN[(c as usize).min(63)] as usize;
                        nz.push(p);
                        block[p] = if b.bit() { -mask } else { mask };
                        coef[pos] = 0;
                        mode[pos] = 0;
                        pos += 1;
                        masks -= 1;
                        if masks < 0 {
                            return;
                        }
                    }
                }
            }
            mask >>= 1;
        }
    }

    fn plane(&mut self, b: &mut Bits, idx: usize, chroma: bool) -> Result<()> {
        let (bw, bh) = if chroma { ((self.width + 15) >> 4, (self.height + 15) >> 4) } else { ((self.width + 7) >> 3, (self.height + 7) >> 3) };
        let width = (self.width >> chroma as usize).max(8).div_ceil(8) * 8;
        let log2 = |x: usize| usize::BITS - 1 - x.leading_zeros();
        let l = |x: usize| log2(x + 511) + 1;
        self.bundles[BLOCK_TYPES].len = l(width >> 3);
        self.bundles[SUB_BLOCK_TYPES].len = l(width >> 4);
        self.bundles[COLORS].len = l(bw * 64);
        for k in [INTRA_DC, INTER_DC, X_OFF, Y_OFF] {
            self.bundles[k].len = l(width >> 3);
        }
        self.bundles[PATTERN].len = l(bw << 3);
        self.bundles[RUN].len = l(bw * 48);
        for k in 0..SOURCES {
            if k == COLORS {
                for i in 0..16 {
                    self.col_high[i] = Self::read_tree(b);
                }
                self.col_last = 0;
            }
            if k != INTRA_DC && k != INTER_DC {
                self.bundles[k].tree = Self::read_tree(b);
            }
            let bu = &mut self.bundles[k];
            bu.dec = Some(0);
            bu.ptr = 0;
        }
        let stride = self.cur[idx].stride;
        let mut block = [0i32; 64];
        let mut ublock = [0u8; 64];
        let mut coefs: Vec<usize> = Vec::with_capacity(64);
        for by in 0..bh {
            self.read_block_types(b, BLOCK_TYPES)?;
            self.read_block_types(b, SUB_BLOCK_TYPES)?;
            self.read_colors(b);
            self.read_patterns(b);
            self.read_motion(b, X_OFF);
            self.read_motion(b, Y_OFF);
            self.read_dcs(b, INTRA_DC, false)?;
            self.read_dcs(b, INTER_DC, true)?;
            self.read_runs(b);
            let mut bx = 0;
            while bx < bw {
                let at = by * 8 * stride + bx * 8;
                let blk = self.value(BLOCK_TYPES);
                // A 16x16 block's other quarters were decoded with it.
                if ((by & 1) != 0 || (bx & 1) != 0) && blk == SCALED {
                    bx += 2;
                    continue;
                }
                match blk {
                    SKIP => self.copy(idx, at, at),
                    SCALED => {
                        let sub = self.value(SUB_BLOCK_TYPES);
                        match sub {
                            RUNS => {
                                let path = self.paths[b.read(4) as usize];
                                let mut i = 0usize;
                                let mut s = 0usize;
                                loop {
                                    let run = self.value(RUN) as usize + 1;
                                    i += run;
                                    if i > 64 {
                                        bail!("run past the block");
                                    }
                                    if b.bit() {
                                        let v = self.value(COLORS) as u8;
                                        for _ in 0..run {
                                            ublock[path[s] as usize] = v;
                                            s += 1;
                                        }
                                    } else {
                                        for _ in 0..run {
                                            ublock[path[s] as usize] = self.value(COLORS) as u8;
                                            s += 1;
                                        }
                                    }
                                    if i >= 63 {
                                        break;
                                    }
                                }
                                if i == 63 {
                                    ublock[path[s] as usize] = self.value(COLORS) as u8;
                                }
                            }
                            INTRA => {
                                block = [0; 64];
                                block[0] = self.value(INTRA_DC);
                                coefs.clear();
                                let q = Self::read_dct(b, &mut block, &mut coefs)?;
                                dequant(&mut block, &self.intra_q[q.min(15)], &coefs);
                                idct(&mut block);
                                for i in 0..64 {
                                    ublock[i] = block[i] as u8;
                                }
                            }
                            FILL => {
                                let v = self.value(COLORS) as u8;
                                let d = &mut self.cur[idx].data;
                                for y in 0..16 {
                                    d[at + y * stride..at + y * stride + 16].fill(v);
                                }
                            }
                            PATTERNED => {
                                let col = [self.value(COLORS) as u8, self.value(COLORS) as u8];
                                for j in 0..8 {
                                    let mut v = self.value(PATTERN);
                                    for i in 0..8 {
                                        ublock[i + j * 8] = col[(v & 1) as usize];
                                        v >>= 1;
                                    }
                                }
                            }
                            RAW => {
                                for i in 0..64 {
                                    ublock[i] = self.value(COLORS) as u8;
                                }
                            }
                            other => bail!("bad 16x16 block type {other}"),
                        }
                        if sub != FILL {
                            let d = &mut self.cur[idx].data;
                            for j in 0..8 {
                                for i in 0..8 {
                                    let v = ublock[j * 8 + i];
                                    let p = at + 2 * j * stride + 2 * i;
                                    d[p] = v;
                                    d[p + 1] = v;
                                    d[p + stride] = v;
                                    d[p + stride + 1] = v;
                                }
                            }
                        }
                        bx += 2;
                        continue;
                    }
                    MOTION => {
                        let r = self.motion(idx, at, bx, by, bw, bh)?;
                        self.copy(idx, at, r);
                    }
                    RUNS => {
                        let path = self.paths[b.read(4) as usize];
                        let mut i = 0usize;
                        let mut s = 0usize;
                        let d = |p: u8| at + (p as usize & 7) + (p as usize >> 3) * stride;
                        loop {
                            let run = self.value(RUN) as usize + 1;
                            i += run;
                            if i > 64 {
                                bail!("run past the block");
                            }
                            if b.bit() {
                                let v = self.value(COLORS) as u8;
                                for _ in 0..run {
                                    self.cur[idx].data[d(path[s])] = v;
                                    s += 1;
                                }
                            } else {
                                for _ in 0..run {
                                    let v = self.value(COLORS) as u8;
                                    self.cur[idx].data[d(path[s])] = v;
                                    s += 1;
                                }
                            }
                            if i >= 63 {
                                break;
                            }
                        }
                        if i == 63 {
                            let v = self.value(COLORS) as u8;
                            self.cur[idx].data[d(path[s])] = v;
                        }
                    }
                    RESIDUE => {
                        let r = self.motion(idx, at, bx, by, bw, bh)?;
                        self.copy(idx, at, r);
                        block = [0; 64];
                        let masks = b.read(7) as i32;
                        Self::read_residue(b, &mut block, masks);
                        let d = &mut self.cur[idx].data;
                        for j in 0..8 {
                            for i in 0..8 {
                                let p = at + j * stride + i;
                                d[p] = (d[p] as i32 + block[j * 8 + i]) as u8;
                            }
                        }
                    }
                    INTRA => {
                        block = [0; 64];
                        block[0] = self.value(INTRA_DC);
                        coefs.clear();
                        let q = Self::read_dct(b, &mut block, &mut coefs)?;
                        dequant(&mut block, &self.intra_q[q.min(15)], &coefs);
                        idct(&mut block);
                        let d = &mut self.cur[idx].data;
                        for j in 0..8 {
                            for i in 0..8 {
                                d[at + j * stride + i] = block[j * 8 + i] as u8;
                            }
                        }
                    }
                    FILL => {
                        let v = self.value(COLORS) as u8;
                        let d = &mut self.cur[idx].data;
                        for j in 0..8 {
                            d[at + j * stride..at + j * stride + 8].fill(v);
                        }
                    }
                    INTER => {
                        let r = self.motion(idx, at, bx, by, bw, bh)?;
                        self.copy(idx, at, r);
                        block = [0; 64];
                        block[0] = self.value(INTER_DC);
                        coefs.clear();
                        let q = Self::read_dct(b, &mut block, &mut coefs)?;
                        dequant(&mut block, &self.inter_q[q.min(15)], &coefs);
                        idct(&mut block);
                        let d = &mut self.cur[idx].data;
                        for j in 0..8 {
                            for i in 0..8 {
                                let p = at + j * stride + i;
                                d[p] = (d[p] as i32 + block[j * 8 + i]) as u8;
                            }
                        }
                    }
                    PATTERNED => {
                        let col = [self.value(COLORS) as u8, self.value(COLORS) as u8];
                        for j in 0..8 {
                            let mut v = self.value(PATTERN);
                            for i in 0..8 {
                                self.cur[idx].data[at + j * stride + i] = col[(v & 1) as usize];
                                v >>= 1;
                            }
                        }
                    }
                    RAW => {
                        for j in 0..8 {
                            for i in 0..8 {
                                let v = self.value(COLORS) as u8;
                                self.cur[idx].data[at + j * stride + i] = v;
                            }
                        }
                    }
                    other => bail!("bad block type {other}"),
                }
                bx += 1;
            }
        }
        b.align32();
        Ok(())
    }

    /// Where a motion vector points in the last frame (checked against it).
    fn motion(&mut self, idx: usize, at: usize, _bx: usize, _by: usize, bw: usize, bh: usize) -> Result<usize> {
        let x = self.value(X_OFF) as isize;
        let y = self.value(Y_OFF) as isize;
        let stride = self.cur[idx].stride as isize;
        let r = at as isize + x + y * stride;
        let end = ((bh as isize - 1) * stride + bw as isize - 1) * 8;
        if r < 0 || r > end {
            bail!("motion out of the frame ({x}, {y})");
        }
        Ok(r as usize)
    }

    /// An 8x8 block from the last frame at `from` to this one at `to`.
    fn copy(&mut self, idx: usize, to: usize, from: usize) {
        let stride = self.cur[idx].stride;
        let (src, dst) = (&self.prev[idx].data, &mut self.cur[idx].data);
        for j in 0..8 {
            let (s, d) = (from + j * stride, to + j * stride);
            dst[d..d + 8].copy_from_slice(&src[s..s + 8]);
        }
    }
}

fn dequant(block: &mut [i32; 64], q: &[i32; 64], coefs: &[usize]) {
    block[0] = ((block[0] as i64 * q[0] as i64) >> 11) as i32;
    for &i in coefs {
        let p = tables::SCAN[i] as usize;
        block[p] = ((block[p] as i64 * q[i] as i64) >> 11) as i32;
    }
}

/// The 8x8 inverse DCT (AAN-style; its input is pre-scaled by the
/// dequantisers), columns then rows; leaves pixels (unclamped) in `block`.
fn idct(block: &mut [i32; 64]) {
    const A1: i32 = 2896;
    const A2: i32 = 2217;
    const A3: i32 = 3784;
    const A4: i32 = -5352;
    let mul = |a: i32, x: i32| (a.wrapping_mul(x)) >> 11;
    let pass = |s: [i32; 8]| -> [i32; 8] {
        let a0 = s[0] + s[4];
        let a1 = s[0] - s[4];
        let a2 = s[2] + s[6];
        let a3 = mul(A1, s[2] - s[6]);
        let a4 = s[5] + s[3];
        let a5 = s[5] - s[3];
        let a6 = s[1] + s[7];
        let a7 = s[1] - s[7];
        let b0 = a4 + a6;
        let b1 = mul(A3, a5 + a7);
        let b2 = mul(A4, a5) - b0 + b1;
        let b3 = mul(A1, a6 - a4) - b2;
        let b4 = mul(A2, a7) + b3 - b1;
        [a0 + a2 + b0, a1 + a3 - a2 + b2, a1 - a3 + a2 + b3, a0 - a2 - b4, a0 - a2 + b4, a1 - a3 + a2 - b3, a1 + a3 - a2 - b2, a0 + a2 - b0]
    };
    let mut temp = [0i32; 64];
    for c in 0..8 {
        let col: [i32; 8] = std::array::from_fn(|r| block[r * 8 + c]);
        let out = if col[1..].iter().all(|&v| v == 0) { [col[0]; 8] } else { pass(col) };
        for r in 0..8 {
            temp[r * 8 + c] = out[r];
        }
    }
    for r in 0..8 {
        let row: [i32; 8] = std::array::from_fn(|c| temp[r * 8 + c]);
        let out = pass(row);
        for c in 0..8 {
            block[r * 8 + c] = (out[c] + 0x7F) >> 8;
        }
    }
}
