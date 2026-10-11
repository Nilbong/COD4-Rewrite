//! The parsed script to a small stack machine's code, per function.

use crate::ast::*;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum Target {
    /// Before linking: a script path (or none: this script and its
    /// includes, then builtins) and a name.
    Name(Option<Rc<str>>, Rc<str>),
    Func(super::FuncId),
    Builtin(Rc<str>),
}

/// Where an assignment's path starts.
#[derive(Clone, Copy, Debug)]
pub enum Root {
    Local(u16),
    SelfRef,
    Level,
    Game,
    Anim,
    /// An expression's value, pushed first (must be an object).
    Value,
}

#[derive(Clone, Debug)]
pub enum Op {
    Undef,
    Int(i32),
    Float(f32),
    Str(Rc<str>),
    IStr(Rc<str>),
    Anim(Rc<str>),
    AnimTree,
    EmptyArray,
    /// Three values to a vector.
    Vector,
    SelfRef,
    Level,
    Game,
    AnimObj,
    Local(u16),
    Field(Rc<str>),
    Index,
    FuncRef(Target),
    Bin(BinOp),
    Not,
    BitNot,
    Neg,
    Jump(u32),
    /// Pops; jumps if false.
    JumpFalse(u32),
    /// `&&` / `||`: if the top decides it, leave it (as 0/1) and jump, else pop.
    AndJump(u32),
    OrJump(u32),
    /// The top to 0/1.
    Bool,
    /// Arguments pushed in order (after the object if `method`).
    Call { target: Target, argc: u8, method: bool, thread: bool, line: u32 },
    /// The pointer pushed last.
    CallPtr { argc: u8, method: bool, thread: bool, line: u32 },
    Pop,
    Dup,
    Ret,
    RetUndef,
    Wait,
    WaitFrameEnd,
    /// Object and event pushed; resumes with `vars` values pushed (the
    /// notify's arguments).
    Waittill { vars: u8 },
    /// Object, event and the first argument to match pushed.
    WaittillMatch,
    /// Path keys pushed (after the root's value for [`Root::Value`]), then
    /// the value. `fields[i]`: whether key `i` is a `.field` (else an index).
    Store { root: Root, fields: Rc<[bool]> },
}

pub struct Compiled {
    pub name: Rc<str>,
    pub params: usize,
    pub locals: usize,
    pub code: Vec<Op>,
    pub lines: Vec<u32>,
}

/// Compile a function of `script` (its lower-case path).
pub fn function(f: &Function, script: &str) -> Compiled {
    let mut c = Compiler { code: Vec::new(), lines: Vec::new(), locals: HashMap::new(), breaks: Vec::new(), continues: Vec::new(), line: f.line };
    for p in &f.params {
        c.local(p);
    }
    for s in &f.body {
        c.stmt(s);
    }
    c.emit(Op::RetUndef);
    Compiled { name: format!("{script}::{}", f.name).into(), params: f.params.len(), locals: c.locals.len(), code: c.code, lines: c.lines }
}

struct Compiler {
    code: Vec<Op>,
    lines: Vec<u32>,
    locals: HashMap<String, u16>,
    /// Jumps to patch at the end of each enclosing loop / switch.
    breaks: Vec<Vec<usize>>,
    continues: Vec<Vec<usize>>,
    line: u32,
}

impl Compiler {
    fn emit(&mut self, op: Op) -> usize {
        self.code.push(op);
        self.lines.push(self.line);
        self.code.len() - 1
    }

    fn here(&self) -> u32 {
        self.code.len() as u32
    }

    fn patch(&mut self, at: usize, to: u32) {
        match &mut self.code[at] {
            Op::Jump(t) | Op::JumpFalse(t) | Op::AndJump(t) | Op::OrJump(t) => *t = to,
            _ => unreachable!(),
        }
    }

    fn local(&mut self, name: &str) -> u16 {
        let n = self.locals.len() as u16;
        *self.locals.entry(name.to_owned()).or_insert(n)
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Expr(e, line) => {
                self.line = *line;
                self.expr_stmt(e);
            }
            Stmt::Assign(target, op, value, line) => {
                self.line = *line;
                self.assign(target, *op, value);
            }
            Stmt::Step(target, up, line) => {
                self.line = *line;
                let one = Expr::Int(1);
                self.assign(target, if *up { AssignOp::Add } else { AssignOp::Sub }, &one);
            }
            Stmt::Wait(e, line) => {
                self.line = *line;
                self.expr(e);
                self.emit(Op::Wait);
            }
            Stmt::WaitFrameEnd => {
                self.emit(Op::WaitFrameEnd);
            }
            Stmt::If(cond, then, other) => {
                self.expr(cond);
                let skip = self.emit(Op::JumpFalse(0));
                self.stmt(then);
                match other {
                    Some(other) => {
                        let over = self.emit(Op::Jump(0));
                        let t = self.here();
                        self.patch(skip, t);
                        self.stmt(other);
                        let t = self.here();
                        self.patch(over, t);
                    }
                    None => {
                        let t = self.here();
                        self.patch(skip, t);
                    }
                }
            }
            Stmt::While(cond, body) => {
                let top = self.here();
                self.expr(cond);
                let exit = self.emit(Op::JumpFalse(0));
                self.loop_body(body, top);
                self.emit(Op::Jump(top));
                let end = self.here();
                self.patch(exit, end);
                self.end_loop(end);
            }
            Stmt::For(init, cond, step, body) => {
                if let Some(init) = init {
                    self.stmt(init);
                }
                let top = self.here();
                let exit = cond.as_ref().map(|c| {
                    self.expr(c);
                    self.emit(Op::JumpFalse(0))
                });
                // `continue` goes to the step.
                self.breaks.push(Vec::new());
                self.continues.push(Vec::new());
                self.stmt(body);
                let step_at = self.here();
                for at in self.continues.pop().unwrap_or_default() {
                    self.patch(at, step_at);
                }
                if let Some(step) = step {
                    self.stmt(step);
                }
                self.emit(Op::Jump(top));
                let end = self.here();
                if let Some(exit) = exit {
                    self.patch(exit, end);
                }
                for at in self.breaks.pop().unwrap_or_default() {
                    self.patch(at, end);
                }
            }
            Stmt::Switch(on, cases, body) => {
                // The value in a hidden local, tested against each label.
                let tmp = self.local(&format!(" switch{}", self.code.len()));
                self.expr(on);
                self.emit(Op::Store { root: Root::Local(tmp), fields: Rc::from([]) });
                let mut to_case = Vec::new();
                for (label, _) in cases {
                    if let Some(label) = label {
                        self.emit(Op::Local(tmp));
                        self.expr(label);
                        self.emit(Op::Bin(BinOp::Eq));
                        let not = self.emit(Op::JumpFalse(0));
                        to_case.push(Some(self.emit(Op::Jump(0))));
                        let t = self.here();
                        self.patch(not, t);
                    } else {
                        to_case.push(None);
                    }
                }
                // No label matched: `default`, or past the end.
                let fallback = self.emit(Op::Jump(0));
                self.breaks.push(Vec::new());
                let mut starts = HashMap::new();
                for (i, s) in body.iter().enumerate() {
                    starts.insert(i, self.here());
                    self.stmt(s);
                }
                let end = self.here();
                let start_of = |i: usize| *starts.get(&i).unwrap_or(&end);
                let mut default = end;
                for ((label, at), jump) in cases.iter().zip(&to_case) {
                    match jump {
                        Some(j) => self.patch(*j, start_of(*at)),
                        None if label.is_none() => default = start_of(*at),
                        None => {}
                    }
                }
                self.patch(fallback, default);
                for at in self.breaks.pop().unwrap_or_default() {
                    self.patch(at, end);
                }
            }
            Stmt::Break => {
                let at = self.emit(Op::Jump(0));
                if let Some(b) = self.breaks.last_mut() {
                    b.push(at);
                }
            }
            Stmt::Continue => {
                let at = self.emit(Op::Jump(0));
                if let Some(c) = self.continues.last_mut() {
                    c.push(at);
                }
            }
            Stmt::Return(e) => match e {
                Some(e) => {
                    self.expr(e);
                    self.emit(Op::Ret);
                }
                None => {
                    self.emit(Op::RetUndef);
                }
            },
            Stmt::Block(b) => {
                for s in b {
                    self.stmt(s);
                }
            }
            Stmt::Empty => {}
        }
    }

    fn loop_body(&mut self, body: &Stmt, top: u32) {
        self.breaks.push(Vec::new());
        self.continues.push(Vec::new());
        self.stmt(body);
        for at in self.continues.pop().unwrap_or_default() {
            self.patch(at, top);
        }
    }

    fn end_loop(&mut self, end: u32) {
        for at in self.breaks.pop().unwrap_or_default() {
            self.patch(at, end);
        }
    }

    /// An expression whose value is dropped; `waittill`'s extra arguments
    /// are variables it sets.
    fn expr_stmt(&mut self, e: &Expr) {
        if let Expr::Call(call) = e
            && let (Callee::Named(None, name), Some(object), false) = (&call.callee, &call.object, call.thread)
        {
            match name.as_str() {
                "waittill" if !call.args.is_empty() => {
                    self.expr(object);
                    self.expr(&call.args[0]);
                    let vars = &call.args[1..];
                    self.emit(Op::Waittill { vars: vars.len() as u8 });
                    // Popped last first.
                    for v in vars.iter().rev() {
                        self.store_top(v);
                    }
                    return;
                }
                "waittillmatch" if call.args.len() >= 2 => {
                    self.expr(object);
                    self.expr(&call.args[0]);
                    self.expr(&call.args[1]);
                    self.emit(Op::WaittillMatch);
                    return;
                }
                _ => {}
            }
        }
        self.expr(e);
        self.emit(Op::Pop);
    }

    /// Store the value on top of the stack into `target`.
    fn store_top(&mut self, target: &Expr) {
        let (root, path) = self.lvalue(target);
        // The value is under the path keys: compute the keys into hidden
        // locals isn't needed for plain names, which is all waittill uses.
        if path.is_empty() {
            self.emit(Op::Store { root, fields: Rc::from([]) });
        } else {
            // Rare: `self waittill("x", self.a)`. Keep it simple: drop it.
            self.emit(Op::Pop);
        }
    }

    /// The path of an assignment's target: its root and keys (true: field).
    fn lvalue<'e>(&mut self, e: &'e Expr) -> (Root, Vec<(bool, &'e Expr)>) {
        match e {
            Expr::Ident(n) => (Root::Local(self.local(n)), Vec::new()),
            Expr::SelfRef => (Root::SelfRef, Vec::new()),
            Expr::Level => (Root::Level, Vec::new()),
            Expr::Game => (Root::Game, Vec::new()),
            Expr::Anim_ => (Root::Anim, Vec::new()),
            Expr::Field(base, _) => {
                let (root, mut path) = self.lvalue(base);
                path.push((true, e));
                (root, path)
            }
            Expr::Index(base, _) => {
                let (root, mut path) = self.lvalue(base);
                path.push((false, e));
                (root, path)
            }
            _ => (Root::Value, Vec::new()),
        }
    }

    fn assign(&mut self, target: &Expr, op: AssignOp, value: &Expr) {
        let (root, path) = self.lvalue(target);
        // A target that isn't a path at all (a call's result) can't be set.
        if matches!(root, Root::Value) && path.is_empty() {
            self.expr(value);
            self.emit(Op::Pop);
            return;
        }
        if let Root::Value = root {
            // The deepest non-path base is evaluated first.
            let mut base = target;
            while let Expr::Field(b, _) | Expr::Index(b, _) = base {
                base = b;
            }
            self.expr(base);
        }
        let mut fields = Vec::new();
        for (field, e) in &path {
            match e {
                Expr::Field(_, name) => {
                    self.emit(Op::Str(name.as_str().into()));
                }
                Expr::Index(_, index) => {
                    self.expr(index);
                }
                _ => {}
            }
            fields.push(*field);
        }
        if op == AssignOp::Set {
            self.expr(value);
        } else {
            self.expr(target);
            self.expr(value);
            self.emit(Op::Bin(match op {
                AssignOp::Add => BinOp::Add,
                AssignOp::Sub => BinOp::Sub,
                AssignOp::Mul => BinOp::Mul,
                AssignOp::Div => BinOp::Div,
                AssignOp::Mod => BinOp::Mod,
                AssignOp::And => BinOp::BitAnd,
                AssignOp::Or => BinOp::BitOr,
                AssignOp::Xor => BinOp::BitXor,
                AssignOp::Shl => BinOp::Shl,
                AssignOp::Shr => BinOp::Shr,
                AssignOp::Set => unreachable!(),
            }));
        }
        self.emit(Op::Store { root, fields: fields.into() });
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Undefined => {
                self.emit(Op::Undef);
            }
            Expr::Int(v) => {
                self.emit(Op::Int(*v));
            }
            Expr::Float(v) => {
                self.emit(Op::Float(*v));
            }
            Expr::Str(s) => {
                self.emit(Op::Str(s.as_str().into()));
            }
            Expr::IStr(s) => {
                self.emit(Op::IStr(s.as_str().into()));
            }
            Expr::Anim(n) => {
                self.emit(Op::Anim(n.as_str().into()));
            }
            Expr::AnimTree => {
                self.emit(Op::AnimTree);
            }
            Expr::Vector(v) => {
                for x in v.iter() {
                    self.expr(x);
                }
                self.emit(Op::Vector);
            }
            Expr::EmptyArray => {
                self.emit(Op::EmptyArray);
            }
            Expr::SelfRef => {
                self.emit(Op::SelfRef);
            }
            Expr::Level => {
                self.emit(Op::Level);
            }
            Expr::Game => {
                self.emit(Op::Game);
            }
            Expr::Anim_ => {
                self.emit(Op::AnimObj);
            }
            Expr::Ident(n) => match n.as_str() {
                "true" if !self.locals.contains_key("true") => {
                    self.emit(Op::Int(1));
                }
                "false" if !self.locals.contains_key("false") => {
                    self.emit(Op::Int(0));
                }
                _ => {
                    let slot = self.local(n);
                    self.emit(Op::Local(slot));
                }
            },
            Expr::Field(base, name) => {
                self.expr(base);
                self.emit(Op::Field(name.as_str().into()));
            }
            Expr::Index(base, index) => {
                self.expr(base);
                self.expr(index);
                self.emit(Op::Index);
            }
            Expr::FuncRef(path, name) => {
                self.emit(Op::FuncRef(Target::Name(path.as_deref().map(Into::into), name.as_str().into())));
            }
            Expr::Call(call) => self.call(call),
            Expr::Binary(BinOp::And, a, b) => {
                self.expr(a);
                let j = self.emit(Op::AndJump(0));
                self.expr(b);
                self.emit(Op::Bool);
                let t = self.here();
                self.patch(j, t);
            }
            Expr::Binary(BinOp::Or, a, b) => {
                self.expr(a);
                let j = self.emit(Op::OrJump(0));
                self.expr(b);
                self.emit(Op::Bool);
                let t = self.here();
                self.patch(j, t);
            }
            Expr::Binary(op, a, b) => {
                self.expr(a);
                self.expr(b);
                self.emit(Op::Bin(*op));
            }
            Expr::Not(a) => {
                self.expr(a);
                self.emit(Op::Not);
            }
            Expr::BitNot(a) => {
                self.expr(a);
                self.emit(Op::BitNot);
            }
            Expr::Neg(a) => {
                self.expr(a);
                self.emit(Op::Neg);
            }
        }
    }

    fn call(&mut self, call: &Call) {
        let line = call.line;
        self.line = line;
        let method = call.object.is_some();
        if let Some(o) = &call.object {
            self.expr(o);
        }
        for a in &call.args {
            self.expr(a);
        }
        let argc = call.args.len() as u8;
        match &call.callee {
            Callee::Named(path, name) => {
                let target = Target::Name(path.as_deref().map(Into::into), name.as_str().into());
                self.emit(Op::Call { target, argc, method, thread: call.thread, line });
            }
            Callee::Pointer(ptr) => {
                self.expr(ptr);
                self.emit(Op::CallPtr { argc, method, thread: call.thread, line });
            }
        }
    }
}
