//! Leaving a match: everything it brought into the world goes when the game
//! goes back to the menus.
//!
//! Rather than every part of the game tagging what it spawns, the entities
//! that exist before a match starts are noted (once the app is built, and
//! again on entering the load screen), and on leaving [`GameState::InGame`] every
//! top-level entity spawned since (the map, players and bots, the camera,
//! the HUD, sounds) is despawned, with its children. The map's data is
//! dropped too.

use crate::state::GameState;
use bevy::ecs::observer::Observer;
use bevy::prelude::*;
use bevy::window::Window;
use std::collections::HashSet;

pub struct SessionPlugin;

impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BeforeMatch>()
            .add_systems(OnEnter(GameState::Loading), note_entities)
            .add_systems(OnExit(GameState::InGame), clear_match);
        if let Ok(dir) = std::env::var("COD4RW_LEAVETEST") {
            app.insert_resource(LeaveTest(dir.into())).add_systems(Update, leave_test);
        }
    }

    /// The entities there are once the app is built, before anything runs
    /// (a match started from the command line sets up before even
    /// `PreStartup`).
    fn finish(&self, app: &mut App) {
        let world = app.world_mut();
        let all: HashSet<Entity> = world.query::<Entity>().iter(world).collect();
        world.insert_resource(BeforeMatch(all));
    }
}

/// Debug aid: with `COD4RW_LEAVETEST=<dir>`, leave the match after 8 s, go
/// back into it from the menu 4 s later, and do that again; screenshot each
/// step, then exit.
#[derive(Resource)]
struct LeaveTest(std::path::PathBuf);

fn leave_test(
    mut commands: Commands,
    real: Res<Time<Real>>,
    test: Res<LeaveTest>,
    state: Res<State<GameState>>,
    mut next: ResMut<NextState<GameState>>,
    mut step: Local<(usize, f32)>,
    mut exit: MessageWriter<AppExit>,
    entities: Query<Entity>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let now = real.elapsed_secs();
    let shot = |commands: &mut Commands, name: &str| {
        std::fs::create_dir_all(&test.0).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.0.join(name)));
    };
    let (n, at) = &mut *step;
    match (*n, state.get()) {
        (0, GameState::InGame) if now > 8.0 => {
            info!("leave test: {} entities in the match; leaving", entities.iter().count());
            next.set(GameState::Frontend);
            (*n, *at) = (1, now);
        }
        (1, GameState::Frontend) if now - *at > 3.0 => {
            info!("leave test: {} entities in the menu", entities.iter().count());
            shot(&mut commands, "menu.png");
            (*n, *at) = (2, now);
        }
        (2, GameState::Frontend) if now - *at > 1.0 => {
            next.set(GameState::Loading);
            (*n, *at) = (3, now);
        }
        (3, GameState::InGame) if now - *at > 8.0 => {
            info!("leave test: {} entities in the second match", entities.iter().count());
            shot(&mut commands, "second_match.png");
            (*n, *at) = (4, now);
        }
        // Twice round: the second time the class menu was up during the match.
        (4, GameState::InGame) if now - *at > 2.0 && !test.0.join("second_menu.png").exists() => {
            info!("leave test: leaving again");
            next.set(GameState::Frontend);
            (*n, *at) = (5, now);
        }
        (5, GameState::Frontend) if now - *at > 3.0 => {
            shot(&mut commands, "second_menu.png");
            (*n, *at) = (6, now);
        }
        (6, GameState::Frontend) if now - *at > 1.0 => {
            next.set(GameState::Loading);
            (*n, *at) = (7, now);
        }
        (7, GameState::InGame) if now - *at > 8.0 => {
            info!("leave test: {} entities in the third match", entities.iter().count());
            shot(&mut commands, "third_match.png");
            (*n, *at) = (8, now);
        }
        (8, _) if now - *at > 2.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

/// The entities that existed before the match.
#[derive(Resource, Default)]
struct BeforeMatch(HashSet<Entity>);

fn note_entities(mut before: ResMut<BeforeMatch>, all: Query<Entity>) {
    before.0 = all.iter().collect();
    debug!("session: {} entities before the match", before.0.len());
}

#[allow(clippy::type_complexity)]
fn clear_match(
    mut commands: Commands,
    before: Res<BeforeMatch>,
    // Things in the world, on screen or playing: not gamepads that were
    // plugged in meanwhile, or other bookkeeping.
    roots: Query<
        Entity,
        (Without<ChildOf>, Without<Window>, Without<Observer>, Or<(With<Transform>, With<Node>, With<PlaybackSettings>)>),
    >,
    mut time: ResMut<Time<Virtual>>,
) {
    let mut gone = 0;
    for e in roots.iter().filter(|e| !before.0.contains(e)) {
        commands.entity(e).try_despawn();
        gone += 1;
    }
    // The map's zones and the match's state.
    commands.remove_resource::<crate::content::Content>();
    commands.remove_resource::<crate::world::MapInfo>();
    commands.remove_resource::<crate::tdm::MatchState>();
    // A bug report prompt may have paused the game.
    time.unpause();
    info!("left the match: cleared {gone} of {} top-level entities", roots.iter().count());
}
