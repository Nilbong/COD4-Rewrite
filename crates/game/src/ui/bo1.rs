//! Black Ops guns in Create a Class ([`crate::bo1`]). Picking a primary or
//! secondary weapon first asks which game's: "Call of Duty 4" opens CoD4's
//! own list, "Black Ops" lists BO1's weapon groups and guns, then the gun's
//! attachments and (primary) camos; "World at War" does the same with WaW's
//! ([`super::waw`]). Each game is offered when it is installed.
//!
//! BO1's guns, attachments and camos are added as rows of CoD4's stats and
//! attachment tables ([`extend`]), and its popups are CoD4's own (the class's
//! weapon list, attachment and camo popups) with BO1's rows, so the menu
//! interpreter lays them out, highlights them, previews the gun under the
//! mouse and picks attachments as it does for CoD4's guns.
//!
//! Popups are named `t5|<kind>|<class>|<weapon stat>|<arg>`.

use super::attachments::{self, retarget_highlight};
use super::expr::Env;
use super::Frontend;
use crate::bo1::{self, Group};
use iw3::menu::{Item, Menu, StringTable, Token, item_type, op};
use std::collections::HashMap;
use std::sync::Arc;

const KEY_PREFIX: &str = "t5|";
/// BO1 camos' rows in CoD4's attachment table: `t5_camo_<n>`.
const CAMO_PREFIX: &str = "t5_camo_";
/// BO1-only attachments' indices in CoD4's attachment table (column 9).
const FIRST_ATTACHMENT_INDEX: i32 = 100;

/// Black Ops' guns, attachments and camos as rows of CoD4's
/// `mp/statstable.csv` and `mp/attachmenttable.csv`, with their names
/// (`T5_`-prefixed keys) in `strings`.
pub fn extend(strings: &mut HashMap<String, String>, tables: &mut HashMap<String, StringTable>) {
    let Some(data) = bo1::data() else { return };
    let mut text = |key: &str| -> String {
        let ours = format!("T5_{}", key.to_ascii_uppercase());
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
    if let Some(t) = tables.get_mut("mp/statstable.csv") {
        for g in &data.guns {
            let (group, _) = g.group.cod4();
            // [index, unlock stat, group, name key, name, -, picture, description key, attachments]
            let row = vec![
                g.index.to_string(),
                (3000 + g.index).to_string(),
                group.into(),
                text(&g.key),
                g.id(),
                String::new(),
                g.picture.clone(),
                text(&g.description_key),
                g.attachments.join(" "),
            ];
            add(t, row);
        }
    }
    // Create a Class's attribute bars.
    if let Some(t) = tables.get_mut("mp/attributestable.csv") {
        for (group, key, values) in attributes(data) {
            add(t, [group.to_owned(), key].into_iter().chain(values.iter().map(i32::to_string)).collect());
        }
    }
    // Every attachment's name: Black Ops' rows show them, CoD4's names differ.
    for a in &data.attachments {
        text(&a.key);
    }
    if let Some(t) = tables.get_mut("mp/attachmenttable.csv") {
        let known: Vec<String> = (0..t.rows).filter_map(|r| t.get(r, 4).map(str::to_ascii_lowercase)).collect();
        for (k, a) in data.attachments.iter().enumerate() {
            if known.contains(&a.name) || attachments::bit(&a.name) == 0 {
                continue;
            }
            // [-, -, type, name key, name, -, picture, description key, -, index, bit]
            let row = vec![
                String::new(),
                String::new(),
                format!("attachment_{}", a.name),
                text(&a.key),
                a.name.clone(),
                String::new(),
                a.picture.clone(),
                text(&format!("{}_DESC", a.key)),
                String::new(),
                (FIRST_ATTACHMENT_INDEX + k as i32).to_string(),
                attachments::bit(&a.name).to_string(),
            ];
            add(t, row);
        }
        for c in &data.camos {
            // [-, -, "camo", name key, name, -, picture, description key, -, -, bit, camo stat]:
            // camo stats from 100 up, so lookups by stat (the class's camo
            // swatch) find these rows rather than CoD4's.
            let key = text(&c.key);
            let row = vec![
                String::new(),
                String::new(),
                "camo".into(),
                key.clone(),
                format!("{CAMO_PREFIX}{}", c.index),
                String::new(),
                c.picture.clone(),
                key,
                String::new(),
                String::new(),
                "1".into(),
                bo1::camo_stat(c.index).to_string(),
            ];
            add(t, row);
        }
    }
    // The game choice's labels.
    strings.insert("T5_GAME_COD4".into(), "Call of Duty 4".into());
    strings.insert("T5_GAME_BO1".into(), "Black Ops".into());
}

/// Class baselines for the attribute bars (accuracy, damage, range,
/// mobility), from CoD4's own table: mobility and range go mostly by class.
fn class_attributes(group: bo1::Group) -> [f32; 4] {
    match group {
        bo1::Group::Assault => [72.0, 68.0, 75.0, 66.0],
        bo1::Group::Smg => [80.0, 72.0, 40.0, 100.0],
        bo1::Group::Lmg => [65.0, 68.0, 75.0, 33.0],
        bo1::Group::Sniper => [68.0, 85.0, 100.0, 66.0],
        bo1::Group::Shotgun => [28.0, 85.0, 20.0, 100.0],
        bo1::Group::Pistol => [0.0; 4],
    }
}

/// Black Ops guns' rows in CoD4's `mp/attributestable.csv` (accuracy,
/// damage, range, fire rate, mobility, 0..100), and their attachments'
/// changes (`t5_ak47_silencer`). CoD4's values are set by hand; these follow
/// its pattern: a class baseline, moved by the gun's damage, recoil and
/// range against the other guns of its class and by its move speed. Fire
/// rate comes from rounds per minute as CoD4's automatics' do (600 rpm 25,
/// 940 rpm 85); single shot guns get CoD4's semi-automatic or bolt/pump
/// values. Pistols are all 0, as in CoD4.
fn attributes(data: &bo1::Data) -> Vec<(&'static str, String, [i32; 5])> {
    let defs: Vec<(&bo1::Gun, crate::weapons::WeaponDef)> =
        data.guns.iter().filter_map(|g| Some((g, data.weapon_def(&g.id(), &[])?))).collect();
    // Class means of damage, recoil and range.
    let mean = |group: bo1::Group, f: &dyn Fn(&crate::weapons::WeaponDef) -> f32| {
        let v: Vec<f32> = defs.iter().filter(|(g, _)| g.group == group).map(|(_, d)| f(d)).filter(|x| *x > 0.0).collect();
        if v.is_empty() { 1.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
    };
    let kick = |d: &crate::weapons::WeaponDef| d.ads_kick_pitch.1.abs().max(d.ads_kick_pitch.0.abs()).max(0.01);
    let mut out = Vec::new();
    for (g, d) in &defs {
        let (group, _) = g.group.cod4();
        let mut values = [0; 5];
        if g.group != bo1::Group::Pistol {
            let [acc, dmg, range, mob] = class_attributes(g.group);
            let accuracy = acc * (mean(g.group, &kick) / kick(d)).powf(0.5);
            let damage = dmg + 1.2 * (d.damage - mean(g.group, &|d| d.damage));
            let reach = range * (d.max_damage_range.max(1.0) / mean(g.group, &|d| d.max_damage_range)).powf(0.35);
            let file = data.weapon(&format!("{}_mp", g.name));
            let single = file.is_some_and(|w| w.get("fireType").to_ascii_lowercase().contains("single"));
            // Bolt actions and pumps work the action between shots.
            let bolt = file.is_some_and(|w| w.get("boltAction") == "1" || w.get("rechamberTime").trim().parse::<f32>().is_ok_and(|t| t > 0.0));
            let rate = match (single, g.group) {
                (false, _) => 0.177 * 60.0 / d.fire_time.max(0.01) - 81.0,
                (true, _) if bolt => 20.0,
                (true, bo1::Group::Sniper) => 50.0,
                (true, _) => if d.fire_time > 0.25 { 20.0 } else { 35.0 },
            };
            let mobility = mob * d.move_speed_scale;
            values = [accuracy, damage, reach, rate, mobility].map(|v| v.round().clamp(5.0, 100.0) as i32);
        }
        out.push((group, g.id(), values));
        for a in &g.attachments {
            // CoD4's convention for the attachments it shares; others' are
            // their own kind's.
            let delta = match a.as_str() {
                "silencer" => [0, 0, -20, 0, 0],
                "acog" | "lps" | "vzoom" => [-10, 0, 20, 0, 0],
                "grip" => [10, 0, 0, 0, 0],
                "rf" => [0, 0, 0, 15, 0],
                _ => [0; 5],
            };
            out.push((group, format!("{}_{a}", g.id()), delta));
        }
    }
    out
}

/// Toggle a Black Ops attachment in a set: one per slot (top, bottom,
/// muzzle, trigger); clicking one already on takes it off.
pub fn toggle(set: i32, name: &str) -> i32 {
    let b = attachments::bit(name);
    if b == 0 {
        return 0;
    }
    if set & b != 0 {
        return set & !b;
    }
    let Some(data) = bo1::data() else { return set | b };
    let slot = data.attachment(name).map(|a| a.slot.as_str());
    let others = attachments::names(set).into_iter().filter(|n| data.attachment(n).map(|a| a.slot.as_str()) == slot);
    others.fold(set, |s, n| s & !attachments::bit(n)) | b
}

/// The weapon stat of a Black Ops popup's name.
pub fn key_stat(key: &str) -> Option<i32> {
    key.strip_prefix(KEY_PREFIX)?.split('|').nth(2)?.parse().ok()
}

/// `t5|kind|class|stat|arg`.
fn key(kind: &str, class: &str, stat: i32, arg: &str) -> String {
    format!("{KEY_PREFIX}{kind}|{class}|{stat}|{arg}")
}

pub(super) fn str_exp(s: &str) -> Vec<Token> {
    vec![Token::Op(op::LEFTPAREN), Token::Str(s.into())]
}

/// Point `"setLocalVarInt" "ui_highlight" N` in a script at another row.
fn retarget_focus(script: &str, row: i32) -> String {
    let key = "\"ui_highlight\" ";
    let Some(at) = script.find(key) else { return script.into() };
    let start = at + key.len();
    let end = script[start..].find(|c: char| !c.is_ascii_digit()).map_or(script.len(), |e| start + e);
    format!("{}{row}{}", &script[..start], &script[end..])
}

/// A list row: a CoD4 template row moved and retargeted.
pub(super) struct Row {
    pub label: Vec<Token>,
    pub action: String,
    /// The template's highlighted-thing name (`mp5`) and ours.
    pub rename: Option<(String, String)>,
}

/// A CoD4 list popup with its rows replaced by `rows`: the first row is the
/// template, the rows' column and frame stretch to fit, and locks and "new"
/// marks go (Black Ops' guns are all unlocked).
pub(super) fn with_rows(template: &Menu, name: &str, rows: &[Row]) -> Option<Menu> {
    let left = |it: &Item| it.window.rect.x > -100.0 && it.window.rect.x < 230.0;
    // Rows are the buttons that move the highlight.
    let mut ys: Vec<f32> = template
        .items
        .iter()
        .filter(|it| it.ty == item_type::BUTTON && left(it) && it.on_focus.contains("\"ui_highlight\""))
        .map(|it| it.window.rect.y)
        .collect();
    ys.sort_by(f32::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    let (&first, &last) = (ys.first()?, ys.last()?);
    let pitch = ys.windows(2).map(|w| w[1] - w[0]).reduce(f32::min).unwrap_or(20.0);
    let in_row = |it: &Item, y: f32| left(it) && (it.window.rect.y - y).abs() < 0.5;
    let template_row: Vec<Item> = template.items.iter().filter(|it| in_row(it, first)).cloned().collect();
    let delta = (rows.len() as f32 - ys.len() as f32) * pitch;
    let mut m = template.clone();
    m.window.name = name.to_owned();
    m.items.retain(|it| !ys.iter().any(|&y| in_row(it, y)));
    for it in m.items.iter_mut().filter(|it| left(it)) {
        let r = &mut it.window.rect;
        if r.y <= first + 0.5 && r.y + r.h >= last + pitch - 0.5 {
            r.h += delta;
        } else if r.y > last + 0.5 {
            r.y += delta;
        }
    }
    let locks = |it: &Item| {
        let pic = it.window.background.as_deref().unwrap_or("");
        let exp = it.material_exp.iter().any(|t| matches!(t, Token::Str(s) if s.starts_with("specialty_locked") || s.starts_with("specialty_new")));
        pic.starts_with("specialty_locked") || pic.starts_with("specialty_new") || exp
    };
    for (i, row) in rows.iter().enumerate() {
        let n = i as i32 + 1;
        for mut it in template_row.iter().filter(|it| !locks(it)).cloned() {
            it.window.rect.y = first + i as f32 * pitch;
            it.window.name.clear();
            // Unlock checks go; the highlight follows the row.
            if it.visible_exp.iter().any(|t| matches!(t, Token::Op(op::STAT))) {
                it.visible_exp.clear();
            }
            retarget_highlight(&mut it.visible_exp, n);
            if let Some((from, to)) = &row.rename {
                for t in it.visible_exp.iter_mut().chain(it.text_exp.iter_mut()).chain(it.material_exp.iter_mut()) {
                    if let Token::Str(s) = t {
                        if s.eq_ignore_ascii_case(from) {
                            *s = to.clone();
                        }
                    }
                }
                it.on_focus = it.on_focus.replace(&format!(" {from}\""), &format!(" {to}\""));
            }
            it.on_focus = retarget_focus(&it.on_focus, n);
            if !it.text_exp.is_empty() && it.text_exp != str_exp("") {
                it.text_exp = row.label.clone();
            }
            if it.ty == item_type::BUTTON {
                it.action = row.action.clone();
                it.dvar_flags = 0;
            }
            m.items.push(it);
        }
    }
    Some(m)
}

/// The name a template row highlights (`set ui_primary_highlighted mp5`).
pub(super) fn highlighted_name(template: &Menu, dvar: &str) -> Option<String> {
    let needle = format!("set {dvar} ");
    template.items.iter().find_map(|it| {
        let at = it.on_focus.find(&needle)? + needle.len();
        Some(it.on_focus[at..].split(|c: char| c == '"' || c == ';' || c.is_whitespace()).next()?.to_owned())
    })
}

impl Frontend {
    /// The class base stat (200, 210, ...) of the Create a Class menu open.
    fn bo1_class_base(&self) -> Option<i32> {
        let menu = self.stack.iter().rev().find(|m| m.name.starts_with("menu_cac_"))?;
        let lower = menu.menu.on_open.to_ascii_lowercase();
        let at = lower.find("statgetindvar ")? + "statgetindvar ".len();
        let n: i32 = lower[at..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
        Some(n - n % 10)
    }

    /// Opening CoD4's weapon popup `key` opens the game choice instead (unless
    /// it was just picked there); opening its camo popup for a Black Ops (or
    /// World at War) gun opens that game's camos.
    pub(super) fn bo1_redirect(&mut self, key: &str) -> Option<String> {
        if bo1::data().is_none() && crate::waw::data().is_none() {
            return None;
        }
        if self.bo1_bypass.as_deref() == Some(key) {
            self.bo1_bypass = None;
            return None;
        }
        let (class, rest) = key.split_once("_popup_cac_")?;
        let base = self.bo1_class_base()?;
        match rest {
            "primary" | "primary2" | "secondary" => Some(key_for("game", class, if rest == "primary" { base + 1 } else { base + 3 }, rest)),
            "camo" if bo1::is_index(self.stat(base + 1)) => Some(key_for("popup_cac_camo", class, base + 1, "")),
            "camo" if crate::waw::is_index(self.stat(base + 1)) => Some(super::waw::camo_key(class, base + 1)),
            _ => None,
        }
    }

    /// Black Ops' popups, built from the class's CoD4 ones.
    pub(super) fn bo1_menu(&self, key: &str) -> Option<Arc<Menu>> {
        let parts: Vec<&str> = key.strip_prefix(KEY_PREFIX)?.split('|').collect();
        let [kind, class, stat, arg] = parts[..] else { return None };
        let stat: i32 = stat.parse().ok()?;
        let template = |name: &str| self.assets.menu(&format!("{class}_{name}"));
        let click = |script: String| format!("\"play\" \"mouse_click\" ; {script}");
        // "Call of Duty 4" / "Black Ops" / "World at War" (the ones
        // installed), in the weapon groups' popup.
        if kind == "game" {
            let t = template("popup_cac_primary")?;
            let games = [("T5_GAME_COD4", "cod4", true), ("T5_GAME_BO1", "bo1", bo1::data().is_some()), ("T5_GAME_WAW", "waw", crate::waw::data().is_some())];
            let rows: Vec<Row> = games
                .into_iter()
                .filter(|g| g.2)
                .map(|(label, game, _)| Row {
                    label: str_exp(&format!("@{label}")),
                    action: click(format!("\"close\" \"self\" ; \"uiScript\" \"t5Game\" \"{game}\" \"{class}\" \"{stat}\" \"{arg}\" ;")),
                    rename: None,
                })
                .collect();
            return Some(Arc::new(with_rows(&t, key, &rows)?));
        }
        let data = bo1::data()?;
        let menu = match kind {
            "groups" => {
                let t = template("popup_cac_primary")?;
                let rows: Vec<Row> = Group::PRIMARY
                    .iter()
                    .filter(|g| data.guns.iter().any(|gun| gun.group == **g))
                    .map(|g| Row {
                        label: str_exp(g.cod4().1),
                        action: click(format!("\"uiScript\" \"t5List\" \"{class}\" \"{stat}\" \"{}\" ;", group_name(*g))),
                        rename: None,
                    })
                    .collect();
                with_rows(&t, key, &rows)?
            }
            "list" => {
                let group = group_from_name(arg)?;
                let t = template(if group == Group::Pistol { "popup_cac_secondary" } else { list_template(group) })?;
                let from = highlighted_name(&t, if group == Group::Pistol { "ui_sidearm_highlighted" } else { "ui_primary_highlighted" });
                let rows: Vec<Row> = data
                    .guns
                    .iter()
                    .filter(|g| g.group == group)
                    .map(|g| Row {
                        label: str_exp(&format!("@T5_{}", g.key.to_ascii_uppercase())),
                        action: click(format!("\"uiScript\" \"t5Pick\" \"{class}\" \"{stat}\" \"{}\" ;", g.index)),
                        rename: from.clone().map(|f| (f, g.id())),
                    })
                    .collect();
                let mut m = with_rows(&t, key, &rows)?;
                // The popup's own preview shows the gun under the mouse.
                m.on_open = format!(
                    "\"execnow\" \"set ui_primary_highlighted {}; set ui_sidearm_highlighted {}\" ; ",
                    rows.first().and_then(|r| r.rename.as_ref()).map_or("", |r| r.1.as_str()),
                    rows.first().and_then(|r| r.rename.as_ref()).map_or("", |r| r.1.as_str()),
                );
                m
            }
            "attachment_popup" => {
                let gun = data.guns.iter().find(|g| g.index == self.stat(stat))?;
                let t = self.assets.menu(&format!("{class}_attachment_popup_{}", attachment_template(gun.group)))?;
                let from = highlighted_name(&t, "ui_attachment_highlighted");
                // Rows pick like CoD4's (`statsetusingtable`), so the
                // interpreter toggles them and adds "Accept".
                let next = if stat % 10 == 1 { format!("\"open\" \"{}\" ; ", key_for("popup_cac_camo", class, stat, "")) } else { String::new() };
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
                        (a.clone(), format!("@T5_{}", key.to_ascii_uppercase()))
                    }))
                    .map(|(name, label)| Row { label: str_exp(&label), action: pick(&name), rename: from.clone().map(|f| (f, name)) })
                    .collect();
                with_rows(&t, key, &rows)?
            }
            "popup_cac_camo" => {
                let t = template("popup_cac_camo")?;
                let from = highlighted_name(&t, "ui_camo_highlighted");
                let rows: Vec<Row> = data
                    .camos
                    .iter()
                    .map(|c| {
                        let name = format!("{CAMO_PREFIX}{}", c.index);
                        Row {
                            label: str_exp(&format!("@T5_{}", c.key.to_ascii_uppercase())),
                            action: click(format!(
                                "\"statsetusingtable\" ( \"{}\" , \"tablelookup\" ( \"{}\" , 4 , \"{name}\" , \"11\" ) ) ; \"close\" \"self\" ;",
                                stat - stat % 10 + 9,
                                attachments::TABLE
                            )),
                            rename: from.clone().map(|f| (f, name)),
                        }
                    })
                    .collect();
                with_rows(&t, key, &rows)?
            }
            _ => return None,
        };
        Some(Arc::new(menu))
    }

    /// `uiScript t5Game <cod4|bo1|waw> <class> <stat> <popup>`,
    /// `t5List <class> <stat> <group>`, `t5Pick <class> <stat> <index>`.
    pub(super) fn bo1_script(&mut self, args: &[String]) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        let stat: i32 = a(3).parse().unwrap_or(0);
        match a(0).to_ascii_lowercase().as_str() {
            "t5game" => {
                let (class, popup) = (a(2), a(4));
                if a(1) == "cod4" {
                    let original = format!("{class}_popup_cac_{popup}");
                    self.bo1_bypass = Some(original.clone());
                    self.open(&original);
                } else if a(1) == "waw" {
                    self.waw_game(class, stat, popup);
                } else if popup == "secondary" {
                    self.set_dvar("ui_weapon_class_selected", "@MPUI_PISTOLS");
                    self.open(&key_for("list", class, stat, group_name(Group::Pistol)));
                } else {
                    self.open(&key_for("groups", class, stat, ""));
                }
            }
            "t5list" => {
                if let Some(g) = group_from_name(a(3)) {
                    let stat: i32 = a(2).parse().unwrap_or(0);
                    self.set_dvar("ui_weapon_class_selected", g.cod4().1);
                    self.open(&key_for("list", a(1), stat, group_name(g)));
                }
            }
            "t5pick" => {
                let (class, stat): (&str, i32) = (a(1), a(2).parse().unwrap_or(0));
                let index: i32 = a(3).parse().unwrap_or(0);
                let Some(gun) = bo1::data().and_then(|d| d.guns.iter().find(|g| g.index == index)) else { return };
                let has_attachments = !gun.attachments.is_empty();
                self.stats.set(stat, index);
                self.stats.set(stat + 1, 0);
                self.stats.set(attachments::SET_STATS + stat, 0);
                if stat % 10 == 1 {
                    self.stats.set(stat - stat % 10 + 9, 0);
                }
                // Close the weapon popups, CoD4's and ours.
                let open: Vec<String> = self.stack.iter().map(|m| m.name.clone()).collect();
                for name in open.iter().filter(|n| n.starts_with(KEY_PREFIX) || n.contains("_popup_cac_") || n.contains("_attachment_popup_")) {
                    self.close(name);
                }
                if has_attachments {
                    self.open(&key_for("attachment_popup", class, stat, ""));
                } else if stat % 10 == 1 {
                    self.open(&key_for("popup_cac_camo", class, stat, ""));
                }
            }
            _ => {}
        }
    }
}

fn key_for(kind: &str, class: &str, stat: i32, arg: &str) -> String {
    key(kind, class, stat, arg)
}

fn group_name(g: Group) -> &'static str {
    match g {
        Group::Assault => "assault",
        Group::Smg => "smg",
        Group::Lmg => "lmg",
        Group::Sniper => "sniper",
        Group::Shotgun => "shotgun",
        Group::Pistol => "pistol",
    }
}

fn group_from_name(name: &str) -> Option<Group> {
    [Group::Assault, Group::Smg, Group::Lmg, Group::Sniper, Group::Shotgun, Group::Pistol].into_iter().find(|g| group_name(*g) == name)
}

/// The class's CoD4 weapon list a group's list is made from.
fn list_template(g: Group) -> &'static str {
    match g {
        Group::Assault => "popup_cac_assault",
        Group::Smg => "popup_cac_smg",
        Group::Lmg => "popup_cac_lmg",
        Group::Sniper => "popup_cac_sniper",
        Group::Shotgun | Group::Pistol => "popup_cac_shotgun",
    }
}

/// The class's CoD4 attachment popup a group's is made from.
fn attachment_template(g: Group) -> &'static str {
    match g {
        Group::Assault => "assault",
        Group::Smg => "smg",
        Group::Lmg => "lmg",
        Group::Sniper => "sniper",
        Group::Shotgun => "shotgun",
        Group::Pistol => "pistol",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let k = key("attachment_popup", "heavy_gunner", 213, "");
        assert_eq!(key_stat(&k), Some(213));
        assert!(k.contains("attachment_popup"));
        assert_eq!(retarget_focus("\"setLocalVarInt\" \"ui_highlight\" 1 ; x", 7), "\"setLocalVarInt\" \"ui_highlight\" 7 ; x");
    }
}
