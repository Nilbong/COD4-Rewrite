//! The Combat Record and its emblem editor in the new UI's look. The classic
//! screens' buttons and fields (built in `menu.rs`) are kept and moved into
//! the new layout: the tabs and their rows down the left column, the record
//! in the panel on the right; everything is drawn here from the same data.

use super::*;
use crate::ui::next::home::virtual_rect;
use crate::ui::next::kit::{Button, HUD_ICONS, Kit, MENU_ICONS, R, View, icons};
use crate::ui::next::shell::*;
use crate::ui::next::theme::{self as t, Type, col_x, cols};

pub(super) const RECORD_VIEW: &str = "next_record";
pub(super) const EMBLEM_VIEW: &str = "next_emblem";

/// The right panel.
const PANEL: R = R::new(826.0, 192.0, 998.0, 780.0);

/// The classic layout's coordinates (the panel's `rect()` units) to the
/// new panel's.
fn from_classic(r: &VRect) -> R {
    if r.horz_align == 1 {
        // A left-column row.
        return R::new(col_x(0), 226.0 + (r.y - 34.0) * 1.95, cols(4), 42.0);
    }
    let lx = (r.x + 386.0) / BODY_SCALE - 440.0;
    let (sx, sy) = (PANEL.w / 420.0, PANEL.h / 382.0);
    R::new(PANEL.x + (lx + 440.0) * sx, PANEL.y + (r.y - 34.0) * sy, r.w / BODY_SCALE * sx, r.h * sy)
}

/// The emblem editor's canvas and palette.
const CANVAS: R = R::new(826.0, 236.0, 640.0, 640.0);
fn cell(i: usize) -> R {
    let c = CANVAS.w / GRID as f32;
    R::new(CANVAS.x + (i % GRID) as f32 * c, CANVAS.y + (i / GRID) as f32 * c, c, c)
}
fn swatch(i: usize) -> R {
    R::new(1510.0 + (i % 4) as f32 * 78.0, 280.0 + (i / 4) as f32 * 78.0, 66.0, 66.0)
}

/// The left column's groups by the classic layout's y, and their titles.
fn group_of(y: f32) -> usize {
    if y < 170.0 {
        0
    } else if y < 360.0 {
        1
    } else {
        3
    }
}

fn group_title(group: usize, tab: usize, editor: bool) -> &'static str {
    match (group, tab, editor) {
        (0, _, true) => "Emblem",
        (0, _, _) => "Record",
        (1, 2, _) => "Filter",
        (1, 3, _) => "Edit",
        _ => "Pages",
    }
}

const LEFT_TOP: f32 = 176.0;

/// The left column's places: (group header y, title), and each classic
/// row's (classic y -> rect).
fn left_layout(ys: &[f32], tab: usize, editor: bool) -> (Vec<(f32, &'static str)>, Vec<(f32, R)>) {
    let mut ys = ys.to_vec();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ys.dedup();
    let groups = ys.iter().map(|&y| group_of(y)).collect::<std::collections::BTreeSet<_>>().len();
    // Main menu rows (48 on a 50 pitch) when they fit, else a little tighter.
    let fit = |pitch: f32| LEFT_TOP + groups as f32 * 34.0 + (groups.saturating_sub(1)) as f32 * 22.0 + ys.len() as f32 * pitch <= 980.0;
    let pitch = if fit(50.0) { 50.0 } else if fit(46.0) { 46.0 } else { 43.0 };
    let mut heads = Vec::new();
    let mut rows = Vec::new();
    let mut y = LEFT_TOP;
    let mut group = None;
    for cy in ys {
        let g = group_of(cy);
        if group != Some(g) {
            if group.is_some() {
                y += 22.0;
            }
            heads.push((y, group_title(g, tab, editor)));
            y += 34.0;
            group = Some(g);
        }
        rows.push((cy, R::new(col_x(0), y, cols(4), pitch - 2.0)));
        y += pitch;
    }
    (heads, rows)
}

/// The classic screen's buttons and fields, moved; its art left out.
pub(super) fn restyle(menu: Menu, editor: bool, tab: usize) -> Menu {
    let mut out = Menu { items: Vec::new(), ..menu.clone() };
    out.window.name = if editor { EMBLEM_VIEW } else { RECORD_VIEW }.into();
    let left_ys: Vec<f32> = menu
        .items
        .iter()
        .filter(|it| it.ty == item_type::BUTTON && it.window.rect.horz_align == 1 && !it.window.name.eq_ignore_ascii_case("back"))
        .map(|it| it.window.rect.y)
        .collect();
    let (heads, left) = left_layout(&left_ys, tab, editor);
    for (y, title) in heads {
        let mut h = Item::default();
        h.window.name = format!("crsec_{title}");
        h.window.rect = virtual_rect(R::new(col_x(0), y, cols(4), 30.0));
        out.items.push(h);
    }
    for it in menu.items {
        if it.ty != item_type::BUTTON && it.ty != item_type::EDITFIELD {
            continue;
        }
        if it.window.name.eq_ignore_ascii_case("back") {
            continue;
        }
        let r = if let Some(i) = it.window.name.strip_prefix("pixel_").and_then(|n| n.parse::<usize>().ok()) {
            cell(i)
        } else if let Some(i) = it.action.split("combatColor").nth(1).and_then(|s| s.trim().split(|c: char| !c.is_ascii_digit()).next()?.parse::<usize>().ok()) {
            swatch(i)
        } else if it.window.rect.horz_align == 1 {
            left.iter().find(|(y, _)| (*y - it.window.rect.y).abs() < 0.5).map_or_else(|| from_classic(&it.window.rect), |(_, r)| *r)
        } else if it.window.rect.horz_align == 3 && it.window.rect.w / BODY_SCALE >= 390.0 {
            // A full-width row of the panel: the panel's width.
            let r = from_classic(&it.window.rect);
            R::new(PANEL.x, r.y, PANEL.w, r.h)
        } else if it.ty == item_type::EDITFIELD {
            let mut r = from_classic(&it.window.rect);
            r.h = 56.0;
            r
        } else {
            from_classic(&it.window.rect)
        };
        let mut item = it.clone();
        item.window.rect = virtual_rect(r);
        item.window.style = 0;
        item.window.border = 0;
        item.window.background = None;
        item.text_scale = 0.0;
        out.items.push(item);
    }
    if let Some(template) = out.items.iter().find(|it| it.ty == item_type::BUTTON).cloned() {
        out.items.push(crate::ui::next::cac::back_item(&template, &menu.on_esc));
    }
    out
}

fn rgba(c: [f32; 4]) -> t::Rgba {
    c
}

/// A row's place back in design units.
fn place_of(it: &Item) -> R {
    let r = it.window.rect;
    let k = 1080.0 / 480.0;
    R::new(r.x * k + 960.0, r.y * k, r.w * k, r.h * k)
}

pub(super) fn paint(fe: &Frontend, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
    let editor = om.menu.window.name == EMBLEM_VIEW;
    let mut k = Kit::new(fe, ops, View::new(Vec2::ZERO, Vec2::new(pl.w, pl.h)));
    let b = k.v.bleed();
    k.fill(b, t::hex(0x050607, 1.0));
    const ASPECT: f32 = 1672.0 / 941.0;
    let (w, h) = if b.w / b.h > ASPECT { (b.w, b.w / ASPECT) } else { (b.h * ASPECT, b.h) };
    k.pic(R::new(b.cx() - w * 0.5, b.cy() - h * 0.5, w, h), "next:bg_barracks", [1.0; 4]);
    k.grad_h(R::new(b.x, b.y, b.w * 0.5, b.h), t::hex(0x050607, 1.0), 0.9, 0.0);
    k.fill(b, t::hex(0x050607, 0.45));
    let hot = fe.focus.as_ref().filter(|(m, _)| *m == om.name).and_then(|(_, i)| om.menu.items.get(*i));
    let hot_name = hot.map(|it| (it.window.name.clone(), it.action.clone()));
    let is_hot = |it: &Item| hot_name.as_ref().is_some_and(|(n, a)| *n == it.window.name && *a == it.action);
    let tab = fe.dvar("cr_tab").parse::<usize>().unwrap_or(0).min(TABS.len() - 1);
    let filter = fe.dvar("cr_filter").parse::<usize>().unwrap_or(0);

    if editor {
        header(&mut k, "Combat Record  /  Identity", "Emblem Editor");
    } else {
        header(&mut k, "Barracks", "Combat Record");
    }

    // The left column's rows: tabs (the current one marked), then the
    // tab's own (filters, actions, pages).
    for it in om.menu.items.iter().filter(|it| it.window.name.starts_with("crsec_")) {
        let r = place_of(it);
        section(&mut k, r.x, r.y, r.w, it.window.name.trim_start_matches("crsec_"));
    }
    let rows: Vec<&Item> = om.menu.items.iter().filter(|it| it.window.rect.horz_align == 2 && place_of(it).x < 700.0 && it.ty == item_type::BUTTON && it.window.name != "cac_back").collect();
    for it in &rows {
        let r = place_of(it);
        let label = it.text.clone();
        let current = it.action.contains(&format!("combatTab {tab}")) && !editor && TABS.contains(&label.as_str())
            || it.action.contains(&format!("combatFilter {filter}"));
        menu_row(&mut k, r, &label, is_hot(it), None);
        let icon = match label.as_str() {
            "Overview" => Some(icons::HOME),
            "Weapons" => Some(icons::RIFLE),
            "Challenges" => Some(icons::TROPHY),
            "Identity" => Some(icons::HELMET),
            "Calling Cards" => Some(icons::STAR),
            "All" | "All weapons" => Some(icons::CHECK),
            "Attachments" => Some(icons::CROSSHAIR),
            "Camos" => Some(icons::CRATE),
            "Career" => Some(icons::TROPHY),
            "Save Name & Clan Tag" | "Save Emblem" => Some(icons::CHECK),
            "Edit Emblem" => Some(icons::STAR),
            "Choose Calling Card" => Some(icons::PEOPLE),
            _ => None,
        };
        if let Some(i) = icon {
            let x = r.right() - if is_hot(it) { 78.0 } else { 46.0 };
            k.icon(MENU_ICONS, i, R::new(x, r.cy() - 11.0, 22.0, 22.0), if is_hot(it) { t::INK } else { t::with_alpha(t::INK_DIM, 0.6) });
        } else if label == "Previous" || label == "Previous gun" {
            k.chevron(Vec2::new(r.right() - 34.0, r.cy()), 16.0, 2, 2.5, t::INK_DIM);
        } else if label == "Next" {
            k.chevron(Vec2::new(r.right() - 34.0, r.cy()), 16.0, 0, 2.5, t::INK_DIM);
        }
        if current && !is_hot(it) {
            // The open tab / filter: the label lit and a tick.
            k.text_mid(r.x + 24.0, r.cy(), Type { size: 28.0, ..t::ROW_TEXT }, &label, t::ACCENT, 0);
        }
    }

    if editor {
        emblem_editor(&mut k, fe, om, &is_hot);
    } else {
        match tab {
            1 => weapons(&mut k, fe, om, &is_hot),
            2 => challenges(&mut k, fe, om, &is_hot),
            3 => identity_tab(&mut k, fe, om, &is_hot),
            4 => cards(&mut k, fe, om, &is_hot),
            _ => overview(&mut k, fe),
        }
    }
    crate::ui::next::cac::back_button(&mut k, hot_name.as_ref().is_some_and(|(n, _)| n == "cac_back"));
    footer(&mut k, "", &[(Button::Confirm, "Select"), (Button::Back, "Back")]);
}

/// A calling card's art tiled across `r`.
fn card_art(k: &mut Kit, r: R, index: usize, earned: bool) {
    let tile = r.h;
    let n = (r.w / tile).ceil() as usize;
    let c = if earned { [0.62, 0.62, 0.62, 1.0] } else { [0.28, 0.28, 0.28, 1.0] };
    for i in 0..n {
        let x = r.x + i as f32 * tile;
        let w = (r.right() - x).min(tile);
        k.pic_uv(R::new(x, r.y, w, r.h), CARDS[index].material, bevy::math::Rect::new(0.0, 0.0, w / tile, 1.0), c);
    }
}

/// An emblem's pixels in `r`.
fn emblem(k: &mut Kit, pixels: &[u8; PIXELS], r: R, grid: bool) {
    let c = r.w / GRID as f32;
    k.fill(r, t::hex(0x000000, 0.4));
    for (i, &p) in pixels.iter().enumerate() {
        let cr = R::new(r.x + (i % GRID) as f32 * c, r.y + (i / GRID) as f32 * c, c, c);
        let colour = if p == 0 && grid {
            if ((i % GRID) + i / GRID) % 2 == 0 { t::hex(0x2a2f31, 1.0) } else { t::hex(0x202426, 1.0) }
        } else {
            rgba(PALETTE[usize::from(p.min(15))])
        };
        if colour[3] > 0.0 {
            k.fill(if grid { cr.inset(0.5) } else { cr }, colour);
        }
    }
    k.frame(r, 1.5, t::LINE);
}

/// The player banner: calling card, emblem, name, rank and XP.
fn banner(k: &mut Kit, fe: &Frontend, r: R) {
    let card = selected_card(&fe.stats);
    card_art(k, r, card, true);
    k.grad_h(r, t::hex(0x050607, 1.0), 0.85, 0.2);
    grid_frame(k, r.inset(-6.0));
    let rank = rank(fe);
    let e = R::new(r.x + 32.0, r.y + 28.0, 120.0, 120.0);
    match fe.stats.dvars.get(EMBLEM).and_then(|s| decode_emblem(s)) {
        Some(px) => emblem(k, &px, e, false),
        None if !rank.icon.is_empty() => k.pic(e, &rank.icon, [1.0; 4]),
        None => {}
    }
    let x = e.right() + 28.0;
    k.text(x, r.y + 30.0, t::LABEL, CARDS[card].name, t::ACCENT, 0);
    k.text(x, r.y + 56.0, Type { size: 52.0, caps: false, ..t::DISPLAY }, &identity(&fe.stats), t::INK, 0);
    if !rank.icon.is_empty() {
        k.pic(R::new(x, r.y + 120.0, 30.0, 30.0), &rank.icon, [1.0; 4]);
    }
    k.text(x + 40.0, r.y + 124.0, Type { size: 22.0, ..t::H2 }, &format!("{}  \u{b7}  Level {}", rank.name, rank.level), t::INK_DIM, 0);
    let bar = R::new(x, r.bottom() - 26.0, r.right() - x - 40.0, 6.0);
    let f = rank.next.map_or(1.0, |n| (rank.xp - rank.min) as f32 / (n - rank.min).max(1) as f32);
    k.fill(bar, t::hex(0xffffff, 0.14));
    k.fill(R::new(bar.x, bar.y, bar.w * f.clamp(0.0, 1.0), bar.h), t::ACCENT);
    let xp = rank.next.map_or_else(|| "Maximum rank".into(), |n| format!("{} XP to next level", (n - rank.xp).max(0)));
    k.text(bar.right(), bar.y - 30.0, t::MICRO, &xp, t::INK_DIM, 2);
}

fn overview(k: &mut Kit, fe: &Frontend) {
    banner(k, fe, R::new(PANEL.x, PANEL.y, PANEL.w, 200.0));
    let s = &fe.stats;
    section_icon(k, PANEL.x, PANEL.y + 222.0, PANEL.w, "Career", Some(icons::TROPHY));
    let tiles = [
        ("K/D ratio", ratio(s.get(stat::KILLS), s.get(stat::DEATHS)), format!("{} kills  \u{b7}  {} deaths", s.get(stat::KILLS), s.get(stat::DEATHS))),
        ("W/L ratio", ratio(s.get(stat::WINS), s.get(stat::LOSSES)), format!("{} wins  \u{b7}  {} losses", s.get(stat::WINS), s.get(stat::LOSSES))),
        ("Time played", duration(s.get(stat::TIME_PLAYED_TOTAL).max(0) as u64), String::new()),
        ("Headshots", s.get(stat::HEADSHOTS).to_string(), String::new()),
        ("Assists", s.get(stat::ASSISTS).to_string(), String::new()),
        ("Best streak", s.get(stat::KILL_STREAK).to_string(), String::new()),
        ("Total XP", s.get(stat::RANKXP).to_string(), String::new()),
    ];
    let tw = (PANEL.w - 2.0 * 16.0) / 3.0;
    for (i, (label, value, sub)) in tiles.iter().enumerate() {
        let r = R::new(PANEL.x + (i % 3) as f32 * (tw + 16.0), PANEL.y + 266.0 + (i / 3) as f32 * 132.0, tw, 120.0);
        grid_panel(k, r, 30.0, 0.7);
        let (sheet, icon) = [
            (HUD_ICONS, 10),
            (MENU_ICONS, icons::TROPHY),
            (HUD_ICONS, 14),
            (MENU_ICONS, icons::CROSSHAIR),
            (MENU_ICONS, icons::PEOPLE),
            (HUD_ICONS, 11),
            (MENU_ICONS, icons::STAR),
        ][i];
        k.icon(sheet, icon, R::new(r.right() - 64.0, r.y + 22.0, 40.0, 40.0), t::with_alpha(t::ACCENT, 0.85));
        k.text(r.x + 24.0, r.y + 20.0, t::MICRO, label, t::INK_DIM, 0);
        k.text(r.x + 24.0, r.y + 44.0, Type { size: 44.0, ..t::NUMERAL }, value, t::INK, 0);
        if !sub.is_empty() {
            k.text(r.x + 24.0, r.bottom() - 28.0, Type { size: 16.0, ..t::CAPTION }, sub, t::INK_MUTE, 0);
        }
    }
    // The favourite guns: under the tabs.
    let x = col_x(0);
    let mut y = 500.0;
    section_icon(k, x, y, cols(4), "Preferred weapons", Some(icons::RIFLE));
    y += 40.0;
    let guns: Vec<_> = sorted_weapons(fe).into_iter().filter(|key| metric(&fe.stats, key, "kills") > 0).take(3).collect();
    if guns.is_empty() {
        k.text(x, y, Type { size: 19.0, ..t::CAPTION }, "Play a match to build your weapon record.", t::INK_MUTE, 0);
    }
    for key in &guns {
        let r = R::new(x, y, cols(4), 92.0);
        grid_panel(k, r, 26.0, 0.6);
        let picture = fe.table_lookup("mp/statstable.csv", 4, key, 6);
        if !picture.is_empty() {
            k.pic(R::new(r.right() - 168.0, r.y + 10.0, 144.0, 72.0), &picture, [1.0; 4]);
        }
        k.text(r.x + 20.0, r.y + 16.0, Type { size: 26.0, ..t::H2 }, &gun_name(fe, key), t::INK, 0);
        k.text(r.x + 20.0, r.y + 54.0, t::MICRO, &format!("{} kills", metric(&fe.stats, key, "kills")), t::INK_DIM, 0);
        y += 100.0;
    }
}

fn page_line(k: &mut Kit, fe: &Frontend, count: usize, size: usize) {
    let page = fe.record_page(count, size) + 1;
    let pages = count.div_ceil(size).max(1);
    k.text(PANEL.right(), PANEL.y - 30.0, t::MICRO, &format!("Page {page} / {pages}  \u{b7}  {count} entries"), t::INK_DIM, 2);
}

fn weapons(k: &mut Kit, fe: &Frontend, om: &OpenMenu, is_hot: &dyn Fn(&Item) -> bool) {
    section_icon(k, PANEL.x, PANEL.y, PANEL.w, "Weapon statistics", Some(icons::RIFLE));
    k.text(PANEL.x, PANEL.y + 38.0, Type { size: 18.0, ..t::CAPTION }, "Select a weapon to see its challenges.", t::INK_MUTE, 0);
    for it in om.menu.items.iter().filter(|it| it.action.contains("combatWeapon ")) {
        let r = place_of(it);
        let key = it.action.split("combatWeapon").nth(1).unwrap_or("").trim().trim_end_matches(';').trim().to_owned();
        let on = is_hot(it);
        grid_panel(k, r, 26.0, if on { 1.0 } else { 0.55 });
        if on {
            k.grad_h(r.inset(2.0), t::ACCENT, 0.22, 0.0);
            k.fill(R::new(r.x, r.y, t::FOCUS_BAR, r.h), t::ACCENT);
        }
        let picture = fe.table_lookup("mp/statstable.csv", 4, &key, 6);
        if !picture.is_empty() {
            let ph = r.h - 16.0;
            k.pic(R::new(r.x + 20.0, r.y + 8.0, ph * 2.0, ph), &picture, [1.0; 4]);
        }
        k.text_mid(r.x + 228.0, r.cy(), Type { size: 26.0, ..t::H2 }, &gun_name(fe, &key), t::INK, 0);
        let cols_x = [r.x + 560.0, r.x + 670.0, r.x + 780.0, r.x + 890.0];
        let vals = [
            ("Kills", metric(&fe.stats, &key, "kills").to_string()),
            ("Heads", metric(&fe.stats, &key, "heads").to_string()),
            ("Shots", metric(&fe.stats, &key, "shots").to_string()),
            ("Held", format!("{}m", metric(&fe.stats, &key, "seconds") / 60)),
        ];
        for (x, (l, v)) in cols_x.iter().zip(vals.iter()) {
            k.text(*x, r.y + 14.0, t::MICRO, l, t::INK_MUTE, 0);
            k.text(*x, r.y + 36.0, Type { size: 26.0, ..t::NUMERAL }, v, t::INK, 0);
        }
    }
    page_line(k, fe, sorted_weapons(fe).len(), WEAPON_PAGE);
}

fn challenges(k: &mut Kit, fe: &Frontend, om: &OpenMenu, is_hot: &dyn Fn(&Item) -> bool) {
    // The weapon row.
    if let Some(it) = om.menu.items.iter().find(|it| it.action.trim_end().ends_with("combatCycleWeapon ;") || it.action.contains("combatCycleWeapon ;")) {
        let r = place_of(it);
        let key = fe.dvar("cr_weapon");
        let value = if key.is_empty() { "All".to_owned() } else { gun_name(fe, &key) };
        row(k, r, "Weapon", Some(&value), is_hot(it));
    }
    let levels = challenge_levels(fe);
    let page = fe.record_page(levels.len(), CHALLENGE_PAGE);
    if levels.is_empty() {
        k.text(PANEL.x, PANEL.y + 90.0, Type { size: 24.0, ..t::H2 }, "No challenges in this selection.", t::INK, 0);
    }
    let p = super::Painter { fe, pl: &Placement::new(1.0, 1.0), ops: &mut Vec::new() };
    for (row_i, entry) in levels.iter().skip(page * CHALLENGE_PAGE).take(CHALLENGE_PAGE).enumerate() {
        let r = R::new(PANEL.x, PANEL.y + 104.0 + row_i as f32 * 130.0, PANEL.w, 120.0);
        grid_panel(k, r, 26.0, 0.55);
        let (title, status, goal, reward, f, done) = match entry {
            RecordChallenge::Native(c, stage) => {
                let l = &c.levels[*stage];
                let state = fe.stats.get(c.state);
                let (progress, done) = challenge_progress(state, *stage, fe.stats.get(l.progress), l.target);
                let title = fe.localize(&format!("@{}", l.name));
                let weapon = weapon_of(c).map(|key| gun_name(fe, key)).unwrap_or_default();
                let status = if done {
                    "Complete".to_owned()
                } else if state == 0 {
                    p.challenge_rank(c).map_or_else(|| "Locked".into(), |level| format!("Level {level}"))
                } else if state < *stage as i32 + 1 {
                    "Locked stage".into()
                } else {
                    format!("{progress} / {}", l.target)
                };
                let reward = if !l.unlock_text.is_empty() {
                    format!("{} XP  \u{b7}  {}", l.xp, fe.localize(&format!("@{}", l.unlock_text)))
                } else {
                    format!("{} XP", l.xp)
                };
                (format!("{title}  {weapon}"), status, p.challenge_goal(c, l), reward, progress as f32 / l.target.max(1) as f32, done)
            }
            RecordChallenge::Mastery(weapon, camo) => {
                use crate::ui::mastery;
                let pr = mastery::progress(&fe.stats, &fe.assets, weapon, *camo);
                let available = mastery::unlocked(&fe.stats, &fe.assets, weapon, *camo);
                let status = if pr.complete { "Complete" } else if available { "In progress" } else { "Locked" };
                let (goal, detail) = pr.description.rsplit_once('(').map_or((pr.description.as_str(), ""), |(a, b)| (a, b.trim_end_matches([')', '.'])));
                let xp = match *camo {
                    mastery::GOLD => 1000,
                    mastery::PLATINUM => 2500,
                    _ => 5000,
                };
                (
                    format!("{}  {}", mastery::camo_name(*camo), gun_name(fe, weapon)),
                    status.to_owned(),
                    goal.replace(" with this weapon", ""),
                    format!("{detail}  \u{b7}  {xp} XP"),
                    pr.current as f32 / pr.target.max(1) as f32,
                    pr.complete,
                )
            }
        };
        let mastery = matches!(entry, RecordChallenge::Mastery(..));
        let (sheet, icon) = if mastery { (MENU_ICONS, icons::CRATE) } else { (HUD_ICONS, 11) };
        let ib = R::new(r.x + 20.0, r.y + 24.0, 64.0, 64.0);
        icon_box(k, ib);
        k.icon(sheet, icon, ib.inset(14.0), if done { t::ACCENT } else { t::INK_DIM });
        let x = r.x + 104.0;
        k.text(x, r.y + 18.0, Type { size: 26.0, ..t::H2 }, &title, t::INK, 0);
        k.text(r.right() - 28.0, r.y + 22.0, t::LABEL, &status, if done { t::ACCENT } else { t::INK_DIM }, 2);
        k.text(x, r.y + 54.0, Type { size: 19.0, ..t::BODY }, &goal, t::INK_DIM, 0);
        k.text(x, r.y + 80.0, Type { size: 16.0, ..t::CAPTION }, &reward, t::INK_MUTE, 0);
        let bar = R::new(x, r.bottom() - 14.0, r.right() - 28.0 - x, 5.0);
        k.fill(bar, t::hex(0xffffff, 0.12));
        k.fill(R::new(bar.x, bar.y, bar.w * f.clamp(0.0, 1.0), bar.h), if done { t::ACCENT } else { t::INK });
        if done {
            k.check(Vec2::new(r.right() - 40.0, r.y + 66.0), 22.0, 3.0, t::ACCENT);
        }
    }
    page_line(k, fe, levels.len(), CHALLENGE_PAGE);
}

fn identity_tab(k: &mut Kit, fe: &Frontend, om: &OpenMenu, is_hot: &dyn Fn(&Item) -> bool) {
    banner(k, fe, R::new(PANEL.x, PANEL.y, PANEL.w, 200.0));
    for it in om.menu.items.iter().filter(|it| it.ty == item_type::EDITFIELD) {
        let r = place_of(it);
        let label = if it.dvar.contains("clan") { "Clan tag" } else { "Name" };
        k.text(r.x, r.y - 30.0, t::LABEL, label, t::INK_DIM, 0);
        let editing = fe.editing.as_ref().is_some_and(|(m, i)| *m == om.name && om.menu.items.get(*i).is_some_and(|x| x.dvar == it.dvar));
        k.fill(r, t::hex(0x000000, 0.5));
        k.frame(r, 1.5, if editing || is_hot(it) { t::ACCENT } else { t::LINE });
        let mut v = fe.dvar(&it.dvar);
        if editing && (fe.millis() / 500) % 2 == 0 {
            v.push('_');
        }
        k.text_mid(r.x + 18.0, r.cy(), Type { size: 26.0, ..t::ROW_TEXT }, &v, t::INK, 0);
    }
    section_icon(k, PANEL.x, PANEL.y + 232.0, PANEL.w, "Player identity", Some(icons::HELMET));
    let y = PANEL.y + 470.0;
    k.text(PANEL.x, y, Type { size: 20.0, ..t::BODY }, "Your name and clan tag show in matches.", t::INK_DIM, 0);
    k.text(PANEL.x, y + 32.0, Type { size: 18.0, ..t::CAPTION }, "Name up to 16 characters, clan tag up to 4.", t::INK_MUTE, 0);
}

fn cards(k: &mut Kit, fe: &Frontend, om: &OpenMenu, is_hot: &dyn Fn(&Item) -> bool) {
    let selected = selected_card(&fe.stats);
    section_icon(k, PANEL.x, PANEL.y, PANEL.w, "Calling cards", Some(icons::STAR));
    for it in om.menu.items.iter().filter(|it| it.action.contains("combatCard ")) {
        let id = it.action.split("combatCard").nth(1).unwrap_or("").trim().trim_end_matches(';').trim();
        let Some(i) = CARDS.iter().position(|c| c.id == id) else { continue };
        let card = &CARDS[i];
        let r = place_of(it).pad(0.0, 4.0);
        let earned = fe.stats.get(card.stat) >= card.target;
        card_art(k, r, i, earned);
        k.grad_h(r, t::hex(0x050607, 1.0), 0.85, 0.1);
        let on = is_hot(it);
        if on {
            k.frame(r, 2.0, t::INK);
            k.brackets(r.inset(-5.0), 16.0, 3.0, t::ACCENT);
        } else {
            k.frame(r, 1.0, t::LINE);
        }
        k.text(r.x + 28.0, r.y + 18.0, Type { size: 30.0, ..t::H1 }, card.name, if earned { t::INK } else { t::INK_DIM }, 0);
        k.text(r.x + 28.0, r.y + 58.0, Type { size: 18.0, ..t::CAPTION }, card.goal, t::INK_DIM, 0);
        let bonus = if card.stat == stat::RANK { 1 } else { 0 };
        if selected == i {
            k.text(r.right() - 28.0, r.cy() - 10.0, t::LABEL, "Equipped", t::ACCENT, 2);
            k.check(Vec2::new(r.right() - 150.0, r.cy() - 2.0), 20.0, 3.0, t::ACCENT);
        } else if earned {
            k.text(r.right() - 28.0, r.cy() - 10.0, t::LABEL, "Available", t::INK_DIM, 2);
        } else {
            let have = fe.stats.get(card.stat).max(0) + bonus;
            let need = card.target + bonus;
            k.text(r.right() - 28.0, r.cy() - 22.0, t::LABEL, &format!("{have} / {need}"), t::INK_DIM, 2);
            k.icon(MENU_ICONS, icons::LOCK, R::new(r.right() - 48.0, r.cy() + 6.0, 20.0, 20.0), t::INK_MUTE);
        }
    }
}

fn emblem_editor(k: &mut Kit, fe: &Frontend, om: &OpenMenu, is_hot: &dyn Fn(&Item) -> bool) {
    section_icon(k, CANVAS.x, PANEL.y, CANVAS.w, "Paint your emblem", Some(icons::STAR));
    emblem(k, &fe.combat_record.pixels, CANVAS, true);
    let cur = usize::from(fe.combat_record.color);
    k.text(1510.0, PANEL.y, t::LABEL, "Colours", t::INK_DIM, 0);
    for (i, c) in PALETTE.iter().enumerate() {
        let r = swatch(i);
        if i == 0 {
            k.fill(r, t::hex(0x2a2f31, 1.0));
            k.text_mid(r.cx(), r.cy(), Type { size: 26.0, ..t::H2 }, "X", t::INK_DIM, 1);
        } else {
            k.fill(r, rgba(*c));
        }
        if i == cur {
            k.frame(r.inset(-4.0), 3.0, t::INK);
        }
    }
    k.text(1510.0, swatch(15).bottom() + 24.0, Type { size: 17.0, ..t::CAPTION }, "X erases a cell.", t::INK_MUTE, 0);
    // The focused cell or swatch.
    for it in om.menu.items.iter().filter(|it| is_hot(it)) {
        if it.window.name.starts_with("pixel_") || it.action.contains("combatColor") {
            k.frame(place_of(it).inset(-2.0), 2.5, t::ACCENT);
        }
    }
    // A preview of the banner with it.
    let r = R::new(1510.0, 700.0, 314.0, 176.0);
    card_art(k, r, selected_card(&fe.stats), true);
    k.grad_h(r, t::hex(0x050607, 1.0), 0.8, 0.2);
    emblem(k, &fe.combat_record.pixels, R::new(r.x + 20.0, r.y + 20.0, 96.0, 96.0), false);
    k.text(r.x + 20.0, r.bottom() - 46.0, Type { size: 20.0, ..t::H2 }, &identity(&fe.stats), t::INK, 0);
    k.text(r.x + 20.0, r.bottom() - 20.0, t::MICRO, "Preview  \u{b7}  save to apply", t::INK_MUTE, 0);
}
