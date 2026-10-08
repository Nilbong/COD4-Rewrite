//! The Camo Editor: players' own camos ([`crate::custom_camos`]) in Create a
//! Class. Every camo popup (CoD4's, Black Ops' and World at War's) has a
//! "Custom Camos" row opening the picker: eight slots, each equipped with a
//! click, or made and changed in the editor. Both screens are CoD4's own
//! Rank & Challenges shell, rows and panel (as the Combat Record), with a
//! live 3D preview of the class's gun to drag (or turn with the right
//! stick).
//!
//! The editor's settings are multi rows over `ccamo_*` dvars: a click or
//! the left arrow/right arrow (a pad's left and right, a right click back)
//! steps them; the colour swatches set the colour row last chosen; the name
//! is typed (on a pad, arcade style: [`Frontend::pad_edit`]). Each change
//! rebuilds the draft ([`custom_camos::DRAFT`]) and its preview.

use super::{Frontend, Op, OpenMenu, draw::Placement, expr::Env};
use crate::custom_camos::{self, CustomCamo, FINISHES, PALETTE, PATTERNS, ROTATION_STEP, SCALES, SLOTS};
use iw3::menu::{EditField, Item, ItemData, Menu, Multi, Rect as VRect, Window, flags, item_type};
use std::sync::Arc;

#[path = "combat_record/style.rs"]
mod style;

pub(super) const PICKER: &str = "cod4rw_camo_picker";
pub(super) const EDITOR: &str = "cod4rw_camo_editor";
/// The class weapon stat the picker equips.
const STAT: &str = "ccamo_stat";
/// The slot under the cursor in the picker, and the one being edited.
const SLOT: &str = "ccamo_slot";
/// The colour row the swatches set (1..4).
const ACTIVE: &str = "ccamo_active";
const PATTERN: &str = "ccamo_pattern";
const COUNT: &str = "ccamo_count";
const COLOR: &str = "ccamo_color";
const SCALE: &str = "ccamo_scale";
const ROTATION: &str = "ccamo_rotation";
const FINISH: &str = "ccamo_finish";
const NAME: &str = "ccamo_name";
/// Preview keys (see [`super::preview`]).
const PICKER_PREVIEW: i32 = 2100;
const EDITOR_PREVIEW: i32 = 2101;
/// The popups' row.
pub(super) const ROW_NAME: &str = "camo_custom";
pub(super) const ROW_LABEL: &str = "COD4RW_CAMO_CUSTOM";
pub(super) const ICON: &str = "cod4rw_ccamo_icon";
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 0.9];
const MUTED: [f32; 4] = [0.69, 0.69, 0.69, 1.0];
const GOLD: [f32; 4] = [1.0, 0.85, 0.5, 0.8];
/// Panel layout as the Combat Record's (centre-aligned virtual units).
const BODY_SCALE: f32 = 366.0 / 420.0;

const COLOR_NAMES: [&str; 24] = [
    "Black", "Charcoal", "Gunmetal", "Stone", "Bone", "Olive Drab", "Field Green", "Forest", "Sage", "Earth",
    "Coyote", "Tan", "Sand", "Navy", "Steel Blue", "Sky", "Crimson", "Rust", "Orange", "Gold", "Jade", "Violet",
    "Pink", "Teal",
];

fn rect(x: f32, y: f32, w: f32, h: f32) -> VRect {
    VRect { x: -386.0 + (x + 440.0) * BODY_SCALE, y, w: w * BODY_SCALE, h, horz_align: 3, vert_align: 1 }
}

fn left_rect(y: f32) -> VRect {
    VRect { x: 0.0, y, w: 220.0, h: 22.0, horz_align: 1, vert_align: 1 }
}

/// The swatch material of custom camo `camo` (see [`super::assets`]).
pub(super) fn swatch(camo: usize) -> String {
    format!("cod4rw_ccamo_{camo}")
}

/// Copy the profile's custom camos to where the guns are built from.
pub(super) fn sync(stats: &super::stats::Stats) {
    for n in 0..SLOTS {
        let def = stats.dvars.get(&format!("{}{n}", custom_camos::DVAR_PREFIX)).and_then(|s| CustomCamo::decode(s));
        custom_camos::set(custom_camos::FIRST + n, def);
    }
}

pub(super) fn keeps_dvar(name: &str) -> bool {
    name.strip_prefix(custom_camos::DVAR_PREFIX).is_some_and(|n| n.parse::<usize>().is_ok_and(|n| n < SLOTS))
}

fn nearest(c: [u8; 3]) -> usize {
    let d = |p: &[u8; 3]| (0..3).map(|i| (p[i] as i32 - c[i] as i32).pow(2)).sum::<i32>();
    (0..PALETTE.len()).min_by_key(|&i| d(&PALETTE[i])).unwrap_or(0)
}

fn multi(labels: Vec<String>) -> ItemData {
    ItemData::Multi(Multi {
        strings: (0..labels.len()).map(|i| i.to_string()).collect(),
        values: (0..labels.len()).map(|i| i as f32).collect(),
        labels,
        str_def: true,
    })
}

/// Does a row step through its choices both ways (left and right)?
pub(super) fn is_row(item: &Item) -> bool {
    item.ty == item_type::MULTI && item.dvar.starts_with("ccamo_")
}

impl Frontend {
    fn slot(&self) -> usize {
        self.dvar(SLOT).parse::<usize>().unwrap_or(0).min(SLOTS - 1)
    }

    fn class_stat(&self) -> i32 {
        self.dvar(STAT).parse().unwrap_or(201)
    }

    /// The draft as its rows set it.
    fn draft(&self) -> CustomCamo {
        let n = |d: &str| self.dvar(d).parse::<usize>().unwrap_or(0);
        let mut colors = [[0; 3]; 4];
        for (i, c) in colors.iter_mut().enumerate() {
            *c = PALETTE[n(&format!("{COLOR}{}", i + 1)).min(PALETTE.len() - 1)];
        }
        let name = custom_camos::clean_name(&self.dvar(NAME));
        CustomCamo {
            name: if name.is_empty() { format!("Custom Camo {}", self.slot() + 1) } else { name },
            pattern: n(PATTERN).min(PATTERNS.len() - 1),
            colors,
            count: n(COUNT) + 2,
            scale: n(SCALE).min(SCALES.len() - 1),
            rotation: n(ROTATION) as u16 * ROTATION_STEP,
            finish: n(FINISH).min(FINISHES.len() - 1),
        }
    }

    fn set_rows(&mut self, def: &CustomCamo) {
        self.set_dvar(PATTERN, &def.pattern.to_string());
        self.set_dvar(COUNT, &(def.count.clamp(2, 4) - 2).to_string());
        for (i, c) in def.colors.iter().enumerate() {
            self.set_dvar(&format!("{COLOR}{}", i + 1), &nearest(*c).to_string());
        }
        self.set_dvar(SCALE, &def.scale.to_string());
        self.set_dvar(ROTATION, &(def.rotation / ROTATION_STEP).to_string());
        self.set_dvar(FINISH, &def.finish.to_string());
        self.set_dvar(NAME, &def.name);
        self.set_dvar(ACTIVE, "1");
    }

    pub(super) fn custom_camo_menu(&self, key: &str) -> Option<Arc<Menu>> {
        match key {
            PICKER => Some(Arc::new(self.picker_menu())),
            EDITOR => Some(Arc::new(self.editor_menu())),
            _ => None,
        }
    }

    fn screen(&self, name: &str, title: &str, back: &str) -> Menu {
        let mut m = style::screen(self, name, title);
        let back = format!("\"play\" \"mouse_click\" ; \"uiScript\" {back} ;");
        m.on_esc = back.clone();
        for it in m.items.iter_mut().filter(|it| it.window.name.eq_ignore_ascii_case("back")) {
            it.action = back.clone();
        }
        m
    }

    fn picker_menu(&self) -> Menu {
        let mut m = self.screen(PICKER, "CUSTOM CAMOS", "ccamoBack");
        let equipped = self.class_camo(self.class_stat()) as usize;
        for n in 0..SLOTS {
            let camo = custom_camos::FIRST + n;
            let label = match custom_camos::get(camo) {
                Some(def) => format!("{}. {}", n + 1, def.name),
                None => format!("{}. Empty Slot", n + 1),
            };
            let mut row = style::row(self, left_rect(34.0 + n as f32 * 24.0), n as i32 + 1, &label, &format!("ccamoSlot {n}"), equipped == camo);
            for it in row.iter_mut().filter(|it| it.ty == item_type::BUTTON) {
                it.on_focus += &format!(" \"execnow\" \"set {SLOT} {n}\" ;");
            }
            m.items.extend(row);
        }
        let defined = custom_camos::get(custom_camos::FIRST + self.slot()).is_some();
        let mut actions = vec![("Create New", "ccamoNew")];
        if defined {
            actions = vec![("Edit Camo", "ccamoEdit"), ("Delete Camo", "ccamoDelete")];
        }
        if custom_camos::is_custom(equipped) {
            actions.push(("Remove Camo", "ccamoUnequip"));
        }
        for (i, (label, command)) in actions.into_iter().enumerate() {
            m.items.extend(style::row(self, left_rect(238.0 + i as f32 * 24.0), 20 + i as i32, label, command, false));
        }
        m.items.extend(style::panel(self, rect(-440.0, 34.0, 420.0, 382.0)));
        m
    }

    fn editor_menu(&self) -> Menu {
        let mut m = self.screen(EDITOR, "CAMO EDITOR", "ccamoCancel");
        let count = self.dvar(COUNT).parse::<usize>().unwrap_or(1) + 2;
        let solid = self.dvar(PATTERN).parse::<usize>().ok() == Some(PATTERNS.len() - 1);
        let mut rows: Vec<(String, &str, Vec<String>)> = vec![
            ("Pattern:".into(), PATTERN, PATTERNS.iter().map(|p| p.name.to_owned()).collect()),
        ];
        if !solid {
            rows.push(("Colours:".into(), COUNT, vec!["2".into(), "3".into(), "4".into()]));
        }
        let colors = ["ccamo_color1", "ccamo_color2", "ccamo_color3", "ccamo_color4"];
        for (i, dvar) in colors.iter().enumerate().take(if solid { 1 } else { count }) {
            rows.push((format!("Colour {}:", i + 1), dvar, COLOR_NAMES.iter().map(|s| (*s).to_owned()).collect()));
        }
        rows.push(("Scale:".into(), SCALE, SCALES.iter().map(|s| format!("{:.0}%", s * 100.0)).collect()));
        let turns = 360 / ROTATION_STEP;
        rows.push(("Rotation:".into(), ROTATION, (0..turns).map(|i| format!("{} deg", i * ROTATION_STEP)).collect()));
        rows.push(("Finish:".into(), FINISH, FINISHES.iter().map(|s| (*s).to_owned()).collect()));
        let mut y = 34.0;
        for (i, (label, dvar, labels)) in rows.into_iter().enumerate() {
            let mut row = style::row(self, left_rect(y), i as i32 + 1, &label, "ccamoDraft", false);
            for it in row.iter_mut().filter(|it| it.ty == item_type::BUTTON) {
                it.ty = item_type::MULTI;
                it.dvar = dvar.into();
                it.data = multi(labels.clone());
                if let Some(n) = dvar.strip_prefix(COLOR) {
                    it.on_focus += &format!(" \"execnow\" \"set {ACTIVE} {n}\" ;");
                }
            }
            m.items.extend(row);
            y += 24.0;
        }
        let mut name = style::row(self, left_rect(y), 15, "Name:", "ccamoDraft", false);
        for it in name.iter_mut().filter(|it| it.ty == item_type::BUTTON) {
            it.ty = item_type::EDITFIELD;
            it.dvar = NAME.into();
            it.action.clear();
            it.on_accept = "\"uiScript\" \"ccamoDraft\" ;".into();
            let max = custom_camos::NAME_LEN as i32;
            it.data = ItemData::EditField(EditField { max_chars: max, max_paint_chars: max, ..EditField::default() });
        }
        m.items.extend(name);
        y += 36.0;
        for (i, (label, command)) in [("Save Camo", "ccamoSave"), ("Cancel", "ccamoCancel")].into_iter().enumerate() {
            m.items.extend(style::row(self, left_rect(y + i as f32 * 24.0), 16 + i as i32, label, command, false));
        }
        m.items.extend(style::panel(self, rect(-440.0, 34.0, 420.0, 382.0)));
        // The palette: click a swatch to set the colour row last chosen.
        for (i, _) in PALETTE.iter().enumerate() {
            let (x, y) = swatch_at(i);
            m.items.push(Item {
                window: Window { rect: rect(x, y, 26.0, 26.0 * BODY_SCALE), dynamic_flags: flags::VISIBLE, ..Window::default() },
                ty: item_type::BUTTON,
                action: format!("\"play\" \"mouse_click\" ; \"uiScript\" ccamoSwatch {i} ;"),
                on_focus: "\"play\" \"mouse_over\"".into(),
                ..Item::default()
            });
        }
        m
    }

    /// The draft changed: rebuild it and its preview (and the rows, when
    /// the colours shown change).
    fn draft_changed(&mut self) {
        let def = self.draft();
        custom_camos::set(custom_camos::DRAFT, Some(def));
        self.previews.refresh(EDITOR_PREVIEW);
        let colour_rows = self
            .stack
            .iter()
            .find(|om| om.name == EDITOR)
            .map(|om| om.menu.items.iter().filter(|it| it.dvar.starts_with(COLOR) && it.ty == item_type::MULTI).count());
        let solid = self.dvar(PATTERN).parse::<usize>().ok() == Some(PATTERNS.len() - 1);
        let wanted = if solid { 1 } else { self.dvar(COUNT).parse::<usize>().unwrap_or(1) + 2 };
        if colour_rows.is_some_and(|n| n != wanted) {
            let focus = self.focus.clone();
            self.close(EDITOR);
            self.open(EDITOR);
            if let Some((m, i)) = focus.filter(|(m, _)| m == EDITOR) {
                let len = self.menu_item(EDITOR).map_or(0, |mm| mm.items.len());
                self.set_focus(Some((m, i.min(len.saturating_sub(1)))));
            }
        }
    }

    fn reopen_picker(&mut self) {
        let focus = self.focus.clone();
        self.close(PICKER);
        self.open(PICKER);
        if let Some(f) = focus.filter(|(m, _)| m == PICKER) {
            self.set_focus(Some(f));
        }
    }

    fn edit_slot(&mut self, n: usize, fresh: bool) {
        self.set_dvar(SLOT, &n.to_string());
        let def = custom_camos::get(custom_camos::FIRST + n).filter(|_| !fresh).map_or_else(
            || CustomCamo { name: format!("Custom Camo {}", n + 1), ..CustomCamo::default() },
            |d| (*d).clone(),
        );
        self.set_rows(&def);
        custom_camos::set(custom_camos::DRAFT, Some(def));
        self.previews.refresh(EDITOR_PREVIEW);
        self.open(EDITOR);
    }

    /// A row stepped one way (`dir` ±1). Whether it was the editor's.
    pub(super) fn camo_row_step(&mut self, item: &Item, dir: i32) -> bool {
        let ItemData::Multi(m) = &item.data else { return false };
        if !is_row(item) || m.labels.is_empty() {
            return false;
        }
        let n = m.labels.len() as i32;
        let at = super::multi_index(m, &self.dvar(&item.dvar)) as i32;
        let next = m.strings[(at + dir).rem_euclid(n) as usize].clone();
        self.set_dvar(&item.dvar, &next);
        if let Some(c) = item.dvar.strip_prefix(COLOR) {
            self.set_dvar(ACTIVE, c);
        }
        // (The reticle screen's rows step the same way: `reticle_menu`.)
        if !super::reticle_menu::is_row(&item.dvar) {
            self.draft_changed();
        }
        true
    }

    /// `uiScript ccamo...`.
    pub(super) fn custom_camo_script(&mut self, args: &[String]) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        match a(0).to_ascii_lowercase().as_str() {
            // From a camo popup, for class weapon stat `a(1)`.
            "ccamoopen" => {
                if let Ok(stat) = a(1).parse::<i32>() {
                    self.set_dvar(STAT, &stat.to_string());
                    let equipped = self.class_camo(stat) as usize;
                    let slot = if custom_camos::is_custom(equipped) { equipped - custom_camos::FIRST } else { 0 };
                    self.set_dvar(SLOT, &slot.min(SLOTS - 1).to_string());
                    self.open(PICKER);
                }
            }
            "ccamoback" => self.close(PICKER),
            "ccamoslot" => {
                let Ok(n) = a(1).parse::<usize>() else { return };
                if n >= SLOTS {
                    return;
                }
                self.set_dvar(SLOT, &n.to_string());
                if custom_camos::get(custom_camos::FIRST + n).is_some() {
                    // Equip it, and back to the class.
                    self.equip_custom_camo(self.class_stat(), (custom_camos::FIRST + n) as i32);
                    self.stats.save_if_changed();
                    self.close(PICKER);
                    let popups: Vec<String> =
                        self.stack.iter().filter(|m| m.name.contains("popup_cac_camo")).map(|m| m.name.clone()).collect();
                    for p in popups {
                        self.close(&p);
                    }
                } else {
                    self.edit_slot(n, true);
                }
            }
            "ccamonew" => self.edit_slot(self.slot(), true),
            "ccamoedit" => self.edit_slot(self.slot(), false),
            "ccamodelete" => {
                let n = self.slot();
                let camo = (custom_camos::FIRST + n) as i32;
                self.stats.dvars.remove(&format!("{}{n}", custom_camos::DVAR_PREFIX));
                self.set_dvar(&format!("{}{n}", custom_camos::DVAR_PREFIX), "");
                custom_camos::set(camo as usize, None);
                if self.class_camo(self.class_stat()) == camo {
                    self.equip_custom_camo(self.class_stat(), 0);
                }
                self.stats.save_if_changed();
                self.reopen_picker();
            }
            "ccamounequip" => {
                self.equip_custom_camo(self.class_stat(), 0);
                self.stats.save_if_changed();
                self.reopen_picker();
            }
            "ccamodraft" => self.draft_changed(),
            "ccamoswatch" => {
                if let Ok(i) = a(1).parse::<usize>()
                    && i < PALETTE.len()
                {
                    let row = self.dvar(ACTIVE).parse::<usize>().unwrap_or(1).clamp(1, 4);
                    self.set_dvar(&format!("{COLOR}{row}"), &i.to_string());
                    self.draft_changed();
                }
            }
            "ccamosave" => {
                let n = self.slot();
                let def = self.draft();
                self.set_dvar(&format!("{}{n}", custom_camos::DVAR_PREFIX), &def.encode());
                custom_camos::set(custom_camos::FIRST + n, Some(def));
                self.stats.save_if_changed();
                self.editing = None;
                self.close(EDITOR);
                self.reopen_picker();
            }
            "ccamocancel" => {
                self.editing = None;
                self.close(EDITOR);
            }
            _ => {}
        }
    }

    pub(super) fn paint_custom_camo(&self, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
        if om.name != PICKER && om.name != EDITOR {
            return;
        }
        let mut p = Painter { fe: self, pl, ops };
        let gun = self.gun_for(self.class_stat()).map_or_else(|| "ak47:".to_owned(), |(g, _)| g);
        if om.name == PICKER {
            let camo = custom_camos::FIRST + self.slot();
            let def = custom_camos::get(camo);
            p.heading(&def.as_ref().map_or_else(|| format!("SLOT {} - EMPTY", self.slot() + 1), |d| d.name.to_uppercase()));
            p.gun(PICKER_PREVIEW, &gun, if def.is_some() { camo } else { 0 });
            match &def {
                Some(d) => {
                    p.swatch(&swatch(camo), -422.0, 300.0, 52.0);
                    p.text(-360.0, 316.0, 18.0, &format!("{} - {}", PATTERNS[d.pattern].name, FINISHES[d.finish]), GOLD);
                    p.text(-360.0, 340.0, 14.4, &format!("{} colours, scale {:.0}%, {} deg", d.used().len(), SCALES[d.scale] * 100.0, d.rotation), WHITE);
                    let equipped = self.class_camo(self.class_stat()) as usize == camo;
                    p.text(-422.0, 380.0, 14.4, if equipped { "Equipped on this weapon." } else { "Select the slot to equip it." }, MUTED);
                }
                None => {
                    p.text(-422.0, 316.0, 18.0, "Empty slot", GOLD);
                    p.text(-422.0, 340.0, 14.4, "Select it to design a new camo.", WHITE);
                }
            }
            p.text(-422.0, 400.0, 14.4, "Custom camos work on every weapon.", MUTED);
        } else {
            p.heading("DESIGN YOUR CAMO");
            p.gun(EDITOR_PREVIEW, &gun, custom_camos::DRAFT);
            let draft = self.draft();
            p.swatch(&swatch(custom_camos::DRAFT), -422.0, 286.0, 40.0);
            p.fit(-370.0, 302.0, 18.0, 330.0, &draft.name, GOLD);
            let active = self.dvar(ACTIVE).parse::<usize>().unwrap_or(1).clamp(1, 4);
            p.text(-370.0, 320.0, 14.4, &format!("Swatches set Colour {active}"), MUTED);
            for (i, c) in PALETTE.iter().enumerate() {
                let (x, y) = swatch_at(i);
                p.fill(x, y, 26.0, 26.0 * BODY_SCALE, [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]);
                if draft.colors.get(active - 1).is_some_and(|d| *d == *c) {
                    p.border(x - 1.0, y - 1.0, 28.0, 26.0 * BODY_SCALE + 2.0, WHITE);
                }
            }
            // Each colour row's colour, beside it.
            for it in om.menu.items.iter().filter(|it| it.ty == item_type::MULTI && it.dvar.starts_with(COLOR)) {
                let i = self.dvar(&it.dvar).parse::<usize>().unwrap_or(0).min(PALETTE.len() - 1);
                let c = PALETTE[i];
                let r = VRect { x: 14.0, y: it.window.rect.y + 5.0, w: 12.0, h: 12.0, horz_align: 1, vert_align: 1 };
                let (pos, size) = pl.rect(&r);
                p.ops.push(Op::Fill { pos, size, color: [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0] });
            }
            // The swatch under the cursor or pad focus.
            if let Some((name, index)) = &self.focus
                && *name == om.name
                && let Some(it) = om.menu.items.get(*index)
                && it.action.contains("ccamoSwatch")
            {
                let (pos, size) = pl.rect(&it.window.rect);
                let line = pl.scale * 2.0;
                for (pos, size) in [
                    (pos, bevy::math::Vec2::new(size.x, line)),
                    (pos + bevy::math::Vec2::new(0.0, size.y - line), bevy::math::Vec2::new(size.x, line)),
                    (pos, bevy::math::Vec2::new(line, size.y)),
                    (pos + bevy::math::Vec2::new(size.x - line, 0.0), bevy::math::Vec2::new(line, size.y)),
                ] {
                    p.ops.push(Op::Fill { pos, size, color: GOLD });
                }
            }
        }
    }
}

/// Where palette swatch `i` goes: two rows of twelve under the preview.
fn swatch_at(i: usize) -> (f32, f32) {
    (-422.0 + (i % 12) as f32 * 32.0, 338.0 + (i / 12) as f32 * 30.0 * BODY_SCALE)
}

struct Painter<'a> {
    fe: &'a Frontend,
    pl: &'a Placement,
    ops: &'a mut Vec<Op>,
}

impl Painter<'_> {
    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let (pos, size) = self.pl.rect(&rect(x, y, w, h));
        self.ops.push(Op::Fill { pos, size, color });
    }
    fn border(&mut self, x: f32, y: f32, w: f32, h: f32, c: [f32; 4]) {
        self.fill(x, y, w, 1.0, c);
        self.fill(x, y + h - 1.0, w, 1.0, c);
        self.fill(x, y, 1.0, h, c);
        self.fill(x + w - 1.0, y, 1.0, h, c);
    }
    fn text(&mut self, x: f32, y: f32, height: f32, text: &str, color: [f32; 4]) {
        let (pos, _) = self.pl.rect(&rect(x, y, 0.0, 0.0));
        let font = self.fe.font_for(1, height * self.pl.scale);
        let k = height * self.pl.scale / self.fe.assets.fonts[font].pixel_height as f32;
        self.ops.push(Op::Text { text: text.to_owned(), x: pos.x, y: pos.y, font, k, color, shadow: self.pl.scale });
    }
    fn fit(&mut self, x: f32, y: f32, height: f32, width: f32, text: &str, color: [f32; 4]) {
        let font = self.fe.font_for(1, height * self.pl.scale);
        let k = height * self.pl.scale / self.fe.assets.fonts[font].pixel_height as f32;
        let fits = |s: &str| super::draw::text_width(&self.fe.assets.fonts[font], s, k) <= width * BODY_SCALE * self.pl.scale;
        let mut text = text.to_owned();
        if !fits(&text) {
            while !text.is_empty() && !fits(&format!("{text}...")) {
                text.pop();
            }
            text += "...";
        }
        self.text(x, y, height, &text, color);
    }
    fn heading(&mut self, text: &str) {
        let height = 18.0 * self.pl.scale;
        let font = self.fe.font_for(1, height);
        let k = height / self.fe.assets.fonts[font].pixel_height as f32;
        let width = super::draw::text_width(&self.fe.assets.fonts[font], text, k) / self.pl.scale;
        self.text(-230.0 - width / BODY_SCALE * 0.5, 47.0, 18.0, text, WHITE);
    }
    fn swatch(&mut self, material: &str, x: f32, y: f32, size: f32) {
        let (pos, sz) = self.pl.rect(&rect(x, y, size, size * BODY_SCALE));
        self.ops.push(Op::Pic { pos, size: sz, material: material.into(), color: [1.0; 4] });
        self.border(x, y, size, size * BODY_SCALE, MUTED);
    }
    /// The class's gun, turnable, filling the top of the panel.
    fn gun(&mut self, key: i32, weapon: &str, camo: usize) {
        let (pos, size) = self.pl.rect(&rect(-430.0, 58.0, 400.0, 220.0));
        self.ops.push(Op::Gun { pos, size, key, weapon: weapon.into(), camo, picture: String::new() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{assets::UiAssets, stats::Stats};

    #[test]
    #[ignore = "requires a local CoD4 asset installation"]
    fn design_save_equip_and_reach_the_match_loadout() {
        let mut fe = Frontend::bare(UiAssets::load().expect("installed native UI assets"));
        fe.stats = Stats::in_memory();
        fe.stats.set(crate::ui::progression::stat::RANK, 54);
        fe.stats.set_dvar(crate::ui::progression::UNLOCKS_DVAR, "all");
        for weapon in crate::ui::mastery::weapons(&fe.assets) {
            fe.stats.set(weapon.unlock_stat, 1 | 2 | 4 | 8 | 16 | 32);
        }
        crate::ui::progression::refresh_unlocks(&mut fe.stats, &fe.assets);
        fe.stats.set(201, 20);
        fe.open("menu_cac_assault");
        fe.open("assault_popup_cac_camo");
        // The popup's row opens the picker for the primary.
        let popup = fe.stack.last().unwrap();
        let row = popup.menu.items.iter().position(|it| it.action.contains("ccamoOpen")).expect("Custom Camos row");
        assert!(fe.item_visible(popup, row));
        let action = popup.menu.items[row].action.clone();
        fe.run(&action, "assault_popup_cac_camo");
        assert_eq!(fe.stack.last().unwrap().name, PICKER);
        // An empty slot opens the editor; rows step both ways.
        fe.custom_camo_script(&["ccamoSlot".into(), "2".into()]);
        assert_eq!(fe.stack.last().unwrap().name, EDITOR);
        let pattern = fe.menu_item(EDITOR).unwrap().items.iter().find(|it| it.dvar == PATTERN).cloned().unwrap();
        assert!(fe.camo_row_step(&pattern, -1));
        assert_eq!(fe.draft().pattern, custom_camos::PATTERNS.len() - 1 - 3);
        fe.custom_camo_script(&["ccamoSwatch".into(), "19".into()]);
        assert_eq!(custom_camos::get(custom_camos::DRAFT).unwrap().colors[0], PALETTE[19]);
        fe.set_dvar(NAME, "Gold, Rush!");
        fe.custom_camo_script(&["ccamoSave".into()]);
        let saved = CustomCamo::decode(fe.stats.dvars.get("cod4rw_ccamo_2").unwrap()).unwrap();
        assert_eq!(saved.name, "Gold Rush");
        assert_eq!(custom_camos::get(custom_camos::FIRST + 2).as_deref(), Some(&saved));
        // Picking it equips it on the class's primary and closes the popups.
        fe.custom_camo_script(&["ccamoSlot".into(), "2".into()]);
        assert_eq!(fe.class_camo(201), (custom_camos::FIRST + 2) as i32);
        assert!(!fe.stack.iter().any(|m| m.name == PICKER || m.name.contains("popup_cac_camo")));
        assert_eq!(fe.class_loadout("custom1").unwrap().guns[0].camo, custom_camos::FIRST + 2);
        // Deleting it takes it off the gun.
        fe.custom_camo_script(&["ccamoOpen".into(), "201".into()]);
        fe.custom_camo_script(&["ccamoDelete".into()]);
        assert_eq!(fe.class_camo(201), 0);
        assert!(custom_camos::get(custom_camos::FIRST + 2).is_none());
        // The editor and picker draw.
        let mut ops = Vec::new();
        fe.paint(&Placement::new(1280.0, 720.0), &mut ops);
        assert!(ops.iter().any(|op| matches!(op, Op::Gun { key: PICKER_PREVIEW, .. })));
    }
}
