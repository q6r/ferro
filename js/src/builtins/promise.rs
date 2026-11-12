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

