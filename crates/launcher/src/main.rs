//! Launcher.exe: double-click to play.
//!
//! 1. Asks GitHub for the latest release of the game
//!    (`github.com/Nilbong/COD4-Rewrite/releases`) and, if it's newer than
//!    the copy beside the launcher (`game\cod4rw.exe`, its version in
//!    `game\version.txt`), downloads it, with progress.
//! 2. Starts the game (any arguments are passed on) and closes.
//!
//! Offline, or if GitHub can't be reached, it starts the copy it has. Nothing
//! is installed: the game reads Call of Duty 4's own files from the player's
//! install, so none of them are downloaded.
//!
//! `Launcher.exe --build` builds the game from this folder's source instead
//! (needs Rust: https://rustup.rs) and starts that build.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "Nilbong/COD4-Rewrite";
const GAME_ASSET: &str = "cod4rw.exe";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let here = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
    println!("CoD4 Rewrite launcher");
    println!();
    let result = if args.iter().any(|a| a == "--build") {
        build_from_source(&here).and_then(|exe| start(&exe, &here, &args))
    } else {
        update(&here).and_then(|exe| start(&exe, &exe.parent().unwrap_or(&here).to_path_buf(), &args))
    };
    if let Err(e) = result {
        println!();
        println!("Couldn't start the game: {e}");
        pause();
    }
}

/// Make sure `game\cod4rw.exe` is the latest release; its path.
fn update(here: &Path) -> Result<PathBuf, String> {
    let dir = here.join("game");
    let exe = dir.join(GAME_ASSET);
    let have = std::fs::read_to_string(dir.join("version.txt")).ok().map(|v| v.trim().to_owned());
    print!("Checking for updates... ");
    let _ = std::io::stdout().flush();
    match latest_release() {
        Ok((tag, url, size)) => {
            if have.as_deref() == Some(tag.as_str()) && exe.exists() {
                println!("up to date ({tag}).");
                return Ok(exe);
            }
            println!("{} {tag} available.", if have.is_some() { "update" } else { "version" });
            std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
            download(&url, size, &exe)?;
            std::fs::write(dir.join("version.txt"), &tag).map_err(|e| e.to_string())?;
            println!("Installed {tag}.");
            Ok(exe)
        }
        Err(e) if exe.exists() => {
            println!("couldn't reach GitHub ({e}); starting the version already here.");
            Ok(exe)
        }
        Err(e) => Err(format!(
            "couldn't download the game ({e}). Check your internet connection, or see https://github.com/{REPO}/releases"
        )),
    }
}

/// The latest release's tag and the game's download (URL, size in bytes).
fn latest_release() -> Result<(String, String, u64), String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = ureq::get(&url)
        .set("User-Agent", "cod4rw-launcher")
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "no releases published yet".to_owned(),
            e => e.to_string(),
        })?;
    let json: serde_json::Value = serde_json::from_reader(resp.into_reader()).map_err(|e| e.to_string())?;
    let tag = json["tag_name"].as_str().ok_or("the release has no tag")?.to_owned();
    let asset = json["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(GAME_ASSET))))
        .ok_or_else(|| format!("release {tag} has no {GAME_ASSET}"))?;
    let url = asset["browser_download_url"].as_str().ok_or("no download link")?.to_owned();
    Ok((tag, url, asset["size"].as_u64().unwrap_or(0)))
}

/// Download `url` to `to`, showing progress; a part file until complete, so
/// a broken download never replaces a working game.
fn download(url: &str, size: u64, to: &Path) -> Result<(), String> {
    let part = to.with_extension("part");
    let resp = ureq::get(url).set("User-Agent", "cod4rw-launcher").call().map_err(|e| e.to_string())?;
    let total = resp.header("Content-Length").and_then(|s| s.parse::<u64>().ok()).unwrap_or(size);
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&part).map_err(|e| format!("can't write {}: {e}", part.display()))?;
    let mut buf = vec![0u8; 1 << 16];
    let (mut done, mut shown) = (0u64, u64::MAX);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        let pct = if total > 0 { done * 100 / total } else { 0 };
        if pct != shown {
            shown = pct;
            print!("\rDownloading... {pct:3}%  ({:.1} / {:.1} MB)", done as f64 / 1e6, total as f64 / 1e6);
            let _ = std::io::stdout().flush();
        }
    }
    println!();
    drop(file);
    if total > 0 && done != total {
        let _ = std::fs::remove_file(&part);
        return Err(format!("download incomplete ({done} of {total} bytes)"));
    }
    // The old game may still be running: move it aside first.
    if to.exists() {
        let old = to.with_extension("old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(to, &old).map_err(|e| format!("can't replace the game (is it still running?): {e}"))?;
    }
    std::fs::rename(&part, to).map_err(|e| e.to_string())
}

/// Build the game from this folder's source with cargo; the exe's path.
fn build_from_source(here: &Path) -> Result<PathBuf, String> {
    if !here.join("Cargo.toml").exists() {
        return Err("--build needs the launcher in the source folder (next to Cargo.toml)".into());
    }
    if Command::new("cargo").arg("--version").output().is_err() {
        return Err("building needs Rust: install it from https://rustup.rs (with the C++ build tools it offers), then run Launcher.exe --build again".into());
    }
    println!("Building the game from source (the first build takes 10-15 minutes)...");
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "game"])
        .current_dir(here)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("the build failed (see above)".into());
    }
    Ok(here.join("target").join("release").join(GAME_ASSET))
}

/// Start the game (arguments passed on, minus the launcher's own) and return.
fn start(exe: &Path, dir: &Path, args: &[String]) -> Result<(), String> {
    let pass: Vec<&String> = args.iter().filter(|a| a.as_str() != "--build").collect();
    println!("Starting the game...");
    Command::new(exe).args(pass).current_dir(dir).spawn().map_err(|e| format!("can't start {}: {e}", exe.display()))?;
    Ok(())
}

/// Leave an error on screen until a key is pressed.
fn pause() {
    println!();
    print!("Press Enter to close.");
    let _ = std::io::stdout().flush();
    let _ = std::io::stdin().read_line(&mut String::new());
}
