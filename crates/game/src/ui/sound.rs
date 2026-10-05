//! Menu sounds: the sound aliases menu scripts `play` (`mouse_over`,
//! `mouse_click`, ...) and the music a menu loops while it is open (its
//! `soundName`, `music_mainmenu_mp` for the main menus). The aliases come
//! from the UI zones, read by [`crate::audio::bank`].

use super::Frontend;
use crate::audio::bank::{self, Sources, Variant};
use crate::state::GameState;
use bevy::audio::Volume;
use bevy::prelude::*;
use std::collections::HashMap;

/// What the menus want heard: sounds to start and the music to loop.
#[derive(Default)]
pub struct MenuAudio {
    /// Aliases `play`ed since last frame.
    pub queue: Vec<String>,
    /// The last opened menu's `soundName`.
    pub music: Option<String>,
    sources: Sources,
    missing: HashMap<String, ()>,
}

/// The playing menu music and its alias.
#[derive(Component)]
pub struct MenuMusic(String);

impl Frontend {
    /// An alias's audio and the variation picked.
    fn sound(&mut self, alias: &str, audio: &mut Assets<AudioSource>) -> Option<(Handle<AudioSource>, Variant)> {
        let key = alias.to_ascii_lowercase();
        let v = self.assets.sounds.get(&key).and_then(|v| bank::pick(v)).cloned();
        let vfs = self.assets.vfs();
        let made = v.and_then(|v| Some((self.audio.sources.handle(&v.file, &vfs, audio)?, v)));
        if made.is_none() && self.audio.missing.insert(key, ()).is_none() {
            debug!("ui: no sound {alias}");
        }
        made
    }
}

/// Start `play`ed sounds, and loop the menu music while in the menus.
pub(super) fn play(
    mut commands: Commands,
    mut fe: ResMut<Frontend>,
    mut audio: ResMut<Assets<AudioSource>>,
    state: Res<State<GameState>>,
    music: Query<(Entity, &MenuMusic)>,
) {
    for alias in std::mem::take(&mut fe.audio.queue) {
        if let Some((source, v)) = fe.sound(&alias, &mut audio) {
            commands.spawn((AudioPlayer(source), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(v.volume.1))));
        }
    }
    let wanted = fe.audio.music.clone().filter(|_| *state.get() == GameState::Frontend);
    if music.iter().any(|(_, m)| Some(&m.0) == wanted.as_ref()) {
        return;
    }
    for (e, _) in &music {
        commands.entity(e).despawn();
    }
    if let Some(alias) = wanted {
        if let Some((source, v)) = fe.sound(&alias, &mut audio) {
            info!("ui: music {alias}");
            let mode = if v.looping { PlaybackSettings::LOOP } else { PlaybackSettings::ONCE };
            commands.spawn((MenuMusic(alias), AudioPlayer(source), mode.with_volume(Volume::Linear(v.volume.1))));
        }
    }
}
