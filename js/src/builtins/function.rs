//! Function e Function.prototype (call, apply, bind, toString).

use std::rc::Rc;

use super::Realm;
use crate::ops::data_in_chain;
use crate::string::JsStr;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let proto = &r.function_proto;
    {
        let mut b = proto.borrow_mut();
        b.props.insert(PropKey::str("length"), crate::ops::readonly(Value::Number(0.0)));
        b.props.insert(PropKey::str("name"), crate::ops::readonly(Value::str("")));
    }
    r.ctor("Function", 1, function_ctor, proto);
    // AsyncFunction, GeneratorFunction e AsyncGeneratorFunction: não são globais,
    // mas aparecem como `.constructor` das funções desses tipos
    for (fp, name, tag) in [
        (&r.async_function_proto, "AsyncFunction", "AsyncFunction"),
        (&r.generator_function_proto, "GeneratorFunction", "GeneratorFunction"),
        (&r.async_generator_function_proto, "AsyncGeneratorFunction", "AsyncGeneratorFunction"),
    ] {
        let c = super::native_fn(&r.function_proto, name, 1, function_ctor, true);
        super::set_native_data(&c, Value::str(name));
        c.borrow_mut().props.insert(
            PropKey::str("prototype"),
            Property {
                slot: Slot::Data {
                    value: Value::Object(fp.clone()),
                    writable: false,
                },
                enumerable: false,
                configurable: false,
            },
        );
        fp.borrow_mut().props.insert(PropKey::str("constructor"), Property::hidden(Value::Object(c)));
        r.tag(fp, tag);
    }
    r.generator_function_proto.borrow_mut().props.insert(
        PropKey::str("prototype"),
        Property::hidden(Value::Object(r.generator_proto.clone())),
    );
    r.generator_proto.borrow_mut().props.insert(
        PropKey::str("constructor"),
        Property::hidden(Value::Object(r.generator_function_proto.clone())),
    );
    r.method(proto, "call", 1, call);
    r.method(proto, "apply", 2, apply);
    r.method(proto, "bind", 1, bind);
    r.method(proto, "toString", 0, to_string);
    proto.borrow_mut().props.insert(
        PropKey::Sym(r.sym_has_instance.clone()),
        Property {
            slot: Slot::Data {
                value: Value::Object(r.function_has_instance.clone()),
                writable: false,
            },
            enumerable: false,
            configurable: false,
        },
    );
}

/// `new Function("a", "b", "return a + b")`.
fn function_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let n = a.args.len();
    let mut params = Vec::new();
    for i in 0..n.saturating_sub(1) {
        params.push(vm.to_rust_string(&a.args[i])?);
    }
    let body = if n > 0 { vm.to_rust_string(&a.args[n - 1])? } else { String::new() };
    let prefix = match super::native_data(&a.callee) {
        Value::String(s) if s.eq_str("AsyncFunction") => "async function",
        Value::String(s) if s.eq_str("GeneratorFunction") => "function*",
        Value::String(s) if s.eq_str("AsyncGeneratorFunction") => "async function*",
        _ => "function",
    };
    let def = match crate::parser::parse_function_parts_kind(&params.join(","), &body, prefix) {
        Ok(d) => d,
        Err(e) => return Err(vm.syntax_error(e.msg)),
    };
    let code = match crate::compiler::compile_standalone(&def, "anonymous") {
        Ok(c) => c,
        Err(e) => return Err(vm.syntax_error(e.msg)),
    };
    Ok(Value::Object(vm.make_closure(code, None, None)))
}

fn call(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if !a.this.is_callable() {
        return Err(vm.type_error("Function.prototype.call called on non-function"));
    }
    let this = a.arg(0);
    let args = if a.args.len() > 1 { a.args[1..].to_vec() } else { Vec::new() };
    vm.call(&a.this.clone(), this, args)
}

/// Lista de valores de um array-like (`apply`, `Reflect.apply`).
pub fn list_from_array_like(vm: &mut Vm, v: &Value) -> Result<Vec<Value>, Value> {
    match v {
        Value::Undefined | Value::Null => Ok(Vec::new()),
        Value::Object(o) => {
            if let Kind::Array(items) = &o.borrow().kind {
                return Ok(items
                    .iter()
                    .map(|x| match x {
                        Value::Empty => Value::Undefined,
                        x => x.clone(),
                    })
                    .collect());
            }
            let len = vm.length_of(o)?;
            let mut out = Vec::with_capacity(len.min(65536));
            for i in 0..len {
                out.push(vm.get_obj(o, &crate::ops::index_key(i))?);
            }
            Ok(out)
        }
        _ => Err(vm.type_error("CreateListFromArrayLike called on non-object")),
    }
}

fn apply(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if !a.this.is_callable() {
        return Err(vm.type_error("Function.prototype.apply was called on a non-function"));
    }
    let args = list_from_array_like(vm, &a.arg(1))?;
    vm.call(&a.this.clone(), a.arg(0), args)
}

