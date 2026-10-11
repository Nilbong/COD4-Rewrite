//! The player's hands calling in a hardpoint, as CoD4 shows it: each
//! hardpoint is a weapon (`radar_mp`, `airstrike_mp`, `helicopter_mp`, all
//! three the C4's viewmodel, with its detonator), and `_hardpoints.gsc`'s
//! `hardpointItemWaiter` switches to it, triggers it and switches back to
//! the gun. So: the gun goes down, the detonator comes up and is pressed,
//! goes away, and the gun comes back up. The airstrike's is held up while
//! its spot is picked on the map (`selectAirstrikeLocation` waits for the
//! confirm before switching back), and only pressed once it's picked.
//!
//! Modern Warfare 2's sentry gun is carried out instead ([`super::sentry`]):
//! the gun goes down and the hands are out of sight until it's planted
//! (as Modern Warfare 2 shows it), then the gun comes back up. Its care
//! package's marker is thrown as a grenade is (the grenades' own anims).
//!
//! [`HardpointHands`] on a local player's pawn is what `crate::viewmodel`
//! shows and animates; meanwhile the gun can't fire, aim or reload.

use super::{Hardpoint, StreakNotice, airstrike};
use crate::combat::{Dead, Pawn};
use crate::splitscreen::LocalSlot;
use crate::weapons::{WeaponDef, WeaponInput, WeaponState};
use bevy::prelude::*;

pub(super) fn build(app: &mut App) {
    app.init_resource::<HardpointDefs>()
        .add_systems(OnEnter(crate::state::GameState::InGame), load.after(crate::world::load_map).in_set(crate::state::Setup::Content))
        .add_systems(Update, (start, carry_sentry, advance).chain().after(super::use_hardpoints).run_if(crate::state::in_game))
        .add_systems(Update, hold_gun.after(crate::splitscreen::InputGathered).before(crate::weapons::WeaponSet).run_if(crate::state::in_game));
}

/// The hardpoints' weapon files (their viewmodel and animations).
#[derive(Resource, Default)]
struct HardpointDefs {
    uav: Option<&'static WeaponDef>,
    airstrike: Option<&'static WeaponDef>,
    helicopter: Option<&'static WeaponDef>,
}

impl HardpointDefs {
    fn of(&self, item: Hardpoint) -> Option<&'static WeaponDef> {
        match item {
            Hardpoint::Uav => self.uav,
            Hardpoint::Airstrike => self.airstrike,
            Hardpoint::Helicopter => self.helicopter,
            // The care package's marker is thrown as a grenade, and the
            // sentry is carried out in the hands' place.
            Hardpoint::CarePackage | Hardpoint::Sentry => None,
        }
    }
}

fn load(mut defs: ResMut<HardpointDefs>, content: Res<crate::content::Content>) {
    if defs.uav.is_some() {
        return;
    }
    let def = |name: &str| crate::loadout::bot_weapon(&content, &format!("{name}:")).filter(|d| d.name == format!("{name}_mp"));
    *defs = HardpointDefs { uav: def("radar"), airstrike: def("airstrike"), helicopter: def("helicopter") };
    if defs.uav.is_none() {
        warn!("hardpoint hands: no radar_mp weapon file; calling in shows no hands");
    }
}

/// Where a call-in's hands are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandsPhase {
    /// The gun going down (its drop).
    GunDown,
    /// The detonator coming up (the hardpoint's raise).
    Raise,
    /// Holding it up (the airstrike's, while its spot is picked).
    Hold,
    /// Pressing it.
    Press,
    /// Putting it away (the hardpoint's drop).
    PutAway,
    /// The gun coming back up (its raise).
    GunUp,
    /// No hands in sight (carrying a sentry gun out).
    Away,
}

/// A hardpoint being called in by this local player's hands.
#[derive(Component, Clone, Copy, Debug)]
pub struct HardpointHands {
    /// The hardpoint's weapon (the detonator's viewmodel and anims).
    pub def: &'static WeaponDef,
    pub phase: HandsPhase,
    pub started: f32,
    pub until: f32,
    /// Pressed once it's up (UAV, helicopter; the airstrike's once its
    /// spot is picked), else put away unpressed (the pick called off).
    press: bool,
    /// Waiting on the airstrike's spot.
    selecting: bool,
    /// When its map closed (the call-in can come a frame after).
    closed_at: f32,
    /// Carrying a sentry gun out: away until it's planted.
    carry: bool,
}

impl HardpointHands {
    /// Is the detonator (rather than the gun) in hand?
    pub fn item_in_hand(&self) -> bool {
        matches!(self.phase, HandsPhase::Raise | HandsPhase::Hold | HandsPhase::Press | HandsPhase::PutAway)
    }
}

/// How long after the airstrike's map closes its call-in may still come.
const CALL_GRACE: f32 = 0.2;

/// How long the detonator's press lasts (`iFireTime`, 800 ms, is the time
/// to the next press; the press itself is over in about this).
const PRESS: f32 = 0.6;

/// Start the hands on a call-in (the airstrike's on opening its map).
fn start(
    mut commands: Commands,
    time: Res<Time>,
    defs: Res<HardpointDefs>,
    mut notices: MessageReader<StreakNotice>,
    selecting: Option<Res<airstrike::Selecting>>,
    mut hands: Query<(Entity, &Pawn, &WeaponState, Option<&mut HardpointHands>), (With<LocalSlot>, Without<Dead>)>,
    mut was_selecting: Local<Option<Entity>>,
) {
    let now = time.elapsed_secs();
    let begin = |w: &WeaponState, def: &'static WeaponDef, press: bool, selecting: bool| HardpointHands {
        def,
        phase: HandsPhase::GunDown,
        started: now,
        until: now + w.def.drop_time.max(0.05),
        press,
        selecting,
        closed_at: f32::INFINITY,
        carry: false,
    };
    // The airstrike's map opened: up comes the detonator, held while the
    // spot is picked.
    let owner = selecting.as_ref().map(|s| s.owner);
    if owner != *was_selecting {
        if let (Some(e), Some(def)) = (owner, defs.airstrike) {
            if let Ok((_, _, w, None)) = hands.get_mut(e) {
                commands.entity(e).insert(begin(w, def, false, true));
            }
        }
        // Closed: picked (pressed, below) or called off (put away).
        if let Some(e) = was_selecting.filter(|_| owner.is_none()) {
            if let Ok((_, _, _, Some(mut h))) = hands.get_mut(e) {
                h.selecting = false;
                h.closed_at = now;
            }
        }
        *was_selecting = owner;
    }
    for n in notices.read() {
        let StreakNotice::CalledIn { item, by, .. } = n else { continue };
        let Some(def) = defs.of(*item) else { continue };
        for (e, pawn, w, h) in &mut hands {
            if pawn.name != *by {
                continue;
            }
            match h {
                // The airstrike's spot picked: pressed once it's up.
                Some(mut h) if h.def.name == def.name => {
                    h.press = true;
                    h.selecting = false;
                }
                Some(_) => {}
                None => {
                    commands.entity(e).insert(begin(w, def, true, false));
                }
            }
        }
    }
}

/// A sentry gun picked up to carry out: the gun down and the hands away.
fn carry_sentry(
    mut commands: Commands,
    time: Res<Time>,
    carriers: Query<(Entity, &WeaponState), (With<LocalSlot>, With<super::sentry::Carrying>, Without<HardpointHands>, Without<Dead>)>,
) {
    let now = time.elapsed_secs();
    for (e, w) in &carriers {
        commands.entity(e).insert(HardpointHands {
            def: w.def,
            phase: HandsPhase::GunDown,
            started: now,
            until: now + w.def.drop_time.max(0.05),
            press: false,
            selecting: false,
            closed_at: f32::INFINITY,
            carry: true,
        });
    }
}

/// Each part of the call-in to its time, the next after it.
fn advance(
    mut commands: Commands,
    time: Res<Time>,
    mut hands: Query<(Entity, &WeaponState, &mut HardpointHands, Has<Dead>, Has<super::sentry::Carrying>)>,
) {
    let now = time.elapsed_secs();
    for (e, w, mut h, dead, carrying) in &mut hands {
        if dead {
            commands.entity(e).remove::<HardpointHands>();
            continue;
        }
        let next = match h.phase {
            // (A spot picked is called in a frame or so after the map
            // closes: called off only if nothing came.)
            HandsPhase::Hold if !h.selecting && (h.press || now - h.closed_at > CALL_GRACE) => Some(if h.press { (HandsPhase::Press, PRESS) } else { (HandsPhase::PutAway, h.def.drop_time) }),
            HandsPhase::Hold => None,
            // The sentry planted: the gun back up.
            HandsPhase::Away if !carrying => Some((HandsPhase::GunUp, w.def.raise_time)),
            HandsPhase::Away => None,
            _ if now < h.until => None,
            HandsPhase::GunDown if h.carry => Some((HandsPhase::Away, 0.0)),
            HandsPhase::GunDown => Some((HandsPhase::Raise, h.def.raise_time)),
            HandsPhase::Raise if h.selecting || !h.press => Some((HandsPhase::Hold, 0.0)),
            HandsPhase::Raise => Some((HandsPhase::Press, PRESS)),
            HandsPhase::Press => Some((HandsPhase::PutAway, h.def.drop_time)),
            HandsPhase::PutAway => Some((HandsPhase::GunUp, w.def.raise_time)),
            HandsPhase::GunUp => {
                commands.entity(e).remove::<HardpointHands>();
                None
            }
        };
        if let Some((phase, seconds)) = next {
            h.phase = phase;
            h.started = now;
            h.until = now + seconds.max(0.05);
        }
    }
}

/// No firing, aiming or reloading with the detonator in hand (or the gun
/// on its way down or up).
fn hold_gun(mut pawns: Query<&mut WeaponInput, With<HardpointHands>>) {
    for mut input in &mut pawns {
        input.fire = false;
        input.ads = false;
        input.reload = false;
        input.inspect = false;
    }
}
