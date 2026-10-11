//! Game Folders: which Call of Duty installs were found, and a folder picker
//! for the ones that weren't (`gamepaths` saves the choice). Shown when CoD4
//! can't be found, once when an optional game isn't, and whenever the
//! launcher is run with `--folders`.

use gamepaths::{Game, Source};
use std::io::Write;
use std::path::PathBuf;

/// Make sure CoD4 is found (asking for its folder if not) and, the first
/// time or when `always` (`--folders`), offer to pick the others. Err if
/// the player gives up without CoD4.
pub fn check(always: bool) -> Result<(), String> {
    let missing_optional = Game::ALL.iter().any(|&g| !g.required() && gamepaths::find(g).is_none());
    let asked_before = marker().is_some_and(|m| m.exists());
    print_status();
    if gamepaths::find(Game::Cod4).is_none() {
        println!();
        println!("Call of Duty 4 is needed to play: the game uses its maps, guns and sounds.");
        menu();
        return if gamepaths::find(Game::Cod4).is_some() { Ok(()) } else { Err("Call of Duty 4's folder wasn't set".into()) };
    }
    if always || (missing_optional && !asked_before) {
        if !always {
            println!();
            println!("Black Ops, Modern Warfare 2 and World at War are optional: each one you have adds its guns, maps and characters.");
        }
        menu();
        if let Some(m) = marker() {
            let _ = std::fs::create_dir_all(m.parent().unwrap_or(&m));
            let _ = std::fs::write(&m, "The launcher has shown Game Folders once. Run Launcher.exe --folders to see it again.\n");
        }
    } else if missing_optional {
        println!("(Run Launcher.exe --folders to pick folders for the others.)");
    }
    Ok(())
}

/// Each game: where it is and how it was found, or that it wasn't.
fn print_status() {
    println!();
    println!("Game folders:");
    for (i, g) in Game::ALL.into_iter().enumerate() {
        let what = match gamepaths::find_with_source(g) {
            Some((p, how)) => format!(
                "{}  ({})",
                p.display(),
                match how {
                    Source::EnvVar => "from ".to_owned() + g.env_var(),
                    Source::Chosen => "chosen".to_owned(),
                    Source::Steam => "Steam".to_owned(),
                    Source::Usual => "found".to_owned(),
                }
            ),
            None if g.required() => "NOT FOUND (needed)".to_owned(),
            None => "not found (optional)".to_owned(),
        };
        println!("  {}. {:<33} {}", i + 1, g.title(), what);
    }
}

/// Pick folders until the player presses Enter.
fn menu() {
    loop {
        println!();
        print!("Type a number to choose that game's folder, or press Enter to continue: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let Some(game) = line.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| Game::ALL.get(i).copied()) else {
            println!("Type 1 to {}, or just press Enter.", Game::ALL.len());
            continue;
        };
        let Some(folder) = pick(game) else {
            println!("No folder chosen.");
            continue;
        };
        match gamepaths::set_chosen(game, Some(&folder)) {
            Ok(()) => println!("Saved: {}.", game.title()),
            Err(e) => println!("{e}"),
        }
        print_status();
    }
}

/// Windows' folder picker (through PowerShell), or a typed path if it
/// can't open.
fn pick(game: Game) -> Option<PathBuf> {
    println!("Choose the folder with {} in it (the window may open behind this one).", game.exe());
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description = 'Choose the {} folder (with {})'; $d.ShowNewFolderButton = $false; if ($d.ShowDialog() -eq 'OK') {{ $d.SelectedPath }}",
        game.title().replace('\'', ""),
        game.exe()
    );
    let picked = std::process::Command::new("powershell").args(["-NoProfile", "-STA", "-Command", &script]).output();
    match picked {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            (!path.is_empty()).then(|| PathBuf::from(path))
        }
        _ => {
            print!("Type or paste the folder's path: ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok()?;
            let path = line.trim().trim_matches('"');
            (!path.is_empty()).then(|| PathBuf::from(path))
        }
    }
}

/// Set once Game Folders has been offered for the optional games.
fn marker() -> Option<PathBuf> {
    gamepaths::config_file().map(|f| f.with_file_name("launcher_folders_asked.txt"))
}
