//! The Modern HUD (the settings' HUD Style): the same information as CoD4's,
//! drawn from our own art (`assets/ui/hud`: two atlases and a manifest of
//! their sprites): charcoal panels with tan borders and olive accents, and
//! ivory digits. The minimap sits in a frame top left with the team scores
//! and the clock under it, the compass on a rail along the top, the ammo
//! (clip / reserve, a strip of rounds, the weapon's name and the grenades)
//! bottom right, the kill feed bottom left on rows; a crosshair and hit
//! marker of its own. Everything is laid out in CoD's 640x480 virtual
//! screen, so splitscreen viewports scale it as they do the classic HUD.

use super::{Painter, vr};
use crate::weapons::WeaponState;
use bevy::prelude::*;
use iw3::menu::Rect as VRect;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The atlases, as UI materials ([`picture`]).
const ELEMENTS: &str = "hudm:elements";
const GLYPHS: &str = "hudm:glyphs";

/// The art's colours.
pub(super) const TAN: [f32; 4] = [0.86, 0.79, 0.62, 1.0];
pub(super) const IVORY: [f32; 4] = [0.96, 0.93, 0.84, 1.0];
pub(super) const OLIVE: [f32; 4] = [0.62, 0.70, 0.38, 1.0];
pub(super) const ORANGE: [f32; 4] = [0.93, 0.55, 0.22, 1.0];
const WHITE: [f32; 4] = [1.0; 4];

/// The embedded atlases.
pub(in crate::ui) fn picture(name: &str) -> Option<&'static [u8]> {
    match name {
        ELEMENTS => Some(include_bytes!("../../../assets/ui/hud/hud-elements.png")),
        GLYPHS => Some(include_bytes!("../../../assets/ui/hud/hud-numeric-glyphs.png")),
        // The user's kill streak icons (white on clear, 256 square).
        "ks:uav" => Some(include_bytes!("../../../assets/ui/killstreaks/uav.png")),
        "ks:airstrike" => Some(include_bytes!("../../../assets/ui/killstreaks/airstrike.png")),
        "ks:helicopter" => Some(include_bytes!("../../../assets/ui/killstreaks/helicopter.png")),
        "ks:care_package" => Some(include_bytes!("../../../assets/ui/killstreaks/care_package.png")),
        "ks:sentry_gun" => Some(include_bytes!("../../../assets/ui/killstreaks/sentry_gun.png")),
        // The user's grenade and equipment icons.
        "oh:frag" => Some(include_bytes!("../../../assets/ui/offhand/frag.png")),
        "oh:flash" => Some(include_bytes!("../../../assets/ui/offhand/flash.png")),
        "oh:stun" => Some(include_bytes!("../../../assets/ui/offhand/stun.png")),
        "oh:smoke" => Some(include_bytes!("../../../assets/ui/offhand/smoke.png")),
        "oh:claymore" => Some(include_bytes!("../../../assets/ui/offhand/claymore.png")),
        "oh:c4" => Some(include_bytes!("../../../assets/ui/offhand/c4.png")),
        "oh:rpg" => Some(include_bytes!("../../../assets/ui/offhand/rpg.png")),
        "oh:gl" => Some(include_bytes!("../../../assets/ui/offhand/gl.png")),
        _ => None,
    }
}

/// A sprite: its atlas, where in it (0..1), and its size in pixels.
struct Sprite {
    sheet: &'static str,
    uv: Rect,
    size: Vec2,
}

/// The manifest's sprites, by name (glyphs by their own: "0", "colon", ...).
fn sprites() -> &'static HashMap<String, Sprite> {
    static SPRITES: OnceLock<HashMap<String, Sprite>> = OnceLock::new();
    SPRITES.get_or_init(|| {
        let mut out = HashMap::new();
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(include_str!("../../../assets/ui/hud/hud-assets-manifest.json")) else {
            warn!("hud: unreadable manifest");
            return out;
        };
        for (key, sheet) in [("hud_elements", ELEMENTS), ("numeric_glyphs", GLYPHS)] {
            let Some(list) = manifest["sheets"][key]["sprites"].as_object() else { continue };
            let atlas = manifest["sheets"][key]["size"].as_array().and_then(|s| Some(Vec2::new(s.first()?.as_f64()? as f32, s.get(1)?.as_f64()? as f32)));
            let Some(atlas) = atlas else { continue };
            for (name, s) in list {
                let r: Vec<f32> = s["rect"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect()).unwrap_or_default();
                if let [x, y, w, h] = r[..] {
                    let uv = Rect::new(x / atlas.x, y / atlas.y, (x + w) / atlas.x, (y + h) / atlas.y);
                    out.insert(name.clone(), Sprite { sheet, uv, size: Vec2::new(w, h) });
                }
            }
        }
        out
    })
}

/// A sprite's height for a width, keeping its proportions.
fn height_for(name: &str, w: f32) -> f32 {
    sprites().get(name).map_or(w, |s| w * s.size.y / s.size.x.max(1.0))
}

/// Draw a sprite into `r` (virtual units), tinted, turned clockwise.
pub(super) fn sprite(p: &mut Painter, name: &str, r: VRect, color: [f32; 4], rot: f32, layer: u8) {
    if let Some(s) = sprites().get(name) {
        p.image_uv(s.sheet, r, color, Some(s.uv), rot, layer);
    }
}

/// A sprite's left part: `share` (0..1) of its width.
fn sprite_part(p: &mut Painter, name: &str, r: VRect, share: f32, color: [f32; 4], layer: u8) {
    let Some(s) = sprites().get(name) else { return };
    let share = share.clamp(0.0, 1.0);
    if share <= 0.0 {
        return;
    }
    let uv = Rect::new(s.uv.min.x, s.uv.min.y, s.uv.min.x + (s.uv.max.x - s.uv.min.x) * share, s.uv.max.y);
    p.image_uv(s.sheet, VRect { w: r.w * share, ..r }, color, Some(uv), 0.0, layer);
}

/// The glyph for a character, and where it sits in a line `height` tall:
/// its top offset and height, as shares of the line.
fn glyph(c: char) -> Option<(&'static str, f32, f32)> {
    Some(match c {
        '0' => ("0", 0.0, 1.0),
        '1' => ("1", 0.0, 1.0),
        '2' => ("2", 0.0, 1.0),
        '3' => ("3", 0.0, 1.0),
        '4' => ("4", 0.0, 1.0),
        '5' => ("5", 0.0, 1.0),
        '6' => ("6", 0.0, 1.0),
        '7' => ("7", 0.0, 1.0),
        '8' => ("8", 0.0, 1.0),
        '9' => ("9", 0.0, 1.0),
        ':' => ("colon", 0.2, 0.62),
        '/' => ("slash", 0.0, 1.0),
        '+' => ("plus", 0.17, 0.66),
        '-' => ("minus", 0.4, 0.22),
        '.' => ("decimal_point", 0.72, 0.28),
        'x' => ("multiply", 0.2, 0.62),
        _ => return None,
    })
}

/// A line of the art's digits, `height` tall with its top at `y`; `align`
/// 0 puts its left at `x`, 1 its right. Returns its width.
#[allow(clippy::too_many_arguments)]
pub(super) fn digits(p: &mut Painter, text: &str, x: f32, y: f32, height: f32, (horz, vert): (u8, u8), align: f32, color: [f32; 4]) -> f32 {
    let gap = height * 0.06;
    let parts: Vec<(&str, f32, f32, f32)> = text
        .chars()
        .filter_map(glyph)
        .map(|(name, top, h)| {
            let s = &sprites()[name];
            let gh = height * h;
            (name, top * height, gh, gh * s.size.x / s.size.y.max(1.0))
        })
        .collect();
    let width = parts.iter().map(|g| g.3).sum::<f32>() + gap * parts.len().saturating_sub(1) as f32;
    let mut at = x - width * align;
    for (name, top, h, w) in parts {
        sprite(p, name, vr(at, y + top, w, h, horz, vert), color, 0.0, 1);
        at += w + gap;
    }
    width
}

// --- the minimap's frame and the compass

/// The minimap: its frame (virtual, top left) and the map inside it.
pub(super) const FRAME: [f32; 4] = [8.0, 8.0, 112.0, 110.0];
pub(super) fn map_rect() -> [f32; 4] {
    // The frame's border, as the art's core rectangle has it (6 of 220).
    let [x, y, w, h] = FRAME;
    let inset = w * 6.0 / 220.0 + 1.0;
    [x + inset, y + inset, w - 2.0 * inset, h - 2.0 * inset]
}
/// Its icons' sizes (virtual units).
pub(super) const PLAYER_ICON: f32 = 11.0;
pub(super) const FRIENDLY_ICON: f32 = 8.0;
pub(super) const ENEMY_ICON: f32 = 6.0;

/// Under the map, and the frame over it. Past the map's edges the frame
/// shows a charcoal plot with a faint grid, not the world behind.
pub(super) fn minimap_frame(p: &mut Painter) {
    let [x, y, w, h] = FRAME;
    p.image("white", vr(x + 2.0, y + 2.0, w - 4.0, h - 4.0, 1, 1), [0.075, 0.075, 0.068, 1.0], 0);
    let [mx, my, mw, mh] = map_rect();
    const LINES: usize = 6;
    for i in 1..LINES {
        let f = i as f32 / LINES as f32;
        p.image("white", vr(mx + mw * f, my, 0.5, mh, 1, 1), [TAN[0], TAN[1], TAN[2], 0.08], 0);
        p.image("white", vr(mx, my + mh * f, mw, 0.5, 1, 1), [TAN[0], TAN[1], TAN[2], 0.08], 0);
    }
    sprite(p, "minimap_frame", vr(x, y, w, h, 1, 1), WHITE, 0.0, 1);
}

/// The compass along the top: the rail, and the points of the compass
/// sliding along it with the player's heading (turns clockwise from north,
/// 0..1).
pub(super) fn compass_rail(p: &mut Painter, heading: f32) {
    let w = 150.0;
    let h = height_for("compass_rail", w);
    let (x, y) = (-w * 0.5, 4.0);
    p.image("white", vr(x + 3.0, y + 3.0, w - 6.0, h - 6.0, 2, 1), [0.07, 0.07, 0.06, 0.55], 1);
    sprite(p, "compass_rail", vr(x, y, w, h, 2, 1), WHITE, 0.0, 1);
    // A quarter turn each way shows.
    const SPAN: f32 = 0.25;
    const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    for (i, name) in POINTS.iter().enumerate() {
        let mut d = i as f32 / 8.0 - heading;
        d -= d.round();
        if d.abs() > SPAN {
            continue;
        }
        let px = d / SPAN * (w * 0.5 - 12.0);
        let fade = (1.0 - (d.abs() / SPAN).powi(2)).clamp(0.0, 1.0);
        let color = if i % 2 == 0 { [IVORY[0], IVORY[1], IVORY[2], fade] } else { [TAN[0], TAN[1], TAN[2], 0.85 * fade] };
        let size = if i % 2 == 0 { 18.0 } else { 14.0 };
        p.text(name, px, y + h * 0.5 + size * 0.38, 2, 1, size, 0, color, 0.5, true);
    }
}

// --- scores and the clock

/// The team scores under the minimap (ours green on top, theirs orange; in
/// free-for-all, the player's and the leader's), and the clock under them.
pub(super) fn scores(p: &mut Painter, ours: i32, theirs: i32, labels: (&str, &str), time_left: f32) {
    let [x, y, w, _] = FRAME;
    let top = y + FRAME[3] + 6.0;
    let h = height_for("team_score_panel", w);
    sprite(p, "team_score_panel", vr(x, top, w, h, 1, 1), WHITE, 0.0, 1);
    // The art's two rows: each a shield, a coloured tick, then the bar.
    for (row, (label, score, color)) in [(labels.0, ours, OLIVE), (labels.1, theirs, ORANGE)].into_iter().enumerate() {
        let row_top = top + h * (0.08 + 0.48 * row as f32);
        let row_h = h * 0.38;
        p.text(label, x + w * 0.25, row_top + row_h * 0.78, 1, 1, row_h * 0.62, 0, [color[0], color[1], color[2], 0.95], 0.0, true);
        digits(p, &score.to_string(), x + w * 0.93, row_top + row_h * 0.16, row_h * 0.68, (1, 1), 1.0, IVORY);
    }
    // The clock: m:ss right of the plate's clock.
    let ty = top + h + 4.0;
    let tw = w * 0.8;
    let th = height_for("match_timer_plate", tw);
    sprite(p, "match_timer_plate", vr(x, ty, tw, th, 1, 1), WHITE, 0.0, 1);
    if time_left.is_finite() {
        let secs = time_left.max(0.0).ceil() as i32;
        let text = format!("{}:{:02}", secs / 60, secs % 60);
        let color = if secs <= 30 { ORANGE } else { IVORY };
        digits(p, &text, x + tw * 0.62, ty + th * 0.26, th * 0.48, (1, 1), 0.5, color);
    }
}

// --- ammo, the weapon and grenades

/// Bottom right: the weapon's name on its plate, a strip of the clip's
/// rounds, the counter (clip / reserve), and the grenades left of it.
pub(super) fn ammo(p: &mut Painter, w: &WeaponState, name: &str, grenades: &[(&'static str, u32)], equipment: Option<(&str, u32)>) {
    let pw = 124.0;
    let ph = height_for("ammo_counter_panel", pw);
    let (px, py) = (-pw - 8.0, -ph - 8.0);
    sprite(p, "ammo_counter_panel", vr(px, py, pw, ph, 3, 3), WHITE, 0.0, 1);
    // The clip big left of the art's slash, the reserve smaller right of it.
    let low = w.clip * 4 <= w.def.clip_size.max(1);
    digits(p, &w.clip.to_string(), px + pw * 0.56, py + ph * 0.2, ph * 0.56, (3, 3), 1.0, if low { ORANGE } else { IVORY });
    digits(p, &w.reserve.to_string(), px + pw * 0.8, py + ph * 0.4, ph * 0.36, (3, 3), 0.5, [IVORY[0], IVORY[1], IVORY[2], 0.85]);
    // The rounds: the strip's ticks lit for what's in the clip.
    let sw = pw * 0.62;
    let sh = height_for("bullet_tick_strip", sw);
    let (sx, sy) = (px + pw - sw - 4.0, py - sh - 1.0);
    sprite(p, "bullet_tick_strip", vr(sx, sy, sw, sh, 3, 3), [1.0, 1.0, 1.0, 0.25], 0.0, 1);
    let share = w.clip as f32 / w.def.clip_size.max(1) as f32;
    sprite_part(p, "bullet_tick_strip", vr(sx, sy, sw, sh, 3, 3), share, if low { ORANGE } else { WHITE }, 1);
    // The weapon's name.
    let nw = pw * 0.9;
    let nh = height_for("weapon_name_plate", nw);
    let (nx, ny) = (px + pw - nw, sy - nh - 1.0);
    sprite(p, "weapon_name_plate", vr(nx, ny, nw, nh, 3, 3), WHITE, 0.0, 1);
    p.text(&name.to_uppercase(), nx + nw * 0.5, ny + nh * 0.68, 3, 3, nh * 0.5, 0, TAN, 0.5, true);
    // Grenades (and the equipment) left of the panel: icon, then "x2".
    let mut gx = px - 6.0;
    let icon_h = ph * 0.82;
    let items = grenades.iter().map(|&(n, c)| (n, c, true)).chain(equipment.map(|(n, c)| (n, c, false)));
    for (icon, count, own_art) in items {
        let count_w = digits(p, &format!("x{count}"), gx, py + ph * 0.48, ph * 0.34, (3, 3), 1.0, [IVORY[0], IVORY[1], IVORY[2], if count > 0 { 0.9 } else { 0.35 }]);
        gx -= count_w + 2.0;
        let alpha = if count > 0 { 1.0 } else { 0.35 };
        if own_art {
            let iw = icon_h * sprites().get(icon).map_or(0.6, |s| s.size.x / s.size.y.max(1.0));
            sprite(p, icon, vr(gx - iw, py + ph * 0.1, iw, icon_h, 3, 3), [1.0, 1.0, 1.0, alpha], 0.0, 1);
            gx -= iw + 8.0;
        } else {
            // The equipment's own icon, from CoD4's.
            p.image(icon, vr(gx - icon_h, py + ph * 0.1, icon_h, icon_h, 3, 3), [1.0, 1.0, 1.0, 0.8 * alpha], 1);
            gx -= icon_h + 8.0;
        }
    }
}

// --- hit marker

pub(super) fn hit_marker(p: &mut Painter, alpha: f32) {
    let size = 26.0;
    sprite(p, "hit_marker", vr(-size * 0.5, -size * 0.5, size, size, 2, 2), [1.0, 1.0, 1.0, alpha], 0.0, 1);
}

// --- kill feed

/// The kill feed: its rows' height, and their text's.
pub(super) const FEED_ROW: (f32, f32) = (26.0, 13.0);

/// One kill feed row's plate, as wide as its line (`w`, from `x`; `y` its
/// top): the art's ends, and a plain stretch of its middle between them
/// (its divided cells would fall anywhere).
pub(super) fn feed_row(p: &mut Painter, x: f32, y: f32, w: f32, alpha: f32) {
    let Some(s) = sprites().get("kill_feed_row") else { return };
    let h = FEED_ROW.0;
    let color = [1.0, 1.0, 1.0, 0.85 * alpha];
    let (u0, u1, v0, v1) = (s.uv.min.x, s.uv.max.x, s.uv.min.y, s.uv.max.y);
    let du = u1 - u0;
    // The ends: 8% of the art each, as wide as they are tall there.
    let cap = h * (s.size.x * 0.08) / s.size.y.max(1.0);
    let mid = (w - 2.0 * cap).max(0.0);
    let part = |a: f32, b: f32| Rect::new(u0 + du * a, v0, u0 + du * b, v1);
    p.image_uv(s.sheet, vr(x, y, cap, h, 1, 3), color, Some(part(0.0, 0.08)), 0.0, 1);
    p.image_uv(s.sheet, vr(x + cap, y, mid, h, 1, 3), color, Some(part(0.1, 0.3)), 0.0, 1);
    p.image_uv(s.sheet, vr(x + cap + mid, y, cap, h, 1, 3), color, Some(part(0.92, 1.0)), 0.0, 1);
}
