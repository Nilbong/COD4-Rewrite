//! Player stats (CoD4's `mpdata`): the custom classes and unlocks that the
//! menus read with `stat()` and write with `statset`, kept between runs in
//! a small text file.
//!
//! Custom class N lives in stats `200 + 10N + 1..9`: primary weapon,
//! attachment, secondary, its attachment, three perks, special grenade and
//! camo. Weapon `i` keeps its unlock flags in stat `3000 + i` (bit 0
//! unlocked, then attachment and camo bits; 65536 marks it "new").

use super::assets::UiAssets;
use bevy::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;

/// Unlocked, with native attachments and ordinary camos (bits from
/// `mp/attachmenttable.csv`, column 10). Mastery camos are refreshed by
/// `mastery` after loading saved progress and the unlock setting.
const WEAPON_UNLOCKED: i32 = 1 | 2 | 4 | 8 | 16 | 32 | 256 | 512 | 1024 | 2048 | 4096;
/// Black Ops' and World at War's attachment bits (see `attachments`).
const OTHER_GAME_ATTACHMENTS: i32 = ((1 << 28) - (1 << 6)) & !65536;

pub struct Stats {
    values: HashMap<i32, i32>,
    /// Saved dvars: the custom class names.
    pub dvars: HashMap<String, String>,
    path: Option<PathBuf>,
    dirty: bool,
}

fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("cod4rw"))
}

impl Stats {
    #[cfg(test)]
    pub(super) fn in_memory() -> Self {
        Self { values: HashMap::new(), dvars: HashMap::new(), path: None, dirty: false }
    }

    /// Defaults (every class from `mp/classtable.csv`, everything unlocked),
    /// overridden by the saved file. Debug runs (`persist: false`) neither
    /// read nor write it.
    ///
    /// Unlocks are the player's choice ([`super::progression::UNLOCKS_DVAR`],
    /// or `COD4RW_UNLOCKS=all|cod4`, which is saved): everything, or CoD4's
    /// by rank and challenges. A new profile starts with CoD4's; one saved
    /// before there was a choice (and debug runs) with everything.
    pub fn load(assets: &UiAssets, persist: bool) -> Stats {
        let mut values = HashMap::from([(260, 1)]);
        let mut unlocked = HashMap::new();
        let num = |s: Option<&str>| s.and_then(|s| s.trim().parse::<i32>().ok());
        if let Some(t) = assets.table("mp/classtable.csv") {
            for r in 0..t.rows {
                if let (Some(stat), Some(v)) = (num(t.get(r, 1)), num(t.get(r, 5))) {
                    values.insert(stat, v);
                }
            }
        }
        if let Some(t) = assets.table("mp/statstable.csv") {
            for r in 0..t.rows {
                let named = t.get(r, 3).is_some_and(|n| !n.is_empty());
                let Some(stat) = num(t.get(r, 1)).filter(|_| named) else { continue };
                // Black Ops' and World at War's guns: every attachment too
                // (bits past CoD4's; 65536 stays the "new" mark).
                let other_game = if stat >= 3000 + crate::bo1::FIRST_INDEX { OTHER_GAME_ATTACHMENTS } else { 0 };
                let v = if stat >= 3000 { WEAPON_UNLOCKED | other_game } else { 1 };
                values.insert(stat, v);
                unlocked.insert(stat, v);
            }
        }
        let path = data_dir().map(|d| d.join("stats.txt")).filter(|_| persist);
        // Debug runs can read (never write) stats from `COD4RW_STATSFILE`.
        let read_from = path.clone().or_else(|| std::env::var_os("COD4RW_STATSFILE").map(PathBuf::from));
        let mut dvars = HashMap::new();
        let text = read_from.and_then(|p| std::fs::read_to_string(p).ok());
        let saved = text.is_some();
        if let Some(text) = text {
            for line in text.lines() {
                if let Some((name, value)) = line.strip_prefix("dvar ").and_then(|r| r.split_once(' ')) {
                    dvars.insert(name.to_owned(), value.to_owned());
                    continue;
                }
                let mut parts = line.split_whitespace();
                if let (Some(Ok(k)), Some(Ok(v))) = (parts.next().map(str::parse), parts.next().map(str::parse)) {
                    values.insert(k, v);
                }
            }
        }
        let mut stats = Stats { values, dvars, path, dirty: false };
        let dvar = super::progression::UNLOCKS_DVAR;
        let chosen = std::env::var("COD4RW_UNLOCKS").ok().filter(|v| v == "all" || v == "cod4");
        let mode = chosen.or_else(|| stats.dvars.get(dvar).cloned()).unwrap_or_else(|| {
            if saved || !persist { "all" } else { "cod4" }.to_owned()
        });
        stats.set_dvar(dvar, &mode);
        if mode != "cod4" {
            // Everything (keeping the menus' "new" marks).
            for (k, v) in unlocked {
                stats.set(k, v | (stats.get(k) & 65536));
            }
        }
        super::progression::refresh_unlocks(&mut stats, assets);
        info!("ui: unlocks: {mode}");
        stats
    }

    pub fn get(&self, index: i32) -> i32 {
        self.values.get(&index).copied().unwrap_or(0)
    }

    pub fn add(&mut self, index: i32, delta: i32) {
        self.set(index, self.get(index) + delta);
    }

    pub fn set(&mut self, index: i32, value: i32) {
        if self.values.insert(index, value) != Some(value) {
            self.dirty = true;
        }
    }

    /// Is this dvar saved with the stats?
    pub fn keeps(name: &str) -> bool {
        name.strip_prefix("customclass").is_some_and(|n| n.parse::<u8>().is_ok())
            || name == super::progression::UNLOCKS_DVAR
            || name == super::scope::SCOPE_STYLE_DVAR
            || name == super::options::FILM_TINT_DVAR
            || name == super::options::LIGHTING_DVAR
            || crate::settings::is_setting(name)
            || super::combat_record::keeps_dvar(name)
            || super::custom_camo::keeps_dvar(name)
    }

    pub fn set_dvar(&mut self, name: &str, value: &str) {
        if self.dvars.get(name).map(String::as_str) != Some(value) {
            self.dvars.insert(name.to_owned(), value.to_owned());
            self.dirty = true;
        }
    }

    pub fn save_if_changed(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(path) = &self.path else { return };
        let mut keys: Vec<_> = self.values.keys().copied().collect();
        keys.sort_unstable();
        let mut text: String = keys.iter().map(|k| format!("{k} {}\n", self.values[k])).collect();
        let mut names: Vec<_> = self.dvars.iter().collect();
        names.sort();
        for (k, v) in names {
            text += &format!("dvar {k} {v}\n");
        }
        let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(path, text));
        if let Err(e) = saved {
            warn!("ui: can't save stats to {}: {e}", path.display());
        }
    }
}
