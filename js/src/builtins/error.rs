//! Error e seus subtipos (TypeError, RangeError, ...).

use super::{Realm, native_data, set_native_data};
use crate::ops::data_in_chain;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let base = make(r, "Error", &r.error_proto, None);
    r.method(&r.error_proto, "toString", 0, to_string);
    r.method(&base, "captureStackTrace", 1, capture_stack_trace);
    base.borrow_mut().props.insert(PropKey::str("stackTraceLimit"), Property::data(Value::Number(10.0)));
    for (name, proto) in [
        ("TypeError", &r.type_error_proto),
        ("RangeError", &r.range_error_proto),
        ("ReferenceError", &r.reference_error_proto),
        ("SyntaxError", &r.syntax_error_proto),
        ("EvalError", &r.eval_error_proto),
        ("URIError", &r.uri_error_proto),
    ] {
        make(r, name, proto, Some(&base));
    }
    let agg_proto = ObjRef::new(Object::new(Some(r.error_proto.clone()), Kind::Ordinary));
    let agg = r.ctor("AggregateError", 2, aggregate_ctor, &agg_proto);
    agg.borrow_mut().proto = Some(base);
    set_native_data(&agg, Value::Object(agg_proto.clone()));
    agg_proto.borrow_mut().props.insert(PropKey::str("name"), Property::hidden(Value::str("AggregateError")));
    agg_proto.borrow_mut().props.insert(PropKey::str("message"), Property::hidden(Value::str("")));
}

fn make(r: &Realm, name: &str, proto: &ObjRef, parent: Option<&ObjRef>) -> ObjRef {
    let c = r.ctor(name, 1, error_ctor, proto);
    set_native_data(&c, Value::Object(proto.clone()));
    if let Some(p) = parent {
        c.borrow_mut().proto = Some(p.clone());
    }
    proto.borrow_mut().props.insert(PropKey::str("name"), Property::hidden(Value::str(name)));
    proto.borrow_mut().props.insert(PropKey::str("message"), Property::hidden(Value::str("")));
    c
}

/// Cria o objeto de erro com mensagem, causa e pilha.
fn build(vm: &mut Vm, a: &CallArgs, msg_index: usize) -> Result<ObjRef, Value> {
    let default = match native_data(&a.callee) {
        Value::Object(p) => p,
        _ => vm.realm.error_proto.clone(),
    };
    let nt = a.new_target.clone().unwrap_or_else(|| a.callee.clone());
    let proto = vm.proto_from_ctor(Some(&nt), &default)?;
    let o = ObjRef::new(Object::new(Some(proto.clone()), Kind::Error));
    let msg = a.arg(msg_index);
    let mut msg_text = String::new();
    if !msg.is_undefined() {
        let s = vm.to_string(&msg)?;
        msg_text = s.to_rust();
        o.borrow_mut().props.insert(PropKey::str("message"), Property::hidden(Value::String(s)));
    }
    let opts = a.arg(msg_index + 1);
    if let Value::Object(oo) = &opts
        && vm.has_property(oo, &PropKey::str("cause"))?
    {
        let cause = vm.get_obj(oo, &PropKey::str("cause"))?;
        o.borrow_mut().props.insert(PropKey::str("cause"), Property::hidden(cause));
    }
    let name = match data_in_chain(&proto, "name") {
        Some(Value::String(s)) => s.to_rust(),
        _ => "Error".into(),
    };
    let header = if msg_text.is_empty() { name } else { format!("{name}: {msg_text}") };
    let stack = vm.stack_trace(&header);
    o.borrow_mut().props.insert(PropKey::str("stack"), Property::hidden(Value::str(&stack)));
    Ok(o)
}

fn error_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Object(build(vm, a, 0)?))
}

fn aggregate_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = build(vm, a, 1)?;
    let errors = vm.iterate_to_vec(&a.arg(0))?;
    let arr = vm.array_value(errors);
    o.borrow_mut().props.insert(PropKey::str("errors"), Property::hidden(arr));
    Ok(Value::Object(o))
}

fn to_string(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(_) = &a.this else {
        return Err(vm.type_error("Error.prototype.toString requires that 'this' be an Object"));
    };
    let name = vm.get_str(&a.this, "name")?;
    let name = if name.is_undefined() { "Error".to_string() } else { vm.to_rust_string(&name)? };
    let msg = vm.get_str(&a.this, "message")?;
    let msg = if msg.is_undefined() { String::new() } else { vm.to_rust_string(&msg)? };
    Ok(Value::str(&match (name.is_empty(), msg.is_empty()) {
        (true, _) => msg,
        (_, true) => name,
        _ => format!("{name}: {msg}"),
    }))
}

fn capture_stack_trace(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Value::Object(o) = a.arg(0) {
        let header = {
            let v = Value::Object(o.clone());
            let f = vm.get_str(&v, "toString")?;
            let s = vm.call(&f, v, Vec::new())?;
            vm.to_rust_string(&s)?
        };
        let stack = vm.stack_trace(&header);
        o.borrow_mut().props.insert(PropKey::str("stack"), Property::hidden(Value::str(&stack)));
    }
    Ok(Value::Undefined)
}
