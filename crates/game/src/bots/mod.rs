//! Bots that play like people.
//!
//! - [`nav`]: a walkable graph of the map, flood-filled from the spawns over
//!   the real collision, with A* routes.
//! - [`perception`]: what each bot knows: enemies it has noticed (sight takes
//!   time, especially at the edge of view), gunfire it heard, who shot it,
//!   teammates' callouts, and fading memories.
//! - [`aim`]: a model of a hand on a mouse: reaction time, Fitts'-law flicks
//!   that over- and undershoot, corrective flicks, laggy tracking, delayed
//!   recoil control and tremor. No lock-on.
//! - [`tactical`]: what a player learns about a map: spawn areas, lanes,
//!   holding spots, where enemies will spawn next, and (per team, this match)
//!   where enemies keep showing up.
//! - This module: personality, role and tactics. Several times a second a
//!   bot scores its options (fight, take cover, investigate, flank, hold a
//!   spot, hunt) and then moves with the same keys a player has.

pub mod aim;
mod class;
mod equipment;
pub mod demos;
mod grenade;
mod hardpoints;
mod objective;
pub mod lab;
mod resupply;
pub mod learned;
mod lookstat;
pub mod motion;
pub mod nets;
pub mod nav;
pub mod perception;
pub mod profile;
pub mod record;
pub mod spectate;
pub mod tactical;

pub use spectate::{Spectate, SpectateArg};

use crate::collision;
use crate::combat::{Damage, Dead, Health, Killed, Pawn, Team};
use crate::movement::{MoveInput, Mover, Stance, ViewAngles};
use crate::units::u;
use crate::weapons::{ShotFired, WeaponInput, WeaponState};
use crate::world::{MapInfo, SpawnKind};
use aim::{gaussian, AimGoal, AimProfile, AimState};
use avian3d::prelude::*;
use bevy::prelude::*;
use nav::NavGraph;
use perception::{Callouts, Contact, Knowledge, PawnView, Source};
use profile::{MoveStyle, PlayerProfile};
use rand::Rng;
use std::collections::HashMap;
use tactical::{Front, Lateral, TacticalMap, TeamIntel};

pub struct BotsPlugin;

impl Plugin for BotsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Callouts>()
            .init_resource::<TeamIntel>()
            .init_resource::<NavTrouble>()
            .add_systems(
                Update,
                (class::class_bots, floor_line, build_nav, build_tactics, check_objectives, hardpoints::hardpoints, grenade::plan_grenades, equipment::plan_gear, think, resupply::resupply, knife, objective::use_objectives)
                    .chain()
                    .before(crate::movement::MovementSet)
                    .run_if(crate::state::in_game),
            )
            // The graph belongs to the map: rebuild it for the next one.
            .add_systems(
                OnExit(crate::state::GameState::InGame),
                |mut commands: Commands,
                 mut callouts: ResMut<Callouts>,
                 mut intel: ResMut<TeamIntel>,
                 mut trouble: ResMut<NavTrouble>| {
                    commands.remove_resource::<NavGraph>();
                    commands.remove_resource::<TacticalMap>();
                    callouts.0.clear();
                    intel.clear();
                    trouble.0.clear();
                },
            );
        if std::env::var_os("COD4RW_SIM").is_some() {
            app.add_systems(Update, (debug_log, behaviour_log, grenade::tally).run_if(crate::state::in_game));
        }
        record::setup(app);
        lookstat::setup(app);
        profile::setup(app);
        demos::setup(app);
        spectate::setup(app);
    }
}

/// CoD4's bot difficulties and the skill each stands for (the lobby's
/// settings).
pub const TIERS: [(&str, f32); 4] = [("Recruit", 0.25), ("Regular", 0.5), ("Hardened", 0.7), ("Veteran", 0.9)];
/// Skill between one difficulty and the next, about.
const TIER_STEP: f32 = 0.2;

/// The difficulty a skill is nearest.
pub fn tier_name(skill: f32) -> &'static str {
    TIERS.iter().min_by(|a, b| (a.1 - skill).abs().total_cmp(&(b.1 - skill).abs())).map_or("Regular", |t| t.0)
}

/// Skills for `n` bots in a match set to `skill`. Like a public lobby
/// they're mixed: most play at the setting, some a difficulty or two
/// better or worse.
pub fn lobby_skills(skill: f32, n: usize, rng: &mut impl Rng) -> Vec<f32> {
    // Difficulties away from the setting, and how often.
    const MIX: [(f32, f32); 5] = [(-2.0, 0.06), (-1.0, 0.22), (0.0, 0.44), (1.0, 0.22), (2.0, 0.06)];
    let options: Vec<(f32, f32)> =
        MIX.iter().map(|&(d, w)| (skill + d * TIER_STEP, w)).filter(|&(s, _)| (0.04..=1.0).contains(&s)).collect();
    let total: f32 = options.iter().map(|o| o.1).sum();
    (0..n)
        .map(|_| {
            let mut r = rng.random::<f32>() * total;
            options.iter().find(|o| {
                r -= o.1;
                r <= 0.0
            })
            .map_or(skill, |o| o.0)
        })
        .collect()
}

/// [`lobby_skills`] for two sides of `counts` bots, dealt so the sides come
/// out even: the best left goes to the side with less skill so far (with
/// room). `already` is skill a side has before its bots (the player).
pub fn deal_skills(skill: f32, counts: [usize; 2], already: [f32; 2], rng: &mut impl Rng) -> [Vec<f32>; 2] {
    let mut all = lobby_skills(skill, counts[0] + counts[1], rng);
    all.sort_by(|a, b| b.total_cmp(a));
    let mut sides: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
    let mut sum = already;
    for s in all {
        let room = |k: usize| sides[k].len() < counts[k];
        let k = if !room(0) || (room(1) && sum[1] < sum[0]) { 1 } else { 0 };
        sides[k].push(s);
        sum[k] += s;
    }
    for side in &mut sides {
        rand::seq::SliceRandom::shuffle(side.as_mut_slice(), rng);
    }
    sides
}

/// A rank to go with `skill`: better players have played more, so mostly
/// higher, with plenty of spread (no prestige).
fn rank_for(skill: f32, rng: &mut impl Rng) -> (i32, i32) {
    let rank = (4.0 + 50.0 * skill + 9.0 * gaussian(rng)).round().clamp(0.0, 54.0) as i32;
    (rank, 0)
}

/// Kinds of gun, for what a bot carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GunKind {
    Smg,
    Rifle,
    Lmg,
    Shotgun,
    Sniper,
}

/// CoD4's guns of each kind, favourites first.
const GUNS: [(GunKind, &[&str]); 5] = [
    (GunKind::Smg, &["mp5", "p90", "uzi", "ak74u", "skorpion"]),
    (GunKind::Rifle, &["m16", "ak47", "m4", "g36c", "g3", "m14", "mp44"]),
    (GunKind::Lmg, &["rpd", "saw", "m60e4"]),
    (GunKind::Shotgun, &["winchester1200", "m1014"]),
    (GunKind::Sniper, &["m40a3", "remington700", "dragunov", "m21"]),
];

/// A gun to suit a personality: how likely each kind is, then one of its
/// guns (the favourites more often).
fn choose_gun(p: &Personality, rng: &mut impl Rng) -> (&'static str, GunKind) {
    // Weights: SMG, rifle, LMG, shotgun, sniper.
    let weights = if p.aggression > 0.65 {
        [0.55, 0.30, 0.0, 0.15, 0.0]
    } else if p.patience > 0.6 && p.aggression < 0.5 {
        [0.2, 0.45, 0.2, 0.0, 0.15]
    } else {
        [0.3, 0.55, 0.15, 0.0, 0.0]
    };
    let mut r = rng.random::<f32>() * weights.iter().sum::<f32>();
    let k = weights.iter().position(|w| {
        r -= w;
        r <= 0.0
    });
    let (kind, guns) = GUNS[k.unwrap_or(1)];
    // Earlier in the list, more likely: weights n, n-1, ..., 1.
    let n = guns.len();
    let mut pick = rng.random_range(0..n * (n + 1) / 2);
    let i = (0..n).find(|&i| {
        let w = n - i;
        if pick < w {
            true
        } else {
            pick -= w;
            false
        }
    });
    (guns[i.unwrap_or(0)], kind)
}

/// How a bot likes to play.
#[derive(Clone, Debug)]
struct Personality {
    /// 0 = careful, 1 = rushes.
    aggression: f32,
    /// How long it will hold an angle.
    patience: f32,
    /// Distance it likes to fight at.
    preferred_range: f32,
    /// Chance of going for the head when it has time.
    head_bias: f32,
    /// How readily it sprints between fights.
    sprinter: f32,
}

/// What a bot does for its team when nothing is happening.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// Holds strong spots covering the lanes enemies come through.
    Anchor,
    /// Pushes toward where enemies are about to spawn, along the main lanes.
    Rusher,
    /// Heads the same way by side routes.
    Flanker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Hunt,
    Engage,
    Cover,
    Investigate,
    Flank,
    /// Wait around a remembered enemy.
    Hold,
    /// Hold one of the map's strong spots, aiming down its lanes.
    Post,
    /// Play the game mode's objective ([`objective`]): a flag, the bomb.
    Objective,
}

/// Someone out of view who just shot us, or whose footsteps came up behind
/// us: once the hit or the sound has registered, flick round to them, as a
/// player does on a damage indicator. Finding them there is then quick, and
/// so is shooting.
#[derive(Clone, Copy)]
struct Alarm {
    target: Entity,
    /// When the flick starts (a reaction time after the hit or sound).
    at: f32,
    /// When to give up looking.
    until: f32,
    /// Where to look (where they'd come into view, if they're behind
    /// something), and when to work it out again.
    point: Option<Vec3>,
    next_point: f32,
}

/// Reaction to a hit, gunfire or footsteps, as a multiple of the reaction
/// to an enemy seen; how often sounds prompt a turn (base, plus this much
/// more for the best bots).
const ALARM_REACTION: f32 = 1.0;
const SOUND_REACTION: f32 = 1.3;
const FOOTSTEP_TURN: (f32, f32) = (0.35, 0.5);
const GUNFIRE_TURN: (f32, f32) = (0.6, 0.35);
/// How long an alarm lasts after the flick starts.
const ALARM_FOR: f32 = 1.5;
/// Shooting someone found by turning to an alarm takes this share of the
/// usual reaction time: the decision is already made.
const ALARMED_REACTION: f32 = 0.35;

/// Holding a position: what to watch.
struct HoldPlan {
    /// Index into `TacticalMap::spots` when holding a known spot.
    spot: Option<usize>,
    /// Points to watch (on the floor), most dangerous first.
    angles: Vec<Vec3>,
    angle_i: usize,
    next_switch: f32,
    /// Standing, crouched or prone while holding (see [`hold_stance`]).
    stance: Stance,
    /// Holding with sights up (real players mostly hold from the hip and
    /// aim down sights when someone shows up).
    ads: bool,
}

/// Nav points bots got stuck trying to reach this match, and how often.
/// Routes avoid them more each time, so a ledge that can't really be
/// climbed or a gap that's too tight stops catching bots.
#[derive(Resource, Default)]
pub struct NavTrouble(HashMap<u32, u32>);

/// Map knowledge a bot reasons with this frame.
struct TacCtx<'a> {
    tactics: Option<&'a TacticalMap>,
    intel: &'a TeamIntel,
    front: Option<Front>,
    /// Where enemies of our team will likely spawn next, weighted.
    enemy_spawns: &'a [(Vec3, f32)],
    team: Team,
    /// Holding spots teammates have claimed.
    taken: Vec<usize>,
    /// What living teammates are up to.
    mates: Vec<&'a MatePlan>,
    /// Left/right across the map, looking from our spawns to theirs.
    lateral: Option<Lateral>,
    trouble: &'a HashMap<u32, u32>,
    motion: Option<&'a motion::Library>,
    /// The game mode's objectives (Domination's flags).
    objectives: Option<&'a crate::modes::Objectives>,
}

impl TacCtx<'_> {
    /// How well `p` suits a bot that likes the `lane` side of the map: 1 on
    /// its side, falling off away from it.
    fn lane_fit(&self, lane: f32, p: Vec3) -> f32 {
        self.lateral.map_or(1.0, |l| (-2.0 * (l.of(p) - lane).abs()).exp())
    }

    /// Domination: how well `p` suits holding or hunting, by the flags in
    /// play (see [`objective::flag_fit`]); 1 in other modes.
    fn flag_fit(&self, p: Vec3) -> f32 {
        self.objectives.map_or(1.0, |o| objective::flag_fit(o, self.team, p))
    }

    /// Where teammates are and are going.
    fn mate_points(&self) -> Vec<Vec3> {
        self.mates.iter().flat_map(|m| std::iter::once(m.feet).chain(m.dest)).collect()
    }
}

/// A bot's movement plan as its teammates see it, so a team spreads out
/// instead of all taking one route to one place.
struct MatePlan {
    entity: Entity,
    team: Team,
    feet: Vec3,
    vel: Vec3,
    dest: Option<Vec3>,
    mode: Mode,
    focus: Option<Entity>,
    /// Coarse cells the rest of its route passes through.
    route: std::collections::HashSet<Cell>,
}

/// A coarse map cell, about a corridor wide.
type Cell = (i32, i32, i32);

fn cell(p: Vec3) -> Cell {
    ((p.x / u(160.0)).floor() as i32, (p.y / u(100.0)).floor() as i32, (p.z / u(160.0)).floor() as i32)
}

impl MatePlan {
    fn of(entity: Entity, bot: &Bot, team: Team, feet: Vec3, vel: Vec3, nav: Option<&NavGraph>) -> Self {
        let route = match nav {
            Some(nav) => bot.path.iter().skip(bot.path_i).map(|&i| cell(nav.nodes[i as usize].pos)).collect(),
            None => Default::default(),
        };
        MatePlan { entity, team, feet, vel, dest: bot.dest, mode: bot.mode, focus: bot.focus, route }
    }
}

struct Engagement {
    target: Entity,
    /// When the first aiming move may start (reaction time).
    react_at: f32,
    aim_head: bool,
    crouch: bool,
    /// Last time the target was in view.
    last_visible: f32,
    /// Debug stats: when the target came into view, and our first shot.
    seen_at: f32,
    first_shot: Option<f32>,
    /// Dropped prone to shoot (a "dropshot").
    drop: bool,
    /// Aiming down sights or from the hip; the roll that decides it (kept a
    /// while) and when to roll again.
    ads: bool,
    ads_next: f32,
    ads_roll: f32,
    /// Moving or not, and since when.
    moving: bool,
    moving_since: f32,
}

#[derive(Component)]
pub struct Bot {
    pub skill: f32,
    /// The rank (0-based) and prestige it shows on the scoreboard, going
    /// with its skill.
    pub rank: (i32, i32),
    /// Reloads only once it's quiet (weaker players reload with enemies
    /// about).
    careful_reload: bool,
    /// Not pre-aiming anyone until (weaker players often just look where
    /// they're going).
    no_preaim_until: f32,
    /// Its whole class: that gun with an attachment, a pistol, perks,
    /// grenades and equipment ([`class`]).
    pub class: crate::loadout::ClassLoadout,
    personality: Personality,
    role: Role,
    aim_profile: AimProfile,
    aim: AimState,
    /// Owns this bot's route searches (spread over frames, one at a time:
    /// [`nav::NavGraph::path_sliced`]); and whether one is under way.
    route_id: u64,
    routing: bool,
    /// The learned aim model's state ([`nets`], with `COD4RW_NETAIM`).
    net_aim: nets::NetAimState,
    /// Mean reaction time in seconds.
    reaction: f32,
    know: Knowledge,
    mode: Mode,
    mode_since: f32,
    next_decide: f32,
    last_look: f32,
    engagement: Option<Engagement>,
    /// Who/where the current non-fight mode is about.
    focus: Option<Entity>,
    // Navigation.
    dest: Option<Vec3>,
    path: Vec<u32>,
    path_i: usize,
    steer: Option<Vec3>,
    next_shortcut: f32,
    next_repath: f32,
    progress_pos: Vec3,
    progress_time: f32,
    stuck: u8,
    hold_until: f32,
    hold: Option<HoldPlan>,
    /// The spot held last, to move on from after a kill.
    last_spot: Option<usize>,
    // Looking around while moving.
    glance: Option<(Vec3, f32)>,
    /// Someone out of view just shot us or ran up behind us.
    alarm: Option<Alarm>,
    /// Where a remembered enemy would come into view, and when to re-check.
    preaim: Option<(Entity, Vec3)>,
    next_preaim: f32,
    next_glance: f32,
    /// An angle being checked while moving, and until when.
    check: Option<(Vec3, f32)>,
    next_check: f32,
    /// Peeking out of cover: (side, phase end, coming back).
    peek: Option<(f32, f32, bool)>,
    next_peek: f32,
    /// Since when we've been letting a teammate ahead get some distance.
    yield_from: Option<f32>,
    /// A teammate we found ourselves following, and until when we route
    /// around the way they're going.
    leader: Option<(Entity, f32)>,
    /// How it moves in a fight.
    style: MoveStyle,
    /// What set this frame's view target (for the sim log), and last frame's.
    look_src: &'static str,
    prev_look: &'static str,
    /// Where to look along the route, and until when that holds.
    ahead: Option<(Vec3, f32)>,
    /// Move keys last frame, for not flipping them on small view changes.
    last_keys: (f32, f32),
    /// Where we last decided whether to crouch while holding, and the answer.
    hold_crouch: Option<(Vec3, Stance)>,
    /// What last asked for a jump (for the behaviour log).
    jump_src: &'static str,
    /// Where a mantle on the route ends, while at its foot (face it).
    mantle_to: Option<Vec3>,
    /// Out of fights people don't hold their view level: a habit of
    /// looking a little down (radians, up positive), and a slow wander
    /// about it, updated last at the time given.
    pitch_habit: f32,
    pitch_drift: f32,
    drift_at: f32,
    /// A grenade being thrown, and when to think about another.
    nade: Option<grenade::Nade>,
    next_nade: f32,
    /// Running from a live grenade, and the last one noticed.
    flee: Option<grenade::Flee>,
    flee_seen: Option<Entity>,
    /// Equipment being used, when to think about it next, and where the
    /// last claymore went down.
    gear: Option<equipment::Gear>,
    next_gear: f32,
    claymore_at: Option<Vec3>,
    /// Where an enemy helicopter in sight is.
    heli: Option<Vec3>,
    /// Enemy claymores known about (entity, where, facing), to route round,
    /// and those already looked at.
    claymores: Vec<(Entity, Vec3, Vec3)>,
    claymore_checked: Vec<Entity>,
    /// When to think about knifing someone next.
    next_melee: f32,
    /// The mode's objective being played ([`Mode::Objective`]).
    goal: Option<objective::Goal>,
    /// Objectives with no route there from where the bot was, and until
    /// when to leave them be (rather than trying the same route forever).
    blocked: Vec<(objective::Goal, f32)>,
    /// Stepping aside to get unstuck: which way, and until when.
    unstick: Option<(f32, f32)>,
    /// Stopped on the way to check an angle, until.
    pause_until: f32,
    /// A semi-automatic's trigger can be pulled again from.
    next_pull: f32,
    /// A nav point we just got stuck trying to reach, to report.
    stuck_at: Option<u32>,
    /// The motion-matching clip being played.
    playing: Option<motion::Playing>,
    /// View turn rate last frame (radians/s).
    turn_rate: f32,
    /// Whether the matched player was sprinting.
    matched_sprint: bool,
    /// The side of the map this bot likes to play, -1 (left) to 1 (right)
    /// looking from our spawns towards theirs; teammates get different ones.
    lane: f32,
    /// Whether to sprint when travelling, and until when that holds.
    sprint_choice: bool,
    next_sprint_choice: f32,
    // Fighting.
    strafe: f32,
    /// Forward input while fighting, for styles that push or back off.
    advance: f32,
    next_strafe: f32,
    crouch_tap: bool,
    next_crouch_tap: f32,
    burst_until: f32,
    next_burst: f32,
    last_feet: Vec3,
}

impl Bot {
    /// What the bot is up to, in a line (for bug reports).
    pub fn summary(&self) -> String {
        let dest = self.dest.map_or("no destination".to_string(), |d| {
            format!("heading for {:?}", crate::units::to_cod(d).map(|v| v.round() as i32))
        });
        format!(
            "{:?} ({:?}, {} {:.2}), {dest}, looking via {}{}",
            self.mode,
            self.role,
            tier_name(self.skill),
            self.skill,
            self.look_src,
            if self.stuck > 0 { format!(", stuck x{}", self.stuck) } else { String::new() }
        )
    }

    /// This bot's favourite side of the map (see [`Bot::lane`]).
    pub fn with_lane(mut self, lane: f32) -> Self {
        self.lane = lane.clamp(-1.0, 1.0);
        self
    }

    /// A bot of roughly `skill` (0..1). With a recorded player's `profile`
    /// it plays like them, `skill` 0.5 being their level.
    pub fn new(skill: f32, profile: Option<&PlayerProfile>) -> Self {
        let mut rng = rand::rng();
        // A touch either side of the skill it was dealt ([`lobby_skills`]).
        let skill = (skill + rng.random_range(-0.05..0.05)).clamp(0.0, 1.0);
        let mut personality = Personality {
            aggression: rng.random_range(0.15..0.95),
            patience: rng.random_range(0.1..0.9),
            preferred_range: u(rng.random_range(350.0..1400.0)),
            head_bias: (skill * rng.random_range(0.2..0.8)).min(0.8),
            sprinter: rng.random_range(0.4..1.0),
        };
        let mut aim_profile = AimProfile::for_skill(skill, &mut rng);
        let mut reaction = (0.32 - 0.14 * skill) * rng.random_range(0.9..1.1);
        let mut style = MoveStyle::default();
        if let Some(p) = profile {
            // Their timings, scaled by how far this bot's skill is from 0.5.
            let slow = 1.0 + (0.5 - skill) * 0.8;
            personality = p.personality(personality, skill, &mut rng);
            // What the profile doesn't measure comes from a matching skill.
            let like = p.fitts_b.map_or(0.5, |b| ((0.11 - b - profile::FITTS_OFFSET.1) / 0.05).clamp(0.0, 1.0));
            aim_profile = AimProfile::for_skill((like + skill - 0.5).clamp(0.0, 1.0), &mut rng);
            let vary = |rng: &mut rand::rngs::ThreadRng, v: f32| v * slow * rng.random_range(0.9..1.1);
            // Measured values convert to settings that measure the same
            // (see the offsets in `profile`).
            if let Some(a) = p.fitts_a {
                aim_profile.fitts_a = vary(&mut rng, (a - profile::FITTS_OFFSET.0).clamp(0.02, 0.4));
            }
            if let Some(b) = p.fitts_b {
                aim_profile.fitts_b = vary(&mut rng, (b + profile::FITTS_OFFSET.1).clamp(0.02, 0.4));
            }
            if let Some(g) = p.gain_bias {
                aim_profile.gain_bias = g.clamp(0.6, 1.3);
            }
            if let Some(g) = p.gain_sd {
                aim_profile.gain_sd = vary(&mut rng, (g * profile::GAIN_SD_SCALE).clamp(0.01, 0.5));
            }
            if let Some(r) = p.reaction {
                reaction = vary(&mut rng, (r - profile::REACTION_OFFSET).clamp(0.08, 1.0));
            }
            style = MoveStyle::from_profile(p);
        }
        // Players have favourite guns: rushers SMGs and shotguns, patient
        // ones rifles, machine guns and the odd sniper rifle. The gun sets
        // the range they like to fight at, and snipers always scope in.
        let (gun, kind) = choose_gun(&personality, &mut rng);
        personality.preferred_range = u(match kind {
            GunKind::Shotgun => rng.random_range(200.0..450.0),
            GunKind::Smg => rng.random_range(350.0..800.0),
            GunKind::Rifle => rng.random_range(700.0..1400.0),
            GunKind::Lmg => rng.random_range(800.0..1500.0),
            GunKind::Sniper => rng.random_range(1500.0..2500.0),
        });
        match kind {
            GunKind::Sniper => style.ads_range = 0.0,
            GunKind::Shotgun => style.ads_range = style.ads_range.max(u(900.0)),
            _ => {}
        }
        let rank = rank_for(skill, &mut rng);
        let class = class::class_for(gun, kind, &personality, skill, rank, &mut rng);
        let role = if personality.aggression > 0.72 {
            Role::Rusher
        } else if personality.patience > 0.5 && personality.aggression < 0.6 {
            Role::Anchor
        } else {
            Role::Flanker
        };
        Bot {
            skill,
            rank,
            class,
            careful_reload: rng.random::<f32>() < 1.0 - 1.5 * (0.6 - skill).max(0.0),
            no_preaim_until: 0.0,
            role,
            aim_profile,
            aim: AimState::default(),
            route_id: rand::random(),
            routing: false,
            net_aim: nets::NetAimState::default(),
            reaction,
            personality,
            know: Knowledge::default(),
            mode: Mode::Hunt,
            mode_since: 0.0,
            next_decide: 0.0,
            last_look: 0.0,
            engagement: None,
            focus: None,
            dest: None,
            path: Vec::new(),
            path_i: 0,
            steer: None,
            next_shortcut: 0.0,
            next_repath: 0.0,
            progress_pos: Vec3::ZERO,
            progress_time: 0.0,
            stuck: 0,
            hold_until: 0.0,
            hold: None,
            last_spot: None,
            glance: None,
            next_glance: 0.0,
            alarm: None,
            preaim: None,
            next_preaim: 0.0,
            check: None,
            next_check: 0.0,
            peek: None,
            next_peek: 0.0,
            yield_from: None,
            leader: None,
            style,
            look_src: "none",
            prev_look: "none",
            ahead: None,
            last_keys: (0.0, 0.0),
            hold_crouch: None,
            jump_src: "",
            mantle_to: None,
            pitch_habit: (-3.5 + 1.5 * gaussian(&mut rng)).to_radians(),
            pitch_drift: 0.0,
            drift_at: 0.0,
            nade: None,
            next_nade: 0.0,
            flee: None,
            flee_seen: None,
            gear: None,
            next_gear: 0.0,
            claymore_at: None,
            heli: None,
            claymores: Vec::new(),
            claymore_checked: Vec::new(),
            next_melee: 0.0,
            goal: None,
            blocked: Vec::new(),
            unstick: None,
            pause_until: 0.0,
            next_pull: 0.0,
            stuck_at: None,
            playing: None,
            turn_rate: 0.0,
            matched_sprint: false,
            lane: 0.0,
            sprint_choice: false,
            next_sprint_choice: 0.0,
            strafe: 0.0,
            advance: 0.0,
            next_strafe: 0.0,
            crouch_tap: false,
            next_crouch_tap: 0.0,
            burst_until: 0.0,
            next_burst: 0.0,
            last_feet: Vec3::splat(f32::MAX),
        }
    }

    fn set_mode(&mut self, mode: Mode, now: f32) {
        if self.mode != mode {
            self.mode = mode;
            self.mode_since = now;
            self.path.clear();
            self.dest = None;
            self.glance = None;
            self.peek = None;
            if !matches!(mode, Mode::Hold | Mode::Post) {
                self.hold = None;
            }
        }
    }

    fn go_to(&mut self, dest: Vec3) {
        if self.dest.is_none_or(|d| d.distance(dest) > u(48.0)) {
            self.dest = Some(dest);
            self.path.clear();
        }
    }
}

/// Build the navigation graph once the map's collision is queryable.
fn build_nav(
    mut commands: Commands,
    spatial: SpatialQuery,
    map: Option<Res<MapInfo>>,
    nav: Option<Res<NavGraph>>,
    mantles: Query<(Entity, &Collider, &collision::MantleSurface)>,
) {
    let (Some(map), None) = (map, nav) else { return };
    let filter = collision::movement_filter();
    let ready = map.spawns.iter().take(3).any(|s| {
        spatial.cast_ray(s.pos + Vec3::Y * u(40.0), Dir3::NEG_Y, u(200.0), true, &filter).is_some()
    });
    if !ready {
        return;
    }
    let t0 = std::time::Instant::now();
    let seeds: Vec<Vec3> = map.spawns.iter().map(|s| s.pos).collect();
    let boxes: Vec<(Vec3, Vec3)> = mantles
        .iter()
        .map(|(_, c, _)| {
            let b = c.aabb(Vec3::ZERO, Quat::IDENTITY);
            (b.min, b.max)
        })
        .collect();
    let over: Vec<Entity> = mantles.iter().filter(|(_, _, s)| s.over).map(|(e, _, _)| e).collect();
    let graph = NavGraph::build(&spatial, &seeds, &boxes, &over);
    let links: usize = graph.nodes.iter().map(|n| n.links.len()).sum();
    let climbs = graph.nodes.iter().flat_map(|n| &n.links).filter(|l| l.mantle).count();
    info!("nav graph: {} points, {links} links ({climbs} mantles) in {:?}", graph.nodes.len(), t0.elapsed());
    if let Ok(path) = std::env::var("COD4RW_NAVDUMP") {
        let mut out = String::new();
        for n in &graph.nodes {
            let c = n.pos / u(1.0);
            out.push_str(&format!("{:.0},{:.0},{:.0},{}
", c.x, c.y, c.z, n.links.len()));
        }
        std::fs::write(path, out).ok();
    }
    commands.insert_resource(graph);
}

/// Debug aid (in sims, `COD4RW_SIM`): once, whether each objective's
/// nearest navigation point can be reached from the spawns, and back.
fn check_objectives(
    spatial: SpatialQuery,
    nav: Option<Res<NavGraph>>,
    map: Option<Res<MapInfo>>,
    objectives: Option<Res<crate::modes::Objectives>>,
    mut done: Local<bool>,
) {
    let (Some(nav), Some(map), Some(o), false) = (nav, map, objectives, *done) else { return };
    // `COD4RW_NAVEXPLAIN=x,y,z;x,y,z` (CoD units): how the points there link.
    for spot in std::env::var("COD4RW_NAVEXPLAIN").unwrap_or_default().split(';') {
        let v: Vec<f32> = spot.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if v.len() == 3 {
            for line in nav.explain(&spatial, crate::units::pos([v[0], v[1], v[2]])) {
                info!("nav explain: {line}");
            }
        }
    }
    if std::env::var_os("COD4RW_SIM").is_none() {
        *done = true;
        return;
    }
    *done = true;
    let spawns: Vec<u32> = map.spawns.iter().filter_map(|s| nav.nearest(s.pos)).collect();
    let reach: Vec<Vec<bool>> = spawns.iter().take(24).map(|&s| nav.reachable(s)).collect();
    let targets: Vec<(String, Vec3)> =
        o.flags.iter().map(|f| (format!("flag {}", f.label), f.pos)).chain(o.sites.iter().map(|s| (format!("site {}", s.label), s.pos))).collect();
    for (name, pos) in targets {
        let Some(n) = nav.nearest(pos) else {
            warn!("objective check: {name}: no navigation point near");
            continue;
        };
        let to = reach.iter().filter(|r| r[n as usize]).count();
        let back = nav.reachable(n);
        let from = spawns.iter().take(24).filter(|&&s| back[s as usize]).count();
        let at = nav.nodes[n as usize].pos;
        info!(
            "objective check: {name}: nearest point {:.0} units off ({} links), reached from {to}/{} spawns, reaches {from}",
            at.distance(pos) / u(1.0),
            nav.nodes[n as usize].links.len(),
            reach.len()
        );
    }
    // Where the parts of the graph come closest (a gap the graph missed).
    let first = nav.reachable(spawns[0]);
    if first.iter().filter(|&&r| r).count() < nav.nodes.len() * 9 / 10 {
        let (inside, outside): (Vec<usize>, Vec<usize>) = (0..nav.nodes.len()).partition(|&i| first[i]);
        let mut gaps: Vec<(f32, usize, usize)> = Vec::new();
        for &i in &outside {
            let p = nav.nodes[i].pos;
            if let Some((d, j)) = inside.iter().map(|&j| (nav.nodes[j].pos.distance(p), j)).min_by(|a, b| a.0.total_cmp(&b.0)) {
                gaps.push((d, i, j));
            }
        }
        gaps.sort_by(|a, b| a.0.total_cmp(&b.0));
        info!("objective check: {} of {} points can't be reached from the first spawn", outside.len(), nav.nodes.len());
        let mut shown: Vec<Vec3> = Vec::new();
        for (d, i, j) in gaps {
            let (a, b) = (nav.nodes[i].pos, nav.nodes[j].pos);
            if shown.iter().any(|s| s.distance(a) < u(200.0)) {
                continue;
            }
            shown.push(a);
            let (ca, cb) = (crate::units::to_cod(a), crate::units::to_cod(b));
            info!("  gap {:.0} units: ({:.0} {:.0} {:.0}) to ({:.0} {:.0} {:.0})", d / u(1.0), ca[0], ca[1], ca[2], cb[0], cb[1], cb[2]);
            if shown.len() >= 8 {
                break;
            }
        }
    }
}

/// Debug aid: `COD4RW_FLOORLINE=x,y,dx,dy,n` (CoD units) logs the floor
/// under n points from (x, y) in steps of (dx, dy): its height and slope,
/// as the navigation graph's probes see it.
fn floor_line(spatial: SpatialQuery, map: Option<Res<MapInfo>>, mut done: Local<bool>) {
    let (Some(_), false) = (map, *done) else { return };
    let Some(v) = std::env::var("COD4RW_FLOORLINE").ok().map(|s| s.split(',').filter_map(|x| x.trim().parse::<f32>().ok()).collect::<Vec<_>>()) else {
        *done = true;
        return;
    };
    let filter = collision::movement_filter();
    // Wait for the world's collision.
    let probe = |x: f32, y: f32| {
        let top = crate::units::pos([x, y, 600.0]);
        spatial.cast_ray(top, Dir3::NEG_Y, u(2000.0), true, &filter).map(|h| (600.0 - h.distance / u(1.0), h.normal.y))
    };
    if v.len() < 5 || probe(v[0], v[1]).is_none() {
        return;
    }
    *done = true;
    for k in 0..v[4] as usize {
        let (x, y) = (v[0] + v[2] * k as f32, v[1] + v[3] * k as f32);
        match probe(x, y) {
            Some((z, n)) => info!("floor line ({x:.0} {y:.0}): z {z:.1} normal.y {n:.2}"),
            None => info!("floor line ({x:.0} {y:.0}): none"),
        }
    }
}

/// Analyse the map once its navigation graph exists.
fn build_tactics(
    mut commands: Commands,
    spatial: SpatialQuery,
    map: Option<Res<MapInfo>>,
    map_name: Option<Res<crate::world::MapName>>,
    nav: Option<Res<NavGraph>>,
    tactics: Option<Res<tactical::TacticalMap>>,
    // Real matches on this map are worth waiting the few seconds it takes
    // to read them at startup.
    reading: Option<Res<demos::Loading>>,
    notes: Option<Res<learned::DemoNotes>>,
) {
    let (Some(map), Some(nav), None, None) = (map, nav, tactics, reading) else { return };
    let notes = notes.as_deref().zip(map_name.as_deref()).and_then(|(n, m)| n.get(&m.0));
    let t0 = std::time::Instant::now();
    let t = tactical::TacticalMap::build(&nav, &map, &spatial, notes);
    info!(
        "map analysis: {} spawn areas, {} lane points, {} holding spots ({} from real matches) in {:?}",
        t.areas.len(),
        t.lanes.len(),
        t.spots.len(),
        t.spots.iter().filter(|s| !s.looks.is_empty()).count(),
        t0.elapsed()
    );
    if let Ok(path) = std::env::var("COD4RW_TACTICS_DUMP") {
        tactical::dump(&t, &nav, &path);
    }
    if let (Ok(path), Some(name)) = (std::env::var("COD4RW_HOLDFIT"), map_name.as_deref()) {
        tactical::fit_dump(&nav, &map, &spatial, notes, &name.0, &path);
        std::process::exit(0);
    }
    commands.insert_resource(t);
}

/// Seconds between perception updates and between tactical decisions.
const LOOK_INTERVAL: f32 = 0.05;
const DECIDE_INTERVAL: f32 = 0.25;

/// With `COD4RW_SIM`: time spent thinking (all of `think`) and planning
/// routes, in microseconds, and how many routes; with the worst frame.
static THINK_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static THINK_MAX_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static THINK_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PATH_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PATH_MAX_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PATHS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Route searches this frame, and how many a frame may have (they're the
/// costliest thing a bot does: spread out, no frame gets several).
static ROUTES_THIS_FRAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const ROUTES_PER_FRAME: u32 = 1;
/// Expansions a route search gets a frame (about 0.3 ms).
const ROUTE_SLICE: usize = 1500;
static DECIDES_THIS_FRAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const DECIDES_PER_FRAME: u32 = 3;
/// Phases of [`think`] (µs over the log period): the shared start (team
/// info, claims, plans), perception, decide, act.
static PHASE_US: [std::sync::atomic::AtomicU64; 6] = [const { std::sync::atomic::AtomicU64::new(0) }; 6];
static PHASE_SHARED_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn phase(i: usize, since: std::time::Instant) {
    PHASE_US[i].fetch_add(since.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
}

/// Add a timing to a total and its worst.
fn clock(total: &std::sync::atomic::AtomicU64, max: &std::sync::atomic::AtomicU64, since: std::time::Instant) {
    let us = since.elapsed().as_micros() as u64;
    total.fetch_add(us, std::sync::atomic::Ordering::Relaxed);
    max.fetch_max(us, std::sync::atomic::Ordering::Relaxed);
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn think(
    time: Res<Time>,
    spatial: SpatialQuery,
    map: Option<Res<MapInfo>>,
    nav: Option<Res<NavGraph>>,
    mut callouts: ResMut<Callouts>,
    mut shots: MessageReader<ShotFired>,
    mut damage: MessageReader<Damage>,
    mut killed: MessageReader<Killed>,
    tactics: Option<Res<TacticalMap>>,
    mut intel: ResMut<TeamIntel>,
    mut trouble: ResMut<NavTrouble>,
    motion_lib: Option<Res<motion::MotionLibrary>>,
    mut fired_at: Local<HashMap<Entity, f32>>,
    mut bots: Query<
        (
            Entity,
            &mut Bot,
            &Pawn,
            &Transform,
            &Mover,
            &WeaponState,
            &Health,
            &mut ViewAngles,
            &mut MoveInput,
            &mut WeaponInput,
            Option<&crate::grenades::Flashed>,
            Option<&crate::grenades::Stunned>,
            Has<crate::perks::Downed>,
        ),
        Without<Dead>,
    >,
    pawns: Query<(Entity, &Pawn, &Transform, &Mover, Option<&crate::loadout::Loadout>), Without<Dead>>,
    (hitboxes, objectives): (Query<&crate::combat::Hitbox>, Option<Res<crate::modes::Objectives>>),
) {
    let Some(map) = map else { return };
    let started = std::time::Instant::now();
    let now = time.elapsed_secs();
    let dt = time.delta_secs().max(1e-4);
    let mut rng = rand::rng();

    for s in shots.read() {
        fired_at.insert(s.shooter, now);
    }
    let hits: Vec<(Entity, Entity)> = damage.read().filter_map(|d| d.attacker.map(|a| (d.target, a))).collect();
    let snapshot: Vec<PawnView> = pawns
        .iter()
        .map(|(e, p, tf, m, loadout)| PawnView {
            entity: e,
            team: p.team,
            id: p.id,
            feet: tf.translation,
            eye_height: m.eye_height,
            vel: m.velocity,
            stance: m.stance,
            sprinting: m.sprinting,
            quiet: crate::perks::has(loadout, "specialty_quieter"),
        })
        .collect();
    let alive: Vec<Entity> = snapshot.iter().map(|p| p.entity).collect();

    // What each team learns this match: where enemies kill them from.
    let deaths: Vec<Killed> = killed.read().cloned().collect();
    if let Some(nav) = nav.as_deref() {
        if now - intel.decayed_at > 1.0 {
            let dt = now - intel.decayed_at;
            intel.decay(dt.min(5.0));
            intel.decayed_at = now;
        }
        for k in &deaths {
            let victim_team = pawns.get(k.victim).map(|(_, p, _, _, _)| p.team).ok();
            let killer = k.attacker.and_then(|a| snapshot.iter().find(|p| p.entity == a));
            if let (Some(team), Some(killer)) = (victim_team, killer) {
                if killer.team != team || crate::combat::free_for_all() {
                    intel.deposit(nav, team, killer.feet, 2.0);
                }
            }
        }
    }
    let t_shared = std::time::Instant::now();
    ROUTES_THIS_FRAME.store(0, std::sync::atomic::Ordering::Relaxed);
    DECIDES_THIS_FRAME.store(0, std::sync::atomic::Ordering::Relaxed);
    // Per team: where its enemies will spawn next, and where the fight is.
    let team_info = |team: Team| {
        let mine: Vec<Vec3> = snapshot.iter().filter(|p| p.team == team).map(|p| p.feet).collect();
        let spawns = tactics.as_deref().map(|t| t.predicted_enemy_spawns(&map, &mine)).unwrap_or_default();
        let front = tactics.as_deref().and_then(|t| t.front(&map, &mine));
        (spawns, front)
    };
    let (allies_spawns, allies_front) = team_info(Team::Allies);
    let (axis_spawns, axis_front) = team_info(Team::Axis);
    // Across the map, from where each team will spawn to where the other will.
    let lateral = |from: Option<Front>, to: Option<Front>| {
        let (from, to) = (from?.enemy, to?.enemy);
        tactics.as_deref()?.lateral(from, to)
    };
    let (allies_lateral, axis_lateral) = (lateral(axis_front, allies_front), lateral(allies_front, axis_front));
    // Spots teammates are already holding.
    let claims: Vec<(Team, usize, Entity)> = bots
        .iter()
        .filter_map(|(e, b, p, ..)| b.hold.as_ref().and_then(|h| h.spot).map(|s| (p.team, s, e)))
        .collect();
    let mut plans: Vec<MatePlan> = bots
        .iter()
        .map(|(e, b, p, tf, m, ..)| MatePlan::of(e, b, p.team, tf.translation, m.velocity, nav.as_deref()))
        .collect();
    let mut noticed_by_team: Vec<(Team, Vec3)> = Vec::new();
    callouts.0.retain(|c| now - c.3 < 6.0);
    let mut new_callouts = Vec::new();

    for (me, mut bot, pawn, tf, mover, weapon, health, mut view, mut mv, mut wi, flashed, stunned, downed) in &mut bots {
        // Flashbanged: blind (nothing seen, nobody kept in sight) until it
        // fades. Stunned: slow to move and turn.
        if PHASE_SHARED_DONE.swap(true, std::sync::atomic::Ordering::Relaxed) == false {
            phase(0, t_shared);
        }
        let blind = flashed.map_or(0.0, |f| f.strength(now)) > 0.35;
        let stun = stunned.map_or(0.0, |s| s.strength(now));
        let bot = &mut *bot;
        let feet = tf.translation;
        let eye = mover.eye(feet);

        // Respawned (or teleported): start fresh.
        if feet.distance(bot.last_feet) > u(300.0) {
            bot.aim.reset();
            bot.engagement = None;
            bot.set_mode(Mode::Hunt, now);
            bot.mode = Mode::Hunt;
            bot.path.clear();
            bot.dest = None;
            bot.know.contacts.retain(|_, c| c.source != Source::Seen);
            for c in bot.know.contacts.values_mut() {
                c.visible = false;
                c.noticed_at = None;
                c.awareness = 0.0;
            }
            bot.progress_pos = feet;
            bot.progress_time = now;
        }
        bot.last_feet = feet;

        // --- Perception.
        let t_look = std::time::Instant::now();
        let enemies: Vec<PawnView> = snapshot.iter().filter(|p| p.hostile_to(pawn)).copied().collect();
        if blind {
            for c in bot.know.contacts.values_mut() {
                c.visible = false;
            }
        } else if now - bot.last_look >= LOOK_INTERVAL {
            let look_dt = (now - bot.last_look).min(0.25);
            // A little jitter, so bots that spawned together don't all look
            // in the same frame (the same rate on average).
            bot.last_look = now + rng.random_range(-0.01..0.01);
            let noticed =
                perception::look(&mut bot.know, &spatial, eye, view.forward(), &enemies, &fired_at, bot.skill, now, look_dt);
            for e in noticed {
                if let Some(c) = bot.know.contacts.get(&e) {
                    new_callouts.push((pawn.team, e, c.pos, now));
                    noticed_by_team.push((pawn.team, c.pos));
                }
                bot.next_decide = now; // react to new enemies at once
            }
            let heard = perception::listen(&mut bot.know, feet, &enemies, &fired_at, now, &mut rng);
            // Shots or footsteps out of view, out of a fight: often worth a
            // look.
            for (e, sound) in heard {
                let (base, per_skill) = match sound {
                    perception::Sound::Footsteps => FOOTSTEP_TURN,
                    perception::Sound::Gunfire => GUNFIRE_TURN,
                };
                let behind = enemies.iter().find(|p| p.entity == e).is_some_and(|p| {
                    let to = (p.feet - feet).normalize_or_zero();
                    to.dot(view.forward()) < 0.26
                });
                if behind
                    && bot.alarm.is_none()
                    && bot.engagement.is_none()
                    && rng.random::<f32>() < base + per_skill * bot.skill
                {
                    let at = now + bot.reaction * SOUND_REACTION * (0.25 * gaussian(&mut rng)).exp();
                    bot.alarm = Some(Alarm { target: e, at, until: at + ALARM_FOR, point: None, next_point: 0.0 });
                }
            }
            // Free-for-all: nobody tells anybody anything.
            if !crate::combat::free_for_all() {
                perception::hear_callouts(&mut bot.know, pawn.team, &callouts, now);
            }
            perception::forget(&mut bot.know, &alive, now);
        }
        for &(target, attacker) in &hits {
            if target == me {
                if let Some(a) = snapshot.iter().find(|p| p.entity == attacker && p.hostile_to(pawn)) {
                    perception::shot_by(&mut bot.know, attacker, a.feet, now, &mut rng);
                    bot.next_decide = now;
                    let seen = bot.know.contacts.get(&attacker).is_some_and(|c| c.noticed());
                    if !seen && bot.alarm.is_none_or(|a| a.target != attacker) {
                        let at = now + bot.reaction * ALARM_REACTION * (0.25 * gaussian(&mut rng)).exp();
                        bot.alarm = Some(Alarm { target: attacker, at, until: at + ALARM_FOR, point: None, next_point: 0.0 });
                    }
                }
            }
        }

        // Got a kill while holding: people move on rather than wait for the
        // revenge.
        if deaths.iter().any(|k| k.attacker == Some(me) && k.victim != me) && matches!(bot.mode, Mode::Post | Mode::Hold) {
            bot.hold_until = now;
            bot.next_decide = now;
        }

        phase(1, t_look);
        // --- Tactics.
        let (enemy_spawns, front, lateral) = match pawn.team {
            Team::Allies => (&allies_spawns, allies_front, allies_lateral),
            Team::Axis => (&axis_spawns, axis_front, axis_lateral),
        };
        let tc = TacCtx {
            tactics: tactics.as_deref(),
            intel: &intel,
            front,
            enemy_spawns,
            team: pawn.team,
            taken: claims.iter().filter(|c| c.0 == pawn.team && c.2 != me).map(|c| c.1).collect(),
            mates: plans.iter().filter(|m| m.team == pawn.team && m.entity != me && !crate::combat::free_for_all()).collect(),
            lateral,
            trouble: &trouble.0,
            motion: motion_lib.as_deref().map(|m| &*m.0),
            objectives: objectives.as_deref().filter(|o| !o.flags.is_empty() || !o.sites.is_empty() || o.hq.is_some()),
        };
        // At most a few decisions a frame: one over waits a frame.
        if now >= bot.next_decide && DECIDES_THIS_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < DECIDES_PER_FRAME {
            bot.next_decide = now + DECIDE_INTERVAL * rng.random_range(0.8..1.2);
            let t = std::time::Instant::now();
            decide(bot, me, &spatial, nav.as_deref(), &tc, &map, pawn, feet, eye, health, weapon, now, &mut rng);
            phase(2, t);
        }

        // --- Movement, looking and shooting.
        let t_act = std::time::Instant::now();
        let mut plan = act(bot, &spatial, nav.as_deref(), &tc, feet, eye, mover, &view, weapon, now, &mut rng);
        phase(3, t_act);
        grenade::steer(bot, &mut plan, feet, eye, &view, now);
        equipment::shoot_heli(bot, &mut plan, &view, eye, feet, now, &mut rng);
        equipment::steer(bot, &mut plan, &view, eye, now);
        // Downed (Last Stand): prone where they fell, fighting on with the
        // pistol.
        if downed {
            plan.stance = Stance::Prone;
            plan.forward = 0.0;
            plan.right = 0.0;
            plan.sprint = false;
            plan.jump = false;
        }
        if let Some(node) = bot.stuck_at.take() {
            *trouble.0.entry(node).or_default() += 1;
        }
        // Later bots this frame see where we're now headed.
        if let Some(mine) = plans.iter_mut().find(|m| m.entity == me) {
            *mine = MatePlan::of(me, bot, pawn.team, feet, mover.velocity, nav.as_deref());
        }

        // Aim: the hand model moves the view towards the chosen goal.
        let yaw_before = view.yaw;
        // A learned aim model (`COD4RW_NETAIM`) takes over while engaging.
        let moving = Vec2::new(mover.velocity.x, mover.velocity.z).length() > crate::units::u(20.0);
        let net = nets::aim_net().is_some_and(|n| bot.net_aim.update(n, &mut view, plan.look, moving, dt, &mut rng));
        if net {
            bot.aim.sync(&view);
        } else {
            bot.aim.update(&bot.aim_profile, &mut view, plan.look, now, dt, &mut rng);
        }
        // Motion matching turns the view the way the matched player did.
        if plan.turn.is_some() || plan.pitch.is_some() {
            if let Some(rate) = plan.turn {
                view.yaw += rate * dt;
            }
            if let Some(p) = plan.pitch {
                view.pitch += (p - view.pitch) * (dt * 6.0).min(1.0);
            }
            bot.aim.sync(&view);
        }
        // Walking about (not fighting, not turning to a noise), real players
        // rarely look more than 100 degrees off the way they're going (8% of
        // the time; bots 15-22%): ease the view back within that (bot lab,
        // 2026-10-06: looking backwards roughly halved on five of six maps).
        if !matches!(bot.mode, Mode::Engage | Mode::Cover) && bot.look_src != "alarm" {
            let v = Vec2::new(mover.velocity.x, mover.velocity.z);
            if v.length() > crate::units::u(80.0) {
                let travel = (-v.x).atan2(-v.y);
                let off = wrap_angle(view.yaw - travel);
                // Bot lab experiment `look75`: a tighter limit.
                let limit = if lab::on("look75") { 75f32 } else { 100f32 }.to_radians();
                if off.abs() > limit {
                    let excess = off.abs() - limit;
                    view.yaw -= off.signum() * excess.min(3.5 * dt);
                    bot.aim.sync(&view);
                }
            }
        }
        if stun > 0.0 {
            // As the player's: turning damped to about a third.
            let k = 1.0 - 0.65 * stun;
            view.yaw = yaw_before + wrap_angle(view.yaw - yaw_before) * k;
            bot.aim.sync(&view);
        }
        bot.turn_rate = wrap_angle(view.yaw - yaw_before) / dt;

        // Fire discipline and friendly-fire check.
        let mut fire = plan.fire;
        if fire {
            let bullet = collision::bullet_filter();
            let own = |e: Entity| hitboxes.get(e).map_or(true, |h| h.owner != me);
            let friendly_in_way = Dir3::new(view.forward())
                .ok()
                .and_then(|d| spatial.cast_ray_predicate(eye, d, perception::SIGHT_RANGE, true, &bullet, &own))
                .and_then(|hit| hitboxes.get(hit.entity).ok())
                .and_then(|hb| snapshot.iter().find(|p| p.entity == hb.owner))
                .is_some_and(|p| !p.hostile_to(pawn));
            fire = !friendly_in_way;
        }
        // Semi-automatics and bursts take a pull per shot (or burst): let go
        // once it's spent, and pull again no faster than a finger clicks.
        if weapon.def.fire_type != 0 && fire {
            if weapon.trigger_spent() {
                fire = false;
                if wi.fire {
                    bot.next_pull = now + rng.random_range(0.08..0.16) * (1.4 - 0.6 * bot.skill);
                }
            } else if !wi.fire && now < bot.next_pull {
                fire = false;
            }
        }
        *wi = WeaponInput { fire, ads: plan.ads, reload: plan.reload, ..default() };
        *mv = MoveInput {
            forward: plan.forward,
            right: plan.right,
            // Jumps need a fresh press: tap rather than hold.
            jump: plan.jump && !mv.jump,
            sprint: plan.sprint,
            stance: plan.stance,
            speed_scale: weapon.speed_scale() * (1.0 - 0.45 * stun),
            lean: 0.0,
        };
    }
    callouts.0.extend(new_callouts);
    PHASE_SHARED_DONE.store(false, std::sync::atomic::Ordering::Relaxed);
    clock(&THINK_US, &THINK_MAX_US, started);
    THINK_FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Some(nav) = nav.as_deref() {
        for (team, pos) in noticed_by_team {
            intel.deposit(nav, team, pos, 1.0);
        }
    }
}

/// The nearest enemy the bot has noticed and can see, preferring whoever is
/// shooting at it.
fn current_threat(know: &Knowledge, feet: Vec3, now: f32) -> Option<(Entity, Contact)> {
    know.contacts
        .iter()
        .filter(|(_, c)| c.in_fight(now))
        .min_by(|a, b| {
            let score = |c: &Contact| {
                let mut d = c.pos.distance(feet);
                if c.shot_us_at.is_some_and(|t| now - t < 2.0) {
                    d *= 0.5;
                }
                d
            };
            score(a.1).total_cmp(&score(b.1))
        })
        .map(|(e, c)| (*e, *c))
}

/// The freshest enemy the bot remembers but can't see.
fn freshest_memory(know: &Knowledge, now: f32) -> Option<(Entity, Contact)> {
    know.contacts
        .iter()
        .filter(|(_, c)| !c.in_fight(now) && c.age(now) < 15.0)
        .min_by(|a, b| a.1.age(now).total_cmp(&b.1.age(now)))
        .map(|(e, c)| (*e, *c))
}

#[allow(clippy::too_many_arguments)]
fn decide(
    bot: &mut Bot,
    me: Entity,
    spatial: &SpatialQuery,
    nav: Option<&NavGraph>,
    tc: &TacCtx,
    map: &MapInfo,
    pawn: &Pawn,
    feet: Vec3,
    eye: Vec3,
    health: &Health,
    weapon: &WeaponState,
    now: f32,
    rng: &mut impl Rng,
) {
    let p = bot.personality.clone();
    // Better players get out of fights they're losing; weaker ones stay in.
    let sense = 0.4 + bot.skill;
    let threat = current_threat(&bot.know, feet, now);
    let memory = freshest_memory(&bot.know, now);
    let visible_count = bot.know.contacts.values().filter(|c| c.in_fight(now)).count();
    let health_frac = health.current / crate::combat::max_health();
    let hurt = health_frac < 0.5;
    let empty = weapon.clip == 0 || weapon.reloading();
    let recently_shot = bot.know.contacts.values().any(|c| c.shot_us_at.is_some_and(|t| now - t < 1.5));
    let stick = |m: Mode| if bot.mode == m { 0.2 } else { 0.0 };

    let mut options: Vec<(Mode, f32)> = vec![(Mode::Hunt, 0.3 + stick(Mode::Hunt))];
    // Holding a strong spot: what anchors do, others now and then.
    if tc.tactics.is_some_and(|t| !t.spots.is_empty()) && !hurt {
        let base = match bot.role {
            Role::Anchor => 0.8 + 0.3 * p.patience,
            Role::Flanker => 0.15 + 0.3 * p.patience,
            Role::Rusher => 0.05,
        };
        options.push((Mode::Post, base + stick(Mode::Post)));
    }
    if threat.is_some() {
        let n = (visible_count as f32 - 1.0).max(0.0);
        options.push((
            Mode::Engage,
            1.0 + 0.4 * p.aggression - if hurt { 0.45 * sense } else { 0.0 } - if empty { 2.0 } else { 0.0 } - 0.25 * n
                + stick(Mode::Engage),
        ));
    }
    // In cover with a loaded gun and health to spare: fight from there.
    let settled_in_cover = bot.mode == Mode::Cover && bot.dest.is_none() && !empty && !hurt;
    if (threat.is_some() || recently_shot) && !settled_in_cover {
        let n = (visible_count as f32 - 1.0).max(0.0);
        options.push((
            Mode::Cover,
            0.35 + if hurt { 0.8 * sense } else { 0.0 } + if empty { 1.6 } else { 0.0 } + 0.3 * n - 0.3 * p.aggression
                + stick(Mode::Cover),
        ));
    }
    if let Some((_, c)) = memory {
        let fresh = 1.0 - c.age(now) / 15.0;
        let dist = c.pos.distance(feet);
        // Gunfire across the map is less pressing than an enemy just seen.
        let pull = if c.source == Source::Heard && dist > u(2000.0) { 0.6 } else { 1.0 } / (1.0 + dist / u(4000.0));
        let role_pull = match bot.role {
            Role::Anchor if dist > u(1000.0) => 0.4,
            Role::Anchor => 0.9,
            Role::Flanker => 0.75,
            Role::Rusher => 1.0,
        };
        // Teammates already going after this enemy: one goes straight in,
        // the next goes around, the rest carry on with their own thing.
        let target = memory.map(|(e, _)| e);
        let after = |modes: &[Mode]| tc.mates.iter().filter(|m| modes.contains(&m.mode) && m.focus == target).count();
        let on_it = matches!(bot.mode, Mode::Investigate | Mode::Flank) && bot.focus == target;
        let (direct, around) =
            if on_it { (0, 0) } else { (after(&[Mode::Investigate, Mode::Hold]), after(&[Mode::Flank])) };
        let crowd = [1.0, 0.55, 0.3][(direct + around).min(2)];
        // Mostly the ones whose side of the map it's on.
        let side = 0.35 + 0.65 * tc.lane_fit(bot.lane, c.pos);
        options.push((
            Mode::Investigate,
            (0.45 + 0.35 * p.aggression + 0.25 * fresh) * pull * role_pull * crowd * side
                * if direct > 0 { 0.7 } else { 1.0 }
                + stick(Mode::Investigate),
        ));
        // Flank: flankers go around anyone they know about; others only
        // someone who is staying put, or someone a teammate is pushing.
        let still = c.vel.length() < u(40.0) && c.age(now) > 1.0;
        let pincer = direct > 0 && around == 0;
        if c.age(now) < 12.0 && (still || bot.role == Role::Flanker || pincer) {
            let base = if bot.role == Role::Flanker { 0.75 } else { 0.4 + 0.3 * (1.0 - (p.aggression - 0.6).abs()) };
            let bonus = if pincer { 0.2 } else { 0.0 };
            options.push((Mode::Flank, (base + bonus) * pull * crowd + stick(Mode::Flank)));
        }
        // Waiting for them to come: worth it when they're near and nobody
        // else is already waiting for the same one.
        let waiting = tc.mates.iter().any(|m| m.mode == Mode::Hold && m.focus == target);
        let near = 1.0 / (1.0 + dist / u(2000.0));
        let hold = (0.15 + 0.5 * p.patience * fresh) * near * if waiting { 0.5 } else { 1.0 };
        options.push((Mode::Hold, hold + stick(Mode::Hold)));
    }
    // The game mode's objective: flags, the bomb.
    if let Some(goal) = objective::goal(bot, me, feet, tc) {
        options.push((Mode::Objective, goal.weight(p.aggression) + stick(Mode::Objective)));
    }
    // Hurt and nothing in sight: hang back while health comes back.
    if hurt && threat.is_none() {
        options.push((Mode::Hold, 0.5 + 0.4 * p.patience + stick(Mode::Hold)));
    }
    // With an enemy in view a person fights or breaks for cover; nothing else.
    if threat.is_some() {
        options.retain(|o| matches!(o.0, Mode::Engage | Mode::Cover));
    }
    // A little indecision makes bots less predictable.
    for o in &mut options {
        o.1 += rng.random_range(0.0..0.08);
    }
    // Holding has a time limit; moving on after it runs out.
    if matches!(bot.mode, Mode::Hold | Mode::Post) && now > bot.hold_until {
        let current = bot.mode;
        options.retain(|o| o.0 != current);
    }
    let Some(&(mode, _)) = options.iter().max_by(|a, b| a.1.total_cmp(&b.1)) else { return };

    match mode {
        Mode::Engage => {
            let (target, c) = threat.expect("engage needs a threat");
            if bot.engagement.as_ref().is_none_or(|e| e.target != target) {
                let noticed = c.noticed_at.unwrap_or(now);
                let alarmed = bot.alarm.is_some_and(|a| a.target == target && now >= a.at);
                let reaction = bot.reaction * (0.25 * gaussian(rng)).exp() * if alarmed { ALARMED_REACTION } else { 1.0 };
                let dist = c.pos.distance(feet);
                bot.engagement = Some(Engagement {
                    target,
                    react_at: noticed + reaction,
                    aim_head: rng.random::<f32>() < p.head_bias * (1.0 - (dist / u(2500.0)).min(1.0) * 0.5),
                    // Bot lab experiment `crouchfight`: real players spend
                    // 32-48% of fighting crouched or prone, bots 25%.
                    crouch: if lab::on("crouchfight") {
                        dist > u(300.0) && rng.random::<f32>() < 0.55
                    } else {
                        (bot.style.crouch_any_range || dist > u(700.0)) && rng.random::<f32>() < bot.style.crouch
                    },
                    last_visible: now,
                    seen_at: c.visible_since,
                    first_shot: None,
                    // Dropshot: going prone the moment a close fight starts.
                    drop: dist < u(650.0)
                        && rng.random::<f32>() < bot.style.dropshot.unwrap_or(0.08 + 0.25 * p.aggression * bot.skill),
                    ads: false,
                    ads_next: 0.0,
                    ads_roll: 1.0,
                    moving: false,
                    moving_since: now,
                });
            }
            bot.set_mode(Mode::Engage, now);
        }
        Mode::Cover => {
            // Shoot back on the way, at whoever is in view.
            if let Some((target, c)) = threat {
                if bot.engagement.as_ref().is_none_or(|e| e.target != target) {
                    let noticed = c.noticed_at.unwrap_or(now);
                    let alarmed = bot.alarm.is_some_and(|a| a.target == target && now >= a.at);
                    bot.engagement = Some(Engagement {
                        target,
                        react_at: noticed
                            + bot.reaction * (0.25 * gaussian(rng)).exp() * if alarmed { ALARMED_REACTION } else { 1.0 },
                        aim_head: false,
                        crouch: false,
                        last_visible: now,
                        seen_at: c.visible_since,
                        first_shot: None,
                        drop: false,
                        ads: false,
                        ads_next: 0.0,
                        ads_roll: 1.0,
                        moving: false,
                        moving_since: now,
                    });
                }
            }
            if bot.mode == Mode::Cover && bot.dest.is_some() {
                return finish_decide(bot, mode, threat.is_some());
            }
            let threats: Vec<Vec3> = bot
                .know
                .contacts
                .values()
                .filter(|c| c.noticed() || c.shot_us_at.is_some_and(|t| now - t < 3.0))
                .map(|c| c.pos + Vec3::Y * u(60.0))
                .collect();
            let cover = nav.and_then(|nav| find_cover(nav, spatial, feet, &threats, &tc.mate_points(), u(600.0)));
            match cover {
                Some(spot) => {
                    bot.set_mode(Mode::Cover, now);
                    bot.go_to(spot);
                }
                None if threat.is_some() => bot.set_mode(Mode::Engage, now),
                None => bot.set_mode(Mode::Hunt, now),
            }
        }
        Mode::Investigate => {
            let (e, c) = memory.expect("investigate needs a memory");
            let target = c.predicted(now);
            // Keep going where we were if it's still near them.
            let keep = bot.mode == Mode::Investigate
                && bot.focus == Some(e)
                && bot.dest.is_some_and(|d| d.distance(target) < u(700.0));
            if !keep {
                bot.set_mode(Mode::Investigate, now);
                bot.focus = Some(e);
                bot.go_to(approach(nav, tc, bot.lane, target));
            }
        }
        Mode::Flank => {
            let (e, c) = memory.expect("flank needs a memory");
            if bot.mode != Mode::Flank || bot.focus != Some(e) {
                let avoid = tc.mate_points();
                let fit = |p: Vec3| tc.lane_fit(bot.lane, p);
                let spot = nav.and_then(|nav| find_flank(nav, spatial, feet, c.pos, &avoid, fit, rng));
                match spot {
                    Some(s) => {
                        bot.set_mode(Mode::Flank, now);
                        bot.focus = Some(e);
                        bot.go_to(s);
                    }
                    None => {
                        bot.set_mode(Mode::Investigate, now);
                        bot.focus = Some(e);
                        bot.go_to(spaced(nav, c.predicted(now), &avoid));
                    }
                }
            }
        }
        Mode::Hold => {
            if bot.mode != Mode::Hold {
                bot.set_mode(Mode::Hold, now);
                // Real players' stops in the demos: half under 0.6 s, 99% under 9 s.
                bot.hold_until = now + rng.random_range(1.5..3.0) + 4.0 * p.patience;
                bot.focus = memory.map(|(e, _)| e);
                // Back off into cover from where the enemy was, if there is any near.
                if let (Some(nav), Some((_, c))) = (nav, memory) {
                    let threat = [c.pos + Vec3::Y * u(60.0)];
                    if let Some(spot) = find_cover(nav, spatial, feet, &threat, &tc.mate_points(), u(300.0)) {
                        bot.go_to(spot);
                    }
                }
            }
        }
        Mode::Post => {
            let current = bot.hold.as_ref().and_then(|h| h.spot);
            if bot.mode != Mode::Post || current.is_none() {
                let tactics = tc.tactics.expect("post needs a tactical map");
                let mut taken = tc.taken.clone();
                taken.extend(bot.last_spot);
                match tactics.choose_spot(
                    feet,
                    tc.front,
                    tc.intel,
                    tc.team,
                    &taken,
                    bot.role == Role::Anchor,
                    |p| tc.lane_fit(bot.lane, p) * tc.flag_fit(p),
                    rng,
                ) {
                    Some(spot) => {
                        bot.set_mode(Mode::Post, now);
                        let angles = tactics.spot_angles(spot, tc.front, tc.intel, tc.team);
                        let at = tactics.spots[spot].pos;
                        let watch = angles.first().map(|a| *a + Vec3::Y * u(60.0));
                        let stance = hold_stance(spatial, at, watch, p.patience, rng);
                        let far = watch.is_some_and(|w| w.distance(at) > u(1200.0));
                        let ads = rng.random::<f32>() < if far { HOLD_ADS_FAR } else { HOLD_ADS };
                        bot.hold = Some(HoldPlan { spot: Some(spot), angles, angle_i: 0, next_switch: 0.0, stance, ads });
                        bot.last_spot = Some(spot);
                        bot.hold_until = now
                            + match bot.role {
                                // Real players never held a spot much over 10 s.
                                Role::Anchor => rng.random_range(2.5..6.0) + 4.0 * p.patience,
                                _ => rng.random_range(1.5..4.0),
                            };
                        bot.go_to(tactics.spots[spot].pos);
                    }
                    None => bot.set_mode(Mode::Hunt, now),
                }
            }
        }
        Mode::Objective => {
            let goal = objective::goal(bot, me, feet, tc).expect("objective needs a goal");
            objective::pursue(bot, goal, tc, nav, feet, now, rng);
        }
        Mode::Hunt => {
            if bot.mode != Mode::Hunt || bot.dest.is_none() {
                bot.set_mode(Mode::Hunt, now);
                if let Some(d) = hunt_destination(nav, tc, bot.role, bot.lane, map, pawn.team, feet, rng) {
                    bot.go_to(d);
                }
            }
        }
    }
    finish_decide(bot, mode, threat.is_some());
    let _ = eye;
}

fn finish_decide(bot: &mut Bot, mode: Mode, threat: bool) {
    if !(mode == Mode::Engage || mode == Mode::Cover && threat) {
        bot.engagement = None;
    }
}

/// Somewhere worth going when nothing is known: where enemies are about to
/// spawn (or where they have kept showing up), else the enemy side of the
/// map or anywhere reasonably far away.
fn hunt_destination(
    nav: Option<&NavGraph>,
    tc: &TacCtx,
    role: Role,
    lane: f32,
    map: &MapInfo,
    team: crate::combat::Team,
    feet: Vec3,
    rng: &mut impl Rng,
) -> Option<Vec3> {
    // Somewhere a teammate isn't already headed.
    let crowded = |p: Vec3| tc.mates.iter().filter(|m| m.dest.is_some_and(|d| d.distance(p) < u(700.0))).count();
    // Domination: round the flags in play, as real players fight (a third
    // of their time within 600 units of one, little of it on it).
    if let Some(o) = tc.objectives.filter(|o| !o.flags.is_empty()) {
        if rng.random::<f32>() < objective::HUNT_FLAGS {
            let spots: Vec<Vec3> = o.flags.iter().map(|f| f.pos).filter(|&p| objective::flag_fit(o, team, p) > 1.0).collect();
            if !spots.is_empty() {
                let at = spots[rng.random_range(0..spots.len())];
                let a = rng.random_range(0.0..std::f32::consts::TAU);
                let near = at + Vec3::new(a.cos(), 0.0, a.sin()) * rng.random_range(u(250.0)..u(600.0));
                if let Some(n) = nav.and_then(|n| n.nearest(near)) {
                    return Some(nav.expect("nav").nodes[n as usize].pos);
                }
            }
        }
    }
    let far: Vec<(Vec3, f32)> = tc
        .enemy_spawns
        .iter()
        .filter(|(p, _)| p.distance(feet) > u(600.0))
        .map(|&(p, w)| (p, w * 0.3f32.powi(crowded(p) as i32) * tc.lane_fit(lane, p)))
        .collect();
    if !far.is_empty() && rng.random::<f32>() < if role == Role::Anchor { 0.5 } else { 0.85 } {
        let total: f32 = far.iter().map(|f| f.1).sum();
        let mut pick = rng.random::<f32>() * total;
        for (p, w) in &far {
            pick -= w;
            if pick <= 0.0 {
                return Some(*p);
            }
        }
        return far.last().map(|f| f.0);
    }
    let enemy_start = match team {
        crate::combat::Team::Allies => SpawnKind::AxisStart,
        crate::combat::Team::Axis => SpawnKind::AlliesStart,
    };
    if rng.random::<f32>() < 0.35 {
        let picks: Vec<Vec3> = map
            .spawns
            .iter()
            .filter(|s| s.kind == enemy_start || s.kind == SpawnKind::Tdm)
            .map(|s| s.pos)
            .filter(|p| p.distance(feet) > u(800.0))
            .collect();
        if !picks.is_empty() {
            return Some(picks[rng.random_range(0..picks.len())]);
        }
    }
    if let Some(nav) = nav.filter(|n| !n.nodes.is_empty()) {
        for _ in 0..20 {
            let n = &nav.nodes[rng.random_range(0..nav.nodes.len())];
            let fits = rng.random::<f32>() < tc.lane_fit(lane, n.pos);
            if n.pos.distance(feet) > u(800.0) && !n.crouch_only && crowded(n.pos) == 0 && fits {
                return Some(n.pos);
            }
        }
    }
    map.spawns.get(rng.random_range(0..map.spawns.len().max(1))).map(|s| s.pos)
}

/// How far apart teammates like to stand.
const MATE_SPACING: f32 = u(250.0);

/// `dest`, or the nearest walkable point to it that's not where a teammate
/// is or is going.
fn spaced(nav: Option<&NavGraph>, dest: Vec3, avoid: &[Vec3]) -> Vec3 {
    let free = |p: Vec3| avoid.iter().all(|a| a.distance(p) >= MATE_SPACING);
    if free(dest) {
        return dest;
    }
    let Some(nav) = nav else { return dest };
    nav.within(dest, u(600.0))
        .into_iter()
        .map(|i| &nav.nodes[i as usize])
        .filter(|n| !n.crouch_only && free(n.pos))
        .map(|n| n.pos)
        .min_by(|a, b| a.distance(dest).total_cmp(&b.distance(dest)))
        .unwrap_or(dest)
}

/// Where to check out `target` from: near it, on our side of the map, and
/// not where a teammate is or is going.
fn approach(nav: Option<&NavGraph>, tc: &TacCtx, lane: f32, target: Vec3) -> Vec3 {
    let avoid = tc.mate_points();
    let Some(nav) = nav else { return target };
    nav.within(target, u(700.0))
        .into_iter()
        .map(|i| &nav.nodes[i as usize])
        .filter(|n| !n.crouch_only && avoid.iter().all(|a| a.distance(n.pos) >= MATE_SPACING))
        .map(|n| (n.pos, tc.lane_fit(lane, n.pos) * (-n.pos.distance(target) / u(400.0)).exp()))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or_else(|| spaced(Some(nav), target, &avoid), |(p, _)| p)
}

fn line_clear(spatial: &SpatialQuery, from: Vec3, to: Vec3) -> bool {
    let d = to - from;
    let len = d.length();
    Dir3::new(d).ok().is_none_or(|dir| spatial.cast_ray(from, dir, len, true, &collision::sight_filter()).is_none())
}

/// A nearby nav point hidden from every threat's eye (at standing chest
/// height), not closer to them than we are, and not on top of a teammate.
fn find_cover(
    nav: &NavGraph,
    spatial: &SpatialQuery,
    feet: Vec3,
    threats: &[Vec3],
    avoid: &[Vec3],
    radius: f32,
) -> Option<Vec3> {
    if threats.is_empty() {
        return None;
    }
    let mut candidates = nav.within(feet, radius);
    candidates.sort_by(|&a, &b| {
        nav.nodes[a as usize].pos.distance(feet).total_cmp(&nav.nodes[b as usize].pos.distance(feet))
    });
    let my_threat_dist: f32 = threats.iter().map(|t| t.distance(feet)).fold(f32::MAX, f32::min);
    for &i in candidates.iter().step_by(2).take(60) {
        let n = &nav.nodes[i as usize];
        if n.crouch_only || avoid.iter().any(|a| a.distance(n.pos) < MATE_SPACING) {
            continue;
        }
        let closest = threats.iter().map(|t| t.distance(n.pos)).fold(f32::MAX, f32::min);
        if closest < my_threat_dist - u(64.0) {
            continue;
        }
        let chest = n.pos + Vec3::Y * u(48.0);
        if threats.iter().all(|&t| !line_clear(spatial, t, chest)) {
            return Some(n.pos);
        }
    }
    None
}

/// A point that sees `target` from a different side than we do, away from
/// teammates.
fn find_flank(
    nav: &NavGraph,
    spatial: &SpatialQuery,
    feet: Vec3,
    target: Vec3,
    avoid: &[Vec3],
    fit: impl Fn(Vec3) -> f32,
    rng: &mut impl Rng,
) -> Option<Vec3> {
    let candidates = nav.within(target, u(1100.0));
    if candidates.is_empty() {
        return None;
    }
    let ours = Vec2::new(feet.x - target.x, feet.z - target.z).normalize_or_zero();
    let chest = target + Vec3::Y * u(48.0);
    let mut best: Option<(Vec3, f32)> = None;
    for _ in 0..80 {
        let n = &nav.nodes[candidates[rng.random_range(0..candidates.len())] as usize];
        let off = Vec2::new(n.pos.x - target.x, n.pos.z - target.z);
        if off.length() < u(350.0) || n.crouch_only || avoid.iter().any(|a| a.distance(n.pos) < MATE_SPACING) {
            continue;
        }
        if off.normalize_or_zero().dot(ours) > 0.6 {
            continue;
        }
        if !line_clear(spatial, n.pos + Vec3::Y * u(60.0), chest) {
            continue;
        }
        // Near, and on our side of the map.
        let d = n.pos.distance(feet) / (0.2 + fit(n.pos));
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((n.pos, d));
        }
    }
    best.map(|(p, _)| p)
}

/// What the bot does this frame.
struct Plan {
    look: Option<AimGoal>,
    /// Motion matching turns the view itself: turn rate (radians/s) and
    /// pitch to settle at.
    turn: Option<f32>,
    pitch: Option<f32>,
    forward: f32,
    right: f32,
    jump: bool,
    sprint: bool,
    stance: Stance,
    fire: bool,
    ads: bool,
    reload: bool,
}

/// Angles (yaw, pitch) from `eye` to `p`.
fn angles_to(eye: Vec3, p: Vec3) -> Vec2 {
    let to = p - eye;
    Vec2::new(f32::atan2(-to.x, -to.z), f32::atan2(to.y, Vec2::new(to.x, to.z).length()))
}

/// Keyboard-style movement: a world direction becomes forward/back and
/// strafe keys relative to where the bot looks (8 directions).
/// Like [`keys_for`], but keep last frame's keys while they still point
/// within 35 degrees of `dir`: people don't swap keys at every wobble of
/// the view.
fn keys_sticky(dir: Vec3, yaw: f32, prev: (f32, f32)) -> (f32, f32) {
    let new = keys_for(dir, yaw);
    if new == prev || prev == (0.0, 0.0) {
        return new;
    }
    let rot = Quat::from_rotation_y(yaw);
    let had = (rot * Vec3::NEG_Z * prev.0 + rot * Vec3::X * prev.1).normalize_or_zero();
    let want = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    if want.dot(had) > 35f32.to_radians().cos() { prev } else { new }
}

fn keys_for(dir: Vec3, yaw: f32) -> (f32, f32) {
    let rot = Quat::from_rotation_y(yaw);
    let (fwd, right) = (rot * Vec3::NEG_Z, rot * Vec3::X);
    let d = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    let (f, r) = (d.dot(fwd), d.dot(right));
    let key = |v: f32| if v > 0.38 { 1.0 } else if v < -0.38 { -1.0 } else { 0.0 };
    (key(f), key(r))
}

#[allow(clippy::too_many_arguments)]
fn act(
    bot: &mut Bot,
    spatial: &SpatialQuery,
    nav: Option<&NavGraph>,
    tc: &TacCtx,
    feet: Vec3,
    eye: Vec3,
    mover: &Mover,
    view: &ViewAngles,
    weapon: &WeaponState,
    now: f32,
    rng: &mut impl Rng,
) -> Plan {
    bot.prev_look = bot.look_src;
    bot.look_src = "none";
    let mut plan = Plan {
        turn: None,
        pitch: None,
        look: None,
        forward: 0.0,
        right: 0.0,
        jump: false,
        sprint: false,
        stance: Stance::Stand,
        fire: false,
        ads: false,
        reload: weapon.clip == 0,
    };
    let p = bot.personality.clone();
    let threat_near = bot.know.contacts.values().any(|c| c.age(now) < 4.0 && c.pos.distance(feet) < u(1500.0));
    // A long trip with nobody about to appear: sprint with eyes on the path,
    // as people do (even towards a fight they know about), until the sprint
    // runs out; then walk and check the angles.
    let contact_imminent =
        bot.know.contacts.values().any(|c| c.noticed() || (c.age(now) < 2.5 && c.pos.distance(feet) < u(900.0)));
    let far_to_go = bot.dest.is_some_and(|d| d.distance(feet) > u(600.0));
    let sprint_leg = bot.sprint_choice && !contact_imminent && far_to_go && mover.sprint_left > 0.5;

    // Aim and shoot at whoever we're fighting, even on the way to cover.
    let fight_dist = if matches!(bot.mode, Mode::Engage | Mode::Cover) {
        let moving = Vec2::new(mover.velocity.x, mover.velocity.z).length() > u(40.0);
        fight_aim(bot, view, eye, feet, moving, weapon, now, rng, &mut plan)
    } else {
        None
    };
    if bot.mode == Mode::Engage {
        let Some(dist) = fight_dist else { return plan };
        // Fight movement: A/D tap strafing, a press one way, then straight
        // the other way or a pause. By default close up it's quick flips; at
        // mid range a jiggle with brief stops to steady the shot; far away
        // mostly standing still, with the odd short strafe.
        if now >= bot.next_strafe {
            let b = profile::band(dist / u(1.0));
            // Log-normal timing around the style's means.
            let spread = (0.35 * gaussian(rng) - 0.06).exp();
            // A teammate right beside us: don't strafe into them.
            let right = Quat::from_rotation_y(view.yaw) * Vec3::X;
            let away = tc
                .mates
                .iter()
                .filter(|m| m.feet.distance(feet) < MATE_SPACING)
                .map(|m| -(m.feet - feet).dot(right).signum())
                .next();
            bot.strafe = if bot.strafe == 0.0 {
                away.unwrap_or(if rng.random::<bool>() { 1.0 } else { -1.0 })
            } else if rng.random::<f32>() < bot.style.flip[b] && away != Some(bot.strafe) {
                -bot.strafe
            } else {
                0.0
            };
            // Livelier players pause less, and better ones (weaker ones
            // stand and shoot).
            let still = (1.0 + 0.8 * (0.6 - bot.skill)).clamp(0.7, 1.4);
            let wait = spread
                * if bot.strafe == 0.0 { bot.style.pause[b] * (1.5 - p.aggression) * still } else { bot.style.press[b] };
            bot.next_strafe = now + wait;
            // Jump-shots, for those who do them.
            if bot.style.jumps > 0.0 && plan.stance == Stance::Stand {
                plan.jump = rng.random::<f32>() < bot.style.jumps / 60.0 * wait;
                bot.jump_src = "fight";
            }
            // Pushing in or backing off, for profiles that measure it.
            if let Some(push) = bot.style.push {
                let r = rng.random::<f32>();
                bot.advance = if r < push.max(0.0) { 1.0 } else if r < push.abs() { -1.0 } else { 0.0 };
            }
        }
        let rot = Quat::from_rotation_y(view.yaw);
        let right = rot * Vec3::X;
        // Don't strafe into walls or off ledges.
        if bot.strafe != 0.0 && !walkable(spatial, nav, feet, right * bot.strafe) {
            bot.strafe = -bot.strafe;
            if !walkable(spatial, nav, feet, right * bot.strafe) {
                bot.strafe = 0.0;
            }
        }
        plan.right = bot.strafe;
        // Two teammates on one enemy: spread out rather than both closing in.
        let crowded = tc.mates.iter().any(|m| m.feet.distance(feet) < MATE_SPACING);
        if bot.style.push.is_some() {
            plan.forward = if crowded { bot.advance.min(0.0) } else { bot.advance };
        } else if dist > p.preferred_range * 1.4 && p.aggression > 0.4 && !crowded {
            plan.forward = 1.0;
        } else if dist < p.preferred_range * 0.5 && p.aggression < 0.5 {
            plan.forward = -1.0;
        }
        // Prone: no strafing.
        if plan.stance == Stance::Prone {
            plan.right = 0.0;
            plan.forward = 0.0;
        }
        // Close fights: crouch spam to throw off aim (by default only
        // aggressive players do).
        let taps = bot.style.crouch_taps.unwrap_or(if p.aggression > 0.6 { 2.9 } else { 0.0 });
        if dist < u(500.0) && taps > 0.3 {
            if now >= bot.next_crouch_tap {
                bot.crouch_tap = !bot.crouch_tap;
                bot.next_crouch_tap = now + (1.0 / taps) * rng.random_range(0.6..1.4);
            }
            if bot.crouch_tap {
                plan.stance = Stance::Crouch;
            }
        }
        return plan;
    }

    // Everything else is moving somewhere and looking around.
    if bot.dest.is_none() && bot.mode == Mode::Hold {
        // Arrived (or no cover): just hold.
    }
    let t_follow = std::time::Instant::now();
    let arrived = follow_path(bot, spatial, nav, tc, feet, mover, now, rng, &mut plan, view);
    phase(5, t_follow);
    if arrived {
        match bot.mode {
            Mode::Hunt => {
                bot.dest = None; // decide() picks a new one
                bot.next_decide = now;
            }
            Mode::Investigate | Mode::Flank | Mode::Objective => {
                // Nobody here: hold the area a moment (a flag until it's
                // ours), watching the lanes they'd come back through.
                if bot.mode != Mode::Objective {
                    bot.set_mode(Mode::Hold, now);
                }
                bot.hold_until = now + rng.random_range(1.5..4.0) + 4.0 * p.patience;
                if let Some(t) = tc.tactics {
                    let mut near: Vec<(Vec3, f32)> = t
                        .lanes_near(feet, u(1500.0))
                        .filter(|(_, l)| l.pos.distance(feet) > u(200.0) && line_clear(spatial, eye, l.pos + Vec3::Y * u(60.0)))
                        .map(|(k, l)| (l.pos, t.danger(k, tc.front, tc.intel, tc.team)))
                        .collect();
                    near.sort_by(|a, b| b.1.total_cmp(&a.1));
                    let angles: Vec<Vec3> = near.into_iter().take(3).map(|n| n.0).collect();
                    if !angles.is_empty() {
                        let ads = rng.random::<f32>() < HOLD_ADS;
                        let stance = hold_stance(spatial, feet, angles.first().map(|a| *a + Vec3::Y * u(60.0)), p.patience, rng);
                        bot.hold = Some(HoldPlan { spot: None, angles, angle_i: 0, next_switch: 0.0, stance, ads });
                    }
                }
            }
            _ => {}
        }
    }

    // Where to look: a remembered enemy (pre-aim at head height), else ahead
    // along the route with glances into open sightlines.
    let focus: Option<(Contact, Entity)> = bot
        .focus
        .and_then(|e| bot.know.contacts.get(&e).map(|c| (*c, e)))
        .or_else(|| {
            bot.know
                .contacts
                .iter()
                .filter(|(_, c)| c.age(now) < 10.0)
                .min_by(|a, b| a.1.age(now).total_cmp(&b.1.age(now)))
                .map(|(e, c)| (*c, *e))
        });
    let holding = matches!(bot.mode, Mode::Post | Mode::Hold | Mode::Objective) && bot.dest.is_none() && bot.hold.is_some();
    let fresh_focus = focus.filter(|(c, _)| c.pos.distance(feet) < u(2500.0) && c.age(now) < 6.0);
    // Crosshair placement on someone we know about: where they would come
    // into view, not through the wall at where they are. With nowhere
    // like that in sight, just look along the way.
    let level = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
    let going = bot.steer.map(|st| level(st - feet));
    let preaim = match focus.filter(|(c, _)| {
        let dist = c.pos.distance(feet);
        // Flankers get to their spot first, then set up on the enemy.
        let flanking = bot.mode == Mode::Flank && far_to_go && dist > u(1200.0);
        // On the move, people mostly look where they're going: pre-aim
        // someone remembered only if they're roughly ahead (unless they were
        // just seen close by).
        let behind = going.is_some_and(|g| level(c.pos - feet).dot(g) < 0.26)
            && !(c.age(now) < 3.0 && dist < u(800.0));
        dist < u(2500.0) && !flanking && !behind && now >= bot.no_preaim_until
    }) {
        Some((c, who)) if plan.look.is_none() => {
            if now >= bot.next_preaim || bot.preaim.is_none_or(|(e, _)| e != who) {
                bot.next_preaim = now + 0.4;
                // Crosshair placement takes skill: weaker players often
                // just look where they're going, and aim low (at the body,
                // not the head) when they don't.
                if rng.random::<f32>() > 1.0 - 1.2 * (0.6 - bot.skill).max(0.0) {
                    bot.no_preaim_until = now + rng.random_range(0.8..1.6);
                }
                let low = u(85.0) * (0.6 - bot.skill).max(0.0);
                let head = c.predicted(now) + Vec3::Y * (u(60.0) - low);
                let point = if line_clear(spatial, eye, head) {
                    Some(head)
                } else {
                    nav.and_then(|nav| emergence_point(nav, spatial, feet, eye, c.predicted(now)))
                };
                bot.preaim = point.map(|p| (who, p));
            }
            bot.preaim.filter(|(e, _)| *e == who).map(|(_, p)| (c, p))
        }
        _ => None,
    };
    // An alarm: flick round to them once it has registered, until they're
    // found (then it's a fight) or it's clear they're not there.
    if bot.alarm.is_some_and(|a| now > a.until) {
        bot.alarm = None;
    }
    // Not through a wall at them: where they'd come into view.
    let alarm_contact = bot
        .alarm
        .filter(|a| now >= a.at)
        .and_then(|a| bot.know.contacts.get(&a.target).copied())
        .filter(|c| !c.noticed());
    let alarm_point = match (alarm_contact, bot.alarm.as_mut()) {
        (Some(c), Some(alarm)) => {
            if now >= alarm.next_point {
                alarm.next_point = now + 0.3;
                let head = c.predicted(now) + Vec3::Y * u(60.0);
                alarm.point = if line_clear(spatial, eye, head) {
                    Some(head)
                } else {
                    nav.and_then(|nav| emergence_point(nav, spatial, feet, eye, c.predicted(now))).or(Some(head))
                };
            }
            alarm.point
        }
        _ => None,
    };
    if plan.look.is_some() {
        // Already aiming at someone we're fighting.
    } else if let Some(point) = alarm_point {
        plan.look = Some(AimGoal { angles: angles_to(eye, point), width: 0.08, combat: true });
        bot.look_src = "alarm";
    } else if let (true, None) = (holding, fresh_focus) {
        // Holding an angle: aim at head height where they'd appear, now and
        // then switching to another lane the spot covers.
        let h = bot.hold.as_mut().expect("holding");
        if now >= h.next_switch && !h.angles.is_empty() {
            h.next_switch = now + rng.random_range(2.0..6.0);
            h.angle_i = if rng.random::<f32>() < 0.55 { 0 } else { rng.random_range(0..h.angles.len()) };
        }
        if let Some(&a) = h.angles.get(h.angle_i) {
            plan.look = Some(AimGoal { angles: angles_to(eye, a + Vec3::Y * tactical::CHECK_HEIGHT), width: 0.02, combat: false });
            bot.look_src = "hold";
            plan.ads = h.ads;
        }
        plan.stance = h.stance;
    } else if let Some((c, point)) = preaim {
        plan.look = Some(AimGoal { angles: angles_to(eye, point), width: 0.03, combat: false });
        bot.look_src = "preaim";
        // Not while walking: real players hardly ever move with sights up
        // unless they're shooting.
        let _ = c;
        plan.ads = bot.mode == Mode::Hold && bot.dest.is_none() && bot.hold.as_ref().is_some_and(|h| h.ads);
    } else {
        // Walking a route: move and look the way a real player did in the
        // same situation (motion matching), when there are demos to learn from.
        let matched = match (tc.motion, nav, bot.steer, bot.path.is_empty()) {
            (Some(lib), Some(nav), Some(_), false) => {
                let t = std::time::Instant::now();
                let step = motion_step(bot, lib, nav, spatial, feet, mover, view, now);
                phase(4, t);
                step
            }
            _ => {
                bot.playing = None;
                None
            }
        };
        if let Some(step) = matched {
            bot.check = None;
            bot.glance = None;
            bot.look_src = "motion";
            plan.turn = Some(step.turn * motion::RATE);
            plan.pitch = Some(step.pitch.clamp(-0.6, 0.6));
            // Their movement relative to the view, as keys, unless that way
            // is blocked (then the route's keys stand).
            let speed = Vec2::new(step.forward, step.right).length();
            let rot = Quat::from_rotation_y(view.yaw);
            let world = rot * Vec3::NEG_Z * step.forward + rot * Vec3::X * step.right;
            if speed < 40.0 {
                plan.forward = 0.0;
                plan.right = 0.0;
            } else if walkable(spatial, nav, feet, world) {
                let key = |v: f32| if v > 0.38 { 1.0 } else if v < -0.38 { -1.0 } else { 0.0 };
                plan.forward = key(step.forward / speed);
                plan.right = key(step.right / speed);
            }
            if step.crouch && !step.prone {
                plan.stance = Stance::Crouch;
            }
            bot.matched_sprint = speed > 230.0;
        } else {
        if bot.glance.is_some_and(|(_, until)| now > until) {
            bot.glance = None;
        }
        if sprint_leg {
            bot.check = None;
            bot.glance = None;
        }
        // Check the angles enemies would come from along the way.
        if bot.check.is_some_and(|(_, until)| now > until) {
            bot.check = None;
            // Better players check more of them.
            bot.next_check = now + rng.random_range(1.5..3.5) * (1.0 + 0.6 - bot.skill);
        }
        if let (Some(t), None, Some(steer), false) = (tc.tactics, bot.check, bot.steer, sprint_leg) {
            if now >= bot.next_check {
                bot.next_check = now + 0.5;
                let heading = Vec3::new(steer.x - feet.x, 0.0, steer.z - feet.z).normalize_or_zero();
                if let Some(point) = t.check_point(spatial, eye, feet, heading, tc.front, tc.intel, tc.team, rng) {
                    // Now and then stop to look properly, as real players
                    // pause for a second or two (5% of their time).
                    let pause = rng.random::<f32>() < 0.08 + 0.1 * p.patience;
                    let until = now + if pause { rng.random_range(0.8..2.2) } else { rng.random_range(0.35..0.8) };
                    bot.check = Some((point, until));
                    if pause {
                        bot.pause_until = until;
                    }
                }
            }
        }
        if let Some((point, _)) = bot.check {
            bot.glance = Some((point, now + 0.1));
        } else if bot.glance.is_none() && now >= bot.next_glance && bot.steer.is_some() && tc.tactics.is_none() && !sprint_leg {
            bot.next_glance = now + rng.random_range(1.2..3.5);
            let heading = bot.steer.map(|s| (s - feet).normalize_or_zero()).unwrap_or(view.forward());
            let mut best: Option<(Vec3, f32)> = None;
            for angle in [-100.0f32, -60.0, -30.0, 30.0, 60.0, 100.0] {
                let dir = Quat::from_rotation_y(angle.to_radians()) * Vec3::new(heading.x, 0.0, heading.z).normalize_or_zero();
                let Ok(d) = Dir3::new(dir) else { continue };
                let reach = spatial
                    .cast_ray(eye, d, u(3000.0), true, &collision::sight_filter())
                    .map_or(u(3000.0), |h| h.distance);
                if reach > u(400.0) && best.is_none_or(|(_, r)| reach > r) {
                    best = Some((eye + dir * reach.min(u(1500.0)), reach));
                }
            }
            if let Some((point, _)) = best {
                bot.glance = Some((point, now + rng.random_range(0.35..0.9)));
            }
        }
        if bot.ahead.is_none_or(|(_, until)| now >= until) {
            let point = nav.filter(|_| !bot.path.is_empty()).and_then(|nav| look_ahead(nav, &bot.path, bot.path_i, feet, eye));
            // Glide rather than jump when the route point ahead changes.
            let smoothed = match (point, bot.ahead) {
                (Some(p), Some((old, _))) if old.distance(p) < u(200.0) => Some(old.lerp(p, 0.35)),
                (p, _) => p,
            };
            bot.ahead = smoothed.map(|p| (p, now + 0.1));
        }
        let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
        let point = match (bot.glance, bot.ahead, bot.steer) {
            (Some((g, _)), _, _) => Some(g),
            (None, Some((a, _)), Some(_)) => Some(a),
            // No route (the last few steps): level, the way we're going.
            (None, _, Some(s)) => Some(eye + flat(s - feet) * u(300.0)),
            _ => None,
        };
        if let Some(point) = point {
            plan.look = Some(AimGoal { angles: angles_to(eye, point), width: 0.05, combat: false });
            bot.look_src = match (bot.check.is_some(), bot.glance.is_some()) {
                (true, _) => "check",
                (false, true) => "glance",
                _ => "steer",
            };
        }
        }
    }

    // Peeking: settled behind cover with an enemy around the corner, step out
    // with sights up, then back in.
    let settled = matches!(bot.mode, Mode::Cover | Mode::Hold) && bot.dest.is_none();
    let peek_target = fresh_focus.map(|(c, _)| c.predicted(now) + Vec3::Y * u(60.0));
    if let (true, Some(target), false) = (settled, peek_target, weapon.reloading() || weapon.clip == 0) {
        let right = Quat::from_rotation_y(view.yaw) * Vec3::X;
        match bot.peek {
            None if now >= bot.next_peek => {
                // Which side of our cover sees them?
                let side = [1.0f32, -1.0].into_iter().find(|&side| {
                    let from = eye + right * side * u(56.0);
                    !line_clear(spatial, eye, target) && line_clear(spatial, from, target)
                        && walkable(spatial, nav, feet, right * side)
                });
                match side {
                    Some(side) => bot.peek = Some((side, now + rng.random_range(0.3..0.6), false)),
                    None => bot.next_peek = now + 1.0,
                }
            }
            Some((side, until, back)) => {
                plan.right = if back { -side } else { side };
                plan.ads = true;
                if now >= until {
                    if back {
                        bot.peek = None;
                        bot.next_peek = now + rng.random_range(0.8..2.5) * (1.5 - p.aggression);
                    } else {
                        bot.peek = Some((side, now + rng.random_range(0.3..0.6), true));
                    }
                }
            }
            None => {}
        }
    } else {
        bot.peek = None;
    }

    // Reload when it's quiet; sprint on long quiet trips.
    if (!threat_near || !bot.careful_reload) && weapon.clip < weapon.def.clip_size * 3 / 5 {
        plan.reload = true;
    }
    // On a sprint leg, roughly facing the path: just run forward (the view
    // keeps turning along it) rather than holding a diagonal.
    if let (true, Some(steer)) = (sprint_leg, bot.steer) {
        let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
        if plan.forward > 0.5 && flat(steer - feet).dot(flat(view.forward())) > 0.75 {
            plan.right = 0.0;
        }
    }
    let travelling = bot.steer.is_some() && plan.forward > 0.5 && plan.right == 0.0;
    // Sprint has to be held, so the choice sticks for a few seconds.
    if now >= bot.next_sprint_choice {
        bot.sprint_choice = rng.random::<f32>() < p.sprinter;
        bot.next_sprint_choice = now + rng.random_range(2.0..4.0);
    }
    plan.sprint = travelling && !contact_imminent && !plan.ads && bot.glance.is_none() && bot.sprint_choice;
    if bot.look_src == "motion" {
        plan.sprint = bot.matched_sprint && !contact_imminent;
    }
    if bot.mode == Mode::Post && bot.dest.is_none() {
        if let Some(h) = &bot.hold {
            plan.stance = h.stance;
        }
    }
    // Pausing to check an angle (never mid-fight).
    if now < bot.pause_until && !matches!(bot.mode, Mode::Engage | Mode::Cover) && fresh_focus.is_none() {
        plan.forward = 0.0;
        plan.right = 0.0;
        plan.sprint = false;
    }
    if matches!(bot.mode, Mode::Hold) && bot.steer.is_none() {
        // Patient players crouch behind low cover while they wait.
        if bot.hold_crouch.is_none_or(|(at, _)| at.distance(feet) > u(30.0)) {
            let toward = fresh_focus.map_or(eye + view.forward() * u(1000.0), |(c, _)| c.predicted(now) + Vec3::Y * u(60.0));
            bot.hold_crouch = Some((feet, hold_stance(spatial, feet, Some(toward), p.patience, rng)));
        }
        if let Some((_, stance)) = bot.hold_crouch {
            plan.stance = stance;
        }
    }
    // Closing in on someone, sometimes crouch-walking (real players move
    // crouched 11% of the time out of fights); decided once per approach.
    if matches!(bot.mode, Mode::Investigate | Mode::Flank) && !plan.sprint && plan.stance == Stance::Stand {
        let near = fresh_focus.is_some_and(|(c, _)| c.pos.distance(feet) < u(700.0));
        let roll = ((bot.mode_since.to_bits() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40) as f32 / (1u64 << 24) as f32;
        if near && roll < 0.15 + 0.35 * p.patience {
            plan.stance = Stance::Crouch;
        }
    }
    // The view's pitch out of fights: the habit and a wander (real players
    // out of fights: mean 3.7 degrees down, spread 8), less for better
    // players. Motion matching has real players' own pitch.
    let dt = (now - bot.drift_at).clamp(0.0, 0.1);
    bot.drift_at = now;
    const DRIFT_TIME: f32 = 1.5;
    let sigma = (4.0 * (1.3 - 0.6 * bot.skill)).to_radians();
    bot.pitch_drift += -bot.pitch_drift * dt / DRIFT_TIME + sigma * (2.0 * dt / DRIFT_TIME).sqrt() * gaussian(rng);
    if let (Some(goal), None) = (plan.look.as_mut().filter(|g| !g.combat), plan.turn) {
        goal.angles.y += bot.pitch_habit + bot.pitch_drift;
    }
    // Climbing: face the mantle (it goes the way the view does).
    if let Some(n) = bot.mantle_to.take() {
        plan.look = Some(AimGoal { angles: angles_to(eye, Vec3::new(n.x, eye.y, n.z)), width: 0.05, combat: false });
        plan.turn = None;
        plan.pitch = None;
        plan.stance = Stance::Stand;
        plan.sprint = false;
        bot.look_src = "mantle";
    }
    plan
}

/// Chance of holding a spot with sights up; watching a long sightline.
const HOLD_ADS: f32 = 0.1;
const HOLD_ADS_FAR: f32 = 0.35;
/// On the move, aiming down sights is as likely as standing still this
/// many times closer. Set so bots in fights on Killhouse have sights up
/// as much of their moving time as real players there (31%; at 3.2, from
/// fitting players' shots by distance, bots fighting at their usual 800
/// to 1800 units had them up 54%).
const ADS_MOVING_SCALE: f32 = 8.5;

/// Chance of aiming down sights in a fight at `dist`, where `half` is the
/// distance a standing player does it half the time. Fitted to real
/// players firing at someone in their crosshair:
/// `1 / (1 + (half / dist)^1.5)`.
fn ads_chance(dist: f32, moving: bool, half: f32) -> f32 {
    let half = if moving { half * ADS_MOVING_SCALE } else { half };
    if half <= 0.0 {
        return 1.0;
    }
    1.0 / (1.0 + (half / dist.max(u(1.0))).powf(1.5))
}

/// Aim at and shoot the current engagement's target. Returns the distance
/// to it, or None without one.
#[allow(clippy::too_many_arguments)]
fn fight_aim(
    bot: &mut Bot,
    view: &ViewAngles,
    eye: Vec3,
    feet: Vec3,
    moving: bool,
    weapon: &WeaponState,
    now: f32,
    rng: &mut impl Rng,
    plan: &mut Plan,
) -> Option<f32> {
    let skill = bot.skill;
    let ads_half = bot.style.ads_range;
    let eng = bot.engagement.as_mut()?;
    let c = bot.know.contacts.get(&eng.target).copied()?;
    if c.noticed() {
        eng.last_visible = now;
    }
    let pos = c.predicted(now);
    let dist = pos.distance(feet);
    // Bot lab experiment `stanceaim`: at their head and chest as they stand
    // or crouch (head 4 above the eyes, chest about 0.71 of the eye height:
    // 64/43 standing, 44/28 crouched), not fixed heights that went over a
    // crouched target.
    let point = if lab::on("stanceaim") {
        pos + Vec3::Y * if eng.aim_head { c.eye_height + u(4.0) } else { c.eye_height * 0.71 }
    } else if eng.aim_head {
        pos + Vec3::Y * u(62.0)
    } else {
        pos + Vec3::Y * u(44.0)
    };
    let half = if eng.aim_head { u(5.0) } else { u(9.0) };
    let width = 2.0 * (half / dist.max(u(16.0))).atan();
    let goal = angles_to(eye, point);
    let reacted = now >= eng.react_at;
    bot.look_src = if reacted { "fight" } else { "react" };
    if reacted {
        plan.look = Some(AimGoal { angles: goal, width, combat: true });
    } else {
        // Not reacted yet: the view stays where it was (nothing else may
        // turn it towards them early).
        plan.look = Some(AimGoal { angles: Vec2::new(view.yaw, view.pitch), width: 0.05, combat: false });
    }
    // Sights up or from the hip, as real players do: more often the further
    // away, much less on the move (in fights in the demos: 79% of the time
    // standing, 31% moving). The roll is kept a while (they go into sights
    // about 14 times a minute; rolling every second doubled that), and
    // re-read once they've started or stopped moving for a moment, so a
    // choice made standing doesn't carry on into running about.
    if moving != eng.moving {
        eng.moving = moving;
        eng.moving_since = now;
    }
    if now >= eng.ads_next {
        eng.ads_roll = rng.random();
        eng.ads_next = now + rng.random_range(1.5..3.0);
        eng.ads = eng.ads_roll < ads_chance(dist, moving, ads_half);
    }
    if now - eng.moving_since > if moving { 0.35 } else { 0.15 } {
        eng.ads = eng.ads_roll < ads_chance(dist, moving, ads_half);
    }
    // And down once they've lost sight of them for a second.
    plan.ads = eng.ads && reacted && now - eng.last_visible < 1.0;
    if eng.drop {
        plan.stance = Stance::Prone;
    } else if eng.crouch {
        plan.stance = Stance::Crouch;
    }

    // Trigger: people start firing a little before they are on target
    // (less so when skilled), in bursts by range, and keep a burst going
    // through small misses for about a reaction time.
    if reacted && c.noticed() && !weapon.reloading() {
        let err = AimState::error(view, goal).length();
        let start = width * (1.6 - 1.0 * skill) + 0.015 - 0.011 * skill;
        let keep = width * 3.0 + 0.035;
        if err > keep {
            bot.burst_until = bot.burst_until.min(now + 0.12);
        }
        if err < start || now < bot.burst_until {
            if now >= bot.next_burst && err < start {
                let (burst, pause) = if dist < u(600.0) {
                    (rng.random_range(0.6..1.5), rng.random_range(0.05..0.15))
                } else if dist < u(1500.0) {
                    (rng.random_range(0.25..0.5), rng.random_range(0.15..0.35))
                } else {
                    (rng.random_range(0.08..0.18), rng.random_range(0.25..0.5))
                };
                bot.burst_until = now + burst;
                bot.next_burst = bot.burst_until + pause;
            }
            plan.fire = now < bot.burst_until;
            if plan.fire && eng.first_shot.is_none() {
                eng.first_shot = Some(now);
                if std::env::var_os("COD4RW_SIM").is_some() {
                    info!(
                        "engage: seen->noticed {:.2}s noticed->shot {:.2}s seen->shot {:.2}s at {:.0}u",
                        c.noticed_at.unwrap_or(now) - eng.seen_at,
                        now - c.noticed_at.unwrap_or(now),
                        now - eng.seen_at,
                        dist / u(1.0)
                    );
                }
            }
        }
    }
    // Lost sight for a moment: keep aiming where they were.
    if !c.noticed() {
        plan.look = Some(AimGoal { angles: goal, width: width.max(0.02), combat: false });
    }
    Some(dist)
}

/// Knife someone right in front: nearly always with an empty gun, usually
/// when they're close enough to touch, and now and then from a few steps
/// away (a lunge); better players more readily.
#[allow(clippy::type_complexity)]
fn knife(
    time: Res<Time>,
    mut bots: Query<
        (&mut Bot, &Transform, &Mover, &ViewAngles, &WeaponState, Option<&mut crate::melee::MeleeInput>, Has<crate::melee::Melee>),
        Without<Dead>,
    >,
) {
    let now = time.elapsed_secs();
    let mut rng = rand::rng();
    for (mut bot, tf, mover, view, weapon, input, swinging) in &mut bots {
        let Some(mut input) = input else { continue };
        input.pressed = false;
        if swinging || now < bot.next_melee {
            continue;
        }
        bot.next_melee = now + 0.25;
        let feet = tf.translation;
        let forward = Vec3::new(view.forward().x, 0.0, view.forward().z).normalize_or_zero();
        let near = bot
            .know
            .contacts
            .values()
            .filter(|c| c.noticed() && (c.pos.y - feet.y).abs() < u(45.0))
            .map(|c| Vec3::new(c.pos.x - feet.x, 0.0, c.pos.z - feet.z))
            .filter(|d| d.normalize_or_zero().dot(forward) > 0.85)
            .map(|d| d.length())
            .min_by(f32::total_cmp);
        let Some(dist) = near.filter(|d| *d < u(110.0)) else { continue };
        let empty = weapon.clip == 0 || weapon.reloading();
        let chance = if empty {
            0.9
        } else if dist < u(70.0) {
            0.55 + 0.35 * bot.skill
        } else {
            0.15 + 0.25 * bot.skill
        };
        input.pressed = rng.random::<f32>() < chance;
        if input.pressed {
            bot.next_melee = now + 1.0;
            let _ = mover;
        }
    }
}

/// Where an enemy at `enemy` would first come into view if it walked toward
/// us: the last point along the route between us that we can still see, at
/// head height.
fn emergence_point(nav: &NavGraph, spatial: &SpatialQuery, feet: Vec3, eye: Vec3, enemy: Vec3) -> Option<Vec3> {
    let path = nav.path(nav.nearest(feet)?, nav.nearest(enemy)?, 8_000, |_| 0.0)?;
    let mut last_visible = None;
    for (k, &i) in path.iter().enumerate() {
        let head = nav.nodes[i as usize].pos + Vec3::Y * u(60.0);
        if line_clear(spatial, eye, head) {
            last_visible = Some(head);
        } else if k > 2 {
            break;
        }
    }
    last_visible
}

/// Whether a few steps in `dir` stay on walkable ground.
fn walkable(spatial: &SpatialQuery, nav: Option<&NavGraph>, feet: Vec3, dir: Vec3) -> bool {
    let probe = feet + dir.normalize_or_zero() * u(48.0);
    let wall_free = {
        let shape = Collider::capsule(u(14.0), u(20.0));
        let origin = feet + Vec3::Y * u(40.0);
        Dir3::new(dir).ok().is_none_or(|d| {
            spatial
                .cast_shape(&shape, origin, Quat::IDENTITY, d, &ShapeCastConfig::from_max_distance(u(48.0)), &collision::movement_filter())
                .is_none()
        })
    };
    let ground = nav.is_none_or(|nav| nav.nearest(probe).is_some_and(|i| nav.nodes[i as usize].pos.distance(probe) < u(48.0)));
    wall_free && ground
}

/// Move along the path to `bot.dest`. Returns true on arrival.
#[allow(clippy::too_many_arguments)]
fn follow_path(
    bot: &mut Bot,
    spatial: &SpatialQuery,
    nav: Option<&NavGraph>,
    tc: &TacCtx,
    feet: Vec3,
    mover: &Mover,
    now: f32,
    rng: &mut impl Rng,
    plan: &mut Plan,
    view: &ViewAngles,
) -> bool {
    bot.steer = None;
    bot.mantle_to = None;
    let Some(dest) = bot.dest else { return false };
    let flat = |a: Vec3, b: Vec3| Vec2::new(a.x - b.x, a.z - b.z).length();
    if flat(feet, dest) < u(40.0) && (feet.y - dest.y).abs() < u(60.0) {
        bot.dest = None;
        bot.path.clear();
        return true;
    }

    // Plan a route: a search at a time, a slice of it a frame (the rest keep
    // their way a little longer).
    let wants_route = bot.path.is_empty() || now >= bot.next_repath || bot.routing;
    if wants_route && ROUTES_THIS_FRAME.load(std::sync::atomic::Ordering::Relaxed) < ROUTES_PER_FRAME {
        if !bot.routing {
            bot.next_repath = now + 3.0;
        }
        if let Some(nav) = nav {
            if let (Some(a), Some(b)) = (reachable_node(nav, spatial, feet), nav.nearest(dest)) {
                // Prefer routes along walls and cover over open ground;
                // rushers take the main lanes, flankers avoid them.
                let caution = 0.3 + 0.7 * (1.0 - bot.personality.aggression);
                let claymores = bot.claymores.clone();
                let role = bot.role;
                let traffic = |i: u32| tc.tactics.and_then(|t| t.traffic.get(i as usize)).copied().unwrap_or(0.0);
                // And not the way a teammate is already going, least of all
                // one we've caught ourselves following.
                let leader = bot.leader.filter(|l| now < l.1).map(|l| l.0);
                // Per cell: how many teammates' routes pass (up to 2), and
                // whether the leader's does; counted once, not per point.
                let mut on_routes: bevy::platform::collections::HashMap<Cell, (u8, bool)> = Default::default();
                for m in &tc.mates {
                    for c in &m.route {
                        let e = on_routes.entry(*c).or_default();
                        e.0 = e.0.saturating_add(1);
                        e.1 |= Some(m.entity) == leader;
                    }
                }
                let shared = |i: u32| {
                    on_routes
                        .get(&cell(nav.nodes[i as usize].pos))
                        .map_or(0.0, |&(n, led)| n.min(2) as f32 + if led { 2.0 } else { 0.0 })
                };
                let cost = |i: u32| {
                    let lane = match role {
                        Role::Rusher => (1.0 - traffic(i)) * u(8.0),
                        Role::Flanker => traffic(i) * u(40.0),
                        Role::Anchor => 0.0,
                    };
                    let pos = nav.nodes[i as usize].pos;
                    let side = tc.lateral.map_or(0.0, |l| (l.of(pos) - bot.lane).abs().min(1.5));
                    let trouble = if tc.trouble.is_empty() { 0.0 } else { tc.trouble.get(&i).map_or(0.0, |&n| n.min(4) as f32 * u(300.0)) };
                    // In front of a known enemy claymore (its 192 units and
                    // 70 degrees, with room to spare).
                    let claymore = claymores
                        .iter()
                        .any(|(_, c, f)| (pos - *c).length() < u(240.0) && (pos - *c).normalize_or_zero().dot(*f) > 0.6);
                    let claymore = if claymore { u(4000.0) } else { 0.0 };
                    nav.nodes[i as usize].exposure * caution * u(24.0) + lane + shared(i) * u(16.0) + side * u(24.0) + trouble + claymore
                };
                let t0 = std::time::Instant::now();
                let found = nav.path_sliced(bot.route_id, a, b, now, 60_000, ROUTE_SLICE, cost);
                if !matches!(found, nav::Step::Busy) {
                    ROUTES_THIS_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    clock(&PATH_US, &PATH_MAX_US, t0);
                }
                bot.routing = matches!(found, nav::Step::Working);
                match found {
                    // Not done (or another bot's search running): ask again
                    // next frame.
                    nav::Step::Working => {}
                    nav::Step::Busy => bot.next_repath = now,
                    nav::Step::Found(path) => {
                        PATHS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        bot.path = path;
                        bot.path_i = 0;
                    }
                    nav::Step::NoWay => {
                        // Unreachable: give up on this destination (and
                        // for a while on an objective there).
                        bot.dest = None;
                        bot.next_decide = now;
                        if let Some(g) = bot.goal.filter(|_| bot.mode == Mode::Objective) {
                            bot.blocked.retain(|b| b.1 > now);
                            bot.blocked.push((g, now + objective::BLOCKED_FOR));
                            debug!("bot: no route to {g:?}; leaving it {:.0}s", objective::BLOCKED_FOR);
                        }
                        return false;
                    }
                }
            }
        }
    }

    // Next waypoint: skip ahead to the furthest one we can walk straight to.
    let steer = if let Some(nav) = nav.filter(|_| !bot.path.is_empty()) {
        while bot.path_i < bot.path.len() {
            let n = nav.nodes[bot.path[bot.path_i] as usize].pos;
            if flat(n, feet) < u(28.0) && (n.y - feet.y).abs() < u(40.0) {
                bot.path_i += 1;
            } else {
                break;
            }
        }
        if bot.path_i >= bot.path.len() {
            bot.path.clear();
            Some(dest)
        } else {
            if now >= bot.next_shortcut {
                bot.next_shortcut = now + 0.25;
                let last = (bot.path_i + 6).min(bot.path.len() - 1);
                let mut skipped = false;
                for j in (bot.path_i + 1..=last).rev() {
                    let n = nav.nodes[bot.path[j] as usize].pos;
                    if (n.y - feet.y).abs() < u(16.0) && straight_walk(spatial, feet, n) {
                        bot.path_i = j;
                        skipped = true;
                        break;
                    }
                }
                // Pushed off the route (by a teammate, a strafe, a fight):
                // if the way to the next point is blocked, go back to one we
                // can walk to, or plan again from here.
                let next = nav.nodes[bot.path[bot.path_i] as usize].pos;
                // (A mantle over a wall is blocked: it's climbed.)
                let climb = bot.path_i > 0 && nav.link(bot.path[bot.path_i - 1], bot.path[bot.path_i]).is_some_and(|l| l.mantle);
                if !skipped && !climb && (next.y - feet.y).abs() < u(16.0) && flat(next, feet) > u(20.0) && !straight_walk(spatial, feet, next) {
                    let back = (bot.path_i.saturating_sub(3)..bot.path_i).rev().find(|&j| {
                        let n = nav.nodes[bot.path[j] as usize].pos;
                        (n.y - feet.y).abs() < u(16.0) && straight_walk(spatial, feet, n)
                    });
                    match back {
                        Some(j) => bot.path_i = j,
                        None => bot.next_repath = now,
                    }
                }
            }
            let i = bot.path_i;
            // Jump links and crouch-only spots.
            if i > 0 {
                if let Some(link) = nav.link(bot.path[i - 1], bot.path[i]) {
                    let n = nav.nodes[bot.path[i] as usize].pos;
                    if link.jump && flat(n, feet) < u(56.0) {
                        plan.jump = true;
                        bot.jump_src = "nav link";
                    }
                    // A mantle: at its foot, face it and jump once it's there
                    // to climb (a jump short of it would just hop).
                    if link.mantle && flat(n, feet) < u(90.0) && mover.mantle.is_none() {
                        bot.mantle_to = Some(n);
                        let level = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
                        if level(view.forward()).dot(level(n - feet)) > 0.8
                            && mover.on_ground
                            && crate::movement::mantle_landing(spatial, feet, view.yaw, &[]).is_some()
                        {
                            plan.jump = true;
                            bot.jump_src = "mantle";
                        }
                    }
                }
            }
            if nav.nodes[bot.path[i] as usize].crouch_only {
                plan.stance = Stance::Crouch;
            }
            Some(nav.nodes[bot.path[i] as usize].pos)
        }
    } else {
        Some(dest)
    };

    let Some(steer) = steer else { return false };
    bot.steer = Some(steer);
    // Spacing: let a teammate going the same way get a few steps ahead,
    // and step around one that's in the way.
    let flat_dir = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
    let to = flat_dir(steer - feet);
    let mut dir = to;
    let mut follow = None;
    for m in &tc.mates {
        let d = Vec3::new(m.feet.x - feet.x, 0.0, m.feet.z - feet.z);
        let dist = d.length();
        if dist < 1.0 {
            continue;
        }
        let along = to.dot(d / dist);
        if dist < u(200.0) && along > 0.7 && flat_dir(m.vel).dot(to) > 0.5 && m.vel.length() > u(60.0) {
            follow = Some(m.entity);
        }
        if dist < u(80.0) {
            dir -= d / dist * (1.0 - dist / u(80.0)) * 1.2;
        }
    }
    match (follow, bot.yield_from) {
        (Some(leader), None) => {
            // Following someone: hang back a moment and find another way.
            bot.yield_from = Some(now);
            bot.leader = Some((leader, now + 6.0));
            bot.next_repath = now;
        }
        (None, Some(_)) => bot.yield_from = None,
        _ => {}
    }
    let (f, r) = if bot.yield_from.is_some_and(|t| now - t < 0.6) {
        bot.progress_time = now;
        (0.0, 0.0)
    } else {
        let mut keys = keys_sticky(dir, view.yaw, bot.last_keys);
        // Looking along the route: hold W and let the view do the steering,
        // as players do, while the way is roughly ahead and clear.
        let facing = Vec3::new(view.forward().x, 0.0, view.forward().z).normalize_or_zero();
        let want = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
        let clear_ahead = walkable(spatial, nav, feet, facing);
        // Once steering like that, keep at it through bends (up to 65°).
        let limit = if bot.last_keys == (1.0, 0.0) { 65f32 } else { 45f32 };
        if bot.prev_look == "steer" && want.dot(facing) > limit.to_radians().cos() && clear_ahead {
            keys = (1.0, 0.0);
        } else if keys != keys_for(dir, view.yaw) {
            // Only if that way isn't into a wall.
            let rot = Quat::from_rotation_y(view.yaw);
            let had = rot * Vec3::NEG_Z * keys.0 + rot * Vec3::X * keys.1;
            if !walkable(spatial, nav, feet, had) {
                keys = keys_for(dir, view.yaw);
            }
        }
        bot.last_keys = keys;
        keys
    };
    plan.forward = f;
    plan.right = r;

    // Unstick: step aside (a teammate in a doorway, a corner caught), then
    // re-plan, jumping only if the way on climbs, then give up on the
    // destination. (Hopping first made bots jump three times as often as
    // real players.)
    if let Some((side, until)) = bot.unstick {
        if now < until {
            plan.forward = -0.3;
            plan.right = side;
            plan.sprint = false;
        } else {
            bot.unstick = None;
        }
    }
    if feet.distance(bot.progress_pos) > u(32.0) {
        bot.progress_pos = feet;
        bot.progress_time = now;
        bot.stuck = 0;
    } else if now - bot.progress_time > 1.0 + bot.stuck as f32 {
        bot.stuck += 1;
        bot.progress_time = now;
        match bot.stuck {
            1 => bot.unstick = Some((if rng.random::<bool>() { 1.0 } else { -1.0 }, now + 0.5)),
            2 => {
                // Remember the point we couldn't get to, for everyone.
                let next = bot.path.get(bot.path_i).copied();
                bot.stuck_at = next;
                let climb = next.zip(nav).is_some_and(|(n, nav)| nav.nodes[n as usize].pos.y - feet.y > crate::movement::STEP_HEIGHT * 0.8);
                bot.path.clear();
                if climb {
                    plan.jump = true;
                    bot.jump_src = "unstick";
                }
            }
            _ => {
                bot.dest = None;
                bot.path.clear();
                bot.stuck = 0;
                bot.next_decide = now;
            }
        }
    }
    let _ = mover;
    false
}

/// Wrap an angle difference to -pi..pi.
fn wrap_angle(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// Motion matching for a bot walking its route: re-pick the clip that best
/// fits its situation a few times a second, and return this frame's step.
#[allow(clippy::too_many_arguments)]
fn motion_step(
    bot: &mut Bot,
    lib: &motion::Library,
    nav: &NavGraph,
    spatial: &SpatialQuery,
    feet: Vec3,
    mover: &Mover,
    view: &ViewAngles,
    now: f32,
) -> Option<motion::Step> {
    let fresh = bot.playing.as_ref().is_some_and(|p| now - p.started < 0.25 && p.step(now).is_some());
    if !fresh {
        // Where the route has us in a second, relative to the view.
        let speed = (Vec2::new(mover.velocity.x, mover.velocity.z).length() / u(1.0)).max(170.0);
        let goal = route_point(nav, &bot.path, bot.path_i, feet, u(speed))?;
        let rot = Quat::from_rotation_y(view.yaw);
        let (fwd, right) = (rot * Vec3::NEG_Z, rot * Vec3::X);
        let g = (goal - feet) / u(1.0);
        let v = mover.velocity / u(1.0);
        let f = motion::features(g.dot(fwd), g.dot(right), v.dot(fwd), v.dot(right), bot.turn_rate, view.pitch);
        // Prefer carrying on with the same player's motion where it fits.
        let from = bot.playing.as_ref().map(|p| (p.sample, ((now - p.started) * motion::RATE).round() as usize));
        let sample = lib.best(&f, from)?;
        bot.playing = Some(motion::Playing { sample, clip: *lib.clip(sample), started: now });
    }
    let _ = spatial;
    bot.playing.as_ref().and_then(|p| p.step(now))
}

/// The point `dist` along a route, from `feet`.
fn route_point(nav: &NavGraph, path: &[u32], from: usize, feet: Vec3, dist: f32) -> Option<Vec3> {
    let (mut walked, mut prev) = (0.0, feet);
    for &i in path.iter().skip(from).take(64) {
        let p = nav.nodes[i as usize].pos;
        let step = p.distance(prev);
        if walked + step >= dist {
            return Some(prev.lerp(p, (dist - walked) / step.max(1e-3)));
        }
        walked += step;
        prev = p;
    }
    (walked > u(20.0)).then_some(prev)
}

/// The nav point to start a route from: the nearest one we can walk
/// straight to (the nearest outright may be through a wall).
fn reachable_node(nav: &NavGraph, spatial: &SpatialQuery, feet: Vec3) -> Option<u32> {
    let mut near = nav.within(feet, u(96.0));
    near.sort_by(|&a, &b| nav.nodes[a as usize].pos.distance(feet).total_cmp(&nav.nodes[b as usize].pos.distance(feet)));
    near.into_iter()
        .take(8)
        .find(|&i| {
            let n = nav.nodes[i as usize].pos;
            n.distance(feet) < u(20.0) || ((n.y - feet.y).abs() < u(16.0) && straight_walk(spatial, feet, n))
        })
        .or_else(|| nav.nearest(feet))
}

/// Where to look while walking a route: the point about 150 units along
/// it, at eye height. Close enough that the view leads the movement round
/// bends (so plain W follows the route, as when a player steers with the
/// mouse), far enough to be calm on straights and to look round corners as
/// they come.
fn look_ahead(nav: &NavGraph, path: &[u32], from: usize, feet: Vec3, eye: Vec3) -> Option<Vec3> {
    const AHEAD: f32 = u(150.0);
    let lift = eye.y - feet.y;
    let (mut walked, mut prev) = (0.0, feet);
    for &i in path.iter().skip(from).take(48) {
        let p = nav.nodes[i as usize].pos;
        let step = p.distance(prev);
        if walked + step >= AHEAD {
            let at = prev.lerp(p, (AHEAD - walked) / step.max(1e-3));
            return Some(at + Vec3::Y * lift);
        }
        walked += step;
        prev = p;
    }
    // The route ends sooner: past its end, the way it was going.
    let last = Vec3::new(prev.x - feet.x, 0.0, prev.z - feet.z);
    (walked > u(20.0)).then(|| eye + last.normalize_or_zero() * AHEAD)
}

/// Whether there's low cover just ahead of a player at `feet` facing
/// `target` (something to crouch behind) that a crouching player still
/// sees over.
/// How to wait at a spot: as real players out of fights did when standing
/// still (crouched 31% of the time, prone 11%), crouched behind low cover,
/// and never so low the watched point can't be seen.
fn hold_stance(spatial: &SpatialQuery, at: Vec3, watch: Option<Vec3>, patience: f32, rng: &mut impl Rng) -> Stance {
    let Some(watch) = watch else { return Stance::Stand };
    let sees = |eye: f32| line_clear(spatial, at + Vec3::Y * u(eye), watch);
    if behind_low_cover(spatial, at, watch) && rng.random::<f32>() < 0.6 + 0.3 * patience {
        return Stance::Crouch;
    }
    let r = rng.random::<f32>();
    if r < 0.08 + 0.08 * patience && sees(14.0) {
        Stance::Prone
    } else if r < 0.35 + 0.1 * patience && sees(42.0) {
        Stance::Crouch
    } else {
        Stance::Stand
    }
}

fn behind_low_cover(spatial: &SpatialQuery, feet: Vec3, target: Vec3) -> bool {
    let dir = Vec3::new(target.x - feet.x, 0.0, target.z - feet.z);
    let Ok(d) = Dir3::new(dir) else { return false };
    let low = spatial.cast_ray(feet + Vec3::Y * u(24.0), d, u(80.0), true, &collision::sight_filter()).is_some();
    low && line_clear(spatial, feet + Vec3::Y * u(42.0), target)
}

/// A crouch-sized hull fits along the straight line from `a` to `b`.
fn straight_walk(spatial: &SpatialQuery, a: Vec3, b: Vec3) -> bool {
    let base = a.y.max(b.y) + crate::movement::STEP_HEIGHT + u(1.0);
    // The full player hull and a little more: a narrower probe cuts corners
    // the player then snags on.
    let shape = Collider::capsule(crate::movement::HULL_RADIUS + u(0.5), u(4.0));
    let from = Vec3::new(a.x, base + u(18.0), a.z);
    let delta = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
    let Ok(dir) = Dir3::new(delta) else { return true };
    let config = ShapeCastConfig::from_max_distance(delta.length());
    spatial.cast_shape(&shape, from, Quat::IDENTITY, dir, &config, &collision::movement_filter()).is_none()
        && ground_between(spatial, a, b)
}

/// No holes along the way (sampled every 48 units).
fn ground_between(spatial: &SpatialQuery, a: Vec3, b: Vec3) -> bool {
    let steps = (a.distance(b) / u(48.0)).ceil() as i32;
    (1..steps).all(|k| {
        let p = a.lerp(b, k as f32 / steps as f32);
        spatial
            .cast_ray(p + Vec3::Y * u(24.0), Dir3::NEG_Y, u(64.0), true, &collision::movement_filter())
            .is_some()
    })
}

/// Running totals for [`behaviour_log`].
#[derive(Default)]
struct Behaviour {
    next_sample: f32,
    next_print: f32,
    /// Out of fights, per look source: samples, looking up, looking down,
    /// facing a wall.
    looks: std::collections::BTreeMap<&'static str, [u32; 4]>,
    /// Out of fights: samples, crouched, crouched somewhere exposed.
    crouch: [u32; 3],
    /// Travelling: seconds, move-key changes, seconds pushing against something.
    travel_time: f32,
    key_changes: u32,
    blocked_time: f32,
    /// Times a bot found itself stuck, per mode.
    stuck: std::collections::BTreeMap<String, u32>,
    last: HashMap<Entity, ((f32, f32), u8)>,
    /// Jumps by what asked for them, and whether each bot held jump.
    jumps: std::collections::BTreeMap<&'static str, u32>,
    jumping: HashMap<Entity, bool>,
    /// Mantles climbed (counted with the jumps, as "climbed").
    climbing: HashMap<Entity, bool>,
    /// Seconds per mode (and peeking): [moving, moving with sights up,
    /// still, still with sights up].
    ads: std::collections::BTreeMap<String, [f32; 4]>,
    /// In fight modes: seconds, the target in view, firing, before reacting,
    /// no target.
    fight: [f32; 5],
    /// Out of fights: each bot's current stop (since when, what it was
    /// doing as it stopped), and finished stops' lengths by that.
    stop: HashMap<Entity, (f32, String)>,
    stops: std::collections::BTreeMap<String, Vec<f32>>,
    /// Moving and not firing, per look source (fights by mode): seconds, of them sideways
    /// (60-120 degrees off the view), diagonal (30-60).
    sideways: std::collections::BTreeMap<&'static str, [f32; 5]>,
}

/// With `COD4RW_SIM` set: how natural the bots look. Out of fights: how
/// often they look up at the sky, down at the floor or into a wall (by what
/// they were looking at), crouch in the open; while travelling: how often
/// their move keys change, how long they push against something, and how
/// often they get stuck.
#[allow(clippy::type_complexity)]
fn behaviour_log(
    time: Res<Time>,
    spatial: SpatialQuery,
    nav: Option<Res<NavGraph>>,
    bots: Query<(Entity, &Bot, &Transform, &Mover, &ViewAngles, &MoveInput, &WeaponInput), Without<Dead>>,
    mut b: Local<Behaviour>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let b = &mut *b;
    let sample = now >= b.next_sample;
    if sample {
        b.next_sample = now + 0.25;
    }
    for (e, bot, tf, mover, view, mv, wi) in &bots {
        let fighting = matches!(bot.mode, Mode::Engage | Mode::Cover);
        let moving = Vec2::new(mover.velocity.x, mover.velocity.z).length() > u(40.0);
        let what = if bot.peek.is_some() { "Peek".to_string() } else { format!("{:?}{}", bot.mode, if bot.dest.is_some() { "+dest" } else { "" }) };
        let a = b.ads.entry(what).or_default();
        let k = if moving { 0 } else { 2 };
        a[k] += dt;
        a[k + 1] += if wi.ads { dt } else { 0.0 };
        if moving && !wi.fire {
            let heading = Vec3::new(mover.velocity.x, 0.0, mover.velocity.z).normalize_or_zero();
            let facing = Vec3::new(view.forward().x, 0.0, view.forward().z).normalize_or_zero();
            let off = heading.dot(facing).clamp(-1.0, 1.0).acos().to_degrees();
            let key = match bot.mode {
                Mode::Engage => "Engage",
                Mode::Cover => "Cover",
                _ => bot.look_src,
            };
            let s = b.sideways.entry(key).or_default();
            s[0] += dt;
            s[1] += if (60.0..120.0).contains(&off) { dt } else { 0.0 };
            s[2] += if (30.0..60.0).contains(&off) { dt } else { 0.0 };
            s[3] += if off >= 90.0 { dt } else { 0.0 };
            s[4] += if off >= 135.0 { dt } else { 0.0 };
        }
        if !fighting && !moving {
            b.stop.entry(e).or_insert_with(|| (now, format!("{:?}/{}", bot.mode, bot.look_src)));
        } else if let Some((since, what)) = b.stop.remove(&e) {
            if now - since > 0.15 {
                b.stops.entry(what).or_default().push(now - since);
            }
        }
        if fighting {
            let eng = bot.engagement.as_ref();
            let seen = eng.and_then(|g| bot.know.contacts.get(&g.target)).is_some_and(|c| c.visible);
            let f = &mut b.fight;
            f[0] += dt;
            f[1] += if seen { dt } else { 0.0 };
            f[2] += if wi.fire { dt } else { 0.0 };
            f[3] += if eng.is_some_and(|g| now < g.react_at) { dt } else { 0.0 };
            f[4] += if eng.is_none() { dt } else { 0.0 };
        }
        let keys = (mv.forward, mv.right);
        let (last_keys, last_stuck) = b.last.get(&e).copied().unwrap_or((keys, 0));
        if bot.stuck > 0 && last_stuck == 0 {
            *b.stuck.entry(format!("{:?}", bot.mode)).or_default() += 1;
        }
        b.last.insert(e, (keys, bot.stuck));
        let was = b.jumping.insert(e, mv.jump).unwrap_or(false);
        if mv.jump && !was {
            *b.jumps.entry(bot.jump_src).or_default() += 1;
        }
        let climbing = mover.mantle.is_some();
        if climbing && !b.climbing.insert(e, climbing).unwrap_or(false) {
            *b.jumps.entry("climbed").or_default() += 1;
        }
        b.climbing.insert(e, climbing);
        if !fighting && bot.dest.is_some() {
            b.travel_time += dt;
            b.key_changes += (keys != last_keys) as u32;
            let pushing = keys != (0.0, 0.0);
            let speed = Vec2::new(mover.velocity.x, mover.velocity.z).length();
            if pushing && speed < u(40.0) && mover.on_ground {
                b.blocked_time += dt;
            }
        }
        if !sample {
            continue;
        }
        let eye = mover.eye(tf.translation);
        let pitch = view.pitch.to_degrees();
        let wall = Dir3::new(view.forward())
            .ok()
            .and_then(|d| spatial.cast_ray(eye, d, u(100.0), true, &collision::sight_filter()))
            .is_some();
        let src = if fighting { if bot.look_src == "fight" { "fight" } else { "fight-other" } } else { bot.look_src };
        let l = b.looks.entry(src).or_default();
        l[0] += 1;
        l[1] += (pitch > 25.0) as u32;
        l[2] += (pitch < -35.0) as u32;
        l[3] += wall as u32;
        if fighting {
            continue;
        }
        let crouched = mover.stance != crate::movement::Stance::Stand;
        let exposed = nav
            .as_deref()
            .and_then(|n| n.nearest(tf.translation).map(|i| n.nodes[i as usize].exposure > 0.5))
            .unwrap_or(false);
        b.crouch[0] += 1;
        b.crouch[1] += crouched as u32;
        b.crouch[2] += (crouched && exposed) as u32;
    }
    if now < b.next_print {
        return;
    }
    b.next_print = now + 5.0;
    let pct = |a: u32, n: u32| 100.0 * a as f32 / n.max(1) as f32;
    let looks: Vec<String> = b
        .looks
        .iter()
        .map(|(k, v)| format!("{k} {} (up {:.0}% down {:.0}% wall {:.0}%)", v[0], pct(v[1], v[0]), pct(v[2], v[0]), pct(v[3], v[0])))
        .collect();
    info!("behaviour looks: {}", looks.join(", "));
    let ads: Vec<String> = b
        .ads
        .iter()
        .map(|(k, a)| format!("{k} moving {:.0}s ({:.0}% ADS) still {:.0}s ({:.0}% ADS)", a[0], 100.0 * a[1] / a[0].max(1e-3), a[2], 100.0 * a[3] / a[2].max(1e-3)))
        .collect();
    info!("behaviour ads: {}", ads.join(", "));
    {
        use std::sync::atomic::Ordering::Relaxed;
        let frames = THINK_FRAMES.swap(0, Relaxed).max(1);
        let paths = PATHS.swap(0, Relaxed);
        info!(
            "behaviour cost: think {:.2} ms/frame (worst {:.1}), routes {} ({:.2} ms each, worst {:.1})",
            THINK_US.swap(0, Relaxed) as f32 / frames as f32 / 1000.0,
            THINK_MAX_US.swap(0, Relaxed) as f32 / 1000.0,
            paths,
            PATH_US.load(Relaxed) as f32 / paths.max(1) as f32 / 1000.0,
            PATH_MAX_US.swap(0, Relaxed) as f32 / 1000.0
        );
        let per = |us: u64| us as f32 / frames as f32 / 1000.0;
        info!(
            "behaviour cost phases (ms/frame): shared {:.2}, perception {:.2}, decide {:.2}, act {:.2} (routes {:.2}, motion {:.2}, follow {:.2}); {} points expanded a route",
            per(PHASE_US[0].swap(0, Relaxed)),
            per(PHASE_US[1].swap(0, Relaxed)),
            per(PHASE_US[2].swap(0, Relaxed)),
            per(PHASE_US[3].swap(0, Relaxed)),
            per(PATH_US.swap(0, Relaxed)),
            per(PHASE_US[4].swap(0, Relaxed)),
            per(PHASE_US[5].swap(0, Relaxed)),
            nav::EXPANDED.swap(0, Relaxed) / paths.max(1)
        );
    }
    let mut stops: Vec<(usize, String)> = b
        .stops
        .iter()
        .map(|(k, v)| {
            let mut v = v.clone();
            v.sort_by(f32::total_cmp);
            (v.len(), format!("{k} {} stops, median {:.1}s, total {:.0}s", v.len(), v[v.len() / 2], v.iter().sum::<f32>()))
        })
        .collect();
    stops.sort_by(|a, b| b.0.cmp(&a.0));
    let side: Vec<String> = b
        .sideways
        .iter()
        .map(|(k, s)| {
            let pct = |v: f32| 100.0 * v / s[0].max(1e-3);
            format!("{k} {:.0}s ({:.0}% sideways, {:.0}% diagonal, {:.0}% over 90, {:.0}% backwards)", s[0], pct(s[1]), pct(s[2]), pct(s[3]), pct(s[4]))
        })
        .collect();
    info!("behaviour moving: {}", side.join(", "));
    info!("behaviour stops: {}", stops.iter().take(12).map(|s| s.1.as_str()).collect::<Vec<_>>().join("; "));
    let f = b.fight;
    let share = |x: f32| 100.0 * x / f[0].max(1e-3);
    info!(
        "behaviour fights: {:.0}s, target in view {:.0}%, firing {:.0}%, reacting {:.0}%, no target {:.0}%",
        f[0],
        share(f[1]),
        share(f[2]),
        share(f[3]),
        share(f[4])
    );
    info!(
        "behaviour: crouched {:.0}% of non-fight time ({:.0}% exposed); travelling {:.0}s: {:.1} key changes/s, blocked {:.0}%; stuck {:?}; jumps {:?}",
        pct(b.crouch[1], b.crouch[0]),
        pct(b.crouch[2], b.crouch[0]),
        b.travel_time,
        b.key_changes as f32 / b.travel_time.max(1.0),
        100.0 * b.blocked_time / b.travel_time.max(1.0),
        b.stuck,
        b.jumps
    );
}

/// With `COD4RW_SIM` set: log what every bot is doing every few seconds,
/// and how spread out teams are (distance to the nearest living teammate;
/// how often two teammates walk the same way within 300u of each other,
/// outside fights).
fn debug_log(
    time: Res<Time>,
    mut next: Local<f32>,
    mut spacing: Local<(f32, f32, u32, u32, u32, u32)>,
    bots: Query<(&Bot, &Pawn, &Health, &Transform, &Mover, Has<Dead>, &WeaponState)>,
) {
    let now = time.elapsed_secs();
    if now >= spacing.0 {
        spacing.0 = now + 0.5;
        let alive: Vec<(Team, Vec3, Vec3)> = bots
            .iter()
            .filter(|b| !b.5)
            .map(|(b, p, _, tf, m, _, _)| {
                let v = Vec3::new(m.velocity.x, 0.0, m.velocity.z);
                (p.team, tf.translation, if matches!(b.mode, Mode::Engage) { Vec3::ZERO } else { v })
            })
            .collect();
        for (i, a) in alive.iter().enumerate() {
            let mates = alive.iter().enumerate().filter(|(j, b)| *j != i && b.0 == a.0);
            let Some(near) = mates.clone().map(|(_, b)| b.1.distance(a.1)).min_by(f32::total_cmp) else { continue };
            spacing.1 += near / crate::units::INCH;
            spacing.2 += 1;
            spacing.3 += (near < u(250.0)) as u32;
            if a.2.length() > u(60.0) {
                spacing.5 += 1;
                let train = mates.clone().any(|(_, b)| {
                    b.1.distance(a.1) < u(300.0) && b.2.length() > u(60.0) && b.2.normalize().dot(a.2.normalize()) > 0.7
                });
                spacing.4 += train as u32;
            }
        }
    }
    if now < *next {
        return;
    }
    *next = now + 5.0;
    if spacing.2 > 0 {
        info!(
            "spacing: nearest mate {:.0}u avg, within 250u {:.0}%, moving in a line with a mate {:.0}%",
            spacing.1 / spacing.2 as f32,
            100.0 * spacing.3 as f32 / spacing.2 as f32,
            100.0 * spacing.4 as f32 / spacing.5.max(1) as f32,
        );
    }
    for (bot, pawn, health, tf, _, dead, weapon) in &bots {
        let seen = bot.know.contacts.values().filter(|c| c.noticed()).count();
        info!(
            "bot {:<10} {:<14} skill {:.2} {:?} {:?} {:<11} hp {:3.0} k/d {}/{} contacts {} seen {} dest {} stuck {}{}",
            pawn.name,
            weapon.def.name.trim_end_matches("_mp"),
            bot.skill,
            bot.role,
            pawn.team,
            if dead { "dead".to_string() } else { format!("{:?}", bot.mode) },
            health.current,
            pawn.kills,
            pawn.deaths,
            bot.know.contacts.len(),
            seen,
            bot.dest.map_or("-".to_string(), |d| format!("{:.0}u", d.distance(tf.translation) / crate::units::INCH)),
            bot.stuck,
            bot.goal.filter(|_| bot.mode == Mode::Objective).map_or(String::new(), |g| format!(" goal {g:?}")),
        );
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lobbies_are_mixed_and_sides_even() {
        let mut rng = rand::rng();
        for (_, setting) in TIERS {
            let mut gap = 0.0;
            let mut tiers = std::collections::HashSet::new();
            for _ in 0..200 {
                let [a, b] = deal_skills(setting, [8, 9], [setting, 0.0], &mut rng);
                assert_eq!((a.len(), b.len()), (8, 9));
                assert!(a.iter().chain(&b).all(|s| (0.0..=1.0).contains(s)));
                tiers.extend(a.iter().chain(&b).map(|&s| tier_name(s)));
                gap += (a.iter().sum::<f32>() + setting - b.iter().sum::<f32>()).abs() / 200.0;
            }
            assert!(tiers.len() >= 3, "{setting}: only {tiers:?}");
            assert!(gap < 0.25, "{setting}: sides {gap:.2} apart on average");
        }
    }

    #[test]
    fn better_bots_rank_higher() {
        let mut rng = rand::rng();
        let mean = |skill: f32, rng: &mut rand::rngs::ThreadRng| {
            let ranks: Vec<f32> = (0..400).map(|_| rank_for(skill, rng)).filter(|r| r.1 == 0).map(|r| r.0 as f32).collect();
            ranks.iter().sum::<f32>() / ranks.len() as f32
        };
        assert!(mean(0.25, &mut rng) + 10.0 < mean(0.7, &mut rng));
    }
}
