//! The game window's icon (title bar, taskbar, Alt-Tab): the launcher's
//! emblem (crates/launcher/assets), set once the window exists. The exe's
//! own icon (Explorer, shortcuts) is embedded by build.rs.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// 64x64 RGBA, made from crates/launcher/assets/icon.png.
const ICON: &[u8] = include_bytes!("../../launcher/assets/icon64.rgba");

pub struct WindowIconPlugin;

impl Plugin for WindowIconPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, set_icon);
    }
}

fn set_icon(mut done: Local<bool>, window: Query<Entity, With<PrimaryWindow>>) {
    if *done {
        return;
    }
    let Ok(entity) = window.single() else { return };
    bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
        let Some(w) = windows.get_window(entity) else { return };
        if let Ok(icon) = winit::window::Icon::from_rgba(ICON.to_vec(), 64, 64) {
            w.set_window_icon(Some(icon));
        }
        *done = true;
    });
}
