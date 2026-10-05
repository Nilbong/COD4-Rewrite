//! World at War's menus, fonts, localized strings and string tables as the
//! game's own UI types ([`iw3::menu`]), so the menu interpreter that runs
//! CoD4's `ui_mp` can run World at War's unchanged.
//!
//! The structures are CoD4's with a few additions (`menuDef_t::onFocus`,
//! `itemDef_s::onListboxSelectionChange`, list box focus colours and
//! textures), which have no place in the CoD4 types and are left out.
//! Expression opcodes match CoD4's up to `OP_ISINTERMISSION`'s old slot (82);
//! World at War appends new functions after it (`OP_GAMEHOST`, ...).
//!
//! The multiplayer frontend is spread over these zones, see [`MP_ZONES`]:
//! the menus are in `ui_mp` (main menu, Create a Class, ...), `common_mp`
//! (in-game menus) and `code_post_gfx_mp`, with fixes in `patch_mp`; the
//! fonts and most strings are in `localized_code_post_gfx_mp`.

use crate::zone::{AssetType, GNode, Zone};
use iw3::menu::*;

/// The multiplayer UI zones, in load order: a later zone's menu replaces an
/// earlier one of the same name (`patch_mp` carries fixed menus).
pub const MP_ZONES: [&str; 6] =
    ["code_post_gfx_mp", "localized_code_post_gfx_mp", "ui_mp", "common_mp", "localized_common_mp", "patch_mp"];

/// Menus, fonts, strings and tables of a parsed World at War zone. Material
/// names are the zone's (a leading `,` marks another zone's copy, and is
/// dropped); their images come from [`crate::load_iw3`] and the iwds.
pub fn ui_data(zone: &Zone) -> UiData {
    let mut out = UiData::default();
    for a in &zone.assets {
        let r = &a.root;
        match a.ty {
            AssetType::Menu => out.menus.push(menu(zone, r)),
            AssetType::Font => out.fonts.push(font(zone, r)),
            AssetType::LocalizeEntry => out.strings.push((s(r, "name"), s(r, "value"))),
            AssetType::StringTable => {
                let columns = r.int("columnCount").max(0) as usize;
                let rows = r.int("rowCount").max(0) as usize;
                let values = r.strings("values").into_iter().map(|v| v.unwrap_or("").to_owned()).collect();
                out.tables.push(StringTable { name: s(r, "name"), columns, rows, values });
            }
            _ => {}
        }
    }
    out
}

fn s(n: &GNode, f: &str) -> String {
    n.string(f).unwrap_or("").to_owned()
}

fn color(n: &GNode, f: &str) -> [f32; 4] {
    std::array::from_fn(|i| n.float(&format!("{f}[{i}]")))
}

fn rect(n: &GNode, p: &str) -> Rect {
    Rect {
        x: n.float(&format!("{p}::x")),
        y: n.float(&format!("{p}::y")),
        w: n.float(&format!("{p}::w")),
        h: n.float(&format!("{p}::h")),
        horz_align: n.int(&format!("{p}::horzAlign")) as u8,
        vert_align: n.int(&format!("{p}::vertAlign")) as u8,
    }
}

fn material(zone: &Zone, n: &GNode, f: &str) -> Option<String> {
    n.asset(f).map(|id| zone.assets[id].name.trim_start_matches(',').to_owned())
}

fn window(zone: &Zone, w: Option<&GNode>) -> Window {
    let Some(w) = w else { return Window::default() };
    Window {
        name: s(w, "name"),
        rect: rect(w, "rect"),
        group: s(w, "group"),
        style: w.int("style") as i32,
        border: w.int("border") as i32,
        border_size: w.float("borderSize"),
        owner_draw: w.int("ownerDraw") as i32,
        owner_draw_flags: w.int("ownerDrawFlags") as u32,
        static_flags: w.int("staticFlags") as u32,
        dynamic_flags: w.int("dynamicFlags[0]") as u32,
        fore_color: color(w, "foreColor"),
        back_color: color(w, "backColor"),
        border_color: color(w, "borderColor"),
        outline_color: color(w, "outlineColor"),
        background: material(zone, w, "background"),
    }
}

fn statement(n: &GNode, f: &str) -> Statement {
    let Some(st) = n.node(f) else { return Vec::new() };
    st.nodes("entries")
        .iter()
        .map(|e| {
            if e.int("type") == 0 {
                return Token::Op(e.int("data::op") as u8);
            }
            match e.int("data::operand::dataType") {
                0 => Token::Int(e.int("data::operand::internals::intVal") as i32),
                1 => Token::Float(e.float("data::operand::internals::floatVal")),
                _ => Token::Str(
                    e.node("data")
                        .and_then(|d| d.node("operand"))
                        .and_then(|o| o.node("internals"))
                        .and_then(|i| i.string("stringVal"))
                        .unwrap_or("")
                        .to_owned(),
                ),
            }
        })
        .collect()
}

fn key_handlers(n: &GNode) -> Vec<KeyHandler> {
    let mut out = Vec::new();
    let mut k = n.node("onKey");
    while let Some(h) = k {
        out.push(KeyHandler { key: h.int("key") as i32, action: s(h, "action") });
        k = h.node("next");
    }
    out
}

fn item(zone: &Zone, it: &GNode) -> Item {
    let ty = it.int("type") as i32;
    let td = it.node("typeData");
    let data = match ty {
        item_type::LISTBOX => td.and_then(|t| t.node("listBox")).map(|l| {
            let columns = (0..l.int("numColumns").clamp(0, 16))
                .map(|i| Column {
                    pos: l.int(&format!("columnInfo[{i}]::pos")) as i32,
                    width: l.int(&format!("columnInfo[{i}]::width")) as i32,
                    max_chars: l.int(&format!("columnInfo[{i}]::maxChars")) as i32,
                    alignment: l.int(&format!("columnInfo[{i}]::alignment")) as i32,
                })
                .collect();
            ItemData::ListBox(ListBox {
                element_width: l.float("elementWidth"),
                element_height: l.float("elementHeight"),
                element_style: l.int("elementStyle") as i32,
                columns,
                on_double_click: s(l, "onDoubleClick"),
                not_selectable: l.int("notselectable") != 0,
                no_scroll_bars: l.int("noScrollBars") != 0,
                select_border: color(l, "selectBorder"),
                disable_color: color(l, "disableColor"),
                select_icon: material(zone, l, "selectIcon"),
            })
        }),
        item_type::MULTI => td.and_then(|t| t.node("multi")).map(|m| {
            let count = m.int("count").clamp(0, 32) as usize;
            let strs = |f: &str| m.strings(f).into_iter().take(count).map(|s| s.unwrap_or("").to_owned()).collect();
            ItemData::Multi(Multi {
                labels: strs("dvarList"),
                strings: strs("dvarStr"),
                values: (0..count).map(|i| m.float(&format!("dvarValue[{i}]"))).collect(),
                str_def: m.int("strDef") != 0,
            })
        }),
        item_type::DVARENUM => td.and_then(|t| t.string("enumDvarName")).map(|n| ItemData::EnumDvar(n.to_owned())),
        _ => td.and_then(|t| t.node("editField")).map(|e| {
            ItemData::EditField(EditField {
                min: e.float("minVal"),
                max: e.float("maxVal"),
                default: e.float("defVal"),
                max_chars: e.int("maxChars") as i32,
                max_paint_chars: e.int("maxPaintChars") as i32,
            })
        }),
    }
    .unwrap_or_default();
    Item {
        window: window(zone, it.node("window")),
        ty,
        data_type: it.int("dataType") as i32,
        font_enum: it.int("fontEnum") as i32,
        text_align_mode: it.int("textAlignMode") as i32,
        text_align_x: it.float("textalignx"),
        text_align_y: it.float("textaligny"),
        text_scale: it.float("textscale"),
        text_style: it.int("textStyle") as i32,
        text: s(it, "text"),
        mouse_enter_text: s(it, "mouseEnterText"),
        mouse_exit_text: s(it, "mouseExitText"),
        mouse_enter: s(it, "mouseEnter"),
        mouse_exit: s(it, "mouseExit"),
        action: s(it, "action"),
        on_accept: s(it, "onAccept"),
        on_focus: s(it, "onFocus"),
        leave_focus: s(it, "leaveFocus"),
        dvar: s(it, "dvar"),
        dvar_test: s(it, "dvarTest"),
        enable_dvar: s(it, "enableDvar"),
        dvar_flags: it.int("dvarFlags") as i32,
        on_key: key_handlers(it),
        special: it.float("special"),
        data,
        visible_exp: statement(it, "visibleExp"),
        text_exp: statement(it, "textExp"),
        material_exp: statement(it, "materialExp"),
        rect_x_exp: statement(it, "rectXExp"),
        rect_y_exp: statement(it, "rectYExp"),
        rect_w_exp: statement(it, "rectWExp"),
        rect_h_exp: statement(it, "rectHExp"),
        forecolor_a_exp: statement(it, "forecolorAExp"),
    }
}

fn menu(zone: &Zone, m: &GNode) -> Menu {
    Menu {
        window: window(zone, m.node("window")),
        font: s(m, "font"),
        full_screen: m.int("fullScreen") != 0,
        blur_radius: m.float("blurRadius"),
        on_open: s(m, "onOpen"),
        on_close: s(m, "onClose"),
        on_esc: s(m, "onESC"),
        on_key: key_handlers(m),
        visible_exp: statement(m, "visibleExp"),
        sound_name: s(m, "soundName"),
        focus_color: color(m, "focusColor"),
        disable_color: color(m, "disableColor"),
        rect_x_exp: statement(m, "rectXExp"),
        rect_y_exp: statement(m, "rectYExp"),
        items: m.nodes("items").iter().map(|it| item(zone, it)).collect(),
    }
}

fn font(zone: &Zone, f: &GNode) -> Font {
    Font {
        name: s(f, "fontName"),
        pixel_height: f.int("pixelHeight") as i32,
        material: material(zone, f, "material").unwrap_or_default(),
        glow_material: material(zone, f, "glowMaterial").unwrap_or_default(),
        glyphs: f
            .nodes("glyphs")
            .iter()
            .map(|g| Glyph {
                letter: g.int("letter") as u16,
                x0: g.int("x0") as i8,
                y0: g.int("y0") as i8,
                dx: g.int("dx") as u8,
                pixel_width: g.int("pixelWidth") as u8,
                pixel_height: g.int("pixelHeight") as u8,
                s0: g.float("s0"),
                t0: g.float("t0"),
                s1: g.float("s1"),
                t1: g.float("t1"),
            })
            .collect(),
    }
}
