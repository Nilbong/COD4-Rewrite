//! CoD4's original Rank & Challenges shell, rows and rank-panel artwork.
//!
//! These templates come directly from `menu_challenges` in the installed
//! `ui_mp.ff`; fonts, text styles, colours and materials remain native.

use crate::ui::Frontend;
use iw3::menu::{Item, ItemData, Menu, Rect as VRect, Statement, Token, flags, item_type, op};

fn text(value: &str) -> Statement {
    vec![Token::Op(op::LEFTPAREN), Token::Str(value.into())]
}

fn command(value: &str) -> String {
    format!("\"play\" \"mouse_click\" ; \"uiScript\" {value} ;")
}

fn highlight(id: i32) -> Statement {
    vec![
        Token::Op(op::LEFTPAREN),
        Token::Op(op::LOCALVARINT),
        Token::Str("ui_highlight".into()),
        Token::Op(op::RIGHTPAREN),
        Token::Op(op::EQUALS),
        Token::Int(id),
    ]
}

/// Original blurred soldier backdrop, gold heading, footer and Back button.
pub(super) fn screen(fe: &Frontend, name: &str, title: &str) -> Menu {
    let mut menu = fe.assets.menu("menu_challenges").map_or_else(Menu::default, |source| {
        let mut menu = source.as_ref().clone();
        menu.items = source.items.iter().take(15).cloned().collect();
        menu
    });
    let back = if name == "combat_emblem" { "combatCancelEmblem" } else { "combatBack" };
    menu.window.name = name.into();
    menu.full_screen = true;
    menu.on_open.clear();
    menu.on_close.clear();
    menu.on_key.clear();
    menu.on_esc = command(back);
    menu.visible_exp.clear();
    for item in &mut menu.items {
        if item
            .text_exp
            .iter()
            .any(|token| matches!(token, Token::Str(key) if key.eq_ignore_ascii_case("@MENU_RANK_AND_CHALLENGES_CAP")))
        {
            item.text.clear();
            item.text_exp = text(title);
        }
        if item.window.name.eq_ignore_ascii_case("back") {
            item.action = command(back);
        }
    }
    menu
}

/// Original 22-high menu row, including its angled highlight end.
pub(super) fn row(fe: &Frontend, r: VRect, id: i32, label: &str, action: &str, selected: bool) -> Vec<Item> {
    let Some(source) = fe.assets.menu("menu_challenges") else { return Vec::new() };
    // The first unlocked tier button follows its two resting-background and
    // two focus-highlight pictures. Its locked duplicate must stay excluded.
    let Some(button) = source.items.iter().position(|item| {
        let rect = item.window.rect;
        item.ty == item_type::BUTTON
            && rect.horz_align == 1
            && (rect.y - 34.0).abs() < 0.5
            && (rect.w - 220.0).abs() < 0.5
    }) else {
        return Vec::new();
    };
    let Some(start) = button.checked_sub(4) else { return Vec::new() };
    let end_width = 5.5 * r.h / 22.0;
    source.items[start..=button]
        .iter()
        .cloned()
        .map(|mut item| {
            let original = item.window.rect;
            let is_highlight = item.visible_exp.iter().any(|token| matches!(token, Token::Str(key) if key.eq_ignore_ascii_case("ui_highlight")));
            item.window.rect = if item.ty == item_type::BUTTON {
                r
            } else if original.x > 200.0 {
                VRect { x: r.x + r.w - end_width, w: end_width, ..r }
            } else {
                VRect { w: (r.w - end_width).max(0.0), ..r }
            };
            item.window.name.clear();
            item.window.owner_draw = 0;
            item.window.owner_draw_flags = 0;
            item.window.dynamic_flags = flags::VISIBLE;
            item.dvar.clear();
            item.dvar_test.clear();
            item.enable_dvar.clear();
            item.dvar_flags = 0;
            item.rect_x_exp.clear();
            item.rect_y_exp.clear();
            item.rect_w_exp.clear();
            item.rect_h_exp.clear();
            item.forecolor_a_exp.clear();
            item.visible_exp = if is_highlight && !selected { highlight(id) } else { Vec::new() };
            item.mouse_enter.clear();
            item.mouse_exit.clear();
            item.mouse_enter_text.clear();
            item.mouse_exit_text.clear();
            item.leave_focus.clear();
            item.on_accept.clear();
            item.on_key.clear();
            if item.ty == item_type::BUTTON {
                item.window.name = format!("combat_row_{id}");
                item.text = label.into();
                item.text_exp.clear();
                item.action = command(action);
                item.on_focus = format!(
                    "\"play\" \"mouse_over\" ; \"setLocalVarInt\" \"ui_highlight\" {id} ; \"setLocalVarString\" \"ui_choicegroup\" \"\" ;"
                );
                item.data = ItemData::None;
                if selected {
                    item.window.fore_color = source.focus_color;
                }
            } else {
                item.window.static_flags |= flags::DECORATION;
            }
            item
        })
        .collect()
}

/// Original rank pane's centre gradient, illuminated edges and grey caps.
/// `r` covers the original (-354, 34, 278, 358) virtual rectangle.
pub(super) fn panel(fe: &Frontend, r: VRect) -> Vec<Item> {
    let Some(source) = fe.assets.menu("menu_challenges") else { return Vec::new() };
    let Some(items) = source.items.get(137..147) else { return Vec::new() };
    let sx = r.w / 278.0;
    let sy = r.h / 358.0;
    items
        .iter()
        .cloned()
        .map(|mut item| {
            let original = item.window.rect;
            item.window.rect = VRect {
                x: r.x + (original.x + 354.0) * sx,
                y: r.y + (original.y - 34.0) * sy,
                w: original.w * sx,
                h: original.h * sy,
                horz_align: r.horz_align,
                vert_align: r.vert_align,
            };
            item.window.owner_draw = 0;
            item.window.owner_draw_flags = 0;
            item.window.static_flags |= flags::DECORATION;
            item.window.dynamic_flags = flags::VISIBLE;
            item.visible_exp.clear();
            item.rect_x_exp.clear();
            item.rect_y_exp.clear();
            item.rect_w_exp.clear();
            item.rect_h_exp.clear();
            item
        })
        .collect()
}
