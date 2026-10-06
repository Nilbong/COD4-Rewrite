//! CoD4's perks that do more than change a weapon's numbers (those are in
//! [`crate::loadout`]; Deep Impact is in [`crate::weapons`]' penetration,
//! Martyrdom in [`crate::grenades`], Sonic Boom where explosives go off):
//!
//! - Last Stand (`specialty_pistoldeath`, `_globallogic.gsc`): a bullet that
//!   would kill (not a headshot), or a fall, downs instead. Downed, the pawn
//!   lies prone with its pistol (a Beretta if it has none) and all its
//!   ammo, no grenades, one hit from death, for 10 s (`lastStandTimer( 10 )`),
//!   then bleeds out to whoever downed it; holding F 0.7 s ends it sooner
//!   (`lastStandAllowSuicide`).
//! - Iron Lungs (`specialty_holdbreath`): scoped, the view sways by the
//!   weapon's `adsIdleAmount`; Shift holds the breath to steady it for
//!   `player_breath_hold_time` (4.5 s), 5 s longer with the perk
//!   (`perk_extraBreath`), after which the shooter gasps (sway ×4.5,
//!   `player_breath_gasp_scale`) for `player_breath_gasp_time` (1 s).

use crate::combat::{Damage, Dead, HitLocation, Health};
use crate::content::Content;
use crate::loadout::Loadout;
use crate::movement::{MoveInput, Mover, Stance, ViewAngles};
use crate::player::LocalPlayer;
use crate::weapons::WeaponState;
use bevy::prelude::*;
use std::collections::HashMap;


pub struct PerksPlugin;

impl Plugin for PerksPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                (go_down, bleed_out).chain().after(crate::weapons::WeaponSet),
                downed_controls.after(crate::player::InputSet).before(crate::movement::MovementSet),
                breath.after(crate::player::InputSet).before(crate::weapons::WeaponSet),
            )
                .run_if(crate::state::in_game),
        );
        if let Ok(dir) = std::env::var("COD4RW_LASTSTANDTEST") {
            app.insert_resource(TestDir(dir.into())).add_systems(Update, test.run_if(crate::state::in_game));
        }
    }
}

/// Does `loadout` have `perk`?
pub fn has(loadout: Option<&Loadout>, perk: &str) -> bool {
    loadout.is_some_and(|l| l.class.perks.iter().any(|p| p.eq_ignore_ascii_case(perk)))
}

/// `mayDoLastStand`: only bullets (`MOD_PISTOL_BULLET`, `MOD_RIFLE_BULLET`;
/// not to the head) and falls down a Last Stand pawn; explosives, grenades,
/// kill streaks, the knife and the bomb kill outright.
pub fn may_go_down(loadout: Option<&Loadout>, weapon: &str, location: HitLocation) -> bool {
    if !has(loadout, "specialty_pistoldeath") || location == HitLocation::Head {
        return false;
    }
    let not_a_bullet = crate::grenades::kill_icon(weapon).is_some()
        || crate::killstreaks::kill_icon(weapon).is_some()
        || crate::explosives::is_explosive(weapon)
        || weapon == crate::melee::WEAPON
        || weapon == "briefcase_bomb_mp";
    weapon == "falling" || !not_a_bullet
}

/// Downed in Last Stand.
#[derive(Component, Debug)]
pub struct Downed {
    pub until: f32,
    /// Who downed it and with what: credited when it bleeds out.
    pub attacker: Option<Entity>,
    pub weapon: &'static str,
    /// When F started being held (the coward's way out).
    held_use: Option<f32>,
}

/// `lastStandTimer( 10 )`, and how long F ends it.
pub const LAST_STAND_TIME: f32 = 10.0;
const COWARDS_WAY_OUT: f32 = 0.7;

impl Downed {
    pub fn new(now: f32, attacker: Option<Entity>, weapon: &'static str) -> Downed {
        Downed { until: now + LAST_STAND_TIME, attacker, weapon, held_use: None }
    }

    /// Seconds since going down, at `now`.
    pub fn since(&self, now: f32) -> f32 {
        now - (self.until - LAST_STAND_TIME)
    }
}

/// Just downed: the pistol out with all its ammo, no grenades, and the
/// "Last Stand!" sound.
#[allow(clippy::type_complexity)]
fn go_down(
    mut commands: Commands,
    time: Res<Time>,
    content: Res<Content>,
    mut downed: Query<
        (Entity, &mut WeaponState, Option<&mut Loadout>, Option<&mut crate::grenades::Grenades>, Has<LocalPlayer>),
        Added<Downed>,
    >,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let now = time.elapsed_secs();
    for (e, mut weapon, loadout, grenades, local) in &mut downed {
        if let Some(mut l) = loadout {
            l.take_pistol(&mut weapon, &content, now);
        }
        if let Some(mut g) = grenades {
            g.frags = 0;
            g.specials = 0;
        }
        commands.entity(e).remove::<crate::grenades::Offhand>();
        if local {
            sfx.play("mp_last_stand", None);
            info!("last stand: {LAST_STAND_TIME}s to live");
        }
    }
}

/// Downed pawns stay one hit from death, and bleed out (or take the
/// coward's way out); the dead get up as themselves.
fn bleed_out(
    mut commands: Commands,
    time: Res<Time>,
    mut downed: Query<(Entity, &mut Downed, &mut Health, Option<&crate::splitscreen::PlayerInput>, Has<Dead>)>,
    mut damage: MessageWriter<Damage>,
) {
    let now = time.elapsed_secs();
    for (e, mut d, mut health, local, dead) in &mut downed {
        if dead {
            commands.entity(e).remove::<Downed>();
            continue;
        }
        health.current = health.current.min(1.0);
        // [Use]: F, or the pad's X / Square.
        let pressing = local.is_some_and(|p| p.live && (p.keys.pressed(KeyCode::KeyF) || p.pad.interact));
        d.held_use = if pressing { Some(d.held_use.unwrap_or(now)) } else { None };
        let gave_up = d.held_use.is_some_and(|t| now - t > COWARDS_WAY_OUT);
        if now >= d.until || gave_up {
            damage.write(Damage { target: e, attacker: d.attacker.or(Some(e)), amount: 1000.0, location: HitLocation::Torso, weapon: d.weapon });
        }
    }
}

/// Downed, a pawn lies where it fell: it can't move at all
/// (`PM_DeadMove` zeroes the moves), only turn and shoot.
fn downed_controls(mut pawns: Query<&mut MoveInput, With<Downed>>) {
    for mut mv in &mut pawns {
        mv.stance = Stance::Prone;
        (mv.forward, mv.right) = (0.0, 0.0);
        mv.jump = false;
        mv.sprint = false;
    }
}

/// `player_breath_hold_time`, `perk_extraBreath`, `player_breath_gasp_time`,
/// `player_breath_gasp_scale`, and how fast the sway eases between them
/// (`player_breath_hold_lerp`, `player_breath_gasp_lerp`).
const BREATH_HOLD: f32 = 4.5;
const IRON_LUNGS: f32 = 5.0;
const BREATH_GASP: f32 = 1.0;
const GASP_SCALE: f32 = 4.5;
const HOLD_LERP: f32 = 1.0;
const GASP_LERP: f32 = 6.0;

/// The player's breath while scoped.
#[derive(Default)]
struct Breath {
    /// Seconds held so far, and gasping until when.
    held: f32,
    gasp_until: f32,
    holding: bool,
    /// The sway's scale (1 normal, 0 held, more gasping) and clock (ms).
    scale: f32,
    clock: f32,
    /// The sway last applied to the view (yaw, pitch, radians).
    applied: Vec2,
}

/// Scoped sway, and holding the breath to steady it: the sway moves the
/// view itself (the aim with it), applied as the change since last frame
/// so the player's own aim is kept.
fn breath(
    time: Res<Time>,
    mut players: Query<(Entity, &crate::splitscreen::PlayerInput, &WeaponState, &Mover, &mut ViewAngles, Option<&Loadout>, Has<Dead>)>,
    mut breaths: Local<HashMap<Entity, Breath>>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    breaths.retain(|e, _| players.contains(*e));
    for (e, input, w, mover, mut view, loadout, dead) in &mut players {
        let b = breaths.entry(e).or_default();
        breath_one(&time, input, (w, mover, &mut view, loadout, dead), b, &mut sfx);
    }
}

/// One player's scoped sway and breath: Shift (a pad's L3) holds it.
fn breath_one(
    time: &Time,
    input: &crate::splitscreen::PlayerInput,
    (w, mover, view, loadout, dead): (&WeaponState, &Mover, &mut ViewAngles, Option<&Loadout>, bool),
    b: &mut Breath,
    sfx: &mut crate::audio::Sfx,
) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    let scoped = w.def.ads_overlay.is_some() && w.ads > 0.0 && !dead;
    if !scoped {
        view.yaw -= b.applied.x;
        view.pitch -= b.applied.y;
        *b = Breath { scale: 1.0, ..default() };
        return;
    }
    let hold_time = BREATH_HOLD + if has(loadout, "specialty_holdbreath") { IRON_LUNGS } else { 0.0 };
    let want = (input.keys.pressed(KeyCode::ShiftLeft) || input.pad.breath) && input.live && now >= b.gasp_until;
    if want && b.held < hold_time {
        if !b.holding {
            sfx.play("breathing_hold", None);
        }
        b.holding = true;
        b.held += dt;
    } else if b.holding {
        // Out of breath: a gasp; let go in time: a sigh.
        b.holding = false;
        if b.held >= hold_time {
            b.gasp_until = now + BREATH_GASP;
            sfx.play("breathing_gasp", None);
        } else {
            sfx.play("breathing_better", None);
        }
        b.held = 0.0;
    }
    let (target, rate) = match (b.holding, now < b.gasp_until) {
        (true, _) => (0.0, HOLD_LERP * 4.0),
        (_, true) => (GASP_SCALE, GASP_LERP),
        _ => (1.0, GASP_LERP),
    };
    b.scale += (target - b.scale) * (1.0 - (-rate * dt).exp());
    // `BG_CalculateWeaponAngles`' idle sway, at the ADS amount and speed.
    let d = w.def;
    let stance = match mover.stance {
        Stance::Prone => d.idle_prone_factor,
        Stance::Crouch => d.idle_crouch_factor,
        Stance::Stand => 1.0,
    };
    b.clock += d.ads_idle_speed * dt * 1000.0;
    let amount = d.ads_idle_amount * stance * b.scale * 0.01 * w.ads;
    let sway = Vec2::new((b.clock * 0.0007).sin(), (b.clock * 0.001).sin()) * amount.to_radians();
    view.yaw += sway.x - b.applied.x;
    view.pitch += sway.y - b.applied.y;
    b.applied = sway;
}

#[derive(Resource)]
struct TestDir(std::path::PathBuf);

/// Debug aid: with `COD4RW_LASTSTANDTEST=<dir>` (and a class with Last
/// Stand: `COD4RW_LOADOUT=m16 COD4RW_PERKS=specialty_pistoldeath`), an enemy's
/// bullet would kill the player at 6 s; screenshots downed, and once it has
/// bled out; then exit.
fn test(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<TestDir>,
    player: Query<(Entity, &crate::combat::Pawn, &Mover, &MoveInput, Has<Downed>, Has<Dead>), With<LocalPlayer>>,
    pawns: Query<(Entity, &crate::combat::Pawn)>,
    mut damage: MessageWriter<Damage>,
    mut step: Local<usize>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let t = time.elapsed_secs();
    let Ok((me, pawn, mover, input, downed, dead)) = player.single() else { return };
    let mut shot = |name: &str, step: &mut usize| {
        std::fs::create_dir_all(&dir.0).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.0.join(format!("laststand_{step}_{name}.png"))));
        *step += 1;
    };
    match *step {
        0 if t >= 6.0 => {
            let enemy = pawns.iter().find(|(_, p)| crate::combat::hostile(p, pawn)).map(|(e, _)| e);
            damage.write(Damage { target: me, attacker: enemy, amount: 150.0, location: HitLocation::Torso, weapon: "AK-47" });
            *step += 1;
        }
        1 if t >= 7.5 => {
            info!("last stand test: downed {downed}, dead {dead}, stance {:?} (asked {:?})", mover.stance, input.stance);
            shot("downed", &mut step);
        }
        2 if dead => {
            info!("last stand test: bled out at {t:.1}s");
            shot("bled_out", &mut step);
        }
        3 if t >= 20.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
