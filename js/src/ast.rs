//! Árvore sintática do JavaScript.

use std::rc::Rc;

pub type Name = Rc<str>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VarKind {
    Var,
    Let,
    Const,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Expr(Expr, u32),
    Var(VarKind, Vec<VarDecl>, u32),
    Function(Rc<FunctionDef>),
    Class(Rc<ClassDef>),
    Return(Option<Expr>, u32),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    Block(Vec<Stmt>),
    For {
        init: Option<ForInit>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Box<Stmt>,
    },
    ForIn {
        left: ForHead,
        right: Expr,
        body: Box<Stmt>,
    },
    ForOf {
        left: ForHead,
        right: Expr,
        body: Box<Stmt>,
        is_await: bool,
    },
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    Break(Option<Name>, u32),
    Continue(Option<Name>, u32),
    Throw(Expr, u32),
    Try {
        block: Vec<Stmt>,
        param: Option<Pattern>,
        handler: Option<Vec<Stmt>>,
        finalizer: Option<Vec<Stmt>>,
    },
    Switch(Expr, Vec<Case>),
    Labeled(Name, Box<Stmt>),
    Empty,
    Debugger,
}

#[derive(Debug, Clone)]
pub struct VarDecl {
    pub target: Pattern,
    pub init: Option<Expr>,
}

#[derive(Debug, Clone)]
pub enum ForInit {
    Var(VarKind, Vec<VarDecl>),
    Expr(Expr),
}

/// Lado esquerdo de for-in/for-of.
#[derive(Debug, Clone)]
pub enum ForHead {
    Var(VarKind, Pattern),
    Pattern(Pattern),
}

#[derive(Debug, Clone)]
pub struct Case {
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum PropKeyExpr {
    Name(Name),
    Computed(Box<Expr>),
    Private(Name),
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Ident(Name),
    Object {
        props: Vec<(PropKeyExpr, Pattern)>,
        rest: Option<Box<Pattern>>,
    },
    Array {
        elems: Vec<Option<Pattern>>,
        rest: Option<Box<Pattern>>,
    },
    /// Valor padrão: `x = 1`.
    Default(Box<Pattern>, Box<Expr>),
    /// Alvo de atribuição qualquer (`obj.x`, `a[i]`) em desestruturação.
    Target(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
    Plus,
    Not,
    BitNot,
    Typeof,
    Void,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    InstanceOf,
}

