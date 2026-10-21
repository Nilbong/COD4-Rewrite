//! Live tuning: `tuning.txt` next to where the game is started (the project
//! folder for `Play Latest Build.bat`) holds `name = value` lines, re-read
//! within a second of being saved, so look-and-feel numbers can be adjusted
//! while playing with no rebuild. Code reads them with [`get`]:
//!
//! ```ignore
//! let shake = crate::tune::get("bodycam.walk_shake", 1.0);
//! ```
//!
//! A missing file or line means the default. `#` starts a comment. Each
//! knob a feature reads is listed in the file as `name = value  # default
//! value` the first time it's asked for; a line whose value still equals its
//! listed default follows the code's default (so changing a default in code
//! takes effect, and the line is refreshed), any other value overrides it.
//!
//! Over the file sits the map's look ([`look_set`]): the Map Look Editor's
//! values and a map's saved look (`crate::look_editor`), cleared each map.
//! They win over the file; saving them never writes `tuning.txt`.

use bevy::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};
use std::time::SystemTime;

pub struct TunePlugin;

impl Plugin for TunePlugin {
    fn build(&self, app: &mut App) {
        reload();
        app.add_systems(Update, watch);
    }
}

struct Tuning {
    values: BTreeMap<String, f32>,
    /// The map's look (`crate::look_editor`), over `values`.
    look: BTreeMap<String, f32>,
    /// Knobs asked for, with their defaults (for listing in the file).
    known: BTreeMap<String, f32>,
    modified: Option<SystemTime>,
    dirty: bool,
}

static TUNING: LazyLock<RwLock<Tuning>> =
    LazyLock::new(|| RwLock::new(Tuning { values: BTreeMap::new(), look: BTreeMap::new(), known: BTreeMap::new(), modified: None, dirty: false }));

fn path() -> PathBuf {
    std::env::var_os("COD4RW_TUNING").map_or_else(|| PathBuf::from("tuning.txt"), PathBuf::from)
}

/// The value of knob `name`, or `default` when the file doesn't set it.
pub fn get(name: &str, default: f32) -> f32 {
    if let Ok(t) = TUNING.read() {
        // (The editor's A/B shows the map without its look.)
        if !crate::atmos::climate::original() {
            if let Some(v) = t.look.get(name) {
                return *v;
            }
        }
        if let Some(v) = t.values.get(name) {
            return *v;
        }
        if t.known.contains_key(name) {
            return default;
        }
    }
    if let Ok(mut t) = TUNING.write() {
        t.known.insert(name.to_owned(), default);
        t.dirty = true;
    }
    default
}

fn reload() {
    let path = path();
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let Ok(mut t) = TUNING.write() else { return };
    if modified == t.modified {
        return;
    }
    t.modified = modified;
    t.values.clear();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    for line in text.lines() {
        // A line still at the default it was listed with isn't a choice: the
        // code's default applies (and may since have changed).
        if let Some((k, v, untouched)) = parse(line) {
            if !untouched {
                t.values.insert(k.to_owned(), v);
            }
        }
    }
    info!("tuning: {} values from {}", t.values.len(), path.display());
}

/// A `name = value  # default <d>` line: its name, value and whether the
/// value is still the listed default.
fn parse(line: &str) -> Option<(&str, f32, bool)> {
    let (set, comment) = line.split_once('#').unwrap_or((line, ""));
    let (k, v) = set.split_once('=')?;
    let v = v.trim().parse::<f32>().ok()?;
    let listed = comment.trim().strip_prefix("default").and_then(|d| d.trim().parse::<f32>().ok());
    Some((k.trim(), v, listed == Some(v)))
}

/// Re-read the file when it changes; list newly asked-for knobs in it, and
/// refresh listed defaults the code has since changed.
fn watch(time: Res<Time<Real>>, mut next: Local<f32>) {
    let now = time.elapsed_secs();
    if now < *next {
        return;
    }
    *next = now + 1.0;
    reload();
    // Debug runs (tests) don't write the file.
    if crate::audio::audible() == 0.0 {
        return;
    }
    let (current, missing): (String, Vec<(String, f32)>) = {
        let Ok(mut t) = TUNING.write() else { return };
        if !t.dirty {
            return;
        }
        t.dirty = false;
        let text = std::fs::read_to_string(path()).unwrap_or_default();
        let missing = t
            .known
            .iter()
            .filter(|(k, v)| {
                // Listed as a choice, or listed with today's default: leave it.
                !text.lines().filter_map(parse).any(|(n, val, untouched)| n == k.as_str() && (!untouched || val == **v))
            })
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        (text, missing)
    };
    if missing.is_empty() {
        return;
    }
    // Drop the stale lines for those knobs (old defaults, and the old
    // commented-out "# name = value" listing) so they're listed afresh below.
    let stale = |l: &str| {
        let body = l.trim_start().trim_start_matches('#');
        missing.iter().any(|(n, _)| body.split('=').next().is_some_and(|k| k.trim() == n.as_str()))
    };
    let mut text = if current.trim().is_empty() {
        "# Live tuning: change a number, save, and the game picks it up within a second.\n\
         # A line still showing its default follows the game's default; delete a line\n\
         # (or put its default back) to undo a change.\n"
            .to_owned()
    } else {
        current.lines().filter(|l| !stale(l)).map(|l| format!("{l}\n")).collect()
    };
    for (k, v) in missing {
        text.push_str(&format!("{k} = {v}   # default {v}\n"));
    }
    if std::fs::write(path(), text).is_ok() {
        if let Ok(mut t) = TUNING.write() {
            t.modified = std::fs::metadata(path()).and_then(|m| m.modified()).ok();
        }
    }
}

/// Sets knob `name` in the map's look (over `tuning.txt`), `None` unsets it.
pub fn look_set(name: &str, value: Option<f32>) {
    if let Ok(mut t) = TUNING.write() {
        match value.filter(|v| v.is_finite()) {
            Some(v) => {
                t.look.insert(name.to_owned(), v);
            }
            None => {
                t.look.remove(name);
            }
        }
    }
}

/// The map's look as set ([`look_set`]).
pub fn look_all() -> BTreeMap<String, f32> {
    TUNING.read().map(|t| t.look.clone()).unwrap_or_default()
}

/// Replaces the whole map's look (a map's saved look, or nothing).
pub fn look_replace(values: BTreeMap<String, f32>) {
    if let Ok(mut t) = TUNING.write() {
        t.look = values.into_iter().filter(|(_, v)| v.is_finite()).collect();
    }
}

/// The value knob `name` has without the map's look: `tuning.txt`'s, else
/// the code's default as last asked for, else `fallback`.
pub fn base(name: &str, fallback: f32) -> f32 {
    TUNING.read().ok().and_then(|t| t.values.get(name).or(t.known.get(name)).copied()).unwrap_or(fallback)
}

/// Whether the map's look sets knob `name` (cheap: no copy).
pub fn look_has(name: &str) -> bool {
    !crate::atmos::climate::original() && TUNING.read().is_ok_and(|t| t.look.contains_key(name))
}
