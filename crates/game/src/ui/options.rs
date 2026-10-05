//! Rows this game adds to Options > Game (`options_game`), below CoD4's own:
//! the sniper scope's style ([`super::scope`]), the film's tint and the
//! lighting (baked or ray traced, [`crate::rtgi`]). Each is
//! made from CoD4's Yes/No row there (`monkeytoy`'s label and choice).

use super::Frontend;
use bevy::prelude::*;
use iw3::menu::{ItemData, Menu, Multi, item_type};
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
    match LIGHTING.load(Ordering::Relaxed) {
        1 => Lighting::RayTracedLow,
        2 => Lighting::RayTracedHigh,
        _ => Lighting::Baked,
    }
}

pub(super) fn build(app: &mut App) {
    app.add_systems(Update, sync.run_if(resource_exists::<Frontend>));
}

fn sync(mut fe: ResMut<Frontend>, mut from_env: Local<bool>) {
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
    FILM_TINT.store(fe.dvars.get(FILM_TINT_DVAR).is_some_and(|v| v.eq_ignore_ascii_case("on")), Ordering::Relaxed);
}

/// Add the rows.
pub(super) fn add(menus: &mut HashMap<String, Arc<Menu>>) {
    add_row(menus, super::scope::SCOPE_STYLE_DVAR, "Sniper Scope", &[("Classic", "classic"), ("Lens", "lens")]);
    add_row(menus, FILM_TINT_DVAR, "Film Tint", &[("Off", "off"), ("On", "on")]);
    // Ray tracing needs a GPU with ray queries; without one the row says so.
    if crate::rtgi::supported() {
        add_row(menus, LIGHTING_DVAR, "Lighting", &[("Baked", "baked"), ("Ray Traced Low", "rt_low"), ("Ray Traced High", "rt_high")]);
    } else {
        add_row(menus, LIGHTING_DVAR, "Lighting (no ray tracing GPU)", &[("Baked", "baked")]);
    }
}

/// A row below Options > Game's last: `label`, and a choice of `dvar`'s
/// values (shown, set).
fn add_row(menus: &mut HashMap<String, Arc<Menu>>, dvar: &str, label: &str, choices: &[(&str, &str)]) {
    let Some(menu) = menus.get_mut("options_game") else { return };
    let menu = Arc::make_mut(menu);
    let Some(choice) = menu.items.iter().position(|it| it.dvar.eq_ignore_ascii_case("monkeytoy")) else { return };
    let row_y = menu.items[choice].window.rect.y;
    let label_item = menu.items.iter().position(|it| it.window.rect.y == row_y && it.ty == item_type::BUTTON && !it.text_exp.is_empty());
    let last = menu.items.iter().filter(|it| !it.dvar.is_empty()).map(|it| it.window.rect.y).fold(row_y, f32::max);
    let y = last + 22.0;
    let mut value = menu.items[choice].clone();
    value.window.rect.y = y;
    value.dvar = dvar.into();
    value.data = ItemData::Multi(Multi {
        labels: choices.iter().map(|(l, _)| (*l).into()).collect(),
        strings: choices.iter().map(|(_, v)| (*v).into()).collect(),
        values: (0..choices.len()).map(|i| i as f32).collect(),
        str_def: true,
    });
    if let Some(i) = label_item {
        let mut item = menu.items[i].clone();
        item.window.rect.y = y;
        item.text_exp.clear();
        item.text = label.into();
        menu.items.push(item);
    }
    menu.items.push(value);
}
