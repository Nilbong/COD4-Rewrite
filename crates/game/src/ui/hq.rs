//! Headquarters' Escape menu ([`crate::hq`]): CoD4's in-game `class` menu
//! made over. "Choose Class" opens Create a Class, "Change Team" Supply
//! Drops, Leave Game reads "Leave Headquarters", and the minimap, the map's
//! name, Call Vote and Mute Players go (the rows below closing up).

use iw3::menu::{Item, Menu, Statement, Token, item_type, op};

pub(super) const PAUSE_MENU: &str = "hq_pause";

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
