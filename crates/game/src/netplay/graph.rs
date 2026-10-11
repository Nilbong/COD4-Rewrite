//! The net graph (`COD4RW_NETGRAPH=1`): how the connection is doing, in
//! the top right corner (under the compass), and every 5 s in the log.
//!
//! A guest shows its round trip to the host (ping), the share of snapshots
//! lost, how many arrive a second, how far behind everyone else is drawn
//! (interpolation delay), the jitter, and how its own soldier's prediction
//! is holding (the last correction, eased and snapped ones). The host shows
//! each friend's ping (as their game measures it), how many of their
//! commands arrive a second and how many go missing.

use super::*;

/// What the net graph shows (kept whether it's shown or not).
#[derive(Resource, Default, Debug)]
pub struct NetStats {
    // Guest.
    pub ping: Option<f32>,
    pub loss: f32,
    pub snapshot_rate: f32,
    pub interp_ms: f32,
    pub jitter_ms: f32,
    pub error_cm: f32,
    pub corrections: u32,
    pub snaps: u32,
    pub commands_sent: u32,
    /// Our own bullets fired, and hits our game showed (hit markers).
    pub shots: u32,
    pub hits: u32,
    // Host: by friend.
    /// Their bullets, and the hits that counted here.
    pub peer_shots: HashMap<PeerId, (u32, u32)>,
    pub peer_ping: HashMap<PeerId, u16>,
    pub peer_rate: HashMap<PeerId, f32>,
    pub peer_loss: HashMap<PeerId, f32>,
}

pub fn enabled() -> bool {
    std::env::var("COD4RW_NETGRAPH").is_ok_and(|v| !v.is_empty() && v != "0")
}

#[derive(Component)]
pub(super) struct GraphText;

/// The lines to show.
fn lines(stats: &NetStats, guest: bool, names: &HashMap<PeerId, String>) -> String {
    if guest {
        format!(
            "ping {}  loss {:.1}%  snap {:.1}/s\ninterp {:.0} ms  jitter {:.0} ms\npredict err {:.1} cm  eased {}  snapped {}  cmds {}\nshots {}  hits shown {}",
            stats.ping.map_or("--".into(), |p| format!("{:.0} ms", p * 1000.0)),
            stats.loss * 100.0,
            stats.snapshot_rate,
            stats.interp_ms,
            stats.jitter_ms,
            stats.error_cm,
            stats.corrections,
            stats.snaps,
            stats.commands_sent,
            stats.shots,
            stats.hits,
        )
    } else {
        let mut peers: Vec<&PeerId> = stats.peer_rate.keys().collect();
        peers.sort();
        let mut out = format!("host: {} friend(s)", peers.len());
        for p in peers {
            out += &format!(
                "\n{}: ping {}  cmds {:.0}/s  lost {:.1}%  shots {}  hits {}",
                names.get(p).map_or("?", |n| n.as_str()),
                stats.peer_ping.get(p).map_or("--".into(), |v| format!("{v} ms")),
                stats.peer_rate.get(p).copied().unwrap_or(0.0),
                stats.peer_loss.get(p).copied().unwrap_or(0.0) * 100.0,
                stats.peer_shots.get(p).map_or(0, |s| s.0),
                stats.peer_shots.get(p).map_or(0, |s| s.1),
            );
        }
        out
    }
}

/// Show (and log) the net graph.
#[allow(clippy::too_many_arguments)]
pub(super) fn show(
    mut commands: Commands,
    time: Res<Time>,
    net: Res<NetStart>,
    mut stats: ResMut<NetStats>,
    prediction: Option<Res<super::predict::Prediction>>,
    remotes: Query<(&Pawn, &RemotePlayer)>,
    mut text: Query<&mut Text, With<GraphText>>,
    mut next: Local<(f32, f32)>,
) {
    let guest = net.me != HOST;
    if let Some(p) = prediction.as_deref() {
        stats.ping = p.rtt;
        stats.error_cm = p.error * 100.0;
        stats.corrections = p.corrections;
        stats.snaps = p.snaps;
        stats.commands_sent = p.sent;
    }
    let now = time.elapsed_secs();
    if now < next.0 {
        return;
    }
    next.0 = now + 0.25;
    let names: HashMap<PeerId, String> = remotes.iter().map(|(p, r)| (r.peer, p.name.clone())).collect();
    let s = lines(&stats, guest, &names);
    if enabled() && now >= next.1 {
        next.1 = now + 5.0;
        info!("netgraph: {}", s.replace('\n', " | "));
    }
    if !enabled() {
        return;
    }
    match text.single_mut() {
        Ok(mut t) => t.0 = s,
        Err(_) => {
            commands.spawn((
                GraphText,
                Text::new(s),
                TextFont { font_size: FontSize::Px(15.0), ..default() },
                TextColor(Color::srgb(0.6, 1.0, 0.6)),
                TextShadow::default(),
                Node { position_type: PositionType::Absolute, right: px(8), top: px(64), ..default() },
                GlobalZIndex(1000),
                DespawnOnExit(crate::state::GameState::InGame),
            ));
        }
    }
}

/// Bullets and the hits they made: ours on a guest (what its hit markers
/// showed), each friend's on the host (what counted), for the net graph.
pub(super) fn count_hits(
    mut stats: ResMut<NetStats>,
    mut shots: MessageReader<ShotFired>,
    mut hits: MessageReader<crate::weapons::HitConfirmed>,
    mine: Query<(), With<LocalPlayer>>,
    remotes: Query<&RemotePlayer>,
    net: Res<NetStart>,
) {
    let guest = net.me != HOST;
    for s in shots.read() {
        // (Bullets: a gun's shot that went somewhere.)
        if s.from == s.to {
            continue;
        }
        if guest && mine.contains(s.shooter) {
            stats.shots += 1;
        } else if let Ok(r) = remotes.get(s.shooter) {
            stats.peer_shots.entry(r.peer).or_default().0 += 1;
        }
    }
    for h in hits.read() {
        if guest && mine.contains(h.shooter) {
            stats.hits += 1;
        } else if let Ok(r) = remotes.get(h.shooter) {
            stats.peer_shots.entry(r.peer).or_default().1 += 1;
        }
    }
}
