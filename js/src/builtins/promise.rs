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

