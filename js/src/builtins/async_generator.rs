//! Geradores async (`async function*`) e `for await`.
//!
//! Cada next/return/throw entra numa fila e devolve uma promessa; o gerador
//! roda um pedido por vez. Ele pode pausar em `await` (espera uma promessa e
//! continua sozinho) ou em `yield` (entrega o valor e atende o próximo pedido).

use std::collections::VecDeque;

use super::array::iter_result;
use super::generator::GenState;
use super::promise::{Capability, RESUME_NEXT, RESUME_RETURN, RESUME_THROW};
use super::{Realm, native_data, set_native_data};
use crate::value::*;
use crate::vm::{Frame, Vm};

pub struct AsyncGenRequest {
    pub mode: u8,
    pub value: Value,
    pub cap: Capability,
}

pub struct AsyncGenData {
    pub frame: Option<Box<Frame>>,
    pub state: GenState,
    pub queue: VecDeque<AsyncGenRequest>,
}

pub fn install(r: &Realm) {
    let p = &r.async_generator_proto;
    r.method(p, "next", 1, next);
    r.method(p, "return", 1, ret);
    r.method(p, "throw", 1, throw);
    r.tag(p, "AsyncGenerator");
    r.method_sym(&r.async_iterator_proto, &r.sym_async_iterator, "[Symbol.asyncIterator]", 0, super::array::return_this);
    r.async_generator_function_proto.borrow_mut().props.insert(
        PropKey::str("prototype"),
        Property::hidden(Value::Object(r.async_generator_proto.clone())),
    );
}

fn next(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    enqueue(vm, &a.this, RESUME_NEXT, a.arg(0))
}

fn ret(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    enqueue(vm, &a.this, RESUME_RETURN, a.arg(0))
}

fn throw(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    enqueue(vm, &a.this, RESUME_THROW, a.arg(0))
}

fn enqueue(vm: &mut Vm, this: &Value, mode: u8, value: Value) -> Result<Value, Value> {
    let cap = vm.new_capability(&Value::Undefined)?;
    let promise = cap.promise.clone();
    let g = match this {
        Value::Object(o) if matches!(o.borrow().kind, Kind::AsyncGenerator(_)) => o.clone(),
        _ => {
            let d = vm.describe(this);
            let e = vm.type_error(format!("next method called on incompatible receiver {d}"));
            vm.call(&cap.reject, Value::Undefined, vec![e])?;
            return Ok(promise);
        }
    };
    let running = {
        let mut b = g.borrow_mut();
        let Kind::AsyncGenerator(d) = &mut b.kind else { return Ok(promise) };
        d.queue.push_back(AsyncGenRequest { mode, value, cap });
        d.state == GenState::Running
    };
    if !running {
        resume_next(vm, &g);
    }
    Ok(promise)
}

