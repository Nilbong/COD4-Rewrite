//! The match beyond the pawns, host to guests: the clock and scores,
//! objectives (Domination's flags, the bomb and its sites, Headquarters),
//! who can't respawn, UAVs, rounds and their ends, airstrikes, XP awards,
//! kill streaks, and a guest's own hurts.
//!
//! What's ongoing rides in every snapshot (its `extra`, [`encode`]), so a
//! lost one costs nothing and a friend joining mid-match has it all at
//! once; times go as seconds from the snapshot. One-off news goes as
//! reliable [`GameMsg`]s. A guest's own game logic for all of this stands
//! down ([`super::authority`]) and shows the host's.

use super::wire::{GameMsg, R, W};
use super::*;
use crate::killstreaks::{Hardpoint, Killstreak, StreakNotice, uav};
use crate::modes::{Bomb, Hq, Objectives};
use crate::ui::progression::{Award, AwardKind};

/// The longest round-over strings kept.
const MAX_TEXT: usize = 48;

fn team_byte(t: Option<Team>) -> u8 {
    t.map_or(2, side_of)
}

fn team_from(b: u8) -> Option<Team> {
    (b < 2).then(|| team_of(b))
}

fn award_index(k: AwardKind) -> u8 {
    match k {
        AwardKind::Capture => 0,
        AwardKind::Defend => 1,
        AwardKind::Assault => 2,
        AwardKind::Plant => 3,
        AwardKind::Defuse => 4,
        AwardKind::Hardpoint => 5,
    }
}

fn award_from(i: u8) -> Option<AwardKind> {
    Some(match i {
        0 => AwardKind::Capture,
        1 => AwardKind::Defend,
        2 => AwardKind::Assault,
        3 => AwardKind::Plant,
        4 => AwardKind::Defuse,
        5 => AwardKind::Hardpoint,
        _ => return None,
    })
}

pub(crate) fn hardpoint_index(h: Hardpoint) -> u8 {
    Hardpoint::ALL.iter().position(|x| *x == h).unwrap_or(0) as u8
}

pub(crate) fn hardpoint_from(i: u8) -> Option<Hardpoint> {
    Hardpoint::ALL.get(i as usize).copied()
}

fn held_bits(k: &Killstreak) -> u8 {
    Hardpoint::ALL.iter().enumerate().filter(|(_, h)| k.held.has(**h)).fold(0, |b, (i, _)| b | 1 << i)
}

// ---- Host ----

/// The ongoing state, for this frame's snapshots ([`super::send_world`]).
#[allow(clippy::too_many_arguments)]
pub(super) fn encode(
    time: &Time,
    state: Option<&crate::tdm::MatchState>,
    o: &Objectives,
    radar: &uav::Radar,
    pings: &[(u16, u16)],
    id: &dyn Fn(Entity) -> u16,
) -> Vec<u8> {
    let now = time.elapsed_secs();
    let rel = |t: f32| (t - now).clamp(-1000.0, 10_000.0);
    let mut w = W::new();
    // The clock and scores.
    match state {
        Some(s) => {
            w.f32(if s.time_limit.is_finite() { s.time_left(now).max(0.0) } else { -1.0 });
            w.u16(s.allies.min(u16::MAX as u32) as u16).u16(s.axis.min(u16::MAX as u32) as u16);
        }
        None => {
            w.f32(-1.0).u16(0).u16(0);
        }
    }
    let locked = [Team::Allies, Team::Axis].map(crate::modes::respawn_locked);
    w.u8(o.sudden_death as u8 | (locked[0] as u8) << 1 | (locked[1] as u8) << 2);
    // Flags.
    w.u8(o.flags.len().min(8) as u8);
    for f in o.flags.iter().take(8) {
        let (t, p) = f.capture.map_or((2, 0), |(t, p)| (side_of(t), (p.clamp(0.0, 1.0) * 255.0) as u8));
        w.u8(team_byte(f.owner)).u8(t).u8(p).bool(f.contested);
    }
    // Bomb sites.
    w.u8(o.sites.len().min(8) as u8);
    for s in o.sites.iter().take(8) {
        w.u8(s.destroyed as u8 | team_byte(s.team) << 1);
    }
    // The bomb.
    w.bool(o.bomb.is_some());
    if let Some(b) = &o.bomb {
        w.vec3(b.pos).u16(b.carrier.map_or(BOT, id)).u8(b.planted.map_or(0, |c| c as u8)).f32(b.explodes_at.map_or(-1.0, |t| rel(t).max(0.0)));
    }
    w.u8(team_byte(o.attackers));
    w.bool(o.timer.is_some());
    if let Some((t, bomb)) = o.timer {
        w.f32(rel(t)).bool(bomb);
    }
    w.u8(o.using.len().min(8) as u8);
    for (e, p, defusing) in o.using.iter().take(8) {
        w.u16(id(*e)).u8((p.clamp(0.0, 1.0) * 255.0) as u8).bool(*defusing);
    }
    w.bool(o.round_over.is_some());
    if let Some((t, key, text)) = o.round_over {
        w.u8(side_of(t)).str(key, MAX_TEXT).str(text, MAX_TEXT);
    }
    w.bool(o.hq.is_some());
    if let Some(hq) = &o.hq {
        let (t, p) = hq.capture.map_or((2, 0), |(t, p)| (side_of(t), (p.clamp(0.0, 1.0) * 255.0) as u8));
        w.vec3(hq.pos).vec3(hq.min).vec3(hq.max).u8(team_byte(hq.owner)).u8(t).u8(p).bool(hq.contested).f32(hq.expires_at.map_or(-1.0, |t| rel(t).max(0.0)));
    }
    // UAVs.
    let up = radar.entries();
    w.u8(up.len().min(8) as u8);
    for (side, start, until) in up.iter().take(8) {
        match side {
            uav::Side::Team(t) => w.u8(side_of(*t)).u16(0),
            uav::Side::Player(e) => w.u8(2).u16(id(*e)),
        };
        w.f32(rel(*start)).f32(rel(*until));
    }
    // Everyone's ping, for the scoreboard.
    w.u8(pings.len().min(32) as u8);
    for (pawn, ms) in pings.iter().take(32) {
        w.u16(*pawn).u16(*ms);
    }
    w.0
}

/// The world part of a snapshot's extra and what follows it (the mirrors,
/// [`super::streaks`]).
pub(super) fn split(extra: &[u8]) -> Option<(&[u8], &[u8])> {
    let n = u16::from_le_bytes(extra.get(..2)?.try_into().ok()?) as usize;
    let world = extra.get(2..2 + n)?;
    Some((world, &extra[2 + n..]))
}

/// Host: the one-off news for guests.
#[allow(clippy::too_many_arguments)]
pub(super) fn host_news(
    online: Res<Online>,
    time: Res<Time>,
    objectives: Option<Res<Objectives>>,
    state: Option<Res<crate::tdm::MatchState>>,
    mut awards: MessageReader<Award>,
    mut notices: MessageReader<StreakNotice>,
    mut damage: MessageReader<crate::combat::Damage>,
    strikes: Res<crate::killstreaks::airstrike::Airstrikes>,
    pawns: Query<(&Pawn, Option<&RemotePlayer>, Option<&Killstreak>)>,
    mut sent: Local<HostNews>,
) {
    let t = time.elapsed_secs();
    let id = |e: Entity| pawns.get(e).map_or(BOT, |(p, ..)| p.id as u16);
    let peer = |e: Entity| pawns.get(e).ok().and_then(|(_, r, _)| r.map(|r| r.peer));
    let send = |to: PeerId, m: GameMsg| online.send(to, &LobbyMsg::Game(m.encode()));
    // A round ended (its killcam, everyone held).
    if let Some(o) = &objectives {
        let now: Vec<Option<Team>> = o.flags.iter().map(|f| f.owner).collect();
        for (f, owner) in o.flags.iter().zip(&now) {
            if sent.flags.get(f.label as usize % 8).copied().flatten() != Some(*owner) {
                info!("netplay: objective: flag {} now {:?}", f.label, owner);
            }
        }
        sent.flags = (0..8).map(|i| o.flags.iter().find(|f| f.label as usize % 8 == i).map(|f| f.owner)).collect();
        let key = (o.bomb.and_then(|b| b.planted), o.round_over.map(|r| r.0), o.hq.as_ref().map(|h| h.owner));
        if sent.objective != Some(key) {
            sent.objective = Some(key);
            info!("netplay: objective: bomb planted {:?}, round over {:?}, hq {:?}", key.0, o.round_over.map(|r| (r.0, r.2)), key.2);
        }
    }
    let over = objectives.as_ref().is_some_and(|o| o.round_over.is_some());
    if over && !sent.round_over {
        send(EVERYONE, GameMsg::RoundOver);
    }
    sent.round_over = over;
    // Free-for-all's winner, before the end (it's in the results).
    if let Some(s) = &state
        && s.ended.is_some()
        && !sent.winner
    {
        sent.winner = true;
        if let Some(w) = &s.winner {
            send(EVERYONE, GameMsg::Winner(crate::online::lobby::clean_name(w)));
        }
    } else if state.as_ref().is_some_and(|s| s.ended.is_none()) {
        sent.winner = false;
    }
    for a in awards.read() {
        if let Some(p) = peer(a.pawn) {
            send(p, GameMsg::Award(award_index(a.kind)));
        }
    }
    for n in notices.read() {
        if let StreakNotice::CalledIn { item, by, team } = n {
            send(EVERYONE, GameMsg::Notice { kind: 0, item: hardpoint_index(*item), by: crate::online::lobby::clean_name(by), team: side_of(*team) });
        }
    }
    // A friend hurt: their damage direction and hit feedback (summed, at
    // most ten a second each: a burst is one message).
    for d in damage.read() {
        if let Some(p) = peer(d.target)
            && d.amount > 0.0
        {
            let entry = sent.hurt.entry(p).or_insert((0.0, BOT, 2, 0.0));
            entry.0 += d.amount;
            entry.1 = d.attacker.map_or(BOT, id);
            entry.2 = location_byte(d.location);
        }
    }
    for (p, (amount, attacker, location, next)) in sent.hurt.iter_mut() {
        if *amount > 0.0 && t >= *next {
            send(*p, GameMsg::Hurt { attacker: *attacker, amount: amount.round().clamp(1.0, 255.0) as u8, location: *location });
            *amount = 0.0;
            *next = t + 0.1;
        }
    }
    // Friends' own streaks.
    for (pawn, remote, streak) in &pawns {
        let (Some(r), Some(k)) = (remote, streak) else { continue };
        let now = (k.kills.min(u16::MAX as u32) as u16, held_bits(k));
        if sent.streaks.get(&r.peer) != Some(&now) {
            sent.streaks.insert(r.peer, now);
            send(r.peer, GameMsg::Streak { kills: now.0, held: now.1, chosen: Vec::new() });
            debug!("netplay: {}'s streak {} (held {:#b})", pawn.name, now.0, now.1);
        }
    }
    // Airstrikes: guests fly them too.
    sent.strikes.retain(|c| *c > t - 30.0);
    for (owner, target, dir, called, planes) in strikes.under_way() {
        if sent.strikes.iter().any(|c| (*c - called).abs() < 1e-3) {
            continue;
        }
        sent.strikes.push(called);
        // (The planes' times after the call; the guest's call is now.)
        send(EVERYONE, GameMsg::Airstrike { owner: id(owner), target, dir, planes: planes.map(|p| p - called) });
    }
}

#[derive(Default)]
pub(super) struct HostNews {
    round_over: bool,
    winner: bool,
    streaks: HashMap<PeerId, (u16, u8)>,
    strikes: Vec<f32>,
    /// Objectives last logged.
    flags: Vec<Option<Option<Team>>>,
    objective: Option<(Option<char>, Option<Team>, Option<Option<Team>>)>,
    /// Damage to tell each friend: amount, by whom, where, not before.
    hurt: HashMap<PeerId, (f32, u16, u8, f32)>,
}

pub(super) fn location_byte(l: crate::combat::HitLocation) -> u8 {
    match l {
        crate::combat::HitLocation::Head => 0,
        crate::combat::HitLocation::Neck => 1,
        crate::combat::HitLocation::Torso => 2,
        crate::combat::HitLocation::Legs => 3,
        crate::combat::HitLocation::TorsoLower => 4,
    }
}

pub(super) fn location_from(b: u8) -> crate::combat::HitLocation {
    match b {
        0 => crate::combat::HitLocation::Head,
        1 => crate::combat::HitLocation::Neck,
        3 => crate::combat::HitLocation::Legs,
        4 => crate::combat::HitLocation::TorsoLower,
        _ => crate::combat::HitLocation::Torso,
    }
}

/// Host: what friends ask of it beyond their controls.
pub(super) fn host_requests(
    mut inbox: ResMut<HostInbox>,
    mut remotes: Query<(&RemotePlayer, &mut crate::killstreaks::HardpointInput, &mut Killstreak)>,
    mut stats: ResMut<super::graph::NetStats>,
) {
    for (peer, msg) in inbox.0.drain(..) {
        match msg {
            GameMsg::UseStreak { item, target } => {
                let Some((_, mut input, _)) = remotes.iter_mut().find(|(r, ..)| r.peer == peer) else { continue };
                input.use_now = true;
                input.item = hardpoint_from(item);
                input.target = target;
            }
            GameMsg::Chosen(list) => {
                let Some((_, _, mut streak)) = remotes.iter_mut().find(|(r, ..)| r.peer == peer) else { continue };
                let mut chosen: Vec<Hardpoint> = list.iter().filter_map(|i| hardpoint_from(*i)).collect();
                chosen.dedup();
                chosen.sort_by_key(|h| h.kills());
                chosen.truncate(crate::killstreaks::choice::MAX);
                streak.chosen = chosen;
            }
            GameMsg::Ping(ms) => {
                stats.peer_ping.insert(peer, ms);
            }
            _ => debug!("netplay: ignoring a host's message from player {peer}"),
        }
    }
}

/// Host test aid (`COD4RW_NETTEST_GIVE=1`): friends hold every kill
/// streak (and their picks are all of them), to call in from their game.
pub(super) fn test_give(mut remotes: Query<&mut Killstreak, (With<RemotePlayer>, Added<Killstreak>)>) {
    if !std::env::var("COD4RW_NETTEST_GIVE").is_ok_and(|v| v == "1") {
        return;
    }
    for mut k in &mut remotes {
        for h in Hardpoint::ALL {
            k.held.add(h);
        }
        info!("netplay: test: a friend holds every kill streak");
    }
}

/// Host: what friends sent beyond controls, for [`host_requests`].
#[derive(Resource, Default)]
pub(crate) struct HostInbox(pub Vec<(PeerId, GameMsg)>);

/// Guest: the host's news, for [`guest_news`].
#[derive(Resource, Default)]
pub(crate) struct GuestInbox(pub Vec<GameMsg>);

// ---- Guest ----

/// The pawn the host calls `id`, as this game has it.
pub(super) fn entity_of(id: u16, my_id: Option<u16>, me: Option<Entity>, puppets: &Query<(Entity, &Puppet)>) -> Option<Entity> {
    if Some(id) == my_id {
        me
    } else {
        puppets.iter().find(|(_, p)| p.id == id).map(|(e, _)| e)
    }
}

/// Guest: the host's ongoing state from the newest snapshot.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply(
    time: Res<Time>,
    guest: Res<GuestState>,
    mut applied: Local<Option<u32>>,
    state: Option<ResMut<crate::tdm::MatchState>>,
    objectives: Option<ResMut<Objectives>>,
    mut radar: ResMut<uav::Radar>,
    puppets: Query<(Entity, &Puppet)>,
    me: Query<Entity, With<LocalPlayer>>,
    prediction: Option<Res<super::predict::Prediction>>,
) {
    let Some(snap) = guest.latest.as_ref() else { return };
    if *applied == Some(snap.tick) {
        return;
    }
    *applied = Some(snap.tick);
    let Some((world, _)) = split(&snap.extra) else { return };
    let me = me.single().ok();
    let map = |id: u16| entity_of(id, guest.my_id, me, &puppets);
    if let Err(e) = decode_into(&mut R(world), time.elapsed_secs(), state, objectives, &mut radar, &map, (guest.my_id, prediction.and_then(|p| p.rtt))) {
        warn!("netplay: a bad snapshot state: {e}");
    }
}

fn decode_into(
    r: &mut R,
    now: f32,
    state: Option<ResMut<crate::tdm::MatchState>>,
    objectives: Option<ResMut<Objectives>>,
    radar: &mut uav::Radar,
    map: &dyn Fn(u16) -> Option<Entity>,
    (my_id, my_rtt): (Option<u16>, Option<f32>),
) -> anyhow::Result<()> {
    let at = |rel: f32| now + rel;
    let left = r.f32()?;
    let (allies, axis) = (r.u16()? as u32, r.u16()? as u32);
    if let Some(mut s) = state {
        if left >= 0.0 && s.time_limit.is_finite() && s.ended.is_none() {
            // `time_left` = limit - (now - started).
            let started = now - (s.time_limit - left);
            if (started - s.started).abs() > 0.25 {
                s.started = started;
            }
        }
        if (s.allies, s.axis) != (allies, axis) {
            (s.allies, s.axis) = (allies, axis);
        }
    }
    let bits = r.u8()?;
    let mut o = objectives;
    if let Some(o) = o.as_deref_mut() {
        o.sudden_death = bits & 1 != 0;
    }
    crate::modes::lock_respawns(Team::Allies, bits & 2 != 0);
    crate::modes::lock_respawns(Team::Axis, bits & 4 != 0);
    let n = r.u8()? as usize;
    anyhow::ensure!(n <= 8, "too many flags");
    for i in 0..n {
        let (owner, t, p, contested) = (r.u8()?, r.u8()?, r.u8()?, r.bool()?);
        if let Some(f) = o.as_deref_mut().and_then(|o| o.flags.get_mut(i)) {
            if f.owner != team_from(owner) {
                info!("netplay: objective: flag {} now {:?}", f.label, team_from(owner));
            }
            f.owner = team_from(owner);
            f.capture = team_from(t).map(|t| (t, p as f32 / 255.0));
            f.contested = contested;
        }
    }
    let n = r.u8()? as usize;
    anyhow::ensure!(n <= 8, "too many sites");
    for i in 0..n {
        let b = r.u8()?;
        if let Some(s) = o.as_deref_mut().and_then(|o| o.sites.get_mut(i)) {
            s.destroyed = b & 1 != 0;
            s.team = team_from(b >> 1);
        }
    }
    let bomb = if r.bool()? {
        let pos = r.vec3()?;
        let carrier = r.u16()?;
        let planted = r.u8()?;
        let explodes = r.f32()?;
        Some(Bomb {
            pos,
            carrier: if carrier == BOT { None } else { map(carrier) },
            planted: (planted != 0).then_some(planted as char).filter(|c| c.is_ascii_alphabetic()),
            explodes_at: (explodes >= 0.0).then(|| at(explodes)),
        })
    } else {
        None
    };
    let attackers = team_from(r.u8()?);
    let timer = if r.bool()? { Some((at(r.f32()?), r.bool()?)) } else { None };
    let n = r.u8()? as usize;
    anyhow::ensure!(n <= 8, "too many users");
    let mut using = Vec::new();
    for _ in 0..n {
        let (id, p, defusing) = (r.u16()?, r.u8()?, r.bool()?);
        if let Some(e) = map(id) {
            using.push((e, p as f32 / 255.0, defusing));
        }
    }
    let round_over = if r.bool()? {
        let t = team_from(r.u8()?).unwrap_or(Team::Allies);
        Some((t, intern(&r.str(MAX_TEXT)?), intern(&r.str(MAX_TEXT)?)))
    } else {
        None
    };
    let hq = if r.bool()? {
        let (pos, min, max) = (r.vec3()?, r.vec3()?, r.vec3()?);
        let (owner, t, p, contested, expires) = (r.u8()?, r.u8()?, r.u8()?, r.bool()?, r.f32()?);
        Some(Hq { pos, min, max, owner: team_from(owner), capture: team_from(t).map(|t| (t, p as f32 / 255.0)), contested, expires_at: (expires >= 0.0).then(|| at(expires)) })
    } else {
        None
    };
    if let Some(o) = o.as_deref_mut() {
        if o.bomb.and_then(|b| b.planted) != bomb.and_then(|b| b.planted) || o.round_over.map(|r| r.0) != round_over.map(|r| r.0) || o.hq.as_ref().map(|h| h.owner) != hq.as_ref().map(|h| h.owner) {
            info!("netplay: objective: bomb planted {:?}, round over {:?}, hq {:?}", bomb.and_then(|b| b.planted), round_over.map(|r| (r.0, r.2)), hq.as_ref().map(|h| h.owner));
        }
        o.bomb = bomb;
        o.attackers = attackers;
        o.timer = timer;
        o.using = using;
        o.round_over = round_over;
        o.hq = hq;
    }
    let n = r.u8()? as usize;
    anyhow::ensure!(n <= 8, "too many UAVs");
    let mut up = Vec::new();
    for _ in 0..n {
        let (kind, id) = (r.u8()?, r.u16()?);
        let (start, until) = (r.f32()?, r.f32()?);
        let side = match kind {
            0 | 1 => uav::Side::Team(team_of(kind)),
            _ => match map(id) {
                Some(e) => uav::Side::Player(e),
                None => continue,
            },
        };
        up.push((side, at(start), at(until)));
    }
    radar.replace(up);
    let n = r.u8()? as usize;
    anyhow::ensure!(n <= 32, "too many pings");
    let mut pings = Vec::with_capacity(n);
    for _ in 0..n {
        pings.push((r.u16()? as u32, r.u16()?));
    }
    r.done()?;
    // Ours as we measure it (the host has it from us, a moment old).
    if let (Some(me), Some(rtt)) = (my_id, my_rtt) {
        for p in &mut pings {
            if p.0 == me as u32 {
                p.1 = (rtt * 1000.0).round().clamp(0.0, 9999.0) as u16;
            }
        }
    }
    super::set_pings(pings);
    Ok(())
}

/// Guest: the host's one-off news.
#[allow(clippy::too_many_arguments)]
pub(super) fn guest_news(
    time: Res<Time>,
    mut inbox: ResMut<GuestInbox>,
    guest: Res<GuestState>,
    mut state: Option<ResMut<crate::tdm::MatchState>>,
    mut me: Query<(Entity, Option<&mut Killstreak>), With<LocalPlayer>>,
    puppets: Query<(Entity, &Puppet)>,
    (mut awards, mut notices, mut damage): (MessageWriter<Award>, MessageWriter<StreakNotice>, MessageWriter<crate::combat::Damage>),
    mut strikes: ResMut<crate::killstreaks::airstrike::Airstrikes>,
    mut blasts: Query<(&Transform, &mut crate::explosives::Explosive)>,
) {
    let now = time.elapsed_secs();
    let mine = me.single().ok().map(|(e, _)| e);
    let map = |id: u16| entity_of(id, guest.my_id, mine, &puppets);
    for msg in std::mem::take(&mut inbox.0) {
        match msg {
            GameMsg::RoundOver => {
                info!("netplay: the round is over");
                crate::killcam::round_over(now);
            }
            GameMsg::Award(k) => {
                if let (Some(kind), Some(pawn)) = (award_from(k), mine) {
                    awards.write(Award { pawn, kind });
                }
            }
            GameMsg::Notice { kind: 0, item, by, team } => {
                if let (Some(item), Some(team)) = (hardpoint_from(item), team_from(team)) {
                    notices.write(StreakNotice::CalledIn { item, by, team });
                }
            }
            GameMsg::Notice { .. } => {}
            GameMsg::Streak { kills, held, .. } => {
                let Ok((_, Some(mut k))) = me.single_mut() else { continue };
                k.kills = kills as u32;
                for (i, h) in Hardpoint::ALL.into_iter().enumerate() {
                    let has = held & 1 << i != 0;
                    if has && !k.held.has(h) {
                        k.held.add(h);
                        notices.write(StreakNotice::Earned { streak: kills as u32, item: h });
                    } else if !has && k.held.has(h) {
                        k.held.remove(h);
                    }
                }
            }
            GameMsg::Opened(c) => {
                notices.write(StreakNotice::Opened { item: c.checked_sub(1).and_then(hardpoint_from) });
            }
            GameMsg::Airstrike { owner, target, dir, planes } => {
                let owner = map(owner).unwrap_or(Entity::PLACEHOLDER);
                info!("netplay: an airstrike on CoD {:?}", crate::units::to_cod(target).map(f32::round));
                strikes.replay(owner, target, dir, now, planes.map(|p| now + p.clamp(0.0, 30.0)));
            }
            GameMsg::Blast { weapon, at } => {
                // Ours to set off now (where it went off on the host).
                let near = blasts.iter_mut().filter(|(tf, x)| x.weapon == weapon && tf.translation.distance(at) < 1.5).min_by(|a, b| a.0.translation.distance(at).total_cmp(&b.0.translation.distance(at)));
                if let Some((_, mut x)) = near
                    && !matches!(x.state, crate::explosives::State::Triggered(_))
                {
                    x.state = crate::explosives::State::Triggered(now);
                }
            }
            GameMsg::Winner(name) => {
                if let Some(s) = state.as_deref_mut() {
                    s.winner = Some(name);
                }
            }
            GameMsg::Hurt { attacker, amount, location } => {
                let Some(target) = mine else { continue };
                damage.write(crate::combat::Damage {
                    target,
                    attacker: if attacker == BOT { None } else { map(attacker) },
                    amount: amount as f32,
                    location: location_from(location),
                    weapon: "",
                });
            }
            GameMsg::UseStreak { .. } | GameMsg::Ping(_) | GameMsg::Chosen(_) => {}
        }
    }
}

/// Guest: our kill streak calls go to the host (our own game's stand
/// down), and our picked streaks once.
pub(super) fn guest_streaks(
    online: Res<Online>,
    mut me: Query<(&mut crate::killstreaks::HardpointInput, &Killstreak), With<LocalPlayer>>,
    selecting: Option<Res<crate::killstreaks::airstrike::Selecting>>,
    mut commands: Commands,
    mut told: Local<Option<Vec<u8>>>,
    me_entity: Query<Entity, With<LocalPlayer>>,
) {
    let Ok((mut input, streak)) = me.single_mut() else { return };
    let chosen: Vec<u8> = streak.chosen.iter().map(|h| hardpoint_index(*h)).collect();
    if told.as_ref() != Some(&chosen) {
        online.send(HOST, &LobbyMsg::Game(GameMsg::Chosen(chosen.clone()).encode()));
        *told = Some(chosen);
    }
    if !std::mem::take(&mut input.use_now) {
        return;
    }
    let target = input.target.take();
    let asked = input.item.take();
    let item = asked.filter(|&h| streak.held.has(h)).or_else(|| streak.held.best());
    let Some(item) = item else { return };
    // An airstrike: picked on our map first.
    if item == Hardpoint::Airstrike && target.is_none() {
        match selecting {
            Some(_) => commands.remove_resource::<crate::killstreaks::airstrike::Selecting>(),
            None => {
                if let Ok(owner) = me_entity.single() {
                    commands.insert_resource(crate::killstreaks::airstrike::Selecting { owner });
                }
            }
        }
        return;
    }
    if selecting.is_some() {
        commands.remove_resource::<crate::killstreaks::airstrike::Selecting>();
    }
    info!("netplay: calling in {}", item.name());
    online.send(HOST, &LobbyMsg::Game(GameMsg::UseStreak { item: hardpoint_index(item), target }.encode()));
}
