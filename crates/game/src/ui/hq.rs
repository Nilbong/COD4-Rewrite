//! Headquarters' Escape menu ([`crate::hq`]): CoD4's in-game `class` menu
//! made over. "Choose Class" opens Create a Class, "Change Team" Supply
//! Drops, Leave Game reads "Leave Headquarters", and the minimap, the map's
//! name, Call Vote and Mute Players go (the rows below closing up).

use iw3::menu::{Item, Menu, Statement, Token, item_type, op};

pub(super) const PAUSE_MENU: &str = "hq_pause";

impl super::Frontend {
    /// A Headquarters station's menu ([`crate::hq::STATIONS`]).
    pub fn run_station(&mut self, script: &str) {
        self.menu_slot = 0;
        self.run(script, PAUSE_MENU);
    }
}

fn str_exp(s: &str) -> Statement {
    vec![Token::Op(op::LEFTPAREN), Token::Str(s.into())]
}

fn label(it: &Item) -> String {
    match it.text_exp.as_slice() {
        [_, Token::Str(s)] => s.to_ascii_uppercase(),
        _ => it.text.to_ascii_uppercase(),
    }
}

/// The left column's rows (buttons and their bars).
fn left(it: &Item) -> bool {
    it.window.rect.x < -200.0 && it.window.rect.horz_align == 2
}

/// A match's Escape menu (`class`) without Call Vote and Mute Players: there
/// are no votes or voice to mute, and both led nowhere. The rows under them
/// move up.
pub(super) fn match_pause_menu(class: &Menu) -> Option<Menu> {
    let mut out = class.clone();
    let row_y = |label_name: &str| out.items.iter().find(|it| it.ty == item_type::BUTTON && label(it) == label_name).map(|it| it.window.rect.y);
    let (vote, mute) = (row_y("@MPUI_CALL_VOTE")?, row_y("@MPUI_MUTE_PLAYERS")?);
    let at = |it: &Item, y: f32| left(it) && (it.window.rect.y - y).abs() < 0.5;
    out.items.retain(|it| !(at(it, vote) || at(it, mute)));
    // Two rows out: the next row takes the place after the last one kept
    // (Call Vote's row also had a gap above it).
    let pitch = mute - vote;
    let kept = out.items.iter().filter(|it| it.ty == item_type::BUTTON && left(it) && it.window.rect.y < vote).map(|it| it.window.rect.y).fold(f32::MIN, f32::max);
    let shift = if kept > f32::MIN { mute + pitch - (kept + pitch) } else { 2.0 * pitch };
    for it in out.items.iter_mut() {
        if left(it) && it.window.rect.y > mute {
            it.window.rect.y -= shift;
        }
    }
    Some(out)
}

/// CoD4's team menu without Spectator (there's no spectating yet).
pub(super) fn team_menu(team: &Menu) -> Menu {
    let mut out = team.clone();
    out.items.retain(|it| !(it.ty == item_type::BUTTON && it.action.contains("\"spectator\"")));
    out
}

pub(super) fn pause_menu(class: &Menu) -> Option<Menu> {
    let mut out = class.clone();
    let row_y = |label_name: &str| out.items.iter().find(|it| it.ty == item_type::BUTTON && label(it) == label_name).map(|it| it.window.rect.y);
    let (vote, mute, leave) = (row_y("@MPUI_CALL_VOTE")?, row_y("@MPUI_MUTE_PLAYERS")?, row_y("@MENU_LEAVE_GAME")?);
    let at = |it: &Item, y: f32| left(it) && (it.window.rect.y - y).abs() < 0.5;
    // No minimap or map name (the level isn't a multiplayer map), and no
    // voting or muting alone.
    out.items.retain(|it| {
        let compass = it.window.name.starts_with("mini_map") || it.window.name.starts_with("compass");
        let map_name = it.text_exp.iter().any(|t| matches!(t, Token::Str(s) if s.contains("mapsTable")));
        // The minimap's frame and header bars, top right.
        let r = &it.window.rect;
        let frame = r.horz_align == 3 && (110.0..=380.0).contains(&r.y);
        !(compass || map_name || frame || at(it, vote) || at(it, mute))
    });
    // The game type's name and description (`gametypename`,
    // `gametypedescription`) say what Headquarters is instead.
    for it in out.items.iter_mut() {
        match it.text_exp.as_slice() {
            [_, Token::Op(op::GAMETYPENAME), ..] => it.text_exp = str_exp("Headquarters"),
            [_, Token::Op(op::GAMETYPEDESCRIPTION), ..] => it.text_exp = str_exp("Make classes, open supply drops, and get ready for the next game."),
            _ => {}
        }
    }
    let mut chose = false;
    out.items.retain_mut(|it| {
        if left(it) && it.window.rect.y > mute {
            it.window.rect.y -= leave - vote;
        }
        if it.ty != item_type::BUTTON {
            return true;
        }
        match label(it).as_str() {
            // One of the two (one per team).
            "@MPUI_CHOOSE_CLASS" if chose => return false,
            "@MPUI_CHOOSE_CLASS" => {
                chose = true;
                it.text_exp = str_exp("Create a Class");
                it.visible_exp.clear();
                it.action = "\"play\" \"mouse_click\" ; \"close\" \"self\" ; \"open\" \"pc_cac_popup\" ; ".into();
            }
            "@MPUI_CHANGE_TEAM" => {
                it.text_exp = str_exp("Supply Drops");
                it.action = "\"play\" \"mouse_click\" ; \"close\" \"self\" ; \"open\" \"supply_drops\" ; ".into();
            }
            "@MENU_LEAVE_GAME" => {
                it.text_exp = str_exp("Leave Headquarters");
                it.visible_exp.clear();
                it.action = "\"play\" \"mouse_click\" ; \"close\" \"self\" ; \"exec\" \"disconnect\" ; ".into();
            }
            _ => {}
        }
        true
    });
    // Two Leave Game buttons (by `sv_running`) are now both shown: keep one.
    let mut left_seen = false;
    out.items.retain(|it| {
        if it.ty == item_type::BUTTON && matches!(it.text_exp.as_slice(), [_, Token::Str(s)] if s == "Leave Headquarters") {
            if left_seen {
                return false;
            }
            left_seen = true;
        }
        true
    });
    Some(out)
}
