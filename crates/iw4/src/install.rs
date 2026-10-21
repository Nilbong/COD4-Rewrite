//! Locating a Modern Warfare 2 installation.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// The language folders a Modern Warfare 2 release ships in `zone/`.
const LANGUAGES: &[&str] = &[
    "english",
    "french",
    "german",
    "italian",
    "spanish",
    "russian",
    "polish",
    "japanese",
    "czech",
    "portuguese",
    "korean",
    "chinese",
    "taiwanese",
];

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

    /// Path of a zone, e.g. `zone_path("common_mp")`: the first of
    /// [`Install::zone_candidates`].
    pub fn zone_path(&self, name: &str) -> PathBuf {
        let file = format!("{name}.ff");
        self.zone_candidates(name)
            .into_iter()
            .next()
            .unwrap_or_else(|| self.root.join("zone").join("english").join(file))
    }

    /// Every copy of a zone on disk, best first: the install's language
    /// (`localization.txt`), English, the other languages, `zone/` itself
    /// (the map packs), then any other folder (a mod's, a backup, a zone
    /// builder's output) last.
    pub fn zone_candidates(&self, name: &str) -> Vec<PathBuf> {
        let file = format!("{name}.ff");
        let zone = self.root.join("zone");
        let preferred = self.language();
        let mut dirs: Vec<(u8, String, PathBuf)> = Self::language_dirs(&self.root)
            .map(|d| {
                let folder = d.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                let rank = if preferred.as_deref() == Some(folder.as_str()) {
                    0
                } else if folder == "english" {
                    1
                } else if LANGUAGES.contains(&folder.as_str()) {
                    2
                } else {
                    4
                };
                (rank, folder, d)
            })
            .collect();
        dirs.push((3, String::new(), zone));
        dirs.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        dirs.into_iter().map(|(_, _, d)| d.join(&file)).filter(|p| p.is_file()).collect()
    }

    /// The language `localization.txt` names (its first word), if it has a zone folder.
    pub fn language(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.root.join("localization.txt")).ok()?;
        let word = text.split_whitespace().next()?.to_ascii_lowercase();
        self.root.join("zone").join(&word).is_dir().then_some(word)
    }

    /// The `zone/` folders, with whether each is a language's.
    pub fn zone_dirs(&self) -> Vec<(PathBuf, bool)> {
        let mut out: Vec<(PathBuf, bool)> = Self::language_dirs(&self.root)
            .map(|d| {
                let language = d
                    .file_name()
                    .is_some_and(|n| LANGUAGES.contains(&n.to_string_lossy().to_ascii_lowercase().as_str()));
                (d, language)
            })
            .collect();
        out.sort();
        out
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
