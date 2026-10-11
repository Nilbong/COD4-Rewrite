//! The interpreter: scripts compiled to a stack machine, run as CoD's
//! threads (`thread`, `wait`, `waittill`/`notify`/`endon`), with the
//! game's builtins provided by a [`Host`].

pub mod compile;
mod std_lib;
pub use std_lib::{angle_vectors, vector_to_angles};
pub mod value;

use crate::ast::BinOp;
use compile::{Compiled, Op, Root, Target};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
pub use value::{Array, FuncId, Key, ObjId, Value};

/// What the game provides: builtins, and entities' engine fields
/// (`origin`, `targetname`...).
pub trait Host {
    /// A builtin function or method (`this` for `ent foo()`). `Err` for an
    /// unknown builtin (its name) or a script error.
    fn call(&mut self, vm: &mut Vm, name: &str, this: Option<&Value>, args: &[Value]) -> Result<Value, String>;
    /// An entity's engine field, if it has one by that name.
    fn get_field(&mut self, vm: &mut Vm, entity: u64, name: &str) -> Option<Value>;
    /// Set an engine field; false if it isn't one (a script field then).
    fn set_field(&mut self, vm: &mut Vm, entity: u64, name: &str, value: &Value) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjKind {
    Struct,
    Level,
    Game,
    Anim,
    /// A game entity, by the host's id.
    Entity(u64),
}

pub struct Object {
    pub kind: ObjKind,
    pub fields: HashMap<Rc<str>, Value>,
}

/// One script: its functions by name, and its includes.
struct ScriptInfo {
    funcs: HashMap<Rc<str>, FuncId>,
    includes: Vec<Rc<str>>,
    animtree: Option<Rc<str>>,
}

struct Frame {
    func: FuncId,
    pc: usize,
    locals: Vec<Value>,
    this: Value,
}

#[derive(Clone, Debug)]
enum Wake {
    Ready,
    /// Until this time (s).
    Sleep(f64),
    /// After every other thread this frame.
    FrameEnd,
    /// A notify on (object, event); with `matching`, only one whose first
    /// argument is that.
    Notify(ObjId, Rc<str>, Option<Value>),
    Done,
}

struct Thread {
    frames: Vec<Frame>,
    stack: Vec<Value>,
    wake: Wake,
    /// Ends when one of these is notified.
    endons: Vec<(ObjId, Rc<str>)>,
    /// What to push when it resumes (a notify's arguments).
    resume_with: Option<(u8, Vec<Value>)>,
    vars_wanted: u8,
}

pub struct Vm {
    funcs: Vec<Compiled>,
    scripts: HashMap<Rc<str>, ScriptInfo>,
    /// Each function's script.
    func_script: Vec<Rc<str>>,
    objects: Vec<Option<Object>>,
    free: Vec<u32>,
    entities: HashMap<u64, ObjId>,
    threads: Vec<Thread>,
    ready: VecDeque<usize>,
    pub level: ObjId,
    pub game: ObjId,
    pub anim: ObjId,
    /// Seconds.
    pub time: f64,
    /// Script errors so far (message, function, line).
    pub errors: Vec<String>,
    /// Builtins scripts called that the host didn't know.
    pub missing: HashMap<String, u32>,
    rng: u64,
}

/// A thread may run this many ops before it's taken for stuck.
const OP_BUDGET: u32 = 4_000_000;

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Vm {
        let mut vm = Vm {
            funcs: Vec::new(),
            scripts: HashMap::new(),
            func_script: Vec::new(),
            objects: Vec::new(),
            free: Vec::new(),
            entities: HashMap::new(),
            threads: Vec::new(),
            ready: VecDeque::new(),
            level: ObjId(0),
            game: ObjId(0),
            anim: ObjId(0),
            time: 0.0,
            errors: Vec::new(),
            missing: HashMap::new(),
            rng: 0x2545_F491_4F6C_DD1D,
        };
        vm.level = vm.alloc(ObjKind::Level);
        vm.game = vm.alloc(ObjKind::Game);
        vm.anim = vm.alloc(ObjKind::Anim);
        vm
    }

    // ---- Loading ----

    /// Add a script by its path (`maps/_utility.gsc` or `maps\_utility`).
    pub fn add_script(&mut self, path: &str, src: &str) -> anyhow::Result<()> {
        let script = crate::parse(src)?;
        let key: Rc<str> = script_key(path).into();
        let mut info = ScriptInfo {
            funcs: HashMap::new(),
            includes: script.includes.iter().map(|s| Rc::from(s.as_str())).collect(),
            animtree: script.animtree.as_deref().map(Into::into),
        };
        for f in &script.functions {
            let id = FuncId(self.funcs.len() as u32);
            self.funcs.push(compile::function(f, &key));
            self.func_script.push(key.clone());
            info.funcs.insert(f.name.as_str().into(), id);
        }
        self.scripts.insert(key, info);
        Ok(())
    }

    pub fn has_script(&self, path: &str) -> bool {
        self.scripts.contains_key(script_key(path).as_str())
    }

    /// Resolve every call to a function or a builtin (after all scripts are
    /// in). Returns calls to scripts that aren't loaded.
    pub fn link(&mut self) -> Vec<String> {
        let mut unresolved = Vec::new();
        for i in 0..self.funcs.len() {
            let script = self.func_script[i].clone();
            for j in 0..self.funcs[i].code.len() {
                let t = match &self.funcs[i].code[j] {
                    Op::Call { target: Target::Name(p, n), .. } | Op::FuncRef(Target::Name(p, n)) => (p.clone(), n.clone()),
                    _ => continue,
                };
                let resolved = match self.resolve(&script, t.0.as_deref(), &t.1) {
                    Some(f) => Target::Func(f),
                    None => {
                        if let Some(p) = &t.0 {
                            unresolved.push(format!("{p}::{}", t.1));
                        }
                        Target::Builtin(t.1.clone())
                    }
                };
                match &mut self.funcs[i].code[j] {
                    Op::Call { target, .. } | Op::FuncRef(target) => *target = resolved,
                    _ => {}
                }
            }
        }
        unresolved.sort();
        unresolved.dedup();
        unresolved
    }

    fn resolve(&self, script: &str, path: Option<&str>, name: &str) -> Option<FuncId> {
        match path {
            Some(p) => self.scripts.get(script_key(p).as_str())?.funcs.get(name).copied(),
            None => {
                let s = self.scripts.get(script)?;
                s.funcs.get(name).copied().or_else(|| s.includes.iter().find_map(|inc| self.scripts.get(inc.as_ref())?.funcs.get(name).copied()))
            }
        }
    }

    /// A function by script and name.
    pub fn func(&self, script: &str, name: &str) -> Option<FuncId> {
        self.scripts.get(script_key(script).as_str())?.funcs.get(name).copied()
    }

    pub fn func_name(&self, f: FuncId) -> &str {
        &self.funcs[f.0 as usize].name
    }

    // ---- Objects ----

    pub fn alloc(&mut self, kind: ObjKind) -> ObjId {
        let obj = Object { kind, fields: HashMap::new() };
        match self.free.pop() {
            Some(i) => {
                self.objects[i as usize] = Some(obj);
                ObjId(i)
            }
            None => {
                self.objects.push(Some(obj));
                ObjId(self.objects.len() as u32 - 1)
            }
        }
    }

    pub fn spawn_struct(&mut self) -> Value {
        Value::Object(self.alloc(ObjKind::Struct))
    }

    /// The script object for a game entity (made on first use).
    pub fn entity(&mut self, id: u64) -> Value {
        if let Some(&o) = self.entities.get(&id) {
            return Value::Object(o);
        }
        let o = self.alloc(ObjKind::Entity(id));
        self.entities.insert(id, o);
        Value::Object(o)
    }

    /// The script object of a game entity, if it has one yet.
    pub fn entity_obj(&self, id: u64) -> Option<ObjId> {
        self.entities.get(&id).copied()
    }

    pub fn entity_of(&self, v: &Value) -> Option<u64> {
        match self.object(v.as_obj()?)?.kind {
            ObjKind::Entity(id) => Some(id),
            _ => None,
        }
    }

    pub fn object(&self, o: ObjId) -> Option<&Object> {
        self.objects.get(o.0 as usize)?.as_ref()
    }

    pub fn object_mut(&mut self, o: ObjId) -> Option<&mut Object> {
        self.objects.get_mut(o.0 as usize)?.as_mut()
    }

    /// A script field (not an engine one).
    pub fn field(&self, o: ObjId, name: &str) -> Value {
        self.object(o).and_then(|ob| ob.fields.get(name).cloned()).unwrap_or_default()
    }

    pub fn set_field(&mut self, o: ObjId, name: &str, v: Value) {
        if let Some(ob) = self.object_mut(o) {
            if v.is_defined() {
                ob.fields.insert(name.into(), v);
            } else {
                ob.fields.remove(name);
            }
        }
    }

    /// A game entity is gone: its threads end (after its `death`-style
    /// notifies, which the host sends first), and its object is freed.
    pub fn free_entity(&mut self, id: u64) {
        let Some(o) = self.entities.remove(&id) else { return };
        self.notify(o, "entityshutdown", Vec::new());
        for t in &mut self.threads {
            if t.frames.first().is_some_and(|f| f.this.as_obj() == Some(o)) {
                t.wake = Wake::Done;
            }
        }
        self.objects[o.0 as usize] = None;
        self.free.push(o.0);
    }

    // ---- Threads ----

    /// Start `f` as a thread on `this` with `args` (it runs at the next
    /// [`Vm::run`]).
    pub fn spawn_thread(&mut self, f: FuncId, this: Value, args: Vec<Value>) {
        let frame = self.frame(f, this, args);
        self.threads.push(Thread { frames: vec![frame], stack: Vec::new(), wake: Wake::Ready, endons: Vec::new(), resume_with: None, vars_wanted: 0 });
        self.ready.push_back(self.threads.len() - 1);
    }

    fn frame(&self, f: FuncId, this: Value, mut args: Vec<Value>) -> Frame {
        let c = &self.funcs[f.0 as usize];
        args.truncate(c.params);
        args.resize(c.locals.max(c.params), Value::Undefined);
        Frame { func: f, pc: 0, locals: args, this }
    }

    /// Run `f` on `this` now, until it returns or waits (the engine's
    /// calls into script, like an AI's `aitype` script as it spawns).
    pub fn call_now(&mut self, host: &mut dyn Host, f: FuncId, this: Value, args: Vec<Value>) {
        let frame = self.frame(f, this, args);
        self.threads.push(Thread { frames: vec![frame], stack: Vec::new(), wake: Wake::Ready, endons: Vec::new(), resume_with: None, vars_wanted: 0 });
        let ti = self.threads.len() - 1;
        self.exec(host, ti);
        // Threads it started run now too, as they would have.
        while let Some(i) = self.ready.front().copied() {
            if i < ti {
                break;
            }
            self.ready.pop_front();
            if matches!(self.threads[i].wake, Wake::Ready) {
                self.exec(host, i);
            }
        }
    }

    /// `object notify(event, args...)`: threads waiting on it resume (this
    /// frame), and threads that end on it end.
    pub fn notify(&mut self, o: ObjId, event: &str, args: Vec<Value>) {
        for (i, t) in self.threads.iter_mut().enumerate() {
            if matches!(t.wake, Wake::Done) {
                continue;
            }
            if t.endons.iter().any(|(eo, ev)| *eo == o && **ev == *event) {
                t.wake = Wake::Done;
                continue;
            }
            if let Wake::Notify(wo, ev, matching) = &t.wake
                && *wo == o
                && **ev == *event
            {
                if let Some(m) = matching
                    && !args.first().is_some_and(|a| equal(a, m))
                {
                    continue;
                }
                t.wake = Wake::Ready;
                t.resume_with = Some((t.vars_wanted, args.clone()));
                self.ready.push_back(i);
            }
        }
    }

    pub fn live_threads(&self) -> usize {
        self.threads.iter().filter(|t| !matches!(t.wake, Wake::Done)).count()
    }

    /// Run every thread that can run at `now` (seconds), until all wait.
    pub fn run(&mut self, host: &mut dyn Host, now: f64) {
        self.time = now;
        for (i, t) in self.threads.iter_mut().enumerate() {
            if let Wake::Sleep(until) = t.wake
                && until <= now + 1e-4
            {
                t.wake = Wake::Ready;
                self.ready.push_back(i);
            }
        }
        loop {
            while let Some(i) = self.ready.pop_front() {
                if matches!(self.threads[i].wake, Wake::Ready) {
                    self.exec(host, i);
                }
            }
            // `waittillframeend`: after everything else this frame.
            let mut any = false;
            for (i, t) in self.threads.iter_mut().enumerate() {
                if matches!(t.wake, Wake::FrameEnd) {
                    t.wake = Wake::Ready;
                    self.ready.push_back(i);
                    any = true;
                }
            }
            if !any {
                break;
            }
        }
        // Nothing is queued now: finished threads can go.
        self.threads.retain(|t| !matches!(t.wake, Wake::Done));
    }

    fn error(&mut self, ti: usize, msg: String) {
        let t = &self.threads[ti];
        let place = t.frames.last().map_or(String::new(), |f| {
            let c = &self.funcs[f.func.0 as usize];
            format!("{} line {}", c.name, c.lines.get(f.pc.saturating_sub(1)).copied().unwrap_or(0))
        });
        let line = format!("{place}: {msg}");
        if self.errors.len() < 10_000 {
            self.errors.push(line);
        }
        self.threads[ti].wake = Wake::Done;
    }

    fn exec(&mut self, host: &mut dyn Host, ti: usize) {
        if let Some((n, args)) = self.threads[ti].resume_with.take() {
            for k in 0..n as usize {
                let v = args.get(k).cloned().unwrap_or_default();
                self.threads[ti].stack.push(v);
            }
        }
        let mut budget = OP_BUDGET;
        loop {
            budget -= 1;
            if budget == 0 {
                self.error(ti, "too many operations without a wait (infinite loop?)".into());
                return;
            }
            let t = &mut self.threads[ti];
            let Some(frame) = t.frames.last_mut() else {
                t.wake = Wake::Done;
                return;
            };
            let code = &self.funcs[frame.func.0 as usize].code;
            let Some(op) = code.get(frame.pc).cloned() else {
                t.wake = Wake::Done;
                return;
            };
            frame.pc += 1;
            match self.step(host, ti, op) {
                Ok(true) => {}
                Ok(false) => return,
                Err(e) => {
                    self.error(ti, e);
                    return;
                }
            }
            if matches!(self.threads[ti].wake, Wake::Done) {
                return;
            }
        }
    }

    fn pop(&mut self, ti: usize) -> Value {
        self.threads[ti].stack.pop().unwrap_or_default()
    }

    fn push(&mut self, ti: usize, v: Value) {
        self.threads[ti].stack.push(v);
    }

    fn this(&self, ti: usize) -> Value {
        self.threads[ti].frames.last().map(|f| f.this.clone()).unwrap_or_default()
    }

    /// One op; `Ok(false)` when the thread waits or ends.
    fn step(&mut self, host: &mut dyn Host, ti: usize, op: Op) -> Result<bool, String> {
        match op {
            Op::Undef => self.push(ti, Value::Undefined),
            Op::Int(v) => self.push(ti, Value::Int(v)),
            Op::Float(v) => self.push(ti, Value::Float(v)),
            Op::Str(s) => self.push(ti, Value::Str(s)),
            Op::IStr(s) => self.push(ti, Value::IStr(s)),
            Op::Anim(n) => {
                let tree = self.current_tree(ti);
                self.push(ti, Value::Anim(n, tree));
            }
            Op::AnimTree => {
                let tree = self.current_tree(ti);
                self.push(ti, Value::AnimTree(tree));
            }
            Op::EmptyArray => self.push(ti, Value::Array(Rc::new(Array::default()))),
            Op::Vector => {
                let z = self.pop(ti);
                let y = self.pop(ti);
                let x = self.pop(ti);
                let (Some(x), Some(y), Some(z)) = (x.as_f32(), y.as_f32(), z.as_f32()) else {
                    return Err("vector of non-numbers".into());
                };
                self.push(ti, Value::Vector([x, y, z]));
            }
            Op::SelfRef => {
                let v = self.this(ti);
                self.push(ti, v);
            }
            Op::Level => self.push(ti, Value::Object(self.level)),
            Op::Game => self.push(ti, Value::Object(self.game)),
            Op::AnimObj => self.push(ti, Value::Object(self.anim)),
            Op::Local(i) => {
                let v = self.threads[ti].frames.last().and_then(|f| f.locals.get(i as usize).cloned()).unwrap_or_default();
                self.push(ti, v);
            }
            Op::Field(name) => {
                let base = self.pop(ti);
                let v = self.get_field(host, &base, &name)?;
                self.push(ti, v);
            }
            Op::Index => {
                let index = self.pop(ti);
                let base = self.pop(ti);
                let v = match &base {
                    Value::Array(a) => index.key().and_then(|k| a.get(&k).cloned()).unwrap_or_default(),
                    Value::Vector(v) => index.as_i32().and_then(|i| v.get(i as usize)).map_or(Value::Undefined, |x| Value::Float(*x)),
                    Value::Str(s) => index.as_i32().and_then(|i| s.chars().nth(i as usize)).map_or(Value::Undefined, |c| Value::Str(c.to_string().into())),
                    Value::Object(o) => match index.as_str() {
                        Some(name) => self.get_field(host, &Value::Object(*o), name)?,
                        None => Value::Undefined,
                    },
                    Value::Undefined => return Err("index into undefined".into()),
                    other => return Err(format!("index into {}", other.type_name())),
                };
                self.push(ti, v);
            }
            Op::FuncRef(t) => {
                let v = match t {
                    Target::Func(f) => Value::Func(f),
                    Target::Builtin(n) | Target::Name(_, n) => Value::Builtin(n),
                };
                self.push(ti, v);
            }
            Op::Bin(op) => {
                let b = self.pop(ti);
                let a = self.pop(ti);
                let v = binary(op, &a, &b)?;
                self.push(ti, v);
            }
            Op::Not => {
                let a = self.pop(ti);
                self.push(ti, Value::Int(i32::from(!a.truthy())));
            }
            Op::BitNot => {
                let a = self.pop(ti);
                self.push(ti, Value::Int(!a.as_i32().ok_or("~ of a non-number")?));
            }
            Op::Neg => {
                let a = self.pop(ti);
                let v = match a {
                    Value::Int(v) => Value::Int(-v),
                    Value::Float(v) => Value::Float(-v),
                    Value::Vector(v) => Value::Vector([-v[0], -v[1], -v[2]]),
                    o => return Err(format!("- of {}", o.type_name())),
                };
                self.push(ti, v);
            }
            Op::Jump(to) => self.jump(ti, to),
            Op::JumpFalse(to) => {
                if !self.pop(ti).truthy() {
                    self.jump(ti, to);
                }
            }
            Op::AndJump(to) => {
                let a = self.pop(ti);
                if !a.truthy() {
                    self.push(ti, Value::Int(0));
                    self.jump(ti, to);
                }
            }
            Op::OrJump(to) => {
                let a = self.pop(ti);
                if a.truthy() {
                    self.push(ti, Value::Int(1));
                    self.jump(ti, to);
                }
            }
            Op::Bool => {
                let a = self.pop(ti);
                self.push(ti, Value::Int(i32::from(a.truthy())));
            }
            Op::Call { target, argc, method, thread, line: _ } => {
                let args = self.pop_args(ti, argc);
                let this = if method { self.pop(ti) } else { self.this(ti) };
                match target {
                    Target::Func(f) => return Ok(self.call_func(ti, f, this, args, thread)),
                    Target::Builtin(name) | Target::Name(_, name) => {
                        let v = self.builtin(host, ti, &name, method.then_some(&this), &args)?;
                        self.push(ti, if thread { Value::Undefined } else { v });
                        // A builtin may have ended this thread (an endon it
                        // notified).
                        if matches!(self.threads[ti].wake, Wake::Done) {
                            return Ok(false);
                        }
                    }
                }
            }
            Op::CallPtr { argc, method, thread, line: _ } => {
                let ptr = self.pop(ti);
                let args = self.pop_args(ti, argc);
                let this = if method { self.pop(ti) } else { self.this(ti) };
                match ptr {
                    Value::Func(f) => return Ok(self.call_func(ti, f, this, args, thread)),
                    Value::Builtin(name) => {
                        let v = self.builtin(host, ti, &name, method.then_some(&this), &args)?;
                        self.push(ti, if thread { Value::Undefined } else { v });
                    }
                    other => return Err(format!("call of {} as a function", other.type_name())),
                }
            }
            Op::Pop => {
                self.pop(ti);
            }
            Op::Dup => {
                let v = self.threads[ti].stack.last().cloned().unwrap_or_default();
                self.push(ti, v);
            }
            Op::Ret | Op::RetUndef => {
                let v = if matches!(op, Op::Ret) { self.pop(ti) } else { Value::Undefined };
                let t = &mut self.threads[ti];
                t.frames.pop();
                if t.frames.is_empty() {
                    t.wake = Wake::Done;
                    return Ok(false);
                }
                t.stack.push(v);
            }
            Op::Wait => {
                let secs = self.pop(ti).as_f32().ok_or("wait of a non-number")?;
                // `wait 0` still waits a frame; never less than one.
                self.threads[ti].wake = Wake::Sleep(self.time + secs.max(0.001) as f64);
                return Ok(false);
            }
            Op::WaitFrameEnd => {
                self.threads[ti].wake = Wake::FrameEnd;
                return Ok(false);
            }
            Op::Waittill { vars } => {
                let event = self.pop(ti);
                let obj = self.pop(ti);
                let o = obj.as_obj().ok_or_else(|| format!("waittill on {}", obj.type_name()))?;
                let ev: Rc<str> = event.as_str().ok_or("waittill without an event name")?.into();
                let t = &mut self.threads[ti];
                t.vars_wanted = vars;
                t.wake = Wake::Notify(o, ev, None);
                return Ok(false);
            }
            Op::WaittillMatch => {
                let matching = self.pop(ti);
                let event = self.pop(ti);
                let obj = self.pop(ti);
                let o = obj.as_obj().ok_or_else(|| format!("waittillmatch on {}", obj.type_name()))?;
                let ev: Rc<str> = event.as_str().ok_or("waittillmatch without an event name")?.into();
                let t = &mut self.threads[ti];
                t.vars_wanted = 0;
                t.wake = Wake::Notify(o, ev, Some(matching));
                return Ok(false);
            }
            Op::Store { root, fields } => {
                let value = self.pop(ti);
                let mut keys: Vec<Value> = (0..fields.len()).map(|_| self.pop(ti)).collect();
                keys.reverse();
                let path: Vec<(bool, Value)> = fields.iter().copied().zip(keys).collect();
                self.store(host, ti, root, &path, value)?;
            }
        }
        Ok(true)
    }

    fn current_tree(&self, ti: usize) -> Rc<str> {
        let f = self.threads[ti].frames.last().map(|f| f.func.0 as usize).unwrap_or(0);
        let script = &self.func_script[f];
        self.scripts.get(script).and_then(|s| s.animtree.clone()).unwrap_or_else(|| "".into())
    }

    fn jump(&mut self, ti: usize, to: u32) {
        if let Some(f) = self.threads[ti].frames.last_mut() {
            f.pc = to as usize;
        }
    }

    fn pop_args(&mut self, ti: usize, argc: u8) -> Vec<Value> {
        let st = &mut self.threads[ti].stack;
        let at = st.len().saturating_sub(argc as usize);
        st.split_off(at)
    }

    /// Call a script function: on this thread, or as a new one.
    fn call_func(&mut self, ti: usize, f: FuncId, this: Value, args: Vec<Value>, thread: bool) -> bool {
        if thread {
            // The call's value (none) for the caller to drop.
            self.threads[ti].stack.push(Value::Undefined);
            let frame = self.frame(f, this, args);
            self.threads.push(Thread { frames: vec![frame], stack: Vec::new(), wake: Wake::Ready, endons: Vec::new(), resume_with: None, vars_wanted: 0 });
            // A new thread runs at once, before its caller goes on (CoD's).
            let new = self.threads.len() - 1;
            self.ready.push_front(ti);
            self.ready.push_front(new);
            false
        } else {
            if self.threads[ti].frames.len() > 400 {
                self.threads[ti].wake = Wake::Done;
                self.errors.push(format!("{}: call stack too deep", self.funcs[f.0 as usize].name));
                return false;
            }
            let frame = self.frame(f, this, args);
            self.threads[ti].frames.push(frame);
            true
        }
    }

    fn get_field(&mut self, host: &mut dyn Host, base: &Value, name: &str) -> Result<Value, String> {
        Ok(match base {
            Value::Object(o) => {
                let o = *o;
                if let Some(ObjKind::Entity(id)) = self.object(o).map(|ob| ob.kind)
                    && let Some(v) = host.get_field(self, id, name)
                {
                    return Ok(v);
                }
                self.field(o, name)
            }
            Value::Array(a) if name == "size" => Value::Int(a.len() as i32),
            Value::Str(s) | Value::IStr(s) if name == "size" => Value::Int(s.chars().count() as i32),
            Value::Undefined => return Err(format!("field {name} of undefined")),
            Value::Vector(_) => return Err(format!("field {name} of a vector")),
            _ => Value::Undefined,
        })
    }

    fn store(&mut self, host: &mut dyn Host, ti: usize, root: Root, path: &[(bool, Value)], value: Value) -> Result<(), String> {
        let obj_root = match root {
            Root::Local(i) => {
                let mut slot = std::mem::take(&mut self.threads[ti].frames.last_mut().ok_or("no frame")?.locals[i as usize]);
                let r = self.assign_in(host, &mut slot, path, value);
                self.threads[ti].frames.last_mut().ok_or("no frame")?.locals[i as usize] = slot;
                return r;
            }
            Root::SelfRef => {
                let mut v = self.this(ti);
                if !v.is_defined() || path.is_empty() {
                    // `self = x` isn't a thing; the self of a thread with
                    // none becomes nothing useful.
                    return self.assign_in(host, &mut v, path, value);
                }
                v.as_obj().ok_or("self isn't an object")?
            }
            Root::Level => self.level,
            Root::Game => self.game,
            Root::Anim => self.anim,
            Root::Value => {
                let v = self.pop(ti);
                v.as_obj().ok_or_else(|| format!("field of {}", v.type_name()))?
            }
        };
        if path.is_empty() {
            return Err("assignment to an object itself".into());
        }
        self.assign_obj(host, obj_root, path, value)
    }

    fn assign_obj(&mut self, host: &mut dyn Host, o: ObjId, path: &[(bool, Value)], value: Value) -> Result<(), String> {
        let (_, key) = &path[0];
        let name: Rc<str> = match key {
            Value::Str(s) | Value::IStr(s) => s.clone(),
            other => other.display().into(),
        };
        let rest = &path[1..];
        if rest.is_empty() {
            if let Some(ObjKind::Entity(id)) = self.object(o).map(|ob| ob.kind)
                && host.set_field(self, id, &name, &value)
            {
                return Ok(());
            }
            self.set_field(o, &name, value);
            return Ok(());
        }
        let mut slot = self.field(o, &name);
        if let Value::Object(inner) = slot {
            return self.assign_obj(host, inner, rest, value);
        }
        if let Some(ObjKind::Entity(id)) = self.object(o).map(|ob| ob.kind)
            && !slot.is_defined()
            && let Some(v) = host.get_field(self, id, &name)
        {
            slot = v;
        }
        self.assign_in(host, &mut slot, rest, value)?;
        self.set_field(o, &name, slot);
        Ok(())
    }

    fn assign_in(&mut self, host: &mut dyn Host, place: &mut Value, path: &[(bool, Value)], value: Value) -> Result<(), String> {
        if path.is_empty() {
            *place = value;
            return Ok(());
        }
        if let Value::Object(o) = place {
            return self.assign_obj(host, *o, path, value);
        }
        if !place.is_defined() {
            *place = Value::Array(Rc::new(Array::default()));
        }
        let Value::Array(a) = place else {
            return Err(format!("index into {}", place.type_name()));
        };
        let (_, key) = &path[0];
        let key = key.key().ok_or_else(|| format!("array key of {}", key.type_name()))?;
        let a = Rc::make_mut(a);
        if path.len() == 1 {
            a.set(key, value);
            return Ok(());
        }
        let mut slot = std::mem::take(a.get_mut_or_insert(key.clone()));
        let r = self.assign_in(host, &mut slot, &path[1..], value);
        a.set(key, slot);
        r
    }

    fn builtin(&mut self, host: &mut dyn Host, ti: usize, name: &str, this: Option<&Value>, args: &[Value]) -> Result<Value, String> {
        match name {
            "notify" => {
                let o = this.and_then(Value::as_obj).ok_or("notify on a non-object")?;
                let ev = args.first().and_then(Value::as_str).ok_or("notify without an event")?.to_owned();
                self.notify(o, &ev, args[1..].to_vec());
                return Ok(Value::Undefined);
            }
            "endon" => {
                let o = this.and_then(Value::as_obj).ok_or("endon on a non-object")?;
                let ev = args.first().and_then(Value::as_str).ok_or("endon without an event")?;
                self.threads[ti].endons.push((o, ev.into()));
                return Ok(Value::Undefined);
            }
            _ => {}
        }
        if let Some(v) = std_lib::call(self, name, this, args) {
            return v;
        }
        match host.call(self, name, this, args) {
            Ok(v) => Ok(v),
            Err(e) if e == name => {
                *self.missing.entry(name.to_owned()).or_default() += 1;
                Ok(Value::Undefined)
            }
            Err(e) => Err(format!("{name}: {e}")),
        }
    }

    /// 0..n.
    pub fn random_int(&mut self, n: u32) -> u32 {
        (self.random() % n.max(1) as u64) as u32
    }

    pub(crate) fn random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }
}

/// `maps/_utility.gsc` and `maps\_utility` alike, lower case.
pub fn script_key(path: &str) -> String {
    path.trim_end_matches(".gsc").replace('/', "\\").to_ascii_lowercase()
}

pub fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Undefined, Value::Undefined) => true,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Str(x) | Value::IStr(x), Value::Str(y) | Value::IStr(y)) => x == y,
        (Value::Vector(x), Value::Vector(y)) => x == y,
        (Value::Object(x), Value::Object(y)) => x == y,
        (Value::Func(x), Value::Func(y)) => x == y,
        (Value::Anim(x, _), Value::Anim(y, _)) => x == y,
        (Value::Array(x), Value::Array(y)) => Rc::ptr_eq(x, y),
        _ => match (a.as_f32(), b.as_f32()) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

fn binary(op: BinOp, a: &Value, b: &Value) -> Result<Value, String> {
    use Value::*;
    let num = |x: &Value| x.as_f32();
    Ok(match op {
        BinOp::Eq => Int(i32::from(equal(a, b))),
        BinOp::Ne => Int(i32::from(!equal(a, b))),
        BinOp::Add => match (a, b) {
            (Int(x), Int(y)) => Int(x.wrapping_add(*y)),
            (Str(_) | IStr(_), _) | (_, Str(_) | IStr(_)) => Str(format!("{}{}", a.display(), b.display()).into()),
            (Vector(x), Vector(y)) => Vector([x[0] + y[0], x[1] + y[1], x[2] + y[2]]),
            _ => Float(num(a).ok_or_else(|| mismatch("+", a, b))? + num(b).ok_or_else(|| mismatch("+", a, b))?),
        },
        BinOp::Sub => match (a, b) {
            (Int(x), Int(y)) => Int(x.wrapping_sub(*y)),
            (Vector(x), Vector(y)) => Vector([x[0] - y[0], x[1] - y[1], x[2] - y[2]]),
            _ => Float(num(a).ok_or_else(|| mismatch("-", a, b))? - num(b).ok_or_else(|| mismatch("-", a, b))?),
        },
        BinOp::Mul => match (a, b) {
            (Int(x), Int(y)) => Int(x.wrapping_mul(*y)),
            (Vector(v), s) | (s, Vector(v)) if s.as_f32().is_some() => {
                let s = s.as_f32().unwrap_or(0.0);
                Vector([v[0] * s, v[1] * s, v[2] * s])
            }
            (Vector(x), Vector(y)) => Vector([x[0] * y[0], x[1] * y[1], x[2] * y[2]]),
            _ => Float(num(a).ok_or_else(|| mismatch("*", a, b))? * num(b).ok_or_else(|| mismatch("*", a, b))?),
        },
        BinOp::Div => match (a, b) {
            (Vector(v), s) if s.as_f32().is_some() => {
                let s = s.as_f32().unwrap_or(1.0);
                if s == 0.0 {
                    return Err("divide by 0".into());
                }
                Vector([v[0] / s, v[1] / s, v[2] / s])
            }
            _ => {
                let (x, y) = (num(a).ok_or_else(|| mismatch("/", a, b))?, num(b).ok_or_else(|| mismatch("/", a, b))?);
                if y == 0.0 {
                    return Err("divide by 0".into());
                }
                Float(x / y)
            }
        },
        BinOp::Mod => match (a, b) {
            (Int(x), Int(y)) => {
                if *y == 0 {
                    return Err("divide by 0".into());
                }
                Int(x.wrapping_rem(*y))
            }
            _ => Float(num(a).ok_or_else(|| mismatch("%", a, b))? % num(b).ok_or_else(|| mismatch("%", a, b))?),
        },
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
            let (x, y) = (num(a).ok_or_else(|| mismatch("compare", a, b))?, num(b).ok_or_else(|| mismatch("compare", a, b))?);
            Int(i32::from(match op {
                BinOp::Lt => x < y,
                BinOp::Gt => x > y,
                BinOp::Le => x <= y,
                _ => x >= y,
            }))
        }
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
            let (x, y) = (a.as_i32().ok_or_else(|| mismatch("bit op", a, b))?, b.as_i32().ok_or_else(|| mismatch("bit op", a, b))?);
            Int(match op {
                BinOp::BitAnd => x & y,
                BinOp::BitOr => x | y,
                BinOp::BitXor => x ^ y,
                BinOp::Shl => x.wrapping_shl(y as u32),
                _ => x.wrapping_shr(y as u32),
            })
        }
        BinOp::And => Int(i32::from(a.truthy() && b.truthy())),
        BinOp::Or => Int(i32::from(a.truthy() || b.truthy())),
    })
}

fn mismatch(op: &str, a: &Value, b: &Value) -> String {
    format!("{op} of {} and {}", a.type_name(), b.type_name())
}
