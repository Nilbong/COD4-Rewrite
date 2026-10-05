//! Top-level flow: CoD4's menus, the map's load screen, then the match.

use bevy::prelude::*;

#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameState {
    /// The frontend menus ([`crate::ui`]).
    #[default]
    Frontend,
    /// The map's load screen is up; the map loads as the match starts.
    Loading,
    InGame,
}

/// Match setup on entering [`GameState::InGame`], in order: content (what was
/// `PreStartup`), then everything spawned from it (what was `Startup`).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum Setup {
    Content,
    Spawn,
}

/// Run condition for gameplay systems.
pub fn in_game(state: Option<Res<State<GameState>>>) -> bool {
    state.is_some_and(|s| *s.get() == GameState::InGame)
}

pub struct StatePlugin(pub GameState);

impl Plugin for StatePlugin {
    fn build(&self, app: &mut App) {
        app.insert_state(self.0).configure_sets(OnEnter(GameState::InGame), (Setup::Content, Setup::Spawn).chain());
    }
}
