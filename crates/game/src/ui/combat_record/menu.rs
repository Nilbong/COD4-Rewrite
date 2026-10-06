//! Native IW3-menu controls and drawing for the Combat Record.

use super::*;
use crate::ui::{
    Op, OpenMenu,
    draw::{self, Placement},
    expr::Env,
    progression::{self, Challenge, Level},
};
use iw3::menu::{EditField, Item, ItemData, Menu, Rect as VRect, Token, Window, flags, item_type};
use std::sync::Arc;

#[path = "style.rs"]
mod style;

const RECORD: &str = "combat_record";
const EDITOR: &str = "combat_emblem";
// The original Rank & Challenges label/value colours.
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 0.9];
const MUTED: [f32; 4] = [0.69, 0.69, 0.69, 1.0];
const GOLD: [f32; 4] = [1.0, 0.85, 0.5, 0.8];
const TABS: [&str; 5] = ["Overview", "Weapons", "Challenges", "Identity", "Calling Cards"];
const WEAPON_PAGE: usize = 6;
const CHALLENGE_PAGE: usize = 5;
const FILTERS: [&str; 4] = ["All", "Attachments", "Camos", "Career"];
// Keep the wider record pane clear of the original 220-wide navigation
// column even on CoD4's 640x480 placement. Font sizes remain native.
const BODY_SCALE: f32 = 366.0 / 420.0;

struct CallingCard {
    id: &'static str,
    name: &'static str,
    goal: &'static str,
    stat: i32,
    target: i32,
    material: &'static str,
}

const CARDS: [CallingCard; 6] = [
    CallingCard {
        id: "standard",
        name: "Standard Issue",
        goal: "Ready for duty",
        stat: stat::KILLS,
        target: 0,
        material: "ui_camoskin_brock",
    },
    CallingCard {
        id: "first_blood",
        name: "First Blood",
        goal: "25 career kills",
        stat: stat::KILLS,
        target: 25,
        material: "ui_camoskin_bwmrpt",
    },
    CallingCard {
        id: "sharpshooter",
        name: "Sharpshooter",
        goal: "50 headshots",
        stat: stat::HEADSHOTS,
        target: 50,
        material: "ui_camoskin_bshdwl",
    },
    CallingCard {
        id: "veteran",
        name: "Veteran",
        goal: "1 hour of match time",
        stat: stat::TIME_PLAYED_TOTAL,
        target: 3600,
        material: "ui_camoskin_cmdtgr",
    },
    CallingCard {
        id: "victor",
        name: "Victor",
        goal: "10 match wins",
        stat: stat::WINS,
        target: 10,
        material: "ui_camoskin_stagger",
    },
    CallingCard {
        id: "commander",
        name: "Field Commander",
        goal: "Reach level 25",
        stat: stat::RANK,
        target: 24,
        material: "ui_camoskin_gold",
    },
];

fn selected_card(stats: &Stats) -> usize {
    CARDS
        .iter()
        .position(|c| stats.dvars.get(CARD).is_some_and(|id| id == c.id) && stats.get(c.stat) >= c.target)
        .unwrap_or(0)
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> VRect {
    VRect { x: -386.0 + (x + 440.0) * BODY_SCALE, y, w: w * BODY_SCALE, h, horz_align: 3, vert_align: 1 }
}

fn action(command: &str) -> String {
    format!("uiScript {command} ;")
}

fn button(x: f32, y: f32, w: f32, h: f32, label: &str, command: &str) -> Item {
    Item {
        window: Window {
            rect: rect(x, y, w, h),
            fore_color: WHITE,
            dynamic_flags: flags::VISIBLE,
            ..Window::default()
        },
        ty: item_type::BUTTON,
        font_enum: 1,
        text: label.into(),
        text_scale: 0.375,
        text_style: 3,
        text_align_mode: 8,
        text_align_x: 8.0,
        text_align_y: -2.0,
        action: action(command),
        ..Item::default()
    }
}

fn field(x: f32, y: f32, w: f32, name: &str, max: i32) -> Item {
    let mut it = button(x, y, w, 30.0, "", "combatApplyIdentity");
    it.ty = item_type::EDITFIELD;
    it.window.name = name.into();
    it.dvar = name.into();
    it.action.clear();
    it.data = ItemData::EditField(EditField { max_chars: max, max_paint_chars: max, ..EditField::default() });
    it.on_accept = action("combatApplyIdentity");
    it.window.style = 1;
    it.window.back_color = [0.1, 0.1, 0.1, 0.35];
    it.window.border = 1;
    it.window.border_size = 0.5;
    it.window.border_color = [0.9, 0.9, 0.95, 0.3];
    it
}

fn left_rect(y: f32) -> VRect {
    VRect { x: 0.0, y, w: 220.0, h: 22.0, horz_align: 1, vert_align: 1 }
}

pub(super) fn weapon_keys(fe: &Frontend) -> Vec<String> {
    let Some(t) = fe.assets.table("mp/statstable.csv") else { return Vec::new() };
    let mut keys: Vec<_> = (0..t.rows)
        .filter(|&r| t.get(r, 2).is_some_and(|s| s.starts_with("weapon_")))
        .filter_map(|r| t.get(r, 4))
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
        .map(str::to_owned)
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

fn gun_name(fe: &Frontend, key: &str) -> String {
    let loc = fe.table_lookup("mp/statstable.csv", 4, key, 3);
    let name = if loc.is_empty() { key.to_ascii_uppercase() } else { fe.localize(&format!("@{loc}")) };
    let game = if key.starts_with("t5_") {
        "BO1"
    } else if key.starts_with("t4_") {
        "WaW"
    } else {
        "CoD4"
    };
    format!("{name} ({game})")
}

fn sorted_weapons(fe: &Frontend) -> Vec<String> {
    let mut guns = weapon_keys(fe);
    guns.sort_by(|a, b| {
        metric(&fe.stats, b, "kills")
            .cmp(&metric(&fe.stats, a, "kills"))
            .then_with(|| metric(&fe.stats, b, "seconds").cmp(&metric(&fe.stats, a, "seconds")))
            .then_with(|| gun_name(fe, a).cmp(&gun_name(fe, b)))
    });
    guns
}

fn weapon_of(c: &Challenge) -> Option<&str> {
    c.levels
        .iter()
        .find_map(|l| l.unlock.as_ref().map(|(w, _)| w.as_str()))
        .or_else(|| c.name.strip_prefix("ch_marksman_"))
        .or_else(|| c.name.strip_prefix("ch_expert_"))
}

fn is_camo(l: &Level) -> bool {
    l.unlock.as_ref().is_some_and(|(_, item)| item.starts_with("camo_"))
}

enum RecordChallenge {
    Native(Challenge, usize),
    Mastery(String, usize),
}

fn challenge_levels(fe: &Frontend) -> Vec<RecordChallenge> {
    let filter = fe.dvar("cr_filter").parse::<usize>().unwrap_or(0);
    let weapon = fe.dvar("cr_weapon");
    let mut levels = Vec::new();
    if matches!(filter, 0 | 2) {
        for gun in crate::ui::mastery::weapons(&fe.assets).into_iter().filter(|gun| {
            crate::ui::mastery::canonical(&gun.key) == gun.key
                && (weapon.is_empty() || crate::ui::mastery::canonical(&weapon) == gun.key)
        }) {
            for &camo in crate::ui::mastery::CAMOS {
                levels.push(RecordChallenge::Mastery(gun.key.clone(), camo));
            }
        }
    }
    levels.extend(progression::challenges(&fe.assets).into_iter().flat_map(|c| {
        let matches_weapon = weapon.is_empty() || weapon_of(&c) == Some(weapon.as_str());
        let stages: Vec<_> = (0..c.levels.len())
            .filter(|&i| {
                matches_weapon
                    && match filter {
                        1 => c.levels[i].unlock.is_some() && !is_camo(&c.levels[i]),
                        2 => is_camo(&c.levels[i]),
                        3 => weapon_of(&c).is_none(),
                        _ => true,
                    }
            })
            .collect();
        stages.into_iter().map(|i| RecordChallenge::Native(c.clone(), i)).collect::<Vec<_>>()
    }));
    levels
}

fn challenge_progress(state: i32, stage: usize, saved: i32, target: i32) -> (i32, bool) {
    let done = state == progression::CHALLENGE_DONE || state > stage as i32 + 1;
    (if done { target.max(0) } else { saved.clamp(0, target.max(0)) }, done)
}

struct Rank {
    name: String,
    level: i32,
    icon: String,
    xp: i32,
    min: i32,
    next: Option<i32>,
}

fn rank(fe: &Frontend) -> Rank {
    let xp = fe.stats.get(stat::RANKXP).max(0);
    let mut rank = Rank { name: "Private".into(), level: 1, icon: String::new(), xp, min: 0, next: None };
    if let Some(t) = fe.assets.table("mp/ranktable.csv") {
        let num = |r: usize, c: usize| t.get(r, c).and_then(|v| v.trim().parse::<i32>().ok());
        let mut ranks: Vec<_> = (0..t.rows).filter_map(|r| Some((num(r, 0)?, num(r, 2)?, r))).collect();
        ranks.sort_by_key(|&(id, _, _)| id);
        if let Some(&(id, min, row)) = ranks.iter().rev().find(|&&(_, min, _)| xp >= min) {
            rank.name = fe.localize(&format!("@{}", t.get(row, 5).unwrap_or("")));
            rank.level = num(row, 14).unwrap_or(id + 1);
            rank.min = min;
            rank.icon = t.get(row, 6).unwrap_or("").to_owned();
            rank.next = ranks.iter().find(|&&(next, _, _)| next > id).map(|&(_, min, _)| min);
        }
    }
    rank
}

impl Frontend {
    pub(in crate::ui) fn combat_record_menu(&mut self, key: &str, menu: Option<Arc<Menu>>) -> Option<Arc<Menu>> {
        match key {
            RECORD => Some(Arc::new(self.record_menu())),
            EDITOR => Some(Arc::new(self.emblem_menu())),
            _ => menu.map(|menu| {
                let mut changed = false;
                let mut replacement = (*menu).clone();
                for it in &mut replacement.items {
                    let old_label = it.text_exp.iter().any(
                        |t| matches!(t, Token::Str(s) if self.localize(s).eq_ignore_ascii_case("Rank & Challenges")),
                    );
                    let rank_button = it.ty == item_type::BUTTON
                        && (old_label || self.localize(&it.text).eq_ignore_ascii_case("Rank & Challenges"));
                    if rank_button {
                        it.text_exp.clear();
                        it.text = "Combat Record".into();
                        it.action = action("combatOpen");
                        changed = true;
                    }
                    // Original menus often have separate copies of the row's
                    // label for its highlighted and normal states.
                    if old_label {
                        it.text_exp = vec![Token::Str("Combat Record".into())];
                        changed = true;
                    }
                }
                if changed { Arc::new(replacement) } else { menu }
            }),
        }
    }

    fn record_menu(&self) -> Menu {
        let mut m = style::screen(self, RECORD, "COMBAT RECORD");
        let tab = self.dvar("cr_tab").parse::<usize>().unwrap_or(0).min(TABS.len() - 1);
        for (i, name) in TABS.iter().enumerate() {
            m.items.extend(style::row(
                self,
                left_rect(34.0 + i as f32 * 24.0),
                i as i32 + 1,
                name,
                &format!("combatTab {i}"),
                i == tab,
            ));
        }
        m.items.extend(style::panel(self, rect(-440.0, 34.0, 420.0, 382.0)));
        match tab {
            1 => {
                let guns = sorted_weapons(self);
                let page = self.record_page(guns.len(), WEAPON_PAGE);
                for (row, key) in guns.iter().skip(page * WEAPON_PAGE).take(WEAPON_PAGE).enumerate() {
                    let mut items = style::row(
                        self,
                        rect(-430.0, 84.0 + row as f32 * 46.0, 400.0, 42.0),
                        20 + row as i32,
                        &gun_name(self, key),
                        &format!("combatWeapon {key}"),
                        false,
                    );
                    for it in items.iter_mut().filter(|it| it.ty == item_type::BUTTON) {
                        it.text_align_mode = 4;
                        it.text_align_x = 8.0;
                        it.text_align_y = 1.0;
                        it.text_scale = 0.375;
                    }
                    m.items.extend(items);
                }
                self.page_buttons(&mut m, guns.len(), WEAPON_PAGE);
            }
            2 => {
                let filter = self.dvar("cr_filter").parse::<usize>().unwrap_or(0);
                for (i, name) in FILTERS.iter().enumerate() {
                    m.items.extend(style::row(
                        self,
                        left_rect(178.0 + i as f32 * 24.0),
                        10 + i as i32,
                        name,
                        &format!("combatFilter {i}"),
                        filter == i,
                    ));
                }
                let key = self.dvar("cr_weapon");
                let text =
                    if key.is_empty() { "Weapon: All".into() } else { format!("Weapon: {}", gun_name(self, &key)) };
                m.items.extend(style::row(
                    self,
                    rect(-430.0, 56.0, 400.0, 22.0),
                    20,
                    &text,
                    "combatCycleWeapon",
                    false,
                ));
                m.items.extend(style::row(self, left_rect(298.0), 21, "Previous gun", "combatCycleWeapon -1", false));
                m.items.extend(style::row(self, left_rect(322.0), 22, "All weapons", "combatWeapon all", false));
                self.page_buttons(&mut m, challenge_levels(self).len(), CHALLENGE_PAGE);
            }
            3 => {
                m.items.push(field(-422.0, 224.0, 246.0, "cr_name_draft", 16));
                m.items.push(field(-164.0, 224.0, 124.0, "cr_clan_draft", 4));
                for (i, (label, command)) in [
                    ("Save Name & Clan Tag", "combatApplyIdentity"),
                    ("Edit Emblem", "combatEditEmblem"),
                    ("Choose Calling Card", "combatTab 4"),
                ]
                .iter()
                .enumerate()
                {
                    m.items.extend(style::row(
                        self,
                        left_rect(178.0 + i as f32 * 24.0),
                        10 + i as i32,
                        label,
                        command,
                        false,
                    ));
                }
            }
            4 => {
                for (i, card) in CARDS.iter().enumerate() {
                    // Tile the original square camo art without stretching it.
                    let y = 64.0 + i as f32 * 55.0;
                    let tile = 49.0 / BODY_SCALE;
                    let count = (400.0 / tile).floor() as usize;
                    let x = -430.0 + (400.0 - count as f32 * tile) * 0.5;
                    for column in 0..count {
                        m.items.push(Item {
                            window: Window {
                                rect: rect(x + column as f32 * tile, y, tile, 49.0),
                                style: 3,
                                background: Some(card.material.into()),
                                fore_color: [0.45, 0.45, 0.45, 1.0],
                                static_flags: flags::DECORATION,
                                dynamic_flags: flags::VISIBLE,
                                ..Window::default()
                            },
                            ..Item::default()
                        });
                    }
                    let mut row = style::row(
                        self,
                        rect(-430.0, y, 400.0, 49.0),
                        20 + i as i32,
                        card.name,
                        &format!("combatCard {}", card.id),
                        selected_card(&self.stats) == i,
                    );
                    for it in row.iter_mut().filter(|it| it.ty == item_type::BUTTON) {
                        it.text_align_mode = 4;
                        it.text_align_x = 10.0 * BODY_SCALE;
                        it.text_align_y = 4.0;
                        it.text_scale = 0.375;
                        it.text_style = 3;
                        it.window.fore_color = if self.stats.get(card.stat) >= card.target { WHITE } else { MUTED };
                    }
                    m.items.extend(row);
                }
            }
            _ => {}
        }
        m
    }

    fn emblem_menu(&self) -> Menu {
        let mut m = style::screen(self, EDITOR, "EMBLEM EDITOR");
        for (i, (label, command)) in [
            ("Save Emblem", "combatSaveEmblem"),
            ("Cancel", "combatCancelEmblem"),
            ("Undo", "combatUndo"),
            ("Clear", "combatClear"),
            ("Reset to Chevrons", "combatResetEmblem"),
        ]
        .iter()
        .enumerate()
        {
            m.items.extend(style::row(self, left_rect(34.0 + i as f32 * 24.0), i as i32 + 1, label, command, false));
        }
        m.items.extend(style::panel(self, rect(-440.0, 34.0, 420.0, 382.0)));
        for index in 0..PIXELS {
            let mut it = button(
                -422.0 + (index % GRID) as f32 * 15.0,
                88.0 + (index / GRID) as f32 * 15.0 * BODY_SCALE,
                15.0,
                15.0 * BODY_SCALE,
                "",
                &format!("combatPixel {index}"),
            );
            it.window.name = format!("pixel_{index}");
            m.items.push(it);
        }
        for color in 0..PALETTE.len() {
            m.items.push(button(
                -163.0 + (color % 4) as f32 * 30.0,
                109.0 + (color / 4) as f32 * 30.0 * BODY_SCALE,
                24.0,
                24.0 * BODY_SCALE,
                "",
                &format!("combatColor {color}"),
            ));
        }
        m
    }

    fn record_page(&self, count: usize, size: usize) -> usize {
        self.dvar("cr_page").parse::<usize>().unwrap_or(0).min(count.div_ceil(size).max(1) - 1)
    }

    fn page_buttons(&self, m: &mut Menu, count: usize, size: usize) {
        if count > size {
            m.items.extend(style::row(self, left_rect(370.0), 30, "Previous", "combatPage -1", false));
            m.items.extend(style::row(self, left_rect(394.0), 31, "Next", "combatPage 1", false));
        }
    }

    fn rebuild_record(&mut self) {
        self.close(RECORD);
        self.open(RECORD);
    }

    fn identity_drafts(&mut self) {
        let name = self.stats.dvars.get(NAME).cloned().unwrap_or_else(|| "Player".into());
        let clan = self.stats.dvars.get(CLAN).cloned().unwrap_or_default();
        self.set_dvar("cr_name_draft", &clean_name(&name, 16));
        self.set_dvar("cr_clan_draft", &clean_name(&clan, 4));
    }

    pub(in crate::ui) fn combat_record_script(&mut self, args: &[String]) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        match a(0).to_ascii_lowercase().as_str() {
            "combatopen" => {
                self.set_dvar("cr_tab", "0");
                self.set_dvar("cr_page", "0");
                self.set_dvar("cr_filter", "0");
                self.set_dvar("cr_weapon", "");
                self.identity_drafts();
                self.open(RECORD);
            }
            "combatback" => self.close(RECORD),
            "combattab" => {
                if a(1).parse::<usize>().is_ok_and(|n| n < TABS.len()) {
                    self.set_dvar("cr_tab", a(1));
                    self.set_dvar("cr_page", "0");
                    if a(1) == "3" {
                        self.identity_drafts();
                    }
                    self.rebuild_record();
                }
            }
            "combatfilter" => {
                if a(1).parse::<usize>().is_ok_and(|n| n < FILTERS.len()) {
                    self.set_dvar("cr_filter", a(1));
                    self.set_dvar("cr_page", "0");
                    self.rebuild_record();
                }
            }
            "combatweapon" => {
                if a(1) == "all" || weapon_keys(self).iter().any(|key| key == a(1)) {
                    self.set_dvar("cr_weapon", if a(1) == "all" { "" } else { a(1) });
                    self.set_dvar("cr_tab", "2");
                    self.set_dvar("cr_filter", "0");
                    self.set_dvar("cr_page", "0");
                    self.rebuild_record();
                }
            }
            "combatcycleweapon" => {
                let mut weapons = vec![String::new()];
                weapons.extend(weapon_keys(self));
                let current = weapons.iter().position(|key| *key == self.dvar("cr_weapon")).unwrap_or(0);
                let delta = if a(1) == "-1" { -1 } else { 1 };
                let next = (current as isize + delta).rem_euclid(weapons.len() as isize) as usize;
                self.set_dvar("cr_weapon", &weapons[next]);
                self.set_dvar("cr_page", "0");
                self.rebuild_record();
            }
            "combatpage" => {
                let weapons = self.dvar("cr_tab") == "1";
                let (count, size) = if weapons {
                    (weapon_keys(self).len(), WEAPON_PAGE)
                } else {
                    (challenge_levels(self).len(), CHALLENGE_PAGE)
                };
                let pages = count.div_ceil(size).max(1);
                let next = (self.record_page(count, size) as isize + if a(1) == "-1" { -1 } else { 1 })
                    .rem_euclid(pages as isize);
                self.set_dvar("cr_page", &next.to_string());
                self.rebuild_record();
            }
            "combatapplyidentity" => {
                let name = clean_name(&self.dvar("cr_name_draft"), 16);
                self.set_dvar(NAME, if name.is_empty() { "Player" } else { &name });
                let clan = clean_name(&self.dvar("cr_clan_draft"), 4);
                self.set_dvar(CLAN, &clan);
                self.identity_drafts();
                self.editing = None;
                self.stats.save_if_changed();
            }
            "combateditemblem" => {
                self.combat_record.pixels =
                    self.stats.dvars.get(EMBLEM).and_then(|s| decode_emblem(s)).unwrap_or_else(default_emblem);
                self.combat_record.undo.clear();
                self.open(EDITOR);
            }
            "combatpixel" => {
                if let Ok(index) = a(1).parse() {
                    self.combat_record.paint(index);
                }
            }
            "combatcolor" => {
                if let Ok(color) = a(1).parse::<u8>()
                    && usize::from(color) < PALETTE.len()
                {
                    self.combat_record.color = color;
                }
            }
            "combatundo" => {
                if let Some(pixels) = self.combat_record.undo.pop() {
                    self.combat_record.pixels = pixels;
                }
            }
            "combatclear" | "combatresetemblem" => {
                self.combat_record.checkpoint();
                self.combat_record.pixels =
                    if a(0).eq_ignore_ascii_case("combatclear") { [0; PIXELS] } else { default_emblem() };
            }
            "combatsaveemblem" => {
                let encoded = encode_emblem(&self.combat_record.pixels);
                self.set_dvar(EMBLEM, &encoded);
                self.stats.save_if_changed();
                self.close(EDITOR);
            }
            "combatcancelemblem" => self.close(EDITOR),
            "combatcard" => {
                if let Some(card) = CARDS.iter().find(|c| c.id == a(1) && self.stats.get(c.stat) >= c.target) {
                    self.set_dvar(CARD, card.id);
                    self.stats.save_if_changed();
                    self.rebuild_record();
                }
            }
            _ => {}
        }
    }

    pub(in crate::ui) fn paint_combat_record(&self, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
        if om.name != RECORD && om.name != EDITOR {
            return;
        }
        let mut p = Painter { fe: self, pl, ops };
        if om.name == EDITOR {
            p.heading("EDIT YOUR EMBLEM");
            p.text(-422.0, 74.0, 14.4, "Select a colour, then paint a cell.", WHITE);
            p.emblem(&self.combat_record.pixels, -422.0, 88.0, 240.0, true);
            p.text(-163.0, 94.0, 18.0, "Colours", GOLD);
            for (i, color) in PALETTE.iter().enumerate() {
                let (x, y) = (-163.0 + (i % 4) as f32 * 30.0, 109.0 + (i / 4) as f32 * 30.0 * BODY_SCALE);
                p.fill(x, y, 24.0, 24.0 * BODY_SCALE, if i == 0 { [0.2, 0.2, 0.225, 1.0] } else { *color });
                if i == 0 {
                    p.text(x + 6.0, y + 19.0, 18.0, "X", WHITE);
                }
                if i == usize::from(self.combat_record.color) {
                    p.border(x - 1.0, y - 1.0, 26.0, 24.0 * BODY_SCALE + 2.0, WHITE);
                }
            }
            p.text(-163.0, 255.0, 14.4, "X erases a cell.", MUTED);
            p.card(selected_card(&self.stats), -422.0, 346.0, 382.0, 52.0, true);
            p.emblem(&self.combat_record.pixels, -416.0, 352.0, 40.0, false);
            p.fit(-366.0, 370.0, 18.0, 310.0, &identity(&self.stats), WHITE);
            p.text(-366.0, 390.0, 14.4, "Preview - save to apply", WHITE);
        } else {
            match self.dvar("cr_tab").parse::<usize>().unwrap_or(0) {
                1 => p.weapons(),
                2 => p.challenges(),
                3 => p.profile(),
                4 => p.cards(),
                _ => p.overview(),
            }
        }
        // The original row templates provide menu focus. The editable canvas
        // and text fields alone need a cell/field outline over their content.
        if let Some((name, index)) = &self.focus
            && *name == om.name
            && let Some(it) = om.menu.items.get(*index)
            && (it.window.name.starts_with("pixel_")
                || it.ty == item_type::EDITFIELD
                || it.action.contains("combatColor"))
        {
            let r = it.window.rect;
            let (pos, size) = pl.rect(&r);
            let line = pl.scale;
            for (pos, size) in [
                (pos, Vec2::new(size.x, line)),
                (pos + Vec2::new(0.0, size.y - line), Vec2::new(size.x, line)),
                (pos, Vec2::new(line, size.y)),
                (pos + Vec2::new(size.x - line, 0.0), Vec2::new(line, size.y)),
            ] {
                p.ops.push(Op::Fill { pos, size, color: WHITE });
            }
        }
    }
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
        let fits = |s: &str| draw::text_width(&self.fe.assets.fonts[font], s, k) <= width * BODY_SCALE * self.pl.scale;
        let mut text = text.to_owned();
        if !fits(&text) {
            while !text.is_empty() && !fits(&format!("{text}...")) {
                text.pop();
            }
            text += "...";
        }
        self.text(x, y, height, &text, color);
    }
    fn pic(&mut self, x: f32, y: f32, w: f32, h: f32, material: &str, color: [f32; 4]) {
        let (pos, size) = self.pl.rect(&rect(x, y, w, h));
        self.ops.push(Op::Pic { pos, size, material: material.into(), color });
    }
    fn heading(&mut self, text: &str) {
        let height = 18.0 * self.pl.scale;
        let font = self.fe.font_for(1, height);
        let k = height / self.fe.assets.fonts[font].pixel_height as f32;
        let width = draw::text_width(&self.fe.assets.fonts[font], text, k) / self.pl.scale;
        self.text(-230.0 - width / BODY_SCALE * 0.5, 47.0, 18.0, text, WHITE);
    }
    fn right(&mut self, edge: f32, y: f32, height: f32, text: &str, color: [f32; 4]) {
        let font = self.fe.font_for(1, height * self.pl.scale);
        let k = height * self.pl.scale / self.fe.assets.fonts[font].pixel_height as f32;
        let width = draw::text_width(&self.fe.assets.fonts[font], text, k) / self.pl.scale;
        self.text(edge - width / BODY_SCALE, y, height, text, color);
    }
    fn left_text(&mut self, x: f32, y: f32, height: f32, text: &str, color: [f32; 4]) {
        let r = VRect { x, y, horz_align: 1, vert_align: 1, ..VRect::default() };
        let (pos, _) = self.pl.rect(&r);
        let font = self.fe.font_for(1, height * self.pl.scale);
        let k = height * self.pl.scale / self.fe.assets.fonts[font].pixel_height as f32;
        let mut text = text.to_owned();
        let fits = |s: &str| draw::text_width(&self.fe.assets.fonts[font], s, k) <= 200.0 * self.pl.scale;
        if !fits(&text) {
            while !text.is_empty() && !fits(&format!("{text}...")) {
                text.pop();
            }
            text += "...";
        }
        self.ops.push(Op::Text { text: text.into(), x: pos.x, y: pos.y, font, k, color, shadow: self.pl.scale });
    }
    fn bar(&mut self, x: f32, y: f32, width: f32, progress: f32) {
        self.pic(x, y, width, 6.0, "white", [0.1, 0.1, 0.1, 0.35]);
        self.pic(x, y, width * progress.clamp(0.0, 1.0), 6.0, "gradient_fadein", [1.0, 0.9, 0.5, 0.6]);
    }
    fn emblem(&mut self, pixels: &[u8; PIXELS], x: f32, y: f32, size: f32, grid: bool) {
        let cell = size / GRID as f32;
        self.fill(x, y, size, size * BODY_SCALE, [0.1, 0.1, 0.1, 0.35]);
        for (i, &pixel) in pixels.iter().enumerate() {
            let (cx, cy) = (x + (i % GRID) as f32 * cell, y + (i / GRID) as f32 * cell * BODY_SCALE);
            let color = if pixel == 0 && grid {
                if ((i % GRID) + i / GRID).is_multiple_of(2) {
                    [0.16, 0.18, 0.19, 1.0]
                } else {
                    [0.12, 0.14, 0.15, 1.0]
                }
            } else {
                PALETTE[usize::from(pixel.min(15))]
            };
            if color[3] > 0.0 {
                self.fill(
                    cx,
                    cy,
                    cell - if grid { 1.0 / BODY_SCALE } else { 0.0 },
                    cell * BODY_SCALE - if grid { 1.0 } else { 0.0 },
                    color,
                );
            }
        }
        self.border(x, y, size, size * BODY_SCALE, MUTED);
    }
    fn card(&mut self, index: usize, x: f32, y: f32, w: f32, h: f32, earned: bool) {
        let tile = h / BODY_SCALE;
        let color = if earned { [0.55, 0.55, 0.55, 1.0] } else { [0.25, 0.25, 0.25, 1.0] };
        for column in 0..(w / tile).ceil() as usize {
            let remaining = (w - column as f32 * tile).min(tile);
            let (pos, size) = self.pl.rect(&rect(x + column as f32 * tile, y, remaining, h));
            self.ops.push(Op::Image {
                pos,
                size,
                material: CARDS[index].material.into(),
                color,
                uv: Some(bevy::math::Rect::new(0.0, 0.0, remaining / tile, 1.0)),
                rot: 0.0,
                layer: 1,
            });
        }
        self.pic(x, y, w, h, "gradient_center", [0.1, 0.1, 0.1, 0.6]);
    }
    fn banner(&mut self) {
        self.heading("PLAYER RECORD");
        self.card(selected_card(&self.fe.stats), -422.0, 62.0, 382.0, 64.0, true);
        let rank = rank(self.fe);
        if let Some(pixels) = self.fe.stats.dvars.get(EMBLEM).and_then(|s| decode_emblem(s)) {
            self.emblem(&pixels, -414.0, 70.0, 48.0, false);
        } else if !rank.icon.is_empty() {
            self.pic(-414.0, 70.0, 48.0, 48.0 * BODY_SCALE, &rank.icon, WHITE);
        }
        self.fit(-354.0, 91.0, 22.0, 298.0, &identity(&self.fe.stats), WHITE);
        self.text(-354.0, 114.0, 18.0, CARDS[selected_card(&self.fe.stats)].name, WHITE);
        self.fit(-422.0, 150.0, 18.0, 304.0, &rank.name, GOLD);
        self.right(-40.0, 150.0, 18.0, &format!("Level {}", rank.level), GOLD);
        let xp =
            rank.next.map_or_else(|| "Maximum rank".into(), |next| format!("XP required: {}", (next - rank.xp).max(0)));
        self.text(-422.0, 174.0, 18.0, &xp, WHITE);
        let progress = rank.next.map_or(1.0, |max| (rank.xp - rank.min) as f32 / (max - rank.min).max(1) as f32);
        self.bar(-422.0, 182.0, 382.0, progress);
    }
    fn overview(&mut self) {
        self.banner();
        let s = &self.fe.stats;
        let rows = [
            (
                "Kill / Death Ratio",
                format!(
                    "{}  ({} / {})",
                    ratio(s.get(stat::KILLS), s.get(stat::DEATHS)),
                    s.get(stat::KILLS),
                    s.get(stat::DEATHS)
                ),
            ),
            (
                "Win / Loss Ratio",
                format!(
                    "{}  ({} / {})",
                    ratio(s.get(stat::WINS), s.get(stat::LOSSES)),
                    s.get(stat::WINS),
                    s.get(stat::LOSSES)
                ),
            ),
            ("Time Played", duration(s.get(stat::TIME_PLAYED_TOTAL).max(0) as u64)),
            ("Headshots", s.get(stat::HEADSHOTS).to_string()),
            ("Assists", s.get(stat::ASSISTS).to_string()),
            ("Best Kill Streak", s.get(stat::KILL_STREAK).to_string()),
            ("Total XP", s.get(stat::RANKXP).to_string()),
        ];
        for (i, (label, value)) in rows.iter().enumerate() {
            let y = 217.0 + i as f32 * 25.0;
            self.text(-422.0, y, 18.0, label, GOLD);
            self.right(-40.0, y, 18.0, value, WHITE);
            self.pic(-430.0, y + 9.0, 400.0, 0.5, "white", [1.0, 1.0, 1.0, 0.1]);
        }
        self.left_text(15.0, 194.0, 18.0, "Preferred Weapons", GOLD);
        let guns: Vec<_> = sorted_weapons(self.fe)
            .into_iter()
            .filter(|key| metric(&self.fe.stats, key, "kills") > 0)
            .take(3)
            .collect();
        if guns.is_empty() {
            self.left_text(15.0, 219.0, 14.4, "Play a match to build", MUTED);
            self.left_text(15.0, 239.0, 14.4, "your weapon record.", MUTED);
        } else {
            for (i, key) in guns.iter().enumerate() {
                self.left_text(15.0, 220.0 + i as f32 * 49.0, 18.0, &gun_name(self.fe, key), WHITE);
                self.left_text(
                    31.0,
                    240.0 + i as f32 * 49.0,
                    14.4,
                    &format!("{} kills", metric(&self.fe.stats, key, "kills")),
                    MUTED,
                );
            }
        }
    }
    fn weapons(&mut self) {
        self.heading("WEAPON STATISTICS");
        self.text(-422.0, 75.0, 14.4, "Select a weapon to view its challenges.", WHITE);
        let guns = sorted_weapons(self.fe);
        let page = self.fe.record_page(guns.len(), WEAPON_PAGE);
        for (row, key) in guns.iter().skip(page * WEAPON_PAGE).take(WEAPON_PAGE).enumerate() {
            let y = 120.0 + row as f32 * 46.0;
            self.fit(
                -422.0,
                y,
                14.4,
                382.0,
                &format!(
                    "Kills: {}    Headshots: {}    Shots: {}    Held: {}m",
                    metric(&self.fe.stats, key, "kills"),
                    metric(&self.fe.stats, key, "heads"),
                    metric(&self.fe.stats, key, "shots"),
                    metric(&self.fe.stats, key, "seconds") / 60
                ),
                WHITE,
            );
        }
        self.page_text(guns.len(), WEAPON_PAGE);
    }
    fn page_text(&mut self, count: usize, size: usize) {
        self.text(
            -422.0,
            398.0,
            14.4,
            &format!(
                "Page {} / {}    {count} entries",
                self.fe.record_page(count, size) + 1,
                count.div_ceil(size).max(1)
            ),
            MUTED,
        );
    }
    fn challenges(&mut self) {
        self.heading("CHALLENGES");
        let levels = challenge_levels(self.fe);
        let page = self.fe.record_page(levels.len(), CHALLENGE_PAGE);
        if levels.is_empty() {
            self.text(-422.0, 115.0, 18.0, "No challenges in this selection.", WHITE);
            self.text(-422.0, 140.0, 14.4, "CoD4 progress is available for CoD4 weapons.", MUTED);
        }
        for (row, entry) in levels.iter().skip(page * CHALLENGE_PAGE).take(CHALLENGE_PAGE).enumerate() {
            let RecordChallenge::Native(c, stage) = entry else {
                if let RecordChallenge::Mastery(weapon, camo) = entry {
                    self.mastery_challenge(row, weapon, *camo);
                }
                continue;
            };
            let l = &c.levels[*stage];
            let state = self.fe.stats.get(c.state);
            let (progress, done) = challenge_progress(state, *stage, self.fe.stats.get(l.progress), l.target);
            let y = 95.0 + row as f32 * 58.0;
            let title = self.fe.localize(&format!("@{}", l.name));
            let weapon = weapon_of(c).map(|key| gun_name(self.fe, key)).unwrap_or_default();
            self.fit(-422.0, y, 18.0, 275.0, &format!("{title}  {weapon}"), GOLD);
            let status = if done {
                "Complete".to_owned()
            } else if state == 0 {
                self.challenge_rank(c).map_or_else(|| "Locked".into(), |level| format!("Level {level}"))
            } else if state < *stage as i32 + 1 {
                "Locked stage".into()
            } else {
                format!("{progress} / {}", l.target)
            };
            self.right(-40.0, y, 14.4, &status, WHITE);
            let goal = self.challenge_goal(c, l);
            let reward = if !l.unlock_text.is_empty() {
                format!("{} XP - {}", l.xp, self.fe.localize(&format!("@{}", l.unlock_text)))
            } else {
                format!("{} XP", l.xp)
            };
            self.fit(-422.0, y + 18.0, 14.4, 382.0, &goal, WHITE);
            self.fit(-422.0, y + 35.0, 14.4, 382.0, &reward, MUTED);
            self.bar(-422.0, y + 41.0, 382.0, progress as f32 / l.target.max(1) as f32);
        }
        self.page_text(levels.len(), CHALLENGE_PAGE);
    }
    fn mastery_challenge(&mut self, row: usize, weapon: &str, camo: usize) {
        use crate::ui::mastery;
        let progress = mastery::progress(&self.fe.stats, &self.fe.assets, weapon, camo);
        let available = mastery::unlocked(&self.fe.stats, &self.fe.assets, weapon, camo);
        let y = 95.0 + row as f32 * 58.0;
        self.fit(-422.0, y, 18.0, 275.0, &format!("{}  {}", mastery::camo_name(camo), gun_name(self.fe, weapon)), GOLD);
        let status = if progress.complete {
            "Complete"
        } else if available {
            "Unlock All"
        } else {
            "Locked"
        };
        self.right(-40.0, y, 14.4, status, WHITE);
        let (goal, detail) = progress
            .description
            .rsplit_once('(')
            .map_or((progress.description.as_str(), ""), |(a, b)| (a, b.trim_end_matches([')', '.'])));
        self.fit(-422.0, y + 18.0, 14.4, 382.0, &goal.replace(" with this weapon", ""), WHITE);
        let xp = match camo {
            mastery::GOLD => 1000,
            mastery::PLATINUM => 2500,
            _ => 5000,
        };
        self.fit(-422.0, y + 35.0, 14.4, 382.0, &format!("{detail} - {xp} XP"), MUTED);
        self.bar(-422.0, y + 41.0, 382.0, progress.current as f32 / progress.target.max(1) as f32);
    }
    fn challenge_rank(&self, c: &Challenge) -> Option<i32> {
        let t = self.fe.assets.table("mp/ranktable.csv")?;
        (0..t.rows)
            .find(|&r| {
                t.get(r, 10).is_some_and(|s| s.split(';').any(|key| key.trim() == c.tier || key.trim() == c.first))
            })
            .and_then(|r| t.get(r, 14)?.trim().parse().ok())
    }
    fn challenge_goal(&self, c: &Challenge, l: &Level) -> String {
        if c.name.starts_with("ch_marksman_") {
            return format!("Get {} kills with this weapon", l.target);
        }
        if c.name.starts_with("ch_expert_") {
            return format!("Get {} headshots with this weapon", l.target);
        }
        for tier in 1..=10 {
            if let Some(t) = self.fe.assets.table(&format!("mp/challengetable_tier{tier}.csv"))
                && let Some(row) =
                    (0..t.rows).find(|&r| t.get(r, 3).and_then(|s| s.trim().parse::<i32>().ok()) == Some(l.progress))
            {
                let key = t.get(row, 9).unwrap_or("").trim();
                if !key.is_empty() {
                    let text = self
                        .fe
                        .localize(&format!("@{key}"))
                        .replace("&&1", &l.target.to_string())
                        .replace("%s", &l.target.to_string());
                    return text;
                }
            }
        }
        format!("Target: {}", l.target)
    }
    fn profile(&mut self) {
        self.banner();
        self.text(-422.0, 216.0, 18.0, "Custom Name", GOLD);
        self.text(-164.0, 216.0, 18.0, "Clan Tag", GOLD);
        self.text(-422.0, 282.0, 18.0, "Player Identity", GOLD);
        self.text(-422.0, 310.0, 14.4, "Your name and clan tag appear in matches.", WHITE);
        self.text(-422.0, 332.0, 14.4, "Name: 16 characters. Clan tag: 4 characters.", WHITE);
        self.text(-422.0, 356.0, 14.4, "Choose your emblem and calling card from", MUTED);
        self.text(-422.0, 378.0, 14.4, "the menu on the left.", MUTED);
    }
    fn cards(&mut self) {
        self.heading("CALLING CARDS");
        let selected = selected_card(&self.fe.stats);
        for (i, card) in CARDS.iter().enumerate() {
            let y = 64.0 + i as f32 * 55.0;
            let earned = self.fe.stats.get(card.stat) >= card.target;
            self.text(-420.0, y + 42.0, 14.4, card.goal, WHITE);
            let status = if selected == i {
                "Equipped".into()
            } else if earned {
                "Available".into()
            } else {
                format!(
                    "Locked: {}/{}",
                    self.fe.stats.get(card.stat).max(0) + if card.stat == stat::RANK { 1 } else { 0 },
                    card.target + if card.stat == stat::RANK { 1 } else { 0 }
                )
            };
            self.right(-40.0, y + 22.0, 14.4, &status, if earned { GOLD } else { WHITE });
            if !earned {
                self.pic(-54.0, y + 30.0, 14.0, 14.0, "specialty_locked", WHITE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_and_future_challenge_stages_display_honestly() {
        assert_eq!(challenge_progress(2, 0, 0, 25), (25, true));
        assert_eq!(challenge_progress(2, 1, 10, 75), (10, false));
        assert_eq!(challenge_progress(255, 2, 0, 150), (150, true));
        assert_eq!(challenge_progress(0, 0, -7, 25), (0, false));
        assert_eq!(challenge_progress(1, 0, 300, 25), (25, false));
    }

    #[test]
    #[ignore = "requires a local CoD4 asset installation"]
    fn installed_menus_challenges_and_identity_controls_work_together() {
        let assets = crate::ui::assets::UiAssets::load().unwrap();
        let mut fe = Frontend::bare(assets);
        fe.stats = Stats::in_memory();
        fe.dvars.retain(|key, _| !keeps_dvar(key));
        fe.open("main");
        let main = fe.stack.iter().find(|m| m.name == "main_text").unwrap();
        assert!(main.menu.items.iter().any(|i| i.text == "Combat Record" && i.action.contains("combatOpen")));
        assert_eq!(main.menu.items.iter().filter(|i| i.action.contains("combatOpen")).count(), 1);
        fe.combat_record_script(&["combatOpen".into()]);
        let native = fe.assets.menu("menu_challenges").unwrap();
        let record = fe.menu_item(RECORD).unwrap();
        assert_eq!(record.items[0].window.background, native.items[0].window.background);
        assert_eq!(record.items[6].font_enum, native.items[6].font_enum);
        assert_eq!(record.items[6].text_scale, native.items[6].text_scale);
        assert_eq!(record.items[6].text_style, native.items[6].text_style);
        assert!(record.items.iter().any(|i| i.window.background.as_deref() == Some("button_highlight_end")));
        let challenges = progression::challenges(&fe.assets);
        assert!(!challenges.is_empty());
        fe.set_dvar("cr_tab", "2");
        for filter in [0, 1, 2, 3] {
            fe.set_dvar("cr_filter", &filter.to_string());
            assert!(!challenge_levels(&fe).is_empty(), "empty challenge filter {filter}");
        }
        fe.set_dvar("cr_filter", "0");
        let total: usize = challenges.iter().map(|c| c.levels.len()).sum();
        let levels = challenge_levels(&fe);
        assert_eq!(
            levels.iter().filter(|l| matches!(l, RecordChallenge::Native(..))).count(),
            total,
            "original challenge stages remain available"
        );
        let mastery: Vec<_> = levels
            .iter()
            .filter_map(|l| match l {
                RecordChallenge::Mastery(weapon, camo) => Some((weapon.clone(), *camo)),
                _ => None,
            })
            .collect();
        let guns: Vec<_> = crate::ui::mastery::weapons(&fe.assets)
            .into_iter()
            .filter(|w| crate::ui::mastery::canonical(&w.key) == w.key)
            .collect();
        assert_eq!(mastery.len(), guns.len() * 3);
        for weapon in &guns {
            for &camo in crate::ui::mastery::CAMOS {
                assert_eq!(
                    mastery.iter().filter(|(key, id)| *key == weapon.key && *id == camo).count(),
                    1,
                    "exactly one mastery challenge per gun and finish"
                );
            }
        }
        println!(
            "{total} native challenge stages, {} mastery stages, {} weapons",
            mastery.len(),
            weapon_keys(&fe).len()
        );

        fe.combat_record_script(&["combatTab".into(), "3".into()]);
        let i = fe.menu_item(RECORD).unwrap().items.iter().position(|it| it.dvar == "cr_name_draft").unwrap();
        fe.activate(RECORD, i);
        assert!(fe.typing(), "clicking a field must not immediately save/end editing");
        fe.set_dvar("cr_name_draft", "Shane");
        fe.set_dvar("cr_clan_draft", "COD4");
        fe.combat_record_script(&["combatApplyIdentity".into()]);
        assert_eq!(identity(&fe.stats), "[COD4] Shane");
        assert!(!fe.typing());

        fe.combat_record_script(&["combatEditEmblem".into()]);
        fe.combat_record_script(&["combatClear".into()]);
        fe.combat_record_script(&["combatPixel".into(), "0".into()]);
        fe.combat_record_script(&["combatSaveEmblem".into()]);
        let saved = fe.stats.dvars.get(EMBLEM).unwrap().clone();
        assert_eq!(decode_emblem(&saved).unwrap()[0], 6);
        fe.combat_record_script(&["combatEditEmblem".into()]);
        fe.combat_record_script(&["combatClear".into()]);
        fe.combat_record_script(&["combatCancelEmblem".into()]);
        assert_eq!(fe.stats.dvars.get(EMBLEM), Some(&saved));

        fe.combat_record_script(&["combatCard".into(), "first_blood".into()]);
        assert_eq!(selected_card(&fe.stats), 0);
        fe.stats.set(stat::KILLS, 25);
        fe.combat_record_script(&["combatCard".into(), "first_blood".into()]);
        assert_eq!(selected_card(&fe.stats), 1);
        // All pages draw using the real fonts and table data, without a GPU.
        for tab in 0..TABS.len() {
            fe.combat_record_script(&["combatTab".into(), tab.to_string()]);
            let mut ops = Vec::new();
            fe.paint(&Placement::new(1280.0, 720.0), &mut ops);
            assert!(!ops.is_empty());
        }

        // Exercise the actual ECS/message tracker: a shot/kill from the
        // rifle still belongs to it when a pistol is now in hand. Timer
        // fractions survive frames, class selection and the match end.
        fe.stats = Stats::in_memory();
        let rifle = Box::leak(Box::new(crate::weapons::WeaponDef {
            name: "ak47_reflex_mp".into(),
            display_name: "record-test rifle",
            ..crate::weapons::WeaponDef::fallback()
        }));
        let pistol = Box::leak(Box::new(crate::weapons::WeaponDef {
            name: "beretta_mp".into(),
            display_name: "record-test pistol",
            ..crate::weapons::WeaponDef::fallback()
        }));
        let mut app = App::new();
        app.insert_resource(fe)
            .init_resource::<Tracking>()
            .insert_resource(Time::<()>::default())
            .add_message::<Killed>()
            .add_message::<ShotFired>()
            .add_systems(Update, track);
        let pawn = |team, id| Pawn { name: "Test".into(), team, id, kills: 0, deaths: 0, assists: 0 };
        let me = app
            .world_mut()
            .spawn((
                LocalPlayer,
                pawn(crate::combat::Team::Allies, 1),
                WeaponState { def: pistol, ..WeaponState::default() },
            ))
            .id();
        let enemy = app.world_mut().spawn(pawn(crate::combat::Team::Axis, 2)).id();
        app.world_mut().resource_mut::<Messages<ShotFired>>().write(ShotFired {
            shooter: me,
            weapon: Some(rifle),
            from: Vec3::ZERO,
            to: Vec3::ZERO,
            hit_pawn: true,
            normal: Vec3::Y,
            hit_world: false,
        });
        app.world_mut().resource_mut::<Messages<Killed>>().write(Killed {
            victim: enemy,
            attacker: Some(me),
            weapon: rifle.display_name,
            location: HitLocation::Head,
        });
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(1250));
        app.update();
        let stats = &app.world().resource::<Frontend>().stats;
        assert_eq!(metric(stats, "ak47", "kills"), 1);
        assert_eq!(metric(stats, "ak47", "heads"), 1);
        assert_eq!(metric(stats, "ak47", "shots"), 1);
        assert_eq!(metric(stats, "beretta", "kills"), 0);
        assert_eq!(metric(stats, "beretta", "seconds"), 1);
        assert_eq!(stats.get(stat::TIME_PLAYED_TOTAL), 1);
        app.world_mut().entity_mut(me).insert(AwaitingClass);
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_millis(800));
        app.update();
        assert_eq!(app.world().resource::<Frontend>().stats.get(stat::TIME_PLAYED_TOTAL), 1);
        app.world_mut().entity_mut(me).remove::<AwaitingClass>().insert(Dead { respawn_at: 99.0, killer: None });
        app.update();
        assert_eq!(app.world().resource::<Frontend>().stats.get(stat::TIME_PLAYED_TOTAL), 2);
        assert_eq!(metric(&app.world().resource::<Frontend>().stats, "beretta", "seconds"), 1);
        app.insert_resource(crate::tdm::MatchState { ended: Some((None, 2.85)), ..crate::tdm::MatchState::default() });
        app.update();
        assert_eq!(app.world().resource::<Frontend>().stats.get(stat::TIME_PLAYED_TOTAL), 2);
    }
}
