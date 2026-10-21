//! Text chat: the bus everyone's chat goes through ([`ChatMessage`], kept
//! in [`ChatLog`] for the HUD's chat box), and bots' banter on it, as a
//! public lobby has: now and then, not all the time. Most bots never say
//! anything; a few are chatty, and a tilted one more so. They react to
//! what happens to them (a kill, being killed, the same guy again, a spawn
//! kill, a streak, a lucky shot, the team being crushed, the match's end),
//! and now and then answer each other.
//!
//! Lines come from a small grammar (many templates, names, weapons, the map,
//! counts) typed the way people type in a lobby: lower case, dropped
//! punctuation, "lol", the odd typo. No line comes twice in a match. Mild
//! trash talk only.
//!
//! The lobby says at most a few things a minute (a token bucket), each a
//! typing delay after the moment. `COD4RW_CHAT=0` keeps bots quiet;
//! `COD4RW_CHATLOG=1` (sims) logs every line.

use super::Bot;
use crate::combat::{HitLocation, Killed, Pawn, Team};
use bevy::prelude::*;
use rand::Rng;
use std::collections::{HashMap, HashSet};

/// A line of chat from anyone (bot or player), to show in the chat box.
#[derive(Message, Clone, Debug)]
pub struct ChatMessage {
    pub sender: String,
    pub team: Team,
    pub text: String,
    /// Team chat (`say_team`) rather than everyone.
    pub team_only: bool,
}

/// One line as shown: when it was said (`Time::elapsed_secs`).
#[derive(Clone, Debug)]
pub struct ChatLine {
    pub time: f32,
    pub sender: String,
    pub team: Team,
    pub text: String,
    pub team_only: bool,
}

/// The match's chat so far, newest last (at most [`LOG_LINES`]).
#[derive(Resource, Default, Clone, Debug)]
pub struct ChatLog {
    pub lines: Vec<ChatLine>,
}

pub const LOG_LINES: usize = 50;

/// Lobby-wide: about this many lines a minute at most, a burst of a few.
const PER_MINUTE: f32 = 3.0;
const BURST: f32 = 3.0;
/// Never two lines closer than this (seconds), replies aside.
const MIN_GAP: f32 = 3.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Taunt,
    Complain,
    Again,
    SpawnKilled,
    Streak,
    Lucky,
    Losing,
    Win,
    Loss,
    Reply,
}

/// A line waiting to be "typed".
struct Pending {
    at: f32,
    sender: Entity,
    text: String,
    /// Who it was at (for a reply), and what it was.
    to: Option<Entity>,
    kind: Kind,
}

#[derive(Resource, Default)]
pub(super) struct Chatter {
    tokens: f32,
    refilled: f32,
    last: f32,
    used: HashSet<String>,
    pending: Vec<Pending>,
    /// Times each (victim, killer) happened this match.
    killed_by: HashMap<(Entity, Entity), u32>,
    /// When each pawn last spawned (first seen alive somewhere new).
    spawned: HashMap<Entity, (f32, Vec3)>,
    /// Said its streak line at this many kills.
    streak_said: HashMap<Entity, u32>,
    ended_said: bool,
    losing_said: f32,
}

pub fn enabled() -> bool {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var("COD4RW_CHAT").map_or(true, |v| v != "0"));
    *ON
}

pub(super) fn setup(app: &mut App) {
    app.add_message::<ChatMessage>()
        .init_resource::<ChatLog>()
        .init_resource::<Chatter>()
        .add_systems(Update, (banter, say_pending, keep_log).chain().run_if(crate::state::in_game))
        .add_systems(OnEnter(crate::state::GameState::InGame), |mut log: ResMut<ChatLog>, mut c: ResMut<Chatter>| {
            log.lines.clear();
            *c = Chatter::default();
        });
}

/// How chatty a bot is (0..1): most never type, a few do a lot. Fixed per
/// person (its persona), so the same names are the talkers all session.
pub(super) fn chattiness(bot: &Bot) -> f32 {
    let Some(pp) = &bot.persona else { return 0.0 };
    let h = pp.source.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619)) % 100;
    let base = match h {
        0..=49 => 0.0,
        50..=79 => 0.25,
        80..=94 => 0.55,
        _ => 0.9,
    };
    // Tilted players type more.
    let tilt = if bot.temper.deaths_in_row >= 2 { 1.0 + 0.4 * (bot.temper.deaths_in_row - 1).min(3) as f32 } else { 1.0 };
    (base * tilt).min(1.0)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn banter(
    time: Res<Time>,
    mut chatter: ResMut<Chatter>,
    mut killed: MessageReader<Killed>,
    pawns: Query<(Entity, &Pawn, &Transform, Option<&Bot>)>,
    state: Option<Res<crate::tdm::MatchState>>,
    map: Option<Res<crate::world::MapName>>,
) {
    if !enabled() {
        killed.clear();
        return;
    }
    let now = time.elapsed_secs();
    let c = &mut *chatter;
    let mut rng = rand::rng();
    // Spawns: a pawn far from where it was is new to this life.
    for (e, _, tf, _) in &pawns {
        let p = tf.translation;
        match c.spawned.get(&e) {
            Some(&(_, last)) if last.distance(p) < crate::units::u(300.0) => {
                c.spawned.get_mut(&e).unwrap().1 = p;
            }
            _ => {
                c.spawned.insert(e, (now, p));
            }
        }
    }
    let name = |e: Entity| pawns.get(e).map(|p| p.1.name.clone()).unwrap_or_default();
    let map_name = map.as_ref().map_or("this map".to_string(), |m| m.0.trim_start_matches("mp_").replace('_', " "));
    for k in killed.read() {
        let Some(killer) = k.attacker.filter(|a| *a != k.victim) else { continue };
        let (Ok(vic), Ok(kil)) = (pawns.get(k.victim), pawns.get(killer)) else { continue };
        if !crate::combat::hostile(vic.1, kil.1) {
            continue;
        }
        let times = {
            let n = c.killed_by.entry((k.victim, killer)).or_default();
            *n += 1;
            *n
        };
        let spawn_killed = c.spawned.get(&k.victim).is_some_and(|s| now - s.0 < 3.0);
        let far = kil.2.translation.distance(vic.2.translation) > crate::units::u(1800.0);
        let lucky = k.location == HitLocation::Head && far || k.weapon.to_ascii_lowercase().contains("knife");
        let slots = Slots { killer: name(killer), victim: name(k.victim), weapon: k.weapon.to_string(), map: map_name.clone(), n: 0 };
        // The one killed has the most to say.
        if let Some(bot) = vic.3 {
            let kind = if spawn_killed {
                Kind::SpawnKilled
            } else if times >= 2 && rng.random::<f32>() < 0.7 {
                Kind::Again
            } else if lucky {
                Kind::Lucky
            } else {
                Kind::Complain
            };
            let chance = chattiness(bot) * if kind == Kind::Complain { 0.4 } else { 0.75 };
            if rng.random::<f32>() < chance {
                queue(c, &mut rng, now, k.victim, Some(killer), kind, &slots);
            }
        }
        if let Some(bot) = kil.3 {
            let kills = bot.temper.kills_life;
            let said = c.streak_said.get(&killer).copied().unwrap_or(0);
            if kills >= 5 && kills > said && kills % 5 == 0 && rng.random::<f32>() < chattiness(bot) * 0.6 {
                c.streak_said.insert(killer, kills);
                queue(c, &mut rng, now, killer, None, Kind::Streak, &Slots { n: kills, ..slots.clone() });
            } else if rng.random::<f32>() < chattiness(bot) * 0.3 {
                queue(c, &mut rng, now, killer, Some(k.victim), Kind::Taunt, &slots);
            }
        }
    }
    // A team being crushed, mid-match: someone on it says so (rarely).
    if let Some(st) = state.as_ref().filter(|s| s.ended.is_none()) {
        let gap = st.allies as i32 - st.axis as i32;
        if gap.abs() >= 15 && now - c.losing_said > 90.0 {
            let losing = if gap > 0 { Team::Axis } else { Team::Allies };
            let talker = pawns.iter().filter(|p| p.1.team == losing).filter_map(|p| p.3.map(|b| (p.0, chattiness(b)))).max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((e, ch)) = talker.filter(|t| t.1 > 0.0) {
                c.losing_said = now;
                if rng.random::<f32>() < ch {
                    queue(c, &mut rng, now, e, None, Kind::Losing, &Slots { map: map_name.clone(), ..Slots::default() });
                }
            }
        }
    }
    // The end: a few "gg"s.
    if let Some((winner, _)) = state.as_ref().and_then(|s| s.ended).filter(|_| !c.ended_said) {
        c.ended_said = true;
        let mut talkers: Vec<(Entity, Team, f32)> = pawns.iter().filter_map(|p| p.3.map(|b| (p.0, p.1.team, chattiness(b) + 0.15))).collect();
        rand::seq::SliceRandom::shuffle(talkers.as_mut_slice(), &mut rng);
        let mut said = 0;
        for (e, team, ch) in talkers {
            if said < 3 && rng.random::<f32>() < ch {
                let kind = if winner == Some(team) { Kind::Win } else { Kind::Loss };
                queue(c, &mut rng, now, e, None, kind, &Slots { map: map_name.clone(), ..Slots::default() });
                said += 1;
            }
        }
    }
}

/// Typing delays, the lobby's rate, then onto the bus (and now and then an
/// answer from the other side).
fn say_pending(time: Res<Time>, mut chatter: ResMut<Chatter>, pawns: Query<(Entity, &Pawn, Option<&Bot>)>, mut out: MessageWriter<ChatMessage>) {
    let now = time.elapsed_secs();
    let c = &mut *chatter;
    // Refill.
    c.tokens = (c.tokens + (now - c.refilled).max(0.0) * PER_MINUTE / 60.0).min(BURST);
    c.refilled = now;
    let mut rng = rand::rng();
    let due: Vec<usize> = c.pending.iter().enumerate().filter(|(_, p)| now >= p.at).map(|(i, _)| i).collect();
    for i in due.into_iter().rev() {
        let p = c.pending.remove(i);
        let reply = p.kind == Kind::Reply;
        if (!reply && now - c.last < MIN_GAP) || c.tokens < 1.0 || now - p.at > 8.0 {
            continue;
        }
        let Ok((_, pawn, _)) = pawns.get(p.sender) else { continue };
        c.tokens -= 1.0;
        c.last = now;
        out.write(ChatMessage { sender: pawn.name.clone(), team: pawn.team, text: p.text.clone(), team_only: false });
        // An answer, now and then: from whoever it was at, or someone on
        // their side, if they talk at all.
        if !reply && matches!(p.kind, Kind::Taunt | Kind::Complain | Kind::Again | Kind::Streak) && rng.random::<f32>() < 0.3 {
            let side = pawn.team;
            let answerer = p
                .to
                .and_then(|t| pawns.get(t).ok())
                .filter(|o| o.2.is_some_and(|b| chattiness(b) > 0.0))
                .map(|o| o.0)
                .or_else(|| {
                    pawns.iter().filter(|o| crate::combat::hostile(o.1, pawn) && o.1.team != side && o.2.is_some_and(|b| chattiness(b) > 0.2)).map(|o| o.0).next()
                });
            if let Some(a) = answerer {
                let slots = Slots { killer: pawn.name.clone(), victim: pawn.name.clone(), ..Slots::default() };
                queue(c, &mut rng, now, a, Some(p.sender), Kind::Reply, &slots);
            }
        }
    }
}

fn keep_log(time: Res<Time>, mut said: MessageReader<ChatMessage>, mut log: ResMut<ChatLog>) {
    for m in said.read() {
        if std::env::var_os("COD4RW_CHATLOG").is_some() {
            info!("chat: {}: {}", m.sender, m.text);
        }
        log.lines.push(ChatLine { time: time.elapsed_secs(), sender: m.sender.clone(), team: m.team, text: m.text.clone(), team_only: m.team_only });
    }
    let n = log.lines.len();
    if n > LOG_LINES {
        log.lines.drain(..n - LOG_LINES);
    }
}

// ------------------------------------------------------------------ lines

#[derive(Clone, Default)]
struct Slots {
    killer: String,
    victim: String,
    weapon: String,
    map: String,
    n: u32,
}

fn queue(c: &mut Chatter, rng: &mut impl Rng, now: f32, sender: Entity, to: Option<Entity>, kind: Kind, s: &Slots) {
    // Not two lines queued from one bot at once.
    if c.pending.iter().any(|p| p.sender == sender) {
        return;
    }
    for _ in 0..25 {
        let text = line(kind, s, rng);
        if text.is_empty() || c.used.contains(&text) {
            continue;
        }
        c.used.insert(text.clone());
        let typing = rng.random_range(1.0..3.5) + text.len() as f32 * rng.random_range(0.03..0.08);
        c.pending.push(Pending { at: now + typing, sender, text, to, kind });
        return;
    }
}

/// A name the way people type someone else's: the clan tag dropped, often
/// lower case, sometimes cut short.
fn short(name: &str, rng: &mut impl Rng) -> String {
    let mut n = name.to_string();
    for (open, close) in [('[', ']'), ('{', '}')] {
        if let (Some(a), Some(b)) = (n.find(open), n.find(close)) {
            if a < b {
                n = format!("{}{}", &n[..a], &n[b + 1..]);
            }
        }
    }
    if let Some((_, rest)) = n.split_once('|') {
        n = rest.to_string();
    }
    let n = n.trim().to_string();
    let n = if rng.random::<f32>() < 0.4 { n.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_').to_string() } else { n };
    let n = if rng.random::<f32>() < 0.7 { n.to_lowercase() } else { n };
    if n.is_empty() { name.to_string() } else { n }
}

fn pick<'a>(rng: &mut impl Rng, list: &[&'a str]) -> &'a str {
    list[rng.random_range(0..list.len())]
}

fn line(kind: Kind, s: &Slots, rng: &mut impl Rng) -> String {
    let k = short(&s.killer, rng);
    let v = short(&s.victim, rng);
    let w = s.weapon.to_lowercase();
    let body: &[&str] = match kind {
        Kind::Taunt => &[
            "ez", "sit down {v}", "{v} ur so bad", "nice try {v}", "too easy", "{v} you actually need to delete the game", "{v} stop running at me",
            "thanks for the free kill", "{v} uninstall", "bye {v}", "that was free", "{v} what was that", "get good {v}", "{v} is my favourite target",
            "{v} keeps feeding", "who taught u to play {v}", "{v} try again", "too slow {v}", "{v} pls", "night night {v}", "{v} u ok", "outplayed",
            "{v} really ran straight at me", "{v} nice aim bro", "free", "{v} u want my settings", "{v} stick to campaign",
        ],
        Kind::Complain => &[
            "lag", "wtf", "how", "how did that hit", "nice camping {k}", "{k} sitting in that corner all game", "bruh", "lucky", "this hitreg tho", "i shot first",
            "unreal", "ok {k}", "omg", "how am i dead", "{k} is so lucky", "cmon", "every time", "that was not a hit", "i was behind the wall", "my bullets r fake",
            "nice {w} {k}", "{k} with the {w} again", "sure", "this game man", "i hit him like 5 times", "really", "{k} camper", "great",
        ],
        Kind::Again => &[
            "{k} again??", "ok {k} chill", "{k} stop", "{k} is everywhere", "{k} again", "{k} relax man", "not {k} again", "{k} pls leave me alone",
            "{k} has a thing for me", "who is {k}", "ok {k} i see u", "{k} every single time", "{k} u got no life", "{k} following me around",
        ],
        Kind::SpawnKilled => &[
            "spawns", "spawn killed again", "these spawns are trash", "cant even spawn", "nice spawns {map}", "spawn trap", "i just spawned", "{k} spawn camping",
            "spawned right in front of him", "this spawn system", "0 seconds alive", "great spawn", "{map} spawns never change",
        ],
        Kind::Streak => &[
            "{n} kills", "{n} streak", "cant stop me", "on fire", "{n} and counting", "who wants some", "{n} no deaths", "someone stop me", "{n} lets go",
            "this is too easy", "{n} already", "cmon i need more", "keep coming",
        ],
        Kind::Lucky => &[
            "lucky", "knife lol", "nice knife {k}", "what was that shot", "lucky shot {k}", "{k} u hacking?", "across the map??", "no way", "ok that was clean",
            "that was luck", "{k} nice shot tbh", "from there??",
        ],
        Kind::Losing => &[
            "this team", "anyone gonna help", "we are getting destroyed", "wake up team", "where is everyone", "can someone play the objective", "this is rough",
            "they got all the good players", "we need a rematch", "can we switch teams", "team where r u",
        ],
        Kind::Win => &["gg", "gg ez", "ggs", "gg wp", "too easy", "gg all", "gg guys", "ggwp", "gg nice", "gg next", "easy gg"],
        Kind::Loss => &["gg", "gg i guess", "this team man", "gg lag", "ggs", "rematch", "gg", "gg wp", "gg close", "next one", "gg tho"],
        Kind::Reply => &[
            "cry", "ok", "cope", "{k} relax", "keep crying", "mad?", "who asked", "{k} its a game", "nah ur just bad", "skill issue", "sure buddy", "keep talking",
            "and?", "{k} chill", "lol ok", "u done?", "imagine", "ok and", "say that again", "ratio", "u good {k}",
        ],
    };
    let mut text = pick(rng, body).replace("{k}", &k).replace("{v}", &v).replace("{w}", &w).replace("{map}", &s.map).replace("{n}", &s.n.to_string());
    // A lead-in or a tail, now and then.
    if rng.random::<f32>() < 0.25 {
        text = format!("{}{}", pick(rng, &["lol ", "bruh ", "man ", "yo ", "omg ", "bro "]), text);
    }
    if rng.random::<f32>() < 0.4 {
        text.push_str(pick(rng, &[" lol", " lmao", " xd", " haha", " smh", "!!", "??", " :)", " lmfao", " tbh", " fr"]));
    }
    casual(&text, rng)
}

/// As people type in a lobby: mostly lower case, a stretched word, the odd
/// typo.
fn casual(text: &str, rng: &mut impl Rng) -> String {
    let mut t = if rng.random::<f32>() < 0.85 { text.to_lowercase() } else { text.to_string() };
    if rng.random::<f32>() < 0.15 {
        for (a, b) in [("so ", "sooo "), ("bad", "baaad"), ("ez", "ezzz"), ("no way", "no wayyy"), ("lol", "lool"), ("gg", "ggg")] {
            if t.contains(a) {
                t = t.replacen(a, b, 1);
                break;
            }
        }
    }
    if rng.random::<f32>() < 0.08 && t.len() > 4 {
        let chars: Vec<char> = t.chars().collect();
        let i = rng.random_range(1..chars.len() - 1);
        let mut c = chars.clone();
        match rng.random_range(0..3) {
            0 => c.swap(i, i + 1),
            1 => {
                c.remove(i);
            }
            _ => c.insert(i, chars[i]),
        }
        t = c.into_iter().collect();
    }
    t.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_vary_and_stay_clean() {
        let mut rng = rand::rng();
        let s = Slots { killer: "[KTZ]Tiger943".into(), victim: "Unknown Soldier".into(), weapon: "AK47".into(), map: "crossfire".into(), n: 5 };
        let mut seen = HashSet::new();
        for kind in [Kind::Taunt, Kind::Complain, Kind::Again, Kind::SpawnKilled, Kind::Streak, Kind::Lucky, Kind::Losing, Kind::Win, Kind::Loss, Kind::Reply] {
            for _ in 0..40 {
                let l = line(kind, &s, &mut rng);
                assert!(!l.is_empty() && l.len() < 90 && !l.contains('{'), "{l}");
                seen.insert(l);
            }
        }
        assert!(seen.len() > 250, "only {} different lines", seen.len());
    }

    #[test]
    fn names_lose_clan_tags() {
        let mut rng = rand::rng();
        for _ in 0..20 {
            let n = short("[KTZ]Tiger943", &mut rng);
            assert!(!n.contains('[') && n.to_lowercase().starts_with("tiger"), "{n}");
        }
    }
}
