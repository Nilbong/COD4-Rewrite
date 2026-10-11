//! Photo mode (`COD4RW_PHOTO=1`, from "Map Photos.bat"): a match picked in
//! the menus starts in the free spectator camera ([`crate::bots::Spectate`])
//! with nothing drawn over the view (no HUD, hints or spectator text), and
//! F12 saves a screenshot to `map_shots/<map>_<n>.png` (beside where the
//! game was started), for the map menus' previews and loading screens.

use crate::world::MapName;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

pub struct PhotoPlugin;

impl Plugin for PhotoPlugin {
    fn build(&self, app: &mut App) {
        if active() {
            app.add_systems(Update, shoot.run_if(crate::state::in_game));
        }
    }
}

/// Whether photo mode is on.
pub fn active() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("COD4RW_PHOTO").is_ok_and(|v| v == "1"))
}

/// F12: the window as it is, to the next free `map_shots/<map>_<n>.png`.
fn shoot(mut commands: Commands, keys: Res<ButtonInput<KeyCode>>, map: Res<MapName>) {
    if !keys.just_pressed(KeyCode::F12) {
        return;
    }
    let dir = std::path::Path::new("map_shots");
    std::fs::create_dir_all(dir).ok();
    let path = (1..).map(|n| dir.join(format!("{}_{n}.png", map.0))).find(|p| !p.exists()).unwrap_or_else(|| dir.join("shot.png"));
    info!("photo: {}", path.display());
    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
}
