//! Joining a match under way, and bots filling in for whoever leaves.
//!
//! While the host's match is on it takes players in
//! ([`AcceptsJoinInProgress`]): the lobby (matchmaking's public one) seats a
//! joiner on a side in place of a bot, sends [`JoinInProgress`] and tells
//! their game to load the match. The host's game takes that bot out and
//! their soldier comes in as any friend's does ([`super::spawn_remotes`]),
//! picking a class. Everything ongoing reaches them in the snapshots
//! ([`super::world`]); everyone's details go out again for them.
//!
//! When a friend leaves mid-match a bot takes their place on their side.

use super::*;
use crate::bots::Bot;
use crate::matchmaking::{AcceptsJoinInProgress, JoinInProgress};

/// The host's match takes players in ([`AcceptsJoinInProgress`]) while it's
/// on.
pub(super) fn accept(commands: &mut Commands, on: bool) {
    if on {
        commands.insert_resource(AcceptsJoinInProgress);
    } else {
        commands.remove_resource::<AcceptsJoinInProgress>();
    }
}

/// Host: friends let in mid-match come in, in place of the bot the lobby
/// named (if any: a side with room takes them as well).
pub(super) fn admit_joiners(
    mut commands: Commands,
    mut state: ResMut<HostState>,
    mut joins: MessageReader<JoinInProgress>,
    bots: Query<(Entity, &Pawn, Has<Dead>), With<Bot>>,
) {
    for j in joins.read() {
        let m = j.member.clone();
        info!("netplay: {} joins the match in progress", m.profile.name);
        // The bot the lobby named makes way (none: the side had room).
        if let Some(name) = &j.replaces_bot {
            let named = bots.iter().find(|(_, p, _)| &p.name == name);
            // (Gone already: the side's weakest, a dead one first.)
            let fallback = || {
                let team = team_of(m.team);
                let mut on_side: Vec<(Entity, &Pawn, bool)> = bots.iter().filter(|(_, p, _)| p.team == team).collect();
                on_side.sort_by_key(|(_, p, dead)| (!dead, p.kills));
                on_side.first().copied()
            };
            if let Some((e, p, _)) = named.or_else(fallback) {
                info!("netplay: {} makes way for {}", p.name, m.profile.name);
                commands.entity(e).despawn();
            }
        }
        state.members.retain(|o| o.peer != m.peer);
        state.members.push(m.clone());
        // Everyone's details again, for the newcomer.
        state.sent.clear();
        state.waiting.push(m);
    }
}

/// Host: a bot for each friend who left mid-match.
pub(super) fn refill(
    mut commands: Commands,
    mut state: ResMut<HostState>,
    assets: Option<Res<PawnAssets>>,
    map: Option<Res<crate::world::MapInfo>>,
    config: Option<Res<crate::tdm::MatchConfig>>,
    match_state: Option<Res<crate::tdm::MatchState>>,
    pawns: Query<(&Pawn, &Transform)>,
) {
    if state.refill.is_empty() {
        return;
    }
    let (Some(assets), Some(map)) = (assets, map) else { return };
    let teams = std::mem::take(&mut state.refill);
    if match_state.is_some_and(|s| s.ended.is_some()) {
        return;
    }
    let skill = config.map_or(0.5, |c| c.bot_skill);
    let mut taken: Vec<String> = pawns.iter().map(|(p, _)| p.name.clone()).collect();
    let occupied: Vec<Vec3> = pawns.iter().map(|(_, tf)| tf.translation).collect();
    for team in teams {
        let Some(name) = (0..64).map(crate::tdm::bot_name).find(|n| !taken.contains(n)) else { continue };
        taken.push(name.clone());
        let spawn = crate::combat::pick_spawn(&map, &crate::combat::Spawning::start(team, &occupied));
        let bot = spawn_pawn(&mut commands, &assets, &name, team, &spawn);
        let lane = rand::random_range(-0.8..0.8);
        commands.entity(bot).insert((Bot::new(skill, None).with_lane(lane), WeaponInput::default()));
        info!("netplay: {name} fills in on {team:?}");
    }
}
