//! World at War guns in Create a Class ([`crate::waw`]), next to Black Ops'
//! ([`super::bo1`]): the game choice before the weapon lists offers "World
//! at War", which lists WaW's weapon groups and guns, then the gun's
//! attachments. WaW has no camos, so its guns' camo choice is "No Camo".
//!
//! Like Black Ops', WaW's guns and attachments are rows of CoD4's stats and
//! attachment tables ([`extend`]), with WaW's own attribute bars in CoD4's
//! attributes table, and its popups are the class's CoD4 ones with WaW's
//! rows.
//!
//! Popups are named `t4|<kind>|<class>|<weapon stat>|<arg>`.

use super::Frontend;
use super::attachments;
use super::expr::Env;
use super::bo1::{Row, highlighted_name, str_exp, with_rows};
use crate::waw::{self, Group};
use iw3::menu::{Menu, StringTable};
use std::collections::HashMap;
use std::sync::Arc;

const KEY_PREFIX: &str = "t4|";
/// WaW-only attachments' indices in CoD4's attachment table (column 9).
const FIRST_ATTACHMENT_INDEX: i32 = 200;

/// World at War's guns and attachments as rows of CoD4's
/// `mp/statstable.csv`, `mp/attachmenttable.csv` and
/// `mp/attributestable.csv`, with their names (`T4_`-prefixed keys) in
/// `strings`.
pub fn extend(strings: &mut HashMap<String, String>, tables: &mut HashMap<String, StringTable>) {
    let Some(data) = waw::data() else { return };
    let mut text = |key: &str| -> String {
        let ours = format!("T4_{}", key.to_ascii_uppercase());
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
    let picture = |name: &str| format!("{}{name}", waw::MATERIAL_PREFIX);
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
    // Create a Class's attribute bars: WaW's own, for its guns
    // (`t4_thompson`) and their attachments (`t4_thompson_silenced`).
    if let Some(t) = tables.get_mut("mp/attributestable.csv") {
        for (group, name, values) in &data.attributes {
            let ours = data.guns.iter().any(|g| {
                *name == g.name || name.strip_prefix(&format!("{}_", g.name)).is_some_and(|a| g.attachments.iter().any(|x| x == a))
            });
            if ours {
                add(t, [group.clone(), format!("{}{name}", waw::PREFIX)].into_iter().chain(values.iter().map(i32::to_string)).collect());
            }
        }
    }
    for g in Group::PRIMARY {
        text(g.table().1);
    }
    for a in &data.attachments {
        text(&a.key);
    }
    text("WEAPON_NO_CAMO");
    if let Some(t) = tables.get_mut("mp/attachmenttable.csv") {
        let known: Vec<String> = (0..t.rows).filter_map(|r| t.get(r, 4).map(str::to_ascii_lowercase)).collect();
        for (k, a) in data.attachments.iter().enumerate() {
            if known.contains(&a.name) || attachments::bit(&a.name) == 0 {
                continue;
            }
            // Not every attachment has a description.
            let description = format!("T4_{}", a.description_key.to_ascii_uppercase());
            strings.insert(description.clone(), data.strings.get(&a.description_key.to_ascii_uppercase()).cloned().unwrap_or_default());
            // [-, -, type, name key, name, -, picture, description key, -, index, bit]
            let row = vec![
                String::new(),
                String::new(),
                format!("attachment_{}", a.name),
                format!("T4_{}", a.key.to_ascii_uppercase()),
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
    }
    // The game choice's label.
    strings.insert("T5_GAME_WAW".into(), "World at War".into());
}

/// Click a World at War attachment: a gun takes one, as in WaW, so it
/// replaces any other; clicking the one on takes it off.
pub fn toggle(set: i32, name: &str) -> i32 {
    let b = attachments::bit(name);
    if set & b != 0 { 0 } else { b }
}

/// The weapon stat of a World at War popup's name.
pub fn key_stat(key: &str) -> Option<i32> {
    key.strip_prefix(KEY_PREFIX)?.split('|').nth(2)?.parse().ok()
}

/// `t4|kind|class|stat|arg`.
fn key(kind: &str, class: &str, stat: i32, arg: &str) -> String {
    format!("{KEY_PREFIX}{kind}|{class}|{stat}|{arg}")
}

/// The camo popup for a class's World at War primary (stat `stat`).
pub fn camo_key(class: &str, stat: i32) -> String {
    key("popup_cac_camo", class, stat, "")
}

fn group_name(g: Group) -> &'static str {
    match g {
        Group::BoltAction => "boltaction",
        Group::Rifle => "rifle",
        Group::Smg => "smg",
        Group::Shotgun => "shotgun",
        Group::Lmg => "lmg",
        Group::Pistol => "pistol",
    }
}

fn group_from_name(name: &str) -> Option<Group> {
    Group::PRIMARY.into_iter().chain([Group::Pistol]).find(|g| group_name(*g) == name)
}

/// The heading's string for a group (WaW has no "Pistols"; CoD4's does).
fn heading(g: Group) -> String {
    match g {
        Group::Pistol => "@MPUI_PISTOLS".into(),
        g => format!("@T4_{}", g.table().1),
    }
}

/// The class's CoD4 weapon list a group's list is made from, and its
/// attachment popup.
fn templates(g: Group) -> (&'static str, &'static str) {
    match g {
        Group::BoltAction => ("popup_cac_sniper", "sniper"),
        Group::Rifle => ("popup_cac_assault", "assault"),
        Group::Smg => ("popup_cac_smg", "smg"),
        Group::Shotgun => ("popup_cac_shotgun", "shotgun"),
        Group::Lmg => ("popup_cac_lmg", "lmg"),
        Group::Pistol => ("popup_cac_secondary", "pistol"),
    }
}

impl Frontend {
    /// World at War's popups, built from the class's CoD4 ones.
    pub(super) fn waw_menu(&self, key: &str) -> Option<Arc<Menu>> {
        let parts: Vec<&str> = key.strip_prefix(KEY_PREFIX)?.split('|').collect();
        let [kind, class, stat, arg] = parts[..] else { return None };
        let stat: i32 = stat.parse().ok()?;
        let data = waw::data()?;
        let template = |name: &str| self.assets.menu(&format!("{class}_{name}"));
        let click = |script: String| format!("\"play\" \"mouse_click\" ; {script}");
        let menu = match kind {
            "groups" => {
                let t = template("popup_cac_primary")?;
                let rows: Vec<Row> = Group::PRIMARY
                    .iter()
                    .filter(|g| data.guns.iter().any(|gun| gun.group == **g))
                    .map(|g| Row {
                        label: str_exp(&heading(*g)),
                        action: click(format!("\"uiScript\" \"t4List\" \"{class}\" \"{stat}\" \"{}\" ;", group_name(*g))),
                        rename: None,
                    })
                    .collect();
                with_rows(&t, key, &rows)?
            }
            "list" => {
                let group = group_from_name(arg)?;
                let t = template(templates(group).0)?;
                let from = highlighted_name(&t, if group == Group::Pistol { "ui_sidearm_highlighted" } else { "ui_primary_highlighted" });
                let rows: Vec<Row> = data
                    .guns
                    .iter()
                    .filter(|g| g.group == group)
                    .map(|g| Row {
                        label: str_exp(&format!("@T4_{}", g.key.to_ascii_uppercase())),
                        action: click(format!("\"uiScript\" \"t4Pick\" \"{class}\" \"{stat}\" \"{}\" ;", g.index)),
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
                // interpreter picks them and adds "Accept".
                let pick = |name: &str| {
                    click(format!(
                        "\"statsetusingtable\" ( \"{}\" , \"tablelookup\" ( \"{}\" , 4 , \"{name}\" , 9 ) ) ; \"close\" \"self\" ; ",
                        stat + 1,
                        attachments::TABLE
                    ))
                };
                let rows: Vec<Row> = std::iter::once(("none".to_owned(), "@MPUI_NONE".to_owned()))
                    .chain(gun.attachments.iter().map(|a| {
                        let key = data.attachment(a).map_or(a.clone(), |x| x.key.clone());
                        (a.clone(), format!("@T4_{}", key.to_ascii_uppercase()))
                    }))
                    .map(|(name, label)| Row { label: str_exp(&label), action: pick(&name), rename: from.clone().map(|f| (f, name)) })
                    .collect();
                with_rows(&t, key, &rows)?
            }
            // WaW has no camos: the one choice is none.
            "popup_cac_camo" => {
                let t = template("popup_cac_camo")?;
                let from = highlighted_name(&t, "ui_camo_highlighted");
                let row = Row {
                    label: str_exp("@T4_WEAPON_NO_CAMO"),
                    action: click(format!("\"uiScript\" \"t4NoCamo\" \"{class}\" \"{stat}\" ; \"close\" \"self\" ;")),
                    rename: from.map(|f| (f, "camo_none".into())),
                };
                with_rows(&t, key, &[row])?
            }
            _ => return None,
        };
        Some(Arc::new(menu))
    }

    /// "World at War" in the game choice for the weapon popup `popup` of
    /// class weapon stat `stat`.
    pub(super) fn waw_game(&mut self, class: &str, stat: i32, popup: &str) {
        if popup == "secondary" {
            self.set_dvar("ui_weapon_class_selected", "@MPUI_PISTOLS");
            self.open(&key("list", class, stat, group_name(Group::Pistol)));
        } else {
            self.open(&key("groups", class, stat, ""));
        }
    }

    /// `uiScript t4List <class> <stat> <group>`, `t4Pick <class> <stat>
    /// <index>`, `t4NoCamo <class> <stat>`.
    pub(super) fn waw_script(&mut self, args: &[String]) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        let (class, stat): (&str, i32) = (a(1), a(2).parse().unwrap_or(0));
        match a(0).to_ascii_lowercase().as_str() {
            "t4list" => {
                if let Some(g) = group_from_name(a(3)) {
                    self.set_dvar("ui_weapon_class_selected", &heading(g));
                    self.open(&key("list", class, stat, group_name(g)));
                }
            }
            "t4pick" => {
                let index: i32 = a(3).parse().unwrap_or(0);
                let Some(gun) = waw::data().and_then(|d| d.guns.iter().find(|g| g.index == index)) else { return };
                let has_attachments = !gun.attachments.is_empty();
                self.stats.set(stat, index);
                self.stats.set(stat + 1, 0);
                self.stats.set(attachments::SET_STATS + stat, 0);
                // No camos in World at War.
                if stat % 10 == 1 {
                    self.stats.set(stat - stat % 10 + 9, 0);
                }
                // Close the weapon popups, CoD4's and ours.
                let open: Vec<String> = self.stack.iter().map(|m| m.name.clone()).collect();
                for name in open.iter().filter(|n| n.starts_with("t4|") || n.starts_with("t5|") || n.contains("_popup_cac_") || n.contains("_attachment_popup_")) {
                    self.close(name);
                }
                if has_attachments {
                    self.open(&key("attachment_popup", class, stat, ""));
                }
            }
            "t4nocamo" => self.stats.set(stat - stat % 10 + 9, 0),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_toggling() {
        let k = key("attachment_popup", "heavy_gunner", 213, "");
        assert_eq!(key_stat(&k), Some(213));
        assert_eq!(key_stat("t5|list|x|201|smg"), None);
        let aperture = attachments::bit("aperture");
        // One attachment: a second replaces the first, clicking it again
        // takes it off.
        assert_eq!(toggle(0, "aperture"), aperture);
        assert_eq!(toggle(aperture, "silenced"), attachments::bit("silenced"));
        assert_eq!(toggle(aperture, "aperture"), 0);
    }
}
