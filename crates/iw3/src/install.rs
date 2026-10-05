//! Locating a CoD4 installation.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Install {
    pub root: PathBuf,
}

impl Install {
    /// Use `COD4_PATH` if set, otherwise probe common install locations.
    pub fn locate() -> Result<Install> {
        if let Ok(p) = std::env::var("COD4_PATH") {
            let root = PathBuf::from(p);
            if Self::looks_valid(&root) {
                return Ok(Install { root });
            }
            bail!("COD4_PATH={} does not look like a CoD4 install (expected main/ and zone/)", root.display());
        }
        let mut candidates = Vec::new();
        for drive in ["C", "D", "E", "F", "G", "H"] {
            for sub in [
                r"SteamLibrary\steamapps\common\Call of Duty 4",
                r"Steam\steamapps\common\Call of Duty 4",
                r"Program Files (x86)\Steam\steamapps\common\Call of Duty 4",
                r"Program Files (x86)\Activision\Call of Duty 4 - Modern Warfare",
            ] {
                candidates.push(PathBuf::from(format!(r"{drive}:\{sub}")));
            }
        }
        for c in candidates {
            if Self::looks_valid(&c) {
                return Ok(Install { root: c });
            }
        }
        bail!("could not find a CoD4 install; set COD4_PATH to the folder containing iw3mp.exe")
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

    /// Path of a zone, e.g. `zone_path("mp_killhouse")`.
    pub fn zone_path(&self, name: &str) -> PathBuf {
        self.root.join("zone").join("english").join(format!("{name}.ff"))
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
