//! Locating a Modern Warfare 2 installation.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Install {
    pub root: PathBuf,
}

impl Install {
    /// Use `MW2_PATH` if set, otherwise probe common install locations.
    pub fn locate() -> Result<Install> {
        if let Ok(p) = std::env::var("MW2_PATH") {
            return Self::at(p);
        }
        let mut candidates = Vec::new();
        for drive in ["C", "D", "E", "F", "G", "H"] {
            for sub in [
                r"Games\Call of Duty - Modern Warfare 2",
                r"SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2",
                r"Steam\steamapps\common\Call of Duty Modern Warfare 2",
                r"Program Files (x86)\Steam\steamapps\common\Call of Duty Modern Warfare 2",
            ] {
                candidates.push(PathBuf::from(format!(r"{drive}:\{sub}")));
            }
        }
        match candidates.into_iter().find(|c| Self::looks_valid(c)) {
            Some(root) => Ok(Install { root }),
            None => bail!("could not find a Modern Warfare 2 install; set MW2_PATH to the folder containing iw4mp.exe"),
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
