//! Menu expressions (`visible when(...)`, `exp text(...)`, ...): evaluation
//! of the infix token lists compiled into the zone.

use iw3::menu::{Token, op};

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Int(i32),
    Float(f32),
    Str(String),
}

impl Val {
    pub fn num(&self) -> f32 {
        match self {
            Val::Int(i) => *i as f32,
            Val::Float(f) => *f,
            Val::Str(s) => s.trim().parse().unwrap_or(0.0),
        }
    }

    pub fn int(&self) -> i32 {
        match self {
            Val::Int(i) => *i,
            Val::Float(f) => *f as i32,
            Val::Str(s) => s.trim().parse::<f32>().map_or(0, |f| f as i32),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Val::Int(i) => i.to_string(),
            Val::Float(f) => f.to_string(),
            Val::Str(s) => s.clone(),
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Val::Str(s) => s.trim().parse::<f32>().map_or(!s.is_empty(), |f| f != 0.0),
            v => v.num() != 0.0,
        }
    }
}

/// What expressions can read.
pub trait Env {
    fn dvar(&self, name: &str) -> String;
    fn stat(&self, index: i32) -> i32;
    fn local(&self, name: &str) -> String;
    fn menu_open(&self, name: &str) -> bool;
    /// `tableLookup(table, keyColumn, key, valueColumn)`.
    fn table_lookup(&self, table: &str, key_col: i32, key: &str, val_col: i32) -> String;
    /// Resolve `@KEY` references; other text is returned unchanged.
    fn localize(&self, text: &str) -> String;
    fn millis(&self) -> i64;
    /// The player's team (`TEAM_FREE` outside a match).
    fn team(&self) -> String {
        "TEAM_FREE".into()
    }
    /// A function of the match's state (scores, time left, ...), or `None`
    /// for the defaults outside a match.
    fn game(&self, _f: u8, _args: &[Val]) -> Option<Val> {
        None
    }
}

pub fn eval(tokens: &[Token], env: &dyn Env) -> Val {
    Parser { t: tokens, i: 0, env }.expr(0)
}

struct Parser<'a> {
    t: &'a [Token],
    i: usize,
    env: &'a dyn Env,
}

fn precedence(o: u8) -> Option<u8> {
    Some(match o {
        op::OR => 1,
        op::AND => 2,
        op::BITWISEOR => 3,
        op::BITWISEAND => 4,
        op::EQUALS | op::NOTEQUAL => 5,
        op::LESSTHAN | op::LESSTHANEQUALTO | op::GREATERTHAN | op::GREATERTHANEQUALTO => 6,
        op::BITSHIFTLEFT | op::BITSHIFTRIGHT => 7,
        op::ADD | op::SUBTRACT => 8,
        op::MULTIPLY | op::DIVIDE | op::MODULUS => 9,
        _ => return None,
    })
}

impl Parser<'_> {
    fn at(&self, o: u8) -> bool {
        self.t.get(self.i) == Some(&Token::Op(o))
    }

    fn expr(&mut self, min: u8) -> Val {
        let mut lhs = self.unary();
        while let Some(Token::Op(o)) = self.t.get(self.i) {
            let o = *o;
            let Some(p) = precedence(o) else { break };
            if p < min {
                break;
            }
            self.i += 1;
            let rhs = self.expr(p + 1);
            lhs = binary(o, lhs, rhs);
        }
        lhs
    }

    fn unary(&mut self) -> Val {
        let Some(t) = self.t.get(self.i).cloned() else { return Val::Int(0) };
        self.i += 1;
        match t {
            Token::Int(i) => Val::Int(i),
            Token::Float(f) => Val::Float(f),
            Token::Str(s) => Val::Str(s),
            Token::Op(op::LEFTPAREN) => {
                let v = self.expr(0);
                // The outermost `)` of `exp text(...)` is not stored.
                if self.at(op::RIGHTPAREN) {
                    self.i += 1;
                }
                v
            }
            Token::Op(op::NOT) => Val::Int(!self.unary().truthy() as i32),
            // A leading minus negates the sum after it: `( - 41 + 0 - x )` is
            // -(41 + 0 - x), as the HUD's `scorebars` rows need to swap.
            Token::Op(op::SUBTRACT) => match self.expr(precedence(op::ADD).unwrap_or(0)) {
                Val::Int(i) => Val::Int(-i),
                v => Val::Float(-v.num()),
            },
            Token::Op(op::BITWISENOT) => Val::Int(!self.unary().int()),
            Token::Op(f) if f >= op::FIRST_FUNCTION => {
                let args = self.args();
                call(f, &args, self.env)
            }
            _ => Val::Int(0),
        }
    }

    /// Function arguments up to and including the closing `)`.
    fn args(&mut self) -> Vec<Val> {
        let mut args = Vec::new();
        if self.at(op::RIGHTPAREN) {
            self.i += 1;
            return args;
        }
        while self.i < self.t.len() {
            args.push(self.expr(0));
            if self.at(op::COMMA) {
                self.i += 1;
                continue;
            }
            if self.at(op::RIGHTPAREN) {
                self.i += 1;
            }
            break;
        }
        args
    }
}

fn binary(o: u8, a: Val, b: Val) -> Val {
    let both_int = matches!((&a, &b), (Val::Int(_), Val::Int(_)));
    let arith = |fi: fn(i32, i32) -> i32, ff: fn(f32, f32) -> f32| {
        if both_int { Val::Int(fi(a.int(), b.int())) } else { Val::Float(ff(a.num(), b.num())) }
    };
    let cmp = |ord: std::cmp::Ordering| -> bool {
        use std::cmp::Ordering::*;
        match o {
            op::EQUALS => ord == Equal,
            op::NOTEQUAL => ord != Equal,
            op::LESSTHAN => ord == Less,
            op::LESSTHANEQUALTO => ord != Greater,
            op::GREATERTHAN => ord == Greater,
            _ => ord != Less,
        }
    };
    match o {
        // `+` joins strings, e.g. "@" + tableLookup(...).
        op::ADD if matches!(a, Val::Str(_)) || matches!(b, Val::Str(_)) => Val::Str(a.text() + &b.text()),
        op::ADD => arith(i32::wrapping_add, |x, y| x + y),
        op::SUBTRACT => arith(i32::wrapping_sub, |x, y| x - y),
        op::MULTIPLY => arith(i32::wrapping_mul, |x, y| x * y),
        op::DIVIDE => arith(|x, y| if y == 0 { 0 } else { x / y }, |x, y| if y == 0.0 { 0.0 } else { x / y }),
        op::MODULUS => arith(|x, y| if y == 0 { 0 } else { x % y }, |x, y| if y == 0.0 { 0.0 } else { x % y }),
        op::AND => Val::Int((a.truthy() && b.truthy()) as i32),
        op::OR => Val::Int((a.truthy() || b.truthy()) as i32),
        op::BITWISEAND => Val::Int(a.int() & b.int()),
        op::BITWISEOR => Val::Int(a.int() | b.int()),
        op::BITSHIFTLEFT => Val::Int(a.int().wrapping_shl(b.int() as u32)),
        op::BITSHIFTRIGHT => Val::Int(a.int().wrapping_shr(b.int() as u32)),
        _ => {
            let ord = match (&a, &b) {
                (Val::Str(x), Val::Str(y)) => x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase()),
                _ => a.num().partial_cmp(&b.num()).unwrap_or(std::cmp::Ordering::Equal),
            };
            Val::Int(cmp(ord) as i32)
        }
    }
}

fn call(f: u8, args: &[Val], env: &dyn Env) -> Val {
    let a = |i: usize| args.get(i).cloned().unwrap_or(Val::Int(0));
    let s = |i: usize| a(i).text();
    let number = |text: String| Val::Float(text.trim().parse().unwrap_or(0.0));
    if let Some(v) = env.game(f, args) {
        return v;
    }
    match f {
        op::SIN => Val::Float(a(0).num().sin()),
        op::COS => Val::Float(a(0).num().cos()),
        op::MIN | op::MAX => {
            let (x, y) = (a(0), a(1));
            let pick_x = if f == op::MIN { x.num() <= y.num() } else { x.num() >= y.num() };
            if pick_x { x } else { y }
        }
        op::MILLISECONDS => Val::Int(env.millis() as i32),
        op::DVARINT => Val::Int(number(env.dvar(&s(0))).int()),
        op::DVARBOOL => Val::Int(Val::Str(env.dvar(&s(0))).truthy() as i32),
        op::DVARFLOAT => number(env.dvar(&s(0))),
        op::DVARSTRING => Val::Str(env.dvar(&s(0))),
        op::STAT => Val::Int(env.stat(a(0).int())),
        op::UIACTIVE => Val::Int(1),
        op::TEAM => Val::Str(env.team()),
        op::MENUISOPEN => Val::Int(env.menu_open(&s(0)) as i32),
        op::SECONDSASTIME => {
            let t = a(0).int().max(0);
            Val::Str(format!("{}:{:02}", t / 60, t % 60))
        }
        op::TABLELOOKUP => Val::Str(env.table_lookup(&s(0), a(1).int(), &s(2), a(3).int())),
        // The first string with `&&1`, `&&2`... replaced by the rest (or
        // the rest appended where it has no such places).
        op::LOCALIZESTRING => {
            let mut out = env.localize(&s(0));
            for (k, v) in args.iter().enumerate().skip(1) {
                let (place, v) = (format!("&&{k}"), env.localize(&v.text()));
                out = if out.contains(&place) { out.replace(&place, &v) } else { out + &v };
            }
            Val::Str(out)
        }
        op::SECONDSASCOUNTDOWN => {
            let t = a(0).num().ceil().max(0.0) as i32;
            Val::Str(format!("{}:{:02}", t / 60, t % 60))
        }
        op::LOCALVARINT => Val::Int(number(env.local(&s(0))).int()),
        op::LOCALVARBOOL => Val::Int(Val::Str(env.local(&s(0))).truthy() as i32),
        op::LOCALVARFLOAT => number(env.local(&s(0))),
        op::LOCALVARSTRING => Val::Str(env.local(&s(0))),
        op::TOINT => Val::Int(a(0).int()),
        op::TOSTRING => Val::Str(a(0).text()),
        op::TOFLOAT => Val::Float(a(0).num()),
        op::GAMETYPE => Val::Str(env.dvar("ui_netGametypeName")),
        op::GAMETYPENAME | op::GAMETYPEDESCRIPTION => {
            let col = if f == op::GAMETYPENAME { 1 } else { 2 };
            let key = env.table_lookup("mp/gametypesTable.csv", 0, &env.dvar("ui_netGametypeName"), col);
            Val::Str(env.localize(&format!("@{key}")))
        }
        op::MAXPLAYERS => Val::Int(18),
        // Is any of these bits set in any stat of the range?
        op::STATRANGEBITSSET => {
            let mask = a(2).int();
            Val::Int((a(0).int()..=a(1).int()).any(|i| env.stat(i) & mask != 0) as i32)
        }
        _ => Val::Int(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestEnv;
    impl Env for TestEnv {
        fn dvar(&self, name: &str) -> String {
            match name {
                "fs_game" => "",
                "ui_netGametypeName" => "war",
                _ => "0",
            }
            .into()
        }
        fn stat(&self, _: i32) -> i32 {
            1
        }
        fn local(&self, name: &str) -> String {
            if name == "ui_highlight" { "3".into() } else { String::new() }
        }
        fn menu_open(&self, _: &str) -> bool {
            false
        }
        fn table_lookup(&self, _: &str, _: i32, _: &str, _: i32) -> String {
            String::new()
        }
        fn localize(&self, text: &str) -> String {
            text.trim_start_matches('@').to_lowercase()
        }
        fn millis(&self) -> i64 {
            30_000
        }
    }

    fn o(x: u8) -> Token {
        Token::Op(x)
    }

    #[test]
    fn main_menu_expressions() {
        let s = |x: &str| Token::Str(x.into());
        // ( dvarString("fs_game") ) == ""
        let e = [o(op::LEFTPAREN), o(op::DVARSTRING), s("fs_game"), o(op::RIGHTPAREN), o(op::EQUALS), s("")];
        assert_eq!(eval(&e, &TestEnv), Val::Int(1));
        // ( localVarInt("ui_highlight") == 3 && localVarString("ui_choicegroup") == "main" )
        let e = [
            o(op::LEFTPAREN),
            o(op::LOCALVARINT),
            s("ui_highlight"),
            o(op::RIGHTPAREN),
            o(op::EQUALS),
            Token::Int(3),
            o(op::AND),
            o(op::LOCALVARSTRING),
            s("ui_choicegroup"),
            o(op::RIGHTPAREN),
            o(op::EQUALS),
            s("main"),
        ];
        assert_eq!(eval(&e, &TestEnv), Val::Int(0));
        // ( !localVarBool("ui_hideBack") )
        let e = [o(op::LEFTPAREN), o(op::NOT), o(op::LOCALVARBOOL), s("ui_hideBack"), o(op::RIGHTPAREN)];
        assert!(eval(&e, &TestEnv).truthy());
        // ((-107) - ((float(milliseconds() % 60000) / 60000) * (854)))
        let e = [
            o(op::LEFTPAREN),
            o(op::LEFTPAREN),
            o(op::SUBTRACT),
            Token::Int(107),
            o(op::RIGHTPAREN),
            o(op::SUBTRACT),
            o(op::LEFTPAREN),
            o(op::LEFTPAREN),
            o(op::TOFLOAT),
            o(op::MILLISECONDS),
            o(op::RIGHTPAREN),
            o(op::MODULUS),
            Token::Int(60000),
            o(op::RIGHTPAREN),
            o(op::DIVIDE),
            Token::Int(60000),
            o(op::RIGHTPAREN),
            o(op::MULTIPLY),
            o(op::LEFTPAREN),
            Token::Int(854),
            o(op::RIGHTPAREN),
            o(op::RIGHTPAREN),
        ];
        assert!((eval(&e, &TestEnv).num() - (-107.0 - 0.5 * 854.0)).abs() < 1e-3);
        // ( "@MENU_QUIT"   (text expressions omit the final paren)
        let e = [o(op::LEFTPAREN), s("@MENU_QUIT")];
        assert_eq!(eval(&e, &TestEnv), Val::Str("@MENU_QUIT".into()));
        // stat(260) >= 1
        let e = [o(op::STAT), Token::Int(260), o(op::RIGHTPAREN), o(op::GREATERTHANEQUALTO), Token::Int(1)];
        assert!(eval(&e, &TestEnv).truthy());
    }

    #[test]
    fn precedence_and_strings() {
        let e = [Token::Int(2), o(op::ADD), Token::Int(3), o(op::MULTIPLY), Token::Int(4)];
        assert_eq!(eval(&e, &TestEnv), Val::Int(14));
        let e = [Token::Str("@".into()), o(op::ADD), o(op::DVARSTRING), Token::Str("ui_netGametypeName".into()), o(op::RIGHTPAREN)];
        assert_eq!(eval(&e, &TestEnv), Val::Str("@war".into()));
        let e = [o(op::GAMETYPE), o(op::RIGHTPAREN), o(op::EQUALS), Token::Str("WAR".into())];
        assert!(eval(&e, &TestEnv).truthy());
    }

    #[test]
    fn leading_minus_negates_the_sum() {
        // ( - 41 + 0 - ( 1 * 24 ) ): the scorebars row when losing.
        let e = [
            o(op::LEFTPAREN),
            o(op::SUBTRACT),
            Token::Int(41),
            o(op::ADD),
            Token::Int(0),
            o(op::SUBTRACT),
            o(op::LEFTPAREN),
            Token::Int(1),
            o(op::MULTIPLY),
            Token::Int(24),
            o(op::RIGHTPAREN),
            o(op::RIGHTPAREN),
        ];
        assert_eq!(eval(&e, &TestEnv), Val::Int(-17));
        // -5 < 3 still compares the negated number.
        let e = [o(op::SUBTRACT), Token::Int(5), o(op::LESSTHAN), Token::Int(3)];
        assert!(eval(&e, &TestEnv).truthy());
    }

    #[test]
    fn localize_fills_places() {
        // locString("Winning with &&1 of &&2 points.", 10, 750)
        let e = [
            o(op::LEFTPAREN),
            o(op::LOCALIZESTRING),
            Token::Str("Winning with &&1 of &&2 points.".into()),
            o(op::COMMA),
            Token::Int(10),
            o(op::COMMA),
            Token::Int(750),
            o(op::RIGHTPAREN),
        ];
        assert_eq!(eval(&e, &TestEnv), Val::Str("winning with 10 of 750 points.".into()));
        let e = [o(op::SECONDSASCOUNTDOWN), Token::Float(64.2), o(op::RIGHTPAREN)];
        assert_eq!(eval(&e, &TestEnv), Val::Str("1:05".into()));
    }
}
