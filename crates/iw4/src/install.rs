//! Locating a Modern Warfare 2 installation.

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
        let game = gamepaths::Game::Mw2;
        match gamepaths::find(game) {
            Some(root) => Ok(Install { root }),
            None => bail!("could not find a Modern Warfare 2 install; pick its folder (the one with {}) in the launcher's or Options' Game Folders, or set {}", game.exe(), game.env_var()),
        }
    }

    pub fn at(root: impl Into<PathBuf>) -> Result<Install> {
        let root = root.into();
        if !Self::looks_valid(&root) {
            bail!("{} does not look like a Modern Warfare 2 install (expected main/ and zone/<language>/common_mp.ff)", root.display());
        }
        Ok(Install { root })
    }

    fn looks_valid(root: &Path) -> bool {
        root.join("main").is_dir() && Self::language_dirs(root).any(|d| d.join("common_mp.ff").exists())
    }

    /// `zone/english`, ...: every zone is in its language's folder.
    fn language_dirs(root: &Path) -> impl Iterator<Item = PathBuf> {
        std::fs::read_dir(root.join("zone")).into_iter().flatten().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir())
    }

    /// Path of a zone, e.g. `zone_path("common_mp")`.
    pub fn zone_path(&self, name: &str) -> PathBuf {
        let file = format!("{name}.ff");
        // (The map packs' zones sit in `zone/` itself.)
        Self::language_dirs(&self.root)
            .map(|d| d.join(&file))
            .chain(std::iter::once(self.root.join("zone").join(&file)))
            .find(|p| p.exists())
            .unwrap_or_else(|| self.root.join("zone").join("english").join(file))
    }

    /// The `.iwd` archives in load order (later ones override earlier ones).
    pub fn iwd_paths(&self) -> Result<Vec<PathBuf>> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(self.root.join("main"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iwd")))
            .collect();
        out.sort();
        Ok(out)
    }

    /// The `.iwd` archives mounted as one filesystem (images, weapon files, sounds).
    pub fn vfs(&self) -> Result<iw3::iwd::Vfs> {
        iw3::iwd::Vfs::mount(&self.iwd_paths()?)
    }
}
