//! Locating a CoD4 installation.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Install {
    pub root: PathBuf,
}

impl Install {
    /// The install `gamepaths` finds: its environment variable, the
    /// player's chosen folder (Game Folders), Steam's libraries, then the
    /// usual places.
    pub fn locate() -> Result<Install> {
        let game = gamepaths::Game::Cod4;
        match gamepaths::find(game) {
            Some(root) => Ok(Install { root }),
            None => bail!("could not find a CoD4 install; pick its folder (the one with {}) in the launcher's or Options' Game Folders, or set {}", game.exe(), game.env_var()),
        }
    }

    pub fn at(root: impl Into<PathBuf>) -> Result<Install> {
        let root = root.into();
        if !Self::looks_valid(&root) {
            bail!("{} does not look like a CoD4 install", root.display());
        }
        Ok(Install { root })
    }

    fn looks_valid(root: &Path) -> bool {
        root.join("main").is_dir() && root.join("zone").is_dir()
    }

    pub fn main_dir(&self) -> PathBuf {
        self.root.join("main")
    }

    /// Path of a zone, e.g. `zone_path("mp_killhouse")`: the game's own,
    /// else a custom map's (`usermaps/<map>/<map>.ff`, and its `_load` zone
    /// beside it).
    pub fn zone_path(&self, name: &str) -> PathBuf {
        let own = self.root.join("zone").join("english").join(format!("{name}.ff"));
        if own.is_file() {
            return own;
        }
        let dir = name.strip_suffix("_load").unwrap_or(name);
        let custom = self.root.join("usermaps").join(dir).join(format!("{name}.ff"));
        if custom.is_file() { custom } else { own }
    }

    /// Custom maps installed in `usermaps/` (folders holding `<name>.ff`),
    /// sorted by name.
    pub fn usermaps(&self) -> Vec<String> {
        let Ok(dir) = std::fs::read_dir(self.root.join("usermaps")) else { return Vec::new() };
        let mut out: Vec<String> = dir
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| self.root.join("usermaps").join(name).join(format!("{name}.ff")).is_file())
            .collect();
        out.sort();
        out
    }

    /// A custom map's own archives (its textures, sounds and load screen),
    /// none for the game's maps.
    pub fn usermap_iwds(&self, map: &str) -> Vec<PathBuf> {
        let Ok(dir) = std::fs::read_dir(self.root.join("usermaps").join(map)) else { return Vec::new() };
        let mut out: Vec<PathBuf> = dir
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iwd")))
            .collect();
        out.sort();
        out
    }

    /// The archives a match on `map` reads: the game's, then the map's own
    /// (which override them).
    pub fn map_iwd_paths(&self, map: &str) -> Result<Vec<PathBuf>> {
        let mut out = self.iwd_paths()?;
        out.extend(self.usermap_iwds(map));
        Ok(out)
    }

    pub fn iwd_paths(&self) -> Result<Vec<PathBuf>> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(self.main_dir())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iwd")))
            .collect();
        // Later archives override earlier ones; the game sorts names ascending.
        out.sort();
        Ok(out)
    }
}
