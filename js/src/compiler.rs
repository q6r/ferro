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

