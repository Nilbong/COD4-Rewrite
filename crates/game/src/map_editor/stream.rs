//! Models from CoD4's other levels: an index of every model in the
//! install's level zones (`catalog.txt`, made by the iw3 example
//! `editorindex`), and the zones themselves loaded as they're needed, one
//! at a time, on a thread of their own. A zone stays while it's used (the
//! palette's pictures, a model being placed), then goes; what was taken
//! from it (the placed models, characters, animations) stays.

use crate::characters::Game;
use crate::content::Content;
use crate::wardrobe::{Source, load};
use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;
use std::thread::JoinHandle;

/// One model in the index.
pub struct Entry {
    /// The zones holding it (indices into [`Index::zones`]).
    pub zones: Vec<u16>,
    /// Its box (Bevy space).
    pub min: Vec3,
    pub max: Vec3,
    pub bones: u16,
}

pub struct Index {
    /// Each zone and its size on disk.
    pub zones: Vec<(String, u64)>,
    pub models: HashMap<String, Entry>,
}

static INDEX: LazyLock<Index> = LazyLock::new(|| parse(include_str!("catalog.txt")));

pub fn index() -> &'static Index {
    &INDEX
}

fn parse(text: &str) -> Index {
    let mut zones = Vec::new();
    let mut models = HashMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        match f[..] {
            ["z", name, bytes] => zones.push((name.to_owned(), bytes.parse().unwrap_or(u64::MAX))),
            [name, zs, a, b, c, d, e, g, bones] => {
                let n = |s: &str| s.parse::<f32>().unwrap_or(0.0);
                let (lo, hi) = (crate::units::pos([n(a), n(b), n(c)]), crate::units::pos([n(d), n(e), n(g)]));
                models.insert(
                    name.to_owned(),
                    Entry { zones: zs.split(',').filter_map(|z| z.parse().ok()).collect(), min: lo.min(hi), max: lo.max(hi), bones: bones.parse().unwrap_or(0) },
                );
            }
            _ => {}
        }
    }
    Index { zones, models }
}

impl Index {
    pub fn zone_name(&self, z: u16) -> &str {
        self.zones.get(z as usize).map_or("", |z| z.0.as_str())
    }

    /// Whether `zone` holds `model`.
    pub fn holds(&self, zone: &str, model: &str) -> bool {
        self.models.get(&model.to_ascii_lowercase()).is_some_and(|e| e.zones.iter().any(|&z| self.zone_name(z) == zone))
    }

    /// The smallest zone holding `model`.
    pub fn smallest(&self, model: &str) -> Option<&str> {
        let e = self.models.get(&model.to_ascii_lowercase())?;
        e.zones.iter().min_by_key(|&&z| self.zones.get(z as usize).map_or(u64::MAX, |z| z.1)).map(|&z| self.zone_name(z))
    }
}

/// Free memory (bytes), where it can be told.
pub fn free_ram() -> Option<u64> {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct MemoryStatusEx {
            length: u32,
            load: u32,
            total_phys: u64,
            avail_phys: u64,
            total_page: u64,
            avail_page: u64,
            total_virtual: u64,
            avail_virtual: u64,
            avail_extended: u64,
        }
        unsafe extern "system" {
            fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
        }
        let mut s = MemoryStatusEx { length: std::mem::size_of::<MemoryStatusEx>() as u32, load: 0, total_phys: 0, avail_phys: 0, total_page: 0, avail_page: 0, total_virtual: 0, avail_virtual: 0, avail_extended: 0 };
        // SAFETY: a plain struct of the size given, written by the call.
        (unsafe { GlobalMemoryStatusEx(&mut s) } != 0).then_some(s.avail_phys)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// A zone isn't loaded with less free memory than this.
const MIN_FREE: u64 = 3 << 30;
/// Seconds a zone stays once nothing has used it (editor open, closed).
const LINGER_OPEN: f32 = 45.0;
const LINGER_CLOSED: f32 = 5.0;
/// The most models taken from other zones in a match.
pub const MAX_EXTRA_MODELS: usize = 300;

/// A zone in memory.
pub struct Resident {
    pub zone: String,
    pub content: Content,
    used: f32,
}

#[derive(Resource, Default)]
pub struct Streamer {
    loading: Option<(String, JoinHandle<anyhow::Result<crate::wardrobe::load::Loaded>>, f32)>,
    resident: Option<Resident>,
    queue: VecDeque<String>,
    failed: HashSet<String>,
    /// Counts up as each zone arrives (to try again what was waiting).
    pub generation: u32,
    /// Models prepared from other zones this match.
    pub extra_models: usize,
    /// Loads refused for want of memory (said once).
    pub low_memory: bool,
}

impl Streamer {
    /// Asks for `zone` (no-op if it's here, on its way or failed).
    pub fn want(&mut self, zone: &str) {
        if self.resident.as_ref().is_some_and(|r| r.zone == zone)
            || self.loading.as_ref().is_some_and(|l| l.0 == zone)
            || self.failed.contains(zone)
            || self.queue.iter().any(|z| z == zone)
        {
            return;
        }
        self.queue.push_back(zone.to_owned());
    }

    /// The zone to ask for to get `model`: the one here or coming if it
    /// holds it, else the smallest.
    pub fn zone_for(&self, model: &str) -> Option<String> {
        let idx = index();
        let here = self.resident.as_ref().map(|r| r.zone.as_str()).into_iter().chain(self.loading.as_ref().map(|l| l.0.as_str())).chain(self.queue.iter().map(String::as_str));
        for z in here {
            if idx.holds(z, model) {
                return Some(z.to_owned());
            }
        }
        idx.smallest(model).map(str::to_owned)
    }

    /// `zone`'s content, if it's here (and marks it used).
    pub fn content(&mut self, zone: &str, now: f32) -> Option<&mut Content> {
        let r = self.resident.as_mut().filter(|r| r.zone == zone)?;
        r.used = now;
        Some(&mut r.content)
    }

    /// The zone here, if it holds `model`.
    pub fn holding(&mut self, model: &str, now: f32) -> Option<&mut Content> {
        let zone = self.resident.as_ref().filter(|r| index().holds(&r.zone, model))?.zone.clone();
        self.content(&zone, now)
    }

    /// Whether `zone` couldn't be loaded.
    pub fn failed(&self, zone: &str) -> bool {
        self.failed.contains(zone)
    }

    pub fn busy(&self) -> bool {
        self.loading.is_some() || !self.queue.is_empty()
    }

    /// What's loading, for the status line.
    pub fn loading_zone(&self) -> Option<&str> {
        self.loading.as_ref().map(|l| l.0.as_str())
    }

    pub fn reset(&mut self) {
        *self = Streamer::default();
    }
}

/// Zones in and out.
pub fn step(mut s: ResMut<Streamer>, content: Res<Content>, time: Res<Time<Real>>) {
    let now = time.elapsed_secs();
    if let Some((_, h, _)) = &s.loading
        && h.is_finished()
    {
        let (zone, h, t0) = s.loading.take().unwrap();
        match h.join() {
            Ok(Ok((zones, vfs))) => {
                info!("map editor: zone {zone} in ({:.1} s)", now - t0);
                s.resident = Some(Resident { zone, content: Content::new(zones, vfs.unwrap_or_else(|| content.vfs.clone())), used: now });
                s.generation += 1;
            }
            _ => {
                warn!("map editor: zone {zone} didn't load");
                s.failed.insert(zone);
            }
        }
    }
    // Gone once unused a while (sooner with the editor shut), or when
    // another's wanted.
    let linger = if super::open() { LINGER_OPEN } else { LINGER_CLOSED };
    if s.resident.as_ref().is_some_and(|r| now - r.used > linger || (!s.queue.is_empty() && now - r.used > 1.0)) {
        let r = s.resident.take().unwrap();
        info!("map editor: zone {} dropped", r.zone);
    }
    if s.loading.is_none()
        && s.resident.is_none()
        && let Some(zone) = s.queue.front().cloned()
    {
        if free_ram().is_some_and(|f| f < MIN_FREE) {
            if !s.low_memory {
                warn!("map editor: not loading {zone}: under {} GB free", MIN_FREE >> 30);
                s.low_memory = true;
            }
            return;
        }
        s.queue.pop_front();
        let name = intern(&zone);
        s.loading = Some((zone, load(Source::zone(Game::Cod4, name)), now));
    }
}

/// A zone's name for the loader (which keeps `'static` names): the
/// index's own copy, leaked once.
fn intern(zone: &str) -> &'static str {
    static NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| index().zones.iter().map(|z| &*Box::leak(z.0.clone().into_boxed_str())).collect());
    NAMES.iter().copied().find(|n| *n == zone).unwrap_or("")
}
