//! Reflect.

use super::Realm;
use super::function::list_from_array_like;
use super::object::{define_own_property, from_property, set_proto_checked, to_descriptor};
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let o = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    r.tag(&o, "Reflect");
    let fns: &[(&str, u32, NativeFn)] = &[
        ("apply", 3, apply),
        ("construct", 2, construct),
        ("defineProperty", 3, define_property),
        ("deleteProperty", 2, delete_property),
        ("get", 2, get),
        ("getOwnPropertyDescriptor", 2, get_own_property_descriptor),
        ("getPrototypeOf", 1, get_prototype_of),
        ("has", 2, has),
        ("isExtensible", 1, is_extensible),
        ("ownKeys", 1, own_keys),
        ("preventExtensions", 1, prevent_extensions),
        ("set", 3, set),
        ("setPrototypeOf", 2, set_prototype_of),
    ];
    for (name, len, f) in fns {
        r.method(&o, name, *len, *f);
    }
    r.global_value("Reflect", Value::Object(o));
}

fn target(vm: &mut Vm, a: &CallArgs, what: &str) -> Result<ObjRef, Value> {
    match a.arg(0) {
        Value::Object(o) => Ok(o),
        _ => Err(vm.type_error(format!("Reflect.{what} called on non-object"))),
    }
}

fn apply(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = a.arg(0);
    if !f.is_callable() {
        let d = vm.describe(&f);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    let args = list_from_array_like(vm, &a.arg(2))?;
    vm.call(&f, a.arg(1), args)
}

fn construct(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(c) = a.arg(0) else {
        return Err(vm.type_error("Reflect.construct target is not a constructor"));
    };
    if !c.is_constructor() {
        return Err(vm.type_error("Reflect.construct target is not a constructor"));
    }
    let nt = match a.arg(2) {
        Value::Object(n) if n.is_constructor() => n,
        Value::Undefined => c.clone(),
        _ => return Err(vm.type_error("Reflect.construct newTarget is not a constructor")),
    };
    let args = list_from_array_like(vm, &a.arg(1))?;
    vm.construct(&c, args, &nt)
}

fn define_property(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = target(vm, a, "defineProperty")?;
    let key = vm.to_property_key(&a.arg(1))?;
    let d = to_descriptor(vm, &a.arg(2))?;
    Ok(Value::Bool(define_own_property(vm, &o, key, d)?))
}

fn delete_property(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = target(vm, a, "deleteProperty")?;
    let key = vm.to_property_key(&a.arg(1))?;
    if super::proxy::is_proxy(&o) {
        return Ok(Value::Bool(vm.proxy_delete(&o, &key)?));
    }
    Ok(Value::Bool(vm.delete_prop(&o, &key)))
}

