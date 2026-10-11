//! The new UI (work in progress): a modern design system for the menus and
//! the HUD, drawn through the menus' own draw ops. [`theme`] holds the design
//! tokens, [`font`] the type, [`kit`] the primitives and components.
//!
//! For now only mock-ups: `COD4RW_UINEXT=home|lobby|cac` paints that screen
//! over the frontend and `COD4RW_UINEXT=hud` the HUD in a match (with
//! `COD4RW_UISHOT` for screenshots).

// The kit and theme are a library for the screens still to come.
#[allow(dead_code)]
pub mod cac;
pub mod camo_edit;
pub mod character;
pub mod drops;
pub mod color;
#[allow(dead_code)]
pub mod font;
pub mod home;
pub mod lobby;
pub mod maps;
pub mod match_menus;
pub mod modes;
#[allow(dead_code)]
pub mod kit;
mod mock;
pub mod picker;
pub mod popup;
pub mod profiles;
pub mod settings;
pub mod sight;
#[allow(dead_code)]
pub(in crate::ui) mod shell;
#[allow(dead_code)]
pub mod theme;

use super::{Frontend, Op};
use crate::state::GameState;
use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

static PLAYSTATION: AtomicBool = AtomicBool::new(false);
static IN_FRONTEND: AtomicBool = AtomicBool::new(false);

pub(super) fn build(app: &mut App) {
    app.add_systems(Update, (font::make_wanted, textures, track));
    settings::build(app);
    if std::env::var_os("COD4RW_UIDUMP").is_some() {
        app.add_systems(Update, dump_menus);
    }
}

/// Debug aid: with `COD4RW_UIDUMP`, log each menu as it comes on top (as
/// dressed): its items' labels, rects, visibility and actions.
fn dump_menus(fe: Option<Res<Frontend>>, mut last: Local<String>) {
    let Some(fe) = fe else { return };
    let Some(om) = fe.stack.last() else { return };
    let key = format!("{}#{}", om.name, om.menu.items.len());
    if *last == key {
        return;
    }
    *last = key;
    info!("UIDUMP menu {} (window {}) onOpen {:?} onEsc {:?}", om.name, om.menu.window.name, om.menu.on_open, om.menu.on_esc);
    for (i, it) in om.menu.items.iter().enumerate() {
        let raw = if it.text_exp.is_empty() { it.text.clone() } else { super::expr::eval(&it.text_exp, &*fe).text() };
        let r = it.window.rect;
        info!(
            "UIDUMP {i:3} ty {:2} {:?} [{:.0},{:.0} {:.0}x{:.0} a{}/{}] text {:?} shown {} action {:?} focus {:?}",
            it.ty, it.window.name, r.x, r.y, r.w, r.h, r.horz_align, r.vert_align, fe.assets.localize(&raw), fe.item_visible(om, i), it.action, it.on_focus
        );
        if !it.visible_exp.is_empty() {
            info!("UIDUMP     visible {:?} dvar_flags {} dvar_test {:?}", it.visible_exp, it.dvar_flags, it.dvar_test);
        }
    }
}

/// Whether the pad in use is a PlayStation one (its glyphs).
pub fn pad_is_playstation() -> bool {
    PLAYSTATION.load(Ordering::Relaxed)
}

fn track(active: Option<Res<crate::gamepad::ActiveDevice>>, state: Option<Res<State<GameState>>>) {
    PLAYSTATION.store(active.is_some_and(|a| a.pad == Some(crate::gamepad::PadKind::PlayStation)), Ordering::Relaxed);
    IN_FRONTEND.store(state.is_some_and(|s| *s.get() == GameState::Frontend), Ordering::Relaxed);
}

fn textures(fe: Option<ResMut<Frontend>>, mut images: ResMut<Assets<Image>>, mut done: Local<bool>) {
    let Some(mut fe) = fe else { return };
    if std::mem::replace(&mut *done, true) {
        return;
    }
    for (name, image) in kit::textures(&mut images) {
        fe.assets.add_image(name, image);
    }
}

/// The new UI's own pictures (assets/ui/next), embedded, as UI materials
/// named `next:<file>`.
pub(super) fn picture(name: &str) -> Option<&'static [u8]> {
    macro_rules! pictures {
        ($($file:literal),* $(,)?) => {
            match name.strip_prefix("next:")? {
                $($file => Some(include_bytes!(concat!("../../../assets/ui/next/", $file, ".png")).as_slice()),)*
                _ => None,
            }
        };
    }
    pictures!(
        "preview_1",
        "preview_campaign",
        "preview_headquarters",
        "preview_private_match",
        "preview_join_game",
        "preview_create_a_class",
        "preview_character",
        "preview_supply_drops",
        "preview_combat_record",
        "preview_system",
        "bg_loadout",
        "bg_barracks",
        "bg_lobby",
        "icons_menu",
        "icons_hud",
        "panels/row_focus",
        "panels/row_plain",
        "panels/card_sas",
        "panels/card_map",
        "panels/card_world",
        "panels/card_grid",
        "panels/divider",
        "panels/box_plus",
        "panels/box_brackets",
        "panels/frame_big",
        "panels/panel_big",
        "panels/row_accent",
        "panels/footer_tab",
        "panels/progress",
        "panels/header",
        "buttons2/xbox_a",
        "buttons2/xbox_b",
        "buttons2/xbox_x",
        "buttons2/xbox_y",
        "buttons2/xbox_lb",
        "buttons2/xbox_rb",
        "buttons2/xbox_lt",
        "buttons2/xbox_rt",
        "buttons2/xbox_ls",
        "buttons2/xbox_rs",
        "buttons2/xbox_menu",
        "buttons2/xbox_view",
        "buttons2/xbox_dpad_up",
        "buttons2/xbox_dpad_down",
        "buttons2/xbox_dpad_left",
        "buttons2/xbox_dpad_right",
        "buttons2/playstation_cross",
        "buttons2/playstation_circle",
        "buttons2/playstation_square",
        "buttons2/playstation_triangle",
        "buttons2/playstation_l1",
        "buttons2/playstation_r1",
        "buttons2/playstation_l2",
        "buttons2/playstation_r2",
        "buttons2/playstation_l3",
        "buttons2/playstation_r3",
        "buttons2/playstation_options",
        "buttons2/playstation_create",
        "buttons2/playstation_dpad_up",
        "buttons2/playstation_dpad_down",
        "buttons2/playstation_dpad_left",
        "buttons2/playstation_dpad_right",
        "panels/btn_chevron",
        "panels/row_tick",
        "panels/row_glow",
        "panels/box_oct",
        "panels/box_square",
        "panels/reticle",
        "badge_new",
        "logo",
        "cursors/arrow",
        "cursors/arrow_select",
        "cursors/hand",
        "cursors/busy",
        "cursors/move",
        "cursors/text",
    )
}

/// A cursor of the user's pack: its size in pixels and hotspot.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum Cursor {
    Arrow,
    Hand,
    Busy,
    Move,
    Text,
}

impl Cursor {
    fn art(self) -> (&'static str, Vec2, Vec2) {
        match self {
            Cursor::Arrow => ("next:cursors/arrow", Vec2::new(177.0, 230.0), Vec2::new(37.0, 28.0)),
            Cursor::Hand => ("next:cursors/hand", Vec2::new(165.0, 210.0), Vec2::new(70.0, 27.0)),
            Cursor::Busy => ("next:cursors/busy", Vec2::new(200.0, 200.0), Vec2::new(25.0, 22.0)),
            Cursor::Move => ("next:cursors/move", Vec2::new(195.0, 200.0), Vec2::new(100.0, 98.0)),
            Cursor::Text => ("next:cursors/text", Vec2::new(95.0, 170.0), Vec2::new(47.0, 85.0)),
        }
    }
}

/// The new UI's mouse cursor at `p` (window `h` pixels tall); false when
/// the classic one is wanted.
pub(super) fn cursor(fe: &Frontend, p: Vec2, h: f32, ops: &mut Vec<Op>) -> bool {
    if mock_full().is_none() && !home::on_top(fe) && !cac::on_top(fe) && !picker::on_top(fe) && !camo_edit::on_top(fe) && !color::on_top(fe) && !character::on_top(fe) && !lobby::on_top(fe) && !maps::on_top(fe) && !modes::on_top(fe) && !popup::on_top(fe) && !drops::on_top(fe) {
        return false;
    }
    // A hand over something to click.
    let top = fe.stack.last().map(|m| m.name.as_str());
    let over = fe.focus.as_ref().is_some_and(|(m, _)| Some(m.as_str()) == top) && !fe.pad;
    let (material, size, hot) = if over { Cursor::Hand.art() } else { Cursor::Arrow.art() };
    // The arrow glyph about 18 pixels tall at 1080p (its glow around it).
    let k = 26.0 * h / 1080.0 / 230.0;
    ops.push(Op::Pic { pos: (p - hot * k).round(), size: size * k, material: material.into(), color: [1.0; 4] });
    true
}

/// Which mock-up, and an optional focused row (`home:Headquarters`).
pub(super) fn mock_focus() -> Option<&'static str> {
    mock_full()?.split_once(':').map(|(_, f)| f)
}

fn mock_full() -> Option<&'static str> {
    static M: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::env::var("COD4RW_UINEXT").ok().filter(|s| !s.is_empty())).as_deref()
}

/// The mock-up asked for, if any.
fn mock_screen() -> Option<&'static str> {
    mock_full().map(|s| s.split(':').next().unwrap_or(s))
}

/// Whether the new HUD replaces CoD4's (mock-up only for now).
pub(super) fn hud_replaced() -> bool {
    mock_screen() == Some("hud")
}

impl Frontend {
    /// Over the frontend's menus: the new UI's screen (mock-ups for now).
    pub(super) fn paint_next(&self, w: f32, h: f32, ops: &mut Vec<Op>) {
        if !IN_FRONTEND.load(Ordering::Relaxed) {
            return;
        }
        let Some(screen) = mock_screen() else { return };
        let mut k = kit::Kit::new(self, ops, kit::View::new(Vec2::ZERO, Vec2::new(w, h)));
        match screen {
            "lobby" => mock::lobby(&mut k),
            "cac" => mock::cac(&mut k),
            _ => {}
        }
    }

    /// The new HUD over a match (mock-up for now).
    pub(super) fn paint_next_hud(&self, w: f32, h: f32, minimap: Option<(Vec2, Vec2)>, ops: &mut Vec<Op>) {
        if !hud_replaced() {
            return;
        }
        let mut k = kit::Kit::new(self, ops, kit::View::new(Vec2::ZERO, Vec2::new(w, h)));
        mock::hud(&mut k, h, minimap);
    }
}
