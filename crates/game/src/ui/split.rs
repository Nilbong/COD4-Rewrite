//! Splitscreen menus: each local player has their own in-game menus (the
//! class menu as the match starts, their pause menu), drawn in their part
//! of the window and worked by their device alone, while the others play on.
//!
//! The menu machinery is the front end's own: a player's menus (open menus,
//! focus, text being edited, menu locals) are swapped into the [`Frontend`]
//! while they're worked or drawn, then swapped back out. Without
//! splitscreen the front end's own stack is the in-game menus as before.

use super::draw::{DrawList, Placement};
use super::hud::HudState;
use super::pad::{first, spots, step};
use super::{Frontend, Op, OpenMenu};
use crate::combat::{Dead, Pawn, Team};
use crate::loadout::{AwaitingClass, ClassChoice, Loadout};
use crate::splitscreen::{Device, LocalPlayers, LocalSlot, PlayerInput};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use std::collections::HashMap;

/// One local player's in-game menus.
#[derive(Default)]
pub(super) struct SlotMenu {
    stack: Vec<OpenMenu>,
    focus: Option<(String, usize)>,
    editing: Option<(String, usize)>,
    locals: HashMap<String, String>,
    /// Their team (the front end's, for the menus' `team()`; and the
    /// match's view of it).
    team: Team,
    mine: Option<usize>,
    /// Their menus' `scriptMenuResponse`s, till [`update`] answers them.
    responses: Vec<(String, String)>,
    /// The top menu last frame, and the item each menu was left from (a pad
    /// comes back to it).
    last_top: Option<String>,
    left_from: HashMap<String, usize>,
    /// Their profile's stats (players 2 to 4 with a profile picked in the
    /// lobby): their classes in their menus, and their XP.
    pub(super) stats: Option<super::stats::Stats>,
}

impl Frontend {
    /// Swap `slot`'s menus with the front end's own (again to swap back).
    fn swap_slot(&mut self, slot: usize) {
        while self.slot_menus.len() <= slot {
            let (locals, team, mine) = (self.locals.clone(), self.player_team, self.game.mine);
            let n = self.slot_menus.len();
            let stats = self.lobby.slot_profile(n).and_then(|id| {
                let path = super::profiles::stats_path(&id)?;
                let stats = self.stats.load_from(&self.assets, path);
                info!("ui: player {} plays as profile {id:?}", n + 1);
                stats
            });
            self.slot_menus.push(SlotMenu { locals, team, mine, stats, ..default() });
        }
        let m = &mut self.slot_menus[slot];
        std::mem::swap(&mut self.stack, &mut m.stack);
        std::mem::swap(&mut self.focus, &mut m.focus);
        std::mem::swap(&mut self.editing, &mut m.editing);
        std::mem::swap(&mut self.locals, &mut m.locals);
        std::mem::swap(&mut self.player_team, &mut m.team);
        std::mem::swap(&mut self.game.mine, &mut m.mine);
        std::mem::swap(&mut self.responses, &mut m.responses);
        if let Some(stats) = m.stats.as_mut() {
            std::mem::swap(&mut self.stats, stats);
        }
    }

    /// Splitscreen player `slot`'s own profile stats, if they have one.
    pub(super) fn slot_stats(&mut self, slot: usize) -> Option<(&mut super::stats::Stats, &super::assets::UiAssets)> {
        if slot == 0 {
            return None;
        }
        // (Made with their menus: their profile is loaded then.)
        if self.slot_menus.len() <= slot && crate::splitscreen::active() {
            self.swap_slot(slot);
            self.swap_slot(slot);
        }
        Some((self.slot_menus.get_mut(slot)?.stats.as_mut()?, &self.assets))
    }

    /// Run `f` on `slot`'s menus.
    pub(super) fn with_slot<R>(&mut self, slot: usize, f: impl FnOnce(&mut Frontend) -> R) -> R {
        self.swap_slot(slot);
        let out = f(self);
        self.swap_slot(slot);
        out
    }

    /// Does a local player have a menu up (splitscreen)?
    pub fn slot_menu_open(&self, slot: usize) -> bool {
        self.slot_menus.get(slot).is_some_and(|m| !m.stack.is_empty())
    }

    /// Does any local player have a menu up?
    pub fn any_slot_menu(&self) -> bool {
        self.slot_menus.iter().any(|m| !m.stack.is_empty())
    }

    /// Open `menu` for a local player, on their team.
    pub(super) fn open_for(&mut self, slot: usize, menu: &str, team: Team) {
        self.with_slot(slot, |fe| {
            fe.locals.insert("ui_team".into(), team_local(team).into());
            fe.player_team = team;
            fe.game.mine = Some((team == Team::Axis) as usize);
            fe.menu_slot = slot;
            fe.open(menu);
        });
    }

    /// Forget every player's menus (the match is over).
    pub(super) fn clear_slot_menus(&mut self) {
        for m in &mut self.slot_menus {
            if let Some(stats) = m.stats.as_mut() {
                stats.save_if_changed();
            }
        }
        self.slot_menus.clear();
        self.kbm_menu = false;
    }
}

fn team_local(team: Team) -> &'static str {
    if team == Team::Axis { "opfor" } else { "marines" }
}

/// Each local player's menus: Escape (a pad's Menu) opens their pause menu;
/// with one up their device moves through it (the mouse over their part of
/// the window, a pad's D-pad or stick), selects (a click, Enter, A) and goes
/// back (Escape, B); and a class picked is theirs.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn update(
    mut fe: ResMut<Frontend>,
    time: Res<Time>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    devices: Res<LocalPlayers>,
    mut choice: ResMut<ClassChoice>,
    mut hud: ResMut<HudState>,
    players: Query<(&LocalSlot, &PlayerInput, &Pawn, Has<AwaitingClass>, Has<Dead>, Has<Loadout>)>,
    mut was_open: Local<bool>,
) {
    if !crate::splitscreen::active() || fe.match_only {
        return;
    }
    let count = crate::splitscreen::count();
    let now = time.elapsed_secs();
    let mut slots: Vec<_> = players.iter().collect();
    slots.sort_by_key(|p| p.0.0);
    for (slot, input, pawn, awaiting, dead, equipped) in slots {
        let s = slot.0;
        let kbm = devices.devices.get(s) == Some(&Device::KeyboardMouse);
        // A menu's script can close it as it answers (a class picked):
        // the answer still counts.
        let pending = fe.slot_menus.get(s).is_some_and(|m| !m.responses.is_empty());
        if !fe.slot_menu_open(s) && !pending {
            if !awaiting && input.keys.just_pressed(KeyCode::Escape) {
                fe.open_for(s, "class", pawn.team);
            }
            continue;
        }
        let (origin, size) = crate::splitscreen::logical_rect(s, count, &window);
        let pl = Placement::new(size.x.max(1.0), size.y.max(1.0));
        fe.swap_slot(s);
        fe.menu_slot = s;
        fe.esc_used = false;
        let top = fe.stack.last().map(|m| m.name.clone());
        let opened = fe.slot_menus[s].last_top != top;
        fe.slot_menus[s].last_top = top.clone();
        if kbm {
            // The mouse over their part of the window.
            let p = window.cursor_position().map(|c| c - origin).filter(|p| p.cmpge(Vec2::ZERO).all() && p.cmplt(size).all());
            if let Some(p) = p {
                let hover = fe.hit(&pl, p);
                if hover != fe.focus {
                    fe.set_focus(hover);
                }
            }
            if mouse.just_pressed(MouseButton::Left) || input.keys.just_pressed(KeyCode::Enter) {
                if let Some((menu, i)) = fe.focus.clone() {
                    fe.activate(&menu, i);
                }
            }
        } else if let Some(top) = top {
            // A pad: focus moves the way the D-pad or stick points.
            let spots = spots(&fe, &pl);
            let current = fe.focus.as_ref().filter(|(m, _)| *m == top).and_then(|(_, i)| spots.iter().find(|x| x.item == *i).copied());
            if let Some(cur) = current {
                fe.slot_menus[s].left_from.insert(top.clone(), cur.item);
            }
            let remembered = fe.slot_menus[s].left_from.get(&top).and_then(|i| spots.iter().find(|x| x.item == *i).copied());
            let target = match (current, input.pad.menu_dir) {
                (None, _) if opened => remembered.or_else(|| first(&spots)),
                (None, Some(_)) => first(&spots),
                (Some(from), Some(dir)) => step(&spots, from, dir),
                _ => None,
            };
            if let Some(to) = target {
                fe.set_focus(Some((top, to.item)));
            }
            if input.keys.just_pressed(KeyCode::Enter) {
                if let Some((menu, i)) = fe.focus.clone() {
                    fe.activate(&menu, i);
                }
            }
        }
        // Back: the top menu's Escape (not the press that opened it).
        if !opened && input.keys.just_pressed(KeyCode::Escape) && !fe.esc_used {
            if let Some(m) = fe.stack.last() {
                let (name, esc) = (m.name.clone(), m.menu.on_esc.clone());
                fe.esc_used = true;
                fe.run(&esc, &name);
            }
        }
        super::ingame::respond(&mut fe, s, (awaiting, dead, equipped), &mut choice, &mut hud, now);
        if fe.stack.is_empty() {
            fe.slot_menus[s].last_top = None;
        }
        fe.swap_slot(s);
    }
    // A keyboard and mouse player's menu has the mouse; the game takes it
    // back after.
    let kbm_menu = (0..count).any(|s| devices.devices.get(s) == Some(&Device::KeyboardMouse) && fe.slot_menu_open(s));
    fe.kbm_menu = kbm_menu;
    if kbm_menu {
        if cursor.grab_mode != CursorGrabMode::None || cursor.visible {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = false;
        }
    } else if *was_open {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    *was_open = kbm_menu;
}

/// Each player's menus in their part of the window, over their HUD.
pub(super) fn paint(fe: &mut Frontend, list: &mut DrawList, images: &mut Assets<Image>, window: &Window, devices: &LocalPlayers) {
    let count = crate::splitscreen::count();
    for s in 0..count {
        if !fe.slot_menu_open(s) {
            continue;
        }
        let (origin, size) = crate::splitscreen::logical_rect(s, count, window);
        let pl = Placement::new(size.x.max(1.0), size.y.max(1.0));
        let mut ops = Vec::new();
        fe.with_slot(s, |fe| fe.paint(&pl, &mut ops));
        // Their mouse cursor, if theirs is the mouse.
        if devices.devices.get(s) == Some(&Device::KeyboardMouse) {
            if let Some(p) = window.cursor_position().map(|c| c - origin).filter(|p| p.cmpge(Vec2::ZERO).all() && p.cmplt(size).all()) {
                let at = Vec2::splat(32.0 * pl.scale);
                ops.push(Op::Pic { pos: p - at * 0.5, size: at, material: "ui_cursor".into(), color: [1.0; 4] });
            }
        }
        ops.extend(super::hud::menu_banner_ops(fe, &pl, &format!("PLAYER {}", s + 1)));
        for op in &mut ops {
            op.offset(origin);
        }
        fe.emit(ops, images, &mut list.0);
    }
}
