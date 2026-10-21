//! Why a bot does what it does. Real players have a reason behind every
//! move: get kills, stay alive, get the next streak, keep the one they're
//! on, pay back whoever just killed them, take the flag, get out of a spot
//! they've been seen in, help a teammate in a fight. Each bot weighs these
//! by its persona and its state (health, kills this life, deaths in a row,
//! being found) and the strongest one leans its choice of what to do
//! ([`bias`] on `decide`'s options).
//!
//! The bot's mood follows its match too ([`Temper`]): some players tilt
//! after dying twice running (they sprint and push more), some sit tighter
//! when on a streak. How much is their real player's ([`super::persona`]).
//!
//! `--motives` (or `COD4RW_MOTIVES=1`) shows every bot's style and motive
//! over its head.

use super::{Bot, Mode, Personality};
use crate::combat::Killed;
use bevy::prelude::*;
use rand::Rng;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Motive {
    /// Find someone to kill.
    #[default]
    Kill,
    /// Hurt or outnumbered: live.
    Survive,
    /// One kill off the next streak: push for it.
    EarnStreak,
    /// On a streak: don't throw it away.
    ProtectStreak,
    /// Go after whoever just killed us.
    Revenge,
    /// The mode's objective.
    Objective,
    /// Seen (shot from somewhere unseen) while holding: move.
    Reposition,
    /// A teammate nearby is fighting.
    Help,
}

impl Motive {
    pub fn name(self) -> &'static str {
        match self {
            Motive::Kill => "get kills",
            Motive::Survive => "survive",
            Motive::EarnStreak => "earn streak",
            Motive::ProtectStreak => "protect streak",
            Motive::Revenge => "revenge",
            Motive::Objective => "objective",
            Motive::Reposition => "reposition",
            Motive::Help => "help teammate",
        }
    }
}

/// Kills this match, as kill streaks count them (CoD4's 3, 5, 7).
const STREAKS: [u32; 3] = [3, 5, 7];
/// How long a bot goes after its killer before giving up.
const REVENGE_FOR: f32 = 40.0;
/// Being shot from out of sight while holding counts as being found for.
const SPOTTED_FOR: f32 = 6.0;

/// A bot's match so far and the mood it puts it in.
#[derive(Clone, Debug, Default)]
pub struct Temper {
    pub deaths_in_row: u32,
    pub kills_life: u32,
    /// Who killed us last, where they were then, and until when we'd go
    /// after them (when this persona does).
    pub revenge: Option<(Entity, Vec3, f32)>,
    /// When we were last shot from somewhere we couldn't see, holding still.
    pub spotted_at: f32,
    pub motive: Motive,
    pub motive_since: f32,
    /// Its last kill, and whether it reloads after it (rolled at the kill).
    pub last_kill: f32,
    pub reload_after_kill: bool,
}

pub fn overlay() -> bool {
    // (`--motives` on the command line does the same without making it a
    // debug run: sound stays on, stats are kept.)
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var("COD4RW_MOTIVES").is_ok_and(|v| v != "0") || std::env::args().any(|a| a == "--motives"));
    *ON
}

/// The bot's personality as its mood has it now: its persona's own, pushed
/// by tilt (deaths in a row) and streak protection (kills this life).
pub(super) fn mood(base: &Personality, bot: &Bot) -> Personality {
    let mut p = base.clone();
    let (Some(pp), t) = (bot.persona.as_ref(), &bot.temper) else { return p };
    if t.deaths_in_row >= 2 {
        let k = ((t.deaths_in_row - 1) as f32 / 2.0).min(1.0);
        p.sprinter = (p.sprinter + pp.tilt / 0.65 * k).clamp(0.03, 1.0);
        p.aggression = (p.aggression + 0.8 * pp.tilt * k).clamp(0.05, 0.95);
    }
    if t.kills_life >= 3 {
        let k = ((t.kills_life - 2) as f32 / 3.0).min(1.0);
        p.patience = (p.patience + 2.0 * pp.protect * k).clamp(0.05, 0.95);
        p.aggression = (p.aggression - 1.2 * pp.protect * k).clamp(0.05, 0.95);
    }
    if STREAKS.contains(&(t.kills_life + 1)) {
        p.aggression = (p.aggression + 0.08).min(0.95);
    }
    p
}

/// Kills and deaths: each bot's temper, and its mood with it.
pub(super) fn track(time: Res<Time>, mut killed: MessageReader<Killed>, mut bots: Query<(Entity, &mut Bot)>, places: Query<&Transform>) {
    let now = time.elapsed_secs();
    let mut rng = rand::rng();
    for k in killed.read() {
        if let Ok((_, mut bot)) = bots.get_mut(k.victim) {
            let bot = &mut *bot;
            bot.temper.deaths_in_row += 1;
            bot.temper.kills_life = 0;
            // Most players don't go looking for their killer (the next kill
            // is them about as often as chance); some do.
            let keen = bot.persona.as_ref().map_or(0.0, |pp| ((pp.revenge - 0.08) * 3.0).clamp(0.0, 0.8));
            // Where it was killed from: a spot to remember.
            if let Some(at) = k.attacker.filter(|a| *a != k.victim).and_then(|a| places.get(a).ok()) {
                let rate = super::knowledge::rate(bot.knowledge());
                bot.learned.note(at.translation, 0.6 * rate, now);
            }
            bot.temper.revenge = match (k.attacker.filter(|a| *a != k.victim), rng.random::<f32>() < keen) {
                (Some(a), true) => places.get(a).ok().map(|tf| (a, tf.translation, now + REVENGE_FOR)),
                _ => None,
            };
            bot.personality = mood(&bot.base, bot);
        }
        if let Some(a) = k.attacker.filter(|a| *a != k.victim) {
            if let Ok((_, mut bot)) = bots.get_mut(a) {
                let bot = &mut *bot;
                bot.temper.kills_life += 1;
                bot.temper.deaths_in_row = 0;
                bot.temper.last_kill = now;
                bot.temper.reload_after_kill = rng.random::<f32>() < 0.4;
                if bot.temper.revenge.is_some_and(|r| r.0 == k.victim) {
                    bot.temper.revenge = None;
                }
                bot.personality = mood(&bot.base, bot);
            }
        }
    }
}

/// What the bot is in it for right now, from its persona and state.
pub(super) struct Situation {
    pub hurt: bool,
    pub outnumbered: bool,
    pub objective: bool,
    pub mate_fighting: bool,
    pub now: f32,
}

pub(super) fn choose(bot: &Bot, s: &Situation, rng: &mut impl Rng) -> Motive {
    let p = &bot.personality;
    let t = &bot.temper;
    let pp = bot.persona.as_ref();
    let caution = 1.0 - p.aggression;
    let mut options = vec![(Motive::Kill, 0.5 + 0.5 * p.aggression)];
    if s.hurt || s.outnumbered {
        options.push((Motive::Survive, 0.4 + 0.6 * caution + if s.hurt { 0.3 } else { 0.0 }));
    }
    if STREAKS.contains(&(t.kills_life + 1)) {
        options.push((Motive::EarnStreak, 0.55 + 0.3 * p.aggression));
    }
    if t.kills_life >= 3 {
        options.push((Motive::ProtectStreak, 0.4 + pp.map_or(0.0, |pp| 2.0 * pp.protect.max(0.0)) + 0.3 * caution));
    }
    if t.revenge.is_some_and(|r| s.now < r.2) {
        options.push((Motive::Revenge, 1.1));
    }
    if s.objective {
        // Real players spend a median 37% of a Domination match near a flag.
        let keen = pp.map_or(0.37, |pp| pp.objective);
        options.push((Motive::Objective, 0.3 + 1.6 * keen));
    }
    if t.spotted_at > 0.0 && s.now - t.spotted_at < SPOTTED_FOR {
        // Knowing to move once found is experience.
        options.push((Motive::Reposition, 0.5 + 0.9 * pp.map_or(0.5, |pp| pp.knowledge())));
    }
    if s.mate_fighting {
        options.push((Motive::Help, 0.55 + 0.25 * p.aggression));
    }
    for o in &mut options {
        o.1 += rng.random_range(0.0..0.1);
    }
    // Keep a motive a little while rather than flipping every think.
    let keep = if s.now - t.motive_since < 2.0 { 0.25 } else { 0.1 };
    options.iter().map(|&(m, w)| (m, w + if m == t.motive { keep } else { 0.0 })).max_by(|a, b| a.1.total_cmp(&b.1)).map_or(Motive::Kill, |o| o.0)
}

/// How much `motive` leans towards doing `mode`.
pub(super) fn bias(motive: Motive, mode: Mode) -> f32 {
    use Mode::*;
    match (motive, mode) {
        (Motive::Kill, Hunt | Investigate | Flank | Engage) => 1.15,
        (Motive::Survive, Cover | Hold) => 1.4,
        (Motive::Survive, Engage | Investigate) => 0.75,
        (Motive::EarnStreak, Investigate | Flank | Hunt) => 1.3,
        (Motive::ProtectStreak, Post | Hold | Cover) => 1.4,
        (Motive::ProtectStreak, Investigate | Flank) => 0.7,
        // (Hunt is where the killer's spot comes in: see `decide`.)
        (Motive::Revenge, Hunt) => 3.0,
        (Motive::Revenge, Investigate | Flank) => 1.3,
        (Motive::Revenge, Post) => 0.5,
        (Motive::Objective, Objective) => 1.5,
        (Motive::Reposition, Post | Hold) => 0.25,
        (Motive::Reposition, Flank | Cover | Hunt) => 1.3,
        (Motive::Help, Investigate | Flank) => 1.35,
        _ => 1.0,
    }
}
