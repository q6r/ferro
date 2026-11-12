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

    pub fn call_obj(&mut self, o: &ObjRef, this: Value, args: Vec<Value>) -> Result<Value, Value> {
        enum C {
            Closure(Rc<Closure>),
            Native(NativeFn),
            Bound(ObjRef, Value, Vec<Value>),
            Proxy,
            Not,
        }
        let c = match &o.borrow().kind {
            Kind::Function(Func::Closure(c)) => C::Closure(c.clone()),
            Kind::Function(Func::Native(n)) => C::Native(n.f),
            Kind::Function(Func::Bound { target, this, args }) => C::Bound(target.clone(), this.clone(), args.clone()),
            Kind::Proxy { target: Some(t), .. } if t.is_callable() => C::Proxy,
            _ => C::Not,
        };
        match c {
            C::Proxy => self.proxy_call(o, this, args),
            C::Closure(c) => {
                if c.code.is_class_constructor() {
                    let name = c.code.name.to_rust();
                    return Err(self.type_error(format!("Class constructor {name} cannot be invoked without 'new'")));
                }
                self.call_closure(&c, o, this, args, None)
            }
            C::Native(f) => {
                let mut ca = CallArgs {
                    this,
                    args,
                    new_target: None,
                    callee: o.clone(),
                };
                f(self, &mut ca)
            }
            C::Bound(target, bthis, mut bargs) => {
                bargs.extend(args);
                self.call_obj(&target, bthis, bargs)
            }
            C::Not => {
                let d = self.describe(&Value::Object(o.clone()));
                Err(self.type_error(format!("{d} is not a function")))
            }
        }
    }

    fn call_closure(&mut self, c: &Rc<Closure>, fobj: &ObjRef, this: Value, args: Vec<Value>, new_target: Option<&ObjRef>) -> Result<Value, Value> {
        let code = c.code.clone();
        let this = if code.is_arrow() {
            Value::Undefined
        } else if !code.strict && this.is_nullish() {
            Value::Object(self.realm.global.clone())
        } else if !code.strict && !matches!(this, Value::Object(_) | Value::Empty) {
            Value::Object(self.to_object(&this)?)
        } else {
            this
        };
        let nregs = code.nregs as usize;
        let nparams = code.nparams as usize;
        let var_end = code.var_end as usize;
        let (is_generator, is_async) = (code.is_generator, code.is_async);
        // quadros comuns usam vetores reaproveitados; geradores/async guardam o quadro
        let pooled = !is_generator && !is_async;
        let mut regs = if pooled { self.vpool.pop().unwrap_or_default() } else { Vec::new() };
        regs.reserve(nregs);
        let mut args = args;
        if code.needs_args {
            for i in 0..nregs {
                regs.push(if i < nparams {
                    args.get(i).cloned().unwrap_or(Value::Undefined)
                } else if i < var_end {
                    Value::Undefined
                } else {
                    Value::Empty
                });
            }
        } else {
            // ninguém mais olha os argumentos: vão direto para os registradores
            args.truncate(nparams);
            let given = args.len();
            regs.append(&mut args);
            for i in given..nregs {
                regs.push(if i < var_end { Value::Undefined } else { Value::Empty });
            }
            regs.truncate(nregs.max(0));
            if pooled && args.capacity() > 0 {
                self.vpool.push(std::mem::take(&mut args));
            }
        }
        let stack = if pooled { self.vpool.pop().unwrap_or_default() } else { Vec::with_capacity(8) };
        let mut frame = Frame {
            code,
            pc: 0,
            regs,
            stack,
            env: c.env.clone(),
            this,
            func: Some(fobj.clone()),
            home: c.home.clone(),
            new_target: match new_target {
                Some(n) => Value::Object(n.clone()),
                None => Value::Undefined,
            },
            args,
            handlers: Vec::new(),
            suspended: false,
            suspend_await: false,
        };
        if is_generator && is_async {
            let proto = match self.get_obj(fobj, &PropKey::str("prototype"))? {
                Value::Object(p) => p,
                _ => self.realm.async_generator_proto.clone(),
            };
            self.run_frame(&mut frame)?;
            let state = if frame.suspended {
                crate::builtins::generator::GenState::Start
            } else {
                crate::builtins::generator::GenState::Done
            };
            frame.suspended = false;
            return Ok(Value::Object(ObjRef::new(Object::new(
                Some(proto),
                Kind::AsyncGenerator(Box::new(crate::builtins::async_generator::AsyncGenData {
                    frame: Some(Box::new(frame)),
                    state,
                    queue: std::collections::VecDeque::new(),
                })),
            ))));
        }
        if is_generator {
            // o prólogo (parâmetros) roda já; o corpo espera o primeiro next()
            let proto = match self.get_obj(fobj, &PropKey::str("prototype"))? {
                Value::Object(p) => p,
                _ => self.realm.generator_proto.clone(),
            };
            self.run_frame(&mut frame)?;
            let state = if frame.suspended {
                crate::builtins::generator::GenState::Start
            } else {
                crate::builtins::generator::GenState::Done
            };
            frame.suspended = false;
            return Ok(Value::Object(ObjRef::new(Object::new(
                Some(proto),
                Kind::Generator(Box::new(crate::builtins::generator::GenData {
                    frame: Some(Box::new(frame)),
                    state,
                })),
            ))));
        }
        if is_async {
            return self.async_call(frame);
        }
        let r = self.run_frame(&mut frame);
        // devolve os vetores (vazios) para a próxima chamada
        for mut v in [std::mem::take(&mut frame.regs), std::mem::take(&mut frame.stack), std::mem::take(&mut frame.args)] {
            if v.capacity() > 0 && v.capacity() <= 64 && self.vpool.len() < 256 {
                v.clear();
                self.vpool.push(v);
            }
        }
        r
    }

