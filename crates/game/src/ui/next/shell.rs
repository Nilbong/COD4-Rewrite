//! The new UI's shared screen parts: backdrop, player card, header,
//! footer, section labels, list rows and the main menu's rows and previews.

use super::kit::{Button, Kit, R};
use super::theme::{self as t, Type};
use bevy::prelude::*;

// --- Shared shell -------------------------------------------------------------

/// The painted backdrop, cover-cropped to the window, with the scrims that
/// keep text readable over it.
pub(in crate::ui) fn backdrop(k: &mut Kit, darken: f32) {
    let b = k.v.bleed();
    k.fill(b, t::hex(0x050607, 1.0));
    const ASPECT: f32 = 1672.0 / 941.0;
    let (w, h) = if b.w / b.h > ASPECT { (b.w, b.w / ASPECT) } else { (b.h * ASPECT, b.h) };
    // A slow drift.
    let dx = (k.now * 0.02).sin() * 12.0;
    k.pic(R::new(b.cx() - w * 0.5 - 12.0 + dx, b.cy() - h * 0.5 - 8.0, w + 24.0, h + 16.0), super::super::modern::BACKGROUND, [1.0; 4]);
    k.fill(b, t::with_alpha(t::hex(0x07090a, 1.0), darken));
    // Left and bottom scrims, and a top band under the bar.
    k.grad_h(R::new(b.x, b.y, b.w * 0.62, b.h), t::hex(0x050607, 1.0), 0.88, 0.0);
    k.grad_v(R::new(b.x, b.bottom() - 260.0, b.w, 260.0), t::hex(0x050607, 1.0), 0.0, 0.9);
    k.grad_v(R::new(b.x, b.y, b.w, 200.0), t::hex(0x050607, 1.0), 0.75, 0.0);
}

/// The player card, top right: emblem, name, rank and XP.
pub(in crate::ui) fn player_card(k: &mut Kit) {
    let right = 1920.0 - t::SAFE_X;
    let rank = super::super::combat_record::rank(k.fe);
    k.slice3(R::new(right - 640.0, 34.0, 560.0, 96.0), "next:panels/card_sas", Vec2::new(456.0, 144.0), 140.0, 40.0, [1.0; 4]);
    let e = R::new(right - 64.0, 50.0, 64.0, 64.0);
    k.fill(e, t::hex(0x000000, 0.4));
    k.pic(e.inset(4.0), super::super::modern::EMBLEM_MATERIAL, [1.0; 4]);
    k.frame(e, 1.5, t::LINE);
    let x = e.x - 40.0;
    let name = k.fe.profile_name();
    k.text(x, 52.0, Type { size: 26.0, ..t::H2 }, &name, t::INK, 2);
    // Rank icon, level and rank name.
    let line = format!("{}  ·  LV {}", rank.name.to_uppercase(), rank.level);
    let w = k.text(x, 84.0, t::MICRO, &line, t::INK_DIM, 2);
    if !rank.icon.is_empty() {
        k.pic(R::new(x - w - 30.0, 80.0, 22.0, 22.0), &rank.icon, [1.0; 4]);
    }
    // XP to the next rank.
    let f = rank.next.map_or(1.0, |n| (rank.xp - rank.min) as f32 / (n - rank.min).max(1) as f32);
    let bar = R::new(x - 220.0, 108.0, 220.0, 3.0);
    k.fill(bar, t::hex(0xffffff, 0.14));
    k.fill(R::new(bar.x, bar.y, bar.w * f.clamp(0.0, 1.0), bar.h), t::ACCENT);
}

/// Supply drops waiting (a pill) and the settings cog, left of the card.
pub(in crate::ui) fn top_icons(k: &mut Kit, right: f32) {
    let unopened = crate::supply::inventory().unopened;
    let pill = R::new(right - 112.0, 62.0, 112.0, 40.0);
    k.fill(pill, t::hex(0x000000, 0.4));
    k.frame(pill, 1.5, t::HAIRLINE);
    k.icon("next:icons_menu", 5, R::new(pill.x + 12.0, pill.y + 8.0, 24.0, 24.0), t::REWARD);
    k.text_mid(pill.x + 48.0, pill.cy(), Type { size: 22.0, ..t::H2 }, &unopened.to_string(), t::INK, 0);
    if unopened > 0 {
        k.disc(Vec2::new(pill.right() - 12.0, pill.y + 10.0), 5.0, t::REWARD);
    }
    k.icon("next:icons_menu", 4, R::new(pill.x - 54.0, pill.y + 6.0, 28.0, 28.0), t::INK_DIM);
}


/// A sub-screen's header: where it is, its title, the player.
pub(in crate::ui) fn header(k: &mut Kit, crumb: &str, title: &str) {
    let x = t::SAFE_X;
    k.chevron(Vec2::new(x + 6.0, 64.0), 16.0, 2, 2.0, t::INK_DIM);
    k.text(x + 22.0, 56.0, t::LABEL, crumb, t::INK_DIM, 0);
    k.text(x, 78.0, t::DISPLAY, title, t::INK, 0);
    player_card(k);
    top_icons(k, 1920.0 - t::SAFE_X - 660.0);
    k.hline(t::SAFE_X, 1920.0 - t::SAFE_X, 160.0, 1.0, t::HAIRLINE);
}

pub(in crate::ui) fn footer(k: &mut Kit, note: &str, prompts: &[(Button, &str)]) {
    let cy = t::FOOTER_Y + 20.0;
    k.slice3(R::new(t::SAFE_X, t::FOOTER_Y - 22.0, 1920.0 - 2.0 * t::SAFE_X, 16.0), "next:panels/divider", Vec2::new(354.0, 28.0), 120.0, 120.0, [1.0; 4]);
    if !note.is_empty() {
        k.text_mid(t::SAFE_X, cy, t::CAPTION, note, t::INK_MUTE, 0);
    }
    k.prompts_right(1920.0 - t::SAFE_X, cy, prompts);
}

/// An over-line section label with its rule.
pub(in crate::ui) fn section(k: &mut Kit, x: f32, y: f32, w: f32, name: &str) {
    section_icon(k, x, y, w, name, None);
}

/// A section label with one of the menu icons before it.
pub(in crate::ui) fn section_icon(k: &mut Kit, x: f32, y: f32, w: f32, name: &str, icon: Option<usize>) {
    let x0 = x;
    let x = match icon {
        Some(i) => {
            k.icon(super::kit::MENU_ICONS, i, R::new(x, y - 2.0, 20.0, 20.0), t::ACCENT);
            x + 36.0
        }
        None => x,
    };
    let w = w - (x - x0);
    let tw = k.text(x, y, t::LABEL, name, t::INK_DIM, 0);
    let d = R::new(x + tw + 14.0, y + t::LABEL.size * 0.5 - 7.0, w - tw - 14.0, 14.0);
    k.slice3(d, "next:panels/divider", Vec2::new(354.0, 28.0), 120.0, 120.0, [1.0; 4]);
}

/// A list row: at rest, focused (fill, accent bar, chevron) or disabled.
pub(in crate::ui) fn row(k: &mut Kit, r: R, label: &str, value: Option<&str>, focused: bool) {
    if focused {
        k.grad_h(r, t::ACCENT, 0.22, 0.05);
        k.fill(R::new(r.x, r.y, t::FOCUS_BAR, r.h), t::ACCENT);
    } else {
        // The pack's bar with its glowing tick.
        k.slice3(r, "next:panels/row_tick", Vec2::new(330.0, 70.0), 40.0, 40.0, [1.0, 1.0, 1.0, 0.85]);
    }
    let ink = if focused { t::INK } else { t::INK_DIM };
    k.text_mid(r.x + 24.0, r.cy(), t::ROW_TEXT, label, ink, 0);
    match value {
        Some(v) => {
            let vx = r.right() - if focused { 44.0 } else { 20.0 };
            let w = k.text_mid(vx, r.cy(), Type { cut: super::font::Cut::Regular, ..t::ROW_TEXT }, v, if focused { t::INK } else { t::INK_DIM }, 2);
            if focused {
                k.chevron(Vec2::new(vx - w - 22.0, r.cy()), 18.0, 2, 2.0, t::ACCENT);
                k.chevron(Vec2::new(r.right() - 24.0, r.cy()), 18.0, 0, 2.0, t::ACCENT);
            }
        }
        None if focused => k.chevron(Vec2::new(r.right() - 24.0, r.cy()), 18.0, 0, 2.0, t::INK),
        None => {}
    }
}


/// The part of a 4:3 picture that covers a rect of aspect `r` (centred).
pub(in crate::ui) fn crop(r: R, pic: f32) -> Rect {
    let a = r.w / r.h;
    if a > pic {
        let h = pic / a;
        Rect::new(0.0, 0.5 - h * 0.5, 1.0, 0.5 + h * 0.5)
    } else {
        let w = a / pic;
        Rect::new(0.5 - w * 0.5, 0.0, 0.5 + w * 0.5, 1.0)
    }
}


/// A row of a column menu: no fill at rest, a hairline under it; focused,
/// the accent gradient and bar.
pub(in crate::ui) fn menu_row(k: &mut Kit, r: R, label: &str, focused: bool, badge: Option<&str>) {
    let bar = R::new(r.x - 4.0, r.y - 3.0, r.w + 8.0, r.h + 6.0);
    if focused {
        k.slice3(bar, "next:panels/row_focus", Vec2::new(450.0, 88.0), 70.0, 80.0, [1.0; 4]);
        k.grad_h(R::new(r.x + 8.0, r.y + 4.0, r.w * 0.7, r.h - 8.0), t::ACCENT, 0.22, 0.0);
        k.chevron(Vec2::new(r.right() - 30.0, r.cy()), 18.0, 0, 2.5, t::INK);
    } else {
        k.hline(r.x + 24.0, r.right(), r.bottom(), 1.0, t::HAIRLINE);
    }
    let ty = Type { size: 28.0, ..t::ROW_TEXT };
    let tw = k.text_mid(r.x + 24.0, r.cy(), ty, label, if focused { t::INK } else { t::INK_DIM }, 0);
    // Something new inside (for the mock-up: new unlocks in Create a Class).
    if label == "Create a Class" {
        k.pic(R::new(r.x + 24.0 + tw + 14.0, r.cy() - 15.0, 85.0, 30.0), "next:badge_new", [1.0; 4]);
    }
    if let Some(b) = badge {
        let w = k.measure(t::MICRO, b) + 18.0;
        let br = R::new(r.right() - w - 48.0, r.cy() - 13.0, w.max(30.0), 26.0);
        k.fill(br, t::REWARD);
        k.text_mid(br.cx(), br.cy(), t::MICRO, b, t::INK_ON_ACCENT, 1);
    }
}

/// A home row's preview: picture, over-line, title, description.
pub(in crate::ui) fn preview(row: &str) -> (&'static str, &'static str, &str, &'static str) {
    let (pic, over, about) = match row {
        "Campaign" => ("next:preview_campaign", "Story", "Call of Duty 4's campaign, from Crew Expendable to Game Over."),
        "Headquarters" => ("next:preview_headquarters", "Social hub", "Make classes, open supply drops and gather your squad between matches."),
        "Private Match" => ("next:preview_private_match", "Host a match", "Bots, splitscreen for up to four, and lobby codes to invite friends."),
        "Join Game" => ("next:preview_join_game", "Online", "Browse servers and join a match in progress."),
        "Create a Class" => ("next:preview_create_a_class", "Loadout", "Build five custom classes: weapons, attachments, camos, reticles and perks."),
        "Character" => ("next:preview_character", "Loadout", "Choose the soldier you play as on each side."),
        "Supply Drops" => ("next:preview_supply_drops", "Rewards", "Open drops for weapon variants, camos and characters."),
        "Combat Record" => ("next:preview_combat_record", "Career", "Your stats, challenges, emblem and calling cards."),
        "Select Profile" => ("next:preview_1", "Profile", "Switch between player profiles."),
        "Options" => ("next:preview_system", "System", "Graphics, sound, game and HUD settings."),
        "Controls" => ("next:preview_system", "System", "Keyboard, mouse and controller bindings."),
        "Mods" => ("next:preview_system", "System", "Load a mod."),
        _ => ("next:preview_system", "System", "Leave the game."),
    };
    (pic, over, row, about)
}

/// Just the pack's frame around `r` (over a picture).
pub(in crate::ui) fn grid_frame(k: &mut Kit, r: R) {
    k.slice9(r, "next:panels/frame_big", Vec2::new(569.0, 256.0), 84.0, 1.0, [1.0; 4]);
}

/// A panel in the pack's look, sharp at any size: a dark green fill with a
/// fine grid and a glow along its top, drawn here, framed by the pack's
/// outline and corner brackets (`corner`: their size in design units).
pub(in crate::ui) fn grid_panel(k: &mut Kit, r: R, corner: f32, bright: f32) {
    k.fill(r, t::hex(0x050706, 1.0));
    k.grad_v(R::new(r.x, r.y, r.w, r.h * 0.45), t::hex(0x73d959, 1.0), 0.012 * bright, 0.0);
    k.grad_v(R::new(r.x, r.bottom() - r.h * 0.3, r.w, r.h * 0.3), t::hex(0x73d959, 1.0), 0.0, 0.008 * bright);
    let line = t::hex(0x9fd36a, 0.022);
    let step = 32.0;
    let mut x = r.x + step;
    while x < r.right() - 2.0 {
        k.vline(x, r.y + 2.0, r.bottom() - 2.0, 1.0, line);
        x += step;
    }
    let mut y = r.y + step;
    while y < r.bottom() - 2.0 {
        k.hline(r.x + 2.0, r.right() - 2.0, y, 1.0, line);
        y += step;
    }
    k.slice9(r, "next:panels/frame_big", Vec2::new(569.0, 256.0), 84.0, corner / 84.0, [1.0, 1.0, 1.0, 0.75 + 0.25 * bright]);
}

/// An action button (Save, Done, Accept, Start): the pack's chevron bar,
/// lit when focused.
pub(in crate::ui) fn action_button(k: &mut Kit, r: R, label: &str, focused: bool) {
    k.slice3(r, "next:panels/btn_chevron", Vec2::new(396.0, 57.0), 30.0, 80.0, [1.0, 1.0, 1.0, if focused { 1.0 } else { 0.8 }]);
    if focused {
        k.grad_h(R::new(r.x + 4.0, r.y + 4.0, r.w - 8.0, r.h - 8.0), t::ACCENT, 0.55, 0.0);
        k.fill(R::new(r.x, r.y, t::FOCUS_BAR, r.h), t::ACCENT);
    }
    k.text_mid(r.x + 24.0, r.cy(), Type { size: 26.0, ..t::ROW_TEXT }, label, if focused { t::INK } else { t::INK_DIM }, 0);
}

/// A frame for an item's picture (perks, swatches): the pack's square box.
pub(in crate::ui) fn icon_box(k: &mut Kit, r: R) {
    k.slice9(r, "next:panels/box_square", Vec2::new(130.0, 126.0), 30.0, (r.w / 130.0).min(1.0), [1.0; 4]);
}
