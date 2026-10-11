//! Picking a class weapon's red dot reticle ([`crate::reticles`]): a
//! "Reticle..." row in every camo popup opens this screen, CoD4's Rank &
//! Challenges shell as the Camo Editor's, with rows for the shape, colour
//! and size (stepped by a click, the arrow keys or a pad's left and right,
//! as the editor's), a picture of the reticle lit on the lens, and Save.
//! Saved in the class's stats beside its camo; the gun takes it with its
//! camo number ([`crate::reticles::with_camo`]).

use super::{Frontend, Op, OpenMenu, draw::Placement, expr::Env};
use crate::reticles::{COLOURS, Reticle, SHAPES, SIZES};
use iw3::menu::{ItemData, Menu, Multi, Rect as VRect, item_type};
use std::sync::Arc;

#[path = "combat_record/style.rs"]
mod style;

pub(super) const MENU: &str = "cod4rw_reticle";
/// The popups' row.
pub(super) const ROW_NAME: &str = "camo_reticle";
pub(super) const ROW_LABEL: &str = "COD4RW_RETICLE";
/// A class weapon's reticle: this plus its weapon stat (201, 203, ...).
const STATS: i32 = 4900;
/// The class weapon stat being set.
const STAT: &str = "ccamo_ret_stat";
const SHAPE: &str = "ccamo_ret_shape";
const COLOUR: &str = "ccamo_ret_colour";
const SIZE: &str = "ccamo_ret_size";
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 0.9];
const MUTED: [f32; 4] = [0.69, 0.69, 0.69, 1.0];
const GOLD: [f32; 4] = [1.0, 0.85, 0.5, 0.8];
const BODY_SCALE: f32 = 366.0 / 420.0;

fn rect(x: f32, y: f32, w: f32, h: f32) -> VRect {
    VRect { x: -386.0 + (x + 440.0) * BODY_SCALE, y, w: w * BODY_SCALE, h, horz_align: 3, vert_align: 1 }
}

fn left_rect(y: f32) -> VRect {
    VRect { x: 0.0, y, w: 220.0, h: 22.0, horz_align: 1, vert_align: 1 }
}

/// The picture of reticle `code` (see [`super::assets`]).
pub(super) fn picture(code: u16) -> String {
    format!("cod4rw_reticle_{code}")
}

fn multi(labels: Vec<String>) -> ItemData {
    ItemData::Multi(Multi {
        strings: (0..labels.len()).map(|i| i.to_string()).collect(),
        values: (0..labels.len()).map(|i| i as f32).collect(),
        labels,
        str_def: true,
    })
}

/// Is this one of the screen's rows (stepped both ways)?
pub(super) fn is_row(dvar: &str) -> bool {
    dvar.starts_with("ccamo_ret_")
}

impl Frontend {
    /// Class weapon `stat`'s reticle.
    pub(super) fn class_reticle(&self, stat: i32) -> Reticle {
        Reticle::from_code(self.stat(STATS + stat).clamp(0, u16::MAX as i32) as u16)
    }

    /// The reticle as the rows set it.
    fn reticle_rows(&self) -> Reticle {
        let n = |d: &str| self.dvar(d).parse::<u8>().unwrap_or(0);
        // The size row reads small, medium, large; the code's 0 is medium.
        let size = match n(SIZE) {
            0 => 2,
            2 => 1,
            _ => 0,
        };
        Reticle { shape: n(SHAPE).min(SHAPES.len() as u8 - 1), colour: n(COLOUR).min(COLOURS.len() as u8 - 1), size }
    }

    pub(super) fn reticle_menu(&self, key: &str) -> Option<Arc<Menu>> {
        (key == MENU).then(|| Arc::new(self.reticle_screen()))
    }

    fn reticle_screen(&self) -> Menu {
        let mut m = style::screen(self, MENU, "RETICLE");
        let back = "\"play\" \"mouse_click\" ; \"uiScript\" creticleBack ;".to_owned();
        m.on_esc = back.clone();
        for it in m.items.iter_mut().filter(|it| it.window.name.eq_ignore_ascii_case("back")) {
            it.action = back.clone();
        }
        let rows: [(&str, &str, Vec<String>); 3] = [
            ("Shape:", SHAPE, SHAPES.iter().map(|s| (*s).to_owned()).collect()),
            ("Colour:", COLOUR, COLOURS.iter().map(|c| c.0.to_owned()).collect()),
            ("Size:", SIZE, SIZES.iter().map(|s| s.0.to_owned()).collect()),
        ];
        for (i, (label, dvar, labels)) in rows.into_iter().enumerate() {
            let mut row = style::row(self, left_rect(34.0 + i as f32 * 24.0), i as i32 + 1, label, "creticleChanged", false);
            for it in row.iter_mut().filter(|it| it.ty == item_type::BUTTON) {
                it.ty = item_type::MULTI;
                it.dvar = dvar.into();
                it.data = multi(labels.clone());
            }
            m.items.extend(row);
        }
        for (i, (label, command)) in [("Save Reticle", "creticleSave"), ("Classic Red Dot", "creticleReset")].into_iter().enumerate() {
            m.items.extend(style::row(self, left_rect(118.0 + i as f32 * 24.0), 10 + i as i32, label, command, false));
        }
        m.items.extend(style::panel(self, rect(-440.0, 34.0, 420.0, 382.0)));
        m
    }

    /// `uiScript creticle...`.
    pub(super) fn reticle_script(&mut self, args: &[String]) {
        let a = |i: usize| args.get(i).map_or("", String::as_str);
        match a(0).to_ascii_lowercase().as_str() {
            "creticleopen" => {
                let Ok(stat) = a(1).parse::<i32>() else { return };
                self.set_dvar(STAT, &stat.to_string());
                let r = self.class_reticle(stat);
                self.set_dvar(SHAPE, &r.shape.to_string());
                self.set_dvar(COLOUR, &r.colour.to_string());
                let size_row = match r.size {
                    2 => 0,
                    1 => 2,
                    _ => 1,
                };
                self.set_dvar(SIZE, &size_row.to_string());
                self.open(MENU);
            }
            "creticlesave" => {
                let stat: i32 = self.dvar(STAT).parse().unwrap_or(0);
                if (201..250).contains(&stat) {
                    self.stats.set(STATS + stat, self.reticle_rows().code() as i32);
                    self.stats.save_if_changed();
                }
                self.close(MENU);
            }
            "creticlereset" => {
                self.set_dvar(SHAPE, "0");
                self.set_dvar(COLOUR, "0");
                self.set_dvar(SIZE, "1");
            }
            "creticleback" => self.close(MENU),
            _ => {}
        }
    }

    /// The new UI's preview: the lens picture, the reticle's name and the
    /// note under it.
    pub(in crate::ui) fn reticle_preview(&self) -> (String, String, &'static str) {
        let stat: i32 = self.dvar(STAT).parse().unwrap_or(201);
        let r = self.reticle_rows();
        let name = format!("{} {} - {}", COLOURS[r.colour as usize].0, SHAPES[r.shape as usize], SIZES[match r.size {
            1 => 2,
            2 => 0,
            _ => 1,
        }]
        .0);
        let gun = self.gun_for(stat).map(|g| g.0).unwrap_or_default();
        let note = if gun.contains("reflex") { "Shown in this weapon's Red Dot Sight." } else { "Shown when this weapon has a Red Dot Sight." };
        (picture(r.code()), name, note)
    }

    pub(super) fn paint_reticle(&self, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
        if om.name != MENU || om.menu.window.name.starts_with(super::next::picker::PREFIX) {
            return;
        }
        let stat: i32 = self.dvar(STAT).parse().unwrap_or(201);
        let r = self.reticle_rows();
        let text = |ops: &mut Vec<Op>, x: f32, y: f32, h: f32, s: &str, c: [f32; 4]| {
            let (pos, _) = pl.rect(&rect(x, y, 0.0, 0.0));
            let font = self.font_for(1, h * pl.scale);
            let k = h * pl.scale / self.assets.fonts[font].pixel_height as f32;
            ops.push(Op::Text { text: s.to_owned(), x: pos.x, y: pos.y, font, k, color: c, shadow: pl.scale });
        };
        text(ops, -422.0, 52.0, 18.0, "THROUGH THE SIGHT", WHITE);
        // The reticle on the lens, square.
        let (pos, size) = pl.rect(&rect(-380.0, 64.0, 260.0 / BODY_SCALE, 260.0));
        ops.push(Op::Pic { pos, size, material: picture(r.code()), color: [1.0; 4] });
        let name = format!("{} {} - {}", COLOURS[r.colour as usize].0, SHAPES[r.shape as usize], SIZES[match r.size {
            1 => 2,
            2 => 0,
            _ => 1,
        }]
        .0);
        text(ops, -422.0, 350.0, 18.0, &name, GOLD);
        let gun = self.gun_for(stat).map(|g| g.0).unwrap_or_default();
        let note = if gun.contains("reflex") {
            "Shown in this weapon's Red Dot Sight."
        } else {
            "Shown when this weapon has a Red Dot Sight."
        };
        text(ops, -422.0, 374.0, 14.4, note, MUTED);
    }
}
