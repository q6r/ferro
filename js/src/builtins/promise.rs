//! Promise, fila de microtarefas e a execução de funções `async`.
//!
//! Segue o spec: cada `then` vira uma reação; quando a promessa é resolvida,
//! as reações viram tarefas na fila de microtarefas, que roda até esvaziar
//! ao fim de cada script (e de cada timer).

use std::cell::Cell;
use std::rc::Rc;

use super::{Realm, native_data, set_native_data};
use crate::value::*;
use crate::vm::{Frame, Out, Vm};

pub enum PState {
    Pending,
    Fulfilled(Value),
    Rejected(Value),
}

pub struct PromiseData {
    pub state: PState,
    pub fulfill: Vec<Reaction>,
    pub reject: Vec<Reaction>,
    /// Alguém já tratou a rejeição (para avisar "Uncaught (in promise)").
    pub handled: bool,
}

#[derive(Clone)]
pub struct Capability {
    pub promise: Value,
    pub resolve: Value,
    pub reject: Value,
}

#[derive(Clone)]
pub struct Reaction {
    pub cap: Option<Capability>,
    pub handler: Value,
    pub is_reject: bool,
}

pub enum Job {
    Reaction(Reaction, Value),
    Thenable(ObjRef, Value, Value),
    Call(Value, Vec<Value>),
}

/// Estado de uma função async pausada num `await`.
pub struct AsyncData {
    pub frame: Option<Box<Frame>>,
    pub promise: ObjRef,
}

pub const RESUME_NEXT: u8 = 0;
pub const RESUME_THROW: u8 = 1;
pub const RESUME_RETURN: u8 = 2;

pub fn install(r: &Realm) {
    let p = &r.promise_proto;
    let c = r.ctor("Promise", 1, promise_ctor, p);
    r.method(&c, "resolve", 1, promise_resolve);
    r.method(&c, "reject", 1, promise_reject);
    r.method(&c, "all", 1, promise_all);
    r.method(&c, "allSettled", 1, promise_all_settled);
    r.method(&c, "race", 1, promise_race);
    r.method(&c, "any", 1, promise_any);
    r.method(&c, "withResolvers", 0, with_resolvers);
    r.getter_sym(&c, &r.sym_species, "[Symbol.species]", super::array::return_this);
    r.method(p, "then", 2, then);
    r.method(p, "catch", 1, catch);
    r.method(p, "finally", 1, finally);
    r.tag(p, "Promise");
    r.method(&r.global, "queueMicrotask", 1, queue_microtask);
}

// ----------------------------------------------------------- internos

impl Vm {
    pub fn new_promise(&self) -> ObjRef {
        self.new_promise_with_proto(self.realm.promise_proto.clone())
    }

    fn new_promise_with_proto(&self, proto: ObjRef) -> ObjRef {
        ObjRef::new(Object::new(
            Some(proto),
            Kind::Promise(Box::new(PromiseData {
                state: PState::Pending,
                fulfill: Vec::new(),
                reject: Vec::new(),
                handled: false,
            })),
        ))
    }

    /// As funções resolve/reject de uma promessa (só a primeira chamada vale).
    pub fn resolving_functions(&self, p: &ObjRef) -> (ObjRef, ObjRef) {
        let done = Rc::new(Cell::new(false));
        let mk = |f: NativeFn, name: &str| {
            let fo = self.realm.native(name, 1, f);
            let rec = ObjRef::new(Object::new(
                None,
                Kind::PromiseResolver {
                    promise: p.clone(),
                    done: done.clone(),
                },
            ));
            set_native_data(&fo, Value::Object(rec));
            fo
        };
        (mk(resolve_fn, ""), mk(reject_fn, ""))
    }

    /// Resolve `p` com `v` (seguindo thenables).
    pub fn resolve_promise(&mut self, p: &ObjRef, v: Value) {
        if let Value::Object(o) = &v {
            if o.ptr_eq(p) {
                let e = self.type_error("Chaining cycle detected for promise #<Promise>");
                self.reject_promise(p, e);
                return;
            }
            let then = match self.get_obj(o, &PropKey::str("then")) {
                Ok(t) => t,
                Err(e) => {
                    self.reject_promise(p, e);
                    return;
                }
            };
            if then.is_callable() {
                self.microtasks.push_back(Job::Thenable(p.clone(), v.clone(), then));
                return;
            }
        }
        self.settle(p, PState::Fulfilled(v));
    }

    pub fn reject_promise(&mut self, p: &ObjRef, reason: Value) {
        self.settle(p, PState::Rejected(reason));
    }

    fn settle(&mut self, p: &ObjRef, state: PState) {
        let (reactions, arg, unhandled) = {
            let mut b = p.borrow_mut();
            let Kind::Promise(d) = &mut b.kind else { return };
            if !matches!(d.state, PState::Pending) {
                return;
            }
            let (list, arg, rejected) = match &state {
                PState::Fulfilled(v) => (std::mem::take(&mut d.fulfill), v.clone(), false),
                PState::Rejected(v) => (std::mem::take(&mut d.reject), v.clone(), true),
                PState::Pending => return,
            };
            d.fulfill.clear();
            d.reject.clear();
            d.state = state;
            (list, arg, rejected && !d.handled)
        };
        if unhandled {
            self.pending_rejections.push(p.clone());
        }
        for r in reactions {
            self.microtasks.push_back(Job::Reaction(r, arg.clone()));
        }
    }

    /// PerformPromiseThen.
    pub fn perform_then(&mut self, p: &ObjRef, on_ful: Value, on_rej: Value, cap: Option<Capability>) {
        let f = Reaction {
            cap: cap.clone(),
            handler: if on_ful.is_callable() { on_ful } else { Value::Undefined },
            is_reject: false,
        };
        let r = Reaction {
            cap,
            handler: if on_rej.is_callable() { on_rej } else { Value::Undefined },
            is_reject: true,
        };
        let job = {
            let mut b = p.borrow_mut();
            let Kind::Promise(d) = &mut b.kind else { return };
            let was_handled = d.handled;
            d.handled = true;
            match &d.state {
                PState::Pending => {
                    d.fulfill.push(f);
                    d.reject.push(r);
                    None
                }
                PState::Fulfilled(v) => Some((Job::Reaction(f, v.clone()), false)),
                PState::Rejected(v) => Some((Job::Reaction(r, v.clone()), !was_handled)),
            }
        };
        if let Some((job, late)) = job {
            if late {
                // tratada depois de rejeitada: não é mais "não tratada"
                self.pending_rejections.retain(|x| !x.ptr_eq(p));
            }
            self.microtasks.push_back(job);
        }
    }

    /// NewPromiseCapability(C).
    pub fn new_capability(&mut self, ctor: &Value) -> Result<Capability, Value> {
        let intrinsic = match ctor {
            Value::Object(c) => self.is_intrinsic_promise_ctor(c),
            _ => true,
        };
        if intrinsic {
            let p = self.new_promise();
            let (res, rej) = self.resolving_functions(&p);
            return Ok(Capability {
                promise: Value::Object(p),
                resolve: Value::Object(res),
                reject: Value::Object(rej),
            });
        }
        let Value::Object(c) = ctor else { unreachable!() };
        if !c.is_constructor() {
            return Err(self.type_error("Promise resolve or reject function is not callable"));
        }
        let rec = self.new_object();
        let exec = self.realm.native("", 2, capability_executor);
        set_native_data(&exec, Value::Object(rec.clone()));
        let promise = self.construct(c, vec![Value::Object(exec)], c)?;
        let resolve = self.get_obj(&rec, &PropKey::str("resolve"))?;
        let reject = self.get_obj(&rec, &PropKey::str("reject"))?;
        if !resolve.is_callable() || !reject.is_callable() {
            return Err(self.type_error("Promise resolve or reject function is not callable"));
        }
        Ok(Capability { promise, resolve, reject })
    }

    fn is_intrinsic_promise_ctor(&self, c: &ObjRef) -> bool {
        match self.realm.promise_proto.borrow().props.get(&PropKey::str("constructor")) {
            Some(Property {
                slot: Slot::Data { value: Value::Object(x), .. },
                ..
            }) => x.ptr_eq(c),
            _ => false,
        }
    }

    pub fn is_promise(v: &Value) -> bool {
        matches!(v, Value::Object(o) if matches!(o.borrow().kind, Kind::Promise(_)))
    }

    /// PromiseResolve(%Promise%, v): a própria promessa ou uma nova resolvida com v.
    pub fn promise_resolve_value(&mut self, v: Value) -> Result<ObjRef, Value> {
        if let Value::Object(o) = &v
            && matches!(o.borrow().kind, Kind::Promise(_))
        {
            let ctor = self.get_obj(o, &PropKey::str("constructor"))?;
            if let Value::Object(c) = &ctor
                && self.is_intrinsic_promise_ctor(c)
            {
                return Ok(o.clone());
            }
        }
        let p = self.new_promise();
        self.resolve_promise(&p, v);
        Ok(p)
    }

    /// Roda a fila de microtarefas até esvaziar.
    pub fn run_microtasks(&mut self) {
        let mut guard = 0u64;
        while let Some(job) = self.microtasks.pop_front() {
            guard += 1;
            if guard > 50_000_000 {
                break;
            }
            match job {
                Job::Reaction(r, arg) => {
                    let res = if r.handler.is_undefined() {
                        if r.is_reject { Err(arg) } else { Ok(arg) }
                    } else {
                        self.call(&r.handler, Value::Undefined, vec![arg])
                    };
                    match (r.cap, res) {
                        (Some(cap), Ok(v)) => {
                            let _ = self.call(&cap.resolve, Value::Undefined, vec![v]);
                        }
                        (Some(cap), Err(e)) => {
                            let _ = self.call(&cap.reject, Value::Undefined, vec![e]);
                        }
                        (None, Err(e)) => self.report_uncaught(&e),
                        (None, Ok(_)) => {}
                    }
                }
                Job::Thenable(p, thenable, then) => {
                    let (res, rej) = self.resolving_functions(&p);
                    if let Err(e) = self.call(&then, thenable, vec![Value::Object(res), Value::Object(rej.clone())]) {
                        let _ = self.call_obj(&rej, Value::Undefined, vec![e]);
                    }
                }
                Job::Call(f, args) => {
                    if let Err(e) = self.call(&f, Value::Undefined, args) {
                        self.report_uncaught(&e);
                    }
                }
            }
        }
        self.report_unhandled();
    }

    /// Exceção que escapou de um callback (timer, microtarefa).
    pub fn report_uncaught(&mut self, e: &Value) {
        let text = format!("Uncaught {}", super::console::inspect(self, e, 2));
        (self.output)(Out::Error, &text);
    }

    fn report_unhandled(&mut self) {
        let list = std::mem::take(&mut self.pending_rejections);
        for p in list {
            let reason = {
                let b = p.borrow();
                match &b.kind {
                    Kind::Promise(d) if !d.handled => match &d.state {
                        PState::Rejected(r) => Some(r.clone()),
                        _ => None,
                    },
                    _ => None,
                }
            };
            if let Some(r) = reason {
                let text = format!("Uncaught (in promise) {}", super::console::inspect(self, &r, 2));
                (self.output)(Out::Error, &text);
            }
        }
    }

    // ------------------------------------------------------ funções async

    /// Começa uma função async: roda até o primeiro `await` e devolve a promessa.
    pub fn async_call(&mut self, frame: Frame) -> Result<Value, Value> {
        let promise = self.new_promise();
        let co = ObjRef::new(Object::new(
            None,
            Kind::Async(Box::new(AsyncData {
                frame: Some(Box::new(frame)),
                promise: promise.clone(),
            })),
        ));
        self.async_step(&co, None);
        Ok(Value::Object(promise))
    }

    pub fn async_step(&mut self, co: &ObjRef, resume: Option<(Value, u8)>) {
        let mut resume = resume;
        loop {
            let (frame, promise) = {
                let mut b = co.borrow_mut();
                let Kind::Async(d) = &mut b.kind else { return };
                (d.frame.take(), d.promise.clone())
            };
            let Some(mut frame) = frame else { return };
            if let Some((v, mode)) = resume.take() {
                frame.stack.push(v);
                frame.stack.push(Value::Number(mode as f64));
            }
            frame.suspended = false;
            let r = self.run_frame(&mut frame);
            match r {
                Ok(v) if frame.suspended => {
                    if let Kind::Async(d) = &mut co.borrow_mut().kind {
                        d.frame = Some(frame);
                    }
                    match self.promise_resolve_value(v) {
                        Ok(p) => {
                            let on_ful = self.realm.native("", 1, async_fulfilled);
                            let on_rej = self.realm.native("", 1, async_rejected);
                            set_native_data(&on_ful, Value::Object(co.clone()));
                            set_native_data(&on_rej, Value::Object(co.clone()));
                            self.perform_then(&p, Value::Object(on_ful), Value::Object(on_rej), None);
                            return;
                        }
                        Err(e) => {
                            resume = Some((e, RESUME_THROW));
                            continue;
                        }
                    }
                }
                Ok(v) => {
                    self.resolve_promise(&promise, v);
                    return;
                }
                Err(e) => {
                    self.reject_promise(&promise, e);
                    return;
                }
            }
        }
    }
}

fn async_fulfilled(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(co) = native_data(&a.callee) {
        vm.async_step(&co, Some((a.arg(0), RESUME_NEXT)));
    }
    Ok(Value::Undefined)
}

fn async_rejected(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(co) = native_data(&a.callee) {
        vm.async_step(&co, Some((a.arg(0), RESUME_THROW)));
    }
    Ok(Value::Undefined)
}

// ------------------------------------------------------------ nativos

fn resolver_data(a: &CallArgs) -> Option<(ObjRef, Rc<Cell<bool>>)> {
    let Value::Object(rec) = native_data(&a.callee) else { return None };
    let b = rec.borrow();
    match &b.kind {
        Kind::PromiseResolver { promise, done } => Some((promise.clone(), done.clone())),
        _ => None,
    }
}

fn resolve_fn(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Some((p, done)) = resolver_data(a)
        && !done.get()
    {
        done.set(true);
        vm.resolve_promise(&p, a.arg(0));
    }
    Ok(Value::Undefined)
}

fn reject_fn(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Some((p, done)) = resolver_data(a)
        && !done.get()
    {
        done.set(true);
        vm.reject_promise(&p, a.arg(0));
    }
    Ok(Value::Undefined)
}

fn capability_executor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(rec) = native_data(&a.callee) else { return Ok(Value::Undefined) };
    let has = |n: &str| !matches!(rec.borrow().props.get(&PropKey::str(n)), None | Some(Property { slot: Slot::Data { value: Value::Undefined, .. }, .. }));
    if has("resolve") || has("reject") {
        return Err(vm.type_error("Promise executor has already been invoked with non-undefined arguments"));
    }
    rec.borrow_mut().props.insert(PropKey::str("resolve"), Property::data(a.arg(0)));
    rec.borrow_mut().props.insert(PropKey::str("reject"), Property::data(a.arg(1)));
    Ok(Value::Undefined)
}

fn promise_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Some(nt) = a.new_target.clone() else {
        return Err(vm.type_error("Promise constructor cannot be invoked without 'new'"));
    };
    let exec = a.arg(0);
    if !exec.is_callable() {
        let d = vm.describe(&exec);
        return Err(vm.type_error(format!("Promise resolver {d} is not a function")));
    }
    let def = vm.realm.promise_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let p = vm.new_promise_with_proto(proto);
    let (res, rej) = vm.resolving_functions(&p);
    if let Err(e) = vm.call(&exec, Value::Undefined, vec![Value::Object(res), Value::Object(rej.clone())]) {
        vm.call_obj(&rej, Value::Undefined, vec![e])?;
    }
    Ok(Value::Object(p))
}

fn this_promise(vm: &mut Vm, a: &CallArgs, what: &str) -> Result<ObjRef, Value> {
    match &a.this {
        Value::Object(o) if matches!(o.borrow().kind, Kind::Promise(_)) => Ok(o.clone()),
        _ => {
            let d = vm.describe(&a.this);
            Err(vm.type_error(format!("Method Promise.prototype.{what} called on incompatible receiver {d}")))
        }
    }
}

/// SpeciesConstructor(p, %Promise%): `p.constructor[Symbol.species]`.
fn species_constructor(vm: &mut Vm, p: &ObjRef) -> Result<Value, Value> {
    let default = vm.get_str(&Value::Object(vm.realm.global.clone()), "Promise")?;
    let ctor = vm.get_obj(p, &PropKey::str("constructor"))?;
    let c = match ctor {
        Value::Undefined => return Ok(default),
        Value::Object(c) => c,
        _ => return Err(vm.type_error("The .constructor property is not an object")),
    };
    let species = vm.realm.sym_species.clone();
    match vm.get_obj(&c, &PropKey::Sym(species))? {
        Value::Undefined | Value::Null => Ok(default),
        Value::Object(s) if s.is_constructor() => Ok(Value::Object(s)),
        _ => Err(vm.type_error("object.constructor[Symbol.species] is not a constructor")),
    }
}

fn then(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let p = this_promise(vm, a, "then")?;
    let ctor = species_constructor(vm, &p)?;
    let cap = vm.new_capability(&ctor)?;
    let result = cap.promise.clone();
    vm.perform_then(&p, a.arg(0), a.arg(1), Some(cap));
    Ok(result)
}

fn catch(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let then = vm.get_str(&a.this, "then")?;
    vm.call(&then, a.this.clone(), vec![Value::Undefined, a.arg(0)])
}

fn finally(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let on_finally = a.arg(0);
    let then = vm.get_str(&a.this, "then")?;
    if !on_finally.is_callable() {
        return vm.call(&then, a.this.clone(), vec![on_finally.clone(), on_finally]);
    }
    let tf = vm.realm.native("", 1, finally_then);
    let cf = vm.realm.native("", 1, finally_catch);
    set_native_data(&tf, on_finally.clone());
    set_native_data(&cf, on_finally);
    vm.call(&then, a.this.clone(), vec![Value::Object(tf), Value::Object(cf)])
}

fn finally_common(vm: &mut Vm, a: &CallArgs, thunk: NativeFn) -> Result<Value, Value> {
    let on_finally = native_data(&a.callee);
    let r = vm.call(&on_finally, Value::Undefined, Vec::new())?;
    let p = vm.promise_resolve_value(r)?;
    let f = vm.realm.native("", 0, thunk);
    set_native_data(&f, a.arg(0));
    let then = vm.get_obj(&p, &PropKey::str("then"))?;
    vm.call(&then, Value::Object(p), vec![Value::Object(f)])
}

fn finally_then(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    finally_common(vm, a, value_thunk)
}

fn finally_catch(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    finally_common(vm, a, thrower_thunk)
}

fn value_thunk(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(native_data(&a.callee))
}

fn thrower_thunk(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Err(native_data(&a.callee))
}

fn promise_resolve(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let v = a.arg(0);
    if let (Value::Object(c), Value::Object(o)) = (&a.this, &v)
        && matches!(o.borrow().kind, Kind::Promise(_))
    {
        let vc = vm.get_obj(o, &PropKey::str("constructor"))?;
        if let Value::Object(vc) = vc
            && vc.ptr_eq(c)
        {
            return Ok(v);
        }
    }
    let cap = vm.new_capability(&a.this)?;
    vm.call(&cap.resolve, Value::Undefined, vec![v])?;
    Ok(cap.promise)
}

fn promise_reject(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cap = vm.new_capability(&a.this)?;
    vm.call(&cap.reject, Value::Undefined, vec![a.arg(0)])?;
    Ok(cap.promise)
}

fn with_resolvers(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cap = vm.new_capability(&a.this)?;
    let o = vm.new_object();
    vm.define_data(&o, PropKey::str("promise"), cap.promise);
    vm.define_data(&o, PropKey::str("resolve"), cap.resolve);
    vm.define_data(&o, PropKey::str("reject"), cap.reject);
    Ok(Value::Object(o))
}

/// Estado compartilhado de Promise.all/allSettled/any.
fn combinator_record(vm: &mut Vm, cap: &Capability, n_hint: usize) -> ObjRef {
    let rec = vm.new_object();
    let values = vm.array_value(Vec::with_capacity(n_hint));
    vm.define_data(&rec, PropKey::str("values"), values);
    vm.define_data(&rec, PropKey::str("remaining"), Value::Number(1.0));
    vm.define_data(&rec, PropKey::str("resolve"), cap.resolve.clone());
    vm.define_data(&rec, PropKey::str("reject"), cap.reject.clone());
    rec
}

fn rec_get(rec: &ObjRef, k: &str) -> Value {
    rec.borrow().own(&PropKey::str(k)).map(|p| match p.slot {
        Slot::Data { value, .. } => value,
        _ => Value::Undefined,
    }).unwrap_or_default()
}

/// Decrementa "remaining"; devolve true quando chegou a zero.
fn rec_done(vm: &mut Vm, rec: &ObjRef) -> bool {
    let n = match rec_get(rec, "remaining") {
        Value::Number(n) => n - 1.0,
        _ => 0.0,
    };
    vm.define_data(rec, PropKey::str("remaining"), Value::Number(n));
    n <= 0.0
}

/// Elemento de Promise.all: guarda o valor na posição e resolve no fim.
fn element_fn(vm: &mut Vm, rec: &ObjRef, index: usize, kind: u8) -> ObjRef {
    let f = vm.realm.native("", 1, all_element);
    let data = vm.new_object();
    vm.define_data(&data, PropKey::str("rec"), Value::Object(rec.clone()));
    vm.define_data(&data, PropKey::str("index"), Value::Number(index as f64));
    vm.define_data(&data, PropKey::str("kind"), Value::Number(kind as f64));
    vm.define_data(&data, PropKey::str("called"), Value::Bool(false));
    set_native_data(&f, Value::Object(data));
    f
}

