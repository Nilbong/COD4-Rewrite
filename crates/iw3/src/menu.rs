//! The data behind the game's UI: menus (`menuDef_t` / `itemDef_s`), fonts,
//! localized strings and string tables, read from generically loaded zone
//! assets into plain structs.
//!
//! Menu logic is compiled into the zone two ways: *scripts* (`action`,
//! `onOpen`, ...) are kept as command strings, and *expressions*
//! (`visible when(...)`, `exp text(...)`) are infix token lists, see
//! [`Token`] and [`op`].

use crate::zone::generic::GNode;
use crate::zone::{Asset, AssetType, Zone};

/// `rectDef_s`: a rectangle in the 640x480 virtual screen, placed by its
/// alignment modes (`HORIZONTAL_ALIGN_*` / `VERTICAL_ALIGN_*`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub horz_align: u8,
    pub vert_align: u8,
}

/// `windowDef_t`, shared by menus and items.
#[derive(Clone, Debug, Default)]
pub struct Window {
    pub name: String,
    pub rect: Rect,
    pub group: String,
    /// `WINDOW_STYLE_*`: 0 empty, 1 filled, 2 gradient, 3 shader, ...
    pub style: i32,
    pub border: i32,
    pub border_size: f32,
    pub owner_draw: i32,
    pub owner_draw_flags: u32,
    /// `WINDOW_FLAG_*` static flags, e.g. [`flags::DECORATION`].
    pub static_flags: u32,
    /// Initial dynamic flags, e.g. [`flags::VISIBLE`].
    pub dynamic_flags: u32,
    pub fore_color: [f32; 4],
    pub back_color: [f32; 4],
    pub border_color: [f32; 4],
    pub outline_color: [f32; 4],
    /// Background material name.
    pub background: Option<String>,
}

pub mod flags {
    pub const VISIBLE: u32 = 0x4;
    pub const DECORATION: u32 = 0x100000;
    pub const AUTO_WRAPPED: u32 = 0x800000;
    pub const POPUP: u32 = 0x1000000;
}

/// `ItemDefType`.
pub mod item_type {
    pub const TEXT: i32 = 0;
    pub const BUTTON: i32 = 1;
    pub const RADIOBUTTON: i32 = 2;
    pub const CHECKBOX: i32 = 3;
    pub const EDITFIELD: i32 = 4;
    pub const COMBO: i32 = 5;
    pub const LISTBOX: i32 = 6;
    pub const MODEL: i32 = 7;
    pub const OWNERDRAW: i32 = 8;
    pub const NUMERICFIELD: i32 = 9;
    pub const SLIDER: i32 = 10;
    pub const YESNO: i32 = 11;
    pub const MULTI: i32 = 12;
    pub const DVARENUM: i32 = 13;
    pub const BIND: i32 = 14;
}

/// `operationEnum`: operator and function tokens in expressions.
pub mod op {
    pub const RIGHTPAREN: u8 = 1;
    pub const MULTIPLY: u8 = 2;
    pub const DIVIDE: u8 = 3;
    pub const MODULUS: u8 = 4;
    pub const ADD: u8 = 5;
    pub const SUBTRACT: u8 = 6;
    pub const NOT: u8 = 7;
    pub const LESSTHAN: u8 = 8;
    pub const LESSTHANEQUALTO: u8 = 9;
    pub const GREATERTHAN: u8 = 10;
    pub const GREATERTHANEQUALTO: u8 = 11;
    pub const EQUALS: u8 = 12;
    pub const NOTEQUAL: u8 = 13;
    pub const AND: u8 = 14;
    pub const OR: u8 = 15;
    pub const LEFTPAREN: u8 = 16;
    pub const COMMA: u8 = 17;
    pub const BITWISEAND: u8 = 18;
    pub const BITWISEOR: u8 = 19;
    pub const BITWISENOT: u8 = 20;
    pub const BITSHIFTLEFT: u8 = 21;
    pub const BITSHIFTRIGHT: u8 = 22;
    /// Every token from here on is a function call, closed by `RIGHTPAREN`.
    pub const FIRST_FUNCTION: u8 = 23;
    pub const SIN: u8 = 23;
    pub const COS: u8 = 24;
    pub const MIN: u8 = 25;
    pub const MAX: u8 = 26;
    pub const MILLISECONDS: u8 = 27;
    pub const DVARINT: u8 = 28;
    pub const DVARBOOL: u8 = 29;
    pub const DVARFLOAT: u8 = 30;
    pub const DVARSTRING: u8 = 31;
    pub const STAT: u8 = 32;
    pub const UIACTIVE: u8 = 33;
    pub const FLASHBANGED: u8 = 34;
    pub const SCOPED: u8 = 35;
    pub const SCOREBOARDVISIBLE: u8 = 36;
    pub const INKILLCAM: u8 = 37;
    /// `player("score")`: a field of the player.
    pub const PLAYERFIELD: u8 = 38;
    pub const SELECTINGLOCATION: u8 = 39;
    /// `team("name")`: the player's team, `TEAM_ALLIES` and so on;
    /// `team("score")` its score.
    pub const TEAM: u8 = 40;
    pub const OTHERTEAM: u8 = 41;
    pub const MARINES: u8 = 42;
    pub const OPFOR: u8 = 43;
    pub const MENUISOPEN: u8 = 44;
    pub const INLOBBY: u8 = 46;
    pub const INPRIVATEPARTY: u8 = 47;
    pub const PRIVATEPARTYHOST: u8 = 48;
    pub const SECONDSASTIME: u8 = 55;
    pub const TABLELOOKUP: u8 = 56;
    pub const LOCALIZESTRING: u8 = 57;
    pub const LOCALVARINT: u8 = 58;
    pub const LOCALVARBOOL: u8 = 59;
    pub const LOCALVARFLOAT: u8 = 60;
    pub const LOCALVARSTRING: u8 = 61;
    /// Seconds left in the match.
    pub const TIMELEFT: u8 = 62;
    pub const SECONDSASCOUNTDOWN: u8 = 63;
    pub const GAMEMSGWNDACTIVE: u8 = 64;
    pub const TOINT: u8 = 65;
    pub const TOSTRING: u8 = 66;
    pub const TOFLOAT: u8 = 67;
    pub const GAMETYPENAME: u8 = 68;
    pub const GAMETYPE: u8 = 69;
    pub const GAMETYPEDESCRIPTION: u8 = 70;
    /// `score(n)`: the score of the player ranked n.
    pub const SCORE: u8 = 71;
    pub const FOLLOWING: u8 = 73;
    pub const STATRANGEBITSSET: u8 = 74;
    pub const KEYBINDING: u8 = 75;
    pub const MAXPLAYERS: u8 = 78;
    pub const ISINTERMISSION: u8 = 80;
}

/// One expression token.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Op(u8),
    Int(i32),
    Float(f32),
    Str(String),
}

/// A compiled expression (`statement_s`); empty when the item has none.
pub type Statement = Vec<Token>;

#[derive(Clone, Debug)]
pub struct KeyHandler {
    pub key: i32,
    pub action: String,
}

#[derive(Clone, Debug, Default)]
pub struct Menu {
    pub window: Window,
    pub font: String,
    pub full_screen: bool,
    pub blur_radius: f32,
    pub on_open: String,
    pub on_close: String,
    pub on_esc: String,
    pub on_key: Vec<KeyHandler>,
    pub visible_exp: Statement,
    pub sound_name: String,
    pub focus_color: [f32; 4],
    pub disable_color: [f32; 4],
    pub rect_x_exp: Statement,
    pub rect_y_exp: Statement,
    pub items: Vec<Item>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Column {
    pub pos: i32,
    pub width: i32,
    pub max_chars: i32,
    pub alignment: i32,
}

#[derive(Clone, Debug, Default)]
pub struct ListBox {
    pub element_width: f32,
    pub element_height: f32,
    pub element_style: i32,
    pub columns: Vec<Column>,
    pub on_double_click: String,
    pub not_selectable: bool,
    pub no_scroll_bars: bool,
    pub select_border: [f32; 4],
    pub disable_color: [f32; 4],
    pub select_icon: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct EditField {
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub max_chars: i32,
    pub max_paint_chars: i32,
}

/// `multiDef_s`: a list of (label, value) choices for a dvar.
#[derive(Clone, Debug, Default)]
pub struct Multi {
    pub labels: Vec<String>,
    /// String values when `str_def`, otherwise use `values`.
    pub strings: Vec<String>,
    pub values: Vec<f32>,
    pub str_def: bool,
}

#[derive(Clone, Debug, Default)]
pub enum ItemData {
    #[default]
    None,
    ListBox(ListBox),
    EditField(EditField),
    Multi(Multi),
    EnumDvar(String),
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub window: Window,
    pub ty: i32,
    pub data_type: i32,
    pub font_enum: i32,
    /// `ITEM_ALIGN_*`: bits 0-1 horizontal, bits 2-3 vertical.
    pub text_align_mode: i32,
    pub text_align_x: f32,
    pub text_align_y: f32,
    pub text_scale: f32,
    /// `ITEM_TEXTSTYLE_*`.
    pub text_style: i32,
    pub text: String,
    pub mouse_enter_text: String,
    pub mouse_exit_text: String,
    pub mouse_enter: String,
    pub mouse_exit: String,
    pub action: String,
    pub on_accept: String,
    pub on_focus: String,
    pub leave_focus: String,
    pub dvar: String,
    pub dvar_test: String,
    pub enable_dvar: String,
    pub dvar_flags: i32,
    pub on_key: Vec<KeyHandler>,
    /// Feeder id for list boxes and owner draws.
    pub special: f32,
    pub data: ItemData,
    pub visible_exp: Statement,
    pub text_exp: Statement,
    pub material_exp: Statement,
    pub rect_x_exp: Statement,
    pub rect_y_exp: Statement,
    pub rect_w_exp: Statement,
    pub rect_h_exp: Statement,
    pub forecolor_a_exp: Statement,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Glyph {
    pub letter: u16,
    pub x0: i8,
    pub y0: i8,
    pub dx: u8,
    pub pixel_width: u8,
    pub pixel_height: u8,
    pub s0: f32,
    pub t0: f32,
    pub s1: f32,
    pub t1: f32,
}

#[derive(Clone, Debug)]
pub struct Font {
    pub name: String,
    pub pixel_height: i32,
    pub material: String,
    pub glow_material: String,
    pub glyphs: Vec<Glyph>,
}

#[derive(Clone, Debug)]
pub struct StringTable {
    pub name: String,
    pub columns: usize,
    pub rows: usize,
    pub values: Vec<String>,
}

impl StringTable {
    pub fn get(&self, row: usize, col: usize) -> Option<&str> {
        if col >= self.columns {
            return None;
        }
        self.values.get(row * self.columns + col).map(String::as_str)
    }
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
    n.asset(f).map(|id| zone.get(id).name().trim_start_matches(',').to_owned())
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

impl Menu {
    pub fn from_node(zone: &Zone, m: &GNode) -> Menu {
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
}

impl Font {
    pub fn from_node(zone: &Zone, f: &GNode) -> Font {
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

    pub fn glyph(&self, letter: char) -> Option<&Glyph> {
        let l = letter as u32;
        self.glyphs.iter().find(|g| g.letter as u32 == l)
    }
}

/// Everything UI-related in a zone.
#[derive(Default)]
pub struct UiData {
    pub menus: Vec<Menu>,
    pub fonts: Vec<Font>,
    /// (`MENU_QUIT`, `"Quit"`): looked up with `@MENU_QUIT`.
    pub strings: Vec<(String, String)>,
    pub tables: Vec<StringTable>,
}

impl UiData {
    pub fn from_zone(zone: &Zone) -> UiData {
        let mut out = UiData::default();
        for asset in &zone.assets {
            let Asset::Generic(g) = asset else { continue };
            let r = &g.root;
            match g.ty {
                AssetType::Menu => out.menus.push(Menu::from_node(zone, r)),
                AssetType::Font => out.fonts.push(Font::from_node(zone, r)),
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

    /// Just the zone's menus, and of those only the ones `wanted` accepts by
    /// name (the rest aren't parsed): much cheaper than [`UiData::from_zone`]
    /// for adding a match's menus to the front end's.
    pub fn menus_from_zone(zone: &Zone, mut wanted: impl FnMut(&str) -> bool) -> Vec<Menu> {
        zone.assets
            .iter()
            .filter_map(|a| match a {
                Asset::Generic(g) if g.ty == AssetType::Menu && wanted(&g.name) => Some(Menu::from_node(zone, &g.root)),
                _ => None,
            })
            .collect()
    }
}
