//! Gerenciamento explícito de recursos: DisposableStack, AsyncDisposableStack
//! e SuppressedError (os erros juntados durante o descarte).
//!
//! O descarte assíncrono (DisposeResources com Await) é uma máquina de
//! estados, como Array.fromAsync: pausa em cada Await e continua quando a
//! promessa se resolve.

use super::promise::Capability;
use super::{Realm, native_data, set_native_data};
use crate::value::*;
use crate::vm::Vm;

/// Um recurso na pilha: valor, método de descarte (undefined = nenhum) e se
/// o descarte é assíncrono.
#[derive(Clone)]
pub struct Resource {
    pub value: Value,
    pub method: Value,
    pub is_async: bool,
}

pub struct StackData {
    pub is_async: bool,
    pub disposed: bool,
    pub resources: Vec<Resource>,
}

impl StackData {
    fn new(is_async: bool) -> StackData {
        StackData {
            is_async,
            disposed: false,
            resources: Vec::new(),
        }
    }

    pub fn trace(&self, f: &mut dyn FnMut(&Value)) {
        for r in &self.resources {
            f(&r.value);
            f(&r.method);
        }
    }
}

/// Onde o descarte assíncrono parou.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Loop,
    /// Esperando o resultado do método assíncrono do recurso do topo.
    AwaitResult,
    /// Await(undefined) antes de um recurso síncrono.
    AwaitBeforeSync,
    /// Await(undefined) no fim.
    AwaitFinal,
}

/// DisposeResources assíncrono em andamento.
pub struct DisposeJob {
    pub cap: Capability,
    pub resources: Vec<Resource>,
    /// Conclusão acumulada (Err = já houve erro).
    pub completion: Result<(), Value>,
    pub needs_await: bool,
    pub has_awaited: bool,
    pub phase: Phase,
}

impl DisposeJob {
    pub fn trace(&self, f: &mut dyn FnMut(&Value)) {
        for v in [&self.cap.promise, &self.cap.resolve, &self.cap.reject] {
            f(v);
        }
        for r in &self.resources {
            f(&r.value);
            f(&r.method);
        }
        if let Err(e) = &self.completion {
            f(e);
        }
    }
}

pub fn install(r: &Realm) {
    for is_async in [false, true] {
        let name = if is_async { "AsyncDisposableStack" } else { "DisposableStack" };
        let proto = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
        let c = r.ctor(name, 0, if is_async { async_stack_ctor } else { stack_ctor }, &proto);
        // o construtor guarda o protótipo original (para move())
        set_native_data(&c, Value::Object(proto.clone()));
        r.getter(&proto, "disposed", if is_async { async_disposed } else { disposed });
        r.method(&proto, "use", 1, if is_async { async_use } else { use_ });
        r.method(&proto, "adopt", 2, if is_async { async_adopt } else { adopt });
        r.method(&proto, "defer", 1, if is_async { async_defer } else { defer });
        let mv = r.method(&proto, "move", 0, if is_async { async_move } else { move_ });
        set_native_data(&mv, Value::Object(proto.clone()));
        let (dname, sym) = if is_async { ("disposeAsync", &r.sym_async_dispose) } else { ("dispose", &r.sym_dispose) };
        let d = r.method(&proto, dname, 0, if is_async { dispose_async } else { dispose });
        proto.borrow_mut().props.insert(PropKey::Sym(sym.clone()), Property::hidden(Value::Object(d)));
        r.tag(&proto, name);
    }
}

// ------------------------------------------------------------ auxiliares

fn new_stack(vm: &mut Vm, a: &CallArgs, is_async: bool) -> Result<Value, Value> {
    let name = if is_async { "AsyncDisposableStack" } else { "DisposableStack" };
    let Some(nt) = a.new_target.clone() else {
        return Err(vm.type_error(format!("Constructor {name} requires 'new'")));
    };
    let def = match native_data(&a.callee) {
        Value::Object(p) => p,
        _ => vm.realm.object_proto.clone(),
    };
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let kind = Kind::DisposableStack(Box::new(StackData::new(is_async)));
    Ok(Value::Object(ObjRef::new(Object::new(Some(proto), kind))))
}

fn stack_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    new_stack(vm, a, false)
}

fn async_stack_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    new_stack(vm, a, true)
}

/// `this` como pilha do tipo certo (RequireInternalSlot).
fn this_stack(vm: &mut Vm, a: &CallArgs, is_async: bool, what: &str) -> Result<ObjRef, Value> {
    if let Value::Object(o) = &a.this
        && matches!(&o.borrow().kind, Kind::DisposableStack(d) if d.is_async == is_async)
    {
        return Ok(o.clone());
    }
    let class = if is_async { "AsyncDisposableStack" } else { "DisposableStack" };
    let d = vm.describe(&a.this);
    Err(vm.type_error(format!("Method {class}.prototype.{what} called on incompatible receiver {d}")))
}

fn with_data<R>(o: &ObjRef, f: impl FnOnce(&mut StackData) -> R) -> Option<R> {
    match &mut o.borrow_mut().kind {
        Kind::DisposableStack(d) => Some(f(d)),
        _ => None,
    }
}

/// Pilha ainda aberta (senão ReferenceError).
fn open_stack(vm: &mut Vm, a: &CallArgs, is_async: bool, what: &str) -> Result<ObjRef, Value> {
    let o = this_stack(vm, a, is_async, what)?;
    if with_data(&o, |d| d.disposed).unwrap_or(true) {
        let class = if is_async { "AsyncDisposableStack" } else { "DisposableStack" };
        return Err(vm.reference_error(format!("Cannot call {class}.prototype.{what} on an already-disposed {class}")));
    }
    Ok(o)
}

fn push(o: &ObjRef, r: Resource) {
    with_data(o, |d| d.resources.push(r));
}

fn get_method(vm: &mut Vm, v: &Value, s: &Sym) -> Result<Option<Value>, Value> {
    super::iterator::get_method(vm, v, &PropKey::Sym(s.clone()))
}

/// CreateDisposableResource(V, hint) sem método: procura @@dispose
/// (ou @@asyncDispose, caindo para @@dispose embrulhado).
fn resource_for(vm: &mut Vm, v: &Value, is_async: bool) -> Result<Resource, Value> {
    if v.is_nullish() {
        return Ok(Resource {
            value: Value::Undefined,
            method: Value::Undefined,
            is_async,
        });
    }
    if !matches!(v, Value::Object(_)) {
        let d = vm.describe(v);
        return Err(vm.type_error(format!("{d} is not an object")));
    }
    let sync_sym = vm.realm.sym_dispose.clone();
    let method = if is_async {
        let async_sym = vm.realm.sym_async_dispose.clone();
        match get_method(vm, v, &async_sym)? {
            Some(m) => Some(m),
            None => get_method(vm, v, &sync_sym)?.map(|m| {
                let f = vm.realm.native("", 0, sync_dispose_as_async);
                set_native_data(&f, m);
                Value::Object(f)
            }),
        }
    } else {
        get_method(vm, v, &sync_sym)?
    };
    let Some(method) = method else {
        let what = if is_async { "Symbol.asyncDispose" } else { "Symbol.dispose" };
        return Err(vm.type_error(format!("Object does not have a [{what}] method")));
    };
    Ok(Resource {
        value: v.clone(),
        method,
        is_async,
    })
}

/// @@dispose síncrono usado num descarte assíncrono: devolve uma promessa.
fn sync_dispose_as_async(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cap = vm.new_capability(&Value::Undefined)?;
    match vm.call(&native_data(&a.callee), a.this.clone(), Vec::new()) {
        Ok(_) => vm.call(&cap.resolve, Value::Undefined, vec![Value::Undefined])?,
        Err(e) => vm.call(&cap.reject, Value::Undefined, vec![e])?,
    };
    Ok(cap.promise)
}

/// Junta um novo erro ao que já havia (SuppressedError).
fn combine(vm: &mut Vm, completion: Result<(), Value>, e: Value) -> Result<(), Value> {
    match completion {
        Ok(()) => Err(e),
        Err(prev) => Err(suppressed_error(vm, e, prev)),
    }
}

/// Um SuppressedError novo com `error` e `suppressed`.
pub fn suppressed_error(vm: &mut Vm, error: Value, suppressed: Value) -> Value {
    let proto = vm.realm.suppressed_error_proto.clone();
    let o = ObjRef::new(Object::new(Some(proto), Kind::Error));
    {
        let mut b = o.borrow_mut();
        b.props.insert(PropKey::str("error"), Property::hidden(error));
        b.props.insert(PropKey::str("suppressed"), Property::hidden(suppressed));
    }
    let stack = vm.stack_trace("SuppressedError");
    o.borrow_mut().props.insert(PropKey::str("stack"), Property::hidden(Value::str(&stack)));
    Value::Object(o)
}

// ------------------------------------------------------- DisposableStack

fn disposed(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_stack(vm, a, false, "disposed")?;
    Ok(Value::Bool(with_data(&o, |d| d.disposed).unwrap_or(false)))
}

fn use_(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    stack_use(vm, a, false)
}

fn stack_use(vm: &mut Vm, a: &CallArgs, is_async: bool) -> Result<Value, Value> {
    let o = open_stack(vm, a, is_async, "use")?;
    let v = a.arg(0);
    // undefined/null num descarte síncrono não entra na pilha
    if v.is_nullish() && !is_async {
        return Ok(v);
    }
    let r = resource_for(vm, &v, is_async)?;
    push(&o, r);
    Ok(v)
}

fn adopt(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    stack_adopt(vm, a, false)
}

fn stack_adopt(vm: &mut Vm, a: &CallArgs, is_async: bool) -> Result<Value, Value> {
    let o = open_stack(vm, a, is_async, "adopt")?;
    let on_dispose = a.arg(1);
    if !on_dispose.is_callable() {
        let d = vm.describe(&on_dispose);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    let v = a.arg(0);
    // F() = onDispose(value)
    let f = vm.realm.native("", 0, adopt_closure);
    let data = vm.array_value(vec![on_dispose, v.clone()]);
    set_native_data(&f, data);
    push(
        &o,
        Resource {
            value: Value::Undefined,
            method: Value::Object(f),
            is_async,
        },
    );
    Ok(v)
}

fn adopt_closure(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let data = super::function::list_from_array_like(vm, &native_data(&a.callee))?;
    let (f, v) = (data.first().cloned().unwrap_or_default(), data.get(1).cloned().unwrap_or_default());
    vm.call(&f, Value::Undefined, vec![v])
}

fn defer(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    stack_defer(vm, a, false)
}

fn stack_defer(vm: &mut Vm, a: &CallArgs, is_async: bool) -> Result<Value, Value> {
    let o = open_stack(vm, a, is_async, "defer")?;
    let on_dispose = a.arg(0);
    if !on_dispose.is_callable() {
        let d = vm.describe(&on_dispose);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    push(
        &o,
        Resource {
            value: Value::Undefined,
            method: on_dispose,
            is_async,
        },
    );
    Ok(Value::Undefined)
}

fn move_(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    stack_move(vm, a, false)
}

/// move(): uma pilha nova (do protótipo original) leva os recursos.
fn stack_move(vm: &mut Vm, a: &CallArgs, is_async: bool) -> Result<Value, Value> {
    let o = open_stack(vm, a, is_async, "move")?;
    let proto = match native_data(&a.callee) {
        Value::Object(p) => p,
        _ => vm.realm.object_proto.clone(),
    };
    let resources = with_data(&o, |d| {
        d.disposed = true;
        std::mem::take(&mut d.resources)
    })
    .unwrap_or_default();
    let mut data = StackData::new(is_async);
    data.resources = resources;
    Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::DisposableStack(Box::new(data))))))
}

fn dispose(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_stack(vm, a, false, "dispose")?;
    let Some(mut resources) = with_data(&o, |d| {
        if d.disposed {
            return None;
        }
        d.disposed = true;
        Some(std::mem::take(&mut d.resources))
    })
    .flatten() else {
        return Ok(Value::Undefined);
    };
    let mut completion = Ok(());
    while let Some(r) = resources.pop() {
        if r.method.is_undefined() {
            continue;
        }
        if let Err(e) = vm.call(&r.method, r.value.clone(), Vec::new()) {
            completion = combine(vm, completion, e);
        }
    }
    completion.map(|_| Value::Undefined)
}

// -------------------------------------------------- AsyncDisposableStack

fn async_disposed(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = this_stack(vm, a, true, "disposed")?;
    Ok(Value::Bool(with_data(&o, |d| d.disposed).unwrap_or(false)))
}

fn async_use(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    stack_use(vm, a, true)
}

fn async_adopt(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    stack_adopt(vm, a, true)
}

fn async_defer(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    stack_defer(vm, a, true)
}

fn async_move(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    stack_move(vm, a, true)
}

fn dispose_async(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let cap = vm.new_capability(&Value::Undefined)?;
    let o = match this_stack(vm, a, true, "disposeAsync") {
        Ok(o) => o,
        Err(e) => {
            vm.call(&cap.reject, Value::Undefined, vec![e])?;
            return Ok(cap.promise);
        }
    };
    let resources = with_data(&o, |d| {
        if d.disposed {
            return None;
        }
        d.disposed = true;
        Some(std::mem::take(&mut d.resources))
    })
    .flatten();
    let Some(resources) = resources else {
        vm.call(&cap.resolve, Value::Undefined, vec![Value::Undefined])?;
        return Ok(cap.promise);
    };
    let job = DisposeJob {
        cap: cap.clone(),
        resources,
        completion: Ok(()),
        needs_await: false,
        has_awaited: false,
        phase: Phase::Loop,
    };
    let st = ObjRef::new(Object::new(None, Kind::DisposeJob(Some(Box::new(job)))));
    drive(vm, &st, None);
    Ok(cap.promise)
}

enum Step {
    Await(Value),
    Done(Result<(), Value>),
}

/// Roda o descarte até o próximo Await (ou até o fim).
fn drive(vm: &mut Vm, st: &ObjRef, input: Option<Result<Value, Value>>) {
    let Some(mut job) = (match &mut st.borrow_mut().kind {
        Kind::DisposeJob(j) => j.take(),
        _ => None,
    }) else {
        return;
    };
    let mut input = input;
    loop {
        match advance(vm, &mut job, input.take()) {
            Step::Await(v) => match vm.promise_resolve_value(v) {
                Ok(p) => {
                    let on_ful = vm.realm.native("", 1, resumed_ok);
                    let on_rej = vm.realm.native("", 1, resumed_err);
                    set_native_data(&on_ful, Value::Object(st.clone()));
                    set_native_data(&on_rej, Value::Object(st.clone()));
                    if let Kind::DisposeJob(slot) = &mut st.borrow_mut().kind {
                        *slot = Some(job);
                    }
                    vm.perform_then(&p, Value::Object(on_ful), Value::Object(on_rej), None);
                    return;
                }
                Err(e) => input = Some(Err(e)),
            },
            Step::Done(r) => {
                let (f, v) = match r {
                    Ok(()) => (job.cap.resolve.clone(), Value::Undefined),
                    Err(e) => (job.cap.reject.clone(), e),
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

/// DisposeResources com Await, a partir da fase atual.
fn advance(vm: &mut Vm, j: &mut DisposeJob, input: Option<Result<Value, Value>>) -> Step {
    match j.phase {
        Phase::AwaitResult => {
            j.has_awaited = true;
            if let Some(Err(e)) = input {
                j.completion = combine(vm, std::mem::replace(&mut j.completion, Ok(())), e);
            }
        }
        Phase::AwaitBeforeSync => j.needs_await = false,
        Phase::AwaitFinal => return Step::Done(std::mem::replace(&mut j.completion, Ok(()))),
        Phase::Loop => {}
    }
    j.phase = Phase::Loop;
    loop {
        let Some(r) = j.resources.last().cloned() else {
            if j.needs_await && !j.has_awaited {
                j.phase = Phase::AwaitFinal;
                return Step::Await(Value::Undefined);
            }
            return Step::Done(std::mem::replace(&mut j.completion, Ok(())));
        };
        if !r.is_async && j.needs_await && !j.has_awaited {
            j.phase = Phase::AwaitBeforeSync;
            return Step::Await(Value::Undefined);
        }
        j.resources.pop();
        if r.method.is_undefined() {
            j.needs_await = true;
            continue;
        }
        match vm.call(&r.method, r.value.clone(), Vec::new()) {
            Ok(v) if r.is_async => {
                j.phase = Phase::AwaitResult;
                return Step::Await(v);
            }
            Ok(_) => {}
            Err(e) => j.completion = combine(vm, std::mem::replace(&mut j.completion, Ok(())), e),
        }
    }
}
