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
    /// `with (obj) corpo` (só fora do modo estrito)
    With(Expr, Box<Stmt>),
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogOp {
    And,
    Or,
    Nullish,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AssignOp {
    Assign,
    Op(BinOp),
    Logical(LogOp),
}

#[derive(Debug, Clone)]
pub enum ArrayElem {
    Expr(Expr),
    Spread(Expr),
    Hole,
}

#[derive(Debug, Clone)]
pub enum PropKind {
    Init,
    Get,
    Set,
    Spread,
}

#[derive(Debug, Clone)]
pub struct Prop {
    pub key: PropKeyExpr,
    pub value: Expr,
    pub kind: PropKind,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Num(f64),
    BigInt(Rc<str>),
    Str(Rc<[u16]>),
    Template(Vec<Rc<[u16]>>, Vec<Expr>),
    Tagged {
        tag: Box<Expr>,
        cooked: Vec<Option<Rc<[u16]>>>,
        raw: Vec<Rc<str>>,
        exprs: Vec<Expr>,
    },
    Regex(Rc<str>, Rc<str>),
    Ident(Name, u32),
    This,
    Null,
    Bool(bool),
    Array(Vec<ArrayElem>),
    Object(Vec<Prop>),
    Function(Rc<FunctionDef>),
    Class(Rc<ClassDef>),
    Unary(UnOp, Box<Expr>),
    Update {
        inc: bool,
        prefix: bool,
        target: Box<Expr>,
    },
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Logical(LogOp, Box<Expr>, Box<Expr>),
    Assign(AssignOp, Box<Pattern>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Call {
        callee: Box<Expr>,
        args: Vec<ArrayElem>,
        optional: bool,
        line: u32,
    },
    New(Box<Expr>, Vec<ArrayElem>, u32),
    Member {
        obj: Box<Expr>,
        prop: Name,
        optional: bool,
    },
    Index {
        obj: Box<Expr>,
        index: Box<Expr>,
        optional: bool,
    },
    PrivateMember(Box<Expr>, Name),
    /// `#x in obj`
    PrivateIn(Name, Box<Expr>),
    /// Fim de uma cadeia com `?.`: se algum elo deu null/undefined, tudo vira undefined.
    OptionalChain(Box<Expr>),
    SuperMember(Name),
    SuperIndex(Box<Expr>),
    SuperCall(Vec<ArrayElem>),
    Seq(Vec<Expr>),
    Yield(Option<Box<Expr>>, bool),
    Await(Box<Expr>),
    NewTarget,
    /// Cobertura: `...x` dentro de parênteses (vira parâmetro rest de arrow).
    Spread(Box<Expr>),
    /// `import(x)`: carrega um módulo (devolve uma Promise).
    ImportCall(Box<Expr>),
    /// `import.meta` (só em módulos).
    ImportMeta,
    /// Recurso que ainda não é suportado.
    Unsupported(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FnKind {
    Normal,
    Arrow,
    Method,
    Getter,
    Setter,
    ClassConstructor,
    DerivedConstructor,
    /// Inicializador de campos de classe (corpo sintético).
    FieldInit,
}

#[derive(Debug, Clone)]
pub enum FnBody {
    Block(Vec<Stmt>),
    Expr(Box<Expr>),
}

#[derive(Debug)]
pub struct FunctionDef {
    pub name: Option<Name>,
    pub params: Vec<Pattern>,
    pub rest: Option<Pattern>,
    pub body: FnBody,
    pub kind: FnKind,
    pub is_async: bool,
    pub is_generator: bool,
    pub strict: bool,
    /// Texto-fonte (para Function.prototype.toString).
    pub source: Rc<str>,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub enum MemberKind {
    Method,
    Getter,
    Setter,
    Field(Option<Expr>),
    StaticBlock(Vec<Stmt>),
}

#[derive(Debug, Clone)]
pub struct ClassMember {
    pub key: PropKeyExpr,
    pub is_static: bool,
    pub kind: MemberKind,
    pub func: Option<Rc<FunctionDef>>,
}

#[derive(Debug)]
pub struct ClassDef {
    pub name: Option<Name>,
    pub superclass: Option<Expr>,
    pub constructor: Option<Rc<FunctionDef>>,
    pub members: Vec<ClassMember>,
    pub source: Rc<str>,
    pub line: u32,
}

#[derive(Debug)]
pub struct Program {
    pub body: Vec<Stmt>,
    pub strict: bool,
    /// Módulos: nome local importado → (variável com o namespace do módulo
    /// de origem, nome exportado lá; None = o namespace inteiro).
    pub imports: crate::fxhash::FxHashMap<Name, (Name, Option<Name>)>,
}

/// Um item do topo de um módulo.
#[derive(Debug, Clone)]
pub enum ModuleItem {
    Stmt(Stmt),
    Import(ImportDecl),
    Export(ExportDecl),
}

/// `import ... from "x"`.
#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub specifier: String,
    /// (o que importa, nome local)
    pub bindings: Vec<(ImportName, Name)>,
}

#[derive(Debug, Clone)]
pub enum ImportName {
    Default,
    Named(String),
    Namespace,
}

#[derive(Debug, Clone)]
pub enum ExportDecl {
    /// `export var/let/const/function/class ...`: exporta os nomes declarados.
    Decl(Stmt),
    /// `export default function f() {}` / `export default class C {}`.
    DefaultDecl(Stmt, Name),
    /// `export default expressão`.
    DefaultExpr(Expr),
    /// `export { a, b as c }` (nome local, nome exportado).
    Names(Vec<(Name, String)>),
    /// `export { a as b } from "x"` (nome lá, nome exportado aqui).
    From(String, Vec<(String, String)>),
    /// `export * from "x"` / `export * as ns from "x"`.
    All(String, Option<String>),
}
