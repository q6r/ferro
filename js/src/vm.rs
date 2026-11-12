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

/// Fuso horário num instante: deslocamento (minutos, local = UTC + isto) e nomes.
#[derive(Clone, Debug)]
pub struct TzInfo {
    pub offset_min: f64,
    pub short: String,
    pub long: String,
}

/// Função do hospedeiro: fuso (None = local) num instante UTC em ms.
pub type TzFn = Box<dyn Fn(Option<&str>, f64) -> Option<TzInfo>>;

pub struct LexBinding {
    pub value: Value,
    pub is_const: bool,
}

/// Célula de uma variável léxica global; o código guarda a célula na
/// primeira busca e depois acessa direto (sem tabela hash).
pub type LexCell = Rc<std::cell::RefCell<LexBinding>>;

pub struct Vm {
    pub realm: Realm,
    /// let/const/class globais (compartilhados entre scripts).
    pub lex: crate::fxhash::FxHashMap<JsStr, LexCell>,
    /// Muda a cada léxica global nova (invalida o cache do objeto global).
    pub lex_gen: u32,
    pub trace: Vec<Trace>,
    /// Vetores reaproveitados entre chamadas (registradores e pilha dos quadros).
    pub(crate) vpool: Vec<Vec<Value>>,
    pub cur_pc: usize,
    pub max_depth: usize,
    pub output: Box<dyn FnMut(Out, &str)>,
    pub symbol_registry: HashMap<JsStr, Sym>,
    pub rng: u64,
    pub console_timers: HashMap<String, std::time::Instant>,
    pub console_counts: HashMap<String, u64>,
    pub console_indent: usize,
    pub microtasks: VecDeque<crate::builtins::promise::Job>,
    pub pending_rejections: Vec<ObjRef>,
    pub timers: Vec<crate::builtins::timers::Timer>,
    pub timer_seq: u32,
    pub epoch: std::time::Instant,
    /// Padrões de regex já compilados (fonte, flags).
    pub regex_cache: HashMap<(JsStr, JsStr), Rc<crate::regex::Program>>,
    /// Idioma padrão (Intl, toLocaleString). O hospedeiro define.
    pub locale: String,
    /// Fusos horários (o hospedeiro define; o padrão só conhece UTC).
    pub tz: TzFn,
    /// Estado do hospedeiro (o navegador guarda aqui o DOM da página).
    pub host: Option<Rc<dyn std::any::Any>>,
    /// Último recurso para um nome global que não existe (no navegador:
    /// elementos com `id` viram variáveis globais).
    pub global_miss: Option<fn(&mut Vm, &crate::JsStr) -> Option<Value>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Default,
    Number,
    String,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
