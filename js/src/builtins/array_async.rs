//! Array.fromAsync: copia um iterável assíncrono, síncrono ou array-like para
//! um array, esperando cada valor.
//!
//! O algoritmo do spec é uma função async; aqui ele vira uma máquina de
//! estados (`FromAsync`) que pausa em cada `Await` e continua quando a
//! promessa esperada se resolve.

use super::iterator::{IterRec, create_data_or_throw, get_method};
use super::promise::Capability;
use super::{native_data, set_native_data};
use crate::ops::index_key;
use crate::value::*;
use crate::vm::{Vm, truthy};

/// Onde a máquina parou.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Início de uma volta do laço.
    Top,
    /// Esperando o resultado de next().
    Next,
    /// Esperando o valor devolvido por mapfn (iterável).
    Mapped,
    /// Esperando um elemento do array-like.
    Elem,
    /// Esperando o valor devolvido por mapfn (array-like).
    MappedElem,
    /// Esperando o return() do iterador depois de um erro (`pending`).
    Close,
}

pub struct FromAsync {
    pub cap: Capability,
    /// O array (ou objeto do construtor `this`) sendo preenchido.
    pub target: Value,
    pub k: usize,
    pub mapfn: Option<Value>,
    pub this_arg: Value,
    /// Iterador assíncrono (ou o síncrono embrulhado); None = array-like.
    pub iter: Option<IterRec>,
    pub items: Value,
    pub len: usize,
    pub phase: Phase,
    /// Erro a relançar depois de fechar o iterador.
    pub pending: Value,
}

impl FromAsync {
    /// Valores presos (para o coletor de ciclos).
    pub fn trace(&self, f: &mut dyn FnMut(&Value)) {
        for v in [&self.cap.promise, &self.cap.resolve, &self.cap.reject, &self.target, &self.this_arg, &self.items, &self.pending] {
            f(v);
        }
        if let Some(m) = &self.mapfn {
            f(m);
        }
        if let Some(r) = &self.iter {
            r.trace(f);
        }
    }
}

enum Step {
    Await(Value),
    Done(Result<Value, Value>),
}

pub fn from_async(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cap = vm.new_capability(&Value::Undefined)?;
    match start(vm, a, cap.clone()) {
        Ok(st) => drive(vm, &st, None),
        Err(e) => {
            vm.call(&cap.reject, Value::Undefined, vec![e])?;
        }
    }
    Ok(cap.promise)
}

/// A parte síncrona: confere mapfn, escolhe o caminho e cria o array.
fn start(vm: &mut Vm, a: &CallArgs, cap: Capability) -> Result<ObjRef, Value> {
    let items = a.arg(0);
    let mapfn = match a.arg(1) {
        Value::Undefined => None,
        f if f.is_callable() => Some(f),
        f => {
            let d = vm.describe(&f);
            return Err(vm.type_error(format!("{d} is not a function")));
        }
    };
    let async_key = PropKey::Sym(vm.realm.sym_async_iterator.clone());
    let sync_key = PropKey::Sym(vm.realm.sym_iterator.clone());
    let using_async = get_method(vm, &items, &async_key)?;
    let using_sync = if using_async.is_none() { get_method(vm, &items, &sync_key)? } else { None };
    let ctor = match &a.this {
        Value::Object(c) if c.is_constructor() => Some(c.clone()),
        _ => None,
    };
    let (iter, target, len) = if let Some(m) = using_async.or(using_sync.clone()) {
        let it = vm.call(&m, items.clone(), Vec::new())?;
        if !matches!(it, Value::Object(_)) {
            return Err(vm.type_error("Result of the Symbol.asyncIterator method is not an object"));
        }
        let next = vm.get_str(&it, "next")?;
        let rec = if using_sync.is_some() {
            // CreateAsyncFromSyncIterator
            let w = ObjRef::new(Object::new(
                Some(vm.realm.async_from_sync_iter_proto.clone()),
                Kind::IterRecord {
                    iter: it,
                    next,
                    done: false,
                    sync_wrapped: true,
                },
            ));
            let w = Value::Object(w);
            let next = vm.get_str(&w, "next")?;
            IterRec { iter: w, next, done: false }
        } else {
            IterRec { iter: it, next, done: false }
        };
        let target = match &ctor {
            Some(c) => vm.construct(c, Vec::new(), c)?,
            None => vm.array_value(Vec::new()),
        };
        (Some(rec), target, 0)
    } else {
        let o = vm.to_object(&items)?;
        let len = vm.length_of(&o)?;
        let target = match &ctor {
            Some(c) => vm.construct(c, vec![Value::Number(len as f64)], c)?,
            None => {
                if len as f64 > 4294967295.0 {
                    return Err(vm.range_error("Invalid array length"));
                }
                let arr = vm.new_array(Vec::new());
                vm.set_array_length(&arr, &Value::Number(len as f64))?;
                Value::Object(arr)
            }
        };
        (None, target, len)
    };
    let d = FromAsync {
        cap,
        target,
        k: 0,
        mapfn,
        this_arg: a.arg(2),
        iter,
        items,
        len,
        phase: Phase::Top,
        pending: Value::Undefined,
    };
    Ok(ObjRef::new(Object::new(None, Kind::FromAsync(Some(Box::new(d))))))
}

/// Roda a máquina até o próximo Await (ou até o fim) e liga a continuação.
fn drive(vm: &mut Vm, st: &ObjRef, input: Option<Result<Value, Value>>) {
    let Some(mut d) = (match &mut st.borrow_mut().kind {
        Kind::FromAsync(d) => d.take(),
        _ => None,
    }) else {
        return;
    };
    let mut input = input;
    loop {
        match advance(vm, &mut d, input.take()) {
            Step::Await(v) => match vm.promise_resolve_value(v) {
                Ok(p) => {
                    let on_ful = vm.realm.native("", 1, resumed_ok);
                    let on_rej = vm.realm.native("", 1, resumed_err);
                    set_native_data(&on_ful, Value::Object(st.clone()));
                    set_native_data(&on_rej, Value::Object(st.clone()));
                    if let Kind::FromAsync(slot) = &mut st.borrow_mut().kind {
                        *slot = Some(d);
                    }
                    vm.perform_then(&p, Value::Object(on_ful), Value::Object(on_rej), None);
                    return;
                }
                // o próprio Await falhou: continua com o erro
                Err(e) => input = Some(Err(e)),
            },
            Step::Done(r) => {
                let (f, v) = match r {
                    Ok(v) => (d.cap.resolve.clone(), v),
                    Err(e) => (d.cap.reject.clone(), e),
                };
                let _ = vm.call(&f, Value::Undefined, vec![v]);
                return;
            }
        }
    }
}

fn resumed_ok(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(st) = native_data(&a.callee) {
        drive(vm, &st, Some(Ok(a.arg(0))));
    }
    Ok(Value::Undefined)
}

fn resumed_err(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(st) = native_data(&a.callee) {
        drive(vm, &st, Some(Err(a.arg(0))));
    }
    Ok(Value::Undefined)
}

/// AsyncIteratorClose com um erro: chama return() e espera o resultado
/// antes de rejeitar com o erro original.
fn close_with(vm: &mut Vm, d: &mut FromAsync, e: Value) -> Step {
    let iter = d.iter.as_ref().map(|r| r.iter.clone()).unwrap_or_default();
    let Ok(Some(f)) = get_method(vm, &iter, &PropKey::str("return")) else { return Step::Done(Err(e)) };
    match vm.call(&f, iter, Vec::new()) {
        Ok(r) => {
            d.phase = Phase::Close;
            d.pending = e;
            Step::Await(r)
        }
        Err(_) => Step::Done(Err(e)),
    }
}

/// Grava `v` na posição k e passa para a próxima volta.
fn store(vm: &mut Vm, d: &mut FromAsync, v: Value) -> Option<Step> {
    let Value::Object(t) = d.target.clone() else { return Some(Step::Done(Err(Value::Undefined))) };
    if let Err(e) = create_data_or_throw(vm, &t, index_key(d.k), v) {
        return Some(if d.iter.is_some() { close_with(vm, d, e) } else { Step::Done(Err(e)) });
    }
    d.k += 1;
    d.phase = Phase::Top;
    None
}

/// Set(A, "length", k, true) e devolve A.
fn finish(vm: &mut Vm, d: &FromAsync, len: usize) -> Step {
    match vm.put(&d.target, PropKey::str("length"), Value::Number(len as f64), true) {
        Ok(()) => Step::Done(Ok(d.target.clone())),
        Err(e) => Step::Done(Err(e)),
    }
}

/// Avança a partir da fase atual, com o resultado do Await que estava pendente.
fn advance(vm: &mut Vm, d: &mut FromAsync, input: Option<Result<Value, Value>>) -> Step {
    let mut input = input;
    loop {
        let got = input.take().unwrap_or(Ok(Value::Undefined));
        let next_store = match d.phase {
            Phase::Top => {
                if let Some(rec) = &d.iter {
                    if d.k as f64 >= 9007199254740991.0 {
                        let e = vm.type_error("Array.fromAsync: too many elements");
                        return close_with(vm, d, e);
                    }
                    match vm.call(&rec.next, rec.iter.clone(), Vec::new()) {
                        Ok(v) => {
                            d.phase = Phase::Next;
                            return Step::Await(v);
                        }
                        Err(e) => return Step::Done(Err(e)),
                    }
                }
                if d.k >= d.len {
                    return finish(vm, d, d.len);
                }
                match vm.get(&d.items, &index_key(d.k)) {
                    Ok(v) => {
                        d.phase = Phase::Elem;
                        return Step::Await(v);
                    }
                    Err(e) => return Step::Done(Err(e)),
                }
            }
            Phase::Next => {
                let res = match got {
                    Ok(r) => r,
                    Err(e) => return Step::Done(Err(e)),
                };
                if !matches!(res, Value::Object(_)) {
                    let dsc = vm.describe(&res);
                    return Step::Done(Err(vm.type_error(format!("Iterator result {dsc} is not an object"))));
                }
                match vm.get_str(&res, "done") {
                    Ok(done) if truthy(&done) => return finish(vm, d, d.k),
                    Ok(_) => {}
                    Err(e) => return Step::Done(Err(e)),
                }
                let v = match vm.get_str(&res, "value") {
                    Ok(v) => v,
                    Err(e) => return Step::Done(Err(e)),
                };
                match d.mapfn.clone() {
                    Some(f) => match vm.call(&f, d.this_arg.clone(), vec![v, Value::Number(d.k as f64)]) {
                        Ok(m) => {
                            d.phase = Phase::Mapped;
                            return Step::Await(m);
                        }
                        Err(e) => return close_with(vm, d, e),
                    },
                    None => v,
                }
            }
            Phase::Mapped => match got {
                Ok(v) => v,
                Err(e) => return close_with(vm, d, e),
            },
            Phase::Elem => {
                let v = match got {
                    Ok(v) => v,
                    Err(e) => return Step::Done(Err(e)),
                };
                match d.mapfn.clone() {
                    Some(f) => match vm.call(&f, d.this_arg.clone(), vec![v, Value::Number(d.k as f64)]) {
                        Ok(m) => {
                            d.phase = Phase::MappedElem;
                            return Step::Await(m);
                        }
                        Err(e) => return Step::Done(Err(e)),
                    },
                    None => v,
                }
            }
            Phase::MappedElem => match got {
                Ok(v) => v,
                Err(e) => return Step::Done(Err(e)),
            },
            Phase::Close => return Step::Done(Err(d.pending.clone())),
        };
        if let Some(step) = store(vm, d, next_store) {
            return step;
        }
    }
}
