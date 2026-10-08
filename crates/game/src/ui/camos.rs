//! Weapon mastery inside CoD4's original Create a Class menus.
//! Native rows provide the fonts, highlights, locked artwork and swatches.
//! Primary, sidearm/Overkill and RPG camos are saved independently.

use super::{Frontend, attachments, bo1, custom_camo, expr::Env, mastery, reticle_menu, script};
use iw3::menu::{Item, Menu, Statement, StringTable, Token, item_type, op};
use std::collections::HashMap;
use std::sync::Arc;

pub(super) const PLATINUM_SWATCH: &str = "cod4rw_camo_platinum";
pub(super) const DIAMOND_SWATCH: &str = "cod4rw_camo_diamond";
pub(super) const DESCRIPTION_DVAR: &str = "cod4rw_camo_goal";
// CoD4 has one class camo stat. Keep the sidearm and launcher alongside
// the existing extension stats, with a weapon stamp to reject stale saves.
const EXTRA_CAMO: i32 = 4600;
const EXTRA_WEAPON: i32 = 4700;
const RPG_PERK: i32 = 186;
const RPG_INDEX: i32 = 55;
/// The mastery camos. Diamond is a test feature (`--features diamond`); it's
/// last, so the others keep their places (and stats) without it.
#[cfg(feature = "diamond")]
const MASTERY: [(&str, &str, &str, usize, i32, &str); 3] = [
    ("camo_gold", "COD4RW_CAMO_GOLD", "Gold", 6, 8192, "ui_camoskin_gold"),
    ("camo_platinum", "COD4RW_CAMO_PLATINUM", "Platinum", 200, 16384, PLATINUM_SWATCH),
    ("camo_diamond", "COD4RW_CAMO_DIAMOND", "Diamond", 201, 32768, DIAMOND_SWATCH),
];
#[cfg(not(feature = "diamond"))]
const MASTERY: [(&str, &str, &str, usize, i32, &str); 2] = [
    ("camo_gold", "COD4RW_CAMO_GOLD", "Gold", 6, 8192, "ui_camoskin_gold"),
    ("camo_platinum", "COD4RW_CAMO_PLATINUM", "Platinum", 200, 16384, PLATINUM_SWATCH),
];
/// Rows after the camos: (name, label key, label, swatch, script, description).
const EXTRA_ROWS: [(&str, &str, &str, &str, &str, &str); 2] = [
    (custom_camo::ROW_NAME, custom_camo::ROW_LABEL, "Custom Camos...", custom_camo::ICON, "ccamoOpen", "Design your own camos and equip them on any weapon."),
    (reticle_menu::ROW_NAME, reticle_menu::ROW_LABEL, "Reticle...", "cod4rw_reticle_0", "creticleOpen", "The red dot sight's reticle: its shape, colour and size."),
];
const CLASSES: [&str; 5] = ["assault", "specops", "heavygunner", "demolitions", "sniper"];

pub(super) fn key_stat(key: &str) -> Option<i32> {
    let (class, rest) = key.split_once("_popup_cac_")?;
    let base = 200 + CLASSES.iter().position(|&c| c == class)? as i32 * 10;
    match rest {
        "camo" => Some(base + 1),
        "camo2" => Some(base + 3),
        "camorpg" => Some(base + 5),
        _ => None,
    }
}

fn camo_stat(weapon_stat: i32) -> i32 {
    if weapon_stat % 10 == 1 { weapon_stat + 8 } else { EXTRA_CAMO + weapon_stat }
}

fn standard_indices(tables: &HashMap<String, StringTable>) -> Vec<i32> {
    let Some(t) = tables.get("mp/statstable.csv") else { return Vec::new() };
    (0..t.rows)
        .filter(|&r| {
            matches!(
                t.get(r, 2),
                Some(
                    "weapon_assault"
                        | "weapon_smg"
                        | "weapon_lmg"
                        | "weapon_sniper"
                        | "weapon_shotgun"
                        | "weapon_pistol"
                )
            ) || t.get(r, 4) == Some("rpg")
        })
        .filter(|&r| t.get(r, 4).is_some_and(|s| !s.is_empty() && !s.starts_with("t5_") && !s.starts_with("t4_")))
        .filter_map(|r| t.get(r, 0)?.parse().ok())
        .collect()
}

pub(super) fn add(
    strings: &mut HashMap<String, String>,
    tables: &mut HashMap<String, StringTable>,
    menus: &mut HashMap<String, Arc<Menu>>,
) {
    if let Some(table) = tables.get_mut(attachments::TABLE) {
        for (name, label, title, id, bit, swatch) in MASTERY {
            strings.insert(label.into(), title.into());
            let description = format!("{label}_DESC");
            let goal = match id {
                6 => "Get 150 kills and 150 headshots with this weapon.",
                200 => "Earn Gold, then reach 500 kills with this weapon.",
                _ => "Earn Gold and Platinum on every weapon in this weapon class.",
            };
            strings.insert(description.clone(), goal.into());
            if table.columns < 12 {
                continue;
            }
            let row = (0..table.rows).find(|&r| table.get(r, 4) == Some(name));
            let start = if let Some(row) = row {
                row * table.columns
            } else {
                let start = table.values.len();
                table.values.extend(vec![String::new(); table.columns]);
                table.rows += 1;
                start
            };
            for (column, value) in [
                (2, "camo".to_owned()),
                (3, label.into()),
                (4, name.into()),
                (6, swatch.into()),
                (7, description.clone()),
                (8, description),
                (10, bit.to_string()),
                (11, id.to_string()),
            ] {
                table.values[start + column] = value;
            }
        }
    }
    // The Camo Editor's and the reticles' rows: CoD4's "None" row's unlock
    // bits, with their own labels, swatches and a camo number nothing has.
    if let Some(table) = tables.get_mut(attachments::TABLE)
        && table.columns >= 12
        && let Some(none) = (0..table.rows).find(|&r| table.get(r, 4) == Some("camo_none"))
    {
        for (name, label, title, swatch, _, goal) in EXTRA_ROWS {
            strings.insert(label.into(), title.into());
            let description = format!("{label}_DESC");
            strings.insert(description.clone(), goal.into());
            let mut row: Vec<String> = table.values[none * table.columns..(none + 1) * table.columns].to_vec();
            for (column, value) in [
                (3, label.to_owned()),
                (4, name.to_owned()),
                (6, swatch.to_owned()),
                (7, description.clone()),
                (8, description),
                (11, "999".to_owned()),
            ] {
                row[column] = value;
            }
            if !(0..table.rows).any(|r| table.get(r, 4) == Some(name)) {
                table.values.extend(row);
                table.rows += 1;
            }
        }
    }
    let indices = standard_indices(tables);
    let pistols: Vec<i32> = tables
        .get("mp/statstable.csv")
        .into_iter()
        .flat_map(|t| {
            (0..t.rows)
                .filter(|&r| t.get(r, 2) == Some("weapon_pistol"))
                .filter_map(|r| t.get(r, 0)?.parse().ok())
                .filter(|n| indices.contains(n))
        })
        .collect();
    for (i, class) in CLASSES.into_iter().enumerate() {
        let primary = 201 + i as i32 * 10;
        let primary_key = format!("{class}_popup_cac_camo");
        let Some(template) = menus.get(&primary_key).cloned() else { continue };
        for (suffix, stat) in [("camo", primary), ("camo2", primary + 2), ("camorpg", primary + 4)] {
            let key = format!("{class}_popup_cac_{suffix}");
            if let Some(menu) = with_mastery(&template, &key, primary, stat, &indices, &pistols) {
                menus.insert(key, Arc::new(menu));
            }
        }
        let class_key = format!("menu_cac_{class}");
        if let Some(template) = menus.get(&class_key).cloned() {
            if let Some(menu) = class_rows(&template, class, primary, &indices) {
                menus.insert(class_key, Arc::new(menu));
            }
        }
        // Native pistol attachment scripts advance to the primary's camo.
        // Sidearms and Overkill now have their own independent camo popup.
        for (key, menu) in menus.iter_mut().filter(|(key, _)| {
            key.starts_with(&format!("{class}_attachment_popup_")) && (key.ends_with('2') || key.ends_with("pistol"))
        }) {
            let mut patched = (**menu).clone();
            for item in &mut patched.items {
                item.action = item
                    .action
                    .replace(&format!("\"open\" \"{primary_key}\""), &format!("\"open\" \"{class}_popup_cac_camo2\""));
            }
            *menu = Arc::new(patched);
            let _ = key;
        }
    }
}

fn guard(exp: Statement, condition: Statement) -> Statement {
    // Native expressions omit the outermost final ')'. Keep the guard's
    // group closed before appending the unchanged expression.
    let mut out = vec![Token::Op(op::LEFTPAREN)];
    out.extend(condition);
    out.push(Token::Op(op::RIGHTPAREN));
    out.push(Token::Op(op::AND));
    out.extend(if exp.is_empty() { vec![Token::Int(1)] } else { exp });
    out
}

fn weapon_guard(weapon_stat: i32, indices: &[i32]) -> Statement {
    let allowed: &[i32] = if weapon_stat % 10 == 5 { &[RPG_PERK] } else { indices };
    let mut out = Vec::new();
    for (i, &index) in allowed.iter().enumerate() {
        if i != 0 {
            out.push(Token::Op(op::OR));
        }
        out.extend([
            Token::Op(op::LEFTPAREN),
            Token::Op(op::STAT),
            Token::Int(weapon_stat),
            Token::Op(op::RIGHTPAREN),
            Token::Op(op::EQUALS),
            Token::Int(index),
            Token::Op(op::RIGHTPAREN),
        ]);
    }
    if out.is_empty() {
        out.push(Token::Int(0));
    }
    out
}

fn mastery_hover(equal: bool) -> Statement {
    let mut out = Vec::new();
    for (i, (name, ..)) in MASTERY.iter().enumerate() {
        if i != 0 {
            out.push(Token::Op(if equal { op::OR } else { op::AND }));
        }
        out.extend([
            Token::Op(op::LEFTPAREN),
            Token::Op(op::DVARSTRING),
            Token::Str("ui_camo_highlighted".into()),
            Token::Op(op::RIGHTPAREN),
            Token::Op(if equal { op::EQUALS } else { op::NOTEQUAL }),
            Token::Str((*name).into()),
            Token::Op(op::RIGHTPAREN),
        ]);
    }
    out
}

fn selection(action: &str) -> Option<(i32, String)> {
    let tokens = script::tokenize(action);
    let at =
        tokens.iter().position(|t| matches!(t, script::Tok::Word(s) if s.eq_ignore_ascii_case("statsetusingtable")))?;
    let words: Vec<&str> = tokens[at + 1..]
        .iter()
        .take_while(|t| **t != script::Tok::Punct(';'))
        .filter_map(|t| if let script::Tok::Word(s) = t { Some(s.as_str()) } else { None })
        .collect();
    match words[..] {
        [stat, _, table, "4", name, "11", ..] if table.eq_ignore_ascii_case(attachments::TABLE) => {
            Some((stat.parse().ok()?, name.into()))
        }
        _ => None,
    }
}

fn select_script(action: &str, weapon_stat: i32, name: &str) -> String {
    let lower = action.to_ascii_lowercase();
    let Some(at) = lower.find("\"statsetusingtable\"") else { return action.into() };
    let Some(end) = action[at..].find(';').map(|i| at + i + 1) else { return action.into() };
    format!("{}\"uiScript\" \"camoPick\" \"{weapon_stat}\" \"{name}\" ; {}", &action[..at], &action[end..])
}

fn retarget(item: &mut Item, primary: i32, stat: i32) {
    for exp in [&mut item.visible_exp, &mut item.text_exp, &mut item.material_exp] {
        for token in exp {
            if let Token::Int(n) = token {
                if *n == primary {
                    *n = stat;
                } else if *n == primary + 1 {
                    *n = stat + 1;
                } else if *n == primary + 8 {
                    *n = camo_stat(stat);
                }
            }
        }
    }
    if stat % 10 == 5 {
        // RPG is a perk inventory weapon; its unlock lives in stat3055,
        // rather than stat(perk1 + 3000). Replace that nested lookup.
        let needle = [
            Token::Op(op::STAT),
            Token::Op(op::STAT),
            Token::Int(stat),
            Token::Op(op::RIGHTPAREN),
            Token::Op(op::ADD),
            Token::Int(3000),
            Token::Op(op::RIGHTPAREN),
        ];
        let replacement = [Token::Op(op::STAT), Token::Int(3000 + RPG_INDEX), Token::Op(op::RIGHTPAREN)];
        for exp in [&mut item.visible_exp, &mut item.text_exp, &mut item.material_exp] {
            while let Some(at) = exp.windows(needle.len()).position(|w| w == needle) {
                exp.splice(at..at + needle.len(), replacement.clone());
            }
        }
    }
}

fn quiet_backdrop(exp: &mut Statement, camo: i32) {
    // Keep native backgrounds for ordinary camos. Platinum uses None;
    // Diamond uses the original Gold artwork. The new patterned swatches
    // stay in the small picker/tile rather than behind the weapon model.
    let positions: Vec<_> = (0..exp.len())
        .filter_map(|i| {
            let rest = &exp[i..];
            if rest.starts_with(&[Token::Op(op::STAT), Token::Int(camo), Token::Op(op::RIGHTPAREN)]) {
                Some((i, 3))
            } else if rest.starts_with(&[
                Token::Op(op::STAT),
                Token::Op(op::LEFTPAREN),
                Token::Int(camo),
                Token::Op(op::RIGHTPAREN),
            ]) {
                Some((i, 4))
            } else {
                None
            }
        })
        .collect();
    let replacement = vec![
        Token::Op(op::LEFTPAREN),
        Token::Op(op::STAT),
        Token::Int(camo),
        Token::Op(op::RIGHTPAREN),
        Token::Op(op::MULTIPLY),
        Token::Op(op::LEFTPAREN),
        Token::Op(op::STAT),
        Token::Int(camo),
        Token::Op(op::RIGHTPAREN),
        Token::Op(op::LESSTHAN),
        Token::Int(200),
        Token::Op(op::RIGHTPAREN),
        Token::Op(op::ADD),
        Token::Int(6),
        Token::Op(op::MULTIPLY),
        Token::Op(op::LEFTPAREN),
        Token::Op(op::STAT),
        Token::Int(camo),
        Token::Op(op::RIGHTPAREN),
        Token::Op(op::EQUALS),
        Token::Int(201),
        Token::Op(op::RIGHTPAREN),
        Token::Op(op::RIGHTPAREN),
    ];
    for (at, len) in positions.into_iter().rev() {
        exp.splice(at..at + len, replacement.clone());
    }
}

fn with_mastery(
    template: &Menu,
    name: &str,
    primary: i32,
    stat: i32,
    indices: &[i32],
    pistols: &[i32],
) -> Option<Menu> {
    let none = template
        .items
        .iter()
        .find(|it| it.ty == item_type::BUTTON && selection(&it.action).is_some_and(|(_, n)| n == "camo_none"))?;
    let from = none.window.rect.y;
    let row_items: Vec<_> = template
        .items
        .iter()
        .filter(|it| (0.0..=220.0).contains(&it.window.rect.x) && (it.window.rect.y - from).abs() < 0.5)
        .cloned()
        .collect();
    let mut menu = template.clone();
    menu.window.name = name.into();
    // Preserve the six original choices; replace native Gold/Prestige with
    // per-weapon mastery rows and add Diamond immediately below Platinum.
    menu.items.retain(|it| {
        !((0.0..=220.0).contains(&it.window.rect.x)
            && it.window.rect.y >= from + 120.0
            && it.window.rect.y <= from + 160.0
            && it.window.rect.h <= 18.0)
    });
    let rifles: Vec<_> = indices.iter().copied().filter(|i| !pistols.contains(i) && *i != RPG_INDEX).collect();
    for item in &mut menu.items {
        if [126.0, 128.0, 130.0].iter().any(|y| (item.window.rect.y - y).abs() < 0.5)
            && (164.0..=168.0).contains(&item.window.rect.h)
        {
            // Room for the mastery rows and ours (in place of CoD4's two).
            item.window.rect.h += 20.0 * (MASTERY.len() + EXTRA_ROWS.len() - 2) as f32;
        } else if (item.window.rect.y - 294.0).abs() < 0.5 && item.window.rect.x >= 220.0 {
            item.window.rect.y += 20.0 * (MASTERY.len() + EXTRA_ROWS.len() - 2) as f32;
        }
        retarget(item, primary, stat);
        if stat % 10 == 5 && (item.window.rect.y - 106.0).abs() < 0.5 && !item.text_exp.is_empty() {
            item.text_exp = vec![Token::Str("Camouflage".into())];
        }
        if let Some((_, camo)) = selection(&item.action) {
            item.action = select_script(&item.action, stat, &camo);
        }
        // Sidearms and RPG models have no native cloth-camo variants.
        // Offer only None and the three new finishes on those models.
        if (0.0..=220.0).contains(&item.window.rect.x)
            && item.window.rect.y > from
            && item.window.rect.y < from + 120.0
            && item.window.rect.h <= 18.0
        {
            item.visible_exp = if stat % 10 == 5 {
                vec![Token::Int(0)]
            } else {
                guard(item.visible_exp.clone(), weapon_guard(stat, &rifles))
            };
        }
        // Live progress replaces the native static locked/unlocked copy.
        if item.text_exp.iter().any(|t| matches!(t, Token::Str(s) if s == "ui_camo_highlighted"))
            && (item.window.rect.y - 210.0).abs() < 0.5
        {
            item.visible_exp = guard(item.visible_exp.clone(), mastery_hover(false));
        }
    }
    for (i, (camo, label, _, _, _, _)) in MASTERY.into_iter().enumerate() {
        let row = 7 + i as i32;
        for mut item in row_items.clone() {
            item.window.rect.y = from + (6 + i) as f32 * 20.0;
            if stat % 10 == 5 {
                item.window.rect.y -= 100.0;
            } else {
                let mut y = vec![
                    Token::Float(item.window.rect.y),
                    Token::Op(op::SUBTRACT),
                    Token::Int(100),
                    Token::Op(op::MULTIPLY),
                    Token::Op(op::LEFTPAREN),
                ];
                y.extend(weapon_guard(stat, pistols));
                y.push(Token::Op(op::RIGHTPAREN));
                item.rect_y_exp = y;
            }
            item.window.name.clear();
            item.action = item.action.replace("camo_none", camo);
            item.on_focus = bo1::retarget_focus(&item.on_focus.replace("camo_none", camo), row);
            for token in item.visible_exp.iter_mut().chain(item.text_exp.iter_mut()).chain(item.material_exp.iter_mut())
            {
                if let Token::Str(s) = token {
                    if s == "camo_none" {
                        *s = camo.into();
                    } else if s == "@MPUI_NONE" {
                        *s = format!("@{label}");
                    }
                }
            }
            attachments::retarget_highlight(&mut item.visible_exp, row);
            retarget(&mut item, primary, stat);
            item.visible_exp = guard(item.visible_exp, weapon_guard(stat, indices));
            if selection(&item.action).is_some() {
                item.action = select_script(&item.action, stat, camo);
            }
            menu.items.push(item);
        }
    }
    // The Camo Editor's and the reticles' rows, last: they open their
    // screens.
    for (n, (name, label, _, _, script, _)) in EXTRA_ROWS.into_iter().enumerate() {
        let row = 7 + (MASTERY.len() + n) as i32;
        for mut item in row_items.clone() {
            item.window.rect.y = from + (6 + MASTERY.len() + n) as f32 * 20.0;
            if stat % 10 == 5 {
                item.window.rect.y -= 100.0;
            } else {
                let mut y = vec![
                    Token::Float(item.window.rect.y),
                    Token::Op(op::SUBTRACT),
                    Token::Int(100),
                    Token::Op(op::MULTIPLY),
                    Token::Op(op::LEFTPAREN),
                ];
                y.extend(weapon_guard(stat, pistols));
                y.push(Token::Op(op::RIGHTPAREN));
                item.rect_y_exp = y;
            }
            item.window.name.clear();
            item.on_focus = bo1::retarget_focus(&item.on_focus.replace("camo_none", name), row);
            for token in item.visible_exp.iter_mut().chain(item.text_exp.iter_mut()).chain(item.material_exp.iter_mut()) {
                if let Token::Str(s) = token {
                    if s == "camo_none" {
                        *s = name.into();
                    } else if s == "@MPUI_NONE" {
                        *s = format!("@{label}");
                    }
                }
            }
            attachments::retarget_highlight(&mut item.visible_exp, row);
            retarget(&mut item, primary, stat);
            item.visible_exp = guard(item.visible_exp, weapon_guard(stat, indices));
            if item.ty == item_type::BUTTON {
                item.action = format!("\"play\" \"mouse_click\" ; \"uiScript\" \"{script}\" \"{stat}\" ;");
            }
            menu.items.push(item);
        }
    }
    let mut goal =
        template.items.iter().find(|it| (it.window.rect.y - 210.0).abs() < 0.5 && !it.text_exp.is_empty())?.clone();
    goal.window.rect.h = 82.0;
    goal.text_scale = 0.3;
    goal.text_align_mode = 4;
    goal.text_align_y = 0.0;
    goal.text_exp = vec![
        Token::Op(op::LEFTPAREN),
        Token::Op(op::DVARSTRING),
        Token::Str(DESCRIPTION_DVAR.into()),
        Token::Op(op::RIGHTPAREN),
    ];
    goal.visible_exp = guard(vec![Token::Int(1)], mastery_hover(true));
    menu.items.push(goal);
    Some(menu)
}

fn class_rows(template: &Menu, class: &str, primary: i32, indices: &[i32]) -> Option<Menu> {
    let picker = template.items.iter().find(|it| it.ty == item_type::BUTTON && it.action.contains("_pc_rename"))?;
    let source: Vec<_> = template
        .items
        .iter()
        .filter(|it| (it.window.rect.y - picker.window.rect.y).abs() < 0.5 && (0.0..=220.0).contains(&it.window.rect.x))
        .cloned()
        .collect();
    let mut menu = template.clone();
    for mut item in source {
        item.window.name.clear();
        // A compact native row beside the launcher's inventory title keeps
        // the original attribute bars and supply-variant rows clear.
        item.window.rect.x = -138.0 + item.window.rect.x * (54.0 / 220.0);
        item.window.rect.w *= 54.0 / 220.0;
        item.window.rect.y = 236.0;
        item.window.rect.horz_align = 3;
        item.visible_exp = guard(item.visible_exp, weapon_guard(primary + 4, indices));
        attachments::retarget_highlight(&mut item.visible_exp, 11);
        if !item.text_exp.is_empty() {
            item.text_exp = vec![Token::Str("Camo".into())];
        }
        if item.ty == item_type::BUTTON {
            item.action = format!("\"play\" \"mouse_click\" ; \"open\" \"{class}_popup_cac_camorpg\" ;");
            item.on_focus = bo1::retarget_focus(&item.on_focus, 11);
        }
        menu.items.push(item);
    }
    // Camo is picked through the attachment flow (no separate Camo box by
    // the weapon, at the user's request).
    // Swap the preview's model, title and selected background together while
    // inspecting the launcher, so an RPG is never labelled as the rifle.
    let popup = format!("{class}_popup_cac_camorpg");
    let open = |value| {
        vec![
            Token::Op(op::MENUISOPEN),
            Token::Str(popup.clone()),
            Token::Op(op::RIGHTPAREN),
            Token::Op(op::EQUALS),
            Token::Int(value),
        ]
    };
    let originals = menu.items.len();
    for index in 0..originals {
        let item = &menu.items[index];
        let rifle =
            item.material_exp.windows(2).any(|w| matches!(w, [Token::Op(op::STAT), Token::Int(n)] if *n == primary));
        let title =
            item.text_exp.windows(2).any(|w| matches!(w, [Token::Op(op::STAT), Token::Int(n)] if *n == primary));
        // The native backdrop consists of four 69.5px artwork pieces.
        // Keep the new swatch only in our explicit Camo tiles.
        let backdrop = item.material_exp.iter().any(|t| matches!(t, Token::Int(n) if *n == primary + 8))
            && !item.window.name.starts_with("cod4rw_");
        let mut launcher = item.clone();
        if backdrop {
            quiet_backdrop(&mut menu.items[index].material_exp, primary + 8);
            menu.items[index].window.name = "cod4rw_camo_backdrop".into();
        }
        if !rifle && !title && !backdrop {
            continue;
        }
        menu.items[index].visible_exp = guard(menu.items[index].visible_exp.clone(), open(0));
        retarget(&mut launcher, primary, primary + 4);
        if backdrop {
            quiet_backdrop(&mut launcher.material_exp, camo_stat(primary + 4));
            launcher.window.name = "cod4rw_camo_backdrop".into();
        }
        for token in &mut launcher.forecolor_a_exp {
            if let Token::Int(n) = token {
                if *n == primary + 8 {
                    *n = camo_stat(primary + 4);
                }
            }
        }
        if title {
            launcher.text_exp = vec![Token::Str("@WEAPON_RPG".into())];
        }
        launcher.visible_exp = guard(launcher.visible_exp, open(1));
        menu.items.push(launcher);
    }
    Some(menu)
}

impl Frontend {
    pub(super) fn class_weapon(&self, stat: i32) -> String {
        if stat % 10 == 5 {
            if self.stat(stat) == RPG_PERK { "rpg".into() } else { String::new() }
        } else {
            self.table_lookup("mp/statstable.csv", 0, &self.stat(stat).to_string(), 4)
        }
    }

    pub(super) fn class_camo(&self, weapon_stat: i32) -> i32 {
        let camo = self.stat(camo_stat(weapon_stat));
        if weapon_stat % 10 != 1 && self.stat(EXTRA_WEAPON + weapon_stat) != self.stat(weapon_stat) {
            return 0;
        }
        if matches!(camo, 6 | 200 | 201)
            && !mastery::unlocked(&self.stats, &self.assets, &self.class_weapon(weapon_stat), camo as usize)
        {
            return 0;
        }
        camo
    }

    pub(super) fn set_class_stat(&mut self, stat: i32, value: i32) {
        if (201..250).contains(&stat) && matches!(stat % 10, 3 | 5) {
            self.stats.set(camo_stat(stat), 0);
            self.stats.set(EXTRA_WEAPON + stat, 0);
        }
        self.stats.set(stat, value);
    }

    pub(super) fn camo_script(&mut self, args: &[String]) {
        let [command, stat, name, ..] = args else { return };
        if !command.eq_ignore_ascii_case("camoPick") {
            return;
        }
        let Ok(stat) = stat.parse::<i32>() else { return };
        if !(201..250).contains(&stat) || !matches!(stat % 10, 1 | 3 | 5) {
            return;
        }
        let weapon = self.class_weapon(stat);
        let Ok(camo) = self.table_lookup(attachments::TABLE, 4, name, 11).parse::<usize>() else { return };
        let can_pick = if matches!(camo, 6 | 200 | 201) {
            mastery::unlocked(&self.stats, &self.assets, &weapon, camo)
        } else {
            let bit = self.table_lookup(attachments::TABLE, 4, name, 10).parse::<i32>().unwrap_or(0);
            let index = if stat % 10 == 5 { RPG_INDEX } else { self.stat(stat) };
            !weapon.is_empty() && self.stat(3000 + index) & bit != 0
        };
        if can_pick {
            self.stats.set(camo_stat(stat), camo as i32);
            self.stats.set(EXTRA_WEAPON + stat, self.stat(stat));
            // The original close list covers camo/camo2. The launcher
            // popup is new, so close the selected slot directly as well.
            let popup = self.stack.iter().find(|m| key_stat(&m.name) == Some(stat)).map(|m| m.name.clone());
            if let Some(popup) = popup {
                self.close(&popup);
            }
        }
    }

    /// Equip camo `camo` (a custom one, or none) on class weapon `stat`.
    pub(super) fn equip_custom_camo(&mut self, stat: i32, camo: i32) {
        if (201..250).contains(&stat) && matches!(stat % 10, 1 | 3 | 5) {
            self.stats.set(camo_stat(stat), camo);
            self.stats.set(EXTRA_WEAPON + stat, self.stat(stat));
        }
    }

    pub(super) fn camo_description(&self) -> String {
        let Some(stat) = self.stack.iter().rev().find_map(|m| key_stat(&m.name)) else { return String::new() };
        let name = self.dvar("ui_camo_highlighted");
        let Some((_, _, _, camo, _, _)) = MASTERY.iter().find(|(key, ..)| *key == name) else { return String::new() };
        let key = self.class_weapon(stat);
        let progress = mastery::progress(&self.stats, &self.assets, &key, *camo);
        let status = if progress.complete {
            "Unlocked"
        } else if mastery::unlocked(&self.stats, &self.assets, &key, *camo) {
            "Unlock All"
        } else {
            "Locked"
        };
        format!("{}\n{status}: {} / {}", progress.description, progress.current.min(progress.target), progress.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{assets::UiAssets, progression, stats::Stats};

    fn choice(fe: &Frontend, name: &str) -> usize {
        fe.stack
            .last()
            .unwrap()
            .menu
            .items
            .iter()
            .position(|it| it.ty == item_type::BUTTON && it.action.contains("camoPick") && it.action.contains(name))
            .unwrap()
    }

    #[test]
    #[ignore = "requires a local CoD4 asset installation"]
    fn native_mastery_menus_preview_save_and_gate_every_cod4_weapon_and_class_slot() {
        let mut fe = Frontend::bare(UiAssets::load().expect("installed native UI assets"));
        fe.stats = Stats::in_memory();
        fe.stats.set(progression::stat::RANK, 54);
        fe.stats.set_dvar(progression::UNLOCKS_DVAR, "all");
        fe.stats.set(mastery::RPG_PERK_STAT, 1);
        // Unlock All profiles normally receive their base weapon flags
        // from Stats::load; this isolated fixture deliberately skips I/O.
        for weapon in mastery::weapons(&fe.assets) {
            fe.stats.set(weapon.unlock_stat, 1 | 2 | 4 | 8 | 16 | 32);
        }
        progression::refresh_unlocks(&mut fe.stats, &fe.assets);
        assert_eq!(fe.table_lookup(attachments::TABLE, 11, "200", 6), PLATINUM_SWATCH);
        assert_eq!(fe.table_lookup(attachments::TABLE, 11, "201", 6), DIAMOND_SWATCH);
        let weapons = mastery::weapons(&fe.assets);
        for (i, class) in CLASSES.into_iter().enumerate() {
            let base = 200 + i as i32 * 10;
            fe.stats.set(base + 1, 20);
            fe.stats.set(base + 3, 1);
            fe.stats.set(base + 5, 186);
            for weapon in &weapons {
                let slots: &[i32] = if weapon.key == "rpg" { &[5] } else { &[1, 3] };
                for &slot in slots {
                    let stat = base + slot;
                    fe.set_class_stat(stat, if slot == 5 { RPG_PERK } else { weapon.index });
                    fe.stats.set(stat + 1, if slot == 5 { fe.stat(stat + 1) } else { 0 });
                    let suffix = match slot {
                        1 => "camo",
                        3 => "camo2",
                        _ => "camorpg",
                    };
                    let popup_name = format!("{class}_popup_cac_{suffix}");
                    for (camo_name, _, _, camo, _, _) in MASTERY {
                        fe.stack.clear();
                        fe.open(&format!("menu_cac_{class}"));
                        fe.open(&popup_name);
                        let index = choice(&fe, camo_name);
                        assert!(
                            fe.item_visible(fe.stack.last().unwrap(), index),
                            "{class}/{}/{suffix}/{camo_name}",
                            weapon.key
                        );
                        fe.set_focus(Some((popup_name.clone(), index)));
                        assert_eq!(fe.gun_for(stat).unwrap().1, camo, "hover previews correct slot");
                        assert!(!fe.camo_description().is_empty());
                        let action = fe.stack.last().unwrap().menu.items[index].action.clone();
                        fe.run(&action, &popup_name);
                        assert_eq!(
                            fe.class_camo(stat),
                            camo as i32,
                            "{class}/{}/{suffix}/{camo_name}: selection persists independently",
                            weapon.key
                        );
                        assert!(!fe.stack.iter().any(|m| m.name == popup_name));
                        let loadout = fe.class_loadout(&format!("custom{}", i + 1)).unwrap();
                        if slot == 5 {
                            assert_eq!(loadout.inventory_camo, camo);
                        } else {
                            assert_eq!(
                                loadout.guns[if slot == 1 { 0 } else { 1 }].camo,
                                camo,
                                "selected finish reaches match loadout"
                            );
                        }
                        fe.open(&popup_name);
                        assert!(fe.item_visible(fe.stack.last().unwrap(), index));
                        // Native None still clears the selected slot only.
                        let none = choice(&fe, "camo_none");
                        let action = fe.stack.last().unwrap().menu.items[none].action.clone();
                        fe.run(&action, &popup_name);
                        assert_eq!(fe.class_camo(stat), 0);
                    }
                }
            }
            // The normal native pistol flow must advance to the sidearm
            // camo, rather than silently editing the rifle's finish.
            fe.stack.clear();
            fe.set_class_stat(base + 3, 1);
            fe.open(&format!("menu_cac_{class}"));
            let attachment_name = format!("{class}_attachment_popup_pistol");
            fe.open(&attachment_name);
            let popup = fe.stack.last().unwrap();
            let accept = popup
                .menu
                .items
                .iter()
                .enumerate()
                .find(|(index, item)| {
                    item.ty == item_type::BUTTON
                        && fe.item_visible(popup, *index)
                        && item.text_exp.iter().any(|t| matches!(t, Token::Str(s) if s == "@MENU_ACCEPT"))
                })
                .unwrap()
                .1
                .action
                .clone();
            fe.run(&accept, &attachment_name);
            assert_eq!(fe.stack.last().unwrap().name, format!("{class}_popup_cac_camo2"));
        }

        // Progression exposes an inspectable locked row, but its active
        // button and direct script both refuse unearned mastery finishes.
        fe.stack.clear();
        fe.stats = Stats::in_memory();
        fe.stats.set(progression::stat::RANK, 54);
        fe.stats.set(201, 20);
        fe.stats.set_dvar(progression::UNLOCKS_DVAR, "cod4");
        progression::refresh_unlocks(&mut fe.stats, &fe.assets);
        fe.open("menu_cac_assault");
        fe.open("assault_popup_cac_camo");
        for (name, _, _, id, _, _) in MASTERY {
            let index = choice(&fe, name);
            assert!(!fe.item_visible(fe.stack.last().unwrap(), index));
            fe.camo_script(&["camoPick".into(), "201".into(), name.into()]);
            assert_eq!(fe.class_camo(201), 0, "cannot equip locked {id}");
        }
        fe.stats.set_dvar("cod4rw_weapon_ak47_kills", "500");
        fe.stats.set_dvar("cod4rw_weapon_ak47_heads", "150");
        progression::refresh_unlocks(&mut fe.stats, &fe.assets);
        for name in ["camo_gold", "camo_platinum"] {
            assert!(fe.item_visible(fe.stack.last().unwrap(), choice(&fe, name)));
        }
        assert!(!fe.item_visible(fe.stack.last().unwrap(), choice(&fe, "camo_diamond")));
        for weapon in weapons.iter().filter(|w| w.family == "Assault Rifles") {
            fe.stats.set_dvar(&format!("cod4rw_weapon_{}_kills", weapon.key), "500");
            fe.stats.set_dvar(&format!("cod4rw_weapon_{}_heads", weapon.key), "150");
        }
        progression::refresh_unlocks(&mut fe.stats, &fe.assets);
        assert!(fe.item_visible(fe.stack.last().unwrap(), choice(&fe, "camo_diamond")));
        // Live guards also handle reuse of an already-open menu.
        fe.stats.set(201, crate::bo1::FIRST_INDEX);
        for (name, ..) in MASTERY {
            assert!(!fe.item_visible(fe.stack.last().unwrap(), choice(&fe, name)));
        }
        fe.stats.set_dvar(progression::UNLOCKS_DVAR, "all");
        fe.stats.set(mastery::RPG_PERK_STAT, 1);
        progression::refresh_unlocks(&mut fe.stats, &fe.assets);
        fe.set_class_stat(203, 1);
        fe.camo_script(&["camoPick".into(), "203".into(), "camo_platinum".into()]);
        assert_eq!(fe.class_camo(203), 200);
        fe.set_class_stat(203, 2);
        fe.set_class_stat(203, 1);
        assert_eq!(fe.class_camo(203), 0, "changing guns clears secondary camo");
        fe.set_class_stat(205, 186);
        fe.camo_script(&["camoPick".into(), "205".into(), "camo_diamond".into()]);
        assert_eq!(fe.class_camo(205), 201);
        fe.set_class_stat(205, 190);
        fe.set_class_stat(205, 186);
        assert_eq!(fe.class_camo(205), 0, "removing RPG perk clears its finish");
    }

    #[test]
    #[ignore = "requires a local CoD4 asset installation"]
    fn native_locked_rpg_artwork_and_quiet_mastery_backdrops() {
        let mut fe = Frontend::bare(UiAssets::load().expect("installed native UI assets"));
        fe.stats = Stats::in_memory();
        fe.stats.set(progression::stat::RANK, 54);
        fe.stats.set_dvar(progression::UNLOCKS_DVAR, "cod4");
        fe.stats.set(mastery::RPG_PERK_STAT, 1);
        progression::refresh_unlocks(&mut fe.stats, &fe.assets);
        for (i, class) in CLASSES.into_iter().enumerate() {
            let base = 200 + i as i32 * 10;
            fe.stats.set(base + 1, 20);
            fe.stats.set(base + 3, 1);
            fe.stats.set(base + 5, RPG_PERK);
            fe.stack.clear();
            fe.open(&format!("menu_cac_{class}"));
            fe.open(&format!("{class}_popup_cac_camorpg"));
            let popup = fe.stack.last().unwrap();
            let none = choice(&fe, "camo_none");
            assert!(
                fe.item_visible(popup, none),
                "{class}: None selectable with no RPG kills; stat3055={}, expression={:?}",
                fe.stat(3055),
                popup.menu.items[none].visible_exp
            );
            for (name, _, title, _, _, _) in MASTERY {
                assert!(!fe.item_visible(popup, choice(&fe, name)), "unearned {title} active button remains locked");
                let labels: Vec<_> = popup
                    .menu
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| {
                        let raw = if item.text_exp.is_empty() {
                            item.text.clone()
                        } else {
                            super::super::expr::eval(&item.text_exp, &fe).text()
                        };
                        fe.assets.localize(&raw) == title
                    })
                    .collect();
                let label = labels.iter().find(|(index, _)| fe.item_visible(popup, *index));
                assert!(
                    label.is_some(),
                    "{class}: locked {title} label visible; stat3055={}, labels={:?}",
                    fe.stat(3055),
                    labels.iter().map(|(_, item)| &item.visible_exp).collect::<Vec<_>>()
                );
                let y = fe.item_rect(label.unwrap().1).y;
                assert!(
                    popup.menu.items.iter().enumerate().any(|(index, item)| {
                        fe.item_visible(popup, index)
                            && (fe.item_rect(item).y - y).abs() < 0.5
                            && super::super::expr::eval(&item.material_exp, &fe)
                                .text()
                                .to_ascii_lowercase()
                                .contains("lock")
                    }),
                    "{class}: native lock icon accompanies {title}"
                );
            }
            // The large class backdrop is quiet, while the native picker
            // and direct Camo tile continue to use the new finish swatches.
            for (stat, suffix) in [(base + 1, "camo"), (base + 5, "camorpg")] {
                for (camo, native) in [(200, 0), (201, 6)] {
                    fe.stats.set(camo_stat(stat), camo);
                    fe.stack.clear();
                    fe.open(&format!("menu_cac_{class}"));
                    if suffix == "camorpg" {
                        fe.open(&format!("{class}_popup_cac_{suffix}"));
                    }
                    let main = fe.stack.iter().find(|m| m.name == format!("menu_cac_{class}")).unwrap();
                    let backgrounds: Vec<_> = main
                        .menu
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(index, item)| {
                            item.window.name == "cod4rw_camo_backdrop" && fe.item_visible(main, *index)
                        })
                        .map(|(_, item)| item)
                        .collect();
                    assert_eq!(backgrounds.len(), 4, "{class}/{suffix}: all four native backdrop pieces visible");
                    for background in backgrounds {
                        assert_eq!(
                            super::super::expr::eval(&background.material_exp, &fe).text(),
                            fe.table_lookup(attachments::TABLE, 11, &native.to_string(), 6),
                            "{class}/{suffix}: every {camo} backdrop piece uses native {native} artwork; expression={:?}",
                            background.material_exp
                        );
                    }
                }
            }
        }
    }
}
