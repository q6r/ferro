//! Iterator e os ajudantes de iterador: `Iterator.from/concat/zip/zipKeyed` e
//! os métodos de `Iterator.prototype` (map, filter, take, drop, flatMap,
//! reduce, toArray, forEach, some, every, find, chunks, windows, includes, join).
//!
//! Os ajudantes que devolvem iteradores são geradores escritos à mão: cada
//! `next` avança a máquina de estados (`Body`) até o próximo valor, e
//! `return` fecha o que estiver aberto, como o spec manda.

use super::Realm;
use super::array::iter_result;
use super::generator::GenState;
use super::object::{Descriptor, define_own_property};
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Vm, same_value_zero, truthy};

/// Registro de iterador do spec (Iterator Record).
#[derive(Clone)]
pub struct IterRec {
    pub iter: Value,
    pub next: Value,
    pub done: bool,
}

impl IterRec {
    /// IteratorStep: o objeto de resultado, ou None quando acabou.
    pub fn step(&mut self, vm: &mut Vm) -> Result<Option<Value>, Value> {
        if self.done {
            return Ok(None);
        }
        let r = vm.call(&self.next, self.iter.clone(), Vec::new());
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                self.done = true;
                return Err(e);
            }
        };
        if !matches!(r, Value::Object(_)) {
            self.done = true;
            let d = vm.describe(&r);
            return Err(vm.type_error(format!("Iterator result {d} is not an object")));
        }
        match vm.get_str(&r, "done") {
            Ok(d) if truthy(&d) => {
                self.done = true;
                Ok(None)
            }
            Ok(_) => Ok(Some(r)),
            Err(e) => {
                self.done = true;
                Err(e)
            }
        }
    }

    /// IteratorStepValue: o próximo valor, ou None quando acabou.
    pub fn step_value(&mut self, vm: &mut Vm) -> Result<Option<Value>, Value> {
        let Some(r) = self.step(vm)? else { return Ok(None) };
        match vm.get_str(&r, "value") {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                self.done = true;
                Err(e)
            }
        }
    }

    pub fn trace(&self, f: &mut dyn FnMut(&Value)) {
        f(&self.iter);
        f(&self.next);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ZipMode {
    Shortest,
    Longest,
    Strict,
}

pub struct ZipData {
    pub iters: Vec<IterRec>,
    /// Ainda abertos (openIters do spec, na mesma ordem de `iters`).
    pub open: Vec<bool>,
    pub mode: ZipMode,
    pub padding: Vec<Value>,
    /// zipKeyed: as chaves do objeto de resultado.
    pub keys: Option<Vec<PropKey>>,
}

pub enum Op {
    Map(Value),
    Filter(Value),
    /// Quantos ainda podem sair (infinito = sem limite).
    Take(f64),
    /// Quantos ainda faltam pular.
    Drop(f64),
    FlatMap(Value),
    /// Pares (método @@iterator, iterável) e o próximo a abrir.
    Concat(Vec<(Value, Value)>, usize),
    Zip(Box<ZipData>),
    /// Tamanho e o pedaço em formação.
    Chunks(usize, Vec<Value>),
    /// Tamanho, aceita janela incompleta, janela atual.
    Windows(usize, bool, Vec<Value>),
}

pub struct Body {
    pub op: Op,
    /// [[UnderlyingIterator]] (ausente em concat e zip).
    pub under: Option<IterRec>,
    /// Iterador interno de flatMap/concat.
    pub inner: Option<IterRec>,
    pub counter: f64,
}

/// Objeto de %IteratorHelperPrototype%.
pub struct Helper {
    pub state: GenState,
    /// Fica fora do objeto enquanto o ajudante roda.
    pub body: Option<Box<Body>>,
}

impl Helper {
    /// Valores presos (para o coletor de ciclos).
    pub fn trace(&self, f: &mut dyn FnMut(&Value)) {
        let Some(b) = &self.body else { return };
        for r in b.under.iter().chain(b.inner.iter()) {
            r.trace(f);
        }
        match &b.op {
            Op::Map(v) | Op::Filter(v) | Op::FlatMap(v) => f(v),
            Op::Concat(items, _) => {
                for (m, it) in items {
                    f(m);
                    f(it);
                }
            }
            Op::Zip(z) => {
                for r in &z.iters {
                    r.trace(f);
                }
                z.padding.iter().for_each(|v| f(v));
            }
            Op::Chunks(_, buf) | Op::Windows(_, _, buf) => buf.iter().for_each(|v| f(v)),
            Op::Take(_) | Op::Drop(_) => {}
        }
    }
}

pub fn install(r: &Realm) {
    let ip = &r.iterator_proto;
    let c = super::native_fn(&r.function_proto, "Iterator", 0, iterator_ctor, true);
    c.borrow_mut().props.insert(
        PropKey::str("prototype"),
        Property {
            slot: Slot::Data {
                value: Value::Object(ip.clone()),
                writable: false,
            },
            enumerable: false,
            configurable: false,
        },
    );
    r.global_value("Iterator", Value::Object(c.clone()));
    r.method(&c, "from", 1, from);
    r.method(&c, "concat", 0, concat);
    r.method(&c, "zip", 1, zip);
    r.method(&c, "zipKeyed", 1, zip_keyed);

    // constructor e @@toStringTag são acessores (SetterThatIgnoresPrototypeProperties)
    let cget = r.native("get constructor", 0, ctor_get);
    super::set_native_data(&cget, Value::Object(c));
    let cset = r.native("set constructor", 1, ctor_set);
    let tget = r.native("get [Symbol.toStringTag]", 0, tag_get);
    let tset = r.native("set [Symbol.toStringTag]", 1, tag_set);
    for (key, get, set) in [
        (PropKey::str("constructor"), cget, cset),
        (PropKey::Sym(r.sym_to_string_tag.clone()), tget, tset),
    ] {
        ip.borrow_mut().props.insert(
            key,
            Property {
                slot: Slot::Accessor { get: Some(get), set: Some(set) },
                enumerable: false,
                configurable: true,
            },
        );
    }
    r.method(ip, "map", 1, map);
    r.method(ip, "filter", 1, filter);
    r.method(ip, "take", 1, take);
    r.method(ip, "drop", 1, drop_items);
    r.method(ip, "flatMap", 1, flat_map);
    r.method(ip, "reduce", 1, reduce);
    r.method(ip, "toArray", 0, to_array);
    r.method(ip, "forEach", 1, for_each);
    r.method(ip, "some", 1, some);
    r.method(ip, "every", 1, every);
    r.method(ip, "find", 1, find);
    r.method(ip, "chunks", 1, chunks);
    r.method(ip, "windows", 1, windows);
    r.method(ip, "includes", 1, includes);
    r.method(ip, "join", 1, join);
    r.method_sym(ip, &r.sym_dispose, "[Symbol.dispose]", 0, dispose);
    r.method_sym(&r.async_iterator_proto, &r.sym_async_dispose, "[Symbol.asyncDispose]", 0, async_dispose);

    let hp = &r.iterator_helper_proto;
    r.method(hp, "next", 0, helper_next);
    r.method(hp, "return", 0, helper_return);
    r.tag(hp, "Iterator Helper");

    let wp = &r.wrap_for_valid_iter_proto;
    r.method(wp, "next", 0, wrap_next);
    r.method(wp, "return", 0, wrap_return);
}

// ------------------------------------------------------ operações do spec

/// GetMethod: None quando undefined/null; lança se não for função.
pub fn get_method(vm: &mut Vm, v: &Value, key: &PropKey) -> Result<Option<Value>, Value> {
    let f = vm.get(v, key)?;
    if f.is_nullish() {
        return Ok(None);
    }
    if !f.is_callable() {
        let d = vm.describe(&f);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    Ok(Some(f))
}

/// GetIteratorDirect.
pub fn get_iterator_direct(vm: &mut Vm, o: &Value) -> Result<IterRec, Value> {
    let next = vm.get_str(o, "next")?;
    Ok(IterRec {
        iter: o.clone(),
        next,
        done: false,
    })
}

/// GetIterator(v, sync): pelo método @@iterator.
pub fn get_iterator(vm: &mut Vm, v: &Value) -> Result<IterRec, Value> {
    let key = PropKey::Sym(vm.realm.sym_iterator.clone());
    let Some(m) = get_method(vm, v, &key)? else {
        let d = vm.describe(v);
        return Err(vm.type_error(format!("{d} is not iterable")));
    };
    let it = vm.call(&m, v.clone(), Vec::new())?;
    if !matches!(it, Value::Object(_)) {
        return Err(vm.type_error("Result of the Symbol.iterator method is not an object"));
    }
    get_iterator_direct(vm, &it)
}

/// GetIteratorFlattenable: aceita iterador ou iterável (strings só se `strings`).
fn get_iterator_flattenable(vm: &mut Vm, v: &Value, strings: bool, what: &str) -> Result<IterRec, Value> {
    match v {
        Value::Object(_) => {}
        Value::String(_) if strings => {}
        _ => return Err(vm.type_error(format!("{what} called on non-object"))),
    }
    let key = PropKey::Sym(vm.realm.sym_iterator.clone());
    let it = match get_method(vm, v, &key)? {
        None => v.clone(),
        Some(m) => vm.call(&m, v.clone(), Vec::new())?,
    };
    if !matches!(it, Value::Object(_)) {
        return Err(vm.type_error(format!("{what} called on non-object")));
    }
    get_iterator_direct(vm, &it)
}

/// IteratorClose com conclusão normal: os erros de `return` valem.
pub fn close_normal(vm: &mut Vm, iter: &Value) -> Result<(), Value> {
    let Some(f) = get_method(vm, iter, &PropKey::str("return"))? else { return Ok(()) };
    let r = vm.call(&f, iter.clone(), Vec::new())?;
    if !matches!(r, Value::Object(_)) {
        let d = vm.describe(&r);
        return Err(vm.type_error(format!("Iterator result {d} is not an object")));
    }
    Ok(())
}

/// IteratorClose com um erro: chama `return` (ignorando o que ele fizer) e
/// devolve o erro original.
pub fn close_with_error(vm: &mut Vm, iter: &Value, e: Value) -> Value {
    if let Ok(Some(f)) = get_method(vm, iter, &PropKey::str("return")) {
        let _ = vm.call(&f, iter.clone(), Vec::new());
    }
    e
}

/// IteratorClose(iter, conclusão).
pub fn close(vm: &mut Vm, iter: &Value, completion: Result<(), Value>) -> Result<(), Value> {
    match completion {
        Ok(()) => close_normal(vm, iter),
        Err(e) => Err(close_with_error(vm, iter, e)),
    }
}

/// IteratorCloseAll: fecha em ordem inversa, levando a conclusão adiante.
fn close_all(vm: &mut Vm, iters: &[Value], completion: Result<(), Value>) -> Result<(), Value> {
    let mut c = completion;
    for it in iters.iter().rev() {
        c = close(vm, it, c);
    }
    c
}

/// CreateDataPropertyOrThrow.
pub fn create_data_or_throw(vm: &mut Vm, o: &ObjRef, key: PropKey, v: Value) -> Result<(), Value> {
    if !define_own_property(vm, o, key.clone(), Descriptor::data(v, true))? {
        let k = crate::ops::key_display(&key);
        let msg = if super::proxy::is_proxy(o) {
            format!("'defineProperty' on proxy: trap returned falsish for property '{k}'")
        } else if o.borrow().own(&key).is_some() {
            format!("Cannot redefine property: {k}")
        } else {
            format!("Cannot define property {k}, object is not extensible")
        };
        return Err(vm.type_error(msg));
    }
    Ok(())
}

// --------------------------------------------------------- construtor

fn iterator_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = match &a.new_target {
        None => return Err(vm.type_error("Constructor Iterator requires 'new'")),
        Some(nt) if nt.ptr_eq(&a.callee) => return Err(vm.type_error("Abstract class Iterator not directly constructable")),
        Some(nt) => nt.clone(),
    };
    let def = vm.realm.iterator_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::Ordinary))))
}

fn ctor_get(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(super::native_data(&a.callee))
}

fn tag_get(_vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::str("Iterator"))
}

/// SetterThatIgnoresPrototypeProperties(this, %Iterator.prototype%, key, v).
fn setter_ignoring_proto(vm: &mut Vm, a: &CallArgs, key: PropKey, what: &str) -> Result<Value, Value> {
    let Value::Object(o) = &a.this else {
        return Err(vm.type_error(format!("set Iterator.prototype.{what} called on non-object")));
    };
    if o.ptr_eq(&vm.realm.iterator_proto) {
        return Err(vm.type_error(format!("Cannot assign to read only property '{}' of object '[object Iterator]'", crate::ops::key_display(&key))));
    }
    if vm.get_own_property(o, &key)?.is_none() {
        create_data_or_throw(vm, o, key, a.arg(0))?;
    } else {
        vm.put(&a.this, key, a.arg(0), true)?;
    }
    Ok(Value::Undefined)
}

fn ctor_set(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    setter_ignoring_proto(vm, a, PropKey::str("constructor"), "constructor")
}

fn tag_set(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let key = PropKey::Sym(vm.realm.sym_to_string_tag.clone());
    setter_ignoring_proto(vm, a, key, "[Symbol.toStringTag]")
}

/// Iterator.prototype[@@dispose]: chama `return` se houver.
fn dispose(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Some(f) = get_method(vm, &a.this, &PropKey::str("return"))? {
        vm.call(&f, a.this.clone(), Vec::new())?;
    }
    Ok(Value::Undefined)
}

/// %AsyncIteratorPrototype%[@@asyncDispose]: espera o `return` (se houver).
fn async_dispose(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cap = vm.new_capability(&Value::Undefined)?;
    let r = (|| -> Result<Option<ObjRef>, Value> {
        let Some(f) = get_method(vm, &a.this, &PropKey::str("return"))? else { return Ok(None) };
        let r = vm.call(&f, a.this.clone(), Vec::new())?;
        Ok(Some(vm.promise_resolve_value(r)?))
    })();
    match r {
        Ok(None) => {
            vm.call(&cap.resolve, Value::Undefined, vec![Value::Undefined])?;
        }
        Ok(Some(p)) => {
            let unwrap = vm.realm.native("", 1, return_undefined);
            vm.perform_then(&p, Value::Object(unwrap), Value::Undefined, Some(cap.clone()));
        }
        Err(e) => {
            vm.call(&cap.reject, Value::Undefined, vec![e])?;
        }
    }
    Ok(cap.promise)
}

fn return_undefined(_vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Undefined)
}

// ------------------------------------------------- Iterator.from e embrulho

fn from(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let rec = get_iterator_flattenable(vm, &a.arg(0), true, "Iterator.from")?;
    // OrdinaryHasInstance(%Iterator%, iter): o prototype de Iterator é fixo
    let Value::Object(it) = &rec.iter else { unreachable!() };
    let proto = vm.realm.iterator_proto.clone();
    let mut cur = vm.get_prototype_of(it)?;
    while let Some(p) = cur {
        if p.ptr_eq(&proto) {
            return Ok(rec.iter);
        }
        cur = vm.get_prototype_of(&p)?;
    }
    let wp = vm.realm.wrap_for_valid_iter_proto.clone();
    Ok(Value::Object(ObjRef::new(Object::new(Some(wp), Kind::IterWrap(Box::new(rec))))))
}

fn wrapped(vm: &mut Vm, a: &CallArgs, what: &str) -> Result<IterRec, Value> {
    if let Value::Object(o) = &a.this
        && let Kind::IterWrap(r) = &o.borrow().kind
    {
        return Ok((**r).clone());
    }
    let d = vm.describe(&a.this);
    Err(vm.type_error(format!("Method %WrapForValidIteratorPrototype%.{what} called on incompatible receiver {d}")))
}

fn wrap_next(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let rec = wrapped(vm, a, "next")?;
    vm.call(&rec.next, rec.iter, Vec::new())
}

fn wrap_return(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let rec = wrapped(vm, a, "return")?;
    match get_method(vm, &rec.iter, &PropKey::str("return"))? {
        None => Ok(iter_result(vm, Value::Undefined, true)),
        Some(f) => vm.call(&f, rec.iter, Vec::new()),
    }
}

// ------------------------------------------------ %IteratorHelperPrototype%

fn new_helper(vm: &mut Vm, op: Op, under: Option<IterRec>) -> Value {
    let body = Body {
        op,
        under,
        inner: None,
        counter: 0.0,
    };
    let h = Helper {
        state: GenState::Start,
        body: Some(Box::new(body)),
    };
    let proto = vm.realm.iterator_helper_proto.clone();
    Value::Object(ObjRef::new(Object::new(Some(proto), Kind::IterHelper(Box::new(h)))))
}

fn this_helper(vm: &mut Vm, a: &CallArgs, what: &str) -> Result<ObjRef, Value> {
    if let Value::Object(o) = &a.this
        && matches!(o.borrow().kind, Kind::IterHelper(_))
    {
        return Ok(o.clone());
    }
    let d = vm.describe(&a.this);
    Err(vm.type_error(format!("Method Iterator Helper.prototype.{what} called on incompatible receiver {d}")))
}

/// Tira o corpo do ajudante e marca como rodando (None se já acabou).
fn begin(vm: &mut Vm, h: &ObjRef) -> Result<Option<(GenState, Box<Body>)>, Value> {
    let mut b = h.borrow_mut();
    let Kind::IterHelper(d) = &mut b.kind else { return Ok(None) };
    match d.state {
        GenState::Running => {
            drop(b);
            Err(vm.type_error("Generator is already running"))
        }
        GenState::Done => Ok(None),
        s => {
            d.state = GenState::Running;
            Ok(d.body.take().map(|body| (s, body)))
        }
    }
}

fn finish(h: &ObjRef, body: Option<Box<Body>>) {
    if let Kind::IterHelper(d) = &mut h.borrow_mut().kind {
        d.state = if body.is_some() { GenState::Yield } else { GenState::Done };
        d.body = body;
    }
}

fn helper_next(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let h = this_helper(vm, a, "next")?;
    let Some((_, mut body)) = begin(vm, &h)? else {
        return Ok(iter_result(vm, Value::Undefined, true));
    };
    match step(vm, &mut body) {
        Ok(Some(v)) => {
            finish(&h, Some(body));
            Ok(iter_result(vm, v, false))
        }
        Ok(None) => {
            finish(&h, None);
            Ok(iter_result(vm, Value::Undefined, true))
        }
        Err(e) => {
            finish(&h, None);
            Err(e)
        }
    }
}

fn helper_return(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let h = this_helper(vm, a, "return")?;
    let Some((state, mut body)) = begin(vm, &h)? else {
        return Ok(iter_result(vm, Value::Undefined, true));
    };
    let r = if state == GenState::Start {
        // já fica "concluído" enquanto fecha os iteradores de baixo
        finish(&h, None);
        let iters: Vec<Value> = match &body.op {
            Op::Concat(..) => Vec::new(),
            Op::Zip(z) => rec_iters(&z.iters),
            _ => body.under.iter().map(|r| r.iter.clone()).collect(),
        };
        close_all(vm, &iters, Ok(()))
    } else {
        // GeneratorResumeAbrupt: fecha ainda "executando"
        let r = abort(vm, &mut body);
        finish(&h, None);
        r
    };
    r?;
    Ok(iter_result(vm, Value::Undefined, true))
}

/// `return()` com o ajudante parado num yield: fecha o que estiver aberto.
fn abort(vm: &mut Vm, b: &mut Body) -> Result<(), Value> {
    match &b.op {
        Op::FlatMap(_) => {
            let outer = b.under.as_ref().map(|r| r.iter.clone()).unwrap_or_default();
            if let Some(inner) = b.inner.take()
                && let Err(e) = close_normal(vm, &inner.iter)
            {
                return Err(close_with_error(vm, &outer, e));
            }
            close_normal(vm, &outer)
        }
        Op::Concat(..) => match b.inner.take() {
            Some(inner) => close_normal(vm, &inner.iter),
            None => Ok(()),
        },
        Op::Zip(z) => {
            let open = open_iters(z);
            close_all(vm, &open, Ok(()))
        }
        _ => match &b.under {
            Some(u) if !u.done => close_normal(vm, &u.iter.clone()),
            _ => Ok(()),
        },
    }
}

fn open_iters(z: &ZipData) -> Vec<Value> {
    z.iters.iter().zip(&z.open).filter(|(_, o)| **o).map(|(r, _)| r.iter.clone()).collect()
}

/// Avança o ajudante até o próximo valor (None = acabou).
fn step(vm: &mut Vm, b: &mut Body) -> Result<Option<Value>, Value> {
    let Body { op, under, inner, counter } = b;
    match op {
        Op::Map(f) => {
            let u = under.as_mut().expect("map sem iterador");
            let Some(v) = u.step_value(vm)? else { return Ok(None) };
            let r = vm.call(f, Value::Undefined, vec![v, Value::Number(*counter)]);
            *counter += 1.0;
            match r {
                Ok(m) => Ok(Some(m)),
                Err(e) => Err(close_with_error(vm, &u.iter, e)),
            }
        }
        Op::Filter(f) => {
            let u = under.as_mut().expect("filter sem iterador");
            loop {
                let Some(v) = u.step_value(vm)? else { return Ok(None) };
                let r = vm.call(f, Value::Undefined, vec![v.clone(), Value::Number(*counter)]);
                *counter += 1.0;
                match r {
                    Ok(sel) if truthy(&sel) => return Ok(Some(v)),
                    Ok(_) => {}
                    Err(e) => return Err(close_with_error(vm, &u.iter, e)),
                }
            }
        }
        Op::Take(remaining) => {
            let u = under.as_mut().expect("take sem iterador");
            if *remaining == 0.0 {
                u.done = true;
                close_normal(vm, &u.iter.clone())?;
                return Ok(None);
            }
            if *remaining != f64::INFINITY {
                *remaining -= 1.0;
            }
            u.step_value(vm)
        }
        Op::Drop(remaining) => {
            let u = under.as_mut().expect("drop sem iterador");
            while *remaining > 0.0 {
                if *remaining != f64::INFINITY {
                    *remaining -= 1.0;
                }
                if u.step(vm)?.is_none() {
                    return Ok(None);
                }
            }
            u.step_value(vm)
        }
        Op::FlatMap(f) => {
            let u = under.as_mut().expect("flatMap sem iterador");
            loop {
                if let Some(i) = inner {
                    match i.step_value(vm) {
                        Ok(Some(v)) => return Ok(Some(v)),
                        Ok(None) => {
                            *inner = None;
                            *counter += 1.0;
                        }
                        Err(e) => return Err(close_with_error(vm, &u.iter, e)),
                    }
                    continue;
                }
                let Some(v) = u.step_value(vm)? else { return Ok(None) };
                let mapped = match vm.call(f, Value::Undefined, vec![v, Value::Number(*counter)]) {
                    Ok(m) => m,
                    Err(e) => return Err(close_with_error(vm, &u.iter, e)),
                };
                match get_iterator_flattenable(vm, &mapped, false, "Iterator.prototype.flatMap") {
                    Ok(r) => *inner = Some(r),
                    Err(e) => return Err(close_with_error(vm, &u.iter, e)),
                }
            }
        }
        Op::Concat(items, index) => loop {
            if let Some(i) = inner {
                match i.step_value(vm)? {
                    Some(v) => return Ok(Some(v)),
                    None => *inner = None,
                }
                continue;
            }
            let Some((m, iterable)) = items.get(*index).cloned() else { return Ok(None) };
            *index += 1;
            let it = vm.call(&m, iterable, Vec::new())?;
            if !matches!(it, Value::Object(_)) {
                return Err(vm.type_error("Result of the Symbol.iterator method is not an object"));
            }
            *inner = Some(get_iterator_direct(vm, &it)?);
        },
        Op::Zip(z) => zip_step(vm, z),
        Op::Chunks(size, buf) => {
            let u = under.as_mut().expect("chunks sem iterador");
            loop {
                match u.step_value(vm)? {
                    None => {
                        if buf.is_empty() {
                            return Ok(None);
                        }
                        let v = std::mem::take(buf);
                        return Ok(Some(vm.array_value(v)));
                    }
                    Some(v) => {
                        buf.push(v);
                        if buf.len() == *size {
                            let v = std::mem::take(buf);
                            return Ok(Some(vm.array_value(v)));
                        }
                    }
                }
            }
        }
        Op::Windows(size, partial, buf) => {
            let u = under.as_mut().expect("windows sem iterador");
            loop {
                match u.step_value(vm)? {
                    None => {
                        if *partial && !buf.is_empty() && buf.len() < *size {
                            let v = std::mem::take(buf);
                            return Ok(Some(vm.array_value(v)));
                        }
                        return Ok(None);
                    }
                    Some(v) => {
                        if buf.len() == *size {
                            buf.remove(0);
                        }
                        buf.push(v);
                        if buf.len() == *size {
                            return Ok(Some(vm.array_value(buf.clone())));
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------- métodos de Iterator.prototype

/// `this` dos métodos de Iterator.prototype (precisa ser objeto).
fn this_iter(vm: &mut Vm, a: &CallArgs, name: &str) -> Result<Value, Value> {
    match &a.this {
        Value::Object(_) => Ok(a.this.clone()),
        _ => Err(vm.type_error(format!("Iterator.prototype.{name} called on non-object"))),
    }
}

/// Argumento que precisa ser função; senão fecha o iterador e lança.
fn callable_arg(vm: &mut Vm, o: &Value, f: &Value) -> Result<(), Value> {
    if f.is_callable() {
        return Ok(());
    }
    let d = vm.describe(f);
    let e = vm.type_error(format!("{d} is not a function"));
    Err(close_with_error(vm, o, e))
}

fn map(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "map")?;
    let f = a.arg(0);
    callable_arg(vm, &o, &f)?;
    let rec = get_iterator_direct(vm, &o)?;
    Ok(new_helper(vm, Op::Map(f), Some(rec)))
}

fn filter(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "filter")?;
    let f = a.arg(0);
    callable_arg(vm, &o, &f)?;
    let rec = get_iterator_direct(vm, &o)?;
    Ok(new_helper(vm, Op::Filter(f), Some(rec)))
}

fn flat_map(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "flatMap")?;
    let f = a.arg(0);
    callable_arg(vm, &o, &f)?;
    let rec = get_iterator_direct(vm, &o)?;
    Ok(new_helper(vm, Op::FlatMap(f), Some(rec)))
}

/// Limite de take/drop: inteiro entre 0 e 2^53-1 (ou infinito).
fn limit_arg(vm: &mut Vm, o: &Value, v: &Value) -> Result<f64, Value> {
    let n = match vm.to_number(v) {
        Ok(n) => n,
        Err(e) => return Err(close_with_error(vm, o, e)),
    };
    if n.is_nan() || (n.is_finite() && n > 9007199254740991.0) || crate::conv::to_integer(n) < 0.0 {
        let e = vm.range_error(format!("{} must be positive", crate::conv::number_to_string(n)));
        return Err(close_with_error(vm, o, e));
    }
    Ok(crate::conv::to_integer(n))
}

fn take(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "take")?;
    let n = limit_arg(vm, &o, &a.arg(0))?;
    let rec = get_iterator_direct(vm, &o)?;
    Ok(new_helper(vm, Op::Take(n), Some(rec)))
}

fn drop_items(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "drop")?;
    let n = limit_arg(vm, &o, &a.arg(0))?;
    let rec = get_iterator_direct(vm, &o)?;
    Ok(new_helper(vm, Op::Drop(n), Some(rec)))
}

/// Tamanho de chunks/windows: inteiro entre 1 e 2^32-1, sem conversão.
fn size_arg(vm: &mut Vm, o: &Value, v: &Value) -> Result<usize, Value> {
    let e = match v {
        Value::Number(n) if n.is_finite() && n.fract() == 0.0 => {
            if *n >= 1.0 && *n <= 4294967295.0 {
                return Ok(*n as usize);
            }
            vm.range_error(format!("{} must be positive", crate::conv::number_to_string(*n)))
        }
        _ => {
            let d = vm.describe(v);
            vm.type_error(format!("{d} is not an integral number"))
        }
    };
    Err(close_with_error(vm, o, e))
}

fn chunks(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "chunks")?;
    let size = size_arg(vm, &o, &a.arg(0))?;
    let rec = get_iterator_direct(vm, &o)?;
    Ok(new_helper(vm, Op::Chunks(size, Vec::new()), Some(rec)))
}

fn windows(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "windows")?;
    let size = size_arg(vm, &o, &a.arg(0))?;
    let partial = match a.arg(1) {
        Value::Undefined => false,
        Value::String(s) if s.eq_str("only-full") => false,
        Value::String(s) if s.eq_str("allow-partial") => true,
        other => {
            let d = vm.describe(&other);
            let e = vm.type_error(format!("{d} is not a valid value for undersized"));
            return Err(close_with_error(vm, &o, e));
        }
    };
    let rec = get_iterator_direct(vm, &o)?;
    Ok(new_helper(vm, Op::Windows(size, partial, Vec::new()), Some(rec)))
}

fn reduce(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "reduce")?;
    let f = a.arg(0);
    callable_arg(vm, &o, &f)?;
    let mut rec = get_iterator_direct(vm, &o)?;
    let (mut acc, mut counter) = if a.args.len() < 2 {
        match rec.step_value(vm)? {
            Some(v) => (v, 1.0),
            None => return Err(vm.type_error("Reduce of a done iterator with no initial value")),
        }
    } else {
        (a.arg(1), 0.0)
    };
    while let Some(v) = rec.step_value(vm)? {
        match vm.call(&f, Value::Undefined, vec![acc, v, Value::Number(counter)]) {
            Ok(r) => acc = r,
            Err(e) => return Err(close_with_error(vm, &o, e)),
        }
        counter += 1.0;
    }
    Ok(acc)
}

fn to_array(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "toArray")?;
    let mut rec = get_iterator_direct(vm, &o)?;
    let mut items = Vec::new();
    while let Some(v) = rec.step_value(vm)? {
        items.push(v);
    }
    Ok(vm.array_value(items))
}

fn for_each(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "forEach")?;
    let f = a.arg(0);
    callable_arg(vm, &o, &f)?;
    let mut rec = get_iterator_direct(vm, &o)?;
    let mut counter = 0.0;
    while let Some(v) = rec.step_value(vm)? {
        if let Err(e) = vm.call(&f, Value::Undefined, vec![v, Value::Number(counter)]) {
            return Err(close_with_error(vm, &o, e));
        }
        counter += 1.0;
    }
    Ok(Value::Undefined)
}

/// some/every/find: 0 = some, 1 = every, 2 = find.
fn search(vm: &mut Vm, a: &CallArgs, name: &str, kind: u8) -> Result<Value, Value> {
    let o = this_iter(vm, a, name)?;
    let f = a.arg(0);
    callable_arg(vm, &o, &f)?;
    let mut rec = get_iterator_direct(vm, &o)?;
    let mut counter = 0.0;
    while let Some(v) = rec.step_value(vm)? {
        let r = match vm.call(&f, Value::Undefined, vec![v.clone(), Value::Number(counter)]) {
            Ok(r) => truthy(&r),
            Err(e) => return Err(close_with_error(vm, &o, e)),
        };
        let found = match kind {
            0 if r => Some(Value::Bool(true)),
            1 if !r => Some(Value::Bool(false)),
            2 if r => Some(v),
            _ => None,
        };
        if let Some(out) = found {
            close_normal(vm, &o)?;
            return Ok(out);
        }
        counter += 1.0;
    }
    Ok(match kind {
        0 => Value::Bool(false),
        1 => Value::Bool(true),
        _ => Value::Undefined,
    })
}

fn some(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    search(vm, a, "some", 0)
}

fn every(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    search(vm, a, "every", 1)
}

fn find(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    search(vm, a, "find", 2)
}

fn includes(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "includes")?;
    let target = a.arg(0);
    let to_skip = match a.arg(1) {
        Value::Undefined => 0.0,
        Value::Number(n) if n.is_infinite() || (n.is_finite() && n.fract() == 0.0) => n,
        other => {
            let d = vm.describe(&other);
            let e = vm.type_error(format!("{d} is not an integral number"));
            return Err(close_with_error(vm, &o, e));
        }
    };
    if to_skip < 0.0 || (to_skip.is_finite() && to_skip > 9007199254740991.0) {
        let e = vm.range_error(format!("{} is out of range", crate::conv::number_to_string(to_skip)));
        return Err(close_with_error(vm, &o, e));
    }
    let mut rec = get_iterator_direct(vm, &o)?;
    let mut skipped = 0.0;
    while let Some(v) = rec.step_value(vm)? {
        if skipped < to_skip {
            skipped += 1.0;
        } else if same_value_zero(&v, &target) {
            close_normal(vm, &o)?;
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn join(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_iter(vm, a, "join")?;
    let sep = match a.arg(0) {
        Value::Undefined => JsStr::new(","),
        s => match vm.to_string(&s) {
            Ok(s) => s,
            Err(e) => return Err(close_with_error(vm, &o, e)),
        },
    };
    let mut rec = get_iterator_direct(vm, &o)?;
    let mut out: Vec<u16> = Vec::new();
    let mut first = true;
    while let Some(v) = rec.step_value(vm)? {
        if !first {
            out.extend_from_slice(sep.units());
        }
        first = false;
        if v.is_nullish() {
            continue;
        }
        match vm.to_string(&v) {
            Ok(s) => out.extend_from_slice(s.units()),
            Err(e) => return Err(close_with_error(vm, &o, e)),
        }
    }
    Ok(Value::String(JsStr::from_units(out)))
}

// ------------------------------------------------------- Iterator.concat

fn concat(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let mut items = Vec::with_capacity(a.args.len());
    let key = PropKey::Sym(vm.realm.sym_iterator.clone());
    for item in a.args.clone() {
        if !matches!(item, Value::Object(_)) {
            let d = vm.describe(&item);
            return Err(vm.type_error(format!("{d} is not an object")));
        }
        let Some(m) = get_method(vm, &item, &key)? else {
            let d = vm.describe(&item);
            return Err(vm.type_error(format!("{d} is not iterable")));
        };
        items.push((m, item));
    }
    Ok(new_helper(vm, Op::Concat(items, 0), None))
}

// ------------------------------------------------- Iterator.zip/zipKeyed

/// Opções comuns de zip/zipKeyed: modo e o objeto de padding (se houver).
fn zip_options(vm: &mut Vm, options: &Value) -> Result<(ZipMode, Value), Value> {
    let opts = match options {
        Value::Undefined => return Ok((ZipMode::Shortest, Value::Undefined)),
        Value::Object(_) => options.clone(),
        _ => {
            let d = vm.describe(options);
            return Err(vm.type_error(format!("{d} is not an object")));
        }
    };
    let mode = match vm.get_str(&opts, "mode")? {
        Value::Undefined => ZipMode::Shortest,
        Value::String(s) if s.eq_str("shortest") => ZipMode::Shortest,
        Value::String(s) if s.eq_str("longest") => ZipMode::Longest,
        Value::String(s) if s.eq_str("strict") => ZipMode::Strict,
        other => {
            let d = vm.describe(&other);
            return Err(vm.type_error(format!("{d} is not a valid mode")));
        }
    };
    let mut padding = Value::Undefined;
    if mode == ZipMode::Longest {
        padding = vm.get_str(&opts, "padding")?;
        if !matches!(padding, Value::Undefined | Value::Object(_)) {
            let d = vm.describe(&padding);
            return Err(vm.type_error(format!("{d} is not an object")));
        }
    }
    Ok((mode, padding))
}

fn rec_iters(recs: &[IterRec]) -> Vec<Value> {
    recs.iter().map(|r| r.iter.clone()).collect()
}

fn zip(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let iterables = a.arg(0);
    if !matches!(iterables, Value::Object(_)) {
        let d = vm.describe(&iterables);
        return Err(vm.type_error(format!("{d} is not an object")));
    }
    let (mode, pad_opt) = zip_options(vm, &a.arg(1))?;
    let mut iters: Vec<IterRec> = Vec::new();
    let mut input = get_iterator(vm, &iterables)?;
    loop {
        let next = match input.step_value(vm) {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(e) => return Err(close_all(vm, &rec_iters(&iters), Err(e)).unwrap_err()),
        };
        match get_iterator_flattenable(vm, &next, false, "Iterator.zip") {
            Ok(r) => iters.push(r),
            Err(e) => {
                let mut all = vec![input.iter.clone()];
                all.extend(rec_iters(&iters));
                return Err(close_all(vm, &all, Err(e)).unwrap_err());
            }
        }
    }
    let n = iters.len();
    let mut padding = Vec::with_capacity(n);
    if mode == ZipMode::Longest {
        if pad_opt.is_undefined() {
            padding.resize(n, Value::Undefined);
        } else {
            let mut pi = match get_iterator(vm, &pad_opt) {
                Ok(r) => r,
                Err(e) => return Err(close_all(vm, &rec_iters(&iters), Err(e)).unwrap_err()),
            };
            let mut using = true;
            for _ in 0..n {
                if using {
                    match pi.step_value(vm) {
                        Ok(Some(v)) => padding.push(v),
                        Ok(None) => using = false,
                        Err(e) => return Err(close_all(vm, &rec_iters(&iters), Err(e)).unwrap_err()),
                    }
                }
                if !using {
                    padding.push(Value::Undefined);
                }
            }
            if using && let Err(e) = close_normal(vm, &pi.iter) {
                return Err(close_all(vm, &rec_iters(&iters), Err(e)).unwrap_err());
            }
        }
    }
    Ok(new_zip(vm, iters, mode, padding, None))
}

fn zip_keyed(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let iterables = a.arg(0);
    let Value::Object(src) = &iterables else {
        let d = vm.describe(&iterables);
        return Err(vm.type_error(format!("{d} is not an object")));
    };
    let (mode, pad_opt) = zip_options(vm, &a.arg(1))?;
    let mut iters: Vec<IterRec> = Vec::new();
    let mut keys = Vec::new();
    let all_keys = vm.own_keys(src)?;
    for key in all_keys {
        let r = (|| -> Result<Option<IterRec>, Value> {
            let Some(desc) = vm.get_own_property(src, &key)? else { return Ok(None) };
            if !desc.enumerable {
                return Ok(None);
            }
            let v = vm.get(&iterables, &key)?;
            if v.is_undefined() {
                return Ok(None);
            }
            Ok(Some(get_iterator_flattenable(vm, &v, false, "Iterator.zipKeyed")?))
        })();
        match r {
            Ok(Some(rec)) => {
                keys.push(key);
                iters.push(rec);
            }
            Ok(None) => {}
            Err(e) => return Err(close_all(vm, &rec_iters(&iters), Err(e)).unwrap_err()),
        }
    }
    let n = iters.len();
    let mut padding = Vec::with_capacity(n);
    if mode == ZipMode::Longest {
        if pad_opt.is_undefined() {
            padding.resize(n, Value::Undefined);
        } else {
            for key in &keys {
                match vm.get(&pad_opt, key) {
                    Ok(v) => padding.push(v),
                    Err(e) => return Err(close_all(vm, &rec_iters(&iters), Err(e)).unwrap_err()),
                }
            }
        }
    }
    Ok(new_zip(vm, iters, mode, padding, Some(keys)))
}

fn new_zip(vm: &mut Vm, iters: Vec<IterRec>, mode: ZipMode, padding: Vec<Value>, keys: Option<Vec<PropKey>>) -> Value {
    let open = vec![true; iters.len()];
    let z = ZipData {
        iters,
        open,
        mode,
        padding,
        keys,
    };
    new_helper(vm, Op::Zip(Box::new(z)), None)
}

/// Fecha os abertos com um erro novo e devolve o erro.
fn zip_fail(vm: &mut Vm, z: &ZipData, e: Value) -> Value {
    let open = open_iters(z);
    close_all(vm, &open, Err(e)).unwrap_err()
}

/// Um passo de IteratorZip.
fn zip_step(vm: &mut Vm, z: &mut ZipData) -> Result<Option<Value>, Value> {
    let n = z.iters.len();
    if n == 0 {
        return Ok(None);
    }
    let mut results = Vec::with_capacity(n);
    for i in 0..n {
        let result = if !z.open[i] {
            z.padding[i].clone()
        } else {
            match z.iters[i].step_value(vm) {
                Err(e) => {
                    z.open[i] = false;
                    return Err(zip_fail(vm, z, e));
                }
                Ok(Some(v)) => v,
                Ok(None) => {
                    z.open[i] = false;
                    match z.mode {
                        ZipMode::Shortest => {
                            let open = open_iters(z);
                            close_all(vm, &open, Ok(()))?;
                            return Ok(None);
                        }
                        ZipMode::Strict => {
                            if i != 0 {
                                let e = vm.type_error("Iterators passed to Iterator.zip have different lengths");
                                return Err(zip_fail(vm, z, e));
                            }
                            for k in 1..n {
                                match z.iters[k].step(vm) {
                                    Err(e) => {
                                        z.open[k] = false;
                                        return Err(zip_fail(vm, z, e));
                                    }
                                    Ok(None) => z.open[k] = false,
                                    Ok(Some(_)) => {
                                        let e = vm.type_error("Iterators passed to Iterator.zip have different lengths");
                                        return Err(zip_fail(vm, z, e));
                                    }
                                }
                            }
                            return Ok(None);
                        }
                        ZipMode::Longest => {
                            if !z.open.iter().any(|o| *o) {
                                return Ok(None);
                            }
                            z.padding[i].clone()
                        }
                    }
                }
            }
        };
        results.push(result);
    }
    Ok(Some(match &z.keys {
        None => vm.array_value(results),
        Some(keys) => {
            let o = ObjRef::new(Object::new(None, Kind::Ordinary));
            for (k, v) in keys.iter().zip(results) {
                vm.define_data(&o, k.clone(), v);
            }
            Value::Object(o)
        }
    }))
}
