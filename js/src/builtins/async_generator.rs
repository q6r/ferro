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

/// Atende os pedidos da fila enquanto o gerador não estiver rodando.
fn resume_next(vm: &mut Vm, g: &ObjRef) {
    loop {
        let (state, mode, value) = {
            let b = g.borrow();
            let Kind::AsyncGenerator(d) = &b.kind else { return };
            let Some(req) = d.queue.front() else { return };
            (d.state, req.mode, req.value.clone())
        };
        match state {
            GenState::Running => return,
            GenState::Start if mode != RESUME_NEXT => {
                set_state(g, GenState::Done);
                continue;
            }
            GenState::Done => {
                let r = match mode {
                    RESUME_THROW => Err(value),
                    RESUME_RETURN => Ok(value),
                    _ => Ok(Value::Undefined),
                };
                finish_front(vm, g, r, true);
                continue;
            }
            _ => {
                let frame = {
                    let mut b = g.borrow_mut();
                    let Kind::AsyncGenerator(d) = &mut b.kind else { return };
                    d.state = GenState::Running;
                    d.frame.take()
                };
                let Some(mut frame) = frame else {
                    set_state(g, GenState::Done);
                    continue;
                };
                if state == GenState::Yield {
                    frame.stack.push(value);
                    frame.stack.push(Value::Number(mode as f64));
                }
                run(vm, g, frame);
                return;
            }
        }
    }
}

fn set_state(g: &ObjRef, s: GenState) {
    if let Kind::AsyncGenerator(d) = &mut g.borrow_mut().kind {
        d.state = s;
    }
}

/// Resolve (ou rejeita) o pedido da frente com `{ value, done }`.
fn finish_front(vm: &mut Vm, g: &ObjRef, r: Result<Value, Value>, done: bool) {
    let req = {
        let mut b = g.borrow_mut();
        let Kind::AsyncGenerator(d) = &mut b.kind else { return };
        d.queue.pop_front()
    };
    let Some(req) = req else { return };
    match r {
        Ok(v) => {
            let res = iter_result(vm, v, done);
            let _ = vm.call(&req.cap.resolve, Value::Undefined, vec![res]);
        }
        Err(e) => {
            let _ = vm.call(&req.cap.reject, Value::Undefined, vec![e]);
        }
    }
}

fn run(vm: &mut Vm, g: &ObjRef, mut frame: Box<Frame>) {
    let mut frame_opt = Some(frame);
    let mut resume: Option<(Value, u8)> = None;
    loop {
        let Some(f) = frame_opt.take() else { return };
        frame = f;
        if let Some((v, m)) = resume.take() {
            frame.stack.push(v);
            frame.stack.push(Value::Number(m as f64));
        }
        frame.suspended = false;
        let r = vm.run_frame(&mut frame);
        match r {
            Ok(v) if frame.suspended && frame.suspend_await => {
                match vm.promise_resolve_value(v) {
                    Ok(p) => {
                        if let Kind::AsyncGenerator(d) = &mut g.borrow_mut().kind {
                            d.frame = Some(frame);
                        }
                        let on_ful = vm.realm.native("", 1, await_fulfilled);
                        let on_rej = vm.realm.native("", 1, await_rejected);
                        set_native_data(&on_ful, Value::Object(g.clone()));
                        set_native_data(&on_rej, Value::Object(g.clone()));
                        vm.perform_then(&p, Value::Object(on_ful), Value::Object(on_rej), None);
                        return;
                    }
                    Err(e) => {
                        resume = Some((e, RESUME_THROW));
                        frame_opt = Some(frame);
                        continue;
                    }
                }
            }
            Ok(v) if frame.suspended => {
                if let Kind::AsyncGenerator(d) = &mut g.borrow_mut().kind {
                    d.frame = Some(frame);
                    d.state = GenState::Yield;
                }
                finish_front(vm, g, Ok(v), false);
                resume_next(vm, g);
                return;
            }
            Ok(v) => {
                set_state(g, GenState::Done);
                finish_front(vm, g, Ok(v), true);
                resume_next(vm, g);
                return;
            }
            Err(e) => {
                set_state(g, GenState::Done);
                finish_front(vm, g, Err(e), true);
                resume_next(vm, g);
                return;
            }
        }
    }
}

