//! The parsed script. Names are lower case (GSC ignores case in them).

#[derive(Debug, Default)]
pub struct Script {
    /// `#include`d scripts, whose functions this one calls unqualified.
    pub includes: Vec<String>,
    /// `#using_animtree("name")`: the tree `%anim` references come from.
    pub animtree: Option<String>,
    pub functions: Vec<Function>,
}

#[derive(Debug)]
pub struct Function {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    pub line: u32,
}

#[derive(Debug)]
pub enum Stmt {
    Expr(Expr, u32),
    Assign(Expr, AssignOp, Expr, u32),
    /// `x++` / `x--`.
    Step(Expr, bool, u32),
    Wait(Expr, u32),
    WaitFrameEnd,
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Box<Stmt>>, Box<Stmt>),
    /// The cases' labels (`None` for `default`) and where in `body` each
    /// starts; cases fall through.
    Switch(Expr, Vec<(Option<Expr>, usize)>, Vec<Stmt>),
    Break,
    Continue,
    Return(Option<Expr>),
    Block(Vec<Stmt>),
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    And,
    Or,
    Xor,
    Shl,
    Shr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    BitOr,
    BitXor,
    BitAnd,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Shl,
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug)]
pub enum Expr {
    Undefined,
    Int(i32),
    Float(f32),
    Str(String),
    /// `&"KEY"`.
    IStr(String),
    /// `%name`: an animation in the script's tree.
    Anim(String),
    /// `#animtree`.
    AnimTree,
    Vector(Box<[Expr; 3]>),
    /// `[]`.
    EmptyArray,
    SelfRef,
    Level,
    Game,
    Anim_,
    Ident(String),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// `::name` or `path::name`.
    FuncRef(Option<String>, String),
    Call(Box<Call>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    BitNot(Box<Expr>),
    Neg(Box<Expr>),
}

#[derive(Debug)]
pub struct Call {
    /// `self foo()`: who it's called on.
    pub object: Option<Expr>,
    pub callee: Callee,
    pub args: Vec<Expr>,
    /// `thread`: runs on its own.
    pub thread: bool,
    pub line: u32,
}

#[derive(Debug)]
pub enum Callee {
    /// A function by name, in a script (`maps\_utility::flag_wait`) or not.
    Named(Option<String>, String),
    /// `[[ expr ]]`: a function pointer.
    Pointer(Expr),
}
