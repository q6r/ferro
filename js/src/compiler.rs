//! Compilador: AST → bytecode de uma máquina de pilha.
//!
//! Os nomes são resolvidos em tempo de compilação. Variáveis que nenhuma
//! função interna usa ficam em registradores do quadro (rápido); as que são
//! capturadas por closures ficam em ambientes no heap, acessados por
//! (saltos, posição). O que não é encontrado é global.
//!
//! `finally` é copiado em cada saída (break/continue/return) e no caminho de
//! exceção, então a VM não precisa de um mecanismo especial para ele.

use std::cell::RefCell;
use std::collections::HashSet;

use crate::fxhash::{FxHashMap, FxHashSet};
use std::rc::Rc;

use crate::ast::*;
use crate::parser::ParseError;
use crate::resolve::{Captures, Resolver, class_init_key, key_of, slice_key};
use crate::string::JsStr;
use crate::value::{ObjRef, Value};

pub type CResult<T> = Result<T, ParseError>;

/// Tipos de método para DefineMethod.
pub const METHOD: u8 = 0;
pub const GETTER: u8 = 1;
pub const SETTER: u8 = 2;

/// Tipos de erro para ThrowError.
pub const ERR_TYPE: u8 = 0;
pub const ERR_SYNTAX: u8 = 1;
pub const ERR_REFERENCE: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    // --- constantes
    Undef,
    Null,
    True,
    False,
    Num(f64),
    /// Constante de texto (índice em `consts`).
    Str(u32),
    // --- pilha
    Pop,
    Dup,
    /// a b → a b a b
    Dup2,
    Swap,
    /// a b → a b a
    Over,
    /// a b c → c a b
    Rot3,
    /// a b c → b c a
    Roll3,
    /// a b c d → d a b c
    Rot4,
    // --- variáveis
    GetReg(u32),
    SetReg(u32),
    /// Marca o registrador como não inicializado (TDZ de let/const em blocos).
    ClearReg(u32),
    /// Com checagem de TDZ (índice do nome para a mensagem).
    GetRegChk(u32, u32),
    SetRegChk(u32, u32),
    GetEnv(u32, u32),
    SetEnv(u32, u32),
    GetEnvChk(u32, u32, u32),
    SetEnvChk(u32, u32, u32),
    GetGlobal(u32),
    SetGlobal(u32),
    TypeofGlobal(u32),
    DeleteGlobal(u32),
    /// `var` global (cria a propriedade se não existir).
    DeclareVar(u32),
    /// Função global: [fn] → []
    DeclareFunc(u32),
    /// let/const/class globais (nome, é const).
    DeclareLex(u32, bool),
    InitLex(u32),
    PushEnv(u32),
    PopEnv,
    /// Cópia do ambiente para a próxima iteração de `for (let ...)`.
    CopyEnv,
    ConstAssign(u32),
    // --- contexto
    This,
    /// `this` sem checar inicialização (prólogo de construtores derivados).
    ThisRaw,
    GlobalThis,
    NewTarget,
    /// A própria função (expressão de função com nome).
    FnSelf,
    RestArgs(u32),
    Arguments,
    // --- objetos
    NewObject,
    NewArray,
    ArrayPush,
    ArrayHole,
    ArraySpread,
    /// [obj chave valor] → [obj]
    DefineProp,
    /// Como DefineProp, dando nome à função anônima (`[k]: function () {}`).
    DefinePropNamed,
    /// [obj valor] → [obj]
    DefineName(u32),
    /// [obj chave fn] → [obj] (tipo, enumerável)
    DefineMethod(u8, bool),
    /// [obj fonte] → [obj]
    CopyProps,
    /// [fonte k1..kn] → [novo objeto sem essas chaves]
    CopyRest(u32),
    /// [obj proto] → [obj]
    SetProto,
    GetProp(u32),
    /// [obj valor] → [valor]
    SetProp(u32),
    /// [obj chave] → [valor]
    GetIndex,
    /// [obj chave valor] → [valor]
    SetIndex,
    DeleteProp(u32),
    DeleteIndex,
    /// [this] → [valor]
    GetSuper(u32),
    /// [this valor] → [valor]
    SetSuper(u32),
    /// [this chave] → [valor]
    GetSuperIndex,
    /// [this chave valor] → [valor]
    SetSuperIndex,
    NewPrivate(u32),
    /// [obj sym] → [valor]
    GetPrivate,
    /// [obj sym valor] → [valor]
    SetPrivate,
    /// [sym obj] → [bool]
    HasPrivate,
    /// [obj sym valor] → [obj]
    DefinePrivate,
    /// [obj sym fn] → [obj]
    DefinePrivateMethod(u8),
    // --- funções
    Closure(u32),
    /// [super?] → [ctor proto]
    MakeClass(u32, bool),
    /// [ctor proto fn] → [ctor proto]
    SetFields,
    /// [obj fn] → [] (chama fn com this = obj)
    CallInit,
    /// [fn this args...] → [resultado]
    Call(u32),
    /// [fn this array] → [resultado]
    CallSpread,
    /// [ctor args...] → [objeto]
    New(u32),
    NewSpread,
    SuperCall(u32),
    SuperCallSpread,
    Return,
    Throw,
    ThrowError(u8, u32),
    /// Fim do prólogo de um gerador: pausa até o primeiro next().
    InitialYield,
    /// [valor] → pausa devolvendo o valor; ao voltar: [enviado modo]
    Yield,
    /// [enviado modo] → normal: desvia com [enviado]; throw: lança; return: segue com [valor]
    GenResume(u32),
    /// yield*: [reg enviado modo] → [reg valor] ou desvia com [valor fezReturn]
    DelegateStep(u32),
    /// [valor] → pausa a função async até a promessa resolver
    Await,
    /// [valor modo] → [valor] ou lança
    AwaitResume,
    /// for await: [iterável] → [registro]
    GetAsyncIter,
    /// [reg] → [reg promessa-do-resultado]
    AsyncIterNext,
    /// [reg resultado] → [reg valor], ou desvia com [reg] quando acaba
    IterResultValue(u32),
    /// for await: [reg] → [promessa de return()] (espera-se em seguida),
    /// ou desvia com [] se não há return()
    AsyncIterClose(u32),
    TemplateObj(u32),
    Regex(u32, u32),
    // --- desvios
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfTrue(u32),
    /// Se falso, desvia mantendo o valor; senão descarta e segue.
    JumpIfFalseKeep(u32),
    JumpIfTrueKeep(u32),
    JumpIfNotNullishKeep(u32),
    /// Olha o topo: se null/undefined, desvia (sem tirar).
    JumpIfNullish(u32),
    JumpIfNotUndef(u32),
    Try(u32),
    PopTry,
    // --- iteração
    /// [iterável] → [registro]
    GetIter,
    /// [reg] → [reg valor], ou desvia com [reg] quando acaba.
    IterStep(u32),
    /// [reg] → [reg valor|undefined]
    IterValue,
    /// [reg] → [reg array]
    IterRest,
    /// [reg] → [] chamando return() se não terminou.
    IterClose,
    /// [reg exc] → relança exc depois de fechar o iterador.
    IterCloseThrow,
    /// [reg] → []
    IterPop,
    ForIn,
    ForInNext(u32),
    // --- operadores
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
    InstanceOf,
    In,
    Neg,
    Plus,
    Not,
    BitNot,
    Typeof,
    Inc,
    Dec,
    ToNumeric,
    ToPropertyKey,
    ToStr,
    /// Erro se o topo for null/undefined (desestruturação).
    RequireCoercible,
    Nop,
    // --- superinstruções (criadas pelo otimizador de janela, `peephole`)
    /// GetReg a; GetReg b
    GetReg2(u32, u32),
    /// GetReg r; GetProp k
    GetRegProp(u32, u32),
    /// GetReg r; Inc/Dec; SetReg r (`i++` sem usar o valor)
    IncReg(u32),
    DecReg(u32),
    /// Num n; Add / Sub
    AddNum(f64),
    SubNum(f64),
    /// comparação + JumpIfFalse: pula se a comparação for falsa
    JumpIfNotLt(u32),
    JumpIfNotLe(u32),
    JumpIfNotGt(u32),
    JumpIfNotGe(u32),
    JumpIfNotStrictEq(u32),
    JumpIfStrictEq(u32),
}

impl Op {
    fn set_target(&mut self, t: u32) {
        match self {
            Op::Jump(x)
            | Op::JumpIfFalse(x)
            | Op::JumpIfTrue(x)
            | Op::JumpIfFalseKeep(x)
            | Op::JumpIfTrueKeep(x)
            | Op::JumpIfNotNullishKeep(x)
            | Op::JumpIfNullish(x)
            | Op::JumpIfNotUndef(x)
            | Op::Try(x)
            | Op::IterStep(x)
            | Op::ForInNext(x)
            | Op::GenResume(x)
            | Op::DelegateStep(x)
            | Op::IterResultValue(x)
            | Op::AsyncIterClose(x)
            | Op::JumpIfNotLt(x)
            | Op::JumpIfNotLe(x)
            | Op::JumpIfNotGt(x)
            | Op::JumpIfNotGe(x)
            | Op::JumpIfNotStrictEq(x)
            | Op::JumpIfStrictEq(x) => *x = t,
            _ => {}
        }
    }

    /// Destino de pulo da instrução, se houver.
    fn target(&self) -> Option<u32> {
        match *self {
            Op::Jump(x)
            | Op::JumpIfFalse(x)
            | Op::JumpIfTrue(x)
            | Op::JumpIfFalseKeep(x)
            | Op::JumpIfTrueKeep(x)
            | Op::JumpIfNotNullishKeep(x)
            | Op::JumpIfNullish(x)
            | Op::JumpIfNotUndef(x)
            | Op::Try(x)
            | Op::IterStep(x)
            | Op::ForInNext(x)
            | Op::GenResume(x)
            | Op::DelegateStep(x)
            | Op::IterResultValue(x)
            | Op::AsyncIterClose(x)
            | Op::JumpIfNotLt(x)
            | Op::JumpIfNotLe(x)
            | Op::JumpIfNotGt(x)
            | Op::JumpIfNotGe(x)
            | Op::JumpIfNotStrictEq(x)
            | Op::JumpIfStrictEq(x) => Some(x),
            _ => None,
        }
    }
}

/// Otimizador de janela: troca sequências comuns por uma instrução só
/// (menos voltas no laço da VM). Ajusta os destinos de pulo, a tabela de
/// linhas e os nomes das chamadas.
fn peephole(ops: Vec<Op>, lines: &mut [(u32, u32)], call_names: &mut [(u32, u32)]) -> Vec<Op> {
    let n = ops.len();
    // pontos onde alguém pula para: não dá para fundir atravessando eles
    let mut is_target = vec![false; n + 1];
    for op in &ops {
        if let Some(t) = op.target() {
            is_target[(t as usize).min(n)] = true;
        }
    }
    // instruções que guardam o pc delas (nomes de chamadas, linhas) ficam como estão
    let mut pinned = vec![false; n + 1];
    for &(pc, _) in call_names.iter() {
        pinned[(pc as usize).min(n)] = true;
    }
    let free = |i: usize| i < n && !is_target[i] && !pinned[i];
    let mut out: Vec<Op> = Vec::with_capacity(n);
    let mut map: Vec<u32> = vec![0; n + 1];
    let mut i = 0;
    while i < n {
        let at = out.len() as u32;
        map[i] = at;
        let next = |k: usize| if free(i + k) { Some(ops[i + k]) } else { None };
        let (op, used) = match (ops[i], next(1), next(2)) {
            (Op::Dup, Some(Op::SetReg(r)), Some(Op::Pop)) => (Op::SetReg(r), 3),
            (Op::GetReg(r), Some(Op::Inc), Some(Op::SetReg(s))) if r == s => (Op::IncReg(r), 3),
            (Op::GetReg(r), Some(Op::Dec), Some(Op::SetReg(s))) if r == s => (Op::DecReg(r), 3),
            (Op::GetReg(a), Some(Op::GetReg(b)), _) => (Op::GetReg2(a, b), 2),
            (Op::GetReg(r), Some(Op::GetProp(k)), _) => (Op::GetRegProp(r, k), 2),
            (Op::Num(x), Some(Op::Add), _) => (Op::AddNum(x), 2),
            (Op::Num(x), Some(Op::Sub), _) => (Op::SubNum(x), 2),
            (Op::Lt, Some(Op::JumpIfFalse(t)), _) => (Op::JumpIfNotLt(t), 2),
            (Op::Le, Some(Op::JumpIfFalse(t)), _) => (Op::JumpIfNotLe(t), 2),
            (Op::Gt, Some(Op::JumpIfFalse(t)), _) => (Op::JumpIfNotGt(t), 2),
            (Op::Ge, Some(Op::JumpIfFalse(t)), _) => (Op::JumpIfNotGe(t), 2),
            (Op::StrictEq, Some(Op::JumpIfFalse(t)), _) => (Op::JumpIfNotStrictEq(t), 2),
            (Op::StrictNe, Some(Op::JumpIfFalse(t)), _) => (Op::JumpIfStrictEq(t), 2),
            (op, _, _) => (op, 1),
        };
        for k in 1..used {
            map[i + k] = at;
        }
        out.push(op);
        i += used;
    }
    map[n] = out.len() as u32;
    for op in &mut out {
        if let Some(t) = op.target() {
            op.set_target(map[(t as usize).min(n)]);
        }
    }
    for l in lines.iter_mut() {
        l.0 = map[(l.0 as usize).min(n)];
    }
    for c in call_names.iter_mut() {
        c.0 = map[(c.0 as usize).min(n)];
    }
    out
}

/// Template marcado: textos cozidos e crus.
pub type TemplateSrc = (Vec<Option<Rc<[u16]>>>, Vec<Rc<str>>);

/// Função compilada.
pub struct Code {
    pub name: JsStr,
    pub ops: Vec<Op>,
    pub consts: Vec<Value>,
    pub functions: Vec<Rc<Code>>,
    /// (pc, linha) em ordem crescente de pc.
    pub lines: Vec<(u32, u32)>,
    pub nregs: u32,
    /// Registradores que recebem os argumentos.
    pub nparams: u32,
    /// Registradores até aqui começam como undefined; os demais como "vazio" (TDZ).
    pub var_end: u32,
    /// Usa `arguments` ou parâmetro rest (precisa da lista de argumentos).
    pub needs_args: bool,
    /// Valor de `length`.
    pub length: u32,
    pub kind: FnKind,
    pub is_async: bool,
    pub is_generator: bool,
    pub strict: bool,
    pub is_script: bool,
    pub source: Rc<str>,
    pub file: Rc<str>,
    pub line: u32,
    pub templates: Vec<TemplateSrc>,
    pub template_cache: RefCell<Vec<Option<ObjRef>>>,
    /// (pc da chamada, constante com o texto do alvo) para mensagens de erro.
    pub call_names: Vec<(u32, u32)>,
    /// Células de variáveis léxicas globais já encontradas (por constante).
    pub global_cache: RefCell<Vec<Option<crate::vm::LexCell>>>,
    /// Propriedades do objeto global já encontradas: (geração das léxicas,
    /// geração das propriedades, posição), por constante.
    pub gobj_cache: RefCell<Vec<(u32, u32, u32)>>,
}

impl Code {
    pub fn is_constructor(&self) -> bool {
        matches!(self.kind, FnKind::Normal | FnKind::ClassConstructor | FnKind::DerivedConstructor)
            && !self.is_async
            && !self.is_generator
            && !self.is_script
    }

    pub fn is_arrow(&self) -> bool {
        self.kind == FnKind::Arrow
    }

    pub fn is_class_constructor(&self) -> bool {
        matches!(self.kind, FnKind::ClassConstructor | FnKind::DerivedConstructor)
    }

    /// Linha do código-fonte de um pc.
    pub fn line_at(&self, pc: usize) -> u32 {
        let pc = pc as u32;
        match self.lines.binary_search_by(|(p, _)| p.cmp(&pc)) {
            Ok(i) => self.lines[i].1,
            Err(0) => self.line,
            Err(i) => self.lines[i - 1].1,
        }
    }

    pub fn const_str(&self, i: u32) -> JsStr {
        match &self.consts[i as usize] {
            Value::String(s) => s.clone(),
            _ => JsStr::empty(),
        }
    }
}

// ---------------------------------------------------------------- análise

/// Nomes usados por funções internas (essas variáveis precisam ir para o heap).
#[derive(Default)]
struct Usage {
    inner: FxHashSet<Name>,
    uses_arguments: bool,
}

impl Usage {
    /// Nomes comuns: as capturas de verdade vêm do `Resolver`.
    fn name(&mut self, _n: &Name, _d: u32) {}

    /// `this`, `new.target`: só atravessam arrow functions.
    fn special(&mut self, n: &str, d: u32, a: bool) {
        if a && d > 0 {
            self.inner.insert(n.into());
        }
    }

    fn stmts(&mut self, ss: &[Stmt], d: u32, a: bool) {
        for s in ss {
            self.stmt(s, d, a);
        }
    }

    fn stmt(&mut self, s: &Stmt, d: u32, a: bool) {
        match s {
            Stmt::Expr(e, _) | Stmt::Throw(e, _) => self.expr(e, d, a),
            Stmt::Var(_, decls, _) => self.decls(decls, d, a),
            Stmt::Function(f) => {
                if let Some(n) = &f.name {
                    self.name(n, d);
                }
                self.func(f, d, a);
            }
            Stmt::Class(c) => self.class(c, d, a),
            Stmt::Return(e, _) => {
                if let Some(e) = e {
                    self.expr(e, d, a);
                }
            }
            Stmt::If(t, c, e) => {
                self.expr(t, d, a);
                self.stmt(c, d, a);
                if let Some(e) = e {
                    self.stmt(e, d, a);
                }
            }
            Stmt::Block(b) => self.stmts(b, d, a),
            Stmt::For { init, test, update, body } => {
                match init {
                    Some(ForInit::Var(_, decls)) => self.decls(decls, d, a),
                    Some(ForInit::Expr(e)) => self.expr(e, d, a),
                    None => {}
                }
                if let Some(t) = test {
                    self.expr(t, d, a);
                }
                if let Some(u) = update {
                    self.expr(u, d, a);
                }
                self.stmt(body, d, a);
            }
            Stmt::ForIn { left, right, body } | Stmt::ForOf { left, right, body, .. } => {
                match left {
                    ForHead::Var(_, p) | ForHead::Pattern(p) => self.pattern(p, d, a),
                }
                self.expr(right, d, a);
                self.stmt(body, d, a);
            }
            Stmt::While(t, b) | Stmt::DoWhile(b, t) => {
                self.expr(t, d, a);
                self.stmt(b, d, a);
            }
            Stmt::Try {
                block,
                param,
                handler,
                finalizer,
            } => {
                self.stmts(block, d, a);
                if let Some(p) = param {
                    self.pattern(p, d, a);
                }
                if let Some(h) = handler {
                    self.stmts(h, d, a);
                }
                if let Some(f) = finalizer {
                    self.stmts(f, d, a);
                }
            }
            Stmt::Switch(disc, cases) => {
                self.expr(disc, d, a);
                for c in cases {
                    if let Some(t) = &c.test {
                        self.expr(t, d, a);
                    }
                    self.stmts(&c.body, d, a);
                }
            }
            Stmt::Labeled(_, b) => self.stmt(b, d, a),
            Stmt::Break(..) | Stmt::Continue(..) | Stmt::Empty | Stmt::Debugger => {}
        }
    }

    fn decls(&mut self, decls: &[VarDecl], d: u32, a: bool) {
        for v in decls {
            self.pattern(&v.target, d, a);
            if let Some(i) = &v.init {
                self.expr(i, d, a);
            }
        }
    }

    fn pattern(&mut self, p: &Pattern, d: u32, a: bool) {
        match p {
            Pattern::Ident(n) => self.name(n, d),
            Pattern::Object { props, rest } => {
                for (k, v) in props {
                    if let PropKeyExpr::Computed(e) = k {
                        self.expr(e, d, a);
                    }
                    self.pattern(v, d, a);
                }
                if let Some(r) = rest {
                    self.pattern(r, d, a);
                }
            }
            Pattern::Array { elems, rest } => {
                for p in elems.iter().flatten() {
                    self.pattern(p, d, a);
                }
                if let Some(r) = rest {
                    self.pattern(r, d, a);
                }
            }
            Pattern::Default(p, e) => {
                self.pattern(p, d, a);
                self.expr(e, d, a);
            }
            Pattern::Target(e) => self.expr(e, d, a),
        }
    }

    fn func(&mut self, f: &FunctionDef, d: u32, a: bool) {
        let nd = d + 1;
        let na = a && f.kind == FnKind::Arrow;
        // só `this`, `new.target` e `arguments` interessam aqui, e eles só
        // atravessam arrow functions: o resto pode ser pulado
        if !na {
            return;
        }
        for p in &f.params {
            self.pattern(p, nd, na);
        }
        if let Some(r) = &f.rest {
            self.pattern(r, nd, na);
        }
        match &f.body {
            FnBody::Block(b) => self.stmts(b, nd, na),
            FnBody::Expr(e) => self.expr(e, nd, na),
        }
    }

    fn class(&mut self, c: &ClassDef, d: u32, a: bool) {
        if let Some(n) = &c.name {
            self.name(n, d);
        }
        if let Some(s) = &c.superclass {
            self.expr(s, d, a);
        }
        if let Some(f) = &c.constructor {
            self.func(f, d, a);
        }
        for m in &c.members {
            if let PropKeyExpr::Computed(e) = &m.key {
                self.expr(e, d, a);
            }
            match &m.kind {
                // inicializadores rodam numa função própria (não atravessam)
                MemberKind::Field(Some(_)) | MemberKind::StaticBlock(_) => {}
                _ => {}
            }
            if let Some(f) = &m.func {
                self.func(f, d, a);
            }
        }
    }

    fn elems(&mut self, es: &[ArrayElem], d: u32, a: bool) {
        for e in es {
            match e {
                ArrayElem::Expr(e) | ArrayElem::Spread(e) => self.expr(e, d, a),
                ArrayElem::Hole => {}
            }
        }
    }

    fn expr(&mut self, e: &Expr, d: u32, a: bool) {
        match e {
            Expr::Ident(n, _) => {
                if &**n == "arguments" && a {
                    self.uses_arguments = true;
                }
                self.name(n, d);
            }
            Expr::This => self.special("this", d, a),
            Expr::NewTarget => self.special("new.target", d, a),
            Expr::SuperMember(_) => self.special("this", d, a),
            Expr::SuperIndex(i) => {
                self.special("this", d, a);
                self.expr(i, d, a);
            }
            Expr::SuperCall(args) => {
                self.special("this", d, a);
                self.elems(args, d, a);
            }
            Expr::Num(_) | Expr::BigInt(_) | Expr::Str(_) | Expr::Regex(..) | Expr::Null | Expr::Bool(_) | Expr::Unsupported(_) => {}
            Expr::ImportCall(e) => self.expr(e, d, a),
            Expr::ImportMeta => {}
            Expr::Template(_, es) => {
                for e in es {
                    self.expr(e, d, a);
                }
            }
            Expr::Tagged { tag, exprs, .. } => {
                self.expr(tag, d, a);
                for e in exprs {
                    self.expr(e, d, a);
                }
            }
            Expr::Array(es) => self.elems(es, d, a),
            Expr::Object(props) => {
                for p in props {
                    if let PropKeyExpr::Computed(k) = &p.key {
                        self.expr(k, d, a);
                    }
                    self.expr(&p.value, d, a);
                }
            }
            Expr::Function(f) => self.func(f, d, a),
            Expr::Class(c) => self.class(c, d, a),
            Expr::Unary(_, x) | Expr::Await(x) | Expr::Spread(x) | Expr::OptionalChain(x) => self.expr(x, d, a),
            Expr::Update { target, .. } => self.expr(target, d, a),
            Expr::Binary(_, l, r) | Expr::Logical(_, l, r) => {
                self.expr(l, d, a);
                self.expr(r, d, a);
            }
            Expr::Assign(_, p, v) => {
                self.pattern(p, d, a);
                self.expr(v, d, a);
            }
            Expr::Cond(t, c, e) => {
                self.expr(t, d, a);
                self.expr(c, d, a);
                self.expr(e, d, a);
            }
            Expr::Call { callee, args, .. } => {
                self.expr(callee, d, a);
                self.elems(args, d, a);
            }
            Expr::New(c, args, _) => {
                self.expr(c, d, a);
                self.elems(args, d, a);
            }
            Expr::Member { obj, .. } => self.expr(obj, d, a),
            Expr::Index { obj, index, .. } => {
                self.expr(obj, d, a);
                self.expr(index, d, a);
            }
            Expr::PrivateMember(o, _) => self.expr(o, d, a),
            Expr::PrivateIn(_, o) => self.expr(o, d, a),
            Expr::Seq(es) => {
                for e in es {
                    self.expr(e, d, a);
                }
            }
            Expr::Yield(x, _) => {
                if let Some(x) = x {
                    self.expr(x, d, a);
                }
            }
        }
    }
}

/// Nomes declarados por um padrão.
pub(crate) fn bound_names(p: &Pattern, out: &mut Vec<Name>) {
    match p {
        Pattern::Ident(n) => out.push(n.clone()),
        Pattern::Object { props, rest } => {
            for (_, v) in props {
                bound_names(v, out);
            }
            if let Some(r) = rest {
                bound_names(r, out);
            }
        }
        Pattern::Array { elems, rest } => {
            for p in elems.iter().flatten() {
                bound_names(p, out);
            }
            if let Some(r) = rest {
                bound_names(r, out);
            }
        }
        Pattern::Default(p, _) => bound_names(p, out),
        Pattern::Target(_) => {}
    }
}

/// Nomes declarados com `var` (subindo de blocos, sem entrar em funções).
/// `annex_b`: funções declaradas em blocos também viram `var` (modo não estrito).
pub(crate) fn var_names(ss: &[Stmt], out: &mut Vec<Name>, annex_b: bool, nested: bool) {
    for s in ss {
        var_names_stmt(s, out, annex_b, nested);
    }
}

fn var_names_stmt(s: &Stmt, out: &mut Vec<Name>, annex_b: bool, nested: bool) {
    match s {
        Stmt::Var(VarKind::Var, decls, _) => {
            for d in decls {
                bound_names(&d.target, out);
            }
        }
        Stmt::Function(f) if nested && annex_b => {
            if let Some(n) = &f.name {
                out.push(n.clone());
            }
        }
        Stmt::If(_, c, e) => {
            var_names_stmt(c, out, annex_b, true);
            if let Some(e) = e {
                var_names_stmt(e, out, annex_b, true);
            }
        }
        Stmt::Block(b) => var_names(b, out, annex_b, true),
        Stmt::For { init, body, .. } => {
            if let Some(ForInit::Var(VarKind::Var, decls)) = init {
                for d in decls {
                    bound_names(&d.target, out);
                }
            }
            var_names_stmt(body, out, annex_b, true);
        }
        Stmt::ForIn { left, body, .. } | Stmt::ForOf { left, body, .. } => {
            if let ForHead::Var(VarKind::Var, p) = left {
                bound_names(p, out);
            }
            var_names_stmt(body, out, annex_b, true);
        }
        Stmt::While(_, b) | Stmt::DoWhile(b, _) => var_names_stmt(b, out, annex_b, true),
        Stmt::Labeled(_, b) => var_names_stmt(b, out, annex_b, nested),
        Stmt::Try {
            block,
            handler,
            finalizer,
            ..
        } => {
            var_names(block, out, annex_b, true);
            if let Some(h) = handler {
                var_names(h, out, annex_b, true);
            }
            if let Some(f) = finalizer {
                var_names(f, out, annex_b, true);
            }
        }
        Stmt::Switch(_, cases) => {
            for c in cases {
                var_names(&c.body, out, annex_b, true);
            }
        }
        _ => {}
    }
}

/// Declarações léxicas diretas de uma lista de instruções.
pub(crate) fn lexical_decls(ss: &[Stmt], funcs: bool, out: &mut Vec<(Name, Decl)>) {
    for s in ss {
        match s {
            Stmt::Var(kind @ (VarKind::Let | VarKind::Const), decls, _) => {
                let mut names = Vec::new();
                for d in decls {
                    bound_names(&d.target, &mut names);
                }
                let decl = if *kind == VarKind::Let { Decl::Let } else { Decl::Const };
                out.extend(names.into_iter().map(|n| (n, decl)));
            }
            Stmt::Class(c) => {
                if let Some(n) = &c.name {
                    out.push((n.clone(), Decl::Class));
                }
            }
            Stmt::Function(f) if funcs => {
                if let Some(n) = &f.name {
                    out.push((n.clone(), Decl::Func));
                }
            }
            _ => {}
        }
    }
}

fn pattern_name(p: &Pattern) -> Option<Name> {
    match p {
        Pattern::Ident(n) => Some(n.clone()),
        _ => None,
    }
}

fn is_simple_params(def: &FunctionDef) -> bool {
    def.rest.is_none() && def.params.iter().all(|p| matches!(p, Pattern::Ident(_)))
}

// ------------------------------------------------------------- estruturas

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Decl {
    Var,
    Let,
    Const,
    Class,
    ClassInner,
    Param,
    Func,
    Hidden,
    Callee,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Loc {
    Reg(u32),
    Env(u32),
    Global,
}

struct VarC {
    name: Name,
    loc: Loc,
    decl: Decl,
    /// Já inicializada neste ponto da compilação (dispensa checar TDZ).
    ready: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    Function,
    Block,
    Switch,
    Script,
}

struct ScopeC {
    /// Identidade do escopo (endereço do nó da AST), ligando à análise de capturas.
    key: usize,
    vars: Vec<VarC>,
    /// Nome → posições em `vars` (escopos de código minificado têm milhares de nomes).
    index: FxHashMap<Name, Vec<u32>>,
    env_size: u32,
    has_env: bool,
    kind: ScopeKind,
    reg_base: u32,
}

impl ScopeC {
    fn add(&mut self, v: VarC) {
        self.index.entry(v.name.clone()).or_default().push(self.vars.len() as u32);
        self.vars.push(v);
    }

    /// A declaração mais recente com esse nome.
    fn last(&self, name: &str) -> Option<usize> {
        self.index.get(name).and_then(|l| l.last()).map(|&i| i as usize)
    }

    /// A primeira declaração com esse nome que satisfaz `f`.
    fn first_where(&self, name: &str, f: impl Fn(&VarC) -> bool) -> Option<usize> {
        self.index.get(name)?.iter().map(|&i| i as usize).find(|&i| f(&self.vars[i]))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum LoopKind {
    Plain,
    ForIn,
    ForOf,
    ForAwait,
}

enum Ctl {
    Loop {
        labels: Vec<Name>,
        breaks: Vec<usize>,
        continues: Vec<usize>,
        env_depth: u32,
        kind: LoopKind,
    },
    Block {
        labels: Vec<Name>,
        breaks: Vec<usize>,
        env_depth: u32,
        is_switch: bool,
    },
    /// Tratador de exceção ativo (sair dele exige PopTry).
    Try,
    /// Corpo do finally (aponta para a AST, que vive durante toda a compilação).
    Finally(*const [Stmt], u32),
}

enum Res {
    Reg(u32),
    Env(u32, u32),
    Global,
}

struct Found {
    res: Res,
    decl: Decl,
    chk: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Bind {
    /// Declaração (let/const/parâmetro): sem checar TDZ, marca como pronta.
    Init,
    /// Atribuição comum.
    Store,
}

struct FnState {
    ops: Vec<Op>,
    consts: Vec<Value>,
    const_map: FxHashMap<JsStr, u32>,
    functions: Vec<Rc<Code>>,
    lines: Vec<(u32, u32)>,
    templates: Vec<TemplateSrc>,
    call_names: Vec<(u32, u32)>,
    scopes: Vec<ScopeC>,
    reg_top: u32,
    max_regs: u32,
    captured: FxHashSet<Name>,
    ctl: Vec<Ctl>,
    env_depth: u32,
    opt: Vec<Vec<(usize, u32)>>,
    pending_labels: Vec<Name>,
    kind: FnKind,
    strict: bool,
    is_script: bool,
    completion: Option<u32>,
    last_line: u32,
    is_async: bool,
    is_generator: bool,
}

impl FnState {
    fn new(kind: FnKind, strict: bool, captured: FxHashSet<Name>, is_script: bool) -> FnState {
        FnState {
            ops: Vec::new(),
            consts: Vec::new(),
            const_map: FxHashMap::default(),
            functions: Vec::new(),
            lines: Vec::new(),
            templates: Vec::new(),
            call_names: Vec::new(),
            scopes: Vec::new(),
            reg_top: 0,
            max_regs: 0,
            captured,
            ctl: Vec::new(),
            env_depth: 0,
            opt: Vec::new(),
            pending_labels: Vec::new(),
            kind,
            strict,
            is_script,
            completion: None,
            last_line: 0,
            is_async: false,
            is_generator: false,
        }
    }

    fn is_arrow(&self) -> bool {
        self.kind == FnKind::Arrow
    }
}

pub struct Compiler {
    fns: Vec<FnState>,
    file: Rc<str>,
    captures: Captures,
    /// Importações do módulo (nome local → namespace, nome exportado).
    imports: crate::fxhash::FxHashMap<Name, (Name, Option<Name>)>,
}

fn syntax<T>(msg: impl Into<String>, line: u32) -> CResult<T> {
    Err(ParseError {
        msg: msg.into(),
        line,
        eof: false,
    })
}

// ------------------------------------------------------------ entradas

/// Compila um script inteiro.
pub fn compile_script(prog: &Program, file: &str) -> CResult<Rc<Code>> {
    let t = std::time::Instant::now();
    let mut r = Resolver::new(prog.strict);
    r.program(prog);
    if std::env::var_os("FERRO_TEMPOS_JS").is_some() && t.elapsed().as_millis() > 20 {
        eprintln!("[js] capturas: {:.0} ms ({} entradas)", t.elapsed().as_secs_f64() * 1000.0, r.out.len());
    }
    let mut c = Compiler {
        fns: Vec::new(),
        file: file.into(),
        captures: r.out,
        imports: prog.imports.clone(),
    };
    let code = c.script(prog)?;
    // depuração: FERRO_BYTECODE=1 mostra as instruções de cada função
    if std::env::var_os("FERRO_BYTECODE").is_some() {
        fn dump(c: &Code, depth: usize) {
            eprintln!("{}== {} ({} regs)", "  ".repeat(depth), c.name, c.nregs);
            for (i, op) in c.ops.iter().enumerate() {
                eprintln!("{}{i:4} {op:?}", "  ".repeat(depth));
            }
            for f in &c.functions {
                dump(f, depth + 1);
            }
        }
        dump(&code, 0);
    }
    Ok(code)
}

/// Compila uma função isolada no escopo global (`new Function`).
pub fn compile_standalone(def: &FunctionDef, file: &str) -> CResult<Rc<Code>> {
    let mut r = Resolver::new(false);
    r.function(def, def.name.is_some());
    let mut c = Compiler {
        fns: Vec::new(),
        file: file.into(),
        captures: r.out,
        imports: Default::default(),
    };
    c.fns.push(FnState::new(FnKind::Normal, false, FxHashSet::default(), true));
    c.push_scope(ScopeKind::Script, 0);
    let idx = c.function(def, None, None, def.name.is_some())?;
    let st = c.fns.pop().unwrap();
    Ok(st.functions[idx as usize].clone())
}

impl Compiler {
    fn f(&mut self) -> &mut FnState {
        self.fns.last_mut().unwrap()
    }

    fn fr(&self) -> &FnState {
        self.fns.last().unwrap()
    }

    fn emit(&mut self, op: Op) -> usize {
        let f = self.f();
        f.ops.push(op);
        f.ops.len() - 1
    }

    fn here(&self) -> usize {
        self.fr().ops.len()
    }

    fn patch(&mut self, at: usize) {
        let t = self.here() as u32;
        self.f().ops[at].set_target(t);
    }

    fn patch_to(&mut self, at: usize, t: usize) {
        self.f().ops[at].set_target(t as u32);
    }

    fn konst(&mut self, s: &str) -> u32 {
        self.konst_js(JsStr::new(s))
    }

    fn konst_js(&mut self, s: JsStr) -> u32 {
        let f = self.f();
        if let Some(&i) = f.const_map.get(&s) {
            return i;
        }
        let i = f.consts.len() as u32;
        f.consts.push(Value::String(s.clone()));
        f.const_map.insert(s, i);
        i
    }

    fn line(&mut self, line: u32) {
        let f = self.f();
        if line != 0 && f.last_line != line {
            f.last_line = line;
            let pc = f.ops.len() as u32;
            match f.lines.last_mut() {
                Some(l) if l.0 == pc => l.1 = line,
                _ => f.lines.push((pc, line)),
            }
        }
    }

    fn alloc_reg(&mut self) -> u32 {
        let f = self.f();
        let r = f.reg_top;
        f.reg_top += 1;
        f.max_regs = f.max_regs.max(f.reg_top);
        r
    }

    fn free_reg(&mut self, r: u32) {
        let f = self.f();
        if f.reg_top == r + 1 {
            f.reg_top = r;
        }
    }

    // ------------------------------------------------------------ escopos

    fn push_scope(&mut self, kind: ScopeKind, key: usize) {
        let base = self.fr().reg_top;
        self.f().scopes.push(ScopeC {
            key,
            vars: Vec::new(),
            index: FxHashMap::default(),
            env_size: 0,
            has_env: false,
            kind,
            reg_base: base,
        });
    }

    /// Declara um nome no escopo atual. `force_env`: precisa ficar no heap.
    fn declare(&mut self, name: Name, decl: Decl, force_env: bool, ready: bool) -> Loc {
        let script_top = {
            let f = self.fr();
            f.is_script && f.scopes.len() == 1
        };
        {
            let sc = self.f().scopes.last_mut().unwrap();
            if let Some(i) = sc.first_where(&name, |v| v.decl != Decl::Callee) {
                let v = &mut sc.vars[i];
                if matches!(decl, Decl::Let | Decl::Const | Decl::Class) {
                    v.decl = decl;
                    v.ready = false;
                }
                return v.loc;
            }
        }
        let loc = if script_top && decl != Decl::Hidden {
            Loc::Global
        } else if force_env || self.is_captured(&name) {
            let sc = self.f().scopes.last_mut().unwrap();
            let slot = sc.env_size;
            sc.env_size += 1;
            sc.has_env = true;
            Loc::Env(slot)
        } else {
            Loc::Reg(self.alloc_reg())
        };
        self.f().scopes.last_mut().unwrap().add(VarC { name, loc, decl, ready });
        loc
    }

    /// A declaração `name` do escopo atual é usada por alguma função interna?
    fn is_captured(&self, name: &Name) -> bool {
        let key = self.fr().scopes.last().map_or(0, |s| s.key);
        self.captures.contains(&(key, name.clone()))
    }

    /// Abre o ambiente do escopo atual se ele tiver variáveis capturadas, e
    /// zera os registradores léxicos (podem ter sobras de um bloco anterior).
    fn open_env(&mut self) {
        let (has, size, regs) = {
            let sc = self.fr().scopes.last().unwrap();
            let regs: Vec<u32> = if sc.kind == ScopeKind::Function {
                Vec::new()
            } else {
                sc.vars
                    .iter()
                    .filter(|v| matches!(v.decl, Decl::Let | Decl::Const | Decl::Class | Decl::ClassInner))
                    .filter_map(|v| match v.loc {
                        Loc::Reg(r) => Some(r),
                        _ => None,
                    })
                    .collect()
            };
            (sc.has_env, sc.env_size, regs)
        };
        if has {
            self.emit(Op::PushEnv(size));
            self.f().env_depth += 1;
        }
        for r in regs {
            self.emit(Op::ClearReg(r));
        }
    }

    fn pop_scope(&mut self) {
        let sc = self.f().scopes.pop().unwrap();
        if sc.has_env {
            self.emit(Op::PopEnv);
            self.f().env_depth -= 1;
        }
        self.f().reg_top = sc.reg_base;
    }

    fn resolve(&self, name: &str) -> Found {
        let mut hops = 0u32;
        let nf = self.fns.len();
        for fi in (0..nf).rev() {
            let f = &self.fns[fi];
            for sc in f.scopes.iter().rev() {
                if let Some(v) = sc.last(name).map(|i| &sc.vars[i]) {
                    let lexical = matches!(v.decl, Decl::Let | Decl::Const | Decl::Class | Decl::ClassInner)
                        || (v.decl == Decl::Hidden && !v.ready);
                    let chk = lexical && (!v.ready || fi != nf - 1 || sc.kind == ScopeKind::Switch);
                    let res = match v.loc {
                        Loc::Reg(r) if fi == nf - 1 => Res::Reg(r),
                        // não deveria acontecer: capturada mas em registrador
                        Loc::Reg(_) => Res::Global,
                        Loc::Env(s) => Res::Env(hops, s),
                        Loc::Global => Res::Global,
                    };
                    return Found { res, decl: v.decl, chk };
                }
                if sc.has_env {
                    hops += 1;
                }
            }
        }
        Found {
            res: Res::Global,
            decl: Decl::Var,
            chk: false,
        }
    }

    fn has_binding(&self, name: &str) -> bool {
        self.fns
            .iter()
            .any(|f| f.scopes.iter().any(|sc| sc.index.contains_key(name)))
    }

    fn load(&mut self, name: &str) {
        let found = self.resolve(name);
        match found.res {
            Res::Reg(r) => {
                if found.chk {
                    let n = self.konst(name);
                    self.emit(Op::GetRegChk(r, n));
                } else {
                    self.emit(Op::GetReg(r));
                }
            }
            Res::Env(h, s) => {
                if found.chk {
                    let n = self.konst(name);
                    self.emit(Op::GetEnvChk(h, s, n));
                } else {
                    self.emit(Op::GetEnv(h, s));
                }
            }
            Res::Global => {
                if let Some((ns, export)) = self.imports.get(name).cloned() {
                    // importação: lê na hora do namespace do outro módulo (ligação viva)
                    self.load(&ns);
                    if let Some(e) = export {
                        let c = self.konst(&e);
                        self.emit(Op::GetProp(c));
                    }
                } else if name == "undefined" && !self.has_binding(name) {
                    self.emit(Op::Undef);
                } else {
                    let n = self.konst(name);
                    self.emit(Op::GetGlobal(n));
                }
            }
        }
    }

    /// Atribui o topo da pilha (consumindo) a uma variável.
    fn store(&mut self, name: &str) {
        let found = self.resolve(name);
        let n = self.konst(name);
        if matches!(found.res, Res::Global) && self.imports.contains_key(name) {
            self.emit(Op::ConstAssign(n));
            return;
        }
        match found.decl {
            Decl::Const | Decl::ClassInner if !matches!(found.res, Res::Global) => {
                self.emit(Op::ConstAssign(n));
                return;
            }
            Decl::Callee => {
                if self.fr().strict {
                    self.emit(Op::ConstAssign(n));
                } else {
                    self.emit(Op::Pop);
                }
                return;
            }
            _ => {}
        }
        match found.res {
            Res::Reg(r) => {
                if found.chk {
                    self.emit(Op::SetRegChk(r, n));
                } else {
                    self.emit(Op::SetReg(r));
                }
            }
            Res::Env(h, s) => {
                if found.chk {
                    self.emit(Op::SetEnvChk(h, s, n));
                } else {
                    self.emit(Op::SetEnv(h, s));
                }
            }
            Res::Global => {
                self.emit(Op::SetGlobal(n));
            }
        }
    }

    /// Inicializa a declaração `name` mais próxima na função atual.
    fn init(&mut self, name: &str) {
        let mut hops = 0;
        let mut target: Option<(Loc, Decl)> = None;
        {
            let f = self.f();
            for sc in f.scopes.iter_mut().rev() {
                if let Some(i) = sc.last(name) {
                    let v = &mut sc.vars[i];
                    if v.decl != Decl::Hidden || name != "this" {
                        v.ready = true;
                    }
                    target = Some((v.loc, v.decl));
                    break;
                }
                if sc.has_env {
                    hops += 1;
                }
            }
        }
        let n = self.konst(name);
        match target {
            Some((Loc::Reg(r), _)) => {
                self.emit(Op::SetReg(r));
            }
            Some((Loc::Env(s), _)) => {
                self.emit(Op::SetEnv(hops, s));
            }
            Some((Loc::Global, Decl::Let | Decl::Const | Decl::Class)) => {
                self.emit(Op::InitLex(n));
            }
            _ => {
                self.emit(Op::SetGlobal(n));
            }
        }
    }

    /// Guarda na variável `var` da função (Anexo B: função declarada em bloco).
    fn store_function_var(&mut self, name: &str) {
        let f = self.fr();
        let Some(fs) = f.scopes.first() else { return };
        let Some(v) = fs
            .first_where(name, |v| matches!(v.decl, Decl::Var | Decl::Func | Decl::Param))
            .map(|i| &fs.vars[i])
        else {
            self.emit(Op::Pop);
            return;
        };
        let loc = v.loc;
        let hops = f.scopes[1..].iter().filter(|s| s.has_env).count() as u32;
        match loc {
            Loc::Reg(r) => {
                self.emit(Op::SetReg(r));
            }
            Loc::Env(s) => {
                self.emit(Op::SetEnv(hops, s));
            }
            Loc::Global => {
                let n = self.konst(name);
                self.emit(Op::SetGlobal(n));
            }
        }
    }

    // ------------------------------------------------------------- script

    fn script(&mut self, prog: &Program) -> CResult<Rc<Code>> {
        let mut usage = Usage::default();
        usage.stmts(&prog.body, 0, true);
        self.fns.push(FnState::new(FnKind::Normal, prog.strict, usage.inner, true));
        self.push_scope(ScopeKind::Script, 0);
        let completion = self.alloc_reg();
        self.f().completion = Some(completion);
        let mut vars = Vec::new();
        var_names(&prog.body, &mut vars, !prog.strict, false);
        let mut lex = Vec::new();
        lexical_decls(&prog.body, false, &mut lex);
        let mut seen = HashSet::new();
        for v in vars {
            if seen.insert(v.clone()) {
                self.declare(v.clone(), Decl::Var, false, true);
                let n = self.konst(&v);
                self.emit(Op::DeclareVar(n));
            }
        }
        for (n, d) in lex {
            self.declare(n.clone(), d, false, false);
            let k = self.konst(&n);
            self.emit(Op::DeclareLex(k, d == Decl::Const));
        }
        for s in &prog.body {
            if let Stmt::Function(f) = s {
                let name = f.name.clone().unwrap_or_else(|| "".into());
                self.declare(name.clone(), Decl::Func, false, true);
                let idx = self.function(f, None, None, false)?;
                self.emit(Op::Closure(idx));
                let n = self.konst(&name);
                self.emit(Op::DeclareFunc(n));
            }
        }
        self.f().max_regs = self.f().max_regs.max(1);
        self.stmts(&prog.body)?;
        self.emit(Op::GetReg(completion));
        self.emit(Op::Return);
        let st = self.fns.pop().unwrap();
        Ok(Rc::new(self.finish(st, JsStr::empty(), 0, 0, 1, FnKind::Normal, false, false, "".into(), 1)))
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        st: FnState,
        name: JsStr,
        nparams: u32,
        length: u32,
        var_end: u32,
        kind: FnKind,
        is_async: bool,
        is_generator: bool,
        source: Rc<str>,
        line: u32,
    ) -> Code {
        let ntpl = st.templates.len();
        let mut lines = st.lines;
        let mut call_names = st.call_names;
        let ops = if std::env::var_os("FERRO_SEM_PEEPHOLE").is_some() {
            st.ops
        } else {
            peephole(st.ops, &mut lines, &mut call_names)
        };
        let needs_args = ops.iter().any(|o| matches!(o, Op::RestArgs(_) | Op::Arguments));
        Code {
            name,
            ops,
            consts: st.consts,
            functions: st.functions,
            lines,
            nregs: st.max_regs,
            nparams,
            var_end,
            needs_args,
            length,
            kind,
            is_async,
            is_generator,
            strict: st.strict,
            is_script: st.is_script,
            source,
            file: self.file.clone(),
            line,
            templates: st.templates,
            template_cache: RefCell::new(vec![None; ntpl]),
            call_names,
            global_cache: RefCell::new(Vec::new()),
            gobj_cache: RefCell::new(Vec::new()),
        }
    }

    // ------------------------------------------------------------ funções

    /// Compila uma função aninhada e devolve seu índice em `functions`.
    fn function(&mut self, def: &FunctionDef, name: Option<Name>, source: Option<Rc<str>>, self_binding: bool) -> CResult<u32> {
        if def.is_generator || def.is_async {
            // geradores e async chegam no M3.2: por ora a função existe mas lança
        }
        let mut usage = Usage::default();
        for p in &def.params {
            usage.pattern(p, 0, true);
        }
        if let Some(r) = &def.rest {
            usage.pattern(r, 0, true);
        }
        match &def.body {
            FnBody::Block(b) => usage.stmts(b, 0, true),
            FnBody::Expr(e) => usage.expr(e, 0, true),
        }
        let strict = def.strict || self.fr().strict;
        let kind = def.kind;
        let is_arrow = kind == FnKind::Arrow;
        let fname = def.name.clone().or(name).unwrap_or_else(|| "".into());
        let fname_js = match kind {
            FnKind::Getter => JsStr::new(&format!("get {fname}")),
            FnKind::Setter => JsStr::new(&format!("set {fname}")),
            _ => JsStr::new(&fname),
        };
        let captured = usage.inner;
        self.fns.push(FnState::new(kind, strict, captured, false));
        self.f().is_async = def.is_async;
        self.f().is_generator = def.is_generator;
        self.push_scope(ScopeKind::Function, key_of(def));
        let nparams = def.params.len() as u32;
        {
            let f = self.f();
            f.reg_top = nparams;
            f.max_regs = nparams;
        }
        let body_stmts: &[Stmt] = match &def.body {
            FnBody::Block(b) => b,
            FnBody::Expr(_) => &[],
        };
        // declarações
        if self_binding && !fname.is_empty() {
            self.declare(fname.clone(), Decl::Callee, false, true);
        }
        let uses_this = !is_arrow && self.fr().captured.contains("this");
        let uses_nt = !is_arrow && self.fr().captured.contains("new.target");
        let derived = kind == FnKind::DerivedConstructor;
        if uses_this {
            self.declare("this".into(), Decl::Hidden, true, !derived);
        }
        if uses_nt {
            self.declare("new.target".into(), Decl::Hidden, true, true);
        }
        let mut param_names = Vec::new();
        for p in &def.params {
            bound_names(p, &mut param_names);
        }
        if let Some(r) = &def.rest {
            bound_names(r, &mut param_names);
        }
        let uses_arguments = !is_arrow && usage.uses_arguments && !param_names.iter().any(|n| &**n == "arguments");
        if uses_arguments {
            let cap = self.is_captured(&"arguments".into());
            self.declare("arguments".into(), Decl::Hidden, cap, true);
        }
        let simple = is_simple_params(def);
        for (i, p) in def.params.iter().enumerate() {
            if let (true, Pattern::Ident(n)) = (simple, p) {
                if self.is_captured(n) {
                    self.declare(n.clone(), Decl::Param, true, true);
                } else {
                    // o próprio registrador do argumento
                    let sc = self.f().scopes.last_mut().unwrap();
                    if let Some(j) = sc.first_where(n, |v| v.decl != Decl::Callee) {
                        sc.vars[j].loc = Loc::Reg(i as u32);
                    } else {
                        sc.add(VarC {
                            name: n.clone(),
                            loc: Loc::Reg(i as u32),
                            decl: Decl::Param,
                            ready: true,
                        });
                    }
                }
            }
        }
        if !simple {
            for n in &param_names {
                self.declare(n.clone(), Decl::Param, false, true);
            }
        }
        let mut vars = Vec::new();
        var_names(body_stmts, &mut vars, !strict, false);
        for v in &vars {
            self.declare(v.clone(), Decl::Var, false, true);
        }
        let mut funcs = Vec::new();
        for s in body_stmts {
            if let Stmt::Function(f) = s
                && let Some(n) = &f.name
            {
                self.declare(n.clone(), Decl::Func, false, true);
                funcs.push(f.clone());
            }
        }
        let var_end = self.fr().reg_top;
        let mut lex = Vec::new();
        lexical_decls(body_stmts, false, &mut lex);
        for (n, d) in lex {
            self.declare(n, d, false, false);
        }
        // prólogo
        self.line(def.line);
        self.open_env();
        // variáveis `var` no heap começam como undefined
        let env_vars: Vec<(Name, u32)> = self
            .fr()
            .scopes
            .last()
            .unwrap()
            .vars
            .iter()
            .filter_map(|v| match (v.decl, v.loc) {
                (Decl::Var | Decl::Func, Loc::Env(s)) => Some((v.name.clone(), s)),
                _ => None,
            })
            .collect();
        for (_, s) in env_vars {
            self.emit(Op::Undef);
            self.emit(Op::SetEnv(0, s));
        }
        if self_binding && !fname.is_empty() && matches!(self.resolve(&fname).decl, Decl::Callee) {
            self.emit(Op::FnSelf);
            self.init(&fname);
        }
        if uses_this {
            self.emit(Op::ThisRaw);
            self.init("this");
        }
        if uses_nt {
            self.emit(Op::NewTarget);
            self.init("new.target");
        }
        if uses_arguments {
            self.emit(Op::Arguments);
            self.init("arguments");
        }
        if simple {
            for (i, p) in def.params.iter().enumerate() {
                if let Pattern::Ident(n) = p
                    && self.is_captured(n)
                {
                    self.emit(Op::GetReg(i as u32));
                    self.init(n);
                }
            }
        } else {
            for (i, p) in def.params.iter().enumerate() {
                self.emit(Op::GetReg(i as u32));
                self.pattern(p, Bind::Init)?;
            }
            if let Some(r) = &def.rest {
                self.emit(Op::RestArgs(nparams));
                self.pattern(r, Bind::Init)?;
            }
        }
        for f in funcs {
            let idx = self.function(&f, None, None, false)?;
            self.emit(Op::Closure(idx));
            self.init(f.name.as_deref().unwrap_or(""));
        }
        if def.is_generator {
            self.emit(Op::InitialYield);
        }
        // corpo
        match &def.body {
            FnBody::Block(b) => {
                self.stmts(b)?;
                self.emit(Op::Undef);
                self.emit(Op::Return);
            }
            FnBody::Expr(e) => {
                self.expr(e)?;
                self.emit(Op::Return);
            }
        }
        let st = self.fns.pop().unwrap();
        let length = def
            .params
            .iter()
            .take_while(|p| !matches!(p, Pattern::Default(..)))
            .count() as u32;
        let source = source.unwrap_or_else(|| def.source.clone());
        let code = self.finish(st, fname_js, nparams, length, var_end, kind, def.is_async, def.is_generator, source, def.line);
        let f = self.f();
        f.functions.push(Rc::new(code));
        Ok((f.functions.len() - 1) as u32)
    }

    // --------------------------------------------------------- instruções

    fn stmts(&mut self, ss: &[Stmt]) -> CResult<()> {
        for s in ss {
            self.stmt(s)?;
        }
        Ok(())
    }

    /// Abre um escopo de bloco com as declarações léxicas de `ss` e
    /// cria as funções declaradas nele.
    fn enter_block(&mut self, ss: &[Stmt], kind: ScopeKind, key: usize) -> CResult<()> {
        let mut lex = Vec::new();
        lexical_decls(ss, true, &mut lex);
        self.push_scope(kind, key);
        for (n, d) in &lex {
            let ready = *d == Decl::Func;
            self.declare(n.clone(), *d, false, ready);
        }
        self.open_env();
        let sloppy = !self.fr().strict;
        for s in ss {
            if let Stmt::Function(f) = s {
                let idx = self.function(f, None, None, false)?;
                self.emit(Op::Closure(idx));
                let name = f.name.clone().unwrap_or_else(|| "".into());
                if sloppy {
                    self.emit(Op::Dup);
                    self.store_function_var(&name);
                }
                self.init(&name);
            }
        }
        Ok(())
    }

    fn block(&mut self, ss: &[Stmt]) -> CResult<()> {
        self.enter_block(ss, ScopeKind::Block, slice_key(ss))?;
        self.stmts(ss)?;
        self.pop_scope();
        Ok(())
    }

    fn take_labels(&mut self) -> Vec<Name> {
        std::mem::take(&mut self.f().pending_labels)
    }

    fn stmt(&mut self, s: &Stmt) -> CResult<()> {
        if !matches!(s, Stmt::Labeled(..) | Stmt::For { .. } | Stmt::ForIn { .. } | Stmt::ForOf { .. } | Stmt::While(..) | Stmt::DoWhile(..)) {
            self.f().pending_labels.clear();
        }
        match s {
            Stmt::Expr(e, line) => {
                self.line(*line);
                match self.fr().completion {
                    Some(r) => {
                        self.expr(e)?;
                        self.emit(Op::SetReg(r));
                    }
                    None => self.expr_void(e)?,
                }
            }
            Stmt::Var(kind, decls, line) => {
                self.line(*line);
                self.var_decls(*kind, decls, *line)?;
            }
            Stmt::Function(_) => {}
            Stmt::Class(c) => {
                self.line(c.line);
                self.class(c, None)?;
                let name = c.name.clone().unwrap_or_else(|| "".into());
                self.init(&name);
            }
            Stmt::Return(e, line) => {
                self.line(*line);
                if self.fr().is_script {
                    return syntax("'return' fora de função", *line);
                }
                match e {
                    Some(e) => self.expr(e)?,
                    None => {
                        self.emit(Op::Undef);
                    }
                }
                self.emit_return()?;
            }
            Stmt::If(test, cons, alt) => {
                self.expr(test)?;
                let j = self.emit(Op::JumpIfFalse(0));
                self.stmt(cons)?;
                match alt {
                    Some(alt) => {
                        let j2 = self.emit(Op::Jump(0));
                        self.patch(j);
                        self.stmt(alt)?;
                        self.patch(j2);
                    }
                    None => self.patch(j),
                }
            }
            Stmt::Block(b) => self.block(b)?,
            Stmt::For { init, test, update, body } => self.for_loop(init, test, update, body)?,
            Stmt::ForIn { left, right, body } => self.for_in(left, right, body)?,
            Stmt::ForOf { left, right, body, is_await } => {
                if *is_await && !self.fr().is_async {
                    return syntax("for await só pode ser usado dentro de funções async", 0);
                }
                self.for_of(left, right, body, *is_await)?;
            }
            Stmt::While(test, body) => {
                let labels = self.take_labels();
                let top = self.here();
                self.expr(test)?;
                let exit = self.emit(Op::JumpIfFalse(0));
                self.push_loop(labels, LoopKind::Plain);
                self.stmt(body)?;
                self.emit(Op::Jump(top as u32));
                self.pop_loop(top, Some(exit));
            }
            Stmt::DoWhile(body, test) => {
                let labels = self.take_labels();
                let top = self.here();
                self.push_loop(labels, LoopKind::Plain);
                self.stmt(body)?;
                let cont = self.here();
                self.expr(test)?;
                self.emit(Op::JumpIfTrue(top as u32));
                self.pop_loop(cont, None);
            }
            Stmt::Break(label, line) => self.jump_out(label.as_ref(), false, *line)?,
            Stmt::Continue(label, line) => self.jump_out(label.as_ref(), true, *line)?,
            Stmt::Throw(e, line) => {
                self.line(*line);
                self.expr(e)?;
                self.emit(Op::Throw);
            }
            Stmt::Try {
                block,
                param,
                handler,
                finalizer,
            } => self.try_stmt(block, param.as_ref(), handler.as_deref(), finalizer.as_deref())?,
            Stmt::Switch(disc, cases) => self.switch(disc, cases)?,
            Stmt::Labeled(label, body) => {
                self.f().pending_labels.push(label.clone());
                match &**body {
                    Stmt::For { .. } | Stmt::ForIn { .. } | Stmt::ForOf { .. } | Stmt::While(..) | Stmt::DoWhile(..) | Stmt::Labeled(..) => {
                        self.stmt(body)?;
                    }
                    _ => {
                        let labels = self.take_labels();
                        let env_depth = self.fr().env_depth;
                        self.f().ctl.push(Ctl::Block {
                            labels,
                            breaks: Vec::new(),
                            env_depth,
                            is_switch: false,
                        });
                        self.stmt(body)?;
                        if let Some(Ctl::Block { breaks, .. }) = self.f().ctl.pop() {
                            for b in breaks {
                                self.patch(b);
                            }
                        }
                    }
                }
            }
            Stmt::Empty | Stmt::Debugger => {}
        }
        Ok(())
    }

    fn var_decls(&mut self, kind: VarKind, decls: &[VarDecl], line: u32) -> CResult<()> {
        for d in decls {
            match (&d.init, kind) {
                (Some(e), _) => {
                    self.expr_named(e, pattern_name(&d.target))?;
                    self.pattern(&d.target, if kind == VarKind::Var { Bind::Store } else { Bind::Init })?;
                }
                (None, VarKind::Var) => {}
                (None, VarKind::Let) => {
                    self.emit(Op::Undef);
                    self.pattern(&d.target, Bind::Init)?;
                }
                (None, VarKind::Const) => return syntax("const precisa de valor inicial", line),
            }
        }
        Ok(())
    }

    fn push_loop(&mut self, labels: Vec<Name>, kind: LoopKind) {
        let env_depth = self.fr().env_depth;
        self.f().ctl.push(Ctl::Loop {
            labels,
            breaks: Vec::new(),
            continues: Vec::new(),
            env_depth,
            kind,
        });
    }

    /// Fecha o laço: `continue` vai para `cont`, `break` (e `exit`) para cá.
    fn pop_loop(&mut self, cont: usize, exit: Option<usize>) {
        if let Some(Ctl::Loop { breaks, continues, .. }) = self.f().ctl.pop() {
            for c in continues {
                self.patch_to(c, cont);
            }
            for b in breaks {
                self.patch(b);
            }
        }
        if let Some(e) = exit {
            self.patch(e);
        }
    }

    fn for_loop(&mut self, init: &Option<ForInit>, test: &Option<Expr>, update: &Option<Expr>, body: &Stmt) -> CResult<()> {
        let labels = self.take_labels();
        let mut scoped = false;
        let mut per_iter = false;
        if let Some(ForInit::Var(kind, decls)) = init
            && *kind != VarKind::Var
        {
            let mut names = Vec::new();
            for d in decls {
                bound_names(&d.target, &mut names);
            }
            self.push_scope(ScopeKind::Block, key_of(init));
            let decl = if *kind == VarKind::Let { Decl::Let } else { Decl::Const };
            for n in names {
                self.declare(n, decl, false, false);
            }
            self.open_env();
            per_iter = *kind == VarKind::Let && self.fr().scopes.last().unwrap().has_env;
            scoped = true;
        }
        match init {
            Some(ForInit::Var(kind, decls)) => self.var_decls(*kind, decls, 0)?,
            Some(ForInit::Expr(e)) => {
                self.expr(e)?;
                self.emit(Op::Pop);
            }
            None => {}
        }
        if per_iter {
            self.emit(Op::CopyEnv);
        }
        let top = self.here();
        let exit = match test {
            Some(t) => {
                self.expr(t)?;
                Some(self.emit(Op::JumpIfFalse(0)))
            }
            None => None,
        };
        self.push_loop(labels, LoopKind::Plain);
        self.stmt(body)?;
        let cont = self.here();
        if per_iter {
            self.emit(Op::CopyEnv);
        }
        if let Some(u) = update {
            self.expr_void(u)?;
        }
        self.emit(Op::Jump(top as u32));
        self.pop_loop(cont, exit);
        if scoped {
            self.pop_scope();
        }
        Ok(())
    }

    /// Abre o escopo de uma iteração de for-in/for-of e atribui o valor do topo.
    fn bind_for_head(&mut self, left: &ForHead) -> CResult<bool> {
        match left {
            ForHead::Var(VarKind::Var, p) => {
                self.pattern(p, Bind::Store)?;
                Ok(false)
            }
            ForHead::Var(kind, p) => {
                let mut names = Vec::new();
                bound_names(p, &mut names);
                self.push_scope(ScopeKind::Block, key_of(left));
                let decl = if *kind == VarKind::Let { Decl::Let } else { Decl::Const };
                for n in names {
                    self.declare(n, decl, false, false);
                }
                // o valor está na pilha; o ambiente novo não mexe nela
                self.open_env();
                self.pattern(p, Bind::Init)?;
                Ok(true)
            }
            ForHead::Pattern(p) => {
                self.pattern(p, Bind::Store)?;
                Ok(false)
            }
        }
    }

    fn for_in(&mut self, left: &ForHead, right: &Expr, body: &Stmt) -> CResult<()> {
        let labels = self.take_labels();
        self.expr(right)?;
        self.emit(Op::ForIn);
        let top = self.here();
        let next = self.emit(Op::ForInNext(0));
        self.push_loop(labels, LoopKind::ForIn);
        // o laço registrou a profundidade de ambiente de fora da iteração
        let scoped = self.bind_for_head(left)?;
        self.stmt(body)?;
        if scoped {
            self.pop_scope();
        }
        let cont = self.here();
        self.emit(Op::Jump(top as u32));
        self.pop_loop(cont, None);
        self.patch(next);
        self.emit(Op::Pop);
        Ok(())
    }

    fn for_of(&mut self, left: &ForHead, right: &Expr, body: &Stmt, is_await: bool) -> CResult<()> {
        let labels = self.take_labels();
        self.expr(right)?;
        self.emit(if is_await { Op::GetAsyncIter } else { Op::GetIter });
        let top = self.here();
        let step = if is_await {
            self.emit(Op::AsyncIterNext);
            self.emit(Op::Await);
            self.emit(Op::AwaitResume);
            self.emit(Op::IterResultValue(0))
        } else {
            self.emit(Op::IterStep(0))
        };
        self.push_loop(labels, if is_await { LoopKind::ForAwait } else { LoopKind::ForOf });
        let scoped = self.bind_for_head(left)?;
        let try_at = self.emit(Op::Try(0));
        self.f().ctl.push(Ctl::Try);
        self.stmt(body)?;
        self.f().ctl.pop();
        self.emit(Op::PopTry);
        if scoped {
            self.pop_scope();
        }
        let cont = self.here();
        self.emit(Op::Jump(top as u32));
        // exceção no corpo: fecha o iterador e relança
        self.patch(try_at);
        self.emit(Op::IterCloseThrow);
        // break: fecha o iterador
        let (breaks, continues) = match self.f().ctl.pop() {
            Some(Ctl::Loop { breaks, continues, .. }) => (breaks, continues),
            _ => (Vec::new(), Vec::new()),
        };
        for c in continues {
            self.patch_to(c, cont);
        }
        for b in breaks {
            self.patch(b);
        }
        if is_await {
            self.async_iter_close();
        } else {
            self.emit(Op::IterClose);
        }
        let after = self.emit(Op::Jump(0));
        self.patch(step);
        self.emit(Op::IterPop);
        self.patch(after);
        Ok(())
    }

    /// Fecha o iterador de um for await esperando o return() (spec: AsyncIteratorClose).
    fn async_iter_close(&mut self) {
        let skip = self.emit(Op::AsyncIterClose(0));
        self.emit(Op::Await);
        self.emit(Op::AwaitResume);
        self.emit(Op::Pop);
        self.patch(skip);
    }

    fn switch(&mut self, disc: &Expr, cases: &[Case]) -> CResult<()> {
        let labels = self.take_labels();
        self.expr(disc)?;
        let tmp = self.alloc_reg();
        self.emit(Op::SetReg(tmp));
        let all: Vec<Stmt> = cases.iter().flat_map(|c| c.body.iter().cloned()).collect();
        self.enter_block(&all, ScopeKind::Switch, cases.as_ptr() as usize)?;
        let env_depth = self.fr().env_depth;
        self.f().ctl.push(Ctl::Block {
            labels,
            breaks: Vec::new(),
            env_depth,
            is_switch: true,
        });
        let mut jumps = Vec::new();
        for c in cases {
            if let Some(t) = &c.test {
                self.emit(Op::GetReg(tmp));
                self.expr(t)?;
                self.emit(Op::StrictEq);
                jumps.push(Some(self.emit(Op::JumpIfTrue(0))));
            } else {
                jumps.push(None);
            }
        }
        let to_default = self.emit(Op::Jump(0));
        let mut default_pos = None;
        for (i, c) in cases.iter().enumerate() {
            match jumps[i] {
                Some(j) => self.patch(j),
                None => default_pos = Some(self.here()),
            }
            self.stmts(&c.body)?;
        }
        match default_pos {
            Some(p) => self.patch_to(to_default, p),
            None => self.patch(to_default),
        }
        if let Some(Ctl::Block { breaks, .. }) = self.f().ctl.pop() {
            for b in breaks {
                self.patch(b);
            }
        }
        self.pop_scope();
        self.free_reg(tmp);
        Ok(())
    }

    fn try_stmt(&mut self, block: &[Stmt], param: Option<&Pattern>, handler: Option<&[Stmt]>, finalizer: Option<&[Stmt]>) -> CResult<()> {
        let fin = finalizer;
        if let Some(fin) = fin {
            let d = self.fr().env_depth;
            self.f().ctl.push(Ctl::Finally(fin as *const [Stmt], d));
        }
        let try_at = self.emit(Op::Try(0));
        self.f().ctl.push(Ctl::Try);
        self.block(block)?;
        self.f().ctl.pop();
        self.emit(Op::PopTry);
        let mut to_normal = vec![self.emit(Op::Jump(0))];
        self.patch(try_at);
        let mut exc_path = None;
        if let Some(h) = handler {
            let inner = if fin.is_some() {
                let t = self.emit(Op::Try(0));
                self.f().ctl.push(Ctl::Try);
                Some(t)
            } else {
                None
            };
            // parâmetro do catch
            self.push_scope(ScopeKind::Block, param.map(key_of).unwrap_or(0));
            if let Some(p) = param {
                let mut names = Vec::new();
                bound_names(p, &mut names);
                for n in names {
                    self.declare(n, Decl::Let, false, false);
                }
                self.open_env();
                self.pattern(p, Bind::Init)?;
            } else {
                self.emit(Op::Pop);
            }
            self.block(h)?;
            self.pop_scope();
            if let Some(t) = inner {
                self.f().ctl.pop();
                self.emit(Op::PopTry);
                to_normal.push(self.emit(Op::Jump(0)));
                exc_path = Some(t);
            } else {
                to_normal.push(self.emit(Op::Jump(0)));
            }
        }
        if let Some(fin) = fin {
            self.f().ctl.pop();
            // caminho da exceção: roda o finally e relança
            if let Some(t) = exc_path {
                self.patch(t);
            }
            let tmp = self.alloc_reg();
            self.emit(Op::SetReg(tmp));
            self.block(fin)?;
            self.emit(Op::GetReg(tmp));
            self.emit(Op::Throw);
            self.free_reg(tmp);
            for j in to_normal {
                self.patch(j);
            }
            self.block(fin)?;
        } else {
            for j in to_normal {
                self.patch(j);
            }
        }
        Ok(())
    }

    /// Emite as saídas de controle (PopTry, finally, fechar iteradores)
    /// até o nível `target` (exclusivo) da pilha de controle.
    fn unwind(&mut self, target: usize, final_env: Option<u32>) -> CResult<()> {
        let mut i = self.fr().ctl.len();
        let mut cur_env = self.fr().env_depth;
        while i > target {
            i -= 1;
            enum Act {
                PopTry,
                Finally(*const [Stmt], u32),
                IterClose,
                AsyncIterClose,
                PopValue,
                None,
            }
            let act = match &self.fr().ctl[i] {
                Ctl::Try => Act::PopTry,
                Ctl::Finally(b, d) => Act::Finally(*b, *d),
                Ctl::Loop { kind: LoopKind::ForOf, .. } => Act::IterClose,
                Ctl::Loop { kind: LoopKind::ForAwait, .. } => Act::AsyncIterClose,
                Ctl::Loop { kind: LoopKind::ForIn, .. } => Act::PopValue,
                _ => Act::None,
            };
            match act {
                Act::PopTry => {
                    self.emit(Op::PopTry);
                }
                Act::IterClose => {
                    self.emit(Op::IterClose);
                }
                Act::AsyncIterClose => self.async_iter_close(),
                Act::PopValue => {
                    self.emit(Op::Pop);
                }
                Act::Finally(body, d) => {
                    while cur_env > d {
                        self.emit(Op::PopEnv);
                        cur_env -= 1;
                    }
                    let tail = self.f().ctl.split_off(i);
                    let saved = self.fr().env_depth;
                    self.f().env_depth = d;
                    // SEGURANÇA: o ponteiro aponta para a AST recebida por
                    // compile_script/compile_standalone, viva até o fim da compilação.
                    let body: &[Stmt] = unsafe { &*body };
                    self.block(body)?;
                    self.f().env_depth = saved;
                    self.f().ctl.extend(tail);
                }
                Act::None => {}
            }
        }
        if let Some(fe) = final_env {
            while cur_env > fe {
                self.emit(Op::PopEnv);
                cur_env -= 1;
            }
        }
        Ok(())
    }

    fn emit_return(&mut self) -> CResult<()> {
        let needs = self
            .fr()
            .ctl
            .iter()
            .any(|c| matches!(c, Ctl::Finally(..) | Ctl::Loop { kind: LoopKind::ForOf | LoopKind::ForAwait, .. }));
        if needs {
            let tmp = self.alloc_reg();
            self.emit(Op::SetReg(tmp));
            self.unwind(0, None)?;
            self.emit(Op::GetReg(tmp));
            self.free_reg(tmp);
        }
        self.emit(Op::Return);
        Ok(())
    }

    fn jump_out(&mut self, label: Option<&Name>, is_continue: bool, line: u32) -> CResult<()> {
        let ctl = &self.fr().ctl;
        let mut target = None;
        for i in (0..ctl.len()).rev() {
            match &ctl[i] {
                Ctl::Loop { labels, .. } => {
                    if label.is_none_or(|l| labels.contains(l)) {
                        target = Some(i);
                        break;
                    }
                }
                Ctl::Block { labels, is_switch, .. } if !is_continue => {
                    let ok = match label {
                        Some(l) => labels.contains(l),
                        None => *is_switch,
                    };
                    if ok {
                        target = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(t) = target else {
            return syntax(
                match (label, is_continue) {
                    (Some(l), _) => format!("rótulo '{l}' não encontrado"),
                    (None, true) => "'continue' fora de laço".into(),
                    (None, false) => "'break' fora de laço ou switch".into(),
                },
                line,
            );
        };
        if is_continue && !matches!(self.fr().ctl[t], Ctl::Loop { .. }) {
            return syntax("'continue' para um rótulo que não é laço", line);
        }
        let env = match &self.fr().ctl[t] {
            Ctl::Loop { env_depth, .. } | Ctl::Block { env_depth, .. } => *env_depth,
            _ => 0,
        };
        self.unwind(t + 1, Some(env))?;
        let j = self.emit(Op::Jump(0));
        match &mut self.f().ctl[t] {
            Ctl::Loop { breaks, continues, .. } => {
                if is_continue {
                    continues.push(j);
                } else {
                    breaks.push(j);
                }
            }
            Ctl::Block { breaks, .. } => breaks.push(j),
            _ => {}
        }
        Ok(())
    }

    // ---------------------------------------------------------- padrões

    /// Desestrutura o valor do topo da pilha (consumindo-o).
    fn pattern(&mut self, p: &Pattern, mode: Bind) -> CResult<()> {
        match p {
            Pattern::Ident(n) => {
                match mode {
                    Bind::Init => self.init(n),
                    Bind::Store => self.store(n),
                }
                Ok(())
            }
            Pattern::Default(inner, def) => {
                let j = self.emit(Op::JumpIfNotUndef(0));
                self.emit(Op::Pop);
                self.expr_named(def, pattern_name(inner))?;
                self.patch(j);
                self.pattern(inner, mode)
            }
            Pattern::Target(e) => {
                if mode == Bind::Init {
                    return syntax("alvo inválido em declaração", 0);
                }
                self.assign_target(e)
            }
            Pattern::Object { props, rest } => {
                self.emit(Op::RequireCoercible);
                let mut key_regs = Vec::new();
                for (k, sub) in props {
                    self.emit(Op::Dup);
                    match k {
                        PropKeyExpr::Name(n) => {
                            let c = self.konst(n);
                            self.emit(Op::GetProp(c));
                        }
                        PropKeyExpr::Computed(e) => {
                            self.expr(e)?;
                            self.emit(Op::ToPropertyKey);
                            if rest.is_some() {
                                let r = self.alloc_reg();
                                self.emit(Op::Dup);
                                self.emit(Op::SetReg(r));
                                key_regs.push(r);
                            }
                            self.emit(Op::GetIndex);
                        }
                        PropKeyExpr::Private(_) => return syntax("nome privado em desestruturação", 0),
                    }
                    self.pattern(sub, mode)?;
                }
                if let Some(r) = rest {
                    let mut n = 0;
                    let mut ki = 0;
                    for (k, _) in props {
                        match k {
                            PropKeyExpr::Name(name) => {
                                let c = self.konst(name);
                                self.emit(Op::Str(c));
                            }
                            _ => {
                                self.emit(Op::GetReg(key_regs[ki]));
                                ki += 1;
                            }
                        }
                        n += 1;
                    }
                    self.emit(Op::CopyRest(n));
                    for r in key_regs.into_iter().rev() {
                        self.free_reg(r);
                    }
                    self.pattern(r, mode)
                } else {
                    self.emit(Op::Pop);
                    Ok(())
                }
            }
            Pattern::Array { elems, rest } => {
                self.emit(Op::GetIter);
                for e in elems {
                    self.emit(Op::IterValue);
                    match e {
                        Some(p) => self.pattern(p, mode)?,
                        None => {
                            self.emit(Op::Pop);
                        }
                    }
                }
                if let Some(r) = rest {
                    self.emit(Op::IterRest);
                    self.pattern(r, mode)?;
                }
                self.emit(Op::IterClose);
                Ok(())
            }
        }
    }

    /// [valor] → [] atribuindo a `obj.x`, `a[i]`, `this.#p`, `super.x`.
    fn assign_target(&mut self, e: &Expr) -> CResult<()> {
        match e {
            Expr::Ident(n, _) => self.store(n),
            Expr::Member { obj, prop, .. } => {
                self.expr(obj)?;
                self.emit(Op::Swap);
                let c = self.konst(prop);
                self.emit(Op::SetProp(c));
                self.emit(Op::Pop);
            }
            Expr::Index { obj, index, .. } => {
                self.expr(obj)?;
                self.expr(index)?;
                self.emit(Op::Roll3);
                self.emit(Op::SetIndex);
                self.emit(Op::Pop);
            }
            Expr::PrivateMember(obj, name) => {
                self.expr(obj)?;
                self.load_private(name)?;
                self.emit(Op::Roll3);
                self.emit(Op::SetPrivate);
                self.emit(Op::Pop);
            }
            Expr::SuperMember(name) => {
                self.this()?;
                self.emit(Op::Swap);
                let c = self.konst(name);
                self.emit(Op::SetSuper(c));
                self.emit(Op::Pop);
            }
            Expr::SuperIndex(idx) => {
                self.this()?;
                self.expr(idx)?;
                self.emit(Op::Roll3);
                self.emit(Op::SetSuperIndex);
                self.emit(Op::Pop);
            }
            _ => return syntax("alvo de atribuição inválido", 0),
        }
        Ok(())
    }

    fn load_private(&mut self, name: &str) -> CResult<()> {
        let key = format!("#{name}");
        if !self.has_binding(&key) {
            return syntax(format!("nome privado {key} não declarado numa classe"), 0);
        }
        self.load(&key);
        Ok(())
    }

    fn this(&mut self) -> CResult<()> {
        if !self.fr().is_arrow() {
            self.emit(Op::This);
            return Ok(());
        }
        if self.has_binding("this") {
            self.load("this");
        } else {
            self.emit(Op::GlobalThis);
        }
        Ok(())
    }

    // -------------------------------------------------------- expressões

    /// Expressão cujo valor é descartado (`i++;`, atualização do for).
    fn expr_void(&mut self, e: &Expr) -> CResult<()> {
        if let Expr::Update { inc, target, .. } = e
            && let Expr::Ident(n, _) = &**target
        {
            self.load(n);
            self.emit(if *inc { Op::Inc } else { Op::Dec });
            self.store(n);
            return Ok(());
        }
        self.expr(e)?;
        self.emit(Op::Pop);
        Ok(())
    }

    fn expr_named(&mut self, e: &Expr, name: Option<Name>) -> CResult<()> {
        match e {
            Expr::Function(f) if f.name.is_none() && name.is_some() => {
                let idx = self.function(f, name, None, false)?;
                self.emit(Op::Closure(idx));
                Ok(())
            }
            Expr::Class(c) if c.name.is_none() && name.is_some() => self.class(c, name),
            _ => self.expr(e),
        }
    }

    /// Guarda o texto do alvo de uma chamada ("obj.metodo") para erros.
    fn call_name(&mut self, at: usize, callee: &Expr) {
        fn text(e: &Expr, depth: u32) -> Option<String> {
            if depth > 6 {
                return None;
            }
            Some(match e {
                Expr::Ident(n, _) => n.to_string(),
                Expr::This => "this".into(),
                Expr::Member { obj, prop, optional } => {
                    format!("{}{}{prop}", text(obj, depth + 1)?, if *optional { "?." } else { "." })
                }
                Expr::Index { obj, .. } => format!("{}[...]", text(obj, depth + 1)?),
                Expr::PrivateMember(obj, n) => format!("{}.#{n}", text(obj, depth + 1)?),
                Expr::SuperMember(n) => format!("super.{n}"),
                Expr::Call { callee, .. } => format!("{}(...)", text(callee, depth + 1)?),
                Expr::OptionalChain(inner) => text(inner, depth)?,
                Expr::Object(props) if props.is_empty() => "{}".into(),
                Expr::Array(items) if items.is_empty() => "[]".into(),
                Expr::Unary(UnOp::Void, x) if matches!(**x, Expr::Num(_)) => "(void 0)".into(),
                Expr::Num(n) => crate::conv::number_to_string(*n),
                Expr::Str(s) => format!("\"{}\"", String::from_utf16_lossy(s)),
                Expr::Null => "null".into(),
                Expr::Bool(b) => b.to_string(),
                _ if depth > 0 => "(intermediate value)".into(),
                _ => return None,
            })
        }
        if let Some(t) = text(callee, 0) {
            let c = self.konst(&t);
            self.f().call_names.push((at as u32, c));
        }
    }

    fn opt_site(&mut self, extra: u32) {
        let p = self.emit(Op::JumpIfNullish(0));
        if let Some(l) = self.f().opt.last_mut() {
            l.push((p, extra));
        }
    }

    fn args(&mut self, args: &[ArrayElem]) -> CResult<Option<u32>> {
        if args.iter().any(|a| matches!(a, ArrayElem::Spread(_))) {
            self.array_lit(args)?;
            Ok(None)
        } else {
            for a in args {
                if let ArrayElem::Expr(e) = a {
                    self.expr(e)?;
                }
            }
            Ok(Some(args.len() as u32))
        }
    }

    fn array_lit(&mut self, elems: &[ArrayElem]) -> CResult<()> {
        self.emit(Op::NewArray);
        for el in elems {
            match el {
                ArrayElem::Expr(e) => {
                    self.expr(e)?;
                    self.emit(Op::ArrayPush);
                }
                ArrayElem::Spread(e) => {
                    self.expr(e)?;
                    self.emit(Op::ArraySpread);
                }
                ArrayElem::Hole => {
                    self.emit(Op::ArrayHole);
                }
            }
        }
        Ok(())
    }

    /// Empilha [função this] para uma chamada.
    fn callee(&mut self, callee: &Expr, optional: bool) -> CResult<()> {
        match callee {
            Expr::Member { obj, prop, optional: o2 } => {
                self.expr(obj)?;
                if *o2 {
                    self.opt_site(0);
                }
                self.emit(Op::Dup);
                let c = self.konst(prop);
                self.emit(Op::GetProp(c));
                if optional {
                    self.opt_site(1);
                }
                self.emit(Op::Swap);
            }
            Expr::Index { obj, index, optional: o2 } => {
                self.expr(obj)?;
                if *o2 {
                    self.opt_site(0);
                }
                self.emit(Op::Dup);
                self.expr(index)?;
                self.emit(Op::GetIndex);
                if optional {
                    self.opt_site(1);
                }
                self.emit(Op::Swap);
            }
            Expr::PrivateMember(obj, name) => {
                self.expr(obj)?;
                self.emit(Op::Dup);
                self.load_private(name)?;
                self.emit(Op::GetPrivate);
                if optional {
                    self.opt_site(1);
                }
                self.emit(Op::Swap);
            }
            Expr::SuperMember(name) => {
                self.this()?;
                self.emit(Op::Dup);
                let c = self.konst(name);
                self.emit(Op::GetSuper(c));
                if optional {
                    self.opt_site(1);
                }
                self.emit(Op::Swap);
            }
            Expr::SuperIndex(idx) => {
                self.this()?;
                self.emit(Op::Dup);
                self.expr(idx)?;
                self.emit(Op::GetSuperIndex);
                if optional {
                    self.opt_site(1);
                }
                self.emit(Op::Swap);
            }
            _ => {
                self.expr(callee)?;
                if optional {
                    self.opt_site(0);
                }
                self.emit(Op::Undef);
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr) -> CResult<()> {
        match e {
            Expr::Num(n) => {
                self.emit(Op::Num(*n));
            }
            Expr::BigInt(text) => {
                let Some(b) = crate::bigint::BigInt::parse_literal(text) else {
                    return syntax(format!("literal BigInt inválido: {text}"), 0);
                };
                let f = self.f();
                f.consts.push(Value::BigInt(Rc::new(b)));
                let i = (f.consts.len() - 1) as u32;
                self.emit(Op::Str(i));
            }
            Expr::Str(s) => {
                let c = self.konst_js(JsStr::from_slice(s));
                self.emit(Op::Str(c));
            }
            Expr::Template(parts, exprs) => {
                let c = self.konst_js(JsStr::from_slice(&parts[0]));
                self.emit(Op::Str(c));
                for (i, x) in exprs.iter().enumerate() {
                    self.expr(x)?;
                    self.emit(Op::ToStr);
                    self.emit(Op::Add);
                    if let Some(p) = parts.get(i + 1)
                        && !p.is_empty()
                    {
                        let c = self.konst_js(JsStr::from_slice(p));
                        self.emit(Op::Str(c));
                        self.emit(Op::Add);
                    }
                }
            }
            Expr::Tagged { tag, cooked, raw, exprs } => {
                self.callee(tag, false)?;
                let idx = self.fr().templates.len() as u32;
                self.f().templates.push((cooked.clone(), raw.clone()));
                self.emit(Op::TemplateObj(idx));
                for x in exprs {
                    self.expr(x)?;
                }
                self.emit(Op::Call(1 + exprs.len() as u32));
            }
            Expr::Regex(p, f) => {
                let a = self.konst(p);
                let b = self.konst(f);
                self.emit(Op::Regex(a, b));
            }
            Expr::Ident(n, line) => {
                self.line(*line);
                self.load(n);
            }
            Expr::This => self.this()?,
            Expr::Null => {
                self.emit(Op::Null);
            }
            Expr::Bool(b) => {
                self.emit(if *b { Op::True } else { Op::False });
            }
            Expr::Array(elems) => self.array_lit(elems)?,
            Expr::Object(props) => self.object(props)?,
            Expr::Function(f) => {
                let self_binding = f.name.is_some() && f.kind == FnKind::Normal;
                let idx = self.function(f, None, None, self_binding)?;
                self.emit(Op::Closure(idx));
            }
            Expr::Class(c) => self.class(c, None)?,
            Expr::Unary(op, x) => self.unary(*op, x)?,
            Expr::Update { inc, prefix, target } => self.update(*inc, *prefix, target)?,
            Expr::Binary(op, l, r) => {
                self.expr(l)?;
                self.expr(r)?;
                self.emit(match op {
                    BinOp::Add => Op::Add,
                    BinOp::Sub => Op::Sub,
                    BinOp::Mul => Op::Mul,
                    BinOp::Div => Op::Div,
                    BinOp::Mod => Op::Mod,
                    BinOp::Exp => Op::Exp,
                    BinOp::BitAnd => Op::BitAnd,
                    BinOp::BitOr => Op::BitOr,
                    BinOp::BitXor => Op::BitXor,
                    BinOp::Shl => Op::Shl,
                    BinOp::Shr => Op::Shr,
                    BinOp::UShr => Op::UShr,
                    BinOp::Eq => Op::Eq,
                    BinOp::Ne => Op::Ne,
                    BinOp::StrictEq => Op::StrictEq,
                    BinOp::StrictNe => Op::StrictNe,
                    BinOp::Lt => Op::Lt,
                    BinOp::Le => Op::Le,
                    BinOp::Gt => Op::Gt,
                    BinOp::Ge => Op::Ge,
                    BinOp::In => Op::In,
                    BinOp::InstanceOf => Op::InstanceOf,
                });
            }
            Expr::Logical(op, l, r) => {
                self.expr(l)?;
                let j = self.emit(match op {
                    LogOp::And => Op::JumpIfFalseKeep(0),
                    LogOp::Or => Op::JumpIfTrueKeep(0),
                    LogOp::Nullish => Op::JumpIfNotNullishKeep(0),
                });
                self.expr(r)?;
                self.patch(j);
            }
            Expr::Assign(op, target, value) => self.assign(*op, target, value)?,
            Expr::Cond(t, c, a) => {
                self.expr(t)?;
                let j = self.emit(Op::JumpIfFalse(0));
                self.expr(c)?;
                let j2 = self.emit(Op::Jump(0));
                self.patch(j);
                self.expr(a)?;
                self.patch(j2);
            }
            Expr::Call {
                callee,
                args,
                optional,
                line,
            } => {
                self.callee(callee, *optional)?;
                let argc = self.args(args)?;
                self.line(*line);
                let at = match argc {
                    Some(n) => self.emit(Op::Call(n)),
                    None => self.emit(Op::CallSpread),
                };
                self.call_name(at, callee);
            }
            Expr::New(callee, args, line) => {
                self.expr(callee)?;
                let argc = self.args(args)?;
                self.line(*line);
                let at = match argc {
                    Some(n) => self.emit(Op::New(n)),
                    None => self.emit(Op::NewSpread),
                };
                self.call_name(at, callee);
            }
            Expr::Member { obj, prop, optional } => {
                self.expr(obj)?;
                if *optional {
                    self.opt_site(0);
                }
                let c = self.konst(prop);
                self.emit(Op::GetProp(c));
            }
            Expr::Index { obj, index, optional } => {
                self.expr(obj)?;
                if *optional {
                    self.opt_site(0);
                }
                self.expr(index)?;
                self.emit(Op::GetIndex);
            }
            Expr::PrivateMember(obj, name) => {
                self.expr(obj)?;
                self.load_private(name)?;
                self.emit(Op::GetPrivate);
            }
            Expr::PrivateIn(name, obj) => {
                self.load_private(name)?;
                self.expr(obj)?;
                self.emit(Op::HasPrivate);
            }
            Expr::OptionalChain(inner) => {
                self.f().opt.push(Vec::new());
                self.expr(inner)?;
                let sites = self.f().opt.pop().unwrap_or_default();
                if !sites.is_empty() {
                    let mut ends = vec![self.emit(Op::Jump(0))];
                    for (pos, extra) in sites {
                        self.patch(pos);
                        for _ in 0..=extra {
                            self.emit(Op::Pop);
                        }
                        self.emit(Op::Undef);
                        ends.push(self.emit(Op::Jump(0)));
                    }
                    for j in ends {
                        self.patch(j);
                    }
                }
            }
            Expr::SuperMember(name) => {
                self.this()?;
                let c = self.konst(name);
                self.emit(Op::GetSuper(c));
            }
            Expr::SuperIndex(idx) => {
                self.this()?;
                self.expr(idx)?;
                self.emit(Op::GetSuperIndex);
            }
            Expr::SuperCall(args) => {
                if self.fr().is_arrow() {
                    return syntax("super() dentro de arrow function ainda não é suportado", 0);
                }
                let argc = self.args(args)?;
                match argc {
                    Some(n) => self.emit(Op::SuperCall(n)),
                    None => self.emit(Op::SuperCallSpread),
                };
                if self.fr().scopes.first().is_some_and(|s| s.index.contains_key("this")) {
                    self.emit(Op::Dup);
                    self.init("this");
                }
            }
            Expr::Seq(list) => {
                for (i, x) in list.iter().enumerate() {
                    self.expr(x)?;
                    if i + 1 < list.len() {
                        self.emit(Op::Pop);
                    }
                }
            }
            Expr::Yield(arg, delegate) => {
                match arg {
                    Some(a) => self.expr(a)?,
                    None => {
                        self.emit(Op::Undef);
                    }
                }
                let async_gen = self.fr().is_async;
                if async_gen && !*delegate {
                    // em geradores async, yield espera o valor
                    self.emit(Op::Await);
                    self.emit(Op::AwaitResume);
                }
                if *delegate && async_gen {
                    return syntax("yield* em gerador async ainda não é suportado", 0);
                }
                if *delegate {
                    self.emit(Op::GetIter);
                    self.emit(Op::Undef);
                    self.emit(Op::Num(0.0));
                    let top = self.here();
                    let step = self.emit(Op::DelegateStep(0));
                    self.emit(Op::Yield);
                    self.emit(Op::Jump(top as u32));
                    self.patch(step);
                    let normal = self.emit(Op::JumpIfFalse(0));
                    self.emit_return()?;
                    self.patch(normal);
                } else {
                    self.emit(Op::Yield);
                    let resume = self.emit(Op::GenResume(0));
                    if async_gen {
                        self.emit(Op::Await);
                        self.emit(Op::AwaitResume);
                    }
                    self.emit_return()?;
                    self.patch(resume);
                }
            }
            Expr::Await(arg) => {
                self.expr(arg)?;
                self.emit(Op::Await);
                self.emit(Op::AwaitResume);
            }
            Expr::NewTarget => {
                if !self.fr().is_arrow() {
                    self.emit(Op::NewTarget);
                } else if self.has_binding("new.target") {
                    self.load("new.target");
                } else {
                    self.emit(Op::Undef);
                }
            }
            Expr::Spread(_) => return syntax("'...' fora de lugar", 0),
            Expr::Unsupported(what) => {
                let c = self.konst(&format!("{what} ainda não é suportado"));
                self.emit(Op::ThrowError(ERR_SYNTAX, c));
            }
            Expr::ImportCall(spec) => {
                // __ferroImport(especificador, quem importa) → Promise
                let g = self.konst("__ferroImport");
                self.emit(Op::GetGlobal(g));
                self.emit(Op::Undef);
                self.expr(spec)?;
                let file = self.file.clone();
                let f = self.konst(&file);
                self.emit(Op::Str(f));
                self.emit(Op::Call(2));
            }
            Expr::ImportMeta => self.load("__meta"),
        }
        Ok(())
    }

    fn prop_key(&mut self, k: &PropKeyExpr) -> CResult<()> {
        match k {
            PropKeyExpr::Name(n) => {
                let c = self.konst(n);
                self.emit(Op::Str(c));
            }
            PropKeyExpr::Computed(e) => {
                self.expr(e)?;
                self.emit(Op::ToPropertyKey);
            }
            PropKeyExpr::Private(n) => self.load_private(n)?,
        }
        Ok(())
    }

    fn object(&mut self, props: &[Prop]) -> CResult<()> {
        self.emit(Op::NewObject);
        for p in props {
            match p.kind {
                PropKind::Init => match (&p.key, &p.value) {
                    (key, Expr::Function(f)) if f.kind == FnKind::Method => {
                        self.prop_key(key)?;
                        let name = match key {
                            PropKeyExpr::Name(n) => Some(n.clone()),
                            _ => None,
                        };
                        let idx = self.function(f, name, None, false)?;
                        self.emit(Op::Closure(idx));
                        self.emit(Op::DefineMethod(METHOD, true));
                    }
                    (PropKeyExpr::Name(n), v) if &**n == "__proto__" => {
                        self.expr(v)?;
                        self.emit(Op::SetProto);
                    }
                    (PropKeyExpr::Name(n), v) => {
                        self.expr_named(v, Some(n.clone()))?;
                        let c = self.konst(n);
                        self.emit(Op::DefineName(c));
                    }
                    (PropKeyExpr::Computed(k), v) => {
                        self.expr(k)?;
                        self.emit(Op::ToPropertyKey);
                        self.expr(v)?;
                        let anon = match v {
                            Expr::Function(f) => f.name.is_none(),
                            Expr::Class(c) => c.name.is_none(),
                            _ => false,
                        };
                        self.emit(if anon { Op::DefinePropNamed } else { Op::DefineProp });
                    }
                    (PropKeyExpr::Private(_), _) => return syntax("nome privado em objeto literal", 0),
                },
                PropKind::Get | PropKind::Set => {
                    self.prop_key(&p.key)?;
                    let name = match &p.key {
                        PropKeyExpr::Name(n) => Some(n.clone()),
                        _ => None,
                    };
                    if let Expr::Function(f) = &p.value {
                        let idx = self.function(f, name, None, false)?;
                        self.emit(Op::Closure(idx));
                    }
                    let kind = if matches!(p.kind, PropKind::Get) { GETTER } else { SETTER };
                    self.emit(Op::DefineMethod(kind, true));
                }
                PropKind::Spread => {
                    self.expr(&p.value)?;
                    self.emit(Op::CopyProps);
                }
            }
        }
        Ok(())
    }

    fn unary(&mut self, op: UnOp, x: &Expr) -> CResult<()> {
        match op {
            UnOp::Delete => match x {
                Expr::Member { obj, prop, optional } => {
                    self.expr(obj)?;
                    if *optional {
                        self.opt_site(0);
                    }
                    let c = self.konst(prop);
                    self.emit(Op::DeleteProp(c));
                }
                Expr::Index { obj, index, optional } => {
                    self.expr(obj)?;
                    if *optional {
                        self.opt_site(0);
                    }
                    self.expr(index)?;
                    self.emit(Op::DeleteIndex);
                }
                Expr::Ident(n, _) => {
                    if matches!(self.resolve(n).res, Res::Global) {
                        let c = self.konst(n);
                        self.emit(Op::DeleteGlobal(c));
                    } else {
                        self.emit(Op::False);
                    }
                }
                Expr::OptionalChain(inner) => {
                    // delete a?.b
                    self.f().opt.push(Vec::new());
                    self.unary(UnOp::Delete, inner)?;
                    let sites = self.f().opt.pop().unwrap_or_default();
                    if !sites.is_empty() {
                        let mut ends = vec![self.emit(Op::Jump(0))];
                        for (pos, extra) in sites {
                            self.patch(pos);
                            for _ in 0..=extra {
                                self.emit(Op::Pop);
                            }
                            self.emit(Op::True);
                            ends.push(self.emit(Op::Jump(0)));
                        }
                        for j in ends {
                            self.patch(j);
                        }
                    }
                }
                _ => {
                    self.expr(x)?;
                    self.emit(Op::Pop);
                    self.emit(Op::True);
                }
            },
            UnOp::Typeof => {
                if let Expr::Ident(n, _) = x
                    && matches!(self.resolve(n).res, Res::Global)
                    && !self.imports.contains_key(n)
                {
                    let c = self.konst(n);
                    self.emit(Op::TypeofGlobal(c));
                } else {
                    self.expr(x)?;
                    self.emit(Op::Typeof);
                }
            }
            UnOp::Void => {
                self.expr(x)?;
                self.emit(Op::Pop);
                self.emit(Op::Undef);
            }
            UnOp::Neg => {
                if let Expr::Num(n) = x {
                    self.emit(Op::Num(-n));
                } else {
                    self.expr(x)?;
                    self.emit(Op::Neg);
                }
            }
            UnOp::Plus => {
                self.expr(x)?;
                self.emit(Op::Plus);
            }
            UnOp::Not => {
                self.expr(x)?;
                self.emit(Op::Not);
            }
            UnOp::BitNot => {
                self.expr(x)?;
                self.emit(Op::BitNot);
            }
        }
        Ok(())
    }

    fn update(&mut self, inc: bool, prefix: bool, target: &Expr) -> CResult<()> {
        let step = if inc { Op::Inc } else { Op::Dec };
        match target {
            Expr::Ident(n, _) => {
                self.load(n);
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(step);
                    self.emit(Op::Dup);
                } else {
                    self.emit(Op::Dup);
                    self.emit(step);
                }
                self.store(n);
            }
            Expr::Member { obj, prop, .. } => {
                let c = self.konst(prop);
                self.expr(obj)?;
                self.emit(Op::Dup);
                self.emit(Op::GetProp(c));
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(step);
                    self.emit(Op::SetProp(c));
                } else {
                    self.emit(Op::Dup);
                    self.emit(Op::Rot3);
                    self.emit(step);
                    self.emit(Op::SetProp(c));
                    self.emit(Op::Pop);
                }
            }
            Expr::Index { obj, index, .. } => {
                self.expr(obj)?;
                self.expr(index)?;
                self.emit(Op::ToPropertyKey);
                self.emit(Op::Dup2);
                self.emit(Op::GetIndex);
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(step);
                    self.emit(Op::SetIndex);
                } else {
                    self.emit(Op::Dup);
                    self.emit(Op::Rot4);
                    self.emit(step);
                    self.emit(Op::SetIndex);
                    self.emit(Op::Pop);
                }
            }
            Expr::PrivateMember(obj, name) => {
                self.expr(obj)?;
                self.load_private(name)?;
                self.emit(Op::Dup2);
                self.emit(Op::GetPrivate);
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(step);
                    self.emit(Op::SetPrivate);
                } else {
                    self.emit(Op::Dup);
                    self.emit(Op::Rot4);
                    self.emit(step);
                    self.emit(Op::SetPrivate);
                    self.emit(Op::Pop);
                }
            }
            Expr::SuperMember(name) => {
                let c = self.konst(name);
                self.this()?;
                self.emit(Op::Dup);
                self.emit(Op::GetSuper(c));
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(step);
                    self.emit(Op::SetSuper(c));
                } else {
                    self.emit(Op::Dup);
                    self.emit(Op::Rot3);
                    self.emit(step);
                    self.emit(Op::SetSuper(c));
                    self.emit(Op::Pop);
                }
            }
            _ => return syntax("operando inválido para ++/--", 0),
        }
        Ok(())
    }

    fn bin_op(op: BinOp) -> Op {
        match op {
            BinOp::Add => Op::Add,
            BinOp::Sub => Op::Sub,
            BinOp::Mul => Op::Mul,
            BinOp::Div => Op::Div,
            BinOp::Mod => Op::Mod,
            BinOp::Exp => Op::Exp,
            BinOp::BitAnd => Op::BitAnd,
            BinOp::BitOr => Op::BitOr,
            BinOp::BitXor => Op::BitXor,
            BinOp::Shl => Op::Shl,
            BinOp::Shr => Op::Shr,
            BinOp::UShr => Op::UShr,
            BinOp::Eq => Op::Eq,
            BinOp::Ne => Op::Ne,
            BinOp::StrictEq => Op::StrictEq,
            BinOp::StrictNe => Op::StrictNe,
            BinOp::Lt => Op::Lt,
            BinOp::Le => Op::Le,
            BinOp::Gt => Op::Gt,
            BinOp::Ge => Op::Ge,
            BinOp::In => Op::In,
            BinOp::InstanceOf => Op::InstanceOf,
        }
    }

    fn assign(&mut self, op: AssignOp, target: &Pattern, value: &Expr) -> CResult<()> {
        match op {
            AssignOp::Assign => match target {
                Pattern::Ident(n) => {
                    self.expr_named(value, Some(n.clone()))?;
                    self.emit(Op::Dup);
                    self.store(n);
                }
                Pattern::Target(t) => match &**t {
                    Expr::Member { obj, prop, .. } => {
                        self.expr(obj)?;
                        self.expr(value)?;
                        let c = self.konst(prop);
                        self.emit(Op::SetProp(c));
                    }
                    Expr::Index { obj, index, .. } => {
                        self.expr(obj)?;
                        self.expr(index)?;
                        self.expr(value)?;
                        self.emit(Op::SetIndex);
                    }
                    Expr::PrivateMember(obj, name) => {
                        self.expr(obj)?;
                        self.load_private(name)?;
                        self.expr(value)?;
                        self.emit(Op::SetPrivate);
                    }
                    Expr::SuperMember(name) => {
                        self.this()?;
                        self.expr(value)?;
                        let c = self.konst(name);
                        self.emit(Op::SetSuper(c));
                    }
                    Expr::SuperIndex(idx) => {
                        self.this()?;
                        self.expr(idx)?;
                        self.expr(value)?;
                        self.emit(Op::SetSuperIndex);
                    }
                    _ => return syntax("alvo de atribuição inválido", 0),
                },
                Pattern::Object { .. } | Pattern::Array { .. } => {
                    self.expr(value)?;
                    self.emit(Op::Dup);
                    self.pattern(target, Bind::Store)?;
                }
                Pattern::Default(..) => return syntax("atribuição inválida", 0),
            },
            AssignOp::Op(bop) => {
                let bop = Self::bin_op(bop);
                match target {
                    Pattern::Ident(n) => {
                        self.load(n);
                        self.expr(value)?;
                        self.emit(bop);
                        self.emit(Op::Dup);
                        self.store(n);
                    }
                    Pattern::Target(t) => match &**t {
                        Expr::Member { obj, prop, .. } => {
                            let c = self.konst(prop);
                            self.expr(obj)?;
                            self.emit(Op::Dup);
                            self.emit(Op::GetProp(c));
                            self.expr(value)?;
                            self.emit(bop);
                            self.emit(Op::SetProp(c));
                        }
                        Expr::Index { obj, index, .. } => {
                            self.expr(obj)?;
                            self.expr(index)?;
                            self.emit(Op::ToPropertyKey);
                            self.emit(Op::Dup2);
                            self.emit(Op::GetIndex);
                            self.expr(value)?;
                            self.emit(bop);
                            self.emit(Op::SetIndex);
                        }
                        Expr::PrivateMember(obj, name) => {
                            self.expr(obj)?;
                            self.load_private(name)?;
                            self.emit(Op::Dup2);
                            self.emit(Op::GetPrivate);
                            self.expr(value)?;
                            self.emit(bop);
                            self.emit(Op::SetPrivate);
                        }
                        Expr::SuperMember(name) => {
                            let c = self.konst(name);
                            self.this()?;
                            self.emit(Op::Dup);
                            self.emit(Op::GetSuper(c));
                            self.expr(value)?;
                            self.emit(bop);
                            self.emit(Op::SetSuper(c));
                        }
                        _ => return syntax("alvo de atribuição inválido", 0),
                    },
                    _ => return syntax("alvo de atribuição inválido", 0),
                }
            }
            AssignOp::Logical(lop) => {
                let jump = |c: &mut Compiler| {
                    c.emit(match lop {
                        LogOp::And => Op::JumpIfFalseKeep(0),
                        LogOp::Or => Op::JumpIfTrueKeep(0),
                        LogOp::Nullish => Op::JumpIfNotNullishKeep(0),
                    })
                };
                match target {
                    Pattern::Ident(n) => {
                        self.load(n);
                        let j = jump(self);
                        self.expr_named(value, Some(n.clone()))?;
                        self.emit(Op::Dup);
                        self.store(n);
                        self.patch(j);
                    }
                    Pattern::Target(t) => {
                        // [obj] ou [obj chave] ficam na pilha; no atalho, descartamos
                        let (extra, set) = match &**t {
                            Expr::Member { obj, prop, .. } => {
                                let c = self.konst(prop);
                                self.expr(obj)?;
                                self.emit(Op::Dup);
                                self.emit(Op::GetProp(c));
                                (1, Op::SetProp(c))
                            }
                            Expr::Index { obj, index, .. } => {
                                self.expr(obj)?;
                                self.expr(index)?;
                                self.emit(Op::ToPropertyKey);
                                self.emit(Op::Dup2);
                                self.emit(Op::GetIndex);
                                (2, Op::SetIndex)
                            }
                            Expr::PrivateMember(obj, name) => {
                                self.expr(obj)?;
                                self.load_private(name)?;
                                self.emit(Op::Dup2);
                                self.emit(Op::GetPrivate);
                                (2, Op::SetPrivate)
                            }
                            _ => return syntax("alvo de atribuição inválido", 0),
                        };
                        let j = jump(self);
                        self.expr(value)?;
                        self.emit(set);
                        let end = self.emit(Op::Jump(0));
                        self.patch(j);
                        // [alvo... valor] → [valor]
                        for _ in 0..extra {
                            self.emit(Op::Swap);
                            self.emit(Op::Pop);
                        }
                        self.patch(end);
                    }
                    _ => return syntax("alvo de atribuição inválido", 0),
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------- classes

    /// Compila uma classe e deixa o construtor na pilha.
    fn class(&mut self, c: &ClassDef, name: Option<Name>) -> CResult<()> {
        let cname = c.name.clone().or(name).unwrap_or_else(|| "".into());
        let saved_strict = self.fr().strict;
        self.f().strict = true;
        self.push_scope(ScopeKind::Block, key_of(c));
        if let Some(n) = &c.name {
            self.declare(n.clone(), Decl::ClassInner, false, false);
        }
        let mut privs: Vec<Name> = Vec::new();
        for m in &c.members {
            if let PropKeyExpr::Private(n) = &m.key
                && !privs.contains(n)
            {
                privs.push(n.clone());
            }
        }
        for p in &privs {
            self.declare(format!("#{p}").into(), Decl::Hidden, true, true);
        }
        for (i, m) in c.members.iter().enumerate() {
            if matches!(m.kind, MemberKind::Field(_)) && matches!(m.key, PropKeyExpr::Computed(_)) {
                self.declare(format!("%k{i}").into(), Decl::Hidden, true, true);
            }
        }
        self.open_env();
        for p in &privs {
            let k = self.konst(&format!("#{p}"));
            self.emit(Op::NewPrivate(k));
            self.init(&format!("#{p}"));
        }
        if let Some(sup) = &c.superclass {
            self.expr(sup)?;
        }
        let derived = c.superclass.is_some();
        let ctor_def = match &c.constructor {
            Some(f) => f.clone(),
            None => Rc::new(default_constructor(&cname, derived, c.line)),
        };
        let ctor_idx = self.function(&ctor_def, Some(cname.clone()), Some(c.source.clone()), false)?;
        self.emit(Op::MakeClass(ctor_idx, derived));
        // métodos e chaves computadas, em ordem
        for (i, m) in c.members.iter().enumerate() {
            let private = matches!(m.key, PropKeyExpr::Private(_));
            match &m.kind {
                MemberKind::Method | MemberKind::Getter | MemberKind::Setter if !private => {
                    let kind = match m.kind {
                        MemberKind::Getter => GETTER,
                        MemberKind::Setter => SETTER,
                        _ => METHOD,
                    };
                    self.emit(if m.is_static { Op::Over } else { Op::Dup });
                    self.prop_key(&m.key)?;
                    let name = match &m.key {
                        PropKeyExpr::Name(n) => Some(n.clone()),
                        _ => None,
                    };
                    if let Some(f) = &m.func {
                        let idx = self.function(f, name, None, false)?;
                        self.emit(Op::Closure(idx));
                    }
                    self.emit(Op::DefineMethod(kind, false));
                    self.emit(Op::Pop);
                }
                MemberKind::Field(_) => {
                    if let PropKeyExpr::Computed(e) = &m.key {
                        self.expr(e)?;
                        self.emit(Op::ToPropertyKey);
                        self.init(&format!("%k{i}"));
                    }
                }
                _ => {}
            }
        }
        // campos de instância e métodos privados
        let inst: Vec<(usize, &ClassMember)> = c
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                !m.is_static && (matches!(m.kind, MemberKind::Field(_)) || matches!(m.key, PropKeyExpr::Private(_)))
            })
            .collect();
        if !inst.is_empty() {
            let idx = self.init_function(&inst, &cname, class_init_key(c, false))?;
            self.emit(Op::Closure(idx));
            self.emit(Op::SetFields);
        }
        self.emit(Op::Pop);
        if let Some(n) = &c.name {
            self.emit(Op::Dup);
            self.init(n);
        }
        let stat: Vec<(usize, &ClassMember)> = c
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.is_static
                    && (matches!(m.kind, MemberKind::Field(_) | MemberKind::StaticBlock(_)) || matches!(m.key, PropKeyExpr::Private(_)))
            })
            .collect();
        if !stat.is_empty() {
            self.emit(Op::Dup);
            let idx = self.init_function(&stat, &cname, class_init_key(c, true))?;
            self.emit(Op::Closure(idx));
            self.emit(Op::CallInit);
        }
        self.pop_scope();
        self.f().strict = saved_strict;
        Ok(())
    }

