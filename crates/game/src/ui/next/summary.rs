//! The post-match summary (Modern Warfare's): back in the lobby after a
//! match, a full-screen card of how it went: the result, the player's kills,
//! deaths and assists, the XP it earned by where it came from (combat,
//! challenges, the match bonus), the rank bar filling from where it was to
//! where it is, and the challenges completed. Enter (pad A) or Esc moves on;
//! it moves on by itself after a while.
//!
//! The match is recorded as it's played: [`begin`] at its start (the XP and
//! rank then), [`add`] from where XP is given ([`super::super::progression`],
//! [`super::super::challenges`]), [`finish`] at its end (the result and the
//! player's line). Shown once, the next time the lobby is on top.
//!
//! Debug aids: `COD4RW_SUMMARYTEST=1` shows a made-up one over the main
//! menu; `COD4RW_SUMMARYSHOT=<png>` saves a screenshot of it 3 s after it
//! opens and quits (a real match's, played out from the menus).

use super::super::expr::Env;
use super::super::lobby::LOBBY_MENU;
use super::super::progression::stat;
use super::super::{Frontend, OpenMenu, Op, Placement};
use super::cac;
use super::home::virtual_rect;
use super::kit::{Button, Kit, R, View};
use super::shell::*;
use super::theme::{self as t, Type, col_x, cols};
use bevy::prelude::*;
use iw3::menu::{Item, Menu, Window as MenuWindow, flags, item_type};
use std::sync::Mutex;

pub const MENU: &str = "next_summary";
/// It moves on by itself after this long (s).
const AUTO_CLOSE: f32 = 20.0;

/// Where XP came from.
#[derive(Clone, Copy, PartialEq)]
pub enum Source {
    /// Kills, headshots, assists and the match's awards.
    Combat,
    /// The match bonus at the end.
    Bonus,
    /// Anything else (weapon mastery camos).
    Other,
}

#[derive(Clone, Default)]
struct Summary {
    /// A match is being recorded.
    recording: bool,
    /// Finished, waiting to be shown in the lobby.
    ready: bool,
    start_xp: i32,
    combat: i32,
    challenge_xp: i32,
    bonus: i32,
    other: i32,
    challenges: Vec<(String, i32)>,
    outcome: Option<bool>,
    kills: u32,
    deaths: u32,
    assists: u32,
    mode: String,
    map: String,
}

static SUMMARY: Mutex<Option<Summary>> = Mutex::new(None);
/// When the screen opened (frontend ms).
static SHOWN_AT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

fn with<R>(f: impl FnOnce(&mut Summary) -> R) -> R {
    let mut g = SUMMARY.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Summary::default))
}

/// A match starts: the player's XP now, the map.
pub(in crate::ui) fn begin(xp: i32, map: &str) {
    with(|s| *s = Summary { recording: true, start_xp: xp, map: map.to_owned(), ..Summary::default() });
}

/// XP given, and where from.
pub(in crate::ui) fn add(source: Source, xp: i32) {
    with(|s| {
        if !s.recording {
            return;
        }
        match source {
            Source::Combat => s.combat += xp,
            Source::Bonus => s.bonus += xp,
            Source::Other => s.other += xp,
        }
    });
}

/// A challenge completed: its name and XP.
pub(in crate::ui) fn add_challenge(name: &str, xp: i32) {
    with(|s| {
        if s.recording {
            s.challenges.push((name.to_owned(), xp));
            s.challenge_xp += xp;
        }
    });
}

/// The match ended: the result (won, lost, `None` drawn) and the player's
/// line. Shown the next time the lobby is on top.
pub(in crate::ui) fn finish(outcome: Option<bool>, kills: u32, deaths: u32, assists: u32, mode: &str) {
    with(|s| {
        if !s.recording || s.ready {
            return;
        }
        // (Still recording until it's shown: a challenge completed on the
        // match's last frame, after this, still counts.)
        s.ready = true;
        s.outcome = outcome;
        (s.kills, s.deaths, s.assists) = (kills, deaths, assists);
        s.mode = mode.to_owned();
    });
}

pub(super) fn build(app: &mut App) {
    app.add_systems(OnEnter(crate::state::GameState::InGame), start).add_systems(Update, show.run_if(resource_exists::<Frontend>));
}

/// A match starts (not the campaign or Headquarters: no summary for those).
fn start(fe: Option<Res<Frontend>>, map: Option<Res<crate::world::MapName>>) {
    let Some(fe) = fe else { return };
    if crate::campaign::active() || crate::hq::active() {
        with(|s| s.recording = false);
        return;
    }
    begin(fe.stats.get(stat::RANKXP), map.as_ref().map_or("", |m| m.0.as_str()));
}

/// Open it over the lobby once a match is over; move on after a while.
fn show(mut commands: Commands, mut fe: ResMut<Frontend>, keys: Res<ButtonInput<KeyCode>>, mut shot: Local<u8>, mut exit: MessageWriter<AppExit>) {
    let top = fe.stack.last().map(|m| m.name.clone());
    if std::env::var_os("COD4RW_SUMMARYTEST").is_some() && top.as_deref() == Some("main_text") {
        with(|s| {
            if !s.ready && s.start_xp == 0 {
                *s = test_summary(fe.stats.get(stat::RANKXP));
            }
        });
    }
    let wanted = matches!(top.as_deref(), Some(LOBBY_MENU) | Some("main_text")) && super::super::modern::wanted(&fe);
    let due = wanted
        && with(|s| {
            let due = s.ready && s.recording;
            if due {
                (s.ready, s.recording) = (false, false);
            }
            due
        });
    if due {
        SHOWN_AT.store(fe.millis(), std::sync::atomic::Ordering::Relaxed);
        fe.open(MENU);
        return;
    }
    if top.as_deref() == Some(MENU) {
        // Enter (the keypad's too, and the pad's A, which presses Enter in
        // menus) moves on, whatever has the focus. (Not in its first moment:
        // the press that left the match.)
        if since(&fe) > 0.4 && keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter, KeyCode::Space]) {
            fe.close(MENU);
            return;
        }
        if let Ok(path) = std::env::var("COD4RW_SUMMARYSHOT") {
            use bevy::render::view::screenshot::{Screenshot, save_to_disk};
            if *shot == 0 && since(&fe) > 3.0 {
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
                *shot = 1;
            } else if *shot == 1 && since(&fe) > 4.5 {
                exit.write(AppExit::Success);
                *shot = 2;
            }
        }
        if since(&fe) > AUTO_CLOSE {
            fe.close(MENU);
        }
    }
}

fn since(fe: &Frontend) -> f32 {
    (fe.millis() - SHOWN_AT.load(std::sync::atomic::Ordering::Relaxed)).max(0) as f32 / 1000.0
}

/// `COD4RW_SUMMARYTEST`'s made-up match.
fn test_summary(xp: i32) -> Summary {
    Summary {
        recording: true,
        ready: true,
        start_xp: (xp - 1450).max(0),
        combat: 900,
        challenge_xp: 300,
        bonus: 250,
        other: 0,
        challenges: vec![("Marksman I".into(), 100), ("Headshots: M16A4".into(), 150), ("Hat Trick".into(), 50)],
        outcome: Some(true),
        kills: 18,
        deaths: 7,
        assists: 4,
        mode: "Team Deathmatch".into(),
        map: "mp_crash".into(),
    }
}

/// The screen: one Continue button (Enter, pad A) at the footer's right.
pub(in crate::ui) fn menu() -> Menu {
    let mut m = Menu::default();
    m.window.name = MENU.into();
    m.full_screen = true;
    m.on_esc = "\"play\" \"mouse_click\" ; \"close\" \"self\" ; ".into();
    m.items.push(Item {
        ty: item_type::BUTTON,
        window: MenuWindow { rect: virtual_rect(R::new(1824.0 - 360.0, t::FOOTER_Y - 2.0, 360.0, 44.0)), dynamic_flags: flags::VISIBLE, name: "summary_continue".into(), ..MenuWindow::default() },
        text: "Continue".into(),
        action: "\"play\" \"mouse_click\" ; \"close\" \"self\" ; ".into(),
        on_focus: "\"play\" \"mouse_over\" ; ".into(),
        text_scale: 0.0,
        ..Item::default()
    });
    m
}

pub(in crate::ui) fn on_top(fe: &Frontend) -> bool {
    fe.stack.last().is_some_and(|m| m.menu.window.name == MENU)
}

pub(in crate::ui) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    if om.menu.window.name != MENU {
        return;
    }
    let Some(s) = SUMMARY.lock().ok().and_then(|g| g.clone()) else { return };
    let age = since(fe);
    let ease = |d: f32, len: f32| t::ease_out(((age - d) / len).clamp(0.0, 1.0));
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    cac::loadout_backdrop(&mut k);
    let crumb = if s.map.is_empty() { s.mode.clone() } else { format!("{}  /  {}", s.mode, fe.localize(&format!("@MPUI_{}", s.map.trim_start_matches("mp_").to_uppercase()))) };
    header(&mut k, &crumb, "Match Summary");

    // Left: the result, the player's line, the challenges.
    let (x, w) = (col_x(0), cols(5));
    let (word, colour) = match s.outcome {
        Some(true) => ("VICTORY", t::ACCENT),
        Some(false) => ("DEFEAT", t::ENEMY),
        None => ("DRAW", t::REWARD),
    };
    k.alpha = ease(0.0, 0.4);
    k.shift = Vec2::new((1.0 - k.alpha) * -30.0, 0.0);
    k.text(x, 196.0, t::LABEL, "Result", t::INK_DIM, 0);
    k.text(x, 220.0, Type { size: 120.0, ..t::DISPLAY }, word, colour, 0);
    let kd = if s.deaths == 0 { s.kills as f32 } else { s.kills as f32 / s.deaths as f32 };
    let tiles = [("Kills", s.kills.to_string()), ("Deaths", s.deaths.to_string()), ("Assists", s.assists.to_string()), ("K/D", format!("{kd:.2}"))];
    let tw = (w - 3.0 * 16.0) / 4.0;
    for (i, (label, value)) in tiles.iter().enumerate() {
        let r = R::new(x + i as f32 * (tw + 16.0), 390.0, tw, 130.0);
        icon_box(&mut k, r);
        k.text(r.x + 20.0, r.y + 22.0, t::MICRO, label, t::INK_DIM, 0);
        k.text(r.x + 20.0, r.y + 50.0, Type { size: 56.0, ..t::DISPLAY }, value, t::INK, 0);
    }
    section(&mut k, x, 568.0, w, "Challenges completed");
    if s.challenges.is_empty() {
        k.text(x, 612.0, Type { size: 22.0, ..t::BODY }, "None this match.", t::INK_MUTE, 0);
    }
    for (i, (name, xp)) in s.challenges.iter().take(7).enumerate() {
        let r = R::new(x, 604.0 + i as f32 * 48.0, w, 44.0);
        let a = ease(0.6 + i as f32 * 0.15, 0.3);
        k.alpha = a;
        k.fill(r, t::hex(0x000000, 0.35));
        k.check(Vec2::new(r.x + 24.0, r.cy()), 16.0, 2.5, t::ACCENT);
        k.text_mid(r.x + 48.0, r.cy(), Type { size: 22.0, ..t::BODY }, name, t::INK, 0);
        k.text_mid(r.right() - 16.0, r.cy(), Type { size: 22.0, ..t::H2 }, &format!("+{xp}"), t::REWARD, 2);
    }
    k.alpha = 1.0;

    // Right: the XP and the rank.
    let p = R::new(col_x(6), 192.0, cols(6), 780.0);
    k.shift = Vec2::new((1.0 - ease(0.15, 0.4)) * 30.0, 0.0);
    grid_panel(&mut k, p, 84.0, 1.0);
    let (px, pw) = (p.x + 44.0, p.w - 88.0);
    k.text(px, p.y + 36.0, t::LABEL, "XP earned", t::ACCENT, 0);
    let total = s.combat + s.challenge_xp + s.bonus + s.other;
    // The total counts up.
    let shown_total = (total as f32 * ease(0.4, 1.2)).round() as i32;
    k.text(px, p.y + 60.0, Type { size: 96.0, ..t::DISPLAY }, &format!("+{shown_total}"), t::INK, 0);
    let mut rows = vec![("Combat", s.combat, "Kills, headshots, assists and awards"), ("Challenges", s.challenge_xp, "Challenges completed this match")];
    rows.push(("Match bonus", s.bonus, match s.outcome {
        Some(true) => "For the win and the time played",
        Some(false) => "For the time played",
        None => "For the draw and the time played",
    }));
    if s.other > 0 {
        rows.push(("Other", s.other, "Weapon mastery"));
    }
    for (i, (label, xp, about)) in rows.iter().enumerate() {
        let r = R::new(px, p.y + 196.0 + i as f32 * 64.0, pw, 56.0);
        k.alpha = ease(0.3 + i as f32 * 0.12, 0.3);
        k.hline(r.x, r.right(), r.bottom(), 1.0, t::HAIRLINE);
        k.text(r.x, r.y + 4.0, Type { size: 26.0, ..t::H2 }, label, t::INK, 0);
        k.text(r.x, r.y + 34.0, t::MICRO, about, t::INK_MUTE, 0);
        k.text_mid(r.right(), r.cy(), Type { size: 30.0, ..t::H1 }, &format!("+{xp}"), if *xp > 0 { t::REWARD } else { t::INK_MUTE }, 2);
    }
    k.alpha = 1.0;

    // The rank bar, filling from where the match started to where it ended.
    let end_xp = fe.stats.get(stat::RANKXP);
    let now_rank = super::super::combat_record::rank_at(fe, end_xp);
    let was = super::super::combat_record::rank_at(fe, s.start_xp);
    let promoted = now_rank.level > was.level;
    let span = now_rank.next.map_or(1, |n| (n - now_rank.min).max(1)) as f32;
    let from = if promoted { 0.0 } else { (s.start_xp - now_rank.min) as f32 / span };
    let to = (end_xp - now_rank.min) as f32 / span;
    let fill = from + (to - from) * ease(0.9, 1.4);
    let y = p.bottom() - 220.0;
    k.text(px, y, t::LABEL, if promoted { "Promoted" } else { "Rank" }, if promoted { t::REWARD } else { t::INK_DIM }, 0);
    if !now_rank.icon.is_empty() {
        k.pic(R::new(px, y + 30.0, 64.0, 64.0), &now_rank.icon, [1.0; 4]);
    }
    k.text(px + 80.0, y + 34.0, Type { size: 34.0, ..t::H1 }, &now_rank.name, t::INK, 0);
    k.text(px + 80.0, y + 76.0, t::MICRO, &format!("Level {}", now_rank.level), t::INK_DIM, 0);
    let bar = R::new(px, y + 120.0, pw, 14.0);
    k.fill(bar, t::hex(0xffffff, 0.12));
    k.fill(R::new(bar.x, bar.y, bar.w * from.clamp(0.0, 1.0), bar.h), t::INK_DIM);
    if fill > from {
        k.fill(R::new(bar.x + bar.w * from.clamp(0.0, 1.0), bar.y, bar.w * (fill - from).clamp(0.0, 1.0), bar.h), t::ACCENT);
    }
    let to_next = now_rank.next.map_or_else(|| "Top rank".to_owned(), |n| format!("{} XP to Level {}", (n - end_xp).max(0), now_rank.level + 1));
    k.text(px, y + 150.0, t::MICRO, &to_next, t::INK_DIM, 0);
    k.shift = Vec2::ZERO;

    let left = (AUTO_CLOSE - age).ceil().max(0.0) as i32;
    footer(&mut k, &format!("Continuing in {left}"), &[(Button::Confirm, "Continue")]);
}
