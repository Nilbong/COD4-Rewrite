//! Tokens to the [`ast`](crate::ast): a recursive-descent parser for GSC.

use crate::ast::*;
use crate::lexer::{Tok, Token, lex};
use anyhow::{Result, anyhow, bail};

pub fn parse(src: &str) -> Result<Script> {
    let mut p = Parser { toks: lex(src)?, i: 0 };
    p.script()
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.i + n).min(self.toks.len() - 1)].tok
    }

    fn line(&self) -> u32 {
        self.toks[self.i].line
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.i].tok.clone();
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        t
    }

    fn is(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }

    fn is_at(&self, n: usize, p: &str) -> bool {
        matches!(self.peek_at(n), Tok::Punct(q) if *q == p)
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == w)
    }

    fn eat(&mut self, p: &str) -> bool {
        let yes = self.is(p);
        if yes {
            self.next();
        }
        yes
    }

    fn expect(&mut self, p: &str) -> Result<()> {
        if self.eat(p) { Ok(()) } else { Err(self.error(&format!("expected {p}"))) }
    }

    fn error(&self, what: &str) -> anyhow::Error {
        anyhow!("line {}: {what}, found {:?}", self.line(), self.peek())
    }

    fn ident(&mut self) -> Result<String> {
        match self.next() {
            Tok::Ident(s) => Ok(s),
            t => bail!("line {}: expected a name, found {t:?}", self.line()),
        }
    }

    fn script(&mut self) -> Result<Script> {
        let mut s = Script::default();
        loop {
            match self.peek().clone() {
                Tok::Eof => return Ok(s),
                Tok::Directive(d) if d == "include" => {
                    self.next();
                    s.includes.push(self.ident()?);
                    self.expect(";")?;
                }
                Tok::Directive(d) if d == "using_animtree" => {
                    self.next();
                    self.expect("(")?;
                    match self.next() {
                        Tok::Str(name) => s.animtree = Some(name.to_ascii_lowercase()),
                        t => bail!("line {}: #using_animtree wants a string, found {t:?}", self.line()),
                    }
                    self.expect(")")?;
                    self.expect(";")?;
                }
                Tok::Ident(_) => s.functions.push(self.function()?),
                _ => return Err(self.error("expected a function")),
            }
        }
    }

    fn function(&mut self) -> Result<Function> {
        let line = self.line();
        let name = self.ident()?;
        self.expect("(")?;
        let mut params = Vec::new();
        while !self.eat(")") {
            params.push(self.ident()?);
            if !self.is(")") {
                self.expect(",")?;
            }
        }
        let body = self.block()?;
        Ok(Function { name, params, body, line })
    }

    fn block(&mut self) -> Result<Vec<Stmt>> {
        self.expect("{")?;
        let mut out = Vec::new();
        while !self.eat("}") {
            if matches!(self.peek(), Tok::Eof) {
                return Err(self.error("unclosed {"));
            }
            out.push(self.stmt()?);
        }
        Ok(out)
    }

    fn stmt(&mut self) -> Result<Stmt> {
        if self.is("{") {
            return Ok(Stmt::Block(self.block()?));
        }
        if self.eat(";") {
            return Ok(Stmt::Empty);
        }
        let line = self.line();
        if let Tok::Ident(w) = self.peek().clone() {
            match w.as_str() {
                "if" => {
                    self.next();
                    self.expect("(")?;
                    let cond = self.expr()?;
                    self.expect(")")?;
                    let then = Box::new(self.stmt()?);
                    let other = if self.is_word("else") {
                        self.next();
                        Some(Box::new(self.stmt()?))
                    } else {
                        None
                    };
                    return Ok(Stmt::If(cond, then, other));
                }
                "while" => {
                    self.next();
                    self.expect("(")?;
                    let cond = self.expr()?;
                    self.expect(")")?;
                    return Ok(Stmt::While(cond, Box::new(self.stmt()?)));
                }
                "for" => {
                    self.next();
                    self.expect("(")?;
                    let init = if self.is(";") { None } else { Some(Box::new(self.simple()?)) };
                    self.expect(";")?;
                    let cond = if self.is(";") { None } else { Some(self.expr()?) };
                    self.expect(";")?;
                    let step = if self.is(")") { None } else { Some(Box::new(self.simple()?)) };
                    self.expect(")")?;
                    return Ok(Stmt::For(init, cond, step, Box::new(self.stmt()?)));
                }
                "switch" => {
                    self.next();
                    self.expect("(")?;
                    let on = self.expr()?;
                    self.expect(")")?;
                    self.expect("{")?;
                    let (mut cases, mut body) = (Vec::new(), Vec::new());
                    while !self.eat("}") {
                        if self.is_word("case") {
                            self.next();
                            let label = self.expr()?;
                            self.expect(":")?;
                            cases.push((Some(label), body.len()));
                        } else if self.is_word("default") {
                            self.next();
                            self.expect(":")?;
                            cases.push((None, body.len()));
                        } else if matches!(self.peek(), Tok::Eof) {
                            return Err(self.error("unclosed switch"));
                        } else {
                            body.push(self.stmt()?);
                        }
                    }
                    return Ok(Stmt::Switch(on, cases, body));
                }
                "break" => {
                    self.next();
                    self.expect(";")?;
                    return Ok(Stmt::Break);
                }
                "continue" => {
                    self.next();
                    self.expect(";")?;
                    return Ok(Stmt::Continue);
                }
                "return" => {
                    self.next();
                    let value = if self.is(";") { None } else { Some(self.expr()?) };
                    self.expect(";")?;
                    return Ok(Stmt::Return(value));
                }
                "wait" => {
                    self.next();
                    let e = self.expr()?;
                    self.expect(";")?;
                    return Ok(Stmt::Wait(e, line));
                }
                "waittillframeend" => {
                    self.next();
                    self.expect(";")?;
                    return Ok(Stmt::WaitFrameEnd);
                }
                "breakpoint" => {
                    self.next();
                    self.expect(";")?;
                    return Ok(Stmt::Empty);
                }
                _ => {}
            }
        }
        let s = self.simple()?;
        self.expect(";")?;
        Ok(s)
    }

    /// An expression, assignment or `++`/`--` (what `for` takes too).
    fn simple(&mut self) -> Result<Stmt> {
        let line = self.line();
        let e = self.expr()?;
        let op = match self.peek() {
            Tok::Punct("=") => Some(AssignOp::Set),
            Tok::Punct("+=") => Some(AssignOp::Add),
            Tok::Punct("-=") => Some(AssignOp::Sub),
            Tok::Punct("*=") => Some(AssignOp::Mul),
            Tok::Punct("/=") => Some(AssignOp::Div),
            Tok::Punct("%=") => Some(AssignOp::Mod),
            Tok::Punct("&=") => Some(AssignOp::And),
            Tok::Punct("|=") => Some(AssignOp::Or),
            Tok::Punct("^=") => Some(AssignOp::Xor),
            Tok::Punct("<<=") => Some(AssignOp::Shl),
            Tok::Punct(">>=") => Some(AssignOp::Shr),
            Tok::Punct("++") => {
                self.next();
                return Ok(Stmt::Step(e, true, line));
            }
            Tok::Punct("--") => {
                self.next();
                return Ok(Stmt::Step(e, false, line));
            }
            _ => None,
        };
        match op {
            Some(op) => {
                self.next();
                let value = self.expr()?;
                Ok(Stmt::Assign(e, op, value, line))
            }
            None => Ok(Stmt::Expr(e, line)),
        }
    }

    pub fn expr(&mut self) -> Result<Expr> {
        self.binary(0)
    }

    fn binary(&mut self, level: usize) -> Result<Expr> {
        const LEVELS: [&[(&str, BinOp)]; 10] = [
            &[("||", BinOp::Or)],
            &[("&&", BinOp::And)],
            &[("|", BinOp::BitOr)],
            &[("^", BinOp::BitXor)],
            &[("&", BinOp::BitAnd)],
            &[("==", BinOp::Eq), ("!=", BinOp::Ne)],
            &[("<", BinOp::Lt), (">", BinOp::Gt), ("<=", BinOp::Le), (">=", BinOp::Ge)],
            &[("<<", BinOp::Shl), (">>", BinOp::Shr)],
            &[("+", BinOp::Add), ("-", BinOp::Sub)],
            &[("*", BinOp::Mul), ("/", BinOp::Div), ("%", BinOp::Mod)],
        ];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        loop {
            let Tok::Punct(p) = self.peek() else { break };
            let Some(&(_, op)) = LEVELS[level].iter().find(|(s, _)| s == p) else { break };
            self.next();
            let rhs = self.binary(level + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat("~") {
            return Ok(Expr::BitNot(Box::new(self.unary()?)));
        }
        if self.eat("-") {
            return Ok(match self.unary()? {
                Expr::Int(v) => Expr::Int(v.wrapping_neg()),
                Expr::Float(v) => Expr::Float(-v),
                e => Expr::Neg(Box::new(e)),
            });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.is(".") {
                self.next();
                let field = self.ident()?;
                e = Expr::Field(Box::new(e), field);
            } else if self.is("[") && !self.is_at(1, "[") {
                self.next();
                let index = self.expr()?;
                self.expect("]")?;
                e = Expr::Index(Box::new(e), Box::new(index));
            } else if self.method_follows() {
                e = self.call(Some(e))?;
            } else {
                return Ok(e);
            }
        }
    }

    /// After an expression: a call on it (`self foo()`, `guy thread bar()`,
    /// `ent [[func]]()`).
    fn method_follows(&self) -> bool {
        match self.peek() {
            Tok::Ident(w) if w == "thread" => true,
            Tok::Ident(_) => self.is_at(1, "(") || self.is_at(1, "::"),
            Tok::Punct("[") => self.is_at(1, "["),
            _ => false,
        }
    }

    /// A call: `[thread] name(args)`, `[thread] path::name(args)` or
    /// `[thread] [[ptr]](args)`, on `object` if given.
    fn call(&mut self, object: Option<Expr>) -> Result<Expr> {
        let line = self.line();
        let thread = self.is_word("thread");
        if thread {
            self.next();
        }
        let callee = if self.is("[") {
            self.next();
            self.expect("[")?;
            let ptr = self.expr()?;
            self.expect("]")?;
            self.expect("]")?;
            Callee::Pointer(ptr)
        } else if self.is("::") {
            self.next();
            Callee::Named(None, self.ident()?)
        } else {
            let name = self.ident()?;
            if self.eat("::") { Callee::Named(Some(name), self.ident()?) } else { Callee::Named(None, name) }
        };
        let args = self.args()?;
        Ok(Expr::Call(Box::new(Call { object, callee, args, thread, line })))
    }

    fn args(&mut self) -> Result<Vec<Expr>> {
        self.expect("(")?;
        let mut args = Vec::new();
        while !self.eat(")") {
            args.push(self.expr()?);
            if !self.is(")") {
                self.expect(",")?;
            }
        }
        Ok(args)
    }

    fn primary(&mut self) -> Result<Expr> {
        match self.peek().clone() {
            Tok::Int(v) => {
                self.next();
                Ok(Expr::Int(v))
            }
            Tok::Float(v) => {
                self.next();
                Ok(Expr::Float(v))
            }
            Tok::Str(s) => {
                self.next();
                Ok(Expr::Str(s))
            }
            Tok::IStr(s) => {
                self.next();
                Ok(Expr::IStr(s))
            }
            Tok::Directive(d) if d == "animtree" => {
                self.next();
                Ok(Expr::AnimTree)
            }
            Tok::Punct("%") => {
                self.next();
                Ok(Expr::Anim(self.ident()?))
            }
            Tok::Punct("(") => {
                self.next();
                let first = self.expr()?;
                if self.eat(",") {
                    let second = self.expr()?;
                    self.expect(",")?;
                    let third = self.expr()?;
                    self.expect(")")?;
                    return Ok(Expr::Vector(Box::new([first, second, third])));
                }
                self.expect(")")?;
                Ok(first)
            }
            Tok::Punct("[") if self.is_at(1, "]") => {
                self.next();
                self.next();
                Ok(Expr::EmptyArray)
            }
            Tok::Punct("[") => self.call(None),
            Tok::Punct("::") => {
                self.next();
                let name = self.ident()?;
                if self.is("(") {
                    let args = self.args()?;
                    let line = self.line();
                    return Ok(Expr::Call(Box::new(Call { object: None, callee: Callee::Named(None, name), args, thread: false, line })));
                }
                Ok(Expr::FuncRef(None, name))
            }
            Tok::Ident(w) => match w.as_str() {
                "self" => {
                    self.next();
                    Ok(Expr::SelfRef)
                }
                "level" => {
                    self.next();
                    Ok(Expr::Level)
                }
                "game" => {
                    self.next();
                    Ok(Expr::Game)
                }
                "anim" => {
                    self.next();
                    Ok(Expr::Anim_)
                }
                "undefined" => {
                    self.next();
                    Ok(Expr::Undefined)
                }
                "thread" => self.call(None),
                _ if self.is_at(1, "(") => self.call(None),
                _ if self.is_at(1, "::") => {
                    if matches!(self.peek_at(3), Tok::Punct("(")) {
                        return self.call(None);
                    }
                    self.next();
                    self.next();
                    let name = self.ident()?;
                    Ok(Expr::FuncRef(Some(w), name))
                }
                _ => {
                    self.next();
                    Ok(Expr::Ident(w))
                }
            },
            _ => Err(self.error("expected an expression")),
        }
    }
}
