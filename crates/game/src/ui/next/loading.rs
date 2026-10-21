//! The load screen into a match: the map's picture (at its own shape, over
//! a dark copy of itself filling the screen), its name, the game type and
//! what it's about, the match's limits, a tip that changes as it loads
//! (CoD4's own "did you know" lines and ours), and a bar along the bottom.
//!
//! The bar follows the load: a little while the screen comes up, then
//! [`crate::settle::progress`] (the match running smoothly) for the rest.
//! It only ever moves forward.

use super::super::expr::Env;
use super::super::{Frontend, Op};
use super::kit::{Kit, R, View};
use super::theme::{self as t, Type};
use crate::modes::GameMode;
use bevy::prelude::*;
use std::sync::Mutex;

/// What the screen says about the match.
pub(in crate::ui) struct Match<'a> {
    pub map: &'a str,
    pub mode: Option<GameMode>,
    pub hardcore: bool,
    pub time_limit: f32,
    pub score_limit: u32,
    /// The match is loaded and settling (else the screen just came up).
    pub settling: bool,
}

/// Ours, between CoD4's.
const TIPS: &[&str] = &[
    "Create a Class lets you pick three kill streaks; they're earned in order of kills.",
    "Supply drops earn weapon variants, camos and characters as you play.",
    "Leaning round a corner shows less of you than stepping out.",
    "Your Combat Record keeps every weapon's kills, headshots and accuracy.",
    "Hardcore: less health, no HUD to speak of, and friendly fire.",
    "A silenced weapon keeps you off the enemy's minimap when you fire.",
    "Headquarters is the place to try classes and open supply drops between matches.",
    "Players 2 to 4 in splitscreen can play as their own profiles.",
    "The Server Browser lists every public lobby on the relay.",
    "Sprint runs out: let it recover before you need it.",
];

/// Where the bar and the tip are (kept from frame to frame).
struct State {
    shown: f32,
    started: f64,
    tip: usize,
    map: String,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// CoD4's "did you know" lines (`mp/didyouknow.csv`), localized.
fn cod4_tips(fe: &Frontend) -> Vec<String> {
    let Some(table) = fe.assets.table("mp/didyouknow.csv") else { return Vec::new() };
    (0..table.rows)
        .filter_map(|r| (0..table.columns).filter_map(|c| table.get(r, c)).map(str::trim).find(|v| v.chars().any(|c| c.is_ascii_uppercase()) && v.contains('_')))
        .map(|key| fe.assets.localize(&format!("@{}", key.trim_start_matches('@'))))
        .filter(|s| s.len() > 12 && !s.contains('_'))
        // (Not what we have: Old School modes, voice chat, ranked servers,
        // server settings.)
        .filter(|s| !["Old School", "voice chat", "Ranked", "Unranked", "own game mode", "own server"].iter().any(|w| s.contains(w)))
        .collect()
}

fn map_name(id: &str) -> String {
    // (A mission is its title: F.N.G., not Killhouse.)
    if let Some(t) = crate::campaign::title() {
        return t.to_owned();
    }
    super::super::lobby::maps().iter().find(|(m, _)| *m == id).map_or_else(
        || {
            let s = id.trim_start_matches("mp_").replace('_', " ");
            let mut c = s.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str())
        },
        |(_, n)| (*n).to_owned(),
    )
}

pub(in crate::ui) fn ops(fe: &Frontend, m: &Match, window: Vec2) -> Vec<Op> {
    let mut ops = Vec::new();
    let now = fe.millis() as f64 / 1000.0;
    // The bar: the screen coming up is the first tenth, settling the rest.
    let target = if m.settling { 0.15 + 0.85 * crate::settle::progress() } else { 0.08 };
    let (shown, tip) = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = state.as_ref().is_none_or(|s| s.map != m.map || (!m.settling && s.shown > 0.5));
        if fresh {
            let seed = (fe.millis() as usize).wrapping_mul(2654435761);
            *state = Some(State { shown: 0.0, started: now, tip: seed, map: m.map.to_owned() });
        }
        let s = state.as_mut().expect("set above");
        // Ease toward it, never back.
        s.shown = (s.shown + (target - s.shown).max(0.0) * 0.25).max(s.shown).min(1.0);
        // A new tip every seven seconds.
        let tip = s.tip + ((now - s.started) / 7.0) as usize;
        (s.shown, tip)
    };

    let mut k = Kit::new(fe, &mut ops, View::new(Vec2::ZERO, window));
    let b = k.v.bleed();
    let picture = format!("loadscreen_{}", m.map);
    // Behind: the picture filling the screen, dark.
    k.fill(b, t::hex(0x050607, 1.0));
    let a = b.w / b.h;
    let vh = ((4.0 / 3.0) / a).min(1.0);
    k.pic_uv(b, &picture, Rect::new(0.0, 0.5 - vh * 0.5, 1.0, 0.5 + vh * 0.5), [0.2, 0.2, 0.2, 1.0]);
    k.grad_h(R::new(b.x, b.y, b.w * 0.6, b.h), t::hex(0x050607, 1.0), 0.92, 0.0);
    k.grad_v(R::new(b.x, b.bottom() - 340.0, b.w, 340.0), t::hex(0x050607, 1.0), 0.0, 0.95);
    k.grad_v(R::new(b.x, b.y, b.w, 160.0), t::hex(0x050607, 1.0), 0.7, 0.0);

    // The picture itself, at its shape, framed.
    let card = R::new(1920.0 - t::SAFE_X - 880.0, 150.0, 880.0, 660.0);
    k.fill(card, t::hex(0x000000, 0.6));
    k.pic(card.inset(6.0), &picture, [1.0; 4]);
    super::shell::grid_frame(&mut k, card);

    // The map and the game type.
    let x = t::SAFE_X;
    let (mode_name, about) = match m.mode {
        Some(mode) => {
            let name = mode.name();
            (name.to_owned(), super::modes::about(name).1.to_owned())
        }
        None => (String::new(), String::new()),
    };
    let over = if m.hardcore && !mode_name.is_empty() { format!("{mode_name}  \u{b7}  Hardcore") } else { mode_name.clone() };
    k.text(x, 170.0, t::LABEL, &over, t::ACCENT, 0);
    k.text(x, 196.0, Type { size: 96.0, ..t::DISPLAY }, &map_name(m.map), t::INK, 0);
    let mut y = 316.0;
    if !about.is_empty() {
        y += k.para(x, y, 780.0, Type { size: 24.0, ..t::BODY }, &about, t::INK_DIM, 1.35) + 24.0;
    }
    // The limits.
    if m.mode.is_some() && (m.time_limit > 0.0 || m.score_limit > 0) {
        let mut chips = Vec::new();
        if m.time_limit > 0.0 {
            chips.push(format!("{} min", (m.time_limit / 60.0).round()));
        }
        if m.score_limit > 0 {
            chips.push(format!("{} to win", m.score_limit));
        }
        let mut cx = x;
        for c in chips {
            let ty = Type { size: 18.0, ..t::CAPTION };
            let w = k.measure(ty, &c) + 28.0;
            let r = R::new(cx, y, w, 34.0);
            k.fill(r, t::hex(0x000000, 0.45));
            k.frame(r, 1.0, t::LINE);
            k.text_mid(r.x + 14.0, r.cy(), ty, &c, t::INK, 0);
            cx += w + 10.0;
        }
    }

    // A tip.
    let mut tips = cod4_tips(fe);
    tips.extend(TIPS.iter().map(|s| (*s).to_owned()));
    if let Some(tip) = (!tips.is_empty()).then(|| &tips[tip % tips.len()]) {
        let r = R::new(x, 820.0, 1100.0, 96.0);
        k.fill(R::new(r.x, r.y, 3.0, r.h), t::ACCENT);
        k.text(r.x + 24.0, r.y + 6.0, t::LABEL, "Tip", t::ACCENT, 0);
        k.para(r.x + 24.0, r.y + 34.0, r.w - 24.0, Type { size: 24.0, ..t::BODY }, tip, t::INK, 1.3);
    }

    // The bar along the bottom.
    let bar = R::new(x, 1080.0 - 72.0, 1920.0 - 2.0 * x, 6.0);
    k.fill(bar, t::hex(0xffffff, 0.12));
    k.fill(R::new(bar.x, bar.y, bar.w * shown, bar.h), t::ACCENT);
    // A light running along the filled part.
    let glow = ((now * 0.8).fract() as f32) * bar.w * shown;
    k.grad_h(R::new(bar.x + (glow - 80.0).max(0.0), bar.y, 80.0_f32.min(glow), bar.h), t::hex(0xffffff, 1.0), 0.0, 0.5);
    let label = if shown >= 0.999 { "Ready".to_owned() } else { format!("Loading  {}%", (shown * 100.0) as u32) };
    k.text(bar.x, bar.y - 34.0, t::LABEL, &label, t::INK_DIM, 0);
    drop(k);
    ops
}
