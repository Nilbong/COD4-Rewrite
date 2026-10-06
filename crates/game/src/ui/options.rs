//! Rows this game adds to Options > Game (`options_game`), below CoD4's own:
//! the sniper scope's style ([`super::scope`]), the film's tint, the
//! lighting (baked or ray traced, [`crate::rtgi`]) and ragdolls
//! ([`crate::ragdoll`]). Each is
//! made from CoD4's Yes/No row there (`monkeytoy`'s label and choice).

use super::Frontend;
use bevy::prelude::*;
use iw3::menu::{ItemData, Menu};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// Whether the map's film keeps its colour (`r_filmtint`: `off`, the
/// default, or `on`). Off, the film keeps its contrast, brightness and
/// desaturation but tints grey ([`crate::vision`]): Killhouse's, for one,
/// turns everything yellow.
pub const FILM_TINT_DVAR: &str = "r_filmtint";

static FILM_TINT: AtomicBool = AtomicBool::new(false);

/// The map's film keeps its tint (the setting).
pub fn film_tint() -> bool {
    FILM_TINT.load(Ordering::Relaxed)
}

/// The map's lighting (`r_lighting`: `baked`, the default, `rt_low` or
/// `rt_high`; see [`crate::rtgi`]). Takes effect from the next match.
pub const LIGHTING_DVAR: &str = "r_lighting";

static LIGHTING: AtomicU8 = AtomicU8::new(0);

/// The lighting the setting asks for.
pub fn lighting() -> crate::rtgi::Lighting {
    use crate::rtgi::Lighting;
    // `COD4RW_LIGHTING` holds from the start, before the menus are up (a
    // run straight into a map loads it first).
    let from_env = std::env::var("COD4RW_LIGHTING").ok().map(|v| match v.to_ascii_lowercase().as_str() {
        "rt_low" => 1,
        "rt_high" => 2,
        _ => 0,
    });
    match from_env.unwrap_or_else(|| LIGHTING.load(Ordering::Relaxed)) {
        1 => Lighting::RayTracedLow,
        2 => Lighting::RayTracedHigh,
        _ => Lighting::Baked,
    }
}

pub(super) fn build(app: &mut App) {
    app.add_systems(Update, sync.run_if(resource_exists::<Frontend>));
}

fn sync(mut fe: ResMut<Frontend>, mut from_env: Local<bool>, mut settings: ResMut<crate::settings::Settings>) {
    if !std::mem::replace(&mut *from_env, true) {
        if let Ok(tint) = std::env::var("COD4RW_FILMTINT") {
            fe.set_dvar(FILM_TINT_DVAR, &tint);
        }
        if let Ok(lighting) = std::env::var("COD4RW_LIGHTING") {
            fe.set_dvar(LIGHTING_DVAR, &lighting);
        }
    }
    let lighting = match fe.dvars.get(LIGHTING_DVAR).map(|v| v.to_ascii_lowercase()) {
        Some(v) if v == "rt_low" => 1,
        Some(v) if v == "rt_high" => 2,
        _ => 0,
    };
    LIGHTING.store(lighting, Ordering::Relaxed);
    crate::ragdoll::set_enabled(fe.dvars.get(crate::ragdoll::RAGDOLL_DVAR).is_none_or(|v| v != "0"));
    FILM_TINT.store(fe.dvars.get(FILM_TINT_DVAR).is_some_and(|v| v.eq_ignore_ascii_case("on")), Ordering::Relaxed);
    // The settings for the game, when they change.
    let mut fresh = settings.clone();
    if fresh.update(|d| fe.dvars.get(&d.to_ascii_lowercase()).cloned()) {
        *settings = fresh;
    }
}

/// Build the settings pages ([`super::settings_menu`]).
pub(super) fn add(menus: &mut HashMap<String, Arc<Menu>>) {
    super::settings_menu::build(menus);
    // Ray tracing needs a GPU with ray queries; without one Lighting is
    // Baked only.
    if !crate::rtgi::supported() {
        if let Some(menu) = menus.get_mut("options_graphics") {
            for it in &mut Arc::make_mut(menu).items {
                if it.dvar.eq_ignore_ascii_case(LIGHTING_DVAR) {
                    if let ItemData::Multi(m) = &mut it.data {
                        m.labels.truncate(1);
                        m.strings.truncate(1);
                        m.values.truncate(1);
                    }
                }
            }
        }
    }
}

