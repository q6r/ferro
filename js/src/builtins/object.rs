//! Object e Object.prototype.

use super::{Realm, arg_str, this_obj};
use crate::string::JsStr;
use crate::value::*;
use crate::vm::{Vm, same_value, truthy};

pub fn install(r: &Realm) {
    let proto = &r.object_proto;
    let c = r.ctor("Object", 1, object_ctor, proto);
    r.method(&c, "keys", 1, keys);
    r.method(&c, "values", 1, values);
    r.method(&c, "entries", 1, entries);
    r.method(&c, "assign", 2, assign);
    r.method(&c, "create", 2, create);
    r.method(&c, "defineProperty", 3, define_property);
    r.method(&c, "defineProperties", 2, define_properties);
    r.method(&c, "getOwnPropertyNames", 1, get_own_property_names);
    r.method(&c, "getOwnPropertySymbols", 1, get_own_property_symbols);
    r.method(&c, "getOwnPropertyDescriptor", 2, get_own_property_descriptor);
    r.method(&c, "getOwnPropertyDescriptors", 1, get_own_property_descriptors);
    r.method(&c, "getPrototypeOf", 1, get_prototype_of);
    r.method(&c, "setPrototypeOf", 2, set_prototype_of);
    r.method(&c, "freeze", 1, freeze);
    r.method(&c, "isFrozen", 1, is_frozen);
    r.method(&c, "seal", 1, seal);
    r.method(&c, "isSealed", 1, is_sealed);
    r.method(&c, "preventExtensions", 1, prevent_extensions);
    r.method(&c, "isExtensible", 1, is_extensible);
    r.method(&c, "is", 2, is);
    r.method(&c, "fromEntries", 1, from_entries);
    r.method(&c, "hasOwn", 2, has_own);
    r.method(&c, "groupBy", 2, group_by);

    r.method(proto, "hasOwnProperty", 1, has_own_property);
    r.method(proto, "isPrototypeOf", 1, is_prototype_of);
    r.method(proto, "propertyIsEnumerable", 1, property_is_enumerable);
    r.method(proto, "toString", 0, to_string);
    r.method(proto, "toLocaleString", 0, to_locale_string);
    r.method(proto, "valueOf", 0, value_of);
    r.method(proto, "__defineGetter__", 2, define_getter);
    r.method(proto, "__defineSetter__", 2, define_setter);
    r.method(proto, "__lookupGetter__", 1, lookup_getter);
    let get = r.native("get __proto__", 0, proto_get);
    let set = r.native("set __proto__", 1, proto_set);
    proto.borrow_mut().props.insert(
        PropKey::str("__proto__"),
        Property {
            slot: Slot::Accessor {
                get: Some(get),
                set: Some(set),
            },
            enumerable: false,
            configurable: true,
        },
    );
}

fn object_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if let Some(nt) = &a.new_target
        && !nt.ptr_eq(&a.callee)
    {
        let def = vm.realm.object_proto.clone();
        let proto = vm.proto_from_ctor(Some(nt), &def)?;
        return Ok(Value::Object(ObjRef::new(Object::new(Some(proto), Kind::Ordinary))));
    }
    let v = a.arg(0);
    if v.is_nullish() {
        return Ok(Value::Object(vm.new_object()));
    }
    Ok(Value::Object(vm.to_object(&v)?))
}

fn arg_obj(vm: &mut Vm, a: &CallArgs, i: usize) -> Result<ObjRef, Value> {
    vm.to_object(&a.arg(i))
}

/// Chaves de texto próprias e enumeráveis.
pub fn enumerable_keys(vm: &mut Vm, o: &ObjRef) -> Result<Vec<PropKey>, Value> {
    let mut out = Vec::new();
    for k in vm.own_keys(o)? {
        if matches!(k, PropKey::Str(_)) && vm.is_enumerable_own(o, &k)? {
            out.push(k);
        }
    }
    Ok(out)
}

fn keys(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = arg_obj(vm, a, 0)?;
    let ks: Vec<Value> = enumerable_keys(vm, &o)?.iter().map(|k| k.to_value()).collect();
    Ok(vm.array_value(ks))
}

fn values(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = arg_obj(vm, a, 0)?;
    let mut out = Vec::new();
    for k in enumerable_keys(vm, &o)? {
        out.push(vm.get_obj(&o, &k)?);
    }
    Ok(vm.array_value(out))
}

fn entries(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = arg_obj(vm, a, 0)?;
    let mut out = Vec::new();
    for k in enumerable_keys(vm, &o)? {
        let v = vm.get_obj(&o, &k)?;
        out.push(vm.array_value(vec![k.to_value(), v]));
    }
    Ok(vm.array_value(out))
}

