//! A guest's own soldier: moved at once by its own controls (prediction),
//! and put right by replaying them over the host's word (reconciliation).
//!
//! Each frame the guest moves its soldier itself, with the same movement
//! code as the host. Up to 60 times a second the frames so far go to the
//! host as one command (the controls, and how long they were held:
//! [`InputCommand::msec`]); the host moves the friend's soldier by exactly
//! those, command by command ([`super::step_remotes`]). Each snapshot says
//! which command the host has got to ([`Snapshot::acknowledged_input`]) and
//! where that left the soldier: the guest starts from there (with its own
//! record of the soldier's finer state then) and runs the commands the host
//! hasn't had yet, and the frames since. Where that ends is where the
//! soldier should be: a small difference is eased out, a big one (blocked
//! by someone on the host, a knock) snaps.
//!
//! [`InputCommand::msec`]: cod4rw_multiplayer::protocol::InputCommand::msec
//! [`Snapshot::acknowledged_input`]: cod4rw_multiplayer::protocol::Snapshot::acknowledged_input

use super::*;
use crate::movement::{MoveWorld, NetDriven};
use std::collections::VecDeque;

/// Differences smaller than this are rounding: left alone (m).
const NOISE: f32 = 0.01;
/// Differences larger than this snap; smaller ones are eased out (m).
const SNAP: f32 = 1.0;
/// The share of a difference taken out per snapshot.
const EASE: f32 = 0.5;
/// Commands kept waiting for the host (4 s at 60 a second).
const KEPT: usize = 240;

/// The guest's prediction ([`predict_own`]).
#[derive(Resource, Default)]
pub(crate) struct Prediction {
    /// Commands the host hasn't acknowledged, oldest first.
    history: VecDeque<Entry>,
    /// The command closed this frame ([`send_inputs`]), waiting for this
    /// frame's move to finish it.
    closing: Option<InputCommand>,
    /// Frames already moved since the last command closed (ms).
    since_command: f32,
    /// Frame time not yet sent (ms), and the fraction of a millisecond
    /// carried over.
    pending: f32,
    carry: f32,
    /// Command numbers, and the last few sent (redundancy).
    sequence: u32,
    recent: Vec<InputCommand>,
    next_send: f32,
    held: u16,
    in_hand: Option<usize>,
    /// The snapshot tick last reconciled with.
    reconciled: Option<u32>,
    /// For the net graph: the last correction (m), corrections made, the
    /// round trip (s, smoothed).
    pub error: f32,
    pub corrections: u32,
    pub snaps: u32,
    pub rtt: Option<f32>,
    pub sent: u32,
}

struct Entry {
    sequence: u32,
    input: MoveInput,
    view: ViewAngles,
    msec: f32,
    /// Held still (the host doesn't move it either).
    frozen: bool,
    /// Its finer movement state at the end (the host sends only where it
    /// is and how fast).
    mover: Mover,
    sent_at: f32,
}

impl Prediction {
    /// Forget it all (respawned, teleported): the next acknowledgement
    /// starts afresh.
    pub fn reset(&mut self) {
        self.history.clear();
        self.since_command = 0.0;
    }
}

/// A command's movement controls, as the host runs them (shared, so the
/// guest's replay runs the same).
pub(crate) fn move_input_of(c: &InputCommand, speed_scale: f32, held_still: bool) -> MoveInput {
    let pressed = |b: u16| c.buttons & b != 0;
    let mut mv = MoveInput {
        forward: c.forward as f32 / 127.0,
        right: c.right as f32 / 127.0,
        jump: pressed(button::JUMP),
        sprint: pressed(button::SPRINT),
        stance: match c.stance {
            1 => Stance::Crouch,
            2 => Stance::Prone,
            _ => Stance::Stand,
        },
        speed_scale,
        lean: 0.0,
    };
    // Planting or defusing ([`crate::modes`]): nobody moves.
    if held_still {
        (mv.forward, mv.right, mv.jump, mv.sprint) = (0.0, 0.0, false, false);
    }
    mv
}

/// Our controls to the host, up to 60 times a second, each with the few
/// before it in case some go missing.
#[allow(clippy::type_complexity)]
pub(crate) fn send_inputs(
    online: Res<Online>,
    time: Res<Time>,
    state: Res<GuestState>,
    mut p: ResMut<Prediction>,
    me: Query<
        (&MoveInput, &ViewAngles, &WeaponInput, Option<&crate::grenades::GrenadeInput>, Option<&crate::loadout::Loadout>, Option<&crate::splitscreen::PlayerInput>),
        With<LocalPlayer>,
    >,
) {
    let Ok((mv, view, wi, grenades, loadout, player_input)) = me.single() else { return };
    let p = &mut *p;
    let mut buttons = 0;
    let mut set = |on: bool, b: u16| {
        if on {
            buttons |= b;
        }
    };
    set(wi.fire, button::FIRE);
    set(wi.ads, button::AIM);
    set(mv.jump, button::JUMP);
    set(mv.sprint, button::SPRINT);
    set(wi.reload, button::RELOAD);
    // 3rd Person TDM: the cover button; the host does the rest ([`crate::cover`]).
    set(crate::cover::active() && player_input.is_some_and(|p| p.live && p.keys.just_pressed(crate::cover::COVER_KEY)) || crate::cover::autokey(), button::COVER);
    set(grenades.is_some_and(|g| g.frag), button::FRAG);
    set(grenades.is_some_and(|g| g.special), button::SPECIAL);
    // "Use" (F, the pad's X): planting, defusing, opening a crate.
    set(player_input.is_some_and(|p| p.live && (p.keys.pressed(KeyCode::KeyF) || p.pad.interact)), button::USE);
    // Changed between primary and secondary: the host follows.
    let current = loadout.map(|l| l.current);
    set(p.in_hand.is_some() && current != p.in_hand && current.is_some_and(|c| c < 2), button::SWITCH);
    p.in_hand = current;
    // Taps between sends still count.
    p.held |= buttons;
    p.pending += time.delta_secs().min(0.1) * 1000.0;
    let now = time.elapsed_secs();
    if now < p.next_send {
        return;
    }
    p.next_send = now + 1.0 / TICK_RATE;
    let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    p.sequence = p.sequence.wrapping_add(1).max(1);
    let axis = |v: f32| (v.clamp(-1.0, 1.0) * 127.0).round() as i8;
    let total = (p.pending + p.carry).min(250.0);
    let msec = total.floor();
    p.carry = total - msec;
    p.pending = 0.0;
    let command = InputCommand {
        sequence: p.sequence,
        forward: axis(mv.forward),
        right: axis(mv.right),
        yaw: wrap(view.yaw),
        pitch: view.pitch.clamp(-1.55, 1.55),
        buttons: std::mem::take(&mut p.held),
        stance: mv.stance as u8,
        msec: msec as u8,
        // What we're showing of everyone else: the host aims our shots
        // there ([`super::lagcomp`]).
        view_tick: state.clock.map_or(0, |c| c.max(0.0) as u32),
    };
    p.closing = Some(command);
    p.recent.push(command);
    let excess = p.recent.len().saturating_sub(INPUT_REDUNDANCY);
    p.recent.drain(..excess);
    p.sent += 1;
    online.send_inputs(p.recent.iter().rev().copied().collect());
}

/// Our soldier this frame: put right by the host's newest word (if any),
/// then moved by this frame's controls.
#[allow(clippy::type_complexity)]
pub(crate) fn predict_own(
    mut commands: Commands,
    time: Res<Time>,
    state: Res<GuestState>,
    mut p: ResMut<Prediction>,
    world: MoveWorld,
    mut me: Query<
        (Entity, &mut Transform, &mut Mover, &MoveInput, &ViewAngles, Has<Frozen>, Has<Dead>, Has<crate::cover::InCover>, Has<NetDriven>),
        With<LocalPlayer>,
    >,
    mut landed: MessageWriter<crate::movement::Landed>,
) {
    let Ok((e, mut tf, mut mover, input, view, frozen, dead, in_cover, driven)) = me.single_mut() else { return };
    if !driven {
        commands.entity(e).insert(NetDriven);
    }
    let p = &mut *p;
    let now = time.elapsed_secs();
    let dt = time.delta_secs().min(0.1);
    let host = state.my_id.and_then(|id| state.latest.as_ref().map(|s| (s, s.pawns.iter().find(|p| p.id == id))));
    let host_frozen = host.and_then(|(_, p)| p).is_some_and(|p| p.flags & pawn_flags::FROZEN != 0);
    let still = frozen || dead || host_frozen;

    // The host's word, once per snapshot.
    if let Some((snap, Some(hp))) = host
        && p.reconciled != Some(snap.tick)
    {
        p.reconciled = Some(snap.tick);
        let ack = snap.acknowledged_input;
        if let Some(i) = p.history.iter().position(|e| e.sequence == ack) {
            // The round trip: sent to acknowledged, less the time it sat
            // on the host waiting for a snapshot.
            let rtt = (now - p.history[i].sent_at - snap.ack_age as f32 / 1000.0).max(0.0);
            p.rtt = Some(p.rtt.map_or(rtt, |r| r + (rtt - r) * 0.1));
            if !dead && !in_cover && hp.life == 0 {
                let mut pos = Vec3::from_array(hp.position);
                let mut m = p.history[i].mover.clone();
                m.velocity = Vec3::from_array(hp.velocity);
                for e in p.history.iter().skip(i + 1) {
                    if !e.frozen {
                        world.run(&mut pos, &mut m, &e.input, &e.view, e.msec / 1000.0);
                    }
                }
                // The frames since the last command.
                if !still && p.since_command > 0.0 {
                    world.run(&mut pos, &mut m, input, view, p.since_command / 1000.0);
                }
                let error = pos - tf.translation;
                p.error = error.length();
                // (Its own count of steps: the replay's may differ by one.)
                let mut m = m;
                m.steps_total = mover.steps_total;
                if p.error > SNAP {
                    tf.translation = pos;
                    *mover = m;
                    p.snaps += 1;
                    debug!("netplay: predicted {:.2} m off: snapped", p.error);
                } else if p.error > NOISE {
                    tf.translation += error * EASE;
                    *mover = m;
                    p.corrections += 1;
                }
            }
            p.history.drain(..=i);
        } else if p.history.front().is_some_and(|e| cod4rw_multiplayer::security::newer(ack, e.sequence)) {
            // (Acknowledged past what we kept: start afresh.)
            p.history.retain(|e| cod4rw_multiplayer::security::newer(e.sequence, ack));
        }
    }

    // This frame.
    if !still {
        let mut pos = tf.translation;
        if let Some(h) = world.run(&mut pos, &mut mover, input, view, dt) {
            landed.write(crate::movement::Landed { entity: e, fall_height: h });
        }
        tf.translation = pos;
    }
    tf.rotation = Quat::from_rotation_y(view.yaw);
    match p.closing.take() {
        Some(c) => {
            p.history.push_back(Entry {
                sequence: c.sequence,
                // This frame's controls as the host gets them (rounded the
                // same; after any holding still for a plant or cover).
                input: MoveInput { forward: (input.forward.clamp(-1.0, 1.0) * 127.0).round() / 127.0, right: (input.right.clamp(-1.0, 1.0) * 127.0).round() / 127.0, ..*input },
                view: ViewAngles { yaw: c.yaw, pitch: c.pitch },
                msec: c.msec as f32,
                frozen: still,
                mover: mover.clone(),
                sent_at: now,
            });
            while p.history.len() > KEPT {
                p.history.pop_front();
            }
            p.since_command = 0.0;
        }
        None if !still => p.since_command += dt * 1000.0,
        None => {}
    }
}
