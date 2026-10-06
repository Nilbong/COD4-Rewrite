//! CoD4's in-game menus during a match started from the menus: the class
//! menu when the match starts (the player spawns once a class is picked),
//! and Escape's menu, whose Choose Class changes the class from the next
//! spawn. Picking a class responds with `scriptMenuResponse "custom1,0"`
//! (or a default class, `"assault_mp,0"`), which [`crate::loadout`] equips.

use super::Frontend;
use super::hq;
use super::attachments;
use super::draw::DrawList;
use super::hud::HudState;
use super::expr::Env;
use crate::combat::Dead;
use crate::loadout::{AwaitingClass, ClassChoice, ClassLoadout, Gun, Loadout};
use crate::movement::Frozen;
use crate::player::LocalPlayer;
use crate::world::MapName;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

/// CoD4's default classes: `mp/classTable.csv` rows from stat 200, ten each.
const DEFAULT_CLASSES: [&str; 5] = ["assault_mp", "specops_mp", "heavygunner_mp", "demolitions_mp", "sniper_mp"];

/// Run condition: no in-game menu is up (and none just used Escape), so the
/// game can have the mouse.
pub fn no_ingame_menu(fe: Option<Res<Frontend>>) -> bool {
    fe.is_none_or(|fe| fe.stack.is_empty() && !fe.esc_used && !fe.kbm_menu)
}

/// Is an in-game menu up?
pub fn menu_open(fe: Option<&Frontend>) -> bool {
    fe.is_some_and(|fe| !fe.stack.is_empty())
}

/// As the match starts: the class menu, with the player waiting to spawn
/// (in splitscreen every player, who pick in turn).
pub(super) fn start(
    mut commands: Commands,
    mut fe: ResMut<Frontend>,
    map: Res<MapName>,
    player: Query<(Entity, &crate::combat::Pawn), Added<LocalPlayer>>,
    others: Query<(Entity, &crate::splitscreen::LocalSlot, &crate::combat::Pawn), (Added<crate::splitscreen::LocalSlot>, Without<LocalPlayer>)>,
) {
    // A match started without the menus (its HUD's assets load late, so
    // the player looks newly added) has no class to pick.
    let Ok((player, pawn)) = player.single() else { return };
    // The side the lobby put us on.
    fe.player_team = pawn.team;
    if fe.match_only {
        return;
    }
    // The in-game menus were added by `hud::prepare`; the team names and
    // icons follow the map (`hud::sync_game`).
    for (k, v) in [("onlinegame", "1"), ("mapname", map.0.as_str())] {
        fe.set_dvar(k, v);
    }
    fe.stack.clear();
    fe.focus = None;
    fe.clear_slot_menus();
    let team = if pawn.team == crate::combat::Team::Axis { "opfor" } else { "marines" };
    fe.locals.insert("ui_team".into(), team.into());
    fe.menu_slot = 0;
    // Headquarters has no class to pick (its Escape menu edits them).
    if crate::hq::active() {
        return;
    }
    // Splitscreen: everyone picks at once, each in their own part of the
    // window ([`super::split`]).
    if crate::splitscreen::active() {
        for (e, slot, p) in std::iter::once((player, 0, pawn)).chain(others.iter().map(|(e, s, p)| (e, s.0, p))) {
            fe.open_for(slot, "changeclass", p.team);
            commands.entity(e).insert((AwaitingClass, Dead { respawn_at: f32::INFINITY, killer: None }, Frozen));
        }
        return;
    }
    fe.open("changeclass");
    commands.entity(player).insert((AwaitingClass, Dead { respawn_at: f32::INFINITY, killer: None }, Frozen));
}

/// Menu responses, Escape opening the class menu, and the mouse.
#[allow(clippy::too_many_arguments)]
pub(super) fn update(
    mut fe: ResMut<Frontend>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut choice: ResMut<ClassChoice>,
    mut hud: ResMut<HudState>,
    players: Query<(&crate::splitscreen::LocalSlot, &crate::splitscreen::PlayerInput, Has<AwaitingClass>, Has<Dead>, Has<Loadout>)>,
    mut was_open: Local<bool>,
    mut next: ResMut<NextState<crate::state::GameState>>,
) {
    // Leave Game: back to the main menu (`crate::session` clears the match).
    if std::mem::take(&mut fe.leave) {
        fe.stack.clear();
        fe.focus = None;
        fe.clear_slot_menus();
        next.set(crate::state::GameState::Frontend);
        return;
    }
    // A match without the menus has none to open; splitscreen players have
    // their own ([`super::split`]).
    if fe.match_only || crate::splitscreen::active() {
        return;
    }
    let (awaiting, dead, equipped) = players.iter().find(|p| p.0.0 == 0).map_or((false, false, false), |p| (p.2, p.3, p.4));
    respond(&mut fe, 0, (awaiting, dead, equipped), &mut choice, &mut hud, time.elapsed_secs());
    if fe.stack.is_empty() && !fe.esc_used && !awaiting && keys.just_pressed(KeyCode::Escape) {
        fe.menu_slot = 0;
        fe.open(if crate::hq::active() { hq::PAUSE_MENU } else { "class" });
    }
    // The menus draw CoD's cursor; the game takes the mouse back after.
    let open = !fe.stack.is_empty();
    if open {
        if cursor.grab_mode != CursorGrabMode::None || cursor.visible {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = false;
        }
    } else if *was_open {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    *was_open = open;
}

/// The in-game menus' responses for a local player (`slot`; in splitscreen
/// their menus are the front end's while this runs): a team's class menu, back
/// to the match, or a class picked for their next spawn.
pub(super) fn respond(
    fe: &mut Frontend,
    slot: usize,
    (awaiting, dead, equipped): (bool, bool, bool),
    choice: &mut ClassChoice,
    hud: &mut HudState,
    now: f32,
) {
    for (menu, response) in std::mem::take(&mut fe.responses) {
        let r = response.to_ascii_lowercase();
        match r.as_str() {
            "changeclass_marines" | "changeclass_opfor" => {
                fe.close(&menu);
                fe.locals.insert("ui_team".into(), r.trim_start_matches("changeclass_").to_owned());
                fe.open("changeclass");
            }
            // Back to the match, unless there's no class to play yet.
            "back" if !awaiting => fe.close(&menu),
            "back" => {}
            _ => match r.strip_suffix(",0").and_then(|c| fe.class_loadout(c)) {
                Some(class) => {
                    info!("ui: picked {}{}", class.name, if crate::splitscreen::active() { format!(" for player {}", slot + 1) } else { String::new() });
                    if let Some(c) = choice.next.get_mut(slot) {
                        *c = Some(class);
                    }
                    fe.stack.clear();
                    fe.focus = None;
                    if equipped && !dead {
                        let text = fe.assets.localize("@MP_CHANGE_CLASS_NEXT_SPAWN");
                        hud.message(text, now);
                    }
                }
                None => debug!("ui: unhandled menu response {response:?} from {menu}"),
            },
        }
    }
}

/// Draw the HUD, and the open menus over it.
pub(super) fn paint(
    mut fe: ResMut<Frontend>,
    mut hud: ResMut<HudState>,
    mut list: ResMut<DrawList>,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    devices: Res<crate::splitscreen::LocalPlayers>,
) {
    list.0.clear();
    let ops = std::mem::take(&mut hud.ops);
    fe.emit(ops, &mut images, &mut list.0);
    if !fe.stack.is_empty() {
        super::draw_menus(&mut fe, &mut list.0, &mut images, &window);
    }
    // Splitscreen: each player's menus in their part of the window.
    if crate::splitscreen::active() {
        super::split::paint(&mut fe, &mut list, &mut images, &window, &devices);
    }
}

impl Frontend {
    /// A class by the name the class menu responds with: `customN` (from
    /// Create a Class) or one of CoD4's default classes.
    pub(super) fn class_loadout(&self, class: &str) -> Option<ClassLoadout> {
        let custom = class.strip_prefix("custom").and_then(|n| n.parse::<i32>().ok()).filter(|n| (1..=5).contains(n));
        let (base, name) = match custom {
            Some(n) => (200 + 10 * (n - 1), self.localize(&self.dvar(&format!("customclass{n}")))),
            None => {
                let i = DEFAULT_CLASSES.iter().position(|c| c.eq_ignore_ascii_case(class))? as i32;
                (200 + 10 * i, self.localize(&format!("@CLASS_CLASS{}", i + 1)))
            }
        };
        // Custom classes keep statstable indices in their stats; the default
        // classes are the class table's names.
        let item = |k: i32| -> String {
            match custom {
                Some(_) => self.table_lookup("mp/statstable.csv", 0, &self.stat(base + k).to_string(), 4),
                None => self.table_lookup("mp/classtable.csv", 1, &(base + k).to_string(), 4),
            }
        };
        let gun = |k: i32, camo: usize| -> Option<Gun> {
            let weapon = item(k);
            if weapon.is_empty() || weapon == "none" {
                return None;
            }
            let set = match custom {
                Some(_) => attachments::set_of(self, base + k),
                None => attachments::bit(&item(k + 1)),
            };
            let name = self.table_lookup("mp/statstable.csv", 4, &weapon, 3);
            let name = if name.is_empty() { weapon.to_ascii_uppercase() } else { self.localize(&format!("@{name}")) };
            // A supply drop variant: its name, and an Elite's camo unless the
            // class picks one.
            let variant = custom.and_then(|_| crate::supply::inventory().equipped(base + k, &weapon));
            Some(Gun {
                spec: format!("{weapon}:{}", attachments::names(set).join("+")),
                camo: variant.filter(|_| camo == 0).map_or(camo, |v| v.camo),
                name: variant.map_or(name.clone(), |v| format!("{name} {}", v.name)),
                variant: variant.map(|v| v.id.clone()),
            })
        };
        let camo = custom.map_or(0, |_| self.class_camo(base + 1).max(0) as usize);
        let secondary_camo = custom.map_or(0, |_| self.class_camo(base + 3).max(0) as usize);
        let guns: Vec<Gun> = [gun(1, camo), gun(3, secondary_camo)].into_iter().flatten().collect();
        let perks = (5..=7).map(item).filter(|p| p.starts_with("specialty_") && p != "specialty_null").collect();
        let special = Some(item(8)).filter(|g| g.ends_with("_grenade"));
        // C4, claymores or an RPG-7 in the perk-1 slot (as `_class.gsc`
        // turns the tables' `specialty_weapon_*` into weapons).
        let inventory = match item(5).as_str() {
            "specialty_weapon_c4" | "c4_mp" => Some("c4_mp".to_owned()),
            "specialty_weapon_claymore" | "claymore_mp" => Some("claymore_mp".to_owned()),
            "specialty_weapon_rpg" | "rpg_mp" => Some("rpg_mp".to_owned()),
            _ => None,
        };
        let inventory_camo = custom.map_or(0, |_| self.class_camo(base + 5).max(0) as usize);
        (!guns.is_empty()).then_some(ClassLoadout { name, guns, perks, special, inventory, inventory_camo })
    }
}
