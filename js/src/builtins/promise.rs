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

