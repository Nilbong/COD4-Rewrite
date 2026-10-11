//! The new main menu: CoD4's dressed main menu (`main_text`) shown as a
//! column of sections and rows beside a preview of the focused row.
//!
//! Its rows are the classic menu's own buttons (as the lobby, Headquarters,
//! supply drops and the campaign dress it), found by their labels and moved
//! to the column's places with their text cleared: every action, visibility
//! rule, focus sound and the mouse and pad navigation stay the classic menu
//! code's. Only the drawing is this file's.

use super::super::expr::{Env, eval};
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Menu, Rect as VRect, Statement, Window as MenuWindow, flags, item_type};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

/// The menu that replaces `main_text`.
pub const MENU: &str = "main_modern";

/// A row: its label and the words in the classic button's label that find
/// it (any one, lower case).
struct Row {
    label: &'static str,
    keys: &'static [&'static str],
}

const SECTIONS: [(&str, &[Row]); 3] = [
    (
        "Play",
        &[
            Row { label: "Campaign", keys: &["campaign"] },
            Row { label: "Headquarters", keys: &["headquarters"] },
            Row { label: "Private Match", keys: &["private match", "start new server"] },
            Row { label: "Join Game", keys: &["join game"] },
        ],
    ),
    (
        "Player",
        &[
            Row { label: "Create a Class", keys: &["create a class"] },
            Row { label: "Character", keys: &["character"] },
            Row { label: "Supply Drops", keys: &["supply drop"] },
            Row { label: "Combat Record", keys: &["combat record", "rank and challenges", "rank & challenges", "challenges"] },
            Row { label: "Select Profile", keys: &["profile"] },
        ],
    ),
    (
        "System",
        &[
            Row { label: "Options", keys: &["options"] },
            Row { label: "Controls", keys: &["controls"] },
            Row { label: "Single Player", keys: &["single player"] },
            Row { label: "Quit", keys: &["quit"] },
        ],
    ),
];

/// Every row, in order: (section, row).
fn rows() -> impl Iterator<Item = (usize, &'static Row)> {
    SECTIONS.iter().enumerate().flat_map(|(s, (_, rows))| rows.iter().map(move |r| (s, r)))
}

const ROW_H: f32 = 48.0;
const ROW_PITCH: f32 = 50.0;
const TOP: f32 = 176.0;

/// The column's layout for the rows present (by their index in [`rows`]):
/// each section's header y (if it has rows) and each row's rect.
fn layout(present: &[usize]) -> (Vec<(usize, f32)>, Vec<(usize, R)>) {
    let (x, w) = (col_x(0), cols(4));
    let mut headers = Vec::new();
    let mut out = Vec::new();
    let mut y = TOP;
    let mut i = 0;
    for (s, (_, list)) in SECTIONS.iter().enumerate() {
        let here: Vec<usize> = (i..i + list.len()).filter(|k| present.contains(k)).collect();
        i += list.len();
        if here.is_empty() {
            continue;
        }
        headers.push((s, y));
        y += 34.0;
        for k in here {
            out.push((k, R::new(x, y, w, ROW_H)));
            y += ROW_PITCH;
        }
        y += 22.0;
    }
    (headers, out)
}

/// A design rect as a menu item rect: x from the window's centre, y from
/// its top, both scaled by its height, so it lands where the design canvas
/// puts it on any window at least 16:9 wide.
pub(in crate::ui) fn virtual_rect(r: R) -> VRect {
    let k = 480.0 / 1080.0;
    VRect { x: (r.x - 960.0) * k, y: r.y * k, w: r.w * k, h: r.h * k, horz_align: 2, vert_align: 1 }
}

fn item_name(k: usize) -> String {
    format!("modern_{k}")
}

/// The new main menu built from the dressed `main_text`; `None` if none of
/// its buttons are found.
pub(in crate::ui) fn menu(fe: &Frontend, classic: &Menu) -> Option<Menu> {
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = MENU.into();
    let all: Vec<&Row> = rows().map(|(_, r)| r).collect();
    let mut found: Vec<(usize, iw3::menu::Item)> = Vec::new();
    for it in classic.items.iter().filter(|it| it.ty == item_type::BUTTON) {
        let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, fe).text() };
        let label = fe.assets.localize(&raw).to_ascii_lowercase();
        if label.trim().is_empty() {
            continue;
        }
        let Some(k) = all.iter().position(|r| r.keys.iter().any(|w| label.contains(w))) else {
            debug!("new main menu: no row for {label:?}");
            continue;
        };
        // The campaign is the test build's only ("Play Test Build.bat").
        if matches!(all[k].label, "Campaign" | "Single Player") && !crate::modes::test_features() {
            continue;
        }
        found.push((k, it.clone()));
    }
    if found.is_empty() {
        return None;
    }
    let present: Vec<usize> = found.iter().map(|(k, _)| *k).collect();
    let (_, rects) = layout(&present);
    for (k, mut item) in found {
        let r = rects.iter().find(|(i, _)| *i == k).map(|(_, r)| *r).unwrap_or_default();
        item.window = MenuWindow { rect: virtual_rect(r), dynamic_flags: flags::VISIBLE, name: item_name(k), ..MenuWindow::default() };
        // The label stays (the shot harness finds rows by it), undrawn:
        // this file draws it.
        item.text = all[k].label.into();
        item.text_exp = Statement::default();
        item.text_scale = 0.0;
        out.items.push(item);
    }
    Some(out)
}

/// Whether the new main menu is up with nothing over it (it draws its own
/// button prompts then).
pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    let Some(at) = fe.stack.iter().position(|m| m.menu.window.name == MENU) else { return false };
    !fe.stack[at + 1..].iter().any(|m| m.menu.visible_exp.is_empty() || eval(&m.menu.visible_exp, fe).truthy())
}

/// When the menu last opened (ms), for its fade in.
static OPENED_AT: AtomicI64 = AtomicI64::new(i64::MIN);
/// The row last focused: its preview stays when the mouse leaves the rows.
static SHOWN: AtomicUsize = AtomicUsize::new(usize::MAX);

pub(in crate::ui) fn opened(fe: &Frontend) {
    OPENED_AT.store(fe.millis(), Ordering::Relaxed);
}

/// A home row's preview: picture, over-line, description.
fn preview(row: &str) -> (&'static str, &'static str, &'static str) {
    match row {
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
        "Single Player" => ("next:preview_campaign", "Story", "Call of Duty 4's own single player."),
        _ => ("next:preview_system", "System", "Leave the game."),
    }
}

/// Any weapon newly unlocked (its stat's "new" mark).
fn anything_new(fe: &Frontend) -> bool {
    (3000..3600).any(|k| fe.stats.get(k) & 65536 != 0)
}

/// The menu's drawing, under its (invisible) rows.
pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    let since = (fe.millis() - OPENED_AT.load(Ordering::Relaxed)).max(0) as f32 / 1000.0;
    let ease = t::ease_out(since / t::SLOW);

    // Another menu open over this one (Options and its pages): only the
    // backdrop, under it.
    let covered = fe
        .stack
        .iter()
        .skip_while(|m| !std::ptr::eq(*m, om))
        .skip(1)
        .any(|m| m.menu.visible_exp.is_empty() || eval(&m.menu.visible_exp, fe).truthy());
    backdrop(&mut k, 0.05);
    if covered {
        return;
    }
    k.alpha = ease;
    k.pic(R::new(t::SAFE_X, 40.0, 300.0, 300.0 * 383.0 / 1384.0), "next:logo", [1.0; 4]);
    player_card(&mut k);
    top_icons(&mut k, 1920.0 - t::SAFE_X - 660.0);

    // The rows present, and which is focused.
    let all: Vec<&Row> = rows().map(|(_, r)| r).collect();
    let items = |i: usize| om.menu.items.iter().enumerate().filter(move |(_, it)| it.window.name == item_name(i)).map(|(n, _)| n);
    let present: Vec<usize> = (0..all.len()).filter(|&i| items(i).next().is_some()).collect();
    let focused_item = fe.focus.as_ref().filter(|(m, _)| *m == om.name).map(|(_, i)| *i);
    let focused = present.iter().copied().find(|&i| items(i).any(|n| Some(n) == focused_item));
    let forced = super::mock_focus().and_then(|f| all.iter().position(|r| r.label == f));
    if let Some(f) = forced.or(focused) {
        SHOWN.store(f, Ordering::Relaxed);
    }
    let shown = Some(SHOWN.load(Ordering::Relaxed)).filter(|s| present.contains(s)).or(present.first().copied());

    // Slide the column in.
    k.shift = Vec2::new((1.0 - ease) * -24.0, 0.0);
    let (headers, rects) = layout(&present);
    for (s, y) in headers {
        section(&mut k, col_x(0), y, cols(4), SECTIONS[s].0);
    }
    let unopened = crate::supply::inventory().unopened;
    let count = (unopened > 0).then(|| unopened.to_string());
    let new = anything_new(fe);
    for (i, r) in rects {
        if !items(i).any(|n| fe.item_visible(om, n)) {
            continue;
        }
        let label = all[i].label;
        let badge = (label == "Supply Drops").then_some(count.as_deref()).flatten();
        let hot = forced.or(focused) == Some(i);
        menu_row(&mut k, r, label, hot, badge);
        // Its icon, on the right.
        use super::kit::icons::*;
        let icon = match label {
            "Campaign" | "Single Player" => Some(CROSSHAIR),
            "Headquarters" => Some(HOME),
            "Private Match" => Some(PEOPLE),
            "Join Game" => Some(LINK),
            "Create a Class" => Some(RIFLE),
            "Character" => Some(HELMET),
            "Supply Drops" => Some(CRATE),
            "Combat Record" => Some(TROPHY),
            "Select Profile" => Some(STAR),
            "Options" => Some(COG),
            "Controls" => Some(PAD),
            "Mods" => Some(KEYBOARD),
            _ => None,
        };
        if let Some(i) = icon.filter(|_| badge.is_none()) {
            let x = r.right() - if hot { 78.0 } else { 46.0 };
            k.icon(super::kit::MENU_ICONS, i, R::new(x, r.cy() - 12.0, 24.0, 24.0), if hot { t::INK } else { t::with_alpha(t::INK_DIM, 0.6) });
        }
        if label == "Create a Class" && new {
            let tw = k.measure(Type { size: 28.0, ..t::ROW_TEXT }, label);
            k.pic(R::new(r.x + 24.0 + tw + 14.0, r.cy() - 15.0, 85.0, 30.0), "next:badge_new", [1.0; 4]);
        }
    }

    // The preview, sliding the other way.
    k.shift = Vec2::new((1.0 - ease) * 24.0, 0.0);
    if let Some(s) = shown {
        let label = all[s].label;
        let (picture, over, about) = preview(label);
        let art = R::new(col_x(5), TOP, cols(7), 620.0);
        k.fill(art, t::RAISED);
        k.pic_uv(art, picture, crop(art, 2.0), [1.0; 4]);
        k.grad_v(R::new(art.x, art.y + art.h * 0.3, art.w, art.h * 0.7), t::hex(0x050607, 1.0), 0.0, 0.95);
        k.grad_h(R::new(art.x, art.y, art.w * 0.6, art.h), t::hex(0x050607, 1.0), 0.5, 0.0);
        k.slice9(art.inset(-10.0), "next:panels/frame_big", Vec2::new(569.0, 256.0), 84.0, 1.0, [1.0, 1.0, 1.0, 0.95]);
        k.text(art.x + 36.0, art.bottom() - 178.0, t::LABEL, over, t::ACCENT, 0);
        k.text(art.x + 36.0, art.bottom() - 150.0, Type { size: 72.0, ..t::DISPLAY }, label, t::INK, 0);
        k.para(art.x + 36.0, art.bottom() - 64.0, art.w - 72.0, t::BODY, about, t::INK_DIM, 1.3);
    }
    k.shift = Vec2::ZERO;
    footer(&mut k, "Game experience may change during online play.", &[(Button::Confirm, "Select")]);
}
