//! GSC source to tokens. Comments and developer blocks (`/# ... #/`, only
//! compiled into developer builds) are skipped.

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// A name, or a script path (`maps\_utility`).
    Ident(String),
    Int(i32),
    Float(f32),
    Str(String),
    /// `&"KEY"`: a localized string.
    IStr(String),
    /// `#include`, `#using_animtree`, `#animtree`.
    Directive(String),
    Punct(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
}

/// Longest first.
const PUNCT: [&str; 46] = [
    "<<=", ">>=", "==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "++", "--", "+=", "-=", "*=", "/=", "%=", "|=", "&=", "^=", "::", "(", ")", "{",
    "}", "[", "]", ";", ",", ".", "=", "<", ">", "+", "-", "*", "/", "%", "!", "~", "&", "|", "^", ":", "?", "\\",
];

pub fn lex(src: &str) -> Result<Vec<Token>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1u32;
    let ident_start = |c: u8| c.is_ascii_alphabetic() || c == b'_';
    let ident_char = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            _ if c.is_ascii_whitespace() => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if matches!(b.get(i + 1), Some(b'*' | b'#')) => {
                let end: &[u8] = if b[i + 1] == b'*' { b"*/" } else { b"#/" };
                i += 2;
                while i < b.len() && !b[i..].starts_with(end) {
                    line += u32::from(b[i] == b'\n');
                    i += 1;
                }
                i += 2;
            }
            b'"' => {
                let (s, n, lines) = string(&b[i..])?;
                out.push(Token { tok: Tok::Str(s), line });
                line += lines;
                i += n;
            }
            b'&' if b.get(i + 1) == Some(&b'"') => {
                let (s, n, lines) = string(&b[i + 1..])?;
                out.push(Token { tok: Tok::IStr(s), line });
                line += lines;
                i += n + 1;
            }
            b'#' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && ident_char(b[j]) {
                    j += 1;
                }
                if j == start {
                    bail!("line {line}: stray #");
                }
                out.push(Token { tok: Tok::Directive(src[start..j].to_ascii_lowercase()), line });
                i = j;
            }
            _ if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) => {
                let start = i;
                let mut float = false;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                    float |= b[i] == b'.';
                    i += 1;
                }
                if i < b.len() && (b[i] == b'e' || b[i] == b'E') && b.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == b'-' || *d == b'+') {
                    float = true;
                    i += 2;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let text = &src[start..i];
                let tok = if float {
                    Tok::Float(text.parse().map_err(|_| anyhow::anyhow!("line {line}: bad number {text}"))?)
                } else {
                    match text.parse::<i64>() {
                        Ok(v) => Tok::Int(v as i32),
                        Err(_) => bail!("line {line}: bad number {text}"),
                    }
                };
                out.push(Token { tok, line });
            }
            _ if ident_start(c) => {
                let start = i;
                while i < b.len() && ident_char(b[i]) {
                    i += 1;
                }
                // A path: `maps\_utility`, `animscripts\traverse\shared`.
                while i + 1 < b.len() && b[i] == b'\\' && ident_start(b[i + 1]) {
                    i += 1;
                    while i < b.len() && ident_char(b[i]) {
                        i += 1;
                    }
                }
                out.push(Token { tok: Tok::Ident(src[start..i].to_ascii_lowercase()), line });
            }
            _ => {
                let Some(p) = PUNCT.iter().find(|p| b[i..].starts_with(p.as_bytes())) else {
                    bail!("line {line}: unexpected character {:?}", c as char);
                };
                out.push(Token { tok: Tok::Punct(p), line });
                i += p.len();
            }
        }
    }
    out.push(Token { tok: Tok::Eof, line });
    Ok(out)
}

/// A quoted string at the start of `b`: its text, bytes taken and newlines
/// inside.
fn string(b: &[u8]) -> Result<(String, usize, u32)> {
    let mut s = Vec::new();
    let mut i = 1;
    let mut lines = 0;
    while i < b.len() && b[i] != b'"' {
        match b[i] {
            b'\\' if i + 1 < b.len() => {
                s.push(match b[i + 1] {
                    b'n' => b'\n',
                    b't' => b'\t',
                    c => c,
                });
                i += 2;
            }
            c => {
                lines += u32::from(c == b'\n');
                s.push(c);
                i += 1;
            }
        }
    }
    if i >= b.len() {
        bail!("unterminated string");
    }
    Ok((String::from_utf8_lossy(&s).into_owned(), i + 1, lines))
}
