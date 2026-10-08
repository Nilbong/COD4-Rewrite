//! Call of Duty 4 multiplayer, rewritten on Bevy.
//!
//! Usage: `cod4rw [--map mp_killhouse] [--mode war|dm|dom|sd|koth|sab] [--bots 6] [--skill 0.6] [--bodycam]`
//! Without `--map` the game opens on CoD4's main menu.
//! The game data is read from your CoD4 install (set `COD4_PATH` if it isn't
//! found automatically).

mod atmos;
mod audio;
mod autoshot;
mod bake;
mod bipod;
mod bo1;
mod bodycam;
mod bots;
mod bugreport;
mod characters;
mod collision;
mod combat;
mod content;
mod cover;
mod custom_camos;
mod explosives;
mod fog;
mod first_person;
mod fx;
mod ocean;
mod scatter;
mod gallery;
mod gamepad;
mod grenades;
mod gunmodel;
mod killcam;
mod hud;
mod killstreaks;
mod lightmaps;
mod mapfx;
mod loadout;
mod model_lighting;
mod models;
mod modes;
mod melee;
mod mesh_bounds;
mod net;
mod online;
mod netplay;
mod movement;
mod perf;
mod perks;
mod settings;
mod settings_apply;
mod bindings;
mod pickups;
mod quake;
mod render_scale;
mod reticles;
mod hq;
mod ragdoll;
mod player;
mod clutter;
mod props;
mod rtgi;
mod state;
mod session;
mod splitscreen;
mod supply;
mod tdm;
mod tod_light;
mod terrain;
mod textures;
mod thirdperson;
mod ui;
mod units;
mod viewmodel;
mod vision;
mod wardrobe;
mod waw;
mod weapons;
mod walktest;
mod weather;
mod wet;
mod ssr;
mod pom;
mod vm_lights;
mod window_icon;
mod world;

use avian3d::prelude::*;
use bevy::prelude::*;

/// Avian for colliders and spatial queries only. Nothing in the game is a
/// rigid body (pawns, grenades and projectiles move themselves, casting
/// against colliders), so the solver, contacts, joints, forces, mass
/// properties and interpolation are left out: they cost a few ms a frame
/// over the map's thousands of colliders for nothing.
fn physics_plugins() -> bevy::app::PluginGroupBuilder {
    use avian3d::collision::broad_phase::{BroadPhaseCorePlugin, BvhBroadPhasePlugin};
    use avian3d::collision::narrow_phase::NarrowPhasePlugin;
    use avian3d::dynamics::ccd::CcdPlugin;
    use avian3d::dynamics::integrator::IntegratorPlugin;
    use avian3d::dynamics::joints::JointPlugin;
    use avian3d::dynamics::rigid_body::forces::ForcePlugin;
    use avian3d::dynamics::rigid_body::mass_properties::MassPropertyPlugin;
    use avian3d::dynamics::solver::islands::{IslandPlugin, IslandSleepingPlugin};
    use avian3d::dynamics::solver::joint_graph::JointGraphPlugin;
    use avian3d::dynamics::solver::schedule::SolverSchedulePlugin;
    use avian3d::dynamics::solver::solver_body::SolverBodyPlugin;
    use avian3d::dynamics::solver::SolverPlugin;
    use avian3d::interpolation::PhysicsInterpolationPlugin;
    // `COD4RW_FULL_PHYSICS`: all of avian, to compare.
    if std::env::var_os("COD4RW_FULL_PHYSICS").is_some() {
        return PhysicsPlugins::default().build();
    }
    PhysicsPlugins::default()
        .build()
        .disable::<SolverBodyPlugin>()
        .disable::<SolverSchedulePlugin>()
        .disable::<IntegratorPlugin>()
        .disable::<SolverPlugin>()
        .disable::<CcdPlugin>()
        .disable::<IslandPlugin>()
        .disable::<IslandSleepingPlugin>()
        .disable::<JointGraphPlugin<FixedJoint>>()
        .disable::<JointGraphPlugin<RevoluteJoint>>()
        .disable::<JointGraphPlugin<PrismaticJoint>>()
        .disable::<JointGraphPlugin<DistanceJoint>>()
        .disable::<JointGraphPlugin<SphericalJoint>>()
        .disable::<JointPlugin>()
        .disable::<NarrowPhasePlugin<Collider>>()
        .disable::<BroadPhaseCorePlugin>()
        .disable::<BvhBroadPhasePlugin<()>>()
        .disable::<ForcePlugin>()
        .disable::<MassPropertyPlugin>()
        .disable::<PhysicsInterpolationPlugin>()
}

fn main() -> AppExit {
    // `COD4RW_BAKE=<map>`: re-bake the map's lighting (`bake`) and exit.
    if let Ok(map) = std::env::var("COD4RW_BAKE") {
        return if bake::cli(&map) == std::process::ExitCode::SUCCESS { AppExit::Success } else { AppExit::error() };
    }
    if let Ok(dir) = std::env::var("COD4RW_GALLERY") {
        return gallery::run(dir.into());
    }
    // Black Ops' and World at War's guns (if installed), read while the
    // game starts.
    bo1::preload();
    waw::preload();
    let mut map = String::from("mp_killhouse");
    let mut bots_per_team = 6usize;
    let mut bot_skill = 0.6f32;
    let mut gunplay = bodycam::Gunplay::Cod4;
    let mut mode = modes::GameMode::Tdm;
    let mut spectate = None;
    let mut hardcore = false;
    let mut record = false;
    let mut bot_profile = None;
    // Splitscreen co-op for a run: `kbm,pad,pad` (`crate::splitscreen`).
    let mut splitscreen = std::env::var("COD4RW_SPLITSCREEN").ok();
    // Debug aids drive a match directly, skipping the menus.
    let mut first_state = if std::env::vars().any(|(k, _)| k.starts_with("COD4RW_") && !k.starts_with("COD4RW_PAD") && !k.starts_with("COD4RW_FP_") && !matches!(k.as_str(), "COD4RW_UISHOT" | "COD4RW_UIMENUS" | "COD4RW_UIGAME" | "COD4RW_STATSFILE" | "COD4RW_SUPPLYDROPS" | "COD4RW_SUPPLYFILE" | "COD4RW_SUPPLYTIME" | "COD4RW_ADVERTISE" | "COD4RW_MASTER" | "COD4RW_RAGDOLL" | "COD4RW_NETLOOK" | "COD4RW_TIMELIMIT") && !net::setting(&k)) {
        state::GameState::InGame
    } else {
        state::GameState::Frontend
    };
    let mut args = std::env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--map" => {
                map = args.next().expect("--map needs a name");
                first_state = state::GameState::InGame;
            }
            "--bots" => bots_per_team = args.next().and_then(|s| s.parse().ok()).expect("--bots needs a number"),
            "--skill" => bot_skill = args.next().and_then(|s| s.parse().ok()).expect("--skill needs 0..1"),
            "--bodycam" => gunplay = bodycam::Gunplay::Bodycam,
            "--hardcore" => hardcore = true,
            // CoD4's game type names (`war`, `dm`).
            "--mode" => {
                let name = args.next().expect("--mode needs a game type");
                mode = modes::GameMode::ALL.into_iter().find(|m| m.gametype() == name).expect("--mode: war, dm, dom, sd, koth or sab");
            }
            // A bot's name or role to follow, or nothing for the free camera.
            "--spectate" => spectate = Some(args.next_if(|a| !a.starts_with("--")).unwrap_or_default()),
            "--record" => record = true,
            "--bot-profile" => bot_profile = Some(args.next().expect("--bot-profile needs a file")),
            "--splitscreen" => splitscreen = Some(args.next().expect("--splitscreen needs devices, e.g. kbm,pad")),
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }

    // Background sim runs (`COD4RW_SIM`): a small window that stays out of
    // the way of whatever else is on screen.
    // Debug aid: `COD4RW_RES=1920x1080` sizes the window (for timing).
    let resolution = std::env::var("COD4RW_RES").ok().and_then(|r| {
        let (w, h) = r.split_once('x')?;
        Some(bevy::window::WindowResolution::new(w.parse().ok()?, h.parse().ok()?))
    });
    let window = if std::env::var_os("COD4RW_SIM").is_some() {
        Window {
            title: "CoD4 Rewrite (sim)".into(),
            resolution: resolution.clone().unwrap_or_else(|| (640, 360).into()),
            focused: false,
            // A timing run (`COD4RW_RES`) stays in view: a covered window is held
            // to the compositor's rate.
            window_level: if resolution.is_some() { bevy::window::WindowLevel::Normal } else { bevy::window::WindowLevel::AlwaysOnBottom },
            ..default()
        }
    } else {
        Window { title: "CoD4 Rewrite".into(), resolution: resolution.unwrap_or_default(), ..default() }
    };
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(window),
            ..default()
        }))
        .insert_resource(collision::physics_transform_config())
        .add_systems(avian3d::schedule::PhysicsSchedule, collision::sync_moved_colliders.in_set(avian3d::physics_transform::PhysicsTransformSystems::TransformToPosition))
        .add_plugins((physics_plugins(), state::StatePlugin(first_state), ui::UiPlugin, gunmodel::GunModelPlugin, audio::AudioPlugin, bo1::Bo1Plugin, waw::WawPlugin))
        .insert_resource(Gravity(Vec3::ZERO))
        // The broad phase's and solver's sets the collider trees' systems
        // run in (updating AABBs, starting and finishing optimising), placed
        // in the physics step as the left-out plugins placed them.
        .configure_sets(
            PhysicsSchedule,
            (
                (BroadPhaseSystems::First, BroadPhaseSystems::CollectCollisions, BroadPhaseSystems::Last)
                    .chain()
                    .in_set(PhysicsStepSystems::BroadPhase),
                {
                    use avian3d::dynamics::solver::schedule::SolverSystems as S;
                    (
                        S::PrepareSolverBodies,
                        S::PrepareJoints,
                        S::PrepareContactConstraints,
                        S::PreSubstep,
                        S::Substep,
                        S::PostSubstep,
                        S::Restitution,
                        S::Finalize,
                        S::StoreContactImpulses,
                    )
                        .chain()
                        .in_set(PhysicsStepSystems::Solver)
                },
            ),
        )
        // Anything else left unordered runs single-threaded (avian's
        // executor), so avian's ambiguity check (an error) is quieted;
        // `COD4RW_PHYSICS_STRICT` keeps it, to check.
        .edit_schedule(PhysicsSchedule, |schedule| {
            let strict = std::env::var_os("COD4RW_PHYSICS_STRICT").is_some();
            schedule.set_build_settings(bevy::ecs::schedule::ScheduleBuildSettings {
                ambiguity_detection: if strict { bevy::ecs::schedule::LogLevel::Error } else { bevy::ecs::schedule::LogLevel::Ignore },
                ..default()
            });
        })
        .insert_resource(world::MapName({
            atmos::climate::set_map(&map);
            map
        }))
        .insert_resource(tdm::MatchConfig {
            mode,
            score_limit: mode.default_score_limit(),
            time_limit: mode.default_time_limit() as f32 * 60.0,
            hardcore,
            ..tdm::MatchConfig::even(bots_per_team, bot_skill.clamp(0.0, 1.0))
        })
        .insert_resource(gunplay)
        .insert_resource(splitscreen.map(|s| splitscreen::LocalPlayers::parse(&s)).unwrap_or_default())
        .insert_resource(bots::SpectateArg(spectate))
        .insert_resource(bots::record::RecordArg(record))
        .insert_resource(bots::profile::ProfileArg(bot_profile))
        .add_plugins((
            world::WorldPlugin,
            models::ModelsPlugin,
            viewmodel::ViewModelPlugin,
            thirdperson::ThirdPersonPlugin,
            movement::MovementPlugin,
            combat::CombatPlugin,
            weapons::WeaponsPlugin,
            loadout::LoadoutPlugin,
            player::PlayerPlugin,
            bots::BotsPlugin,
            hud::HudPlugin,
            tdm::TdmPlugin,
            bodycam::BodycamPlugin,
            model_lighting::ModelLightingPlugin,
            supply::SupplyPlugin,
        ))
        // The showcase's time-of-day baked lighting.
        .add_plugins(tod_light::TodLightPlugin)
        .add_plugins((ragdoll::RagdollPlugin, pickups::PickupsPlugin, quake::QuakePlugin, hq::HqPlugin, render_scale::RenderScalePlugin))
        .init_resource::<settings::Settings>()
        .add_plugins(settings_apply::SettingsApplyPlugin)
        // The characters worn: the player's (F5: third person) and the bots'.
        .add_plugins(wardrobe::WardrobePlugin)
        .add_plugins(cover::CoverPlugin)
        .add_plugins(first_person::FirstPersonPlugin)
        .add_plugins((weather::WeatherPlugin, wet::WetPlugin, ssr::SsrPlugin, vm_lights::VmLightsPlugin))
        // G: grenades. 5: equipment and grenade launchers.
        .add_plugins((grenades::GrenadesPlugin, explosives::ExplosivesPlugin, perks::PerksPlugin, killcam::KillcamPlugin))
        .add_plugins((melee::MeleePlugin, props::PropsPlugin, walktest::WalkTestPlugin, fog::FogPlugin, atmos::AtmosPlugin, window_icon::WindowIconPlugin))
        .add_plugins((net::NetPlugin, online::OnlinePlugin, netplay::NetplayPlugin, mesh_bounds::MeshBoundsPlugin))
        .add_systems(Update, collision::ray_test.run_if(state::in_game.and_then(|| std::env::var_os("COD4RW_RAYTEST").is_some())))
        // Muzzle flashes, bullet impacts and blood.
        .add_plugins(fx::FxPlugin)
        .add_plugins((ocean::OceanPlugin, scatter::ScatterPlugin))
        // Each map's film grading and its sun's flare, blind and glare.
        .add_plugins(vision::VisionPlugin)
        // Ray-traced lighting, when the Lighting setting asks for it.
        .add_plugins(rtgi::RtgiPlugin)
        // Hardpoints: the UAV, airstrike and helicopter kill streaks earn.
        .add_plugins(killstreaks::KillstreaksPlugin)
        // Xbox and PlayStation controllers, with aim assist.
        .add_plugins(gamepad::GamepadPlugin)
        // Up to four players on one screen.
        .add_plugins(splitscreen::SplitscreenPlugin)
        // Free-for-all and Domination ([`tdm`] runs the match).
        .add_plugins(modes::ModesPlugin)
        // World at War's bipods: deploy prone or at a ledge.
        .add_plugins(bipod::BipodPlugin)
        // F10: screenshot, pause and describe a bug.
        .add_plugins(bugreport::BugReportPlugin)
        // Leaving a match clears it.
        .add_plugins(session::SessionPlugin)
        // Debug aids, all off unless their environment variable is set.
        .add_plugins((
            autoshot::AutoShotPlugin,
            autoshot::SimLogPlugin,
            autoshot::MoveTestPlugin,
            autoshot::ViewModelTestPlugin,
            autoshot::FramesPlugin,
            perf::PerfPlugin,
            bevy::diagnostic::FrameTimeDiagnosticsPlugin::default(),
        ))
        .run()
}
