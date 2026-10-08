//! Supply drops in the menus ([`crate::supply`]): "Character" and "Supply
//! Drops" entries on the main menu, the screen that opens drops (the three
//! items turned over one by one, guns and characters in 3D), the screen that
//! picks the character worn, and a variant row for each weapon in Create a
//! Class with a popup of the variants collected for that gun. They are made
//! of menu items like CoD4's own, so the interpreter lays them out, focuses
//! and runs them; the cards and details are drawn here.

use super::attachments::{highlight_rows, retarget_highlight};
use bevy::math::Vec2;
use super::draw::{self, Placement};
use super::expr::Env;
use super::{Frontend, Op, OpenMenu};
use crate::characters::Character;
use crate::supply::{self, Attribute, Item as DropItem, Rarity, Variant};
use iw3::menu::{Item, Menu, Rect as VRect, Statement, Token, flags, item_type, op};
use std::sync::Arc;

/// The screen that opens drops, and Create a Class's variant popup.
const DROPS_MENU: &str = "supply_drops";
const VARIANTS_MENU: &str = "supply_variants";
/// The screen that picks the character worn.
const CHARACTER_MENU: &str = "supply_character";
/// Its page of the collection, and the character under the mouse.
const CHAR_PAGE_DVAR: &str = "ui_character_page";
const CHAR_HIGHLIGHT_DVAR: &str = "ui_character_highlighted";
/// Characters listed per page.
const CHAR_PAGE_SIZE: usize = 13;
/// The main menu entry's text: "Supply Drops (2)".
const LABEL_DVAR: &str = "ui_supply_label";
/// "1" while there are drops to open.
const CAN_OPEN_DVAR: &str = "ui_supply_can_open";
/// When the last drop was opened ([`Env::millis`]), for the reveal.
const OPENED_DVAR: &str = "ui_supply_opened";
/// The class weapon stat the variant popup is for.
const SLOT_DVAR: &str = "ui_supply_slot";
/// The variant under the mouse in the popup ("none": standard issue).
const HIGHLIGHT_DVAR: &str = "ui_variant_highlighted";
/// Preview keys for the three cards and the character screen's figure (0
/// and 201..249 are Create a Class's).
const CARD_KEY: i32 = 1000;
const FIGURE_KEY: i32 = 1010;

/// Card layout, right-aligned virtual units.
const CARD_X: f32 = -446.0;
const CARD_Y: f32 = 50.0;
const CARD_W: f32 = 136.0;
const CARD_H: f32 = 280.0;
const CARD_GAP: f32 = 8.0;
/// The first card turns over this long after opening, the others after it.
const REVEAL_FIRST_MS: i64 = 450;
const REVEAL_STEP_MS: i64 = 550;
const FLASH_MS: i64 = 400;

const GREY: [f32; 4] = [0.69, 0.69, 0.69, 1.0];
const UP: [f32; 4] = [0.55, 0.95, 0.55, 1.0];
const DOWN: [f32; 4] = [1.0, 0.45, 0.4, 1.0];

fn str_exp(s: &str) -> Statement {
    vec![Token::Op(op::LEFTPAREN), Token::Str(s.into())]
}

fn dvar_exp(name: &str) -> Statement {
    vec![Token::Op(op::LEFTPAREN), Token::Op(op::DVARSTRING), Token::Str(name.into()), Token::Op(op::RIGHTPAREN)]
}

/// `localVarInt("ui_highlight") == row && localVarString("ui_choicegroup") == group`.
fn highlight_exp(row: i32, group: &str) -> Statement {
    vec![
        Token::Op(op::LEFTPAREN),
        Token::Op(op::LOCALVARINT),
        Token::Str("ui_highlight".into()),
        Token::Op(op::RIGHTPAREN),
        Token::Op(op::EQUALS),
        Token::Int(row),
        Token::Op(op::AND),
        Token::Op(op::LOCALVARSTRING),
        Token::Str("ui_choicegroup".into()),
        Token::Op(op::RIGHTPAREN),
        Token::Op(op::EQUALS),
        Token::Str(group.into()),
    ]
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> VRect {
    VRect { x, y, w, h, horz_align: 1, vert_align: 1 }
}

/// A picture or fill (`white`) that takes no clicks.
fn deco(r: VRect, material: &str, color: [f32; 4]) -> Item {
    let mut it = Item::default();
    it.window.rect = r;
    it.window.style = 3;
    it.window.fore_color = color;
    it.window.background = Some(material.into());
    it.window.static_flags = flags::DECORATION;
    it.window.dynamic_flags = flags::VISIBLE;
    it
}

/// A text-only button, text right-aligned like CoD4's list rows.
fn button(r: VRect, label: Statement, action: &str, on_focus: &str, color: [f32; 4]) -> Item {
    let mut it = Item::default();
    it.ty = item_type::BUTTON;
    it.window.rect = r;
    it.window.fore_color = color;
    it.window.dynamic_flags = flags::VISIBLE;
    it.font_enum = 1;
    it.text_scale = 0.4;
    it.text_align_mode = 10;
    it.text_align_x = -6.0;
    it.text_style = 6;
    it.text_exp = label;
    it.action = action.into();
    it.on_focus = on_focus.into();
    it
}

/// Left-column items of a menu (rows, their backgrounds and highlights).
fn left_column(it: &Item) -> bool {
    it.window.rect.horz_align == 1 && it.window.rect.x < 230.0
}

/// Copies of the left-column row whose items sit at `y`.
fn row_at(menu: &Menu, y: f32) -> Vec<Item> {
    menu.items.iter().filter(|it| left_column(it) && (it.window.rect.y - y).abs() < 0.5).cloned().collect()
}

fn next_highlight(menu: &Menu) -> i32 {
    menu.items.iter().flat_map(|it| highlight_rows(&it.visible_exp)).max().unwrap_or(0) + 1
}

/// Point `"setLocalVarInt" "ui_highlight" N` in a script at another row.
fn retarget_focus(script: &str, row: i32) -> String {
    let key = "\"ui_highlight\" ";
    let Some(at) = script.find(key) else { return script.into() };
    let start = at + key.len();
    let end = script[start..].find(|c: char| !c.is_ascii_digit()).map_or(script.len(), |e| start + e);
    format!("{}{row}{}", &script[..start], &script[end..])
}

/// A copied row moved to `y` as highlight row `row`, its button showing
/// `label` and doing `action`.
fn place_row(template: &[Item], y: f32, row: i32, label: Statement, action: &str) -> Vec<Item> {
    template
        .iter()
        .cloned()
        .map(|mut it| {
            it.window.rect.y = y;
            it.window.name.clear();
            retarget_highlight(&mut it.visible_exp, row);
            if it.ty == item_type::BUTTON {
                it.text_exp = label.clone();
                it.action = action.into();
                it.on_focus = retarget_focus(&it.on_focus, row);
                it.dvar_flags = 0;
            }
            it
        })
        .collect()
}

/// The main menu with "Character" and "Supply Drops" under "Create a
/// Class". The rows from "Join Game" to "Create a Class" move up two (the
/// column is empty above them; the auto update row there is never shown) to
/// make room.
fn main_with_drops(menu: &Menu) -> Option<Menu> {
    const DROPS_Y: f32 = 226.0;
    const PITCH: f32 = 24.0;
    // The "Rank & Challenges" row below is the template.
    let template = row_at(menu, DROPS_Y + PITCH);
    template.iter().find(|it| it.ty == item_type::BUTTON)?;
    let row = next_highlight(menu);
    let mut out = menu.clone();
    for it in out.items.iter_mut().filter(|it| left_column(it) && it.window.rect.y < DROPS_Y + 0.5) {
        it.window.rect.y -= 2.0 * PITCH;
    }
    let open = |menu: &str| format!("\"play\" \"mouse_click\" ; \"open\" \"{menu}\" ; ");
    out.items.extend(place_row(&template, DROPS_Y - PITCH, row, str_exp("Character"), &open(CHARACTER_MENU)));
    out.items.extend(place_row(&template, DROPS_Y, row + 1, dvar_exp(LABEL_DVAR), &open(DROPS_MENU)));
    Some(out)
}

/// The class a Create a Class menu edits: its first stat (200, 210, ...),
/// from `statGetInDvar 206 ui_perk2_selection` in its onOpen.
fn class_base(menu: &Menu) -> Option<i32> {
    let lower = menu.on_open.to_ascii_lowercase();
    let at = lower.find("statgetindvar ")? + "statgetindvar ".len();
    let n: i32 = lower[at..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    Some(n - n % 10)
}

/// A Create a Class menu with "Primary Variant" and "Secondary Variant"
/// rows under "Reset to Default", copies of its "Rename" row.
fn cac_with_variants(menu: &Menu) -> Option<Menu> {
    let base = class_base(menu)?;
    let template = row_at(menu, 198.0);
    template.iter().find(|it| it.ty == item_type::BUTTON)?;
    let row = next_highlight(menu);
    let mut out = menu.clone();
    // A group gap like the menu's own between "Perk 3" and "Rename".
    if let Some(gap) = menu.items.iter().find(|it| left_column(it) && it.window.rect.h == 8.0 && it.ty == item_type::TEXT) {
        let mut gap = gap.clone();
        gap.window.rect.y = 246.0;
        out.items.push(gap);
    }
    for (k, (slot, label)) in [(1, "Primary Variant"), (3, "Secondary Variant")].into_iter().enumerate() {
        let action = format!("\"play\" \"mouse_click\" ; \"uiScript\" \"supplyVariants\" \"{}\" ; ", base + slot);
        out.items.extend(place_row(&template, 254.0 + 24.0 * k as f32, row + k as i32, str_exp(label), &action));
    }
    Some(out)
}

/// Drawing in virtual units on top of the menus.
struct Painter<'a> {
    fe: &'a Frontend,
    pl: &'a Placement,
    ops: &'a mut Vec<Op>,
    horz_align: u8,
    /// Everything moved down this far (a card being dealt up).
    dy: f32,
}

impl Painter<'_> {
    fn r(&self, x: f32, y: f32, w: f32, h: f32) -> (bevy::math::Vec2, bevy::math::Vec2) {
        self.pl.rect(&VRect { x, y: y + self.dy, w, h, horz_align: self.horz_align, vert_align: 1 })
    }

    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let (pos, size) = self.r(x, y, w, h);
        self.ops.push(Op::Fill { pos, size, color });
    }

    fn pic(&mut self, x: f32, y: f32, w: f32, h: f32, material: &str, color: [f32; 4]) {
        let (pos, size) = self.r(x, y, w, h);
        self.ops.push(Op::Pic { pos, size, material: material.into(), color });
    }

    /// A line's width in virtual units.
    fn width(&self, scale: f32, text: &str) -> f32 {
        let height = scale * 48.0 * self.pl.scale;
        let f = &self.fe.assets.fonts[self.fe.font_for(1, height)];
        draw::text_width(f, text, height / f.pixel_height as f32) / self.pl.scale
    }

    /// One line with its baseline at `y`, `align` 0 left, 1 centred on `x`,
    /// 2 right. Returns its width.
    /// Lines centred on `x` from baseline `y`, wrapped to `width`. Returns
    /// how many.
    fn wrapped(&mut self, x: f32, y: f32, scale: f32, width: f32, text: &str, color: [f32; 4]) -> usize {
        let height = scale * 48.0 * self.pl.scale;
        let f = &self.fe.assets.fonts[self.fe.font_for(1, height)];
        let lines = draw::wrap(f, text, height / f.pixel_height as f32, width * self.pl.scale);
        for (i, line) in lines.iter().enumerate() {
            self.text(x, y + i as f32 * scale * 48.0, 1, scale, line, color);
        }
        lines.len()
    }

    fn text(&mut self, x: f32, y: f32, align: u8, scale: f32, text: &str, color: [f32; 4]) -> f32 {
        let height = scale * 48.0 * self.pl.scale;
        let font = self.fe.font_for(1, height);
        let f = &self.fe.assets.fonts[font];
        let k = height / f.pixel_height as f32;
        let w = draw::text_width(f, text, k);
        let (pos, _) = self.r(x, y, 0.0, 0.0);
        let x = pos.x - [0.0, w * 0.5, w][align.min(2) as usize];
        self.ops.push(Op::Text { text: text.into(), x, y: pos.y, font, k, color, shadow: self.pl.scale });
        w / self.pl.scale
    }

    fn border(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        self.fill(x, y, w, 1.0, color);
        self.fill(x, y + h - 1.0, w, 1.0, color);
        self.fill(x, y, 1.0, h, color);
        self.fill(x + w - 1.0, y, 1.0, h, color);
    }
}

/// Ops grown about `centre` by `k` (window pixels).
fn scale_ops(ops: &mut [Op], centre: Vec2, k: f32, to: Vec2) {
    let at = |p: Vec2| to + (p - centre) * k;
    for op in ops {
        match op {
            Op::Fill { pos, size, .. } | Op::Pic { pos, size, .. } | Op::Image { pos, size, .. } | Op::Gun { pos, size, .. } => {
                *pos = at(*pos);
                *size *= k;
            }
            Op::Text { x, y, k: tk, shadow, .. } => {
                let p = at(Vec2::new(*x, *y));
                (*x, *y) = (p.x, p.y);
                *tk *= k;
                *shadow *= k;
            }
        }
    }
}

/// Ops squeezed sideways about `cx` to `w` of their width (a card turning).
fn squash(ops: &mut [Op], cx: f32, w: f32) {
    for op in ops {
        match op {
            Op::Fill { pos, size, .. } | Op::Pic { pos, size, .. } | Op::Image { pos, size, .. } | Op::Gun { pos, size, .. } => {
                pos.x = cx + (pos.x - cx) * w;
                size.x *= w;
            }
            Op::Text { x, k, .. } => {
                *x = cx + (*x - cx) * w;
                *k *= w.max(0.3);
            }
        }
    }
}

/// Attribute bar points per step of a variant's tweak.
const ATTRIBUTE_STEP: i32 = 8;

fn with_alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * a]
}

impl Frontend {
    /// Menus as supply drops need them: their own built here, the main menu
    /// and Create a Class with rows added, others as they are.
    pub(super) fn supply_menu(&mut self, key: &str, menu: Option<Arc<Menu>>) -> Option<Arc<Menu>> {
        match key {
            DROPS_MENU => {
                self.refresh_supply();
                self.drops_menu().map(Arc::new)
            }
            VARIANTS_MENU => self.variants_menu().map(Arc::new),
            CHARACTER_MENU => self.character_menu().map(Arc::new),
            "main_text" => {
                self.refresh_supply();
                let menu = menu?;
                Some(main_with_drops(&menu).map_or(menu, Arc::new))
            }
            _ if key.starts_with("menu_cac_") => {
                let menu = menu?;
                Some(cac_with_variants(&menu).map_or(menu, Arc::new))
            }
            _ => menu,
        }
    }

    fn refresh_supply(&mut self) {
        let unopened = supply::inventory().unopened;
        let label = if unopened > 0 { format!("Supply Drops ({unopened})") } else { "Supply Drops".into() };
        self.set_dvar(LABEL_DVAR, &label);
        self.set_dvar(CAN_OPEN_DVAR, if unopened > 0 { "1" } else { "0" });
    }

    /// `uiScript supplyOpen`, `supplyVariants <weapon stat>`,
    /// `supplyEquip <weapon stat> <variant id or none>`, `supplyWear <id>`
    /// and `supplyCharPage` (the character screen's next page).
    pub(super) fn supply_script(&mut self, args: &[String]) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        match a(0).to_ascii_lowercase().as_str() {
            "supplyopen" => {
                let opened = supply::inventory().open().is_some();
                if opened {
                    supply::inventory().save_if_changed();
                    let now = self.millis().to_string();
                    self.set_dvar(OPENED_DVAR, &now);
                }
                self.refresh_supply();
            }
            "supplyvariants" => {
                self.set_dvar(SLOT_DVAR, a(1));
                self.set_dvar(HIGHLIGHT_DVAR, "");
                self.open(VARIANTS_MENU);
            }
            "supplyequip" => {
                if let Ok(stat) = a(1).parse() {
                    let mut inv = supply::inventory();
                    inv.equip(stat, Some(a(2)).filter(|id| *id != "none"));
                    inv.save_if_changed();
                }
            }
            "supplywear" => {
                let mut inv = supply::inventory();
                inv.wear(a(1));
                inv.save_if_changed();
            }
            "supplycharpage" => {
                let pages = supply::inventory().characters_owned().len().div_ceil(CHAR_PAGE_SIZE).max(1);
                let page = (self.dvar(CHAR_PAGE_DVAR).parse::<usize>().unwrap_or(0) + 1) % pages;
                self.set_dvar(CHAR_PAGE_DVAR, &page.to_string());
                // Rebuilt with the page's rows.
                self.close(CHARACTER_MENU);
                self.open(CHARACTER_MENU);
            }
            _ => bevy::log::debug!("ui: unhandled uiScript {args:?}"),
        }
    }

    /// The camo a class weapon's preview shows: an Elite variant's signature
    /// camo (the one under the mouse in the variant popup, else the one
    /// equipped) when the class picks none.
    pub(super) fn variant_camo(&self, weapon_stat: i32, weapon: &str, camo: i32) -> i32 {
        if camo != 0 {
            return camo;
        }
        let hovered = self.stack.last().is_some_and(|m| m.name == VARIANTS_MENU)
            && self.dvar(SLOT_DVAR).parse() == Ok(weapon_stat)
            && !self.dvar(HIGHLIGHT_DVAR).is_empty();
        let v = match hovered {
            true => supply::variant(&self.dvar(HIGHLIGHT_DVAR)),
            false => supply::inventory().equipped(weapon_stat, weapon),
        };
        v.filter(|v| v.weapon == weapon).map_or(0, |v| v.camo as i32)
    }

    /// How much a variant moves a class weapon's attribute bar
    /// (`mp/attributestable.csv` column `col`, 2..6: accuracy, damage,
    /// range, fire rate, mobility): its tweaks to that quality, in steps of
    /// [`ATTRIBUTE_STEP`]. The variant is the one under the mouse in the
    /// variant popup, else the one equipped, on the open class's primary or
    /// secondary when that's `weapon`.
    pub(super) fn variant_attribute(&self, weapon: &str, col: i32) -> i32 {
        let attribute = match col {
            2 => Attribute::Accuracy,
            3 => Attribute::Damage,
            4 => Attribute::Range,
            5 => Attribute::FireRate,
            6 => Attribute::Mobility,
            _ => return 0,
        };
        let Some(base) = self.stack.iter().rev().find(|m| m.name.starts_with("menu_cac_")).and_then(|m| class_base(&m.menu)) else {
            return 0;
        };
        for weapon_stat in [base + 1, base + 3] {
            let own = self.table_lookup("mp/statstable.csv", 0, &self.stat(weapon_stat).to_string(), 4);
            if !own.eq_ignore_ascii_case(weapon) {
                continue;
            }
            let hovered = self.stack.last().is_some_and(|m| m.name == VARIANTS_MENU)
                && self.dvar(SLOT_DVAR).parse() == Ok(weapon_stat)
                && !self.dvar(HIGHLIGHT_DVAR).is_empty();
            let v = match hovered {
                true => supply::variant(&self.dvar(HIGHLIGHT_DVAR)),
                false => supply::inventory().equipped(weapon_stat, weapon),
            };
            let steps: i32 = v.filter(|v| v.weapon == weapon).map_or(0, |v| {
                v.tweaks.iter().filter(|(a, _)| *a == attribute).map(|(_, s)| *s).sum()
            });
            return steps * ATTRIBUTE_STEP;
        }
        0
    }

    fn weapon_name(&self, weapon: &str) -> String {
        let key = self.table_lookup("mp/statstable.csv", 4, weapon, 3);
        if key.is_empty() { weapon.to_ascii_uppercase() } else { self.localize(&format!("@{key}")) }
    }

    fn camo_name(&self, camo: usize) -> String {
        let key = self.table_lookup("mp/attachmenttable.csv", 11, &camo.to_string(), 3);
        self.localize(&format!("@{key}"))
    }

    /// A full screen of our own on Create a Class's backdrop, title and
    /// footer bars and Back button, and its row template.
    fn screen_menu(&self, name: &str, title: &str) -> Option<(Menu, Vec<Item>)> {
        let cac = self.assets.menu("menu_cac_assault")?;
        let mut m = Menu {
            window: cac.window.clone(),
            font: cac.font.clone(),
            full_screen: true,
            focus_color: cac.focus_color,
            // Create a Class's is see-through; rows grey out with this.
            disable_color: [0.5, 0.5, 0.5, 0.6],
            on_esc: "\"play\" \"mouse_click\" ; \"close\" \"self\" ; ".into(),
            ..Menu::default()
        };
        m.window.name = name.into();
        // Everything not in the two columns.
        for it in cac.items.iter().filter(|it| !matches!(it.window.rect.horz_align, 1 | 3)) {
            let mut it = it.clone();
            if it.text_exp.iter().any(|t| matches!(t, Token::Str(s) if s.eq_ignore_ascii_case("@MPUI_CREATE_A_CLASS_CAP"))) {
                it.text_exp = str_exp(title);
            }
            m.items.push(it);
        }
        Some((m, row_at(&cac, 34.0)))
    }

    /// The drop screen: an "Open Supply Drop" row, and room for the cards.
    fn drops_menu(&self) -> Option<Menu> {
        let (mut m, template) = self.screen_menu(DROPS_MENU, "SUPPLY DROPS")?;
        let open = "\"play\" \"mouse_click\" ; \"uiScript\" \"supplyOpen\" ; ";
        let mut row = place_row(&template, 34.0, 1, str_exp("Open Supply Drop"), open);
        for it in row.iter_mut().filter(|it| it.ty == item_type::BUTTON) {
            // Greyed out with none to open (`enableDvar`).
            it.dvar_test = CAN_OPEN_DVAR.into();
            it.enable_dvar = "1".into();
            it.dvar_flags = 1;
        }
        m.items.extend(row);
        Some(m)
    }

    /// The character screen: a page of the collection as rows in rarity
    /// colours (clicking one wears it), "More" for the next page, and the
    /// character under the mouse in 3D.
    fn character_menu(&self) -> Option<Menu> {
        let (mut m, template) = self.screen_menu(CHARACTER_MENU, "CHARACTER")?;
        let owned = supply::inventory().characters_owned();
        let pages = owned.len().div_ceil(CHAR_PAGE_SIZE).max(1);
        let page = self.dvar(CHAR_PAGE_DVAR).parse::<usize>().unwrap_or(0).min(pages - 1);
        let rows = owned.iter().skip(page * CHAR_PAGE_SIZE).take(CHAR_PAGE_SIZE);
        let mut y = 34.0;
        for (i, c) in rows.enumerate() {
            let action = format!("\"play\" \"mouse_click\" ; \"uiScript\" \"supplyWear\" \"{}\" ; ", c.id);
            for mut it in place_row(&template, y, i as i32 + 1, str_exp(c.name), &action) {
                if it.ty == item_type::BUTTON {
                    it.window.fore_color = with_alpha(c.rarity.color(), 0.9);
                    it.text_scale = 0.33;
                    it.on_focus += &format!(" ; \"setdvar\" \"{CHAR_HIGHLIGHT_DVAR}\" \"{}\" ; ", c.id);
                }
                m.items.push(it);
            }
            y += 24.0;
        }
        if pages > 1 {
            let label = format!("More ({}/{pages})", page + 1);
            let action = "\"play\" \"mouse_click\" ; \"uiScript\" \"supplyCharPage\" ; ";
            m.items.extend(place_row(&template, y + 10.0, CHAR_PAGE_SIZE as i32 + 1, str_exp(&label), action));
        }
        Some(m)
    }

    /// Create a Class's variant popup for the weapon in [`SLOT_DVAR`]: the
    /// standard gun and each variant collected for it, laid out like CoD4's
    /// camo popup.
    fn variants_menu(&self) -> Option<Menu> {
        let stat: i32 = self.dvar(SLOT_DVAR).parse().ok()?;
        let weapon = self.table_lookup("mp/statstable.csv", 0, &self.stat(stat).to_string(), 4);
        let owned = supply::inventory().owned_of(&weapon);
        let cac = self.stack.iter().rev().find(|m| m.name.starts_with("menu_cac_")).map(|m| m.menu.clone());
        let mut m = Menu {
            focus_color: cac.as_ref().map_or([1.0, 0.8, 0.4, 1.0], |c| c.focus_color),
            disable_color: cac.as_ref().map_or([0.5, 0.5, 0.5, 1.0], |c| c.disable_color),
            on_esc: "\"play\" \"mouse_click\" ; \"close\" \"self\" ; ".into(),
            ..Menu::default()
        };
        m.window.name = VARIANTS_MENU.into();
        m.window.rect = rect(0.0, 128.0, 224.0, 168.0);
        let close = "\"play\" \"mouse_click\" ; \"close\" \"self\" ; ";
        // Clicking outside closes it; clicking the box itself does nothing.
        m.items.push(button(rect(-598.0, -670.0, 2000.0, 2000.0), Vec::new(), close, "", GREY));
        m.items.push(button(rect(-2.0, 104.0, 504.0, 194.0), Vec::new(), "; ", "", GREY));
        m.items.push(deco(rect(0.0, 128.0, 224.0, 168.0), "white", [0.2, 0.2, 0.22, 1.0]));
        m.items.push(deco(rect(222.0, 128.0, 280.0, 168.0), "white", [0.4, 0.4, 0.42, 1.0]));
        m.items.push(deco(rect(220.0, 130.0, 280.0, 164.0), "white", [0.2, 0.2, 0.225, 1.0]));
        // The row it was opened from, with an arrow.
        m.items.push(deco(rect(2.0, 106.0, 220.0, 22.0), "white", [0.15, 0.15, 0.17, 1.0]));
        let mut header = button(
            rect(-22.0, 106.0, 220.0, 22.0),
            str_exp(if stat % 10 == 1 { "Primary Variant" } else { "Secondary Variant" }),
            "",
            "",
            GREY,
        );
        header.ty = item_type::TEXT;
        header.text_scale = 0.375;
        header.window.static_flags = flags::DECORATION;
        m.items.push(header);
        m.items.push(deco(rect(202.0, 114.0, 16.0, 8.0), "hitech_arrow_right", [0.55, 0.95, 0.55, 0.7]));
        let entries = std::iter::once(None).chain(owned.into_iter().map(Some));
        for (i, v) in entries.enumerate() {
            let (row, y) = (i as i32 + 1, 134.0 + 20.0 * i as f32);
            let id = v.map_or("none", |v| v.id.as_str());
            m.items.push(deco(rect(4.0, y, 211.5, 18.0), "gradient_fadein", [0.9, 0.9, 1.0, 0.07]));
            m.items.push(deco(rect(215.5, y, 4.5, 18.0), "button_highlight_end", [0.9, 0.9, 1.0, 0.07]));
            for (r, material) in [(rect(4.0, y, 211.5, 18.0), "gradient_fadein"), (rect(215.5, y, 4.5, 18.0), "button_highlight_end")] {
                let mut h = deco(r, material, [0.9, 0.95, 1.0, 0.25]);
                h.visible_exp = highlight_exp(row, "popmenu");
                m.items.push(h);
            }
            let action = format!("\"play\" \"mouse_click\" ; \"uiScript\" \"supplyEquip\" \"{stat}\" \"{id}\" ; \"close\" \"self\" ; ");
            let focus = format!(
                "\"play\" \"mouse_submenu_over\" ; \"setLocalVarInt\" \"ui_highlight\" {row} ; \
                 \"setLocalVarString\" \"ui_choicegroup\" \"popmenu\" ; \"setdvar\" \"{HIGHLIGHT_DVAR}\" \"{id}\" ; "
            );
            let (label, color) = match v {
                Some(v) => (v.name, with_alpha(v.rarity.color(), 0.9)),
                None => ("Standard Issue", GREY),
            };
            m.items.push(button(rect(4.0, y, 216.0, 18.0), str_exp(label), &action, &focus, color));
        }
        Some(m)
    }

    /// Drawing for the supply drop menus: the cards, the variant details,
    /// and the variants on Create a Class's weapons.
    pub(super) fn paint_supply(&self, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
        if om.name == DROPS_MENU {
            self.paint_drops(pl, ops);
        } else if om.name == CHARACTER_MENU {
            self.paint_characters(om, pl, ops);
        } else if om.name == VARIANTS_MENU {
            self.paint_variants(om, pl, ops);
        } else if om.name.starts_with("menu_cac_") {
            if let Some(base) = class_base(&om.menu) {
                self.paint_cac_variants(base, pl, ops);
            }
        }
    }

    /// Headquarters' supply drop ([`crate::hq`]): the drop's cards in a row
    /// across the middle, each dealt up from below as its time comes (`age`:
    /// seconds since; `None`, not yet), face down until it lands, then
    /// turned over.
    pub(super) fn paint_reveal_cards(&self, pl: &Placement, ops: &mut Vec<Op>, cards: &[(DropItem, bool, Option<f32>)], since: f32) {
        /// Bigger than the menu's, centred.
        const SCALE: f32 = 1.15;
        const RISE_FROM: f32 = 260.0;
        // Dealt face down, then a beat, then turned.
        let deal = crate::hq::DEAL;
        let hold = crate::hq::FACE_DOWN;
        // The stage: the world dimmed behind, darker at the edges.
        let dim = (since / 0.5).clamp(0.0, 1.0);
        let start = ops.len();
        let mut p = Painter { fe: self, pl, ops, horz_align: 4, dy: 0.0 };
        p.fill(0.0, 0.0, 640.0, 480.0, [0.0, 0.0, 0.0, 0.5 * dim]);
        p.fill(0.0, 0.0, 640.0, 60.0, [0.0, 0.0, 0.0, 0.35 * dim]);
        p.fill(0.0, 420.0, 640.0, 60.0, [0.0, 0.0, 0.0, 0.35 * dim]);
        // Each card's turn: a flash of its rarity's colour over the stage.
        for &(item, _, age) in cards {
            if let Some(t) = age.map(|a| a - deal - hold).filter(|t| (0.0..0.6).contains(t)) {
                let strength = match item.rarity() {
                    Rarity::Elite => 0.4,
                    Rarity::Professional => 0.28,
                    Rarity::Veteran => 0.2,
                    _ => 0.12,
                };
                p.fill(0.0, 0.0, 640.0, 480.0, with_alpha(item.rarity().color(), strength * (1.0 - t / 0.6).powi(2)));
            }
        }
        let back = p.ops.len();
        p.horz_align = 2;
        p.text(0.0, CARD_Y - 16.0, 1, 0.55, "SUPPLY DROP", [1.0, 0.85, 0.45, dim]);
        let total = 3.0 * CARD_W + 2.0 * CARD_GAP;
        let now = self.millis();
        let mut out = 0;
        for (i, &(item, new, age)) in cards.iter().enumerate() {
            let Some(age) = age else { continue };
            let x = -total * 0.5 + i as f32 * (CARD_W + CARD_GAP);
            let k = (age / deal).min(1.0);
            p.dy = RISE_FROM * (1.0 - k).powi(3);
            // Solid cards: nothing of the world shows through.
            p.fill(x, CARD_Y, CARD_W, CARD_H, [0.045, 0.045, 0.05, 1.0]);
            if age < deal + hold {
                p.sealed(x, now, age >= deal);
            } else {
                let turned = age - deal - hold;
                // Turning over: narrow to an edge and open out again.
                let w = (turned / 0.18).clamp(0.0, 1.0);
                let before = p.ops.len();
                p.card(x, item, new, i, (turned * 1000.0) as i64);
                if w < 1.0 {
                    let c = p.r(x + CARD_W * 0.5, CARD_Y, 0.0, 0.0).0.x;
                    squash(&mut p.ops[before..], c, w.max(0.05));
                }
                out += 1;
            }
        }
        // What came out, once it all has.
        if out == cards.len() && out > 0 {
            let new = cards.iter().filter(|c| c.1).count();
            let best = cards.iter().map(|c| c.0.rarity()).max_by_key(|r| *r as u8).unwrap_or(Rarity::Enlisted);
            let line = if new > 0 { format!("{new} new for your collection  ·  best: {}", best.name()) } else { format!("All duplicates  ·  best: {}", best.name()) };
            p.dy = 0.0;
            p.text(0.0, CARD_Y + CARD_H + 26.0, 1, 0.34, &line, with_alpha(GREY, 0.95));
        }
        // Bigger, in the screen's middle (the stage itself stays put).
        let _ = start;
        let middle = (CARD_Y + CARD_H * 0.5) / 480.0 * pl.h;
        scale_ops(&mut ops[back..], Vec2::new(pl.w * 0.5, middle), SCALE, Vec2::new(pl.w * 0.5, pl.h * 0.5));
    }

    fn paint_drops(&self, pl: &Placement, ops: &mut Vec<Op>) {
        let inv = supply::inventory();
        let mut p = Painter { fe: self, pl, ops, horz_align: 1, dy: 0.0 };
        // What's waiting and what's next, under the row.
        let (x, mut y) = (14.0, 92.0);
        p.fill(0.0, 64.0, 222.0, 182.0, [0.0, 0.0, 0.0, 0.35]);
        let waiting = match inv.unopened {
            0 => "No supply drops to open".to_owned(),
            1 => "1 supply drop to open".to_owned(),
            n => format!("{n} supply drops to open"),
        };
        p.text(x, y, 0, 0.375, &waiting, if inv.unopened > 0 { Rarity::Elite.color() } else { GREY });
        y += 20.0;
        let left = supply::drop_interval() - inv.progress;
        p.text(x, y, 0, 0.3, &format!("Next drop in {} of match time", supply::format_time(left)), GREY);
        y += 16.0;
        p.text(x, y, 0, 0.3, &format!("Variants collected: {} of {}", inv.owned_count(), supply::catalogue_size()), GREY);
        y += 16.0;
        let characters = inv.characters_owned().len();
        p.text(x, y, 0, 0.3, &format!("Characters collected: {characters} of {}", crate::characters::CHARACTERS.len()), GREY);
        y += 26.0;
        p.text(x, y, 0, 0.3, "Each drop holds 3 variants or characters:", GREY);
        for rarity in supply::RARITIES {
            y += 16.0;
            let w = p.text(x, y, 0, 0.3, rarity.name(), rarity.color());
            p.text(x + w + 6.0, y, 0, 0.3, &format!("{:.0}%  {}", rarity.chance() * 100.0, rarity.summary()), GREY);
        }

        // The cards.
        p.horz_align = 3;
        let opened: Option<i64> = self.dvar(OPENED_DVAR).parse().ok();
        let now = self.millis();
        for i in 0..supply::DROP_SIZE {
            let x = CARD_X + i as f32 * (CARD_W + CARD_GAP);
            let item = inv.last_drop.get(i).copied();
            let shown_at = opened.map(|t| t + REVEAL_FIRST_MS + REVEAL_STEP_MS * i as i64);
            match (item, shown_at) {
                (Some((item, new)), Some(at)) if now >= at => p.card(x, item, new, i, now - at),
                (Some((item, new)), None) => p.card(x, item, new, i, FLASH_MS),
                _ => p.sealed(x, now, opened.is_some()),
            }
        }
        if inv.last_drop.is_empty() {
            let mid = CARD_X + (3.0 * CARD_W + 2.0 * CARD_GAP) * 0.5;
            let hint = if inv.unopened > 0 { "Open a supply drop to see what's inside" } else { "Play matches to earn supply drops" };
            p.text(mid, CARD_Y + CARD_H + 22.0, 1, 0.33, hint, GREY);
        }
    }

    fn paint_characters(&self, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
        let inv = supply::inventory();
        let worn = inv.wearing();
        let mut p = Painter { fe: self, pl, ops, horz_align: 1, dy: 0.0 };
        // On the rows: a tick on the one worn, and each one's game where
        // its name leaves room.
        for (i, it) in om.menu.items.iter().enumerate() {
            let Some(id) = it.action.split("\"supplyWear\"").nth(1).and_then(|r| r.split('"').nth(1)) else { continue };
            if om.shown[i] {
                let r = it.window.rect;
                let material = if id == worn.id { "hud_checkbox_checked" } else { "hud_checkbox_clear" };
                p.pic(r.x + 6.0, r.y + 5.0, 12.0, 12.0, material, [1.0; 4]);
                if let Some(c) = crate::characters::character(id) {
                    let tag = c.game.short();
                    let name_left = r.x + r.w + it.text_align_x - p.width(it.text_scale, c.name);
                    if r.x + 22.0 + p.width(0.22, tag) + 4.0 < name_left {
                        p.text(r.x + 22.0, r.y + 15.0, 0, 0.22, tag, with_alpha(GREY, 0.75));
                    }
                }
            }
        }
        // The one under the mouse (or worn), in 3D on the right.
        let highlighted = self.dvar(CHAR_HIGHLIGHT_DVAR);
        let c = crate::characters::character(&highlighted).filter(|c| inv.owns_character(c.id)).unwrap_or(worn);
        p.horz_align = 3;
        let (x, w) = (-346.0, 262.0);
        p.fill(x, 40.0, w, 352.0, [0.0, 0.0, 0.0, 0.35]);
        p.border(x, 40.0, w, 352.0, with_alpha(c.rarity.color(), 0.6));
        let (pos, size) = p.r(x + 8.0, 48.0, w - 16.0, 270.0);
        p.ops.push(Op::Gun { pos, size, key: FIGURE_KEY, weapon: format!("char:{}", c.id), camo: 0, picture: String::new() });
        let mid = x + w * 0.5;
        p.text(mid, 344.0, 1, 0.5, c.name, c.rarity.color());
        p.text(mid, 362.0, 1, 0.3, &format!("{}  |  {}", c.rarity.name(), c.game.name()), GREY);
        let state = if c.id == worn.id { "Worn" } else { "Click to wear" };
        p.text(mid, 380.0, 1, 0.28, state, with_alpha(GREY, 0.8));
        p.horz_align = 1;
        p.text(14.0, 418.0, 0, 0.3, &format!("Characters collected: {} of {}", inv.characters_owned().len(), crate::characters::CHARACTERS.len()), GREY);
    }

    fn paint_variants(&self, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
        let Ok(stat) = self.dvar(SLOT_DVAR).parse::<i32>() else { return };
        let weapon = self.table_lookup("mp/statstable.csv", 0, &self.stat(stat).to_string(), 4);
        let inv = supply::inventory();
        let equipped = inv.equipped(stat, &weapon).map_or("none", |v| v.id.as_str());
        let mut p = Painter { fe: self, pl, ops, horz_align: 1, dy: 0.0 };
        // Ticks on the rows, like the attachment popups'.
        for (i, it) in om.menu.items.iter().enumerate() {
            let Some(id) = it.action.split("\"supplyEquip\"").nth(1).and_then(|r| r.split('"').nth(3)) else { continue };
            if !om.shown[i] {
                continue;
            }
            let r = it.window.rect;
            let material = if id == equipped { "hud_checkbox_checked" } else { "hud_checkbox_clear" };
            p.pic(r.x + 4.0, r.y + 3.0, 12.0, 12.0, material, [1.0; 4]);
        }
        // Details of the row under the mouse (or the one equipped).
        let highlighted = self.dvar(HIGHLIGHT_DVAR);
        let id = if highlighted.is_empty() { equipped } else { highlighted.as_str() };
        let (x, mut y) = (236.0, 156.0);
        let gun = self.weapon_name(&weapon);
        match supply::variant(id) {
            Some(v) => {
                p.text(x, y, 0, 0.4583, v.name, v.rarity.color());
                y += 16.0;
                p.text(x, y, 0, 0.3, &format!("{gun}  |  {}  |  {}", v.rarity.name(), v.game().name()), GREY);
                y += 8.0;
                for (text, up) in v.tweak_texts() {
                    y += 16.0;
                    p.text(x, y, 0, 0.33, &text, if up { UP } else { DOWN });
                }
                if v.camo > 0 {
                    y += 20.0;
                    p.text(x, y, 0, 0.28, &format!("Signature camo: {}", self.camo_name(v.camo)), Rarity::Elite.color());
                }
            }
            None => {
                p.text(x, y, 0, 0.4583, "Standard Issue", [1.0, 0.8, 0.4, 1.0]);
                y += 16.0;
                p.text(x, y, 0, 0.3, &format!("The {gun} as issued."), GREY);
                if inv.owned_of(&weapon).is_empty() {
                    y += 28.0;
                    p.text(x, y, 0, 0.3, &format!("No {gun} variants yet."), GREY);
                    y += 14.0;
                    p.text(x, y, 0, 0.3, "Supply drops from the main menu hold them.", GREY);
                }
            }
        }
    }

    /// The variant under each weapon's name on Create a Class's right, in
    /// its rarity's colour.
    fn paint_cac_variants(&self, base: i32, pl: &Placement, ops: &mut Vec<Op>) {
        let inv = supply::inventory();
        let mut p = Painter { fe: self, pl, ops, horz_align: 3, dy: 0.0 };
        for (slot, y) in [(1, 92.0), (3, 180.0)] {
            let weapon = self.table_lookup("mp/statstable.csv", 0, &self.stat(base + slot).to_string(), 4);
            if let Some(v) = inv.equipped(base + slot, &weapon) {
                p.text(-349.0, y, 0, 0.33, v.name, v.rarity.color());
            }
        }
    }
}

impl Painter<'_> {
    /// A card face down: before a drop is opened, or still to turn.
    fn sealed(&mut self, x: f32, now: i64, opening: bool) {
        let pulse = if opening { 0.5 + 0.5 * ((now as f32) * 0.012).sin() } else { 0.0 };
        self.fill(x, CARD_Y, CARD_W, CARD_H, [0.06, 0.06, 0.07, 0.8]);
        self.border(x, CARD_Y, CARD_W, CARD_H, [0.5, 0.5, 0.55, 0.5 + 0.4 * pulse]);
        self.fill(x + 1.0, CARD_Y + 1.0, CARD_W - 2.0, 22.0, [0.9, 0.9, 0.95, 0.08]);
        self.text(x + CARD_W * 0.5, CARD_Y + 17.0, 1, 0.3, "SUPPLY DROP", GREY);
        self.text(x + CARD_W * 0.5, CARD_Y + CARD_H * 0.5 + 20.0, 1, 1.2, "?", [0.8, 0.8, 0.85, 0.35 + 0.4 * pulse]);
    }

    /// A card face up, `age` ms after it turned.
    fn card(&mut self, x: f32, item: DropItem, new: bool, index: usize, age: i64) {
        let rarity = item.rarity();
        let color = rarity.color();
        let y = CARD_Y;
        // Elite cards glow.
        if rarity == Rarity::Elite {
            let glow = 0.25 + 0.15 * ((self.fe.millis() as f32) * 0.004).sin();
            self.fill(x - 3.0, y - 3.0, CARD_W + 6.0, CARD_H + 6.0, with_alpha(color, glow));
        }
        self.fill(x, y, CARD_W, CARD_H, [0.06, 0.06, 0.07, 0.9]);
        self.border(x, y, CARD_W, CARD_H, with_alpha(color, 0.85));
        self.fill(x + 1.0, y + 1.0, CARD_W - 2.0, 22.0, with_alpha(color, 0.25));
        self.text(x + CARD_W * 0.5, y + 17.0, 1, 0.3, rarity.name(), color);
        match item {
            DropItem::Variant(v) => self.variant_card(x, v, index),
            DropItem::Character(c) => self.character_card(x, c, index),
        }
        if new {
            self.pic(x + 4.0, y + 26.0, 36.0, 18.0, "specialty_new", [1.0; 4]);
        }
        // Turning over: a flash that fades.
        if age < FLASH_MS {
            let t = 1.0 - age as f32 / FLASH_MS as f32;
            self.fill(x, y, CARD_W, CARD_H, [1.0, 1.0, 1.0, 0.85 * t]);
        }
    }

    /// A character: standing in 3D, named.
    fn character_card(&mut self, x: f32, c: &Character, index: usize) {
        let (mid, y) = (x + CARD_W * 0.5, CARD_Y);
        let (pos, size) = self.r(x + 6.0, y + 26.0, CARD_W - 12.0, 168.0);
        let weapon = format!("char:{}", c.id);
        let key = CARD_KEY + index as i32;
        // Still loading (another game's zone, say): a shimmer, not an empty card.
        if !self.fe.previews.shown(key, &weapon) {
            let t = self.fe.millis() as f32 * 0.003;
            for k in 0..6 {
                let band = 0.5 + 0.5 * (t - k as f32 * 0.6).sin();
                self.fill(x + 16.0, y + 40.0 + k as f32 * 26.0, CARD_W - 32.0, 18.0, with_alpha(c.rarity.color(), 0.06 + 0.12 * band));
            }
            self.text(mid, y + 120.0, 1, 0.26, "Loading...", with_alpha(GREY, 0.8));
        }
        self.ops.push(Op::Gun { pos, size, key, weapon, camo: 0, picture: String::new() });
        self.fill(x + 12.0, y + 200.0, CARD_W - 24.0, 1.0, with_alpha(c.rarity.color(), 0.5));
        let lines = self.wrapped(mid, y + 222.0, 0.42, CARD_W - 10.0, c.name, c.rarity.color());
        self.text(mid, y + 228.0 + 20.0 * lines as f32, 1, 0.28, c.game.name(), GREY);
    }

    /// A weapon variant: the gun in 3D, its name and tweaks.
    fn variant_card(&mut self, x: f32, v: &Variant, index: usize) {
        let color = v.rarity.color();
        let (mid, y) = (x + CARD_W * 0.5, CARD_Y);
        // The gun in 3D, with the 2D picture until it's ready.
        let (pos, size) = self.r(x + 6.0, y + 40.0, CARD_W - 12.0, 84.0);
        let picture = self.fe.table_lookup("mp/statstable.csv", 4, v.weapon, 6);
        self.ops.push(Op::Gun { pos, size, key: CARD_KEY + index as i32, weapon: format!("{}:", v.weapon), camo: v.camo, picture });

        self.text(mid, y + 140.0, 1, 0.5, v.name, color);
        let gun = self.fe.weapon_name(v.weapon);
        self.text(mid, y + 158.0, 1, 0.3, &gun, GREY);
        self.text(mid, y + 172.0, 1, 0.24, v.game().name(), with_alpha(GREY, 0.8));
        self.fill(x + 12.0, y + 180.0, CARD_W - 24.0, 1.0, with_alpha(color, 0.5));
        let mut ty = y + 184.0;
        for (text, up) in v.tweak_texts() {
            ty += 18.0;
            self.text(mid, ty, 1, 0.33, &text, if up { UP } else { DOWN });
        }
        if v.camo > 0 {
            let camo = self.fe.camo_name(v.camo);
            self.text(mid, y + CARD_H - 14.0, 1, 0.27, &format!("{camo} camo"), color);
        }
    }
}
