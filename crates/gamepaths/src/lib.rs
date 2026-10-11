//! Where the player's Call of Duty installs are. Each game is looked for, in
//! order:
//!
//! 1. its environment variable (`COD4_PATH`, `BLACKOPS_PATH`, `MW2_PATH`,
//!    `WAW_PATH`);
//! 2. the folder the player chose (the launcher's or the Options' Game
//!    Folders), saved in `%LOCALAPPDATA%\cod4rw\game_paths.txt`;
//! 3. every Steam library Steam knows of (`steamapps\libraryfolders.vdf`);
//! 4. the usual Steam and retail folders on drives C: to H:.
//!
//! A folder counts only if it looks like that game's install.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Game {
    Cod4,
    BlackOps,
    Mw2,
    WaW,
}

impl Game {
    pub const ALL: [Game; 4] = [Game::Cod4, Game::BlackOps, Game::Mw2, Game::WaW];

    /// Its name for players.
    pub fn title(self) -> &'static str {
        match self {
            Game::Cod4 => "Call of Duty 4: Modern Warfare",
            Game::BlackOps => "Call of Duty: Black Ops",
            Game::Mw2 => "Call of Duty: Modern Warfare 2",
            Game::WaW => "Call of Duty: World at War",
        }
    }

    /// Its key in `game_paths.txt`.
    pub fn key(self) -> &'static str {
        match self {
            Game::Cod4 => "cod4",
            Game::BlackOps => "blackops",
            Game::Mw2 => "mw2",
            Game::WaW => "waw",
        }
    }

    pub fn env_var(self) -> &'static str {
        match self {
            Game::Cod4 => "COD4_PATH",
            Game::BlackOps => "BLACKOPS_PATH",
            Game::Mw2 => "MW2_PATH",
            Game::WaW => "WAW_PATH",
        }
    }

    /// Only CoD4 is needed to play; the others add their guns, maps and
    /// characters.
    pub fn required(self) -> bool {
        self == Game::Cod4
    }

    /// The exe players will recognise in the folder (for messages).
    pub fn exe(self) -> &'static str {
        match self {
            Game::Cod4 => "iw3mp.exe",
            Game::BlackOps => "BlackOps.exe",
            Game::Mw2 => "iw4mp.exe",
            Game::WaW => "CoDWaWmp.exe",
        }
    }

    /// Its folder under `steamapps\common`.
    fn steam_dir(self) -> &'static str {
        match self {
            Game::Cod4 => "Call of Duty 4",
            Game::BlackOps => "Call of Duty Black Ops",
            Game::Mw2 => "Call of Duty Modern Warfare 2",
            Game::WaW => "Call of Duty World at War",
        }
    }

    /// Retail and other usual folders, under a drive's root.
    fn other_dirs(self) -> &'static [&'static str] {
        match self {
            Game::Cod4 => &[r"Program Files (x86)\Activision\Call of Duty 4 - Modern Warfare"],
            Game::BlackOps => &[r"Call of Duty - Black Ops"],
            Game::Mw2 => &[r"Games\Call of Duty - Modern Warfare 2"],
            Game::WaW => &[r"Program Files (x86)\Activision\Call of Duty - World at War", r"Call of Duty World at War"],
        }
    }

    /// Whether `root` looks like this game's install (its game data, not
    /// just the exe).
    pub fn looks_valid(self, root: &Path) -> bool {
        if !root.join("main").is_dir() {
            return false;
        }
        let zone = root.join("zone");
        match self {
            Game::Cod4 => zone.is_dir(),
            Game::BlackOps => zone.join("common").is_dir(),
            // Their zones are in a language's folder (`zone\english`).
            Game::Mw2 | Game::WaW => std::fs::read_dir(&zone)
                .map(|d| d.flatten().any(|e| e.path().join("common_mp.ff").exists()))
                .unwrap_or(false),
        }
    }
}

/// How a game was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    EnvVar,
    Chosen,
    Steam,
    Usual,
}

/// The game's install folder, if there is one.
pub fn find(game: Game) -> Option<PathBuf> {
    find_with_source(game).map(|(p, _)| p)
}

/// The game's install folder and how it was found.
pub fn find_with_source(game: Game) -> Option<(PathBuf, Source)> {
    if let Some(p) = std::env::var_os(game.env_var()).map(PathBuf::from).filter(|p| game.looks_valid(p)) {
        return Some((p, Source::EnvVar));
    }
    if let Some(p) = chosen(game).filter(|p| game.looks_valid(p)) {
        return Some((p, Source::Chosen));
    }
    if let Some(p) = steam_libraries().iter().map(|l| l.join("steamapps").join("common").join(game.steam_dir())).find(|p| game.looks_valid(p)) {
        return Some((p, Source::Steam));
    }
    usual_dirs(game).into_iter().find(|p| game.looks_valid(p)).map(|p| (p, Source::Usual))
}

/// The settings file: `%LOCALAPPDATA%\cod4rw\game_paths.txt`.
pub fn config_file() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("cod4rw").join("game_paths.txt"))
}

/// The folder the player chose for `game`, if any (whether or not it's
/// still valid).
pub fn chosen(game: Game) -> Option<PathBuf> {
    read_config().into_iter().find(|(k, _)| k == game.key()).map(|(_, v)| PathBuf::from(v))
}

/// Save the player's folder for `game` (`None` forgets it). A folder that
/// isn't the game's install is refused, with why.
pub fn set_chosen(game: Game, folder: Option<&Path>) -> Result<(), String> {
    let folder = match folder {
        Some(f) => {
            let f = fix_up(game, f).ok_or_else(|| format!("{} doesn't look like {} (the folder with {}, main and zone in it)", f.display(), game.title(), game.exe()))?;
            Some(f)
        }
        None => None,
    };
    let file = config_file().ok_or("no LOCALAPPDATA folder")?;
    let mut entries: Vec<(String, String)> = read_config().into_iter().filter(|(k, _)| k != game.key()).collect();
    if let Some(f) = folder {
        entries.push((game.key().to_owned(), f.display().to_string()));
    }
    entries.sort_by_key(|(k, _)| Game::ALL.iter().position(|g| g.key() == k));
    let mut text = String::from("# Where your Call of Duty games are installed (set from the launcher or Options > Game Folders).\n");
    for (k, v) in entries {
        text.push_str(&format!("{k} = {v}\n"));
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    }
    std::fs::write(&file, text).map_err(|e| format!("can't save {}: {e}", file.display()))
}

/// The player may pick the exe's folder, or one a level off: the install
/// itself if it's there.
fn fix_up(game: Game, picked: &Path) -> Option<PathBuf> {
    let picked = if picked.is_file() { picked.parent()? } else { picked };
    let candidates = [Some(picked.to_path_buf()), picked.parent().map(Path::to_path_buf)];
    candidates.into_iter().flatten().find(|p| game.looks_valid(p))
}

fn read_config() -> Vec<(String, String)> {
    let Some(text) = config_file().and_then(|f| std::fs::read_to_string(f).ok()) else { return Vec::new() };
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"').to_owned()))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

fn usual_dirs(game: Game) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for drive in ["C", "D", "E", "F", "G", "H"] {
        for steam in [r"SteamLibrary", r"Steam", r"Program Files (x86)\Steam", r"Program Files\Steam"] {
            out.push(PathBuf::from(format!(r"{drive}:\{steam}\steamapps\common\{}", game.steam_dir())));
        }
        for other in game.other_dirs() {
            out.push(PathBuf::from(format!(r"{drive}:\{other}")));
        }
    }
    out
}

/// Every Steam library folder, from Steam's own list (found once).
pub fn steam_libraries() -> &'static [PathBuf] {
    static LIBS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    LIBS.get_or_init(|| {
        let mut roots: Vec<PathBuf> = steam_root().into_iter().collect();
        roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
        roots.push(PathBuf::from(r"C:\Program Files\Steam"));
        let mut libs = Vec::new();
        for root in roots {
            let Ok(vdf) = std::fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf")) else { continue };
            for lib in std::iter::once(root.clone()).chain(library_paths(&vdf)) {
                if !libs.iter().any(|l: &PathBuf| same_dir(l, &lib)) {
                    libs.push(lib);
                }
            }
        }
        libs
    })
}

/// The `"path"` entries of `libraryfolders.vdf`.
fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("\"path\"")?.trim();
            let value = rest.strip_prefix('"')?.strip_suffix('"')?;
            Some(PathBuf::from(value.replace("\\\\", "\\")))
        })
        .collect()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.display().to_string().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    norm(a) == norm(b)
}

/// Steam's folder, from the registry (`HKCU\Software\Valve\Steam`,
/// `SteamPath`).
fn steam_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = std::process::Command::new("reg")
            .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().find(|l| l.contains("SteamPath"))?;
        let value = line.split("REG_SZ").nth(1)?.trim();
        (!value.is_empty()).then(|| PathBuf::from(value.replace('/', "\\")))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_library_folders() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"E:\\\\Games\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(library_paths(vdf), vec![PathBuf::from(r"C:\Program Files (x86)\Steam"), PathBuf::from(r"E:\Games\SteamLibrary")]);
    }

    #[test]
    fn same_dir_ignores_case_and_slashes() {
        assert!(same_dir(Path::new("c:/program files (x86)/steam/"), Path::new(r"C:\Program Files (x86)\Steam")));
    }
}
