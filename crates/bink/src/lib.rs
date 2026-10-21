//! Bink 1 (`BIK`, revisions up to `i`) video and audio, the format of Call
//! of Duty 4's cinematics (`main/video/*.bik`), decoded in pure Rust.
//! Written from the format's behaviour (as FFmpeg documents it in code);
//! the constant tables are the format's own.

mod audio;
mod bits;
mod tables;
mod video;

use anyhow::{Result, bail};

pub struct AudioTrack {
    pub sample_rate: u32,
    pub channels: usize,
}

pub struct Bink {
    data: Vec<u8>,
    revision: u8,
    pub width: usize,
    pub height: usize,
    /// Frames a second.
    pub fps: f64,
    /// Each frame's (offset, size, keyframe).
    frames: Vec<(usize, usize, bool)>,
    pub tracks: Vec<AudioTrack>,
    video: video::Video,
    audio: Vec<audio::Audio>,
    next: usize,
}

/// One decoded frame: its audio (interleaved, per track), and the picture
/// in [`Bink::rgba`].
pub struct Frame {
    pub index: usize,
    pub audio: Vec<Vec<f32>>,
}

impl Bink {
    pub fn open(data: Vec<u8>) -> Result<Bink> {
        let u32_at = |p: usize| -> Result<u32> {
            Ok(u32::from_le_bytes(data.get(p..p + 4).ok_or_else(|| anyhow::anyhow!("bink header cut short"))?.try_into()?))
        };
        if &data[..3.min(data.len())] != b"BIK" {
            bail!("not a Bink 1 file");
        }
        let revision = data[3];
        if revision > b'i' {
            bail!("Bink revision {} not supported", revision as char);
        }
        let count = u32_at(8)? as usize;
        let width = u32_at(20)? as usize;
        let height = u32_at(24)? as usize;
        let (num, den) = (u32_at(28)?, u32_at(32)?);
        let flags = u32_at(36)?;
        let tracks_n = u32_at(40)? as usize;
        let mut p = 44 + 4 * tracks_n;
        let mut tracks = Vec::new();
        for _ in 0..tracks_n {
            let rate = u16::from_le_bytes([data[p], data[p + 1]]) as u32;
            let fl = u16::from_le_bytes([data[p + 2], data[p + 3]]);
            p += 4;
            tracks.push((rate, fl));
        }
        p += 4 * tracks_n;
        let file_end = (u32_at(4)? as usize + 8).min(data.len());
        let mut frames = Vec::with_capacity(count);
        let mut next = u32_at(p)?;
        for i in 0..count {
            let pos = next;
            next = if i + 1 == count { file_end as u32 } else { u32_at(p + 4 * (i + 1))? };
            let (a, b) = ((pos & !1) as usize, (next & !1) as usize);
            if b <= a || b > data.len() {
                bail!("bad frame index at {i}");
            }
            frames.push((a, b - a, pos & 1 != 0));
        }
        let audio = tracks.iter().map(|&(rate, fl)| audio::Audio::new(rate, if fl & 0x2000 != 0 { 2 } else { 1 }, fl & 0x1000 != 0)).collect();
        let tracks = tracks.iter().map(|&(rate, fl)| AudioTrack { sample_rate: rate, channels: if fl & 0x2000 != 0 { 2 } else { 1 } }).collect();
        Ok(Bink {
            revision,
            width,
            height,
            fps: num as f64 / den.max(1) as f64,
            frames,
            tracks,
            video: video::Video::new(width, height, flags & 0x0010_0000 != 0),
            audio,
            next: 0,
            data,
        })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn revision(&self) -> char {
        self.revision as char
    }

    /// Decode the next frame (`None` past the last).
    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        let Some(&(at, size, _)) = self.frames.get(self.next) else { return Ok(None) };
        let index = self.next;
        self.next += 1;
        let data = &self.data[at..at + size];
        let mut p = 0;
        let mut audio = Vec::with_capacity(self.audio.len());
        for a in &mut self.audio {
            let len = u32::from_le_bytes(data.get(p..p + 4).unwrap_or(&[0; 4]).try_into()?) as usize;
            p += 4;
            let mut samples = Vec::new();
            if len >= 4 && p + len <= data.len() {
                a.decode(&data[p..p + len], &mut samples)?;
            }
            p += len;
            audio.push(samples);
        }
        if p < data.len() {
            self.video.decode(&data[p..])?;
        }
        Ok(Some(Frame { index, audio }))
    }

    /// The current picture as RGBA (BT.601), `width * height * 4` bytes.
    pub fn rgba(&self, out: &mut Vec<u8>) {
        let (w, h) = (self.width, self.height);
        out.resize(w * h * 4, 0);
        let [y, u, v] = [&self.video.cur[0], &self.video.cur[1], &self.video.cur[2]];
        for row in 0..h {
            for col in 0..w {
                let yy = y.data[row * y.stride + col] as f32;
                let cb = u.data[(row / 2) * u.stride + col / 2] as f32 - 128.0;
                let cr = v.data[(row / 2) * v.stride + col / 2] as f32 - 128.0;
                let yv = (yy - 16.0) * 1.164;
                let o = (row * w + col) * 4;
                out[o] = (yv + 1.596 * cr).clamp(0.0, 255.0) as u8;
                out[o + 1] = (yv - 0.392 * cb - 0.813 * cr).clamp(0.0, 255.0) as u8;
                out[o + 2] = (yv + 2.017 * cb).clamp(0.0, 255.0) as u8;
                out[o + 3] = 255;
            }
        }
    }
}
