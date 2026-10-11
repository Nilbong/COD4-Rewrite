//! CoD4's game types beside Team Deathmatch ([`crate::tdm`] runs the match
//! flow for all of them): Free-for-all, Domination ([`dom`]), Search and
//! Destroy ([`sd`]), Headquarters ([`koth`]) and Sabotage ([`sab`]).
//! [`Objectives`] is what's at stake right now, for the HUD, spawning and
//! the bots.

pub mod dom;
pub mod koth;
pub mod sab;
pub mod sd;

use crate::combat::Team;
use crate::units::u;
use bevy::prelude::*;
use std::sync::atomic::{AtomicU8, Ordering};

pub struct ModesPlugin;

impl Plugin for ModesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Objectives>().add_plugins((dom::DomPlugin, sd::SdPlugin, koth::KothPlugin, sab::SabPlugin));
    }
}

/// The game type of the match under way (CoD4's `g_gametype`).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GameMode {
    /// Team Deathmatch (`war`).
    #[default]
    Tdm,
    /// Free-for-all (`dm`): no teams, everyone else is an enemy.
    Ffa,
    /// Domination (`dom`): hold the map's flags.
    Dom,
    /// Search and Destroy (`sd`): rounds of one life, a bomb to plant.
    Sd,
    /// Headquarters (`koth`): take and hold the HQ where it comes up.
    Koth,
    /// Sabotage (`sab`): one bomb, each team's target.
    Sab,
    /// 3rd Person Team Deathmatch (`war3p`): Team Deathmatch seen over the
    /// shoulder, with cover ([`crate::cover`]). Last, so the others keep
    /// their indices.
    Tdm3,
}

impl GameMode {
    /// The game types the lobby offers, in its order.
    pub const ALL: [GameMode; 7] = [GameMode::Tdm, GameMode::Ffa, GameMode::Dom, GameMode::Sd, GameMode::Koth, GameMode::Sab, GameMode::Tdm3];

    /// Whether the lobby offers this mode: work-in-progress ones (3rd Person
    /// TDM) only in test builds ([`test_features`]).
    pub fn offered(self) -> bool {
        self != GameMode::Tdm3 || test_features()
    }

    pub fn name(self) -> &'static str {
        match self {
            GameMode::Tdm => "Team Deathmatch",
            GameMode::Ffa => "Free-for-all",
            GameMode::Dom => "Domination",
            GameMode::Sd => "Search and Destroy",
            GameMode::Koth => "Headquarters",
            GameMode::Sab => "Sabotage",
            GameMode::Tdm3 => "3rd Person TDM",
        }
    }

    /// CoD4's name for it (`g_gametype`, `ui_gametype`).
    pub fn gametype(self) -> &'static str {
        match self {
            GameMode::Tdm => "war",
            GameMode::Ffa => "dm",
            GameMode::Dom => "dom",
            GameMode::Sd => "sd",
            GameMode::Koth => "koth",
            GameMode::Sab => "sab",
            GameMode::Tdm3 => "war3p",
        }
    }

    /// Played in two teams.
    pub fn teams(self) -> bool {
        self != GameMode::Ffa
    }

    /// The score limits the lobby offers, and the default's index: kills (a
    /// team's, or in free-for-all a player's), or Domination's points.
    pub fn score_limits(self) -> (&'static [u32], usize) {
        match self {
            GameMode::Tdm | GameMode::Tdm3 => (&[25, 50, 75, 100, 150, 250], 2),
            GameMode::Ffa => (&[10, 20, 30, 40, 50, 75], 2),
            GameMode::Dom => (&[100, 150, 200, 250, 300, 400], 2),
            // Rounds won.
            GameMode::Sd => (&[2, 3, 4, 5, 6, 7], 2),
            GameMode::Koth => (&[100, 150, 200, 250, 300, 400], 3),
            // Targets destroyed (rounds won).
            GameMode::Sab => (&[1, 2, 3, 4, 5, 6], 0),
        }
    }

    pub fn default_score_limit(self) -> u32 {
        let (limits, i) = self.score_limits();
        limits[i]
    }

    /// Minutes, by CoD4's defaults (`scr_<gametype>_timelimit`); Search and
    /// Destroy's rounds keep their own time ([`sd`]).
    pub fn default_time_limit(self) -> u32 {
        match self {
            GameMode::Dom | GameMode::Koth => 30,
            _ => 10,
        }
    }

    /// Does the match itself run out of time? (Search and Destroy's and
    /// Sabotage's rounds do instead: Sabotage's going to overtime.)
    pub fn timed(self) -> bool {
        !matches!(self, GameMode::Sd | GameMode::Sab)
    }

    /// HUD points per point of score: kills are worth 10, Domination's
    /// points and Search and Destroy's rounds one each.
    pub fn point_scale(self) -> u32 {
        match self {
            GameMode::Dom | GameMode::Sd | GameMode::Koth | GameMode::Sab => 1,
            _ => 10,
        }
    }

    /// The announcer's name for it as a match starts (`<voice>_1mc_<line>`).
    pub fn intro_line(self) -> &'static str {
        match self {
            GameMode::Tdm | GameMode::Tdm3 => "team_deathmtch",
            GameMode::Ffa => "freeforall",
            GameMode::Dom => "domination",
            GameMode::Sd => "searchdestroy",
            GameMode::Koth => "headquarters",
            GameMode::Sab => "sabotage",
        }
    }
}

/// Played over the shoulder, with cover: the local players' view is third
/// person and [`crate::cover`] is live.
pub fn third_person() -> bool {
    current() == GameMode::Tdm3
}

/// The game type under way (set as a match starts, [`crate::tdm`]), for
/// code that has no access to the resource ([`crate::combat::hostile`]).
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> GameMode {
    let i = CURRENT.load(Ordering::Relaxed) as usize;
    GameMode::ALL.get(i).copied().unwrap_or_default()
}

pub(crate) fn set_current(mode: GameMode) {
    CURRENT.store(mode as u8, Ordering::Relaxed);
    for l in &RESPAWN_LOCKED {
        l.store(false, Ordering::Relaxed);
    }
}

/// Sides whose dead can't come back yet: Search and Destroy's round past
/// its grace period, the side holding Headquarters' HQ. [allies, axis].
static RESPAWN_LOCKED: [std::sync::atomic::AtomicBool; 2] =
    [std::sync::atomic::AtomicBool::new(false), std::sync::atomic::AtomicBool::new(false)];

/// May `team`'s dead respawn now? Things that bring someone back early (a
/// skipped killcam, a class picked) ask first.
pub fn respawn_locked(team: crate::combat::Team) -> bool {
    RESPAWN_LOCKED[(team == crate::combat::Team::Axis) as usize].load(Ordering::Relaxed)
}

pub(crate) fn lock_respawns(team: crate::combat::Team, locked: bool) {
    RESPAWN_LOCKED[(team == crate::combat::Team::Axis) as usize].store(locked, Ordering::Relaxed);
}

/// What's at stake right now: Domination's flags, Search and Destroy's
/// bomb sites and bomb. Empty in game types without objectives.
#[derive(Resource, Default, Clone, Debug)]
pub struct Objectives {
    pub flags: Vec<Flag>,
    pub sites: Vec<BombSite>,
    pub bomb: Option<Bomb>,
    /// Search and Destroy: the team planting this round.
    pub attackers: Option<Team>,
    /// When the round (or, once planted, the bomb) runs out; and whether
    /// it's the bomb's timer.
    pub timer: Option<(f32, bool)>,
    /// Pawns planting or defusing: how far along (0..1), and whether
    /// it's a defusal.
    pub using: Vec<(Entity, f32, bool)>,
    /// The round just ended: who won it and why (a string key and its
    /// fallback text).
    pub round_over: Option<(Team, &'static str, &'static str)>,
    /// Headquarters: the HQ up now.
    pub hq: Option<Hq>,
    /// Sabotage's overtime: no respawns, the first target or side down
    /// wins; 90 seconds without, a tie.
    pub sudden_death: bool,
}

/// A Headquarters HQ (`hq_hardpoint`, taken in its `radiotrigger`).
#[derive(Clone, Debug)]
pub struct Hq {
    /// The radio (laptop) itself.
    pub pos: Vec3,
    /// Its trigger's box (Bevy space).
    pub min: Vec3,
    pub max: Vec3,
    pub owner: Option<Team>,
    /// Taking it (from nobody) or destroying it (from its holders): who,
    /// and how far (0..1).
    pub capture: Option<(Team, f32)>,
    /// Both teams are in it: the capture holds.
    pub contested: bool,
    /// Held: when it goes offline by itself.
    pub expires_at: Option<f32>,
}

impl Hq {
    pub fn contains(&self, p: Vec3) -> bool {
        let pad = Vec3::new(u(8.0), u(40.0), u(8.0));
        (p.cmpge(self.min - pad) & p.cmple(self.max + pad)).all()
    }
}

/// A brush entity's box (`model` "*N", from the clip map's models), else a
/// cube `fallback` across about its origin.
pub(crate) fn brush_box(e: &iw3::ents::Entity, clip: Option<&iw3::zone::ClipMap>, fallback: f32) -> Option<(Vec3, Vec3)> {
    let origin = crate::units::pos(e.origin()?);
    let cmodel = e.get("model").and_then(|m| m.strip_prefix('*')).and_then(|i| i.parse::<usize>().ok()).and_then(|i| clip?.cmodels.get(i));
    Some(match cmodel {
        Some(c) => {
            let (a, b) = (crate::units::pos(c.mins), crate::units::pos(c.maxs));
            let (min, max) = (a.min(b), a.max(b));
            // Relative to the entity when it doesn't contain it.
            if (origin.cmpge(min) & origin.cmple(max)).all() { (min, max) } else { (min + origin, max + origin) }
        }
        None => (origin - Vec3::splat(u(fallback * 0.5)), origin + Vec3::splat(u(fallback * 0.5))),
    })
}

/// A Search and Destroy bomb site (`bombzone`, a brush trigger) and its
/// target.
#[derive(Clone, Debug)]
pub struct BombSite {
    /// 'A' or 'B'.
    pub label: char,
    /// The trigger's box (Bevy space).
    pub min: Vec3,
    pub max: Vec3,
    /// The target's model, where the bomb goes.
    pub pos: Vec3,
    pub destroyed: bool,
    /// Sabotage: the team whose target it is (Search and Destroy's belong
    /// to whoever defends this round).
    pub team: Option<Team>,
}

impl BombSite {
    /// Is `p` (feet) inside the trigger, give or take a little?
    pub fn contains(&self, p: Vec3) -> bool {
        let pad = Vec3::new(u(8.0), u(40.0), u(8.0));
        (p.cmpge(self.min - pad) & p.cmple(self.max + pad)).all()
    }

    pub fn letter(&self) -> char {
        self.label.to_ascii_lowercase()
    }
}

/// The bomb: where it is, who has it, and once planted at which site and
/// when it goes off.
#[derive(Clone, Copy, Debug)]
pub struct Bomb {
    pub pos: Vec3,
    pub carrier: Option<Entity>,
    pub planted: Option<char>,
    pub explodes_at: Option<f32>,
}

/// Holding "use" (F, or the pad's X): plants or defuses the bomb in
/// Search and Destroy. The local player's comes from the keys; bots set
/// their own.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct UseObjective(pub bool);

/// A Domination flag (`flag_primary`, a `trigger_radius`).
#[derive(Clone, Debug)]
pub struct Flag {
    /// 'A', 'B', 'C' (`script_label` `_a`, ...).
    pub label: char,
    /// The bottom of its trigger, at the flag.
    pub pos: Vec3,
    /// The trigger's size: a cylinder `radius` across and `height` up from
    /// `pos` (Bevy units).
    pub radius: f32,
    pub height: f32,
    pub owner: Option<Team>,
    /// A capture under way: the team taking it, and how far (0..1).
    pub capture: Option<(Team, f32)>,
    /// Both teams are on it: the capture holds.
    pub contested: bool,
}

impl Flag {
    /// Is `p` inside its trigger?
    pub fn contains(&self, p: Vec3) -> bool {
        let d = p - self.pos;
        Vec2::new(d.x, d.z).length() <= self.radius && (-u(16.0)..=self.height).contains(&d.y)
    }

    /// Its letter for icons and the announcer (`a`).
    pub fn letter(&self) -> char {
        self.label.to_ascii_lowercase()
    }
}

impl Objectives {
    /// What draws `team`'s spawns (weight > 0) or keeps them away (< 0):
    /// its own flags and the enemy's.
    pub fn spawn_anchors(&self, team: Team) -> Vec<(Vec3, f32)> {
        self.flags
            .iter()
            .filter_map(|f| match f.owner {
                Some(o) if o == team => Some((f.pos, 1.0)),
                Some(_) => Some((f.pos, -1.0)),
                None => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_cod4() {
        assert_eq!(GameMode::Ffa.default_score_limit(), 30);
        assert_eq!(GameMode::Tdm.default_score_limit(), crate::tdm::SCORE_LIMIT);
        assert_eq!(GameMode::Dom.default_score_limit(), 200);
        assert_eq!(GameMode::Dom.default_time_limit(), 30);
        assert!(!GameMode::Ffa.teams() && GameMode::Tdm.teams() && GameMode::Dom.teams());
        assert_eq!(GameMode::Ffa.gametype(), "dm");
    }

    #[test]
    fn current_mode_round_trips() {
        for mode in GameMode::ALL {
            set_current(mode);
            assert_eq!(current(), mode);
        }
        set_current(GameMode::Tdm);
    }

    #[test]
    fn flags_trigger_as_cylinders() {
        let f = Flag { label: 'A', pos: Vec3::ZERO, radius: u(160.0), height: u(128.0), owner: None, capture: None, contested: false };
        assert!(f.contains(Vec3::new(u(100.0), u(40.0), 0.0)));
        assert!(!f.contains(Vec3::new(u(170.0), 0.0, 0.0)));
        assert!(!f.contains(Vec3::new(0.0, u(200.0), 0.0)));
        assert_eq!(f.letter(), 'a');
    }
}

/// Work-in-progress features (3rd Person TDM, the campaign) are only offered
/// with `COD4RW_TESTFEATURES=1` ("Play Test Build.bat").
pub fn test_features() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("COD4RW_TESTFEATURES").is_ok_and(|v| v == "1"))
}
