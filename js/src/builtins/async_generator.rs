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
    let afs = &r.async_from_sync_iter_proto;
    r.method(afs, "next", 1, afs_next);
    r.method(afs, "return", 1, afs_return);
    r.method(afs, "throw", 1, afs_throw);
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
            GenState::Done if mode == RESUME_RETURN => {
                // AsyncGeneratorAwaitReturn: espera o valor antes de responder
                match vm.promise_resolve_value(value) {
                    Ok(p) => {
                        set_state(g, GenState::Running);
                        let on_ful = vm.realm.native("", 1, await_return_fulfilled);
                        let on_rej = vm.realm.native("", 1, await_return_rejected);
                        set_native_data(&on_ful, Value::Object(g.clone()));
                        set_native_data(&on_rej, Value::Object(g.clone()));
                        vm.perform_then(&p, Value::Object(on_ful), Value::Object(on_rej), None);
                        return;
                    }
                    Err(e) => {
                        finish_front(vm, g, Err(e), true);
                        continue;
                    }
                }
            }
            GenState::Done => {
                let r = match mode {
                    RESUME_THROW => Err(value),
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

fn continue_after_await(vm: &mut Vm, a: &CallArgs, mode: u8) {
    let Value::Object(g) = native_data(&a.callee) else { return };
    let frame = {
        let mut b = g.borrow_mut();
        let Kind::AsyncGenerator(d) = &mut b.kind else { return };
        d.frame.take()
    };
    if let Some(mut frame) = frame {
        frame.stack.push(a.arg(0));
        frame.stack.push(Value::Number(mode as f64));
        run(vm, &g, frame);
    }
}

fn after_await_return(vm: &mut Vm, a: &CallArgs, r: Result<Value, Value>) {
    let Value::Object(g) = native_data(&a.callee) else { return };
    set_state(&g, GenState::Done);
    finish_front(vm, &g, r, true);
    resume_next(vm, &g);
}

fn await_return_fulfilled(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    after_await_return(vm, a, Ok(a.arg(0)));
    Ok(Value::Undefined)
}

fn await_return_rejected(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    after_await_return(vm, a, Err(a.arg(0)));
    Ok(Value::Undefined)
}

fn await_fulfilled(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    continue_after_await(vm, a, RESUME_NEXT);
    Ok(Value::Undefined)
}

fn await_rejected(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    continue_after_await(vm, a, RESUME_THROW);
    Ok(Value::Undefined)
}

impl Vm {
    /// Iterador para `for await`: Symbol.asyncIterator, ou o síncrono embrulhado.
    pub fn get_async_iterator(&mut self, v: &Value) -> Result<ObjRef, Value> {
        let key = PropKey::Sym(self.realm.sym_async_iterator.clone());
        let m = self.get(v, &key)?;
        if m.is_nullish() {
            // CreateAsyncFromSyncIterator: um objeto que guarda o registro síncrono
            let sync = self.get_iterator(v)?;
            let (iter, next) = match &sync.borrow().kind {
                Kind::IterRecord { iter, next, .. } => (iter.clone(), next.clone()),
                _ => (Value::Undefined, Value::Undefined),
            };
            let wrapper = ObjRef::new(Object::new(
                Some(self.realm.async_from_sync_iter_proto.clone()),
                Kind::IterRecord {
                    iter,
                    next,
                    done: false,
                    sync_wrapped: true,
                },
            ));
            let w = Value::Object(wrapper);
            let next = self.get_str(&w, "next")?;
            return Ok(ObjRef::new(Object::new(
                None,
                Kind::IterRecord {
                    iter: w,
                    next,
                    done: false,
                    sync_wrapped: false,
                },
            )));
        }
        let it = self.call(&m, v.clone(), Vec::new())?;
        if !matches!(it, Value::Object(_)) {
            return Err(self.type_error("Result of the Symbol.asyncIterator method is not an object"));
        }
        let next = self.get_str(&it, "next")?;
        Ok(ObjRef::new(Object::new(
            None,
            Kind::IterRecord {
                iter: it,
                next,
                done: false,
                sync_wrapped: false,
            },
        )))
    }

    /// Chama next() (o embrulho de um iterador síncrono já devolve uma promessa).
    pub fn async_iter_next(&mut self, rec: &ObjRef) -> Result<Value, Value> {
        let (iter, next) = match &rec.borrow().kind {
            Kind::IterRecord { iter, next, .. } => (iter.clone(), next.clone()),
            _ => return Ok(Value::Undefined),
        };
        self.call(&next, iter, Vec::new())
    }
}

// ------------------------------------------- %AsyncFromSyncIteratorPrototype%

/// GetMethod: undefined/null viram undefined; outro não chamável é TypeError.
fn get_method(vm: &mut Vm, v: &Value, name: &str) -> Result<Value, Value> {
    let m = vm.get_str(v, name)?;
    if m.is_nullish() {
        return Ok(Value::Undefined);
    }
    if !m.is_callable() {
        let d = vm.describe(&m);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    Ok(m)
}

/// O iterador síncrono guardado no embrulho (`this`).
fn afs_record(this: &Value) -> Option<(Value, Value)> {
    let Value::Object(o) = this else { return None };
    match &o.borrow().kind {
        Kind::IterRecord { iter, next, .. } => Some((iter.clone(), next.clone())),
        _ => None,
    }
}

/// Rejeita a promessa do pedido e a devolve (IfAbruptRejectPromise).
fn reject_with(vm: &mut Vm, cap: &Capability, e: Value) -> Result<Value, Value> {
    vm.call(&cap.reject, Value::Undefined, vec![e])?;
    Ok(cap.promise.clone())
}

fn afs_next(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cap = vm.new_capability(&Value::Undefined)?;
    let Some((iter, next)) = afs_record(&a.this) else {
        let e = vm.type_error("not an AsyncFromSyncIterator");
        return reject_with(vm, &cap, e);
    };
    let args = if a.args.is_empty() { Vec::new() } else { vec![a.arg(0)] };
    let r = match vm.call(&next, iter, args) {
        Ok(r) => r,
        Err(e) => return reject_with(vm, &cap, e),
    };
    if !matches!(r, Value::Object(_)) {
        let d = vm.describe(&r);
        let e = vm.type_error(format!("Iterator result {d} is not an object"));
        return reject_with(vm, &cap, e);
    }
    afs_continuation(vm, &a.this, r, cap, true)
}

fn afs_return(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cap = vm.new_capability(&Value::Undefined)?;
    let Some((iter, _)) = afs_record(&a.this) else {
        let e = vm.type_error("not an AsyncFromSyncIterator");
        return reject_with(vm, &cap, e);
    };
    let m = match get_method(vm, &iter, "return") {
        Ok(m) => m,
        Err(e) => return reject_with(vm, &cap, e),
    };
    if m.is_nullish() {
        let res = iter_result(vm, a.arg(0), true);
        vm.call(&cap.resolve, Value::Undefined, vec![res])?;
        return Ok(cap.promise);
    }
    let args = if a.args.is_empty() { Vec::new() } else { vec![a.arg(0)] };
    let r = match vm.call(&m, iter, args) {
        Ok(r) => r,
        Err(e) => return reject_with(vm, &cap, e),
    };
    if !matches!(r, Value::Object(_)) {
        let e = vm.type_error("iterator.return() did not return an object");
        return reject_with(vm, &cap, e);
    }
    afs_continuation(vm, &a.this, r, cap, false)
}

fn afs_throw(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cap = vm.new_capability(&Value::Undefined)?;
    let Some((iter, _)) = afs_record(&a.this) else {
        let e = vm.type_error("not an AsyncFromSyncIterator");
        return reject_with(vm, &cap, e);
    };
    let m = match get_method(vm, &iter, "throw") {
        Ok(m) => m,
        Err(e) => return reject_with(vm, &cap, e),
    };
    if m.is_nullish() {
        // sem throw(): fecha o iterador síncrono e rejeita com TypeError
        if let Err(e) = afs_close(vm, &iter) {
            return reject_with(vm, &cap, e);
        }
        let e = vm.type_error("The iterator does not provide a 'throw' method");
        return reject_with(vm, &cap, e);
    }
    let r = match vm.call(&m, iter, vec![a.arg(0)]) {
        Ok(r) => r,
        Err(e) => return reject_with(vm, &cap, e),
    };
    if !matches!(r, Value::Object(_)) {
        let e = vm.type_error("iterator.throw() did not return an object");
        return reject_with(vm, &cap, e);
    }
    afs_continuation(vm, &a.this, r, cap, true)
}

/// IteratorClose com conclusão normal: chama return() e confere o resultado.
fn afs_close(vm: &mut Vm, iter: &Value) -> Result<(), Value> {
    let m = get_method(vm, iter, "return")?;
    if m.is_nullish() {
        return Ok(());
    }
    let r = vm.call(&m, iter.clone(), Vec::new())?;
    if !matches!(r, Value::Object(_)) {
        return Err(vm.type_error("iterator.return() did not return an object"));
    }
    Ok(())
}

/// IteratorClose com um erro: chama return() e ignora o que acontecer nele.
fn afs_close_quiet(vm: &mut Vm, iter: &Value) {
    if let Ok(m) = get_method(vm, iter, "return")
        && !m.is_nullish()
    {
        let _ = vm.call(&m, iter.clone(), Vec::new());
    }
}

/// AsyncFromSyncIteratorContinuation.
fn afs_continuation(vm: &mut Vm, this: &Value, result: Value, cap: Capability, close_on_rejection: bool) -> Result<Value, Value> {
    let done = match vm.get_str(&result, "done") {
        Ok(d) => crate::vm::truthy(&d),
        Err(e) => return reject_with(vm, &cap, e),
    };
    let value = match vm.get_str(&result, "value") {
        Ok(v) => v,
        Err(e) => return reject_with(vm, &cap, e),
    };
    let wrapper = match vm.promise_resolve_value(value) {
        Ok(p) => p,
        Err(e) => {
            if !done
                && close_on_rejection
                && let Some((iter, _)) = afs_record(this)
            {
                afs_close_quiet(vm, &iter);
            }
            return reject_with(vm, &cap, e);
        }
    };
    let unwrap = vm.realm.native("", 1, afs_unwrap);
    set_native_data(&unwrap, Value::Bool(done));
    let on_rej = if done || !close_on_rejection {
        Value::Undefined
    } else {
        let close = vm.realm.native("", 1, afs_close_on_reject);
        set_native_data(&close, this.clone());
        Value::Object(close)
    };
    vm.perform_then(&wrapper, Value::Object(unwrap), on_rej, Some(cap.clone()));
    Ok(cap.promise)
}

fn afs_unwrap(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let done = matches!(native_data(&a.callee), Value::Bool(true));
    Ok(iter_result(vm, a.arg(0), done))
}

fn afs_close_on_reject(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let this = native_data(&a.callee);
    if let Some((iter, _)) = afs_record(&this) {
        afs_close_quiet(vm, &iter);
    }
    Err(a.arg(0))
}
