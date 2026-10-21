//! The developer tools' switch: the map editor (F7), the look editor (F9)
//! and the fly camera (F8) are for building the game, not for players. On
//! with `COD4RW_DEV=1`, or a `dev.txt` beside the exe (`play\dev.txt` for
//! "Play Latest Build.bat"; test copies in `target\release` find
//! `target\release\dev.txt`). Test runs' own switches (`COD4RW_MAPEDIT`,
//! `COD4RW_LOOKEDIT`, `COD4RW_FLYCAM`) work either way.

use std::sync::OnceLock;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let env = std::env::var("COD4RW_DEV").is_ok_and(|v| v.trim() == "1");
        let file = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("dev.txt"))).is_some_and(|p| p.is_file());
        let tests = ["COD4RW_MAPEDIT", "COD4RW_LOOKEDIT", "COD4RW_FLYCAM"].iter().any(|v| std::env::var_os(v).is_some());
        let on = env || file || tests;
        bevy::log::info!("dev tools {}", if on { "enabled" } else { "disabled" });
        on
    })
}

/// A dev tool's key was pressed: whether it may open (logs once if not).
pub fn allow(tool: &str) -> bool {
    if enabled() {
        return true;
    }
    bevy::log::info!("dev tools disabled: {tool} is off in this build (COD4RW_DEV=1 or dev.txt beside the exe turns it on)");
    false
}
