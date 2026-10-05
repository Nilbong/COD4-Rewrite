//! Several attachments on one weapon (CoD4 allows one): a sight (red dot or
//! ACOG), a silencer and a grip can be combined. The grenade launcher is a
//! different weapon model, so it stays on its own, and so does a second
//! sight or underbarrel. Black Ops guns take one attachment per slot (top,
//! bottom, muzzle, trigger; see [`super::bo1::toggle`]), World at War guns
//! one in all ([`super::waw::toggle`]).
//!
//! CoD4's own stat (`stat(weapon + 1)`, one attachment) keeps the set's main
//! attachment, so the original menus and Perk 1 rules keep working; the full
//! set is a bit mask (`mp/attachmenttable.csv` column 10) in stat
//! [`SET_STATS`]` + weapon`.

use super::expr::Env;
use super::script::{self, Tok};
use iw3::menu::{Menu, Statement, Token, item_type, op};

/// The set for class weapon stat N (201, 203, ...) lives in stat SET_STATS + N.
pub const SET_STATS: i32 = 4000;
pub const TABLE: &str = "mp/attachmenttable.csv";

const ACOG: i32 = 2;
const REFLEX: i32 = 4;
const SILENCER: i32 = 8;
const GRIP: i32 = 16;
const GL: i32 = 32;
const SIGHTS: i32 = ACOG | REFLEX;
const UNDERBARREL: i32 = GRIP | GL;

/// By priority for the main attachment: the one CoD's own stat holds. Black
/// Ops and World at War guns' attachments follow (see [`super::bo1`],
/// [`super::waw`]); their sets are kept whole, so CoD's stat holds none of
/// them.
const BY_PRIORITY: [(&str, i32); 27] = [
    ("gl", GL),
    ("acog", ACOG),
    ("reflex", REFLEX),
    ("grip", GRIP),
    ("silencer", SILENCER),
    ("elbit", 1 << 6),
    ("lps", 1 << 7),
    ("vzoom", 1 << 8),
    ("ir", 1 << 9),
    ("upgradesight", 1 << 10),
    ("snub", 1 << 11),
    ("mk", 1 << 12),
    ("ft", 1 << 13),
    ("extclip", 1 << 14),
    ("dualclip", 1 << 15),
    ("rf", 1 << 16),
    ("auto", 1 << 17),
    ("speed", 1 << 18),
    ("aperture", 1 << 19),
    ("telescopic", 1 << 20),
    ("scoped", 1 << 21),
    ("silenced", 1 << 22),
    ("flash", 1 << 23),
    ("bayonet", 1 << 24),
    ("bigammo", 1 << 25),
    ("bipod", 1 << 26),
    ("sawoff", 1 << 27),
];
/// In the order they are listed (CoD4's five keep their order).
const LISTED: [&str; 27] = [
    "reflex", "elbit", "acog", "lps", "vzoom", "ir", "upgradesight", "silencer", "snub", "grip", "gl", "mk", "ft", "extclip",
    "dualclip", "rf", "auto", "speed", "aperture", "telescopic", "scoped", "silenced", "flash", "bayonet", "bigammo", "bipod",
    "sawoff",
];

pub fn bit(name: &str) -> i32 {
    BY_PRIORITY.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map_or(0, |(_, b)| *b)
}

pub fn names(set: i32) -> Vec<&'static str> {
    LISTED.into_iter().filter(|n| set & bit(n) != 0).collect()
}

pub fn has(set: i32, name: &str) -> bool {
    set & bit(name) != 0
}

/// The attachment CoD's own stat holds for a set ("none" when empty).
pub fn main_attachment(set: i32) -> &'static str {
    BY_PRIORITY.iter().find(|(_, b)| set & b != 0).map_or("none", |(n, _)| n)
}

/// Click an attachment: add it (replacing a clashing one) or take it off.
pub fn toggle(set: i32, name: &str) -> i32 {
    let b = bit(name);
    match name {
        _ if b == 0 => 0,
        _ if set & b != 0 => set & !b,
        "gl" => GL,
        _ => {
            let mut s = set & !GL;
            if b & SIGHTS != 0 {
                s &= !SIGHTS;
            }
            if b & UNDERBARREL != 0 {
                s &= !UNDERBARREL;
            }
            s | b
        }
    }
}

/// The attachment set of a class weapon stat (201, 203, ...). The set only
/// stands while CoD's own stat still holds its main attachment: the original
/// scripts (picking a weapon, resetting a class) set just that one.
pub fn set_of(env: &dyn Env, weapon_stat: i32) -> i32 {
    // A Black Ops or World at War gun's set is all there is (CoD's stat
    // holds none).
    if env.stat(weapon_stat) >= crate::bo1::FIRST_INDEX {
        return env.stat(SET_STATS + weapon_stat);
    }
    let single = env.table_lookup(TABLE, 9, &env.stat(weapon_stat + 1).to_string(), 4);
    let set = env.stat(SET_STATS + weapon_stat);
    if set != 0 && main_attachment(set).eq_ignore_ascii_case(&single) { set } else { bit(&single) }
}

/// What an original attachment row's action selects: (attachment stat,
/// attachment), from `statSetUsingTable("202", tableLookup(attachmenttable,
/// 4, "reflex", 9))`.
pub fn row_selects(action: &str) -> Option<(i32, String)> {
    let toks = script::tokenize(action);
    let at = toks.iter().position(|t| matches!(t, Tok::Word(w) if w.eq_ignore_ascii_case("statsetusingtable")))?;
    let words: Vec<&str> = toks[at + 1..]
        .iter()
        .take_while(|t| **t != Tok::Punct(';'))
        .filter_map(|t| match t {
            Tok::Word(w) => Some(w.as_str()),
            _ => None,
        })
        .collect();
    match words[..] {
        [stat, f, table, "4", name, "9", ..] if f.eq_ignore_ascii_case("tablelookup") && table.eq_ignore_ascii_case(TABLE) => {
            let stat: i32 = stat.parse().ok()?;
            matches!(stat % 10, 2 | 4).then(|| (stat, name.to_owned()))
        }
        _ => None,
    }
}

/// Does an expression show an attachment's name (`attachmenttable` column 3)?
fn shows_name(exp: &Statement) -> bool {
    let from_table = exp.iter().any(|t| matches!(t, Token::Str(s) if s.eq_ignore_ascii_case(TABLE)));
    let name_column = exp.iter().rev().find_map(|t| match t {
        Token::Int(i) => Some(*i),
        _ => None,
    }) == Some(3);
    from_table && name_column
}

/// The weapon stat whose attachment name an expression shows,
/// `"@" + tableLookup(attachmenttable, 9, stat(N + 1), 3)`.
pub fn name_text_stat(exp: &Statement) -> Option<i32> {
    let stat = exp.windows(2).find_map(|w| match w {
        [Token::Op(op::STAT), Token::Int(n)] if (202..250).contains(n) && matches!(n % 10, 2 | 4) => Some(*n),
        _ => None,
    })?;
    shows_name(exp).then_some(stat - 1)
}

/// The same in the in-game class menu, where the class is a dvar:
/// `stat(dvarInt("ui_custom_class_highlighted") + 202)`, for a custom class
/// (`ui_multi_dt` set; the default classes read the class table).
pub fn highlighted_name_text_stat(exp: &Statement, env: &dyn Env) -> Option<i32> {
    let highlighted = exp.iter().any(|t| matches!(t, Token::Str(s) if s.eq_ignore_ascii_case("ui_custom_class_highlighted")));
    let offset = exp.iter().find_map(|t| match t {
        Token::Int(n) if matches!(n, 202 | 204) => Some(*n),
        _ => None,
    })?;
    let custom = env.dvar("ui_multi_dt").trim().parse::<f32>().is_ok_and(|v| v > 0.0);
    let class = env.dvar("ui_custom_class_highlighted").trim().parse::<i32>().unwrap_or(0);
    (highlighted && custom && shows_name(exp)).then_some(class + offset - 1)
}

/// Point a `localVarInt("ui_highlight") == N` test at another row.
pub(super) fn retarget_highlight(exp: &mut Statement, row: i32) {
    for i in 0..exp.len().saturating_sub(4) {
        if let [Token::Op(op::LOCALVARINT), Token::Str(s), Token::Op(op::RIGHTPAREN), Token::Op(op::EQUALS), Token::Int(n)] =
            &mut exp[i..i + 5]
        {
            if s.eq_ignore_ascii_case("ui_highlight") {
                *n = row;
            }
        }
    }
}

pub(super) fn highlight_rows(exp: &Statement) -> impl Iterator<Item = i32> + '_ {
    exp.windows(5).filter_map(|w| match w {
        [Token::Op(op::LOCALVARINT), Token::Str(s), Token::Op(op::RIGHTPAREN), Token::Op(op::EQUALS), Token::Int(n)]
            if s.eq_ignore_ascii_case("ui_highlight") =>
        {
            Some(*n)
        }
        _ => None,
    })
}

/// CoD4's attachment popups take one attachment and move straight on to the
/// camo list. Rows toggle attachments instead, so add an "Accept" row below
/// them (a copy of the "No Attachment" row) that moves on.
pub fn with_accept_row(menu: &Menu) -> Option<Menu> {
    let rows: Vec<(usize, String, f32)> = menu
        .items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.ty == item_type::BUTTON)
        .filter_map(|(i, it)| row_selects(&it.action).map(|(_, name)| (i, name, it.window.rect.y)))
        .collect();
    let none_y = rows.iter().find(|r| r.1 == "none")?.2;
    let mut ys: Vec<f32> = rows.iter().map(|r| r.2).collect();
    ys.sort_by(f32::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    let pitch = ys.windows(2).map(|w| w[1] - w[0]).reduce(f32::min)?;
    let below = ys.last()? + pitch;
    // Moving on: the closes and opens of a row that picks an attachment.
    let picker = rows.iter().find(|r| r.1 != "none" && r.1 != "gl")?;
    let toks = script::tokenize(&menu.items[picker.0].action);
    let mut next = String::from("\"play\" \"mouse_click\" ; ");
    for w in toks.windows(2) {
        if let [Tok::Word(cmd), Tok::Word(arg)] = w {
            if cmd.eq_ignore_ascii_case("close") || cmd.eq_ignore_ascii_case("open") {
                next += &format!("\"{cmd}\" \"{arg}\" ; ");
            }
        }
    }
    let row = menu.items.iter().flat_map(|it| highlight_rows(&it.visible_exp)).max().unwrap_or(0) + 1;
    let mut out = menu.clone();
    for it in menu.items.iter().filter(|it| (it.window.rect.y - none_y).abs() < 0.5) {
        let mut c = it.clone();
        c.window.rect.y = below;
        retarget_highlight(&mut c.visible_exp, row);
        if !c.text_exp.is_empty() {
            c.text_exp = vec![Token::Op(op::LEFTPAREN), Token::Str("@MENU_ACCEPT".into())];
        }
        if c.ty == item_type::BUTTON {
            c.action = next.clone();
            c.on_focus = format!(
                "\"play\" \"mouse_submenu_over\" ; \"setLocalVarInt\" \"ui_highlight\" {row} ; \
                 \"setLocalVarString\" \"ui_choicegroup\" \"popmenu\" ; \"setdvar\" \"ui_attachment_highlighted\" \"\" ; "
            );
        }
        out.items.push(c);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling() {
        let s = toggle(0, "silencer");
        let s = toggle(s, "reflex");
        assert_eq!(names(s), ["reflex", "silencer"]);
        // A second sight replaces the first.
        let s = toggle(s, "acog");
        assert_eq!(names(s), ["acog", "silencer"]);
        // Clicking again takes it off.
        assert_eq!(names(toggle(s, "acog")), ["silencer"]);
        // The grenade launcher stands alone, and gives way to anything else.
        assert_eq!(names(toggle(s, "gl")), ["gl"]);
        assert_eq!(names(toggle(GL, "grip")), ["grip"]);
        // Grip and sights mix.
        assert_eq!(names(toggle(toggle(0, "grip"), "reflex")), ["reflex", "grip"]);
        assert_eq!(main_attachment(REFLEX | SILENCER), "reflex");
        assert_eq!(toggle(s, "none"), 0);
    }

    struct ClassMenu;
    impl Env for ClassMenu {
        fn dvar(&self, name: &str) -> String {
            match name {
                "ui_custom_class_highlighted" => "20",
                "ui_multi_dt" => "1000",
                _ => "0",
            }
            .into()
        }
        fn stat(&self, _: i32) -> i32 {
            0
        }
        fn local(&self, _: &str) -> String {
            String::new()
        }
        fn menu_open(&self, _: &str) -> bool {
            false
        }
        fn table_lookup(&self, _: &str, _: i32, _: &str, _: i32) -> String {
            String::new()
        }
        fn localize(&self, text: &str) -> String {
            text.into()
        }
        fn millis(&self) -> i64 {
            0
        }
    }

    #[test]
    fn reads_class_menu_attachment_names() {
        use iw3::menu::op::*;
        let s = |v: &str| Token::Str(v.into());
        // The in-game class menu's secondary attachment name, for the
        // highlighted custom class (custom class 3: stats 220..229).
        let exp = vec![
            Token::Op(LEFTPAREN), s("@"), Token::Op(ADD), Token::Op(TABLELOOKUP), s(TABLE), Token::Op(COMMA), Token::Int(9),
            Token::Op(COMMA), Token::Op(TOINT), Token::Op(MIN), Token::Op(STAT), Token::Op(DVARINT),
            s("ui_custom_class_highlighted"), Token::Op(RIGHTPAREN), Token::Op(ADD), Token::Int(204), Token::Op(RIGHTPAREN),
            Token::Op(COMMA), Token::Int(3), Token::Op(RIGHTPAREN),
        ];
        assert_eq!(highlighted_name_text_stat(&exp, &ClassMenu), Some(223));
        assert_eq!(name_text_stat(&exp), None);
    }

    #[test]
    fn reads_row_actions() {
        let a = r#""play" "mouse_click" ; "statsetusingtable" ( "202" , "tablelookup" ( "mp/attachmenttable.csv" , 4 , "reflex" , 9 ) ) ; "open" "x""#;
        assert_eq!(row_selects(a), Some((202, "reflex".into())));
        let b = r#""statsetusingtable" ( "206" , "tablelookup" ( "mp/statstable.csv" , 4 , "specialty_armorvest" , 1 ) )"#;
        assert_eq!(row_selects(b), None);
    }
}
