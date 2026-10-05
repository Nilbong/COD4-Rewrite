//! The match: spawn the teams, keep score, end and restart matches. Team
//! Deathmatch, and the flow the other game types ([`crate::modes`]) share:
//! in free-for-all the score is each player's own.

use rand::seq::SliceRandom;
use crate::bots::Bot;
use crate::combat::{KillFeed, Killed, Pawn, PawnAssets, Team, hostile, pick_spawn, spawn_pawn};
use crate::modes::GameMode;
use crate::player::LocalPlayer;
use crate::weapons::WeaponInput;
use crate::world::MapInfo;
use bevy::prelude::*;

pub struct TdmPlugin;

impl Plugin for TdmPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(crate::state::GameState::InGame), start_match.in_set(crate::state::Setup::Spawn))
            .add_systems(Update, (score_kills, check_end).run_if(crate::state::in_game));
    }
}

/// CoD4 TDM's defaults: 750 points at 10 per kill, ten minutes.
pub const SCORE_LIMIT: u32 = 75;
pub const TIME_LIMIT: f32 = 10.0 * 60.0;
const POST_MATCH: f32 = 10.0;

/// Is the match Hardcore ([`MatchConfig::hardcore`])?
pub fn hardcore() -> bool {
    HARDCORE.load(std::sync::atomic::Ordering::Relaxed)
}

static HARDCORE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How the next match is set up: from the command line, or the Private
/// Match lobby.
#[derive(Resource, Clone)]
pub struct MatchConfig {
    /// The bots on each side by name: Allies, then Axis.
    pub bots: [Vec<String>; 2],
    pub player_team: Team,
    /// Seconds, and kills to win.
    pub time_limit: f32,
    pub score_limit: u32,
    pub bot_skill: f32,
    pub mode: GameMode,
    /// Hardcore (`scr_hardcore`, CoD4's `hardcore_settings.cfg`): 30
    /// health, friendly fire, little HUD, no killcam, 10 s to respawn.
    pub hardcore: bool,
}

impl MatchConfig {
    /// `bots_per_team` a side, the player being one of the Allies.
    pub fn even(bots_per_team: usize, bot_skill: f32) -> MatchConfig {
        let mut names = (0..).map(bot_name);
        let allies = names.by_ref().take(bots_per_team.saturating_sub(1)).collect();
        let axis = names.take(bots_per_team).collect();
        MatchConfig {
            bots: [allies, axis],
            player_team: Team::Allies,
            time_limit: TIME_LIMIT,
            score_limit: SCORE_LIMIT,
            bot_skill,
            mode: GameMode::Tdm,
            hardcore: false,
        }
    }
}

/// The `i`th bot name ("Bot 17" past the named ones).
pub fn bot_name(i: usize) -> String {
    BOT_NAMES.get(i).map_or_else(|| format!("Bot {}", i + 1), |n| n.to_string())
}

#[derive(Resource)]
pub struct MatchState {
    pub allies: u32,
    pub axis: u32,
    pub started: f32,
    /// Set when the match has ended: (winner, time ended).
    pub ended: Option<(Option<Team>, f32)>,
    /// Seconds, and kills to win.
    pub time_limit: f32,
    pub score_limit: u32,
    pub mode: GameMode,
    /// Free-for-all: the player who won (none on a tie).
    pub winner: Option<String>,
}

impl Default for MatchState {
    fn default() -> Self {
        MatchState {
            allies: 0,
            axis: 0,
            started: 0.0,
            ended: None,
            time_limit: TIME_LIMIT,
            score_limit: SCORE_LIMIT,
            mode: GameMode::Tdm,
            winner: None,
        }
    }
}

impl MatchState {
    pub fn score(&self, team: Team) -> u32 {
        match team {
            Team::Allies => self.allies,
            Team::Axis => self.axis,
        }
    }

    pub fn time_left(&self, now: f32) -> f32 {
        (self.time_limit - (now - self.started)).max(0.0)
    }

    /// Once over, how it went for `pawn`: won, lost, or `None` for a draw.
    pub fn outcome(&self, pawn: &Pawn) -> Option<bool> {
        let (team, _) = self.ended?;
        if self.mode.teams() { team.map(|t| t == pawn.team) } else { self.winner.as_ref().map(|w| *w == pawn.name) }
    }
}

pub const BOT_NAMES: [&str; 16] = [
    "Soap",
    "Gaz",
    "Price",
    "Griggs",
    "Vasquez",
    "Jackson",
    "Paul",
    "Mac",
    "Zakhaev",
    "Al-Asad",
    "Viktor",
    "Kamarov",
    "Nikolai",
    "Pelayo",
    "Wallcroft",
    "Ramirez",
];

fn start_match(
    mut commands: Commands,
    map: Res<MapInfo>,
    assets: Res<PawnAssets>,
    config: Res<MatchConfig>,
    time: Res<Time>,
    profile: Option<Res<crate::bots::profile::PlayerProfile>>,
    spectating: Option<Res<crate::bots::Spectate>>,
    locals: Res<crate::splitscreen::LocalPlayers>,
) {
    // Splitscreen: each player on their side from the lobby (not while
    // spectating; `crate::splitscreen` counted them as the match began).
    let local_count = crate::splitscreen::count();
    let team_of = |slot: usize| if slot == 0 { config.player_team } else { locals.teams.get(slot).copied().unwrap_or(config.player_team) };
    info!(
        "match: {}, you on {:?}; bots {:?} vs {:?}; {:.0} min, first to {}",
        config.mode.name(),
        config.player_team,
        config.bots[0],
        config.bots[1],
        config.time_limit / 60.0,
        config.score_limit
    );
    crate::modes::set_current(config.mode);
    HARDCORE.store(config.hardcore, std::sync::atomic::Ordering::Relaxed);
    if config.hardcore {
        info!("match: hardcore");
    }
    commands.insert_resource(config.mode);
    commands.insert_resource(MatchState {
        started: time.elapsed_secs(),
        // Search and Destroy's rounds keep time instead.
        time_limit: if config.mode.timed() { config.time_limit } else { f32::INFINITY },
        score_limit: config.score_limit,
        mode: config.mode,
        ..default()
    });
    let mut occupied = Vec::new();
    let mut player_spawn = pick_spawn(&map, &crate::combat::Spawning::start(config.player_team, &occupied));
    // Debug: `COD4RW_SPAWN=x,y,z,yaw` (CoD units and degrees) puts the player
    // at a fixed spot, for comparable screenshots.
    if let Some(v) = std::env::var("COD4RW_SPAWN").ok().and_then(|s| {
        let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (v.len() == 4).then_some(v)
    }) {
        player_spawn.pos = crate::units::pos([v[0], v[1], v[2]]);
        player_spawn.yaw = crate::units::yaw_from_cod_degrees(v[3]);
    }
    occupied.push(player_spawn.pos);
    debug!(
        "player spawn at CoD {:?}, yaw {:.0}",
        crate::units::to_cod(player_spawn.pos),
        player_spawn.yaw.to_degrees() + 90.0
    );
    // Spectators watch: a bot plays in their place. In splitscreen each
    // player spawns by the first, Player 1 being the `LocalPlayer`.
    if spectating.is_none() {
        for slot in 0..local_count {
            let spawn = if slot == 0 {
                player_spawn
            } else {
                let s = pick_spawn(&map, &crate::combat::Spawning::start(team_of(slot), &occupied));
                occupied.push(s.pos);
                s
            };
            let name = if local_count > 1 { format!("Player {}", slot + 1) } else { "You".to_owned() };
            let player = spawn_pawn(&mut commands, &assets, &name, team_of(slot), &spawn);
            commands.entity(player).insert((
                crate::splitscreen::LocalSlot(slot),
                crate::splitscreen::PlayerInput::default(),
                WeaponInput::default(),
            ));
            if slot == 0 {
                commands.entity(player).insert(LocalPlayer);
            }
        }
        if local_count > 1 {
            info!("match: splitscreen, {local_count} players: {:?} on {:?}", locals.devices, (0..local_count).map(team_of).collect::<Vec<_>>());
        }
    }

    // Debug: frozen models in front of the player for screenshots.
    if std::env::var_os("COD4RW_DUMMY").is_some() {
        let fwd = Quat::from_rotation_y(player_spawn.yaw) * Vec3::NEG_Z;
        let right = Quat::from_rotation_y(player_spawn.yaw) * Vec3::X;
        let lineup: [(Team, Vec3, f32, &str, Option<Vec3>, crate::movement::Stance); 4] = [
            (Team::Axis, fwd * 3.0 - right * 1.4, std::f32::consts::PI, "dummy axis", None, crate::movement::Stance::Stand),
            (Team::Allies, fwd * 3.0 - right * 0.2, std::f32::consts::PI, "dummy allies", None, crate::movement::Stance::Stand),
            (Team::Axis, fwd * 5.5 - right * 2.0, -std::f32::consts::FRAC_PI_2, "dummy runner", Some(-right * crate::units::u(190.0)), crate::movement::Stance::Stand),
            (Team::Allies, fwd * 5.5 - right * 0.2, std::f32::consts::PI, "dummy crouch", None, crate::movement::Stance::Crouch),
        ];
        for (team, offset, turn, name, vel, stance) in lineup {
            let sp = crate::world::SpawnPoint { pos: player_spawn.pos + offset, yaw: player_spawn.yaw + turn, kind: player_spawn.kind };
            let d = spawn_pawn(&mut commands, &assets, name, team, &sp);
            let mut mover = crate::movement::Mover::default();
            mover.velocity = vel.unwrap_or(Vec3::ZERO);
            mover.stance = stance;
            mover.on_ground = true;
            commands.entity(d).insert((crate::movement::Frozen, mover));
            if name == "dummy allies" {
                commands.entity(d).insert(WeaponInput { ads: true, ..default() });
            }
        }
    }

    let teams = [Team::Allies, Team::Axis];
    let names: Vec<Vec<String>> = teams
        .iter()
        .enumerate()
        .map(|(side, &team)| {
            let mut names = config.bots[side].clone();
            // Spectating: a bot takes the player's place.
            if spectating.is_some() && team == config.player_team {
                let taken: Vec<&String> = config.bots.iter().flatten().collect();
                if let Some(free) = (0..).map(bot_name).find(|n| !taken.contains(&n)) {
                    names.push(free);
                }
            }
            names
        })
        .collect();
    // A mixed lobby, the sides evened out (the player counting as one at
    // the setting).
    let human = |team: Team| team == config.player_team || (local_count > 1 && locals.teams.contains(&team));
    let playing = |team: Team| if spectating.is_none() && human(team) { config.bot_skill } else { 0.0 };
    let skills = crate::bots::deal_skills(
        config.bot_skill,
        [names[0].len(), names[1].len()],
        teams.map(playing),
        &mut rand::rng(),
    );
    for (side, team) in teams.into_iter().enumerate() {
        let count = names[side].len();
        let mut names = names[side].clone().into_iter();
        let mut skills = skills[side].iter().copied();
        // Spread each team's bots across the map, left to right.
        let mut lanes: Vec<f32> =
            (0..count).map(|k| if count < 2 { 0.0 } else { -0.8 + 1.6 * k as f32 / (count - 1) as f32 }).collect();
        lanes.shuffle(&mut rand::rng());
        for lane in lanes {
            let spawn = pick_spawn(&map, &crate::combat::Spawning::start(team, &occupied));
            occupied.push(spawn.pos);
            let name = names.next().unwrap_or_else(|| "Bot".into());
            let bot = spawn_pawn(&mut commands, &assets, &name, team, &spawn);
            let skill = skills.next().unwrap_or(config.bot_skill);
            commands.entity(bot).insert((Bot::new(skill, profile.as_deref()).with_lane(lane), WeaponInput::default()));
        }
    }
}

fn score_kills(mut killed: MessageReader<Killed>, pawns: Query<&Pawn>, mut state: ResMut<MatchState>) {
    for k in killed.read() {
        if state.ended.is_some() {
            continue;
        }
        let (Some(attacker), Ok(victim)) = (k.attacker, pawns.get(k.victim)) else { continue };
        let Ok(att) = pawns.get(attacker) else { continue };
        // Only Team Deathmatch scores kills for the team (free-for-all's
        // score is each player's kills, Domination's its flags).
        if attacker == k.victim || !hostile(att, victim) || state.mode != GameMode::Tdm {
            continue;
        }
        match att.team {
            Team::Allies => state.allies += 1,
            Team::Axis => state.axis += 1,
        }
    }
}

fn check_end(
    mut commands: Commands,
    time: Res<Time>,
    mut state: ResMut<MatchState>,
    mut feed: ResMut<KillFeed>,
    mut pawns: Query<(Entity, &mut Pawn)>,
) {
    let now = time.elapsed_secs();
    match state.ended {
        None => {
            let limit = state.score_limit;
            // Free-for-all: the best player's kills, and who that is unless tied.
            let mut ranked: Vec<(u32, &str)> = pawns.iter().map(|(_, p)| (p.kills, p.name.as_str())).collect();
            ranked.sort_by(|a, b| b.0.cmp(&a.0));
            let top = ranked.first().map_or(0, |r| r.0);
            let leader = (ranked.len() == 1 || ranked.get(1).is_some_and(|r| r.0 < top)).then(|| ranked[0].1.to_owned());
            let over = if state.mode.teams() { state.allies >= limit || state.axis >= limit } else { top >= limit };
            if over || state.time_left(now) <= 0.0 {
                let winner = match state.allies.cmp(&state.axis) {
                    _ if !state.mode.teams() => None,
                    std::cmp::Ordering::Greater => Some(Team::Allies),
                    std::cmp::Ordering::Less => Some(Team::Axis),
                    std::cmp::Ordering::Equal => None,
                };
                if !state.mode.teams() {
                    info!("match: {} wins with {top}", leader.as_deref().unwrap_or("nobody (a tie)"));
                    state.winner = leader;
                }
                state.ended = Some((winner, now));
                for (e, _) in &pawns {
                    commands.entity(e).insert(crate::movement::Frozen);
                }
            }
        }
        Some((_, at)) if now - at > POST_MATCH => {
            // New round: reset scores and kill everyone so they respawn.
            *state = MatchState {
                started: now,
                time_limit: state.time_limit,
                score_limit: state.score_limit,
                mode: state.mode,
                ..default()
            };
            feed.entries.clear();
            for (e, mut p) in &mut pawns {
                p.kills = 0;
                p.deaths = 0;
                p.assists = 0;
                commands
                    .entity(e)
                    .remove::<crate::movement::Frozen>()
                    .insert(crate::combat::Dead { respawn_at: now, killer: None });
            }
        }
        _ => {}
    }
}
