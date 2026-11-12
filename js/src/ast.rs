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

