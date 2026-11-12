//! Máquina virtual: executa o bytecode do compilador.
//!
//! Cada chamada de função JavaScript roda num `Frame` próprio (registradores,
//! pilha de operandos, tratadores de exceção). Chamadas são recursivas no
//! Rust; o limite de profundidade vira `RangeError`, como nos navegadores.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use crate::builtins::Realm;
use crate::compiler::{Code, ERR_REFERENCE, ERR_SYNTAX, GETTER, METHOD, Op, SETTER};
use crate::string::JsStr;
use crate::value::*;

pub struct Handler {
    pub pc: usize,
    pub stack: usize,
    pub env: Option<Env>,
}

pub struct Frame {
    pub code: Rc<Code>,
    pub pc: usize,
    pub regs: Vec<Value>,
    pub stack: Vec<Value>,
    pub env: Option<Env>,
    pub this: Value,
    pub func: Option<ObjRef>,
    pub home: Option<ObjRef>,
    pub new_target: Value,
    pub args: Vec<Value>,
    pub handlers: Vec<Handler>,
    /// Parou num yield/await (o valor devolvido é o do yield/await).
    pub suspended: bool,
    /// A pausa foi um await (e não um yield).
    pub suspend_await: bool,
}

/// Uma entrada da pilha de chamadas (para `error.stack`).
pub struct Trace {
    pub code: Rc<Code>,
    pub pc: usize,
}

/// Para onde vai a saída do console.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    Log,
    Error,
}

