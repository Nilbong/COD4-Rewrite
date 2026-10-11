//! Options > Game Folders' rows ([`crate::settings::FOLDERS`]): where each
//! Call of Duty install is ([`gamepaths`]), and Windows' folder picker to
//! choose one. The picker runs on its own thread (through PowerShell), so
//! the game keeps drawing; the choice is saved by `gamepaths` and used from
//! the next start.

use gamepaths::{Game, Source};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What happened to the last pick: the game and a line about it.
static STATUS: Mutex<Option<(Game, String)>> = Mutex::new(None);
/// The game whose folder is being picked.
static PICKING: Mutex<Option<Game>> = Mutex::new(None);
/// Each game's install, looked up at most once a second (the rows show it
/// every frame).
static FOUND: Mutex<Option<(Instant, Vec<Option<(PathBuf, Source)>>)>> = Mutex::new(None);

fn found(game: Game) -> Option<(PathBuf, Source)> {
    let Ok(mut cache) = FOUND.lock() else { return gamepaths::find_with_source(game) };
    if cache.as_ref().is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(1)) {
        *cache = Some((Instant::now(), Game::ALL.iter().map(|&g| gamepaths::find_with_source(g)).collect()));
    }
    let i = Game::ALL.iter().position(|&g| g == game)?;
    cache.as_ref().and_then(|(_, v)| v[i].clone())
}

fn picking(game: Game) -> bool {
    PICKING.lock().is_ok_and(|p| *p == Some(game))
}

/// The row's value: how the game was found, or that it wasn't (the folder
/// itself is in the line under the rows).
pub fn shown(game: Game) -> String {
    if picking(game) {
        return "Choosing...".into();
    }
    match found(game) {
        Some((_, Source::EnvVar)) => format!("Set by {}", game.env_var()),
        Some((_, Source::Chosen)) => "Your folder".into(),
        Some((_, Source::Steam)) => "Found in Steam".into(),
        Some((_, Source::Usual)) => "Found".into(),
        None => "Not found".into(),
    }
}

/// The line under the rows for a focused game: what it adds, where it is
/// and how it was found, and the last pick's outcome.
pub fn about(game: Game, what: &str) -> String {
    let status = STATUS.lock().ok().and_then(|s| s.clone()).filter(|(g, _)| *g == game).map(|(_, line)| line);
    if let Some(line) = status {
        return format!("{what} {line}");
    }
    let where_ = match found(game) {
        Some((path, _)) => format!("In {}. Select to choose another folder.", path.display()),
        None => format!("Not found. Select to choose its folder (the one with {}).", game.exe()),
    };
    format!("{what} {where_}")
}

/// Open Windows' folder picker for `game` (unless one is open already).
pub fn pick(game: Game) {
    {
        let Ok(mut p) = PICKING.lock() else { return };
        if p.is_some() {
            return;
        }
        *p = Some(game);
    }
    std::thread::spawn(move || {
        let line = match folder_dialog(game) {
            Some(folder) => match gamepaths::set_chosen(game, Some(&folder)) {
                Ok(()) => "Saved. Restart the game to load it.".to_owned(),
                Err(e) => format!("Not saved: {e}."),
            },
            None => "No folder chosen.".to_owned(),
        };
        bevy::log::info!("game folders: {}: {line}", game.title());
        if let Ok(mut s) = STATUS.lock() {
            *s = Some((game, line));
        }
        if let Ok(mut c) = FOUND.lock() {
            *c = None;
        }
        if let Ok(mut p) = PICKING.lock() {
            *p = None;
        }
    });
}

/// Windows' folder picker, kept over the game's window; the folder, if one
/// was chosen.
fn folder_dialog(game: Game) -> Option<PathBuf> {
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         $top = New-Object System.Windows.Forms.Form -Property @{{TopMost = $true; ShowInTaskbar = $false}}; \
         $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
         $d.Description = 'Choose the {} folder (the one with {})'; $d.ShowNewFolderButton = $false; \
         if ($d.ShowDialog($top) -eq 'OK') {{ $d.SelectedPath }}",
        game.title().replace('\'', ""),
        game.exe()
    );
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-STA", "-Command", &script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let out = cmd.output().ok().filter(|o| o.status.success())?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!path.is_empty()).then(|| PathBuf::from(path))
}
