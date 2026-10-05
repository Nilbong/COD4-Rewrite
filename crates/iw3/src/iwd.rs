//! `.iwd` archives (plain zip files) layered into one read-only filesystem.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

type Archive = zip::ZipArchive<BufReader<File>>;

pub struct Vfs {
    archives: Vec<(PathBuf, Mutex<Archive>)>,
    /// Lower-cased path with forward slashes -> (archive index, entry index).
    index: HashMap<String, (usize, usize)>,
}

pub fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

impl Vfs {
    /// Mount archives in order; later archives override earlier entries.
    pub fn mount(paths: &[PathBuf]) -> Result<Vfs> {
        let mut archives = Vec::new();
        let mut index = HashMap::new();
        for path in paths {
            let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
            let mut zip = zip::ZipArchive::new(BufReader::new(file))
                .with_context(|| format!("reading zip directory of {}", path.display()))?;
            let ai = archives.len();
            for ei in 0..zip.len() {
                let Ok(entry) = zip.by_index_raw(ei) else { continue };
                if entry.is_dir() {
                    continue;
                }
                index.insert(normalize(entry.name()), (ai, ei));
            }
            archives.push((path.clone(), Mutex::new(zip)));
        }
        Ok(Vfs { archives, index })
    }

    pub fn contains(&self, path: &str) -> bool {
        self.index.contains_key(&normalize(path))
    }

    pub fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let Some(&(ai, ei)) = self.index.get(&normalize(path)) else { return Ok(None) };
        let (archive_path, zip) = &self.archives[ai];
        let mut zip = zip.lock().unwrap();
        let mut entry = zip.by_index(ei).with_context(|| format!("opening {path} in {}", archive_path.display()))?;
        let mut out = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut out)?;
        Ok(Some(out))
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.index.keys().map(String::as_str)
    }

    pub fn archive_of(&self, path: &str) -> Option<&Path> {
        self.index.get(&normalize(path)).map(|&(ai, _)| self.archives[ai].0.as_path())
    }
}
