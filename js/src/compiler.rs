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

