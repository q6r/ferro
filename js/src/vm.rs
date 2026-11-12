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
    pub fn new() -> Vm {
        let realm = Realm::new();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x2545F4914F6CDD1D);
        Vm {
            realm,
            lex: Default::default(),
            lex_gen: 0,
            trace: Vec::new(),
            cur_pc: 0,
            max_depth: 2000,
            output: Box::new(|kind, text| match kind {
                Out::Log => println!("{text}"),
                Out::Error => eprintln!("{text}"),
            }),
            symbol_registry: HashMap::new(),
            rng: seed | 1,
            console_timers: HashMap::new(),
            vpool: Vec::new(),
            console_counts: HashMap::new(),
            console_indent: 0,
            microtasks: VecDeque::new(),
            pending_rejections: Vec::new(),
            timers: Vec::new(),
            timer_seq: 0,
            epoch: std::time::Instant::now(),
            regex_cache: HashMap::new(),
            locale: "en-US".into(),
            tz: Box::new(|zone, _| match zone {
                None | Some("UTC") | Some("Etc/UTC") | Some("GMT") | Some("Etc/GMT") => Some(TzInfo {
                    offset_min: 0.0,
                    short: "UTC".into(),
                    long: "Coordinated Universal Time".into(),
                }),
                _ => None,
            }),
            host: None,
            global_miss: None,
        }
    }

    /// Executa um script compilado no escopo global.
    pub fn run_script(&mut self, code: Rc<Code>) -> Result<Value, Value> {
        let nregs = code.nregs as usize;
        let mut regs = vec![Value::Empty; nregs];
        for r in regs.iter_mut().take(code.var_end as usize) {
            *r = Value::Undefined;
        }
        let mut frame = Frame {
            code,
            pc: 0,
            regs,
            stack: Vec::with_capacity(16),
            env: None,
            this: Value::Object(self.realm.global.clone()),
            func: None,
            home: None,
            new_target: Value::Undefined,
            args: Vec::new(),
            handlers: Vec::new(),
            suspended: false,
            suspend_await: false,
        };
        self.run_frame(&mut frame)
    }

    /// Agora, em ms desde 1970 (Date.now).
    pub fn epoch_now_ms(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0)
    }

    pub fn run_frame(&mut self, f: &mut Frame) -> Result<Value, Value> {
        if self.trace.len() >= self.max_depth {
            return Err(self.range_error("Maximum call stack size exceeded"));
        }
        if let Some(t) = self.trace.last_mut() {
            t.pc = self.cur_pc;
        }
        self.trace.push(Trace {
            code: f.code.clone(),
            pc: 0,
        });
        let result = loop {
            match self.exec(f) {
                Ok(v) => break Ok(v),
                Err(e) => match f.handlers.pop() {
                    Some(h) => {
                        f.stack.truncate(h.stack);
                        f.env = h.env;
                        f.stack.push(e);
                        f.pc = h.pc;
                    }
                    None => break Err(e),
                },
            }
        };
        self.trace.pop();
        if let Some(t) = self.trace.last() {
            self.cur_pc = t.pc;
        }
        result
    }

    // --------------------------------------------------------- chamadas

    pub fn call(&mut self, f: &Value, this: Value, args: Vec<Value>) -> Result<Value, Value> {
        match f {
            Value::Object(o) => self.call_obj(o, this, args),
            other => {
                let d = self.describe(other);
                Err(self.type_error(format!("{d} is not a function")))
            }
        }
    }

