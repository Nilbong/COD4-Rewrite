//! Bink audio: blocks of transform coefficients (a DCT-III per channel, or
//! one real inverse DFT over interleaved channels), quantised by critical
//! band, each block cross-faded into the last over its first sixteenth.

use crate::bits::Bits;
use anyhow::{Result, bail};

const CRITICAL_FREQS: [u32; 25] = [100, 200, 300, 400, 510, 630, 770, 920, 1080, 1270, 1480, 1720, 2000, 2320, 2700, 3150, 3700, 4400, 5300, 6400, 7700, 9500, 12000, 15500, 24500];
const RLE: [usize; 16] = [2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 32, 64];

pub struct Audio {
    pub sample_rate: u32,
    pub channels: usize,
    dct: bool,
    /// Channels coded in each block (1 when interleaved for the DFT).
    coded: usize,
    frame_len: usize,
    overlap: usize,
    bands: Vec<usize>,
    quant: [f32; 96],
    previous: Vec<Vec<f32>>,
    first: bool,
    fft: Fft,
}

impl Audio {
    pub fn new(sample_rate: u32, channels: usize, dct: bool) -> Audio {
        let mut bits = if sample_rate < 22050 {
            9
        } else if sample_rate < 44100 {
            10
        } else {
            11
        };
        let (mut rate, mut coded) = (sample_rate, channels);
        if !dct {
            // Interleaved: one channel at the summed rate.
            rate *= channels as u32;
            coded = 1;
            bits += (usize::BITS - 1 - channels.leading_zeros()) as usize;
        }
        let frame_len = 1usize << bits;
        let root = if dct { frame_len as f32 / ((frame_len as f32).sqrt() * 32768.0) } else { 2.0 / ((frame_len as f32).sqrt() * 32768.0) };
        let quant = std::array::from_fn(|i| (i as f32 * 0.152_891_65).exp() * root);
        let half = rate.div_ceil(2);
        let mut num_bands = 1;
        while num_bands < 25 && half > CRITICAL_FREQS[num_bands - 1] {
            num_bands += 1;
        }
        let mut bands = vec![2usize];
        for i in 1..num_bands {
            bands.push(((CRITICAL_FREQS[i - 1] as usize * frame_len / half as usize) & !1).max(0));
        }
        bands.push(frame_len);
        Audio {
            sample_rate,
            channels,
            dct,
            coded,
            frame_len,
            overlap: frame_len / 16,
            bands,
            quant,
            previous: vec![vec![0.0; frame_len / 16]; coded],
            first: true,
            fft: Fft::new(if dct { frame_len * 2 } else { frame_len }),
        }
    }

    /// A frame's audio packet to interleaved samples (-1..1).
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<()> {
        if packet.len() < 4 {
            return Ok(());
        }
        let mut b = Bits::new(packet);
        b.skip(32);
        while b.left() > 0 {
            let block = self.block(&mut b)?;
            let keep = self.frame_len - self.overlap;
            if self.dct {
                for i in 0..keep {
                    for ch in &block {
                        out.push(ch[i]);
                    }
                }
            } else {
                out.extend_from_slice(&block[0][..keep]);
            }
            b.align32();
        }
        Ok(())
    }

    fn get_float(b: &mut Bits) -> f32 {
        let power = b.read(5) as i32;
        let f = b.read(23) as f32 * 2f32.powi(power - 23);
        if b.bit() { -f } else { f }
    }

    fn block(&mut self, b: &mut Bits) -> Result<Vec<Vec<f32>>> {
        let n = self.frame_len;
        if self.dct {
            b.skip(2);
        }
        let mut out = Vec::with_capacity(self.coded);
        for _ in 0..self.coded {
            if b.left() < 58 {
                bail!("audio block cut short");
            }
            let mut c = vec![0f32; n + 2];
            c[0] = Self::get_float(b) * self.quant_root();
            c[1] = Self::get_float(b) * self.quant_root();
            let nb = self.bands.len() - 1;
            let quant: Vec<f32> = (0..nb).map(|_| self.quant[(b.read(8) as usize).min(95)]).collect();
            let (mut k, mut q) = (0usize, quant[0]);
            let mut i = 2usize;
            while i < n {
                let j = if b.bit() { i + RLE[b.read(4) as usize] * 8 } else { i + 8 }.min(n);
                let width = b.read(4);
                if width == 0 {
                    c[i..j].fill(0.0);
                    i = j;
                    while self.bands[k] < i {
                        q = quant[k];
                        k += 1;
                    }
                } else {
                    while i < j {
                        if self.bands[k] == i {
                            q = quant[k];
                            k += 1;
                        }
                        let v = b.read(width);
                        c[i] = if v == 0 {
                            0.0
                        } else if b.bit() {
                            -q * v as f32
                        } else {
                            q * v as f32
                        };
                        i += 1;
                    }
                }
            }
            out.push(if self.dct { self.inverse_dct(&c[..n]) } else { self.inverse_dft(&c[..n]) });
        }
        // Into the last block over the overlap.
        let count = (self.overlap * self.coded) as f32;
        for (ch, samples) in out.iter_mut().enumerate() {
            if !self.first {
                let mut j = ch;
                for i in 0..self.overlap {
                    samples[i] = (self.previous[ch][i] * (count - j as f32) + samples[i] * j as f32) / count;
                    j += self.coded;
                }
            }
            self.previous[ch].copy_from_slice(&samples[n - self.overlap..n]);
        }
        self.first = false;
        Ok(out)
    }

    /// The coefficients already carry the quantisation root; the first two
    /// floats are raw values scaled by it too.
    fn quant_root(&self) -> f32 {
        let n = self.frame_len as f32;
        if self.dct { n / (n.sqrt() * 32768.0) } else { 2.0 / (n.sqrt() * 32768.0) }
    }

    /// DCT-III of `c` (the DC counted in full), scaled by 1/N.
    fn inverse_dct(&mut self, c: &[f32]) -> Vec<f32> {
        let n = c.len();
        // Through a 2N-point inverse DFT: x[k] = Re(Σ c[j] e^{iπj(2k+1)/2N}).
        let mut buf = vec![(0f32, 0f32); 2 * n];
        for (j, &v) in c.iter().enumerate() {
            let w = if j == 0 { v } else { v };
            let a = std::f32::consts::PI * j as f32 / (2 * n) as f32;
            buf[j] = (w * a.cos(), w * a.sin());
        }
        self.fft.run(&mut buf, true);
        let scale = 1.0 / n as f32;
        (0..n).map(|k| buf[k].0 * scale).collect()
    }

    /// The real inverse DFT of `c` (c[0] DC, c[1] Nyquist, then re/im
    /// pairs), as Bink's encoder made it.
    fn inverse_dft(&mut self, c: &[f32]) -> Vec<f32> {
        let n = c.len();
        let mut buf = vec![(0f32, 0f32); n];
        buf[0] = (c[0], 0.0);
        buf[n / 2] = (c[1], 0.0);
        for k in 1..n / 2 {
            let (re, im) = (c[2 * k], c[2 * k + 1]);
            buf[k] = (re, im);
            buf[n - k] = (re, -im);
        }
        self.fft.run(&mut buf, true);
        (0..n).map(|k| buf[k].0 * 0.5).collect()
    }
}

/// A radix-2 complex FFT of a fixed size.
struct Fft {
    n: usize,
    twiddle: Vec<(f32, f32)>,
}

impl Fft {
    fn new(n: usize) -> Fft {
        let twiddle = (0..n / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Fft { n, twiddle }
    }

    /// In place; `inverse` uses e^{+i}, unscaled.
    fn run(&self, x: &mut [(f32, f32)], inverse: bool) {
        let n = self.n;
        let mut j = 0;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                x.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = self.twiddle[k * step];
                    let wi = if inverse { -wi } else { wi };
                    let (ar, ai) = x[start + k];
                    let (br, bi) = x[start + k + len / 2];
                    let (tr, ti) = (br * wr - bi * wi, br * wi + bi * wr);
                    x[start + k] = (ar + tr, ai + ti);
                    x[start + k + len / 2] = (ar - tr, ai - ti);
                }
            }
            len <<= 1;
        }
    }
}
