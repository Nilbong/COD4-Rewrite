//! Menu scripts (`action`, `onOpen`, ...) and the console commands they
//! `exec`: `;`-separated commands of quoted or bare arguments.

/// Split a script into commands, each a list of arguments with quotes removed.
pub fn parse(script: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut cmd: Vec<String> = Vec::new();
    let mut chars = script.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ';' => {
                chars.next();
                if !cmd.is_empty() {
                    commands.push(std::mem::take(&mut cmd));
                }
            }
            '"' => {
                chars.next();
                let mut arg = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if chars.peek() == Some(&'"') => arg.push(chars.next().unwrap()),
                        c => arg.push(c),
                    }
                }
                cmd.push(arg);
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            _ => {
                let mut arg = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == ';' || c == '"' {
                        break;
                    }
                    arg.push(c);
                    chars.next();
                }
                cmd.push(arg);
            }
        }
    }
    if !cmd.is_empty() {
        commands.push(cmd);
    }
    commands
}

/// A script token: a quoted or bare word, or one of `;(),`.
#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Word(String),
    Punct(char),
}

/// Split a menu script into tokens. Unlike [`parse`], `;` is only a token:
/// commands are delimited by how many arguments they take (see [`arity`]),
/// as in the game, which runs scripts like `"close" "self" "open" "x"`.
pub fn tokenize(script: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut chars = script.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ';' | '(' | ')' | ',' => {
                chars.next();
                out.push(Tok::Punct(c));
            }
            '"' => {
                chars.next();
                let mut arg = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if chars.peek() == Some(&'"') => arg.push(chars.next().unwrap()),
                        c => arg.push(c),
                    }
                }
                out.push(Tok::Word(arg));
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            _ => {
                let mut arg = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || matches!(c, ';' | '(' | ')' | ',' | '"') {
                        break;
                    }
                    arg.push(c);
                    chars.next();
                }
                out.push(Tok::Word(arg));
            }
        }
    }
    out
}

/// Arguments taken by a menu script command (lower-cased); `None` for
/// commands that take everything up to the next `;`, like `uiScript`.
pub fn arity(command: &str) -> Option<usize> {
    Some(match command {
        "escape" | "focusfirst" | "feedertop" | "feederbottom" => 0,
        "open" | "close" | "show" | "hide" | "fadein" | "fadeout" | "play" | "playlooped" | "exec" | "execnow"
        | "setfocus" | "setfocusbydvar" | "showmenu" | "hidemenu" | "setbackground" | "scriptmenuresponse"
        | "closeforallplayers" | "ingameopen" | "ingameclose" | "openforgametype" | "closeforgametype" => 1,
        "setdvar" | "setlocalvarint" | "setlocalvarbool" | "setlocalvarfloat" | "setlocalvarstring"
        | "statclearbitmask" | "statsetbitmask" => 2,
        "execondvarstringvalue" | "execondvarintvalue" | "execondvarfloatvalue" | "execnowondvarstringvalue"
        | "execnowondvarintvalue" | "execnowondvarfloatvalue" | "scriptmenurespondondvarstringvalue"
        | "scriptmenurespondondvarintvalue" | "scriptmenurespondondvarfloatvalue" => 3,
        "setcolor" => 5,
        "setitemcolor" => 6,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_commands() {
        let s = r#""play" "mouse_click" ; "open" "createserver" ; ; "uiScript" "openMenuOnDvar" "com_playerProfile" "" "player_profile" ; "#;
        assert_eq!(
            parse(s),
            vec![
                vec!["play", "mouse_click"],
                vec!["open", "createserver"],
                vec!["uiScript", "openMenuOnDvar", "com_playerProfile", "", "player_profile"],
            ]
        );
        assert_eq!(parse("set ui_hint_text @MP_NULL"), vec![vec!["set", "ui_hint_text", "@MP_NULL"]]);
        assert_eq!(parse(r#""setLocalVarInt" "ui_highlight" 1"#), vec![vec!["setLocalVarInt", "ui_highlight", "1"]]);
    }

    #[test]
    fn tokenizes_calls() {
        let w = |s: &str| Tok::Word(s.into());
        let p = Tok::Punct;
        assert_eq!(
            tokenize(r#""close" "self" "statsetusingtable" ( "206" , "tablelookup" ( "mp/statstable.csv" , 4 , "x" , 1 ) ) ;"#),
            vec![
                w("close"),
                w("self"),
                w("statsetusingtable"),
                p('('),
                w("206"),
                p(','),
                w("tablelookup"),
                p('('),
                w("mp/statstable.csv"),
                p(','),
                w("4"),
                p(','),
                w("x"),
                p(','),
                w("1"),
                p(')'),
                p(')'),
                p(';'),
            ]
        );
    }
}
