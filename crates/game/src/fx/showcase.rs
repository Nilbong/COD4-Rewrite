//! The showcase maps' effects upgrades (wherever [`crate::atmos::climate`]'s
//! showcase runs), each with its `fx.*` knob in `tuning.txt`:
//! - combat smoke and dust (smoke grenades, impacts, explosions) lit like
//!   the map's ambient effects, and every lit puff shaded across itself: its
//!   sun side bright, its far side in its own shade, nearby lights (muzzle
//!   flashes, explosions) warming the side facing them;
//! - a brief light for every shot, lighting the room and the gun;
//! - small puffs of smoke left in the world at the muzzle during and after
//!   a burst (`fx.wisp 1`; off by default);
//! - spent cases that bounce and rattle (CoD4's `shell_eject_*`) before
//!   they settle, then lie about for a while.
//!
//! `COD4RW_FXSHOWCASE=0|1` forces them off or on (for A/B timing).

use crate::tune::get;

/// The upgrades are on: a showcase map (or forced on), and not switched off.
pub(super) fn on() -> bool {
    match std::env::var("COD4RW_FXSHOWCASE").ok().as_deref() {
        Some("0") => false,
        Some(_) => true,
        None => crate::atmos::climate::showcase() && get("fx.showcase", 1.0) > 0.5,
    }
}

/// How much combat effects are lit (1 fully; 0 their fixed brightness).
pub(super) fn lit_combat() -> f32 {
    get("fx.lit_combat", 1.0).clamp(0.0, 1.0)
}

/// Lit smoke's gain over the light grid, and the least it's lit (of its
/// fixed brightness).
pub(super) fn lit_gain() -> f32 {
    get("fx.lit_gain", 1.8).max(0.0)
}
pub(super) fn lit_floor() -> f32 {
    get("fx.lit_floor", 0.4).clamp(0.0, 1.0)
}

/// How strongly a puff is shaded across itself (0 flat, 1 a ball's).
pub(super) fn shading() -> f32 {
    // (Off by default: shading each sprite's corners left dark rims where
    // the sprites of a cloud overlap.)
    get("fx.shading", 0.0).clamp(0.0, 1.0)
}

/// How much fx and muzzle lights light the smoke.
pub(super) fn smoke_from_lights() -> f32 {
    get("fx.smoke_lights", 1.0).max(0.0)
}

/// A shot's light: on, how long (ms), how far (CoD units), how bright.
pub(super) fn muzzle_light() -> Option<(f64, f32, f32)> {
    (get("fx.muzzle_light", 1.0) > 0.0).then(|| {
        (get("fx.muzzle_light_ms", 60.0).max(10.0) as f64, get("fx.muzzle_light_radius", 220.0).max(16.0), get("fx.muzzle_light", 1.0))
    })
}

/// How many fx lights at once (the nearest).
pub(super) fn max_lights(default: usize) -> usize {
    if on() { get("fx.max_lights", 8.0).clamp(0.0, 32.0) as usize } else { default }
}

/// After a burst of at least this many shots, a wisp of this size (of the
/// effect's own) and greyness.
pub(super) fn wisp() -> Option<(u32, f32, f32)> {
    // (Off by default: CoD4's thin smoke at this size is gone in under a
    // second and hard to tell from the flash's own smoke.)
    (get("fx.wisp", 0.0) > 0.0).then(|| (get("fx.wisp_shots", 3.0).max(1.0) as u32, get("fx.wisp_size", 0.12).max(0.01), get("fx.wisp_grey", 0.8).max(0.0)))
}

/// The effect a muzzle's wisp is made of.
pub(super) const WISP: &str = "smoke/thin_light_smoke_m";

/// Spent cases: bounces (most), how much of their speed a bounce keeps (up,
/// along), and how long a resting one stays (ms).
pub(super) fn case_bounces() -> u8 {
    get("fx.case_bounces", 3.0).clamp(0.0, 8.0) as u8
}
pub(super) fn case_restitution() -> (f32, f32) {
    (get("fx.case_bounce", 0.35).clamp(0.0, 1.0), get("fx.case_slide", 0.55).clamp(0.0, 1.0))
}
pub(super) fn case_linger_ms() -> f32 {
    get("fx.case_linger", 20.0).max(0.0) * 1000.0
}
