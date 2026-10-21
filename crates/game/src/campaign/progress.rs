//! The campaign's progress, as CoD4 keeps it in the player's profile: its
//! mission dvars (`mis_01`, the furthest mission reached; `mis_difficulty`,
//! a digit per level for the hardest difficulty completed; `mis_cheat`)
//! and the difficulty (`g_gameskill`), in
//! `%LOCALAPPDATA%/cod4rw/campaign.txt`. Debug runs read it but never save.

use std::collections::HashMap;

/// The mission dvars the scripts keep (`setmissiondvar`), and the skill.
pub const KEYS: [&str; 4] = ["mis_01", "mis_difficulty", "mis_cheat", "g_gameskill"];

fn path() -> Option<std::path::PathBuf> {
    // (Debug: `COD4RW_CAMPAIGN_PROGRESS=<file>` reads another; runs that
    // set it never write, as every debug run.)
    if let Some(p) = std::env::var_os("COD4RW_CAMPAIGN_PROGRESS") {
        return Some(std::path::PathBuf::from(p));
    }
    std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join("cod4rw").join("campaign.txt"))
}

/// The saved values, with CoD4's defaults for a fresh profile.
pub fn load() -> HashMap<String, String> {
    let mut out: HashMap<String, String> = [
        ("mis_01", "0".to_owned()),
        ("mis_difficulty", "0".repeat(40)),
        ("mis_cheat", "0".to_owned()),
        ("g_gameskill", "1".to_owned()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    if let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=')
                && KEYS.contains(&k.trim())
            {
                out.insert(k.trim().to_owned(), v.trim().to_owned());
            }
        }
    }
    out
}

/// Keep the scripts' mission dvars (not in debug runs).
pub fn save(dvars: &HashMap<String, String>) {
    if crate::audio::silent_run() {
        return;
    }
    let Some(p) = path() else { return };
    let text: String = KEYS.iter().filter_map(|k| dvars.get(*k).map(|v| format!("{k}={v}\n"))).collect();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).ok();
    }
    std::fs::write(p, text).ok();
}

/// Keep one of them (the SP menus' difficulty).
pub fn set(key: &str, value: &str) {
    let mut all = load();
    all.insert(key.to_owned(), value.to_owned());
    save(&all);
}
