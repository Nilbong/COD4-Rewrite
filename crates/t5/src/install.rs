//! Locating a Black Ops installation.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Install {
    pub root: PathBuf,
}

impl Install {
    /// Use `BLACKOPS_PATH` if set, otherwise probe common install locations.
    pub fn locate() -> Result<Install> {
        if let Ok(p) = std::env::var("BLACKOPS_PATH") {
            return Self::at(p);
        }
        let mut candidates = Vec::new();
        for drive in ["C", "D", "E", "F", "G", "H"] {
            for sub in [
                r"Call of Duty - Black Ops",
                r"SteamLibrary\steamapps\common\Call of Duty Black Ops",
                r"Steam\steamapps\common\Call of Duty Black Ops",
                r"Program Files (x86)\Steam\steamapps\common\Call of Duty Black Ops",
            ] {
                candidates.push(PathBuf::from(format!(r"{drive}:\{sub}")));
            }
        }
        match candidates.into_iter().find(|c| Self::looks_valid(c)) {
            Some(root) => Ok(Install { root }),
            None => bail!("could not find a Black Ops install; set BLACKOPS_PATH to the folder containing BlackOps.exe"),
        }
    }

    pub fn at(root: impl Into<PathBuf>) -> Result<Install> {
        let root = root.into();
        if !Self::looks_valid(&root) {
            bail!("{} does not look like a Black Ops install (expected main/ and zone/common/)", root.display());
        }
        Ok(Install { root })
    }

    fn looks_valid(root: &Path) -> bool {
        root.join("main").is_dir() && root.join("zone").join("common").is_dir()
    }

    /// Path of a zone, e.g. `zone_path("common_mp")`. Localized zones
    /// (`en_common_mp`) live in the language folder.
    pub fn zone_path(&self, name: &str) -> PathBuf {
        let zone = self.root.join("zone");
        let common = zone.join("common").join(format!("{name}.ff"));
        if common.exists() {
            return common;
        }
        std::fs::read_dir(&zone)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path().join(format!("{name}.ff"))))
            .find(|p| p.exists())
            .unwrap_or(common)
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
