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

    /// `new ctor(...args)`.
    pub fn construct(&mut self, ctor: &ObjRef, args: Vec<Value>, new_target: &ObjRef) -> Result<Value, Value> {
        enum C {
            Closure(Rc<Closure>),
            Native(NativeFn, bool),
            Bound(ObjRef, Vec<Value>),
            Proxy,
            Not,
        }
        let c = match &ctor.borrow().kind {
            Kind::Function(Func::Closure(c)) => C::Closure(c.clone()),
            Kind::Function(Func::Native(n)) => C::Native(n.f, n.ctor),
            Kind::Function(Func::Bound { target, args, .. }) => C::Bound(target.clone(), args.clone()),
            Kind::Proxy { target: Some(_), .. } => C::Proxy,
            _ => C::Not,
        };
        match c {
            C::Proxy => self.proxy_construct(ctor, args, new_target),
            C::Closure(c) if c.code.is_constructor() => {
                if c.code.kind == crate::ast::FnKind::DerivedConstructor {
                    return self.call_closure(&c, ctor, Value::Empty, args, Some(new_target));
                }
                let proto = self.proto_from_ctor(Some(new_target), &self.realm.object_proto.clone())?;
                let obj = ObjRef::new(Object::new(Some(proto), Kind::Ordinary));
                if let Some(fields) = &c.fields {
                    self.call_obj(fields, Value::Object(obj.clone()), Vec::new())?;
                }
                let r = self.call_closure(&c, ctor, Value::Object(obj.clone()), args, Some(new_target))?;
                Ok(match r {
                    Value::Object(_) => r,
                    _ => Value::Object(obj),
                })
            }
            C::Native(f, true) => {
                let mut ca = CallArgs {
                    this: Value::Undefined,
                    args,
                    new_target: Some(new_target.clone()),
                    callee: ctor.clone(),
                };
                f(self, &mut ca)
            }
            C::Bound(target, mut bargs) => {
                bargs.extend(args);
                let nt = if new_target.ptr_eq(ctor) { target.clone() } else { new_target.clone() };
                self.construct(&target, bargs, &nt)
            }
            _ => {
                let d = self.describe(&Value::Object(ctor.clone()));
                Err(self.type_error(format!("{d} is not a constructor")))
            }
        }
    }

    /// O protótipo de um objeto criado por `new` (respeita subclasses).
    pub fn proto_from_ctor(&mut self, new_target: Option<&ObjRef>, default: &ObjRef) -> Result<ObjRef, Value> {
        if let Some(nt) = new_target {
            let p = self.get_obj(nt, &PropKey::str("prototype"))?;
            if let Value::Object(p) = p {
                return Ok(p);
            }
        }
        Ok(default.clone())
    }

    /// Inicializa os campos de classe da função `func` em `obj`.
    fn init_fields(&mut self, func: &Option<ObjRef>, obj: &Value) -> Result<(), Value> {
        let fields = match func {
            Some(f) => match &f.borrow().kind {
                Kind::Function(Func::Closure(c)) => c.fields.clone(),
                _ => None,
            },
            None => None,
        };
        if let Some(fields) = fields {
            self.call_obj(&fields, obj.clone(), Vec::new())?;
        }
        Ok(())
    }

    // ---------------------------------------------------------- execução

    fn exec(&mut self, f: &mut Frame) -> Result<Value, Value> {
        let code = f.code.clone();
        let ops = &code.ops[..];
        macro_rules! pop {
            () => {
                f.stack.pop().unwrap_or_default()
            };
        }
        macro_rules! peek {
            () => {
                f.stack.last().cloned().unwrap_or_default()
            };
        }
        macro_rules! num2 {
            ($k:expr) => {{
                let b = pop!();
                let a = pop!();
                let r = match (&a, &b) {
                    (Value::Number(x), Value::Number(y)) => Value::Number(crate::ops::num_op($k, *x, *y)),
                    _ => self.arith($k, &a, &b)?,
                };
                f.stack.push(r);
            }};
        }
        macro_rules! cmp {
            ($swap:expr, $test:expr) => {{
                let b = pop!();
                let a = pop!();
                let r = match (&a, &b) {
                    (Value::Number(x), Value::Number(y)) => {
                        let (x, y) = if $swap { (*y, *x) } else { (*x, *y) };
                        if x.is_nan() || y.is_nan() { None } else { Some(x < y || ($test && x == y)) }
                    }
                    _ => {
                        let r = if $swap { self.less_than(&b, &a, false)? } else { self.less_than(&a, &b, true)? };
                        // a <= b é !(b < a)
                        match r {
                            None => None,
                            Some(lt) => Some(lt),
                        }
                    }
                };
                f.stack.push(Value::Bool(r.unwrap_or(false)));
            }};
        }
        loop {
            let op = ops[f.pc];
            self.cur_pc = f.pc;
            f.pc += 1;
            match op {
                Op::Undef => f.stack.push(Value::Undefined),
                Op::Null => f.stack.push(Value::Null),
                Op::True => f.stack.push(Value::Bool(true)),
                Op::False => f.stack.push(Value::Bool(false)),
                Op::Num(n) => f.stack.push(Value::Number(n)),
                Op::Str(i) => f.stack.push(code.consts[i as usize].clone()),
                Op::Pop => {
                    f.stack.pop();
                }
                Op::Dup => {
                    let v = peek!();
                    f.stack.push(v);
                }
                Op::Dup2 => {
                    let n = f.stack.len();
                    let a = f.stack[n - 2].clone();
                    let b = f.stack[n - 1].clone();
                    f.stack.push(a);
                    f.stack.push(b);
                }
                Op::Swap => {
                    let n = f.stack.len();
                    f.stack.swap(n - 1, n - 2);
                }
                Op::Over => {
                    let n = f.stack.len();
                    let v = f.stack[n - 2].clone();
                    f.stack.push(v);
                }
                Op::Rot3 => {
                    let c = pop!();
                    let n = f.stack.len();
                    f.stack.insert(n - 2, c);
                }
                Op::Roll3 => {
                    let n = f.stack.len();
                    let a = f.stack.remove(n - 3);
                    f.stack.push(a);
                }
                Op::Rot4 => {
                    let d = pop!();
                    let n = f.stack.len();
                    f.stack.insert(n - 3, d);
                }
                Op::GetReg(r) => f.stack.push(f.regs[r as usize].clone()),
                Op::SetReg(r) => f.regs[r as usize] = pop!(),
                Op::GetReg2(a, b) => {
                    let x = f.regs[a as usize].clone();
                    let y = f.regs[b as usize].clone();
                    f.stack.push(x);
                    f.stack.push(y);
                }
                Op::GetRegProp(r, n) => {
                    let obj = f.regs[r as usize].clone();
                    let key = PropKey::Str(code.const_str(n));
                    let v = self.get(&obj, &key)?;
                    f.stack.push(v);
                }
                Op::IncReg(r) | Op::DecReg(r) => {
                    let d = if matches!(op, Op::IncReg(_)) { 1.0 } else { -1.0 };
                    let slot = &mut f.regs[r as usize];
                    if let Value::Number(n) = slot {
                        *n += d;
                    } else {
                        let a = slot.clone();
                        let v = match self.to_numeric(&a)? {
                            Value::BigInt(b) => Value::BigInt(Rc::new(b.add(&crate::bigint::BigInt::from_i64(d as i64)))),
                            v => Value::Number(self.to_number(&v)? + d),
                        };
                        f.regs[r as usize] = v;
                    }
                }
                Op::AddNum(y) => {
                    let a = pop!();
                    let r = match &a {
                        Value::Number(x) => Value::Number(x + y),
                        _ => self.add(&a, &Value::Number(y))?,
                    };
                    f.stack.push(r);
                }
                Op::SubNum(y) => {
                    let a = pop!();
                    let r = match &a {
                        Value::Number(x) => Value::Number(x - y),
                        _ => self.arith(crate::ops::Arith::Sub, &a, &Value::Number(y))?,
                    };
                    f.stack.push(r);
                }
                Op::JumpIfNotLt(t) | Op::JumpIfNotLe(t) | Op::JumpIfNotGt(t) | Op::JumpIfNotGe(t) => {
                    let b = pop!();
                    let a = pop!();
                    let ok = match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => match op {
                            Op::JumpIfNotLt(_) => x < y,
                            Op::JumpIfNotLe(_) => x <= y,
                            Op::JumpIfNotGt(_) => x > y,
                            _ => x >= y,
                        },
                        _ => match op {
                            Op::JumpIfNotLt(_) => self.less_than(&a, &b, true)? == Some(true),
                            Op::JumpIfNotGt(_) => self.less_than(&b, &a, false)? == Some(true),
                            Op::JumpIfNotLe(_) => matches!(self.less_than(&b, &a, false)?, Some(false)),
                            _ => matches!(self.less_than(&a, &b, true)?, Some(false)),
                        },
                    };
                    if !ok {
                        f.pc = t as usize;
                    }
                }
                Op::JumpIfNotStrictEq(t) | Op::JumpIfStrictEq(t) => {
                    let b = pop!();
                    let a = pop!();
                    if strict_equals(&a, &b) == matches!(op, Op::JumpIfStrictEq(_)) {
                        f.pc = t as usize;
                    }
                }
                Op::ClearReg(r) => f.regs[r as usize] = Value::Empty,
                Op::GetRegChk(r, n) => {
                    let v = f.regs[r as usize].clone();
                    if matches!(v, Value::Empty) {
                        return Err(self.tdz_error(&code, n));
                    }
                    f.stack.push(v);
                }
                Op::SetRegChk(r, n) => {
                    if matches!(f.regs[r as usize], Value::Empty) {
                        return Err(self.tdz_error(&code, n));
                    }
                    f.regs[r as usize] = pop!();
                }
                Op::GetEnv(h, s) => {
                    let v = env_get(&f.env, h, s);
                    f.stack.push(v);
                }
                Op::SetEnv(h, s) => {
                    let v = pop!();
                    env_set(&f.env, h, s, v);
                }
                Op::GetEnvChk(h, s, n) => {
                    let v = env_get(&f.env, h, s);
                    if matches!(v, Value::Empty) {
                        return Err(self.tdz_error(&code, n));
                    }
                    f.stack.push(v);
                }
                Op::SetEnvChk(h, s, n) => {
                    if matches!(env_get(&f.env, h, s), Value::Empty) {
                        return Err(self.tdz_error(&code, n));
                    }
                    let v = pop!();
                    env_set(&f.env, h, s, v);
                }
                Op::GetGlobal(n) => {
                    let cached = match code.global_cache.borrow().get(n as usize) {
                        Some(Some(cell)) => {
                            let b = cell.borrow();
                            if matches!(b.value, Value::Empty) { None } else { Some(b.value.clone()) }
                        }
                        _ => None,
                    };
                    let v = match cached {
                        Some(v) => v,
                        None => match self.global_prop_cached(&code, n) {
                            Some(v) => v,
                            None => {
                                let name = code.const_str(n);
                                self.cache_global(&code, n, &name);
                                let v = self.get_global(&name)?;
                                self.remember_global_prop(&code, n, &name);
                                v
                            }
                        },
                    };
                    f.stack.push(v);
                }
                Op::SetGlobal(n) => {
                    let v = pop!();
                    let done = match code.global_cache.borrow().get(n as usize) {
                        Some(Some(cell)) => {
                            let mut b = cell.borrow_mut();
                            if matches!(b.value, Value::Empty) || b.is_const {
                                false
                            } else {
                                b.value = v.clone();
                                true
                            }
                        }
                        _ => false,
                    };
                    if !done {
                        let name = code.const_str(n);
                        self.cache_global(&code, n, &name);
                        self.set_global(&name, v, code.strict)?;
                    }
                }
                Op::TypeofGlobal(n) => {
                    let name = code.const_str(n);
                    let lexv = self.lex.get(&name).map(|b| b.borrow().value.clone());
                    let v = match lexv {
                        Some(v) => {
                            if matches!(v, Value::Empty) {
                                return Err(self.tdz_error(&code, n));
                            }
                            v
                        }
                        None => {
                            let g = self.realm.global.clone();
                            match self.get_opt(&g, &PropKey::Str(name.clone()))? {
                                Some(v) => v,
                                None => self.global_miss.and_then(|f| f(self, &name)).unwrap_or(Value::Undefined),
                            }
                        }
                    };
                    f.stack.push(Value::str(typeof_str(&v)));
                }
                Op::DeleteGlobal(n) => {
                    let name = code.const_str(n);
                    let g = self.realm.global.clone();
                    let ok = self.delete_prop(&g, &PropKey::Str(name));
                    f.stack.push(Value::Bool(ok));
                }
                Op::DeclareVar(n) => {
                    let key = PropKey::Str(code.const_str(n));
                    let g = self.realm.global.clone();
                    if !g.borrow().props.contains(&key) {
                        g.borrow_mut().props.insert(
                            key,
                            Property {
                                slot: Slot::Data {
                                    value: Value::Undefined,
                                    writable: true,
                                },
                                enumerable: true,
                                configurable: false,
                            },
                        );
                    }
                }
                Op::DeclareFunc(n) => {
                    let v = pop!();
                    let key = PropKey::Str(code.const_str(n));
                    self.realm.global.borrow_mut().props.insert(
                        key,
                        Property {
                            slot: Slot::Data { value: v, writable: true },
                            enumerable: true,
                            configurable: false,
                        },
                    );
                }
                Op::DeclareLex(n, is_const) => {
                    let name = code.const_str(n);
                    // redeclarar (REPL) reaproveita a célula: caches continuam válidos
                    match self.lex.get(&name) {
                        Some(cell) => {
                            let mut b = cell.borrow_mut();
                            b.value = Value::Empty;
                            b.is_const = is_const;
                        }
                        None => {
                            self.lex_gen = self.lex_gen.wrapping_add(1);
                            self.lex.insert(
                                name,
                                Rc::new(std::cell::RefCell::new(LexBinding {
                                    value: Value::Empty,
                                    is_const,
                                })),
                            );
                        }
                    }
                }
                Op::InitLex(n) => {
                    let v = pop!();
                    let name = code.const_str(n);
                    if let Some(b) = self.lex.get(&name) {
                        b.borrow_mut().value = v;
                    }
                }
                Op::PushEnv(size) => {
                    let parent = f.env.take();
                    f.env = Some(new_env(size as usize, parent));
                }
                Op::PopEnv => {
                    let parent = f.env.as_ref().and_then(|e| e.borrow().parent.clone());
                    f.env = parent;
                }
                Op::CopyEnv => {
                    if let Some(e) = &f.env {
                        let copy = {
                            let b = e.borrow();
                            crate::value::env_with(b.slots.clone(), b.parent.clone())
                        };
                        f.env = Some(copy);
                    }
                }
                Op::ConstAssign(_) => return Err(self.type_error("Assignment to constant variable.")),
                Op::This => {
                    if matches!(f.this, Value::Empty) {
                        return Err(self.reference_error(
                            "Must call super constructor in derived class before accessing 'this' or returning from derived constructor",
                        ));
                    }
                    f.stack.push(f.this.clone());
                }
                Op::ThisRaw => f.stack.push(f.this.clone()),
                Op::GlobalThis => f.stack.push(Value::Object(self.realm.global.clone())),
                Op::NewTarget => f.stack.push(f.new_target.clone()),
                Op::FnSelf => f.stack.push(f.func.clone().map(Value::Object).unwrap_or_default()),
                Op::RestArgs(from) => {
                    let rest: Vec<Value> = f.args.iter().skip(from as usize).cloned().collect();
                    f.stack.push(Value::Object(self.new_array(rest)));
                }
                Op::Arguments => {
                    let a = self.make_arguments(&f.args, f.func.clone(), code.strict);
                    f.stack.push(Value::Object(a));
                }
                Op::NewObject => f.stack.push(Value::Object(self.new_object())),
                Op::NewArray => f.stack.push(Value::Object(self.new_array(Vec::new()))),
                Op::ArrayPush => {
                    let v = pop!();
                    if let Some(Value::Object(a)) = f.stack.last()
                        && let Kind::Array(vec) = &mut a.borrow_mut().kind
                    {
                        vec.push(v);
                    }
                }
                Op::ArrayHole => {
                    if let Some(Value::Object(a)) = f.stack.last()
                        && let Kind::Array(vec) = &mut a.borrow_mut().kind
                    {
                        vec.push(Value::Empty);
                    }
                }
                Op::ArraySpread => {
                    let src = pop!();
                    let items = self.iterate_to_vec(&src)?;
                    if let Some(Value::Object(a)) = f.stack.last()
                        && let Kind::Array(vec) = &mut a.borrow_mut().kind
                    {
                        vec.extend(items);
                    }
                }
                Op::DefineProp => {
                    let v = pop!();
                    let k = pop!();
                    let key = self.to_property_key(&k)?;
                    if let Some(Value::Object(o)) = f.stack.last() {
                        let o = o.clone();
                        self.define_data(&o, key, v);
                    }
                }
                Op::DefinePropNamed => {
                    let v = pop!();
                    let k = pop!();
                    let key = self.to_property_key(&k)?;
                    if let Value::Object(fo) = &v {
                        self.name_from_key(fo, &key, METHOD);
                    }
                    if let Some(Value::Object(o)) = f.stack.last() {
                        let o = o.clone();
                        self.define_data(&o, key, v);
                    }
                }
                Op::DefineName(n) => {
                    let v = pop!();
                    if let Some(Value::Object(o)) = f.stack.last() {
                        let o = o.clone();
                        self.define_data(&o, PropKey::Str(code.const_str(n)), v);
                    }
                }
                Op::DefineMethod(kind, enumerable) => {
                    let fv = pop!();
                    let k = pop!();
                    let key = self.to_property_key(&k)?;
                    let obj = match f.stack.last() {
                        Some(Value::Object(o)) => o.clone(),
                        _ => continue,
                    };
                    if let Value::Object(fo) = &fv {
                        set_home(fo, &obj);
                        self.name_from_key(fo, &key, kind);
                    }
                    self.define_method(&obj, key, fv, kind, enumerable);
                }
                Op::CopyProps => {
                    let src = pop!();
                    if let Some(Value::Object(o)) = f.stack.last() {
                        let o = o.clone();
                        self.copy_data_props(&o, &src, &[])?;
                    }
                }
                Op::CopyRest(n) => {
                    let mut keys = Vec::new();
                    for _ in 0..n {
                        let k = pop!();
                        keys.push(self.to_property_key(&k)?);
                    }
                    let src = pop!();
                    let o = self.new_object();
                    self.copy_data_props(&o, &src, &keys)?;
                    f.stack.push(Value::Object(o));
                }
                Op::SetProto => {
                    let p = pop!();
                    if let Some(Value::Object(o)) = f.stack.last() {
                        match p {
                            Value::Object(p) => o.borrow_mut().proto = Some(p),
                            Value::Null => o.borrow_mut().proto = None,
                            _ => {}
                        }
                    }
                }
                Op::GetProp(n) => {
                    let obj = pop!();
                    let key = PropKey::Str(code.const_str(n));
                    let v = self.get(&obj, &key)?;
                    f.stack.push(v);
                }
                Op::SetProp(n) => {
                    let v = pop!();
                    let obj = pop!();
                    let key = PropKey::Str(code.const_str(n));
                    self.put(&obj, key, v.clone(), code.strict)?;
                    f.stack.push(v);
                }
                Op::GetIndex => {
                    let k = pop!();
                    let obj = pop!();
                    let v = self.get_index(&obj, &k)?;
                    f.stack.push(v);
                }
                Op::SetIndex => {
                    let v = pop!();
                    let k = pop!();
                    let obj = pop!();
                    self.set_index(&obj, &k, v.clone(), code.strict)?;
                    f.stack.push(v);
                }
                Op::DeleteProp(n) => {
                    let obj = pop!();
                    let key = PropKey::Str(code.const_str(n));
                    let r = self.delete_value(&obj, &key, code.strict)?;
                    f.stack.push(Value::Bool(r));
                }
                Op::DeleteIndex => {
                    let k = pop!();
                    let obj = pop!();
                    let key = self.to_property_key(&k)?;
                    let r = self.delete_value(&obj, &key, code.strict)?;
                    f.stack.push(Value::Bool(r));
                }
                Op::GetSuper(n) => {
                    let this = pop!();
                    let key = PropKey::Str(code.const_str(n));
                    let v = self.super_get(&f.home, &this, &key)?;
                    f.stack.push(v);
                }
                Op::SetSuper(n) => {
                    let v = pop!();
                    let this = pop!();
                    let key = PropKey::Str(code.const_str(n));
                    self.super_set(&f.home, &this, key, v.clone())?;
                    f.stack.push(v);
                }
                Op::GetSuperIndex => {
                    let k = pop!();
                    let this = pop!();
                    let key = self.to_property_key(&k)?;
                    let v = self.super_get(&f.home, &this, &key)?;
                    f.stack.push(v);
                }
                Op::SetSuperIndex => {
                    let v = pop!();
                    let k = pop!();
                    let this = pop!();
                    let key = self.to_property_key(&k)?;
                    self.super_set(&f.home, &this, key, v.clone())?;
                    f.stack.push(v);
                }
                Op::NewPrivate(n) => {
                    f.stack.push(Value::Symbol(Sym::new_private(code.const_str(n))));
                }
                Op::GetPrivate => {
                    let s = pop!();
                    let obj = pop!();
                    let v = self.private_get(&obj, &s)?;
                    f.stack.push(v);
                }
                Op::SetPrivate => {
                    let v = pop!();
                    let s = pop!();
                    let obj = pop!();
                    self.private_set(&obj, &s, v.clone())?;
                    f.stack.push(v);
                }
                Op::HasPrivate => {
                    let obj = pop!();
                    let s = pop!();
                    let (Value::Object(o), Value::Symbol(s)) = (&obj, &s) else {
                        return Err(self.type_error("Cannot use 'in' operator to search for a private field in a non-object"));
                    };
                    let has = o.borrow().props.contains(&PropKey::Sym(s.clone()));
                    f.stack.push(Value::Bool(has));
                }
                Op::DefinePrivate => {
                    let v = pop!();
                    let s = pop!();
                    if let (Some(Value::Object(o)), Value::Symbol(s)) = (f.stack.last(), &s) {
                        let key = PropKey::Sym(s.clone());
                        if o.borrow().props.contains(&key) {
                            let d = s.description().map(|d| d.to_rust()).unwrap_or_default();
                            return Err(self.type_error(format!("Cannot initialize {d} twice on the same object")));
                        }
                        o.borrow_mut().props.insert(key, Property::hidden(v));
                    }
                }
                Op::DefinePrivateMethod(kind) => {
                    let fv = pop!();
                    let s = pop!();
                    if let (Some(Value::Object(o)), Value::Symbol(s)) = (f.stack.last(), &s) {
                        let o = o.clone();
                        if let (Value::Object(fo), Some(h)) = (&fv, &f.home) {
                            set_home(fo, h);
                        }
                        self.define_method(&o, PropKey::Sym(s.clone()), fv, kind, false);
                    }
                }
                Op::Closure(i) => {
                    let c = code.functions[i as usize].clone();
                    let home = if c.is_arrow() { f.home.clone() } else { None };
                    let fo = self.make_closure(c, f.env.clone(), home);
                    f.stack.push(Value::Object(fo));
                }
                Op::MakeClass(i, derived) => {
                    let sup = if derived { Some(pop!()) } else { None };
                    let c = code.functions[i as usize].clone();
                    let (ctor, proto) = self.make_class(c, f.env.clone(), sup)?;
                    f.stack.push(Value::Object(ctor));
                    f.stack.push(Value::Object(proto));
                }
                Op::SetFields => {
                    let fv = pop!();
                    let n = f.stack.len();
                    if let (Value::Object(ctor), Value::Object(proto), Value::Object(fo)) = (&f.stack[n - 2], &f.stack[n - 1], &fv) {
                        set_home(fo, proto);
                        let mut b = ctor.borrow_mut();
                        if let Kind::Function(Func::Closure(c)) = &mut b.kind {
                            let mut nc = (**c).clone();
                            nc.fields = Some(fo.clone());
                            *c = Rc::new(nc);
                        }
                    }
                }
                Op::CallInit => {
                    let fv = pop!();
                    let obj = pop!();
                    if let (Value::Object(fo), Value::Object(o)) = (&fv, &obj) {
                        set_home(fo, o);
                    }
                    self.call(&fv, obj, Vec::new())?;
                }
                Op::Call(argc) => {
                    let n = f.stack.len() - argc as usize;
                    let args = f.stack.split_off(n);
                    let this = pop!();
                    let callee = pop!();
                    if !callee.is_callable() {
                        return Err(self.not_callable(&code, &callee));
                    }
                    let r = self.call(&callee, this, args)?;
                    f.stack.push(r);
                }
                Op::CallSpread => {
                    let arr = pop!();
                    let this = pop!();
                    let callee = pop!();
                    let args = array_values(&arr);
                    if !callee.is_callable() {
                        return Err(self.not_callable(&code, &callee));
                    }
                    let r = self.call(&callee, this, args)?;
                    f.stack.push(r);
                }
                Op::New(argc) => {
                    let n = f.stack.len() - argc as usize;
                    let args = f.stack.split_off(n);
                    let callee = pop!();
                    let r = self.new_value(&code, &callee, args)?;
                    f.stack.push(r);
                }
                Op::NewSpread => {
                    let arr = pop!();
                    let callee = pop!();
                    let args = array_values(&arr);
                    let r = self.new_value(&code, &callee, args)?;
                    f.stack.push(r);
                }
                Op::SuperCall(argc) => {
                    let n = f.stack.len() - argc as usize;
                    let args = f.stack.split_off(n);
                    let r = self.super_call(f, args)?;
                    f.stack.push(r);
                }
                Op::SuperCallSpread => {
                    let arr = pop!();
                    let args = array_values(&arr);
                    let r = self.super_call(f, args)?;
                    f.stack.push(r);
                }
                Op::Return => {
                    let v = pop!();
                    if code.kind == crate::ast::FnKind::DerivedConstructor && !matches!(v, Value::Object(_)) {
                        if !v.is_undefined() {
                            return Err(self.type_error("Derived constructors may only return object or undefined"));
                        }
                        if matches!(f.this, Value::Empty) {
                            return Err(self.reference_error(
                                "Must call super constructor in derived class before accessing 'this' or returning from derived constructor",
                            ));
                        }
                        return Ok(f.this.clone());
                    }
                    return Ok(v);
                }
                Op::Throw => return Err(pop!()),
                Op::InitialYield => {
                    f.suspended = true;
                    f.suspend_await = false;
                    return Ok(Value::Undefined);
                }
                Op::Yield => {
                    let v = pop!();
                    f.suspended = true;
                    f.suspend_await = false;
                    return Ok(v);
                }
                Op::Await => {
                    let v = pop!();
                    f.suspended = true;
                    f.suspend_await = true;
                    return Ok(v);
                }
                Op::GetAsyncIter => {
                    let v = pop!();
                    let rec = self.get_async_iterator(&v)?;
                    f.stack.push(Value::Object(rec));
                }
                Op::AsyncIterNext => {
                    let rec = match f.stack.last() {
                        Some(Value::Object(r)) => r.clone(),
                        _ => continue,
                    };
                    let r = self.async_iter_next(&rec)?;
                    f.stack.push(r);
                }
                Op::AsyncIterClose(t) => {
                    let Value::Object(rec) = pop!() else { continue };
                    let iter = match &rec.borrow().kind {
                        Kind::IterRecord { iter, done: false, .. } => Some(iter.clone()),
                        _ => None,
                    };
                    let Some(iter) = iter else {
                        f.pc = t as usize;
                        continue;
                    };
                    if let Kind::IterRecord { done, .. } = &mut rec.borrow_mut().kind {
                        *done = true;
                    }
                    let ret = self.get_str(&iter, "return")?;
                    if ret.is_nullish() {
                        f.pc = t as usize;
                        continue;
                    }
                    let r = self.call(&ret, iter, Vec::new())?;
                    f.stack.push(r);
                }
                Op::IterResultValue(t) => {
                    let res = pop!();
                    if !matches!(res, Value::Object(_)) {
                        let d = self.describe(&res);
                        return Err(self.type_error(format!("Iterator result {d} is not an object")));
                    }
                    if truthy(&self.get_str(&res, "done")?) {
                        if let Some(Value::Object(rec)) = f.stack.last()
                            && let Kind::IterRecord { done, .. } = &mut rec.borrow_mut().kind
                        {
                            *done = true;
                        }
                        f.pc = t as usize;
                    } else {
                        let v = self.get_str(&res, "value")?;
                        f.stack.push(v);
                    }
                }
                Op::GenResume(t) => {
                    let mode = pop!();
                    match mode {
                        Value::Number(m) if m as u8 == crate::builtins::promise::RESUME_THROW => return Err(pop!()),
                        Value::Number(m) if m as u8 == crate::builtins::promise::RESUME_RETURN => {}
                        _ => f.pc = t as usize,
                    }
                }
                Op::AwaitResume => {
                    let mode = pop!();
                    if let Value::Number(m) = mode
                        && m as u8 == crate::builtins::promise::RESUME_THROW
                    {
                        return Err(pop!());
                    }
                }
                Op::DelegateStep(t) => {
                    let mode = match pop!() {
                        Value::Number(m) => m as u8,
                        _ => 0,
                    };
                    let received = pop!();
                    let rec = match f.stack.last() {
                        Some(Value::Object(r)) => r.clone(),
                        _ => continue,
                    };
                    let (iter, next) = match &rec.borrow().kind {
                        Kind::IterRecord { iter, next, .. } => (iter.clone(), next.clone()),
                        _ => (Value::Undefined, Value::Undefined),
                    };
                    let res = match mode {
                        crate::builtins::promise::RESUME_THROW => {
                            let th = self.get_str(&iter, "throw")?;
                            if th.is_nullish() {
                                let _ = self.iter_close(&rec);
                                return Err(self.type_error("The iterator does not provide a 'throw' method"));
                            }
                            self.call(&th, iter.clone(), vec![received])?
                        }
                        crate::builtins::promise::RESUME_RETURN => {
                            let rt = self.get_str(&iter, "return")?;
                            if rt.is_nullish() {
                                f.stack.pop();
                                f.stack.push(received);
                                f.stack.push(Value::Bool(true));
                                f.pc = t as usize;
                                continue;
                            }
                            self.call(&rt, iter.clone(), vec![received])?
                        }
                        _ => self.call(&next, iter.clone(), vec![received])?,
                    };
                    if !matches!(res, Value::Object(_)) {
                        let d = self.describe(&res);
                        return Err(self.type_error(format!("Iterator result {d} is not an object")));
                    }
                    let done = truthy(&self.get_str(&res, "done")?);
                    let value = self.get_str(&res, "value")?;
                    if done {
                        f.stack.pop();
                        f.stack.push(value);
                        f.stack.push(Value::Bool(mode == crate::builtins::promise::RESUME_RETURN));
                        f.pc = t as usize;
                    } else {
                        f.stack.push(value);
                    }
                }
                Op::ThrowError(kind, msg) => {
                    let m = code.const_str(msg).to_rust();
                    return Err(match kind {
                        ERR_SYNTAX => self.syntax_error(m),
                        ERR_REFERENCE => self.reference_error(m),
                        _ => self.type_error(m),
                    });
                }
                Op::TemplateObj(i) => {
                    let v = self.template_object(&code, i as usize);
                    f.stack.push(v);
                }
                Op::Regex(p, fl) => {
                    let v = self.new_regexp(code.const_str(p), code.const_str(fl))?;
                    f.stack.push(v);
                }
                Op::Jump(t) => f.pc = t as usize,
                Op::JumpIfFalse(t) => {
                    if !truthy(&pop!()) {
                        f.pc = t as usize;
                    }
                }
                Op::JumpIfTrue(t) => {
                    if truthy(&pop!()) {
                        f.pc = t as usize;
                    }
                }
                Op::JumpIfFalseKeep(t) => {
                    if !truthy(f.stack.last().unwrap_or(&Value::Undefined)) {
                        f.pc = t as usize;
                    } else {
                        f.stack.pop();
                    }
                }
                Op::JumpIfTrueKeep(t) => {
                    if truthy(f.stack.last().unwrap_or(&Value::Undefined)) {
                        f.pc = t as usize;
                    } else {
                        f.stack.pop();
                    }
                }
                Op::JumpIfNotNullishKeep(t) => {
                    if !f.stack.last().unwrap_or(&Value::Undefined).is_nullish() {
                        f.pc = t as usize;
                    } else {
                        f.stack.pop();
                    }
                }
                Op::JumpIfNullish(t) => {
                    if f.stack.last().unwrap_or(&Value::Undefined).is_nullish() {
                        f.pc = t as usize;
                    }
                }
                Op::JumpIfNotUndef(t) => {
                    if !matches!(f.stack.last(), Some(Value::Undefined) | None) {
                        f.pc = t as usize;
                    }
                }
                Op::Try(t) => f.handlers.push(Handler {
                    pc: t as usize,
                    stack: f.stack.len(),
                    env: f.env.clone(),
                }),
                Op::PopTry => {
                    f.handlers.pop();
                }
                Op::GetIter => {
                    let v = pop!();
                    let rec = self.get_iterator(&v)?;
                    f.stack.push(Value::Object(rec));
                }
                Op::IterStep(t) => {
                    let rec = match f.stack.last() {
                        Some(Value::Object(r)) => r.clone(),
                        _ => continue,
                    };
                    match self.iter_step(&rec)? {
                        Some(v) => f.stack.push(v),
                        None => f.pc = t as usize,
                    }
                }
                Op::IterValue => {
                    let rec = match f.stack.last() {
                        Some(Value::Object(r)) => r.clone(),
                        _ => continue,
                    };
                    let v = self.iter_step(&rec)?.unwrap_or(Value::Undefined);
                    f.stack.push(v);
                }
                Op::IterRest => {
                    let rec = match f.stack.last() {
                        Some(Value::Object(r)) => r.clone(),
                        _ => continue,
                    };
                    let mut items = Vec::new();
                    while let Some(v) = self.iter_step(&rec)? {
                        items.push(v);
                    }
                    f.stack.push(Value::Object(self.new_array(items)));
                }
                Op::IterClose => {
                    if let Value::Object(rec) = pop!() {
                        self.iter_close(&rec)?;
                    }
                }
                Op::IterCloseThrow => {
                    let exc = pop!();
                    if let Value::Object(rec) = pop!() {
                        let _ = self.iter_close(&rec);
                    }
                    return Err(exc);
                }
                Op::IterPop => {
                    f.stack.pop();
                }
                Op::ForIn => {
                    let v = pop!();
                    let it = self.for_in_iterator(&v)?;
                    f.stack.push(Value::Object(it));
                }
                Op::ForInNext(t) => {
                    let it = match f.stack.last() {
                        Some(Value::Object(r)) => r.clone(),
                        _ => continue,
                    };
                    match self.for_in_next(&it) {
                        Some(k) => f.stack.push(Value::String(k)),
                        None => f.pc = t as usize,
                    }
                }
                Op::Add => {
                    let b = pop!();
                    let a = pop!();
                    let r = match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => Value::Number(x + y),
                        (Value::String(x), Value::String(y)) => Value::String(x.concat(y)),
                        _ => self.add(&a, &b)?,
                    };
                    f.stack.push(r);
                }
                Op::Sub => num2!(crate::ops::Arith::Sub),
                Op::Mul => num2!(crate::ops::Arith::Mul),
                Op::Div => num2!(crate::ops::Arith::Div),
                Op::Mod => num2!(crate::ops::Arith::Mod),
                Op::Exp => num2!(crate::ops::Arith::Exp),
                Op::BitAnd => num2!(crate::ops::Arith::And),
                Op::BitOr => num2!(crate::ops::Arith::Or),
                Op::BitXor => num2!(crate::ops::Arith::Xor),
                Op::Shl => num2!(crate::ops::Arith::Shl),
                Op::Shr => num2!(crate::ops::Arith::Shr),
                Op::UShr => num2!(crate::ops::Arith::UShr),
                Op::Eq => {
                    let b = pop!();
                    let a = pop!();
                    let r = self.loose_equals(&a, &b)?;
                    f.stack.push(Value::Bool(r));
                }
                Op::Ne => {
                    let b = pop!();
                    let a = pop!();
                    let r = self.loose_equals(&a, &b)?;
                    f.stack.push(Value::Bool(!r));
                }
                Op::StrictEq => {
                    let b = pop!();
                    let a = pop!();
                    f.stack.push(Value::Bool(strict_equals(&a, &b)));
                }
                Op::StrictNe => {
                    let b = pop!();
                    let a = pop!();
                    f.stack.push(Value::Bool(!strict_equals(&a, &b)));
                }
                Op::Lt => cmp!(false, false),
                Op::Gt => cmp!(true, false),
                Op::Le => {
                    let b = pop!();
                    let a = pop!();
                    let r = match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => *x <= *y,
                        _ => matches!(self.less_than(&b, &a, false)?, Some(false)),
                    };
                    f.stack.push(Value::Bool(r));
                }
                Op::Ge => {
                    let b = pop!();
                    let a = pop!();
                    let r = match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => *x >= *y,
                        _ => matches!(self.less_than(&a, &b, true)?, Some(false)),
                    };
                    f.stack.push(Value::Bool(r));
                }
                Op::InstanceOf => {
                    let b = pop!();
                    let a = pop!();
                    let r = self.instance_of(&a, &b)?;
                    f.stack.push(Value::Bool(r));
                }
                Op::In => {
                    let b = pop!();
                    let a = pop!();
                    let Value::Object(o) = &b else {
                        let k = self.to_string(&a).map(|s| s.to_rust()).unwrap_or_default();
                        let d = match &b {
                            Value::String(s) => s.to_rust(),
                            _ => self.describe(&b),
                        };
                        return Err(self.type_error(format!("Cannot use 'in' operator to search for '{k}' in {d}")));
                    };
                    let key = self.to_property_key(&a)?;
                    let r = self.has_property(o, &key)?;
                    f.stack.push(Value::Bool(r));
                }
                Op::Neg => {
                    let a = pop!();
                    let r = match self.to_numeric(&a)? {
                        Value::BigInt(b) => Value::BigInt(Rc::new(b.neg())),
                        Value::Number(n) => Value::Number(-n),
                        v => v,
                    };
                    f.stack.push(r);
                }
                Op::Plus => {
                    let a = pop!();
                    let n = match a {
                        Value::Number(n) => n,
                        _ => self.to_number(&a)?,
                    };
                    f.stack.push(Value::Number(n));
                }
                Op::ToNumeric => {
                    let a = pop!();
                    let r = match a {
                        Value::Number(_) => a,
                        _ => self.to_numeric(&a)?,
                    };
                    f.stack.push(r);
                }
                Op::Not => {
                    let a = pop!();
                    f.stack.push(Value::Bool(!truthy(&a)));
                }
                Op::BitNot => {
                    let a = pop!();
                    let r = match self.to_numeric(&a)? {
                        Value::BigInt(b) => Value::BigInt(Rc::new(b.not())),
                        v => {
                            let n = crate::conv::to_int32(self.to_number(&v)?);
                            Value::Number(!n as f64)
                        }
                    };
                    f.stack.push(r);
                }
                Op::Typeof => {
                    let a = pop!();
                    f.stack.push(Value::str(typeof_str(&a)));
                }
                Op::Inc | Op::Dec => {
                    let a = pop!();
                    let d = if matches!(op, Op::Inc) { 1.0 } else { -1.0 };
                    let r = match a {
                        Value::Number(n) => Value::Number(n + d),
                        _ => match self.to_numeric(&a)? {
                            Value::BigInt(b) => Value::BigInt(Rc::new(b.add(&crate::bigint::BigInt::from_i64(d as i64)))),
                            v => Value::Number(self.to_number(&v)? + d),
                        },
                    };
                    f.stack.push(r);
                }
                Op::ToPropertyKey => {
                    let a = pop!();
                    let k = self.to_property_key(&a)?;
                    f.stack.push(k.to_value());
                }
                Op::ToStr => {
                    let a = pop!();
                    let s = match a {
                        Value::String(s) => s,
                        _ => self.to_string(&a)?,
                    };
                    f.stack.push(Value::String(s));
                }
                Op::RequireCoercible => {
                    if let Some(v) = f.stack.last()
                        && v.is_nullish()
                    {
                        let d = if matches!(v, Value::Null) { "null" } else { "undefined" };
                        return Err(self.type_error(format!("Cannot destructure '{d}' as it is {d}.")));
                    }
                }
                Op::Nop => {}
            }
        }
    }

    /// Guarda no código a célula da variável léxica global `name`, se existir.
    /// Valor de uma propriedade de dados do objeto global já vista nessa
    /// instrução (sem refazer a busca pelo nome).
    #[inline]
    fn global_prop_cached(&self, code: &Code, n: u32) -> Option<Value> {
        let (lg, pg, idx) = *code.gobj_cache.borrow().get(n as usize)?;
        if idx == u32::MAX || lg != self.lex_gen {
            return None;
        }
        let g = self.realm.global.borrow();
        if g.props.generation() != pg {
            return None;
        }
        let (k, p) = g.props.entry_at(idx as usize)?;
        let PropKey::Str(ks) = k else { return None };
        if *ks != code.const_str(n) {
            return None;
        }
        match &p.slot {
            Slot::Data { value, .. } => Some(value.clone()),
            _ => None,
        }
    }

    fn remember_global_prop(&self, code: &Code, n: u32, name: &JsStr) {
        if self.lex.contains_key(name) {
            return;
        }
        let g = self.realm.global.borrow();
        let key = PropKey::Str(name.clone());
        let Some(i) = g.props.position(&key) else { return };
        if !matches!(g.props.entry_at(i), Some((_, Property { slot: Slot::Data { .. }, .. }))) {
            return;
        }
        let mut c = code.gobj_cache.borrow_mut();
        if c.len() <= n as usize {
            c.resize(n as usize + 1, (0, 0, u32::MAX));
        }
        c[n as usize] = (self.lex_gen, g.props.generation(), i as u32);
    }

    fn cache_global(&mut self, code: &Code, n: u32, name: &JsStr) {
        if let Some(cell) = self.lex.get(name) {
            let mut cache = code.global_cache.borrow_mut();
            if cache.len() <= n as usize {
                cache.resize(n as usize + 1, None);
            }
            cache[n as usize] = Some(cell.clone());
        }
    }

    fn tdz_error(&mut self, code: &Code, n: u32) -> Value {
        let name = code.const_str(n).to_rust();
        if name == "this" {
            return self.reference_error(
                "Must call super constructor in derived class before accessing 'this' or returning from derived constructor",
            );
        }
        self.reference_error(format!("Cannot access '{name}' before initialization"))
    }

    fn not_callable(&mut self, code: &Code, callee: &Value) -> Value {
        let pc = self.cur_pc as u32;
        let hint = code
            .call_names
            .binary_search_by(|(p, _)| p.cmp(&pc))
            .ok()
            .map(|i| code.const_str(code.call_names[i].1).to_rust());
        let d = match hint {
            Some(h) => h,
            None => self.describe(callee),
        };
        self.type_error(format!("{d} is not a function"))
    }

    fn new_value(&mut self, code: &Code, callee: &Value, args: Vec<Value>) -> Result<Value, Value> {
        match callee {
            Value::Object(o) if o.is_constructor() => self.construct(o, args, o),
            _ => {
                let pc = self.cur_pc as u32;
                let hint = code
                    .call_names
                    .binary_search_by(|(p, _)| p.cmp(&pc))
                    .ok()
                    .map(|i| code.const_str(code.call_names[i].1).to_rust());
                let d = match hint {
                    Some(h) => h,
                    None => self.describe(callee),
                };
                Err(self.type_error(format!("{d} is not a constructor")))
            }
        }
    }

    fn super_call(&mut self, f: &mut Frame, args: Vec<Value>) -> Result<Value, Value> {
        let func = f.func.clone();
        let parent = func.as_ref().and_then(|fo| fo.proto());
        let Some(parent) = parent.filter(|p| p.is_constructor()) else {
            return Err(self.type_error("Super constructor is not a constructor"));
        };
        if !matches!(f.this, Value::Empty) {
            return Err(self.reference_error("Super constructor may only be called once"));
        }
        let nt = match &f.new_target {
            Value::Object(o) => o.clone(),
            _ => return Err(self.syntax_error("'super' keyword unexpected here")),
        };
        let r = self.construct(&parent, args, &nt)?;
        f.this = r.clone();
        self.init_fields(&func, &r)?;
        Ok(r)
    }

    fn super_get(&mut self, home: &Option<ObjRef>, this: &Value, key: &PropKey) -> Result<Value, Value> {
        let Some(home) = home else {
            return Err(self.syntax_error("'super' keyword unexpected here"));
        };
        match home.proto() {
            Some(p) => self.get_with(&p, key, this),
            None => Ok(Value::Undefined),
        }
    }

    fn super_set(&mut self, home: &Option<ObjRef>, this: &Value, key: PropKey, v: Value) -> Result<(), Value> {
        let Some(home) = home else {
            return Err(self.syntax_error("'super' keyword unexpected here"));
        };
        match home.proto() {
            Some(p) => {
                self.set_with(&p, key, v, this)?;
            }
            None => {
                self.put(this, key, v, true)?;
            }
        }
        Ok(())
    }

    fn private_get(&mut self, obj: &Value, s: &Value) -> Result<Value, Value> {
        let (Value::Object(o), Value::Symbol(s)) = (obj, s) else {
            return Err(self.type_error("Cannot read private member from a non-object"));
        };
        let prop = o.borrow().props.get(&PropKey::Sym(s.clone())).cloned();
        match prop {
            Some(Property {
                slot: Slot::Data { value, .. },
                ..
            }) => Ok(value),
            Some(Property {
                slot: Slot::Accessor { get: Some(g), .. },
                ..
            }) => self.call_obj(&g, obj.clone(), Vec::new()),
            Some(_) => {
                let d = s.description().map(|d| d.to_rust()).unwrap_or_default();
                Err(self.type_error(format!("'{d}' was defined without a getter")))
            }
            None => {
                let d = s.description().map(|d| d.to_rust()).unwrap_or_default();
                Err(self.type_error(format!("Cannot read private member {d} from an object whose class did not declare it")))
            }
        }
    }

    fn private_set(&mut self, obj: &Value, s: &Value, v: Value) -> Result<(), Value> {
        let (Value::Object(o), Value::Symbol(s)) = (obj, s) else {
            return Err(self.type_error("Cannot write private member to a non-object"));
        };
        let key = PropKey::Sym(s.clone());
        let setter = {
            let mut b = o.borrow_mut();
            match b.props.get_mut(&key) {
                Some(Property {
                    slot: Slot::Data { value, writable },
                    ..
                }) => {
                    if !*writable {
                        drop(b);
                        let d = s.description().map(|d| d.to_rust()).unwrap_or_default();
                        return Err(self.type_error(format!("Private method {d} is not writable")));
                    }
                    *value = v.clone();
                    return Ok(());
                }
                Some(Property {
                    slot: Slot::Accessor { set, .. },
                    ..
                }) => set.clone(),
                None => {
                    drop(b);
                    let d = s.description().map(|d| d.to_rust()).unwrap_or_default();
                    return Err(self.type_error(format!("Cannot write private member {d} to an object whose class did not declare it")));
                }
            }
        };
        match setter {
            Some(st) => {
                self.call_obj(&st, obj.clone(), vec![v])?;
                Ok(())
            }
            None => {
                let d = s.description().map(|d| d.to_rust()).unwrap_or_default();
                Err(self.type_error(format!("'{d}' was defined without a setter")))
            }
        }
    }

    /// Define método/getter/setter (juntando getter e setter do mesmo nome).
    pub fn define_method(&mut self, obj: &ObjRef, key: PropKey, fv: Value, kind: u8, enumerable: bool) {
        let fo = match &fv {
            Value::Object(o) => Some(o.clone()),
            _ => None,
        };
        let mut b = obj.borrow_mut();
        match kind {
            METHOD => {
                b.props.insert(
                    key,
                    Property {
                        slot: Slot::Data {
                            value: fv,
                            writable: true,
                        },
                        enumerable,
                        configurable: true,
                    },
                );
            }
            GETTER | SETTER => {
                let (mut get, mut set) = match b.props.get(&key) {
                    Some(Property {
                        slot: Slot::Accessor { get, set },
                        ..
                    }) => (get.clone(), set.clone()),
                    _ => (None, None),
                };
                if kind == GETTER {
                    get = fo;
                } else {
                    set = fo;
                }
                b.props.insert(
                    key,
                    Property {
                        slot: Slot::Accessor { get, set },
                        enumerable,
                        configurable: true,
                    },
                );
            }
            _ => {}
        }
    }

    /// Nome de função definido por chave computada (`[Symbol.iterator]() {}`).
    fn name_from_key(&mut self, fo: &ObjRef, key: &PropKey, kind: u8) {
        let current = match fo.borrow().props.get(&PropKey::str("name")) {
            Some(Property {
                slot: Slot::Data { value: Value::String(s), .. },
                ..
            }) => !s.is_empty(),
            _ => false,
        };
        if current {
            return;
        }
        let base = match key {
            PropKey::Str(s) => s.to_rust(),
            PropKey::Sym(s) => match s.description() {
                Some(d) => format!("[{d}]"),
                None => String::new(),
            },
        };
        let name = match kind {
            GETTER => format!("get {base}"),
            SETTER => format!("set {base}"),
            _ => base,
        };
        fo.borrow_mut().props.insert(
            PropKey::str("name"),
            Property {
                slot: Slot::Data {
                    value: Value::str(&name),
                    writable: false,
                },
                enumerable: false,
                configurable: true,
            },
        );
    }

    fn template_object(&mut self, code: &Code, i: usize) -> Value {
        if let Some(Some(o)) = code.template_cache.borrow().get(i) {
            return Value::Object(o.clone());
        }
        let (cooked, raw) = &code.templates[i];
        let cooked_vals: Vec<Value> = cooked
            .iter()
            .map(|c| match c {
                Some(u) => Value::String(JsStr::from_slice(u)),
                None => Value::Undefined,
            })
            .collect();
        let raw_vals: Vec<Value> = raw.iter().map(|r| Value::str(r)).collect();
        let raw_arr = self.new_array(raw_vals);
        raw_arr.borrow_mut().frozen = true;
        raw_arr.borrow_mut().extensible = false;
        let arr = self.new_array(cooked_vals);
        arr.borrow_mut().props.insert(
            PropKey::str("raw"),
            Property {
                slot: Slot::Data {
                    value: Value::Object(raw_arr),
                    writable: false,
                },
                enumerable: false,
                configurable: false,
            },
        );
        arr.borrow_mut().frozen = true;
        arr.borrow_mut().extensible = false;
        if let Some(slot) = code.template_cache.borrow_mut().get_mut(i) {
            *slot = Some(arr.clone());
        }
        Value::Object(arr)
    }
}

