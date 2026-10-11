//! Player profiles: each its own stats file (rank, unlocks, classes,
//! combat record, settings), as CoD4's profiles each had their own
//! `mpdata`. The first profile is the original `stats.txt`; the others live
//! in `profiles/<id>/stats.txt`, and `profile.txt` names the one in use.
//! A profile's name is its combat record name.
//!
//! The menus' scripts: `addPlayerProfiles`, `loadPlayerProfile` (the one
//! selected, `ui_profile_selected`), `createPlayerProfile` (named by the
//! create popup's field), `deletePlayerProfile`; `loadProfile <id>` and
//! `selectProfile <id>` for the new UI's list.

use super::Frontend;
use super::expr::Env;
use bevy::prelude::*;
use std::path::PathBuf;

/// The profile picked in the list (its id; "" the first).
pub(super) const SELECTED_DVAR: &str = "ui_profile_selected";
/// The create popup's name field's dvar, when the field isn't found.
const NEW_NAME_DVAR: &str = "ui_playerprofilenamenew";

#[derive(Clone, Debug)]
pub(super) struct Profile {
    /// "" for the first profile, else its folder's name.
    pub id: String,
    pub name: String,
    /// Rank XP (stat 2301), for the list.
    pub xp: i32,
}

fn root() -> Option<PathBuf> {
    super::stats::data_dir()
}

/// The stats file of profile `id`.
pub(super) fn stats_path(id: &str) -> Option<PathBuf> {
    let root = root()?;
    Some(if id.is_empty() { root.join("stats.txt") } else { root.join("profiles").join(id).join("stats.txt") })
}

/// The profile in use (its id).
pub(super) fn active() -> String {
    let id = root().and_then(|r| std::fs::read_to_string(r.join("profile.txt")).ok()).map(|s| s.trim().to_owned()).unwrap_or_default();
    // (Gone since: the first.)
    if id.is_empty() || stats_path(&id).is_some_and(|p| p.exists()) { id } else { String::new() }
}

fn set_active(id: &str) {
    if let Some(r) = root() {
        let _ = std::fs::create_dir_all(&r).and_then(|_| std::fs::write(r.join("profile.txt"), id));
    }
}

/// A stats file's name and rank XP, read without loading it.
fn summary(text: &str) -> (String, i32) {
    let mut name = String::new();
    let mut xp = 0;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix(&format!("dvar {} ", super::combat_record::NAME_DVAR)) {
            name = v.trim().to_owned();
        } else if let Some(v) = line.strip_prefix("2301 ") {
            xp = v.trim().parse().unwrap_or(0);
        }
    }
    (if name.is_empty() { "Player".to_owned() } else { name }, xp)
}

/// The list as last read, and when (the menus ask every frame).
static LISTED: std::sync::Mutex<Option<(std::time::Instant, Vec<Profile>)>> = std::sync::Mutex::new(None);

/// Profiles changed: read them again.
fn forget_list() {
    *LISTED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Every profile: the first (when it has a file, or nothing else does),
/// then the others by name.
pub(super) fn list() -> Vec<Profile> {
    let mut listed = LISTED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, list)) = listed.as_ref().filter(|(at, _)| at.elapsed().as_secs_f32() < 2.0) {
        let _ = at;
        return list.clone();
    }
    let list = read_list();
    *listed = Some((std::time::Instant::now(), list.clone()));
    list
}

fn read_list() -> Vec<Profile> {
    let mut out = Vec::new();
    let Some(root) = root() else { return out };
    if let Ok(dir) = std::fs::read_dir(root.join("profiles")) {
        for e in dir.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            let Ok(text) = std::fs::read_to_string(e.path().join("stats.txt")) else { continue };
            let (name, xp) = summary(&text);
            out.push(Profile { id, name, xp });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let first = std::fs::read_to_string(root.join("stats.txt")).ok();
    if first.is_some() || out.is_empty() {
        let (name, xp) = first.as_deref().map_or(("Player".to_owned(), 0), summary);
        out.insert(0, Profile { id: String::new(), name, xp });
    }
    out
}

/// A folder name for `name`: its letters and digits, made unique.
fn new_id(name: &str) -> String {
    let base: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect::<String>().to_ascii_lowercase();
    let base = if base.is_empty() { "profile".to_owned() } else { base };
    let taken = |id: &str| stats_path(id).is_some_and(|p| p.parent().is_some_and(|d| d.exists()));
    (0..).map(|n| if n == 0 { base.clone() } else { format!("{base}{n}") }).find(|id| !taken(id)).unwrap_or(base)
}

impl Frontend {
    pub(super) fn profile_script(&mut self, args: &[String]) -> bool {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        match a(0).to_ascii_lowercase().as_str() {
            "addplayerprofiles" | "sortplayerprofiles" => {
                self.set_dvar("ui_playerProfileCount", &list().len().to_string());
            }
            "selectactiveplayerprofile" => {
                let id = active();
                self.set_dvar(SELECTED_DVAR, &id);
            }
            "selectprofile" => self.set_dvar(SELECTED_DVAR, a(1)),
            "loadplayerprofile" => {
                let id = self.dvar(SELECTED_DVAR);
                self.load_profile(&id);
            }
            "loadprofile" => self.load_profile(a(1)),
            "createplayerprofile" => {
                // The create popup's field.
                let field = self.stack.iter().rev().flat_map(|m| m.menu.items.iter()).find(|it| it.window.name.eq_ignore_ascii_case("createprofile")).map(|it| it.dvar.clone());
                let dvar = field.filter(|d| !d.is_empty()).unwrap_or_else(|| NEW_NAME_DVAR.into());
                let name = super::combat_record::clean_name(&self.dvar(&dvar), 16);
                if name.trim().is_empty() {
                    return true;
                }
                self.create_profile(name.trim());
                self.set_dvar(&dvar, "");
                self.reopen_profiles();
            }
            "deleteplayerprofile" => {
                let id = self.dvar(SELECTED_DVAR);
                self.delete_profile(&id);
                self.reopen_profiles();
            }
            _ => return false,
        }
        true
    }

    /// Profiles change: the list again (if it's open).
    fn reopen_profiles(&mut self) {
        if let Some(i) = self.stack.iter().position(|m| m.name == "player_profile") {
            self.stack.remove(i);
            self.open("player_profile");
        }
    }

    /// A new profile, named `name`, in use from now: CoD4's unlocks from
    /// rank one, with this profile's settings.
    fn create_profile(&mut self, name: &str) {
        let id = new_id(name);
        let Some(path) = stats_path(&id) else { return };
        let mut text = format!("dvar {} {name}\ndvar {} cod4\n", super::combat_record::NAME_DVAR, super::progression::UNLOCKS_DVAR);
        let mut settings: Vec<_> = self.stats.dvars.iter().filter(|(k, _)| crate::settings::is_setting(k) || k.as_str() == super::modern::STYLE_DVAR).collect();
        settings.sort();
        for (k, v) in settings {
            text += &format!("dvar {k} {v}\n");
        }
        let made = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, text));
        forget_list();
        match made {
            Ok(()) => {
                info!("ui: profile {name:?} created ({id})");
                self.load_profile(&id);
            }
            Err(e) => warn!("ui: can't create profile {name:?}: {e}"),
        }
    }

    /// Delete profile `id`: its folder (or the first's file) goes to
    /// `deleted/`, not away. The one in use becomes the first left.
    fn delete_profile(&mut self, id: &str) {
        let (Some(root), Some(path)) = (root(), stats_path(id)) else { return };
        let was_active = active() == id;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let bin = root.join("deleted").join(format!("{}-{stamp}", if id.is_empty() { "first" } else { id }));
        let from = if id.is_empty() { path.clone() } else { path.parent().map(PathBuf::from).unwrap_or(path.clone()) };
        let moved = std::fs::create_dir_all(&bin).and_then(|_| std::fs::rename(&from, bin.join(from.file_name().unwrap_or_default())));
        forget_list();
        if let Err(e) = moved {
            warn!("ui: can't delete profile {id:?}: {e}");
            return;
        }
        info!("ui: profile {id:?} deleted (kept in {})", bin.display());
        if was_active {
            let next = list().first().map(|p| p.id.clone()).unwrap_or_default();
            self.load_profile(&next);
        }
    }

    /// Use profile `id`: its stats, and the dvars kept with them.
    pub(super) fn load_profile(&mut self, id: &str) {
        let Some(path) = stats_path(id) else { return };
        self.stats.save_if_changed();
        let old: Vec<String> = self.stats.dvars.keys().cloned().collect();
        let Some(stats) = self.stats.load_from(&self.assets, path) else {
            // (Debug runs keep theirs.)
            return;
        };
        set_active(id);
        for k in old {
            self.dvars.remove(&k);
        }
        for (k, v) in super::settings_menu::default_dvars() {
            self.dvars.insert(k, v);
        }
        for (k, v) in &stats.dvars {
            self.dvars.insert(k.clone(), v.clone());
        }
        self.stats = stats;
        forget_list();
        super::custom_camo::sync(&self.stats);
        let name = self.profile_name();
        self.set_dvar("com_playerProfile", &name);
        self.set_dvar(SELECTED_DVAR, id);
        info!("ui: profile {name:?} in use");
    }
}
