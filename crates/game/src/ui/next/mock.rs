//! Mock-ups of key screens in the new design, for the proposal. They use the
//! real kit and real player data where it's at hand (profile, rank, classes,
//! supply drops); the rest is sample data.

use super::kit::{Button, Kit, R};
use super::theme::{self as t, Rgba, Type, col_x, cols};
use super::super::expr::Env;
use bevy::prelude::*;

use super::shell::*;

// --- Private match lobby -----------------------------------------------------

pub fn lobby(k: &mut Kit) {
    backdrop(k, 0.45);
    header(k, "Play", "Private Match");
    let x = col_x(0);
    let w = cols(4);
    let mut y = 192.0;
    section(k, x, y, w, "Match settings");
    y += 34.0;
    let rows = [
        ("Game Mode", "Team Deathmatch"),
        ("Map", "Crash"),
        ("Time Limit", "10 min"),
        ("Score Limit", "750"),
        ("Bots", "5 v 5"),
        ("Bot Difficulty", "Regular"),
        ("Hardcore", "Off"),
        ("Splitscreen", "Off"),
    ];
    for (i, (l, v)) in rows.iter().enumerate() {
        row(k, R::new(x, y, w, t::ROW_H), l, Some(v), i == 1);
        y += t::ROW_H + 6.0;
    }
    // Start, and the online buttons.
    y += 18.0;
    let start = R::new(x, y, w, 76.0);
    k.cut_box(start, t::CUT, t::ACCENT);
    k.text_mid(start.x + 28.0, start.cy(), Type { size: 32.0, ..t::H1 }, "Start Match", t::INK_ON_ACCENT, 0);
    k.chevron(Vec2::new(start.right() - 32.0, start.cy()), 22.0, 0, 3.0, t::INK_ON_ACCENT);
    y += 92.0;
    let half = (w - 12.0) * 0.5;
    for (i, label) in ["Invite Friends", "Join with Code"].iter().enumerate() {
        let r = R::new(x + i as f32 * (half + 12.0), y, half, 56.0);
        k.fill(r, t::hex(0x000000, 0.35));
        k.frame(r, 1.5, t::LINE);
        k.text_mid(r.cx(), r.cy(), Type { size: 22.0, ..t::ROW_TEXT }, label, t::INK, 1);
    }

    // The map, with the lobby code.
    let mx = col_x(4);
    let mw = cols(8);
    let map = R::new(mx, 192.0, mw, 236.0);
    k.fill(map, t::RAISED);
    k.pic_uv(map, "loadscreen_mp_crash", Rect::new(0.0, 0.3, 1.0, 0.72), [1.0; 4]);
    k.fill(map, t::hex(0x050607, 0.3));
    k.grad_h(map, t::hex(0x050607, 1.0), 0.95, 0.1);
    k.text(map.x + 32.0, map.y + 56.0, t::LABEL, "Team Deathmatch  ·  10 min  ·  750", t::ACCENT, 0);
    k.text(map.x + 32.0, map.y + 84.0, Type { size: 72.0, ..t::DISPLAY }, "Crash", t::INK, 0);
    k.text(map.x + 32.0, map.y + 168.0, t::BODY, "Two teams. First to the score limit wins.", t::INK_DIM, 0);
    k.frame(map, 1.0, t::LINE);
    let code = R::new(map.right() - 300.0, map.y + 24.0, 276.0, 64.0);
    k.fill(code, t::hex(0x050607, 0.85));
    k.frame(code, 1.0, t::LINE);
    k.text(code.x + 18.0, code.y + 10.0, t::MICRO, "Lobby code", t::INK_DIM, 0);
    k.text(code.x + 18.0, code.y + 28.0, Type { size: 28.0, tracking: 0.18, ..t::NUMERAL }, "K7QX 9P2M", t::INK, 0);

    // The teams.
    let tw = (mw - t::GUTTER) * 0.5;
    let me = k.fe.profile_name();
    let rank = super::super::combat_record::rank(k.fe);
    let teams: [(&str, Rgba, Vec<(i32, String, &str)>); 2] = [
        (
            "Marines",
            t::FRIENDLY,
            vec![
                (rank.level, me.clone(), "Host"),
                (41, "Ghost".into(), ""),
                (12, "Player 2".into(), "Pad"),
                (0, "Bot  ·  Gaz".into(), "Bot"),
                (0, "Bot  ·  Griggs".into(), "Bot"),
                (0, "Bot  ·  Soap".into(), "Bot"),
            ],
        ),
        (
            "OpFor",
            t::ENEMY,
            vec![
                (0, "Bot  ·  Zakhaev".into(), "Bot"),
                (0, "Bot  ·  Al-Asad".into(), "Bot"),
                (0, "Bot  ·  Viktor".into(), "Bot"),
                (0, "Bot  ·  Yuri".into(), "Bot"),
                (0, "Bot  ·  Kamarov".into(), "Bot"),
            ],
        ),
    ];
    for (i, (name, colour, players)) in teams.iter().enumerate() {
        let tx = mx + i as f32 * (tw + t::GUTTER);
        let mut ty = 452.0;
        k.fill(R::new(tx, ty, 4.0, 40.0), *colour);
        k.text_mid(tx + 18.0, ty + 20.0, t::H1, name, t::INK, 0);
        k.text_mid(tx + tw, ty + 20.0, t::H2, &format!("{}/9", players.len()), t::INK_DIM, 2);
        ty += 56.0;
        for slot in 0..9 {
            let r = R::new(tx, ty, tw, 46.0);
            match players.get(slot) {
                Some((level, who, tag)) => {
                    k.fill(r, if slot == 0 && i == 0 { t::hex(0xffffff, 0.08) } else { t::ROW });
                    if *level > 0 {
                        k.text_mid(r.x + 16.0, r.cy(), t::MICRO, &level.to_string(), t::INK_DIM, 0);
                    }
                    let ink = if tag == &"Bot" { t::INK_DIM } else { t::INK };
                    k.text_mid(r.x + 56.0, r.cy(), Type { size: 22.0, ..t::ROW_TEXT }, who, ink, 0);
                    if !tag.is_empty() {
                        let tag_w = k.measure(t::MICRO, tag) + 16.0;
                        let tr = R::new(r.right() - tag_w - 12.0, r.cy() - 11.0, tag_w, 22.0);
                        let c = if *tag == "Host" { t::ACCENT } else { t::LINE };
                        k.frame(tr, 1.0, c);
                        k.text_mid(tr.cx(), tr.cy(), t::MICRO, tag, if *tag == "Host" { t::ACCENT } else { t::INK_DIM }, 1);
                    }
                }
                None => {
                    k.frame(r, 1.0, t::hex(0xffffff, 0.05));
                    k.text_mid(r.x + 56.0, r.cy(), Type { size: 20.0, ..t::ROW_TEXT }, "Open", t::INK_MUTE, 0);
                }
            }
            ty += 50.0;
        }
    }
    footer(k, "", &[(Button::Confirm, "Change"), (Button::Alt, "Switch Team"), (Button::Alt2, "Invite"), (Button::Back, "Leave Lobby")]);
}

// --- Create a Class ------------------------------------------------------------

/// A statstable item's name and picture.
fn item(k: &Kit, reference: &str) -> (String, String) {
    let fe = k.fe;
    let name = fe.table_lookup("mp/statstable.csv", 4, reference, 3);
    let name = if name.is_empty() { reference.to_owned() } else { fe.assets.localize(&format!("@{name}")) };
    (name, fe.table_lookup("mp/statstable.csv", 4, reference, 6))
}

fn attachment_name(a: &str) -> &str {
    match a {
        "reflex" => "Red Dot Sight",
        "silencer" => "Suppressor",
        "acog" => "ACOG Scope",
        "grip" => "Grip",
        "gl" => "Grenade Launcher",
        _ => a,
    }
}

/// A card's frame: resting or focused (accent foot and corner brackets).
fn card(k: &mut Kit, r: R, focused: bool) {
    k.fill(r, if focused { t::hex(0x1b2122, 0.92) } else { t::PANEL });
    if focused {
        k.frame(r, 1.5, t::LINE_STRONG);
        k.brackets(r.inset(-5.0), 18.0, 2.5, t::ACCENT);
    } else {
        k.frame(r, 1.0, t::HAIRLINE);
    }
}

fn chip(k: &mut Kit, x: f32, y: f32, s: &str, accent: bool) -> f32 {
    let ty = Type { size: 18.0, ..t::CAPTION };
    let w = k.measure(ty, s) + 24.0;
    let r = R::new(x, y, w, 32.0);
    k.fill(r, t::hex(0xffffff, 0.06));
    k.frame(r, 1.0, if accent { t::ACCENT_DIM } else { t::HAIRLINE });
    k.text_mid(r.x + 12.0, r.cy(), ty, s, t::INK, 0);
    w
}

fn stat_bar(k: &mut Kit, x: f32, y: f32, w: f32, label: &str, v: f32, delta: f32) {
    k.text(x, y, t::MICRO, label, t::INK_DIM, 0);
    let bar = R::new(x + 150.0, y + 4.0, w - 150.0, 6.0);
    k.fill(bar, t::hex(0xffffff, 0.12));
    k.fill(R::new(bar.x, bar.y, bar.w * v, bar.h), t::INK);
    if delta > 0.0 {
        k.fill(R::new(bar.x + bar.w * v, bar.y, bar.w * delta, bar.h), t::ACCENT);
    }
    for i in 1..10 {
        let tx = bar.x + bar.w * i as f32 / 10.0;
        k.vline(tx, bar.y, bar.bottom(), 2.0, t::hex(0x0e1213, 1.0));
    }
}

pub fn cac(k: &mut Kit) {
    backdrop(k, 0.55);
    header(k, "Loadout", "Create a Class");
    // The classes.
    let x = col_x(0);
    let w = cols(3);
    let mut y = 192.0;
    section(k, x, y, w, "Custom classes");
    y += 34.0;
    let loadouts: Vec<_> = (1..=5).map(|n| k.fe.class_loadout(&format!("custom{n}"))).collect();
    for (i, l) in loadouts.iter().enumerate() {
        let name = l.as_ref().map_or_else(|| format!("Custom Slot {}", i + 1), |l| l.name.clone());
        row(k, R::new(x, y, w, 52.0), &name, None, i == 0);
        y += 58.0;
    }
    y += 24.0;
    section(k, x, y, w, "Default classes");
    y += 34.0;
    for name in ["Grenadier", "First Recon", "Overwatch", "Demolitions", "Sniper"] {
        let r = R::new(x, y, w, 52.0);
        row(k, r, name, None, false);
        k.lock(Vec2::new(r.right() - 24.0, r.cy()), 18.0, t::INK_MUTE);
        y += 58.0;
    }

    // The class.
    let Some(Some(class)) = loadouts.first() else { return };
    let cx = col_x(3);
    let cw = cols(6);
    k.text(cx, 186.0, t::H1, &class.name, t::INK, 0);
    k.prompts_right(cx + cw, 206.0, &[(Button::Alt, "Rename")]);

    // Primary.
    let p = R::new(cx, 248.0, cw, 304.0);
    card(k, p, true);
    k.text(p.x + 28.0, p.y + 24.0, t::LABEL, "Primary", t::ACCENT, 0);
    if let Some(g) = class.guns.first() {
        let (weapon, atts) = g.spec.split_once(':').unwrap_or((&g.spec, ""));
        k.text(p.x + 28.0, p.y + 50.0, Type { size: 48.0, ..t::H1 }, &g.name, t::INK, 0);
        let mut chip_x = p.x + 28.0;
        for a in atts.split('+').filter(|a| !a.is_empty()) {
            chip_x += chip(k, chip_x, p.y + 112.0, attachment_name(a), false) + 8.0;
        }
        if g.camo & 0xffff != 0 {
            chip(k, chip_x, p.y + 112.0, "Camo", true);
        }
        let (_, picture) = item(k, weapon);
        k.gun(R::new(p.x + p.w * 0.36, p.y + 70.0, p.w * 0.62, 200.0), 201, &g.spec, g.camo, &picture);
        let sx = p.x + 28.0;
        let sw = p.w * 0.42;
        for (i, (l, v, d)) in [("Accuracy", 0.7, 0.0), ("Damage", 0.5, 0.1), ("Range", 0.6, 0.0), ("Fire rate", 0.8, 0.0), ("Mobility", 0.7, 0.0)].iter().enumerate() {
            stat_bar(k, sx, p.y + 170.0 + i as f32 * 24.0, sw, l, *v, *d);
        }
    }
    // Secondary.
    let s = R::new(cx, p.bottom() + 16.0, cw, 168.0);
    card(k, s, false);
    k.text(s.x + 28.0, s.y + 24.0, t::LABEL, "Secondary", t::INK_DIM, 0);
    if let Some(g) = class.guns.get(1) {
        let (weapon, atts) = g.spec.split_once(':').unwrap_or((&g.spec, ""));
        k.text(s.x + 28.0, s.y + 50.0, t::H1, &g.name, t::INK, 0);
        let mut chip_x = s.x + 28.0;
        for a in atts.split('+').filter(|a| !a.is_empty()) {
            chip_x += chip(k, chip_x, s.y + 104.0, attachment_name(a), false) + 8.0;
        }
        let (_, picture) = item(k, weapon);
        k.gun(R::new(s.x + s.w * 0.42, s.y + 4.0, s.w * 0.56, 160.0), 203, &g.spec, g.camo, &picture);
    }
    // Stats legend / mastery strip.
    let m = R::new(cx, s.bottom() + 16.0, cw, 96.0);
    card(k, m, false);
    k.text(m.x + 28.0, m.y + 20.0, t::LABEL, "Weapon mastery", t::INK_DIM, 0);
    k.text(m.x + 28.0, m.y + 46.0, t::H2, "Gold camo  ·  87 / 150 headshots", t::INK, 0);
    let bar = R::new(m.x + m.w * 0.62, m.y + 58.0, m.w * 0.34, 6.0);
    k.fill(bar, t::hex(0xffffff, 0.12));
    k.fill(R::new(bar.x, bar.y, bar.w * 0.58, bar.h), t::REWARD);

    // Perks and the special grenade.
    let px = col_x(9);
    let pw = cols(3);
    let mut py = 248.0;
    let mut perks: Vec<(String, String, String)> = class
        .perks
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (n, pic) = item(k, p);
            (format!("Perk {}", i + 1), n, pic)
        })
        .collect();
    if let Some(g) = &class.special {
        let (n, pic) = item(k, g);
        perks.push(("Special grenade".into(), n, pic));
    }
    for (label, name, pic) in &perks {
        let r = R::new(px, py, pw, 128.0);
        card(k, r, false);
        k.text(r.x + 24.0, r.y + 20.0, t::LABEL, label, t::INK_DIM, 0);
        k.para(r.x + 24.0, r.y + 48.0, r.w - 140.0, Type { size: 26.0, ..t::H2 }, name, t::INK, 1.1);
        if !pic.is_empty() {
            k.pic(R::new(r.right() - 104.0, r.y + 24.0, 80.0, 80.0), pic, [1.0; 4]);
        }
        py += 140.0;
    }
    footer(k, "", &[(Button::Confirm, "Edit"), (Button::NextTab, "Next Class"), (Button::View, "Inspect"), (Button::Back, "Back")]);
}

// --- HUD --------------------------------------------------------------------

/// The HUD with sample values; the minimap's frame goes around the match's
/// own minimap (window height `h`).
pub fn hud(k: &mut Kit, h: f32, minimap: Option<(Vec2, Vec2)>) {
    let b = k.v.bleed();
    let sx = t::SAFE_X * 0.6;
    let top = t::SAFE_Y * 0.6;
    // The minimap where the match draws it (CoD4's 640x480 rect, left
    // aligned): framed, with N, and the score under it.
    let s4 = h / 480.0;
    let to_design = |px: Vec2| (px - k.v.o) / k.v.s;
    // (As hud.rs places it: CoD4's rect, or the Modern HUD's frame's hole.)
    let [mx, my, mw, mh] = if crate::settings_apply::modern_hud() {
        let inset = 112.0 * 6.0 / 220.0 + 1.0;
        [8.0 + inset, 8.0 + inset, 112.0 - 2.0 * inset, 110.0 - 2.0 * inset]
    } else {
        [6.0, 18.0, 102.0, 102.0]
    };
    let (pa, pz) = minimap.map_or((Vec2::new(mx, my) * s4, Vec2::new(mx + mw, my + mh) * s4), |(p, s)| (p, p + s));
    let (a, z) = (to_design(pa), to_design(pz));
    let map = R::new(a.x, a.y, z.x - a.x, z.y - a.y);
    k.fill_under(map.inset(-3.0), t::hex(0x07090a, 0.6));
    k.frame(map.inset(-3.0), 1.5, t::LINE_STRONG);
    k.brackets(map.inset(-3.0), 16.0, 3.0, t::INK);
    k.text_mid(map.cx(), map.y - 14.0, t::MICRO, "N", t::INK, 1);

    // Score: mode, ours, theirs, clock.
    let sy = map.bottom() + 18.0;
    let row_h = 34.0;
    for (i, (team, score, colour, f)) in [("Marines", 45, t::FRIENDLY, 0.6), ("OpFor", 32, t::ENEMY, 0.43)].iter().enumerate() {
        let r = R::new(map.x - 3.0, sy + i as f32 * (row_h + 6.0), map.w + 6.0, row_h);
        k.fill(r, t::hex(0x07090a, 0.6));
        k.fill(R::new(r.x, r.y, 4.0, r.h), *colour);
        k.fill(R::new(r.x + 4.0, r.bottom() - 3.0, (r.w - 4.0) * f, 3.0), t::with_alpha(*colour, 0.7));
        k.text_mid(r.x + 16.0, r.cy(), t::MICRO, team, t::INK_DIM, 0);
        k.text_mid(r.right() - 12.0, r.cy(), Type { size: 26.0, ..t::NUMERAL }, &score.to_string(), t::INK, 2);
    }
    let ty = sy + 2.0 * (row_h + 6.0) + 4.0;
    k.text(map.x, ty, t::MICRO, "TDM  ·  750", t::INK_MUTE, 0);
    k.text(map.right(), ty - 4.0, Type { size: 26.0, ..t::NUMERAL }, "9:54", t::INK, 2);

    // The compass rail, top centre.
    let cx = 960.0;
    let rail = R::new(cx - 300.0, top, 600.0, 34.0);
    let heading = 52.0f32;
    k.grad_h(R::new(rail.x - 60.0, rail.y - 4.0, 360.0, 42.0), t::hex(0x07090a, 1.0), 0.0, 0.45);
    k.grad_h(R::new(cx, rail.y - 4.0, 360.0, 42.0), t::hex(0x07090a, 1.0), 0.45, 0.0);
    for d in (-60..=60).step_by(5) {
        let deg = heading + d as f32;
        let snapped = (deg / 5.0).round() * 5.0;
        let x = cx + (snapped - heading) * 5.0;
        if (x - cx).abs() > 300.0 {
            continue;
        }
        let fade = 1.0 - ((x - cx).abs() / 300.0).powi(2);
        let n = (snapped.rem_euclid(360.0)) as i32;
        let major = n % 45 == 0;
        let len = if major { 12.0 } else if n % 15 == 0 { 8.0 } else { 4.0 };
        k.vline(x, rail.y + 20.0, rail.y + 20.0 + len, 1.5, t::with_alpha(t::INK, 0.75 * fade));
        if major {
            let name = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"][(n / 45) as usize % 8];
            k.text_mid(x, rail.y + 6.0, t::MICRO, name, t::with_alpha(t::INK, fade), 1);
        }
    }
    let hb = R::new(cx - 30.0, rail.bottom() + 8.0, 60.0, 26.0);
    k.fill(hb, t::hex(0x07090a, 0.6));
    k.text_mid(cx, hb.cy(), Type { size: 20.0, ..t::NUMERAL }, &format!("{:03}", heading as i32), t::INK, 1);
    k.vline(cx, rail.y + 16.0, rail.y + 36.0, 2.0, t::ACCENT);

    // Ammo, bottom right.
    let right = b.right().min(1920.0) - sx;
    let bottom = b.bottom().min(1080.0) - t::SAFE_Y * 0.7;
    k.glow(R::new(right - 420.0, bottom - 230.0, 560.0, 300.0), t::hex(0x07090a, 0.55));
    k.text(right, bottom - 160.0, t::LABEL, "M4 Carbine", t::INK, 2);
    k.text(right, bottom - 138.0, t::MICRO, "Red Dot  ·  Suppressor", t::INK_MUTE, 2);
    let rw = k.text(right, bottom - 94.0, Type { size: 32.0, ..t::NUMERAL }, "90", t::INK_DIM, 2);
    k.text(right - rw - 10.0, bottom - 116.0, Type { size: 72.0, ..t::NUMERAL }, "30", t::INK, 2);
    // Rounds in the magazine.
    let pips = 30;
    for i in 0..pips {
        let x = right - 6.0 - i as f32 * 7.0;
        k.fill(R::new(x, bottom - 34.0, 4.0, 14.0), if i < 26 { t::INK } else { t::hex(0xffffff, 0.2) });
    }
    // Lethal and tactical.
    for (i, (key, name, n)) in [("G", "Frag", 1), ("4", "Flash", 2)].iter().enumerate() {
        let x = right - 330.0 - i as f32 * 120.0;
        let r = R::new(x, bottom - 92.0, 104.0, 44.0);
        k.fill(r, t::hex(0x07090a, 0.5));
        k.frame(r, 1.0, t::HAIRLINE);
        k.text_mid(r.x + 12.0, r.cy(), t::MICRO, name, t::INK_DIM, 0);
        k.text_mid(r.right() - 12.0, r.cy(), Type { size: 22.0, ..t::NUMERAL }, &n.to_string(), t::INK, 2);
        let kw = k.measure(t::MICRO, key) + 12.0;
        let kr = R::new(r.cx() - kw * 0.5, r.y - 14.0, kw, 20.0);
        k.fill(kr, t::hex(0x07090a, 0.85));
        k.frame(kr, 1.0, t::LINE);
        k.text_mid(kr.cx(), kr.cy(), t::MICRO, key, t::INK, 1);
    }

    // Killstreaks, bottom left, the feed over them.
    let left = b.x.max(0.0) + sx;
    for (i, (name, need, have)) in [("UAV", 3, 2), ("Airstrike", 5, 2), ("Helicopter", 7, 2)].iter().enumerate() {
        let r = R::new(left + i as f32 * 128.0, bottom - 64.0, 120.0, 64.0);
        let ready = have >= need;
        k.fill(r, t::hex(0x07090a, 0.55));
        k.frame(r, 1.0, if ready { t::ACCENT } else { t::HAIRLINE });
        k.text(r.x + 12.0, r.y + 10.0, t::MICRO, name, if ready { t::ACCENT } else { t::INK_DIM }, 0);
        for p in 0..*need {
            let pr = R::new(r.x + 12.0 + p as f32 * 14.0, r.bottom() - 18.0, 10.0, 6.0);
            k.fill(pr, if p < *have { t::INK } else { t::hex(0xffffff, 0.15) });
        }
        k.text(r.right() - 12.0, r.y + 30.0, Type { size: 22.0, ..t::NUMERAL }, &need.to_string(), t::INK_MUTE, 2);
    }
    let me = k.fe.profile_name();
    let feed = [(me.as_str(), "M4", "Zakhaev", true), ("Ghost", "M40A3", "Al-Asad", true), ("Viktor", "AK-47", "Gaz", false)];
    for (i, (a, gun, v, ours)) in feed.iter().enumerate() {
        let y = bottom - 120.0 - i as f32 * 36.0;
        let ty = Type { size: 20.0, ..t::ROW_TEXT };
        let (ca, cv) = if *ours { (t::FRIENDLY, t::ENEMY) } else { (t::ENEMY, t::FRIENDLY) };
        let w1 = k.measure(ty, a);
        let w2 = k.measure(t::MICRO, gun) + 16.0;
        let w3 = k.measure(ty, v);
        let r = R::new(left, y, w1 + w2 + w3 + 48.0, 30.0);
        k.grad_h(r, t::hex(0x07090a, 1.0), 0.6, 0.0);
        k.text_mid(left + 10.0, r.cy(), ty, a, ca, 0);
        let gr = R::new(left + 20.0 + w1, r.cy() - 10.0, w2, 20.0);
        k.frame(gr, 1.0, t::LINE);
        k.text_mid(gr.cx(), gr.cy(), t::MICRO, gun, t::INK_DIM, 1);
        k.text_mid(gr.right() + 10.0, r.cy(), ty, v, cv, 0);
    }

    // Crosshair, a hit marker, XP.
    let c = Vec2::new(960.0, 540.0);
    for d in [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y] {
        k.line(c + d * 14.0, c + d * 30.0, 2.0, t::with_alpha(t::INK, 0.9));
    }
    for d in [Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0), Vec2::new(1.0, -1.0), Vec2::new(-1.0, -1.0)] {
        let d = d.normalize();
        k.line(c + d * 10.0, c + d * 22.0, 2.5, t::INK);
    }
    k.text(c.x + 60.0, c.y - 40.0, Type { size: 30.0, ..t::NUMERAL }, "+100", t::REWARD, 0);
    k.text(c.x + 60.0, c.y - 6.0, t::MICRO, "Headshot", t::INK, 0);
}
