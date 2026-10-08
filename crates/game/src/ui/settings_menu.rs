//! The settings pages ([`crate::settings`]) in CoD4's own options menus:
//! each page takes over one of them (Graphics, Texture Settings as
//! Quality, Sound, Game Options; Look, Move, Combat, Interact, Multiplayer
//! Controls as Controller), its rows made from the page's own first row
//! (highlight bars, backdrop, label and value), so they look and behave as
//! CoD4's: choices step both ways (click, right click, the arrow keys, a
//! pad's left and right), sliders have a bar to click or drag, binds take
//! the next key or mouse button pressed. A line about the focused setting
//! shows where CoD4's graphics warning was. The left-hand lists lose the
//! pages we don't have, and their resets reset ours.

use super::expr::Env;
use super::Frontend;
use crate::bindings::{self, Action, Input};
use crate::settings::{self, Kind, Setting};
use iw3::menu::{EditField, Item, ItemData, Menu, Multi, Token, item_type};
use std::collections::HashMap;
use std::sync::Arc;

/// The first row's `ui_highlight` index; ours count up from here.
const FIRST_HIGHLIGHT: i32 = 100;
/// The description item's name.
const DESC: &str = "settings_desc";
/// Where a slider's bar goes, past its number (virtual units from the
/// value's text), and how long it is.
const BAR_AFTER: f32 = 46.0;
const BAR_LENGTH: f32 = 64.0;

/// The menus with the Options and Controls lists down their left side.
fn has_nav(name: &str) -> bool {
    name.starts_with("options_") || name == "main_options" || name == "main_controls" || name == "controls_multi"
}

/// Build the pages and fix the lists.
pub(super) fn build(menus: &mut HashMap<String, Arc<Menu>>) {
    // CoD4's graphics warning box: where the descriptions go.
    let desc_template = menus.get("options_graphics").and_then(|m| {
        m.items.iter().find(|it| it.text.eq_ignore_ascii_case("@MENU_GRAPHICS_WARNING")).cloned()
    });
    for page in settings::PAGES {
        let Some(menu) = menus.get_mut(page.menu) else {
            bevy::log::warn!("settings: no {} menu for {}", page.menu, page.title);
            continue;
        };
        let menu = Arc::make_mut(menu);
        // Test features' rows only in builds that have them.
        let rows: Vec<Setting> = page.rows.iter().filter(|s| cfg!(feature = "raytracing") || s.dvar != "r_lighting").copied().collect();
        if !rebuild(menu, &rows) {
            bevy::log::warn!("settings: {} has no rows to copy", page.menu);
            continue;
        }
        if let Some(mut d) = desc_template.clone() {
            d.window.name = DESC.into();
            d.text.clear();
            d.window.static_flags |= iw3::menu::flags::DECORATION;
            menu.items.push(d);
        }
    }
    for (name, menu) in menus.iter_mut() {
        if has_nav(name) {
            fix_nav(Arc::make_mut(menu));
        }
        // A page goes back to its list (Escape, B and its Back button): the
        // list's buttons close the list to open a page, so CoD4's "close
        // self" would drop all the way out.
        // The lists themselves go back to where they were opened from: the
        // main menu (under them) or, in a match, its pause menu.
        if name == "main_options" || name == "main_controls" {
            let menu = Arc::make_mut(menu);
            let back = "\"close\" \"self\" ; \"settingsBack\" ; ".to_owned();
            menu.on_esc = back.clone();
            for it in menu.items.iter_mut().filter(|it| it.ty == item_type::BUTTON && it.text.eq_ignore_ascii_case("@MENU_BACK")) {
                it.action = format!("\"play\" \"mouse_click\" ; {back}");
            }
        }
        if let Some(parent) = parent_of(name) {
            let menu = Arc::make_mut(menu);
            let back = format!("\"close\" \"self\" ; \"open\" \"{parent}\" ; ");
            menu.on_esc = back.clone();
            for it in menu.items.iter_mut().filter(|it| it.ty == item_type::BUTTON && it.text.eq_ignore_ascii_case("@MENU_BACK")) {
                it.action = format!("\"play\" \"mouse_click\" ; {back}");
            }
        }
    }
}

/// The settings list a page belongs to.
fn parent_of(page: &str) -> Option<&'static str> {
    match page {
        "options_look" | "options_move" | "options_shoot" | "options_misc" | "controls_multi" | "options_control_defaults" => Some("main_controls"),
        "options_graphics" | "options_graphics_texture" | "options_sound" | "options_game" | "options_graphics_defaults" | "options_defaults" => {
            Some("main_options")
        }
        _ => None,
    }
}

/// The value items of a page's rows (right of the lists), by height.
fn value_rows(menu: &Menu) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..menu.items.len())
        .filter(|&i| {
            let it = &menu.items[i];
            it.window.rect.x > -200.0 && it.on_focus.contains("ui_highlight") && super::interactive(it)
        })
        .collect();
    rows.sort_by(|&a, &b| menu.items[a].window.rect.y.total_cmp(&menu.items[b].window.rect.y));
    rows
}

/// Replace a page's rows with `rows`, made like its first. Whether it had one.
fn rebuild(menu: &mut Menu, rows: &[Setting]) -> bool {
    let values = value_rows(menu);
    let Some(&first) = values.first() else { return false };
    let first_item = menu.items[first].clone();
    let (x0, y0) = (first_item.window.rect.x, first_item.window.rect.y);
    let last_y = values.last().map_or(y0, |&i| menu.items[i].window.rect.y);
    let pitch = values.get(1).map_or(22.0, |&i| menu.items[i].window.rect.y - y0).max(16.0);
    let old_highlight = highlight_of(&first_item.on_focus);
    // The first row's items: everything in the right column at its height.
    let right = |it: &Item| it.window.rect.x >= x0 - 2.0;
    let template: Vec<Item> = menu.items.iter().filter(|it| right(it) && (it.window.rect.y - y0).abs() < 0.5).cloned().collect();
    // Out go the old rows, the Apply buttons and the warning.
    menu.items.retain(|it| {
        let row = right(it) && it.window.rect.y >= y0 - 0.5 && it.window.rect.y <= last_y + 0.5;
        let apply = it.text.eq_ignore_ascii_case("@MENU_APPLY") || it.text.eq_ignore_ascii_case("@MENU_GRAPHICS_WARNING");
        !(row || apply)
    });
    for (n, setting) in rows.iter().enumerate() {
        let y = y0 + n as f32 * pitch;
        let index = FIRST_HIGHLIGHT + n as i32;
        for t in &template {
            let mut it = t.clone();
            it.window.rect.y = y;
            renumber(&mut it.visible_exp, old_highlight, index);
            let is_value = t.window.rect.x == first_item.window.rect.x && t.dvar == first_item.dvar && super::interactive(t);
            let is_label = !is_value && t.ty == item_type::BUTTON || (!is_value && !t.text_exp.is_empty());
            if is_value {
                value_item(&mut it, setting, index);
            } else if is_label {
                it.text_exp.clear();
                it.text = setting.label.into();
            }
            menu.items.push(it);
        }
    }
    true
}

/// The `ui_highlight` index a focus script sets.
fn highlight_of(script: &str) -> i32 {
    let after = script.split("ui_highlight").nth(1).unwrap_or("");
    after.split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()).and_then(|s| s.parse().ok()).unwrap_or(-1)
}

fn renumber(exp: &mut [Token], from: i32, to: i32) {
    let mut after_highlight = false;
    for t in exp.iter_mut() {
        match t {
            Token::Str(s) if s.eq_ignore_ascii_case("ui_highlight") => after_highlight = true,
            Token::Int(v) if after_highlight && *v == from => {
                *v = to;
                after_highlight = false;
            }
            _ => {}
        }
    }
}

/// Make a row's value item a setting's.
fn value_item(it: &mut Item, s: &Setting, index: i32) {
    it.dvar = settings::dvar_of(s).into();
    it.on_focus = format!("\"play\" \"mouse_over\" ; \"setLocalVarInt\" \"ui_highlight\" \"{index}\" ; \"setLocalVarString\" \"ui_choicegroup\" \"\"");
    it.action = "\"play\" \"mouse_click\"".into();
    it.text.clear();
    it.text_exp.clear();
    it.visible_exp.clear();
    match s.kind {
        Kind::Toggle => {
            it.ty = item_type::MULTI;
            it.data = ItemData::Multi(Multi {
                labels: vec!["@MENU_OFF".into(), "@MENU_ON".into()],
                strings: vec!["0".into(), "1".into()],
                values: vec![0.0, 1.0],
                str_def: true,
            });
        }
        Kind::Choice(choices) => {
            it.ty = item_type::MULTI;
            it.data = ItemData::Multi(Multi {
                labels: choices.iter().map(|(l, _)| (*l).into()).collect(),
                strings: choices.iter().map(|(_, v)| (*v).into()).collect(),
                values: (0..choices.len()).map(|i| i as f32).collect(),
                str_def: true,
            });
        }
        Kind::Slider { min, max, .. } => {
            it.ty = item_type::SLIDER;
            it.data = ItemData::EditField(EditField { min, max, default: s.default.parse().unwrap_or(min), max_chars: 0, max_paint_chars: 0 });
        }
        Kind::Bind(_) => {
            it.ty = item_type::BIND;
            it.data = ItemData::None;
        }
    }
}

/// The left-hand lists: our pages' names, the resets ours, and no Voice
/// Chat or Cheats.
fn fix_nav(menu: &mut Menu) {
    let label = |it: &Item| match it.text_exp.as_slice() {
        [_, Token::Str(s)] => s.to_ascii_uppercase(),
        _ => String::new(),
    };
    // Out go the pages we don't have, and the list closes up after them.
    let gone = |l: &str| matches!(l, "@MENU_VOICECHAT" | "@MENU_CHEATS_LOWCASE" | "@MENU_MULTIPLAYER_OPTIONS");
    let mut gone_ys: Vec<f32> =
        menu.items.iter().filter(|it| it.window.rect.x < -200.0 && gone(&label(it))).map(|it| it.window.rect.y).collect();
    gone_ys.sort_by(|a, b| a.total_cmp(b));
    gone_ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    let nav_ys: Vec<f32> = {
        let mut ys: Vec<f32> = menu.items.iter().filter(|it| it.window.rect.x < -200.0 && it.ty == item_type::BUTTON).map(|it| it.window.rect.y).collect();
        ys.sort_by(|a, b| a.total_cmp(b));
        ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        ys
    };
    let pitch = nav_ys.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 1.0).fold(f32::MAX, f32::min);
    menu.items.retain(|it| !(it.window.rect.x < -200.0 && gone_ys.iter().any(|y| (it.window.rect.y - y).abs() < 0.5)));
    if pitch.is_finite() {
        for it in menu.items.iter_mut().filter(|it| it.window.rect.x < -200.0) {
            let above = gone_ys.iter().filter(|y| **y < it.window.rect.y - 0.5).count();
            it.window.rect.y -= above as f32 * pitch;
        }
    }
    // Our names in the page headers too.
    for it in menu.items.iter_mut() {
        for t in it.text_exp.iter_mut() {
            if let Token::Str(s) = t {
                match s.to_ascii_uppercase().as_str() {
                    "@MENU_TEXTURE_SETTINGS" => *s = "Quality...".into(),
                    "@MENU_MULTIPLAYER_CONTROLS" => *s = "Controller...".into(),
                    _ => {}
                }
            }
        }
    }
    for it in menu.items.iter_mut().filter(|it| it.window.rect.x < -200.0) {
        let l = label(it);
        let rename = |it: &mut Item, to: &str| it.text_exp = vec![Token::Op(16), Token::Str(to.into())];
        match l.as_str() {
            "QUALITY..." => {}
            "CONTROLLER..." => {
                it.visible_exp.clear();
            }
            "@MENU_RESET_SYSTEM_DEFAULTS" if it.ty == item_type::BUTTON => {
                rename(it, "Reset Options");
                it.action = "\"play\" \"mouse_click\" ; \"settingsReset\" \"options\"".into();
            }
            "@MENU_RESET_SYSTEM_DEFAULTS" => rename(it, "Reset Options"),
            "@MENU_SET_DEFAULT_CONTROLS" if it.ty == item_type::BUTTON => {
                it.action = "\"play\" \"mouse_click\" ; \"settingsReset\" \"controls\"".into();
            }
            _ => {}
        }
    }
}

impl Frontend {
    /// The setting a menu item edits.
    pub(super) fn setting_of(item: &Item) -> Option<&'static Setting> {
        (!item.dvar.is_empty()).then(|| settings::find(&item.dvar)).flatten()
    }

    /// How a setting's value item shows its value (sliders and binds; the
    /// choices are CoD4's own multis).
    pub(super) fn setting_value(&self, item: &Item) -> Option<String> {
        let s = Self::setting_of(item)?;
        match s.kind {
            Kind::Slider { decimals, suffix, .. } => {
                let v: f32 = self.dvar(&item.dvar).trim().parse().unwrap_or(0.0);
                Some(format!("{v:.prec$}{suffix}", prec = decimals as usize))
            }
            Kind::Bind(action) if self.capturing == Some(action) => Some("Press a key...".into()),
            Kind::Bind(_) => Some(bindings::display_value(&self.dvar(&item.dvar), "", "or")),
            _ => None,
        }
    }

    /// Set a setting, keeping the quality preset in step: a preset sets
    /// its settings; changing one of those makes the preset whichever it
    /// now matches, or Custom.
    pub(super) fn set_setting(&mut self, dvar: &str, value: &str) {
        self.set_dvar(dvar, value);
        if dvar.eq_ignore_ascii_case("r_preset") {
            if let Some(p) = settings::preset_index(value) {
                for (d, vals) in settings::PRESET_DVARS {
                    self.set_dvar(d, vals[p]);
                }
            }
        } else if settings::PRESET_DVARS.iter().any(|(d, _)| d.eq_ignore_ascii_case(dvar)) {
            let preset = settings::preset_of(|d| self.dvar(d));
            let name = preset.map_or("custom", |p| ["low", "medium", "high", "ultra"][p]);
            self.set_dvar("r_preset", name);
        }
    }

    /// A setting's row stepped one way (`dir` ±1): the next or previous
    /// choice, a slider's step. Whether it was a setting's.
    pub(super) fn setting_step(&mut self, item: &Item, dir: i32) -> bool {
        let Some(s) = Self::setting_of(item) else { return false };
        match (s.kind, &item.data) {
            (Kind::Toggle | Kind::Choice(_), ItemData::Multi(m)) if !m.labels.is_empty() => {
                let n = m.labels.len() as i32;
                let at = super::multi_index(m, &self.dvar(&item.dvar)) as i32;
                let next = (at + dir).rem_euclid(n) as usize;
                let v = m.strings[next].clone();
                self.set_setting(&item.dvar, &v);
                true
            }
            (Kind::Slider { min, max, step, .. }, _) => {
                let v: f32 = self.dvar(&item.dvar).trim().parse().unwrap_or(min);
                self.set_slider(&item.dvar, (v + step * dir as f32).clamp(min, max), step);
                true
            }
            _ => false,
        }
    }

    fn set_slider(&mut self, dvar: &str, v: f32, step: f32) {
        let v = (v / step).round() * step;
        let text = format!("{}", (v * 1000.0).round() / 1000.0);
        self.set_setting(dvar, &text);
    }

    /// Activating a setting's row (a click, Enter, a pad's A). Whether it
    /// was one: binds start listening, sliders go where they were clicked.
    pub(super) fn setting_activate(&mut self, item: &Item, clicked: Option<f32>) -> bool {
        let Some(s) = Self::setting_of(item) else { return false };
        match s.kind {
            Kind::Bind(action) => {
                self.capturing = Some(action);
                self.capture_armed = false;
                true
            }
            Kind::Slider { min, max, step, .. } => {
                match clicked.and_then(|x| self.slider_fraction(item, x)) {
                    Some(f) => self.set_slider(&item.dvar, min + f * (max - min), step),
                    None => {
                        self.setting_step(item, 1);
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Where along a slider's bar a screen x is (0..1), if on or near it.
    pub(super) fn slider_fraction(&self, item: &Item, screen_x: f32) -> Option<f32> {
        let pl = super::Placement::new(self.screen.x, self.screen.y);
        let (x0, x1) = self.slider_bar(item, &pl);
        ((x1 - x0) > 1.0 && screen_x >= x0 - 8.0 && screen_x <= x1 + 8.0).then(|| ((screen_x - x0) / (x1 - x0)).clamp(0.0, 1.0))
    }

    /// A slider's bar, in screen x.
    fn slider_bar(&self, item: &Item, pl: &super::Placement) -> (f32, f32) {
        let r = self.item_rect(item);
        let (pos, _) = pl.rect(&r);
        let sx = pl.sx(r.horz_align);
        let start = pos.x + (item.text_align_x + BAR_AFTER) * sx;
        (start, start + BAR_LENGTH * sx)
    }

    /// Draw a slider's bar.
    pub(super) fn paint_slider(&self, item: &Item, pl: &super::Placement, fore: [f32; 4], ops: &mut Vec<super::Op>) {
        let Some(s) = Self::setting_of(item) else { return };
        let Kind::Slider { min, max, .. } = s.kind else { return };
        let r = self.item_rect(item);
        let (pos, size) = pl.rect(&r);
        let (x0, x1) = self.slider_bar(item, pl);
        let v: f32 = self.dvar(&item.dvar).trim().parse().unwrap_or(min);
        let f = ((v - min) / (max - min).max(1e-6)).clamp(0.0, 1.0);
        let h = 4.0 * pl.scale;
        let y = pos.y + (size.y - h) * 0.5;
        let a = fore[3].max(0.6);
        ops.push(super::Op::Fill { pos: bevy::math::Vec2::new(x0, y), size: bevy::math::Vec2::new(x1 - x0, h), color: [0.3, 0.3, 0.3, a] });
        ops.push(super::Op::Fill { pos: bevy::math::Vec2::new(x0, y), size: bevy::math::Vec2::new((x1 - x0) * f, h), color: [0.85, 0.85, 0.75, a] });
        let knob = bevy::math::Vec2::new(4.0 * pl.scale, 10.0 * pl.scale);
        ops.push(super::Op::Fill {
            pos: bevy::math::Vec2::new(x0 + (x1 - x0) * f - knob.x * 0.5, pos.y + (size.y - knob.y) * 0.5),
            size: knob,
            color: [1.0, 1.0, 1.0, a],
        });
    }

    /// The line about the focused setting.
    pub(super) fn setting_desc(&self) -> String {
        let Some((menu, i)) = &self.focus else { return String::new() };
        let Some(m) = self.menu_item(menu) else { return String::new() };
        m.items.get(*i).and_then(Self::setting_of).map_or(String::new(), |s| s.desc.to_owned())
    }

    /// Is this the description item?
    pub(super) fn is_desc(item: &Item) -> bool {
        item.window.name == DESC
    }

    /// Listening for a bind: the next key or mouse button binds (from the
    /// frame after the press that started it), Backspace or Delete clears,
    /// Escape gives up. Whether it took the input this frame.
    pub(super) fn capture(&mut self, keys: &bevy::input::ButtonInput<bevy::input::keyboard::KeyCode>, mouse: &bevy::input::ButtonInput<bevy::input::mouse::MouseButton>, injected: &[bevy::input::keyboard::KeyCode]) -> bool {
        use bevy::input::keyboard::KeyCode;
        let Some(action) = self.capturing else { return false };
        if !self.capture_armed {
            // Wait for what started it to be let go.
            if keys.get_pressed().next().is_none() && mouse.get_pressed().next().is_none() {
                self.capture_armed = true;
            }
            return true;
        }
        // Escape (a pad's B too) gives up.
        if keys.just_pressed(KeyCode::Escape) {
            self.capturing = None;
            return true;
        }
        let key = keys.get_just_pressed().copied().find(|k| !injected.contains(k));
        let input = match key {
            Some(KeyCode::Escape) => {
                self.capturing = None;
                return true;
            }
            Some(KeyCode::Backspace | KeyCode::Delete) => {
                self.set_dvar(action.dvar(), "");
                self.capturing = None;
                return true;
            }
            Some(k) if bindings::KEYS.contains(&k) => Some(Input::Key(k)),
            Some(_) => None,
            None => mouse.get_just_pressed().next().map(|m| Input::Mouse(*m)),
        };
        if let Some(input) = input {
            let mut values: HashMap<&'static str, String> =
                Action::ALL.iter().map(|a| (a.dvar(), self.dvars.get(a.dvar()).cloned().unwrap_or_else(|| a.default_value()))).collect();
            bindings::bind(&mut values, action, input);
            for (d, v) in values {
                if self.dvar(d) != v {
                    self.set_dvar(d, &v);
                }
            }
            self.capturing = None;
            self.run("\"play\" \"mouse_click\"", "");
        }
        true
    }

    /// `settingsReset options|controls`: back to the defaults.
    pub(super) fn reset_settings(&mut self, which: &str) {
        let controls = which.eq_ignore_ascii_case("controls");
        let pages: Vec<&settings::Page> = settings::PAGES
            .iter()
            .filter(|p| matches!(p.menu, "options_look" | "options_move" | "options_shoot" | "options_misc" | "controls_multi") == controls)
            .collect();
        for p in pages {
            for s in p.rows {
                let (d, v) = (settings::dvar_of(s), settings::default_of(s));
                self.set_dvar(d, &v);
            }
        }
    }
}

/// The settings' defaults, for the menus' dvars before the saved ones.
pub(super) fn default_dvars() -> impl Iterator<Item = (String, String)> {
    settings::all()
        .map(|s| (settings::dvar_of(s).to_ascii_lowercase(), settings::default_of(s)))
        .chain([("ui_allow_graphic_change".to_owned(), "1".to_owned())])
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_index_is_read_from_the_focus_script() {
        assert_eq!(highlight_of("\"play\" \"mouse_over\" ; \"setLocalVarInt\" \"ui_highlight\" \"22\" ; "), 22);
        assert_eq!(highlight_of("\"setLocalVarInt\" \"ui_highlight\" 3 ;"), 3);
    }
}
