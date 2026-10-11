//! Modern Warfare 2 guns in Create a Class ([`crate::mw2guns`]), next to
//! Black Ops' and World at War's: the game choice before the weapon lists
//! offers "Modern Warfare 2", which lists MW2's weapon groups and guns, then
//! the gun's attachments (two at most, of the pairs MW2 allows, as its Bling
//! did) and its camos.
//!
//! Like the others, MW2's guns, attachments and camos are rows of CoD4's
//! stats and attachment tables ([`extend`]), with MW2's own attribute bars,
//! and its popups are the class's CoD4 ones with MW2's rows. A camo row's
//! value (column 11) is [`crate::mw2guns::CAMO_BASE`] plus its index.
//!
//! Popups are named `iw4|<kind>|<class>|<weapon stat>|<arg>`.

use super::Frontend;
use super::attachments;
use super::expr::Env;
use super::bo1::{Row, highlighted_name, str_exp, with_rows};
use crate::mw2guns::{self, Group};
use iw3::menu::{Menu, StringTable};
use std::collections::HashMap;
use std::sync::Arc;

const KEY_PREFIX: &str = "iw4|";
/// MW2 camos' rows in CoD4's attachment table: `iw4_camo_<n>`.
const CAMO_PREFIX: &str = "iw4_camo_";
/// MW2-only attachments' indices in CoD4's attachment table (column 9).
const FIRST_ATTACHMENT_INDEX: i32 = 300;

/// MW2's guns, attachments and camos as rows of CoD4's `mp/statstable.csv`,
/// `mp/attachmenttable.csv` and `mp/attributestable.csv`, with their names
/// (`IW4_`-prefixed keys) in `strings`.
pub fn extend(strings: &mut HashMap<String, String>, tables: &mut HashMap<String, StringTable>) {
    let Some(data) = mw2guns::data() else { return };
    let mut text = |key: &str| -> String {
        let ours = format!("IW4_{}", key.to_ascii_uppercase());
        let value = data.strings.get(&key.to_ascii_uppercase()).cloned().unwrap_or_else(|| key.to_owned());
        strings.insert(ours.clone(), value);
        ours
    };
    let add = |table: &mut StringTable, cells: Vec<String>| {
        let mut cells = cells;
        cells.resize(table.columns, String::new());
        table.values.extend(cells);
        table.rows += 1;
    };
    let picture = |name: &str| format!("{}{name}", mw2guns::MATERIAL_PREFIX);
    if let Some(t) = tables.get_mut("mp/statstable.csv") {
        for g in &data.guns {
            // [index, unlock stat, group, name key, name, -, picture, description key, attachments]
            let row = vec![
                g.index.to_string(),
                (3000 + g.index).to_string(),
                g.group.table().0.into(),
                text(&g.key),
                g.id(),
                String::new(),
                picture(&g.picture),
                text(&g.description_key),
                g.attachments.join(" "),
            ];
            add(t, row);
        }
    }
    // Create a Class's attribute bars: MW2's own, for its guns (`iw4_m4`)
    // and their attachments (`iw4_m4_reflex`).
    if let Some(t) = tables.get_mut("mp/attributestable.csv") {
        for (group, name, values) in &data.attributes {
            let ours = data.guns.iter().any(|g| {
                *name == g.name || name.strip_prefix(&format!("{}_", g.name)).is_some_and(|a| g.attachments.iter().any(|x| x == a))
            });
            if ours {
                add(t, [group.clone(), format!("{}{name}", mw2guns::PREFIX)].into_iter().chain(values.iter().map(i32::to_string)).collect());
            }
        }
    }
    for g in Group::PRIMARY.into_iter().chain(Group::SECONDARY) {
        text(g.table().1);
    }
    for a in &data.attachments {
        text(&a.key);
    }
    if let Some(t) = tables.get_mut("mp/attachmenttable.csv") {
        let known: Vec<String> = (0..t.rows).filter_map(|r| t.get(r, 4).map(str::to_ascii_lowercase)).collect();
        for (k, a) in data.attachments.iter().enumerate() {
            if known.contains(&a.name) || attachments::bit(&a.name) == 0 {
                continue;
            }
            let description = text(&a.description_key);
            // [-, -, type, name key, name, -, picture, description key, -, index, bit]
            let row = vec![
                String::new(),
                String::new(),
                format!("attachment_{}", a.name),
                format!("IW4_{}", a.key.to_ascii_uppercase()),
                a.name.clone(),
                String::new(),
                picture(&a.picture),
                description,
                String::new(),
                (FIRST_ATTACHMENT_INDEX + k as i32).to_string(),
                attachments::bit(&a.name).to_string(),
            ];
            add(t, row);
        }
        for c in &data.camos {
            // [-, -, "camo", name key, name, -, picture, description key, -, -, bit, camo stat]
            let key = text(&c.key);
            let row = vec![
                String::new(),
                String::new(),
                "camo".into(),
                key.clone(),
                format!("{CAMO_PREFIX}{}", c.index),
                String::new(),
                picture(&c.picture),
                text(&c.description_key),
                String::new(),
                String::new(),
                "1".into(),
                (mw2guns::CAMO_BASE + c.index).to_string(),
            ];
            add(t, row);
        }
    }
    // The game choice's label.
    strings.insert("T5_GAME_MW2".into(), "Modern Warfare 2".into());
}

/// Click an MW2 attachment: it goes on with at most one other, of a pair MW2
/// allows (any it clashes with come off, the older one when there would be
/// three); clicking one already on takes it off.
pub fn toggle(set: i32, name: &str) -> i32 {
    let b = attachments::bit(name);
    if b == 0 {
        return 0;
    }
    if set & b != 0 {
        return set & !b;
    }
    let Some(data) = mw2guns::data() else { return b };
    let mut on: Vec<&str> = attachments::mw2_names(set).into_iter().filter(|n| data.compatible(n, name)).collect();
    while on.len() > 1 {
        on.remove(0);
    }
    on.iter().fold(b, |s, n| s | attachments::bit(n))
}

/// The weapon stat of an MW2 popup's name.
pub fn key_stat(key: &str) -> Option<i32> {
    key.strip_prefix(KEY_PREFIX)?.split('|').nth(2)?.parse().ok()
}

/// `iw4|kind|class|stat|arg`.
fn key(kind: &str, class: &str, stat: i32, arg: &str) -> String {
    format!("{KEY_PREFIX}{kind}|{class}|{stat}|{arg}")
}

/// The camo popup for a class's MW2 primary (stat `stat`).
pub fn camo_key(class: &str, stat: i32) -> String {
    key("popup_cac_camo", class, stat, "")
}

fn group_name(g: Group) -> &'static str {
    match g {
        Group::Assault => "assault",
        Group::Smg => "smg",
        Group::Lmg => "lmg",
        Group::Sniper => "sniper",
        Group::Shotgun => "shotgun",
        Group::MachinePistol => "machinepistol",
        Group::Pistol => "pistol",
    }
}

fn group_from_name(name: &str) -> Option<Group> {
    Group::PRIMARY.into_iter().chain(Group::SECONDARY).find(|g| group_name(*g) == name)
}

/// The class's CoD4 weapon list a group's list is made from, and its
/// attachment popup.
fn templates(g: Group) -> (&'static str, &'static str) {
    match g {
        Group::Assault => ("popup_cac_assault", "assault"),
        Group::Smg => ("popup_cac_smg", "smg"),
        Group::Lmg => ("popup_cac_lmg", "lmg"),
        Group::Sniper => ("popup_cac_sniper", "sniper"),
        Group::Shotgun => ("popup_cac_shotgun", "shotgun"),
        Group::MachinePistol | Group::Pistol => ("popup_cac_secondary", "pistol"),
    }
}

impl Frontend {
    /// MW2's popups, built from the class's CoD4 ones.
    pub(super) fn mw2_menu(&self, key: &str) -> Option<Arc<Menu>> {
        let parts: Vec<&str> = key.strip_prefix(KEY_PREFIX)?.split('|').collect();
        let [kind, class, stat, arg] = parts[..] else { return None };
        let stat: i32 = stat.parse().ok()?;
        let data = mw2guns::data()?;
        let template = |name: &str| self.assets.menu(&format!("{class}_{name}"));
        let click = |script: String| format!("\"play\" \"mouse_click\" ; {script}");
        let menu = match kind {
            // Primary groups, or (for the sidearm) the secondary ones.
            "groups" | "secondary_groups" => {
                let t = template("popup_cac_primary")?;
                let groups: &[Group] = if kind == "groups" { &Group::PRIMARY } else { &Group::SECONDARY };
                let rows: Vec<Row> = groups
                    .iter()
                    .filter(|g| data.guns.iter().any(|gun| gun.group == **g))
                    .map(|g| Row {
                        label: str_exp(&format!("@IW4_{}", g.table().1)),
                        action: click(format!("\"uiScript\" \"iw4List\" \"{class}\" \"{stat}\" \"{}\" ;", group_name(*g))),
                        rename: None,
                    })
                    .collect();
                with_rows(&t, key, &rows)?
            }
            "list" => {
                let group = group_from_name(arg)?;
                let t = template(templates(group).0)?;
                let secondary = group.secondary();
                let from = highlighted_name(&t, if secondary { "ui_sidearm_highlighted" } else { "ui_primary_highlighted" });
                let rows: Vec<Row> = data
                    .guns
                    .iter()
                    .filter(|g| g.group == group)
                    .map(|g| Row {
                        label: str_exp(&format!("@IW4_{}", g.key.to_ascii_uppercase())),
                        action: click(format!("\"uiScript\" \"iw4Pick\" \"{class}\" \"{stat}\" \"{}\" ;", g.index)),
                        rename: from.clone().map(|f| (f, g.id())),
                    })
                    .collect();
                let mut m = with_rows(&t, key, &rows)?;
                // The popup's own preview shows the gun under the mouse.
                let first = rows.first().and_then(|r| r.rename.as_ref()).map_or("", |r| r.1.as_str());
                m.on_open = format!("\"execnow\" \"set ui_primary_highlighted {first}; set ui_sidearm_highlighted {first}\" ; ");
                m
            }
            "attachment_popup" => {
                let gun = data.guns.iter().find(|g| g.index == self.stat(stat))?;
                let t = self.assets.menu(&format!("{class}_attachment_popup_{}", templates(gun.group).1))?;
                let from = highlighted_name(&t, "ui_attachment_highlighted");
                // Rows pick like CoD4's (`statsetusingtable`), so the
                // interpreter toggles them and adds "Accept"; a primary's
                // camos follow.
                let next = if stat % 10 == 1 && !gun.camos.is_empty() { format!("\"open\" \"{}\" ; ", camo_key(class, stat)) } else { String::new() };
                let pick = |name: &str| {
                    click(format!(
                        "\"statsetusingtable\" ( \"{}\" , \"tablelookup\" ( \"{}\" , 4 , \"{name}\" , 9 ) ) ; \"close\" \"self\" ; {next}",
                        stat + 1,
                        attachments::TABLE
                    ))
                };
                let rows: Vec<Row> = std::iter::once(("none".to_owned(), "@MPUI_NONE".to_owned()))
                    .chain(gun.attachments.iter().map(|a| {
                        let key = data.attachment(a).map_or(a.clone(), |x| x.key.clone());
                        (a.clone(), format!("@IW4_{}", key.to_ascii_uppercase()))
                    }))
                    .map(|(name, label)| Row { label: str_exp(&label), action: pick(&name), rename: from.clone().map(|f| (f, name)) })
                    .collect();
                with_rows(&t, key, &rows)?
            }
            // The camos the gun has models for (MW2's pistols have none).
            "popup_cac_camo" => {
                let t = template("popup_cac_camo")?;
                let from = highlighted_name(&t, "ui_camo_highlighted");
                let gun = data.guns.iter().find(|g| g.index == self.stat(stat));
                let none = Row {
                    label: str_exp("@MPUI_NONE"),
                    action: click(format!("\"uiScript\" \"iw4NoCamo\" \"{class}\" \"{stat}\" ; \"close\" \"self\" ;")),
                    rename: from.clone().map(|f| (f, "camo_none".into())),
                };
                let mut rows: Vec<Row> = std::iter::once(none)
                    .chain(data.camos.iter().filter(|c| gun.is_some_and(|g| g.camos.contains(&c.index))).map(|c| {
                        let name = format!("{CAMO_PREFIX}{}", c.index);
                        Row {
                            label: str_exp(&format!("@IW4_{}", c.key.to_ascii_uppercase())),
                            action: click(format!(
                                "\"statsetusingtable\" ( \"{}\" , \"tablelookup\" ( \"{}\" , 4 , \"{name}\" , \"11\" ) ) ; \"close\" \"self\" ;",
                                stat - stat % 10 + 9,
                                attachments::TABLE
                            )),
                            rename: from.clone().map(|f| (f, name)),
                        }
                    }))
                    .collect();
                rows.push(Row {
                    label: str_exp(&format!("@{}", super::custom_camo::ROW_LABEL)),
                    action: click(format!("\"uiScript\" \"ccamoOpen\" \"{stat}\" ;")),
                    rename: from.clone().map(|f| (f, super::custom_camo::ROW_NAME.into())),
                });
                rows.push(Row {
                    label: str_exp(&format!("@{}", super::reticle_menu::ROW_LABEL)),
                    action: click(format!("\"uiScript\" \"creticleOpen\" \"{stat}\" ;")),
                    rename: from.map(|f| (f, super::reticle_menu::ROW_NAME.into())),
                });
                with_rows(&t, key, &rows)?
            }
            _ => return None,
        };
        Some(Arc::new(menu))
    }

    /// "Modern Warfare 2" in the game choice for the weapon popup `popup` of
    /// class weapon stat `stat`: MW2's primaries, or its secondaries for the
    /// sidearm.
    pub(super) fn mw2_game(&mut self, class: &str, stat: i32, popup: &str) {
        self.open(&key(if popup == "secondary" { "secondary_groups" } else { "groups" }, class, stat, ""));
    }

    /// `uiScript iw4List <class> <stat> <group>`, `iw4Pick <class> <stat>
    /// <index>`, `iw4NoCamo <class> <stat>`.
    pub(super) fn mw2_script(&mut self, args: &[String]) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        let (class, stat): (&str, i32) = (a(1), a(2).parse().unwrap_or(0));
        match a(0).to_ascii_lowercase().as_str() {
            "iw4list" => {
                if let Some(g) = group_from_name(a(3)) {
                    self.set_dvar("ui_weapon_class_selected", &format!("@IW4_{}", g.table().1));
                    self.open(&key("list", class, stat, group_name(g)));
                }
            }
            "iw4pick" => {
                let index: i32 = a(3).parse().unwrap_or(0);
                let Some(gun) = mw2guns::data().and_then(|d| d.guns.iter().find(|g| g.index == index)) else { return };
                let (has_attachments, has_camos) = (!gun.attachments.is_empty(), !gun.camos.is_empty());
                self.stats.set(stat, index);
                self.stats.set(stat + 1, 0);
                self.stats.set(attachments::SET_STATS + stat, 0);
                if stat % 10 == 1 {
                    self.stats.set(stat - stat % 10 + 9, 0);
                }
                // Close the weapon popups, CoD4's and ours.
                let open: Vec<String> = self.stack.iter().map(|m| m.name.clone()).collect();
                for name in open.iter().filter(|n| n.starts_with(KEY_PREFIX) || n.starts_with("t5|") || n.contains("_popup_cac_") || n.contains("_attachment_popup_")) {
                    self.close(name);
                }
                if has_attachments {
                    self.open(&key("attachment_popup", class, stat, ""));
                } else if stat % 10 == 1 && has_camos {
                    self.open(&camo_key(class, stat));
                }
            }
            "iw4nocamo" => self.stats.set(stat - stat % 10 + 9, 0),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let k = key("attachment_popup", "heavy_gunner", 213, "");
        assert_eq!(key_stat(&k), Some(213));
        assert_eq!(key_stat("t4|list|x|201|smg"), None);
    }
}
