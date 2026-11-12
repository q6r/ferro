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

fn assign(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let target = arg_obj(vm, a, 0)?;
    let tv = Value::Object(target.clone());
    for src in a.args.iter().skip(1).cloned().collect::<Vec<_>>() {
        if src.is_nullish() {
            continue;
        }
        let so = vm.to_object(&src)?;
        for k in vm.own_keys(&so)? {
            if !vm.is_enumerable_own(&so, &k)? {
                continue;
            }
            let v = vm.get_with(&so, &k, &src)?;
            vm.put(&tv, k, v, true)?;
        }
    }
    Ok(tv)
}

fn create(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let proto = match a.arg(0) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return Err(vm.type_error("Object prototype may only be an Object or null")),
    };
    let o = ObjRef::new(Object::new(proto, Kind::Ordinary));
    let props = a.arg(1);
    if !props.is_undefined() {
        define_props_from(vm, &o, &props)?;
    }
    Ok(Value::Object(o))
}

/// Descritor de propriedade parcial (campos ausentes = None).
#[derive(Default, Clone)]
pub struct Descriptor {
    pub value: Option<Value>,
    pub writable: Option<bool>,
    pub get: Option<Option<ObjRef>>,
    pub set: Option<Option<ObjRef>>,
    pub enumerable: Option<bool>,
    pub configurable: Option<bool>,
}

impl Descriptor {
    pub fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }

    pub fn data(v: Value, all: bool) -> Descriptor {
        Descriptor {
            value: Some(v),
            writable: Some(all),
            enumerable: Some(all),
            configurable: Some(all),
            ..Default::default()
        }
    }
}

pub fn to_descriptor(vm: &mut Vm, v: &Value) -> Result<Descriptor, Value> {
    let Value::Object(o) = v else {
        let d = vm.describe(v);
        return Err(vm.type_error(format!("Property description must be an object: {d}")));
    };
    let mut d = Descriptor::default();
    let has = |vm: &mut Vm, n: &str| vm.has_property(o, &PropKey::str(n)).unwrap_or(false);
    if has(vm, "enumerable") {
        d.enumerable = Some(truthy(&vm.get_obj(o, &PropKey::str("enumerable"))?));
    }
    if has(vm, "configurable") {
        d.configurable = Some(truthy(&vm.get_obj(o, &PropKey::str("configurable"))?));
    }
    if has(vm, "value") {
        d.value = Some(vm.get_obj(o, &PropKey::str("value"))?);
    }
    if has(vm, "writable") {
        d.writable = Some(truthy(&vm.get_obj(o, &PropKey::str("writable"))?));
    }
    for (name, is_get) in [("get", true), ("set", false)] {
        if has(vm, name) {
            let f = vm.get_obj(o, &PropKey::str(name))?;
            let f = match f {
                Value::Undefined => None,
                Value::Object(fo) if fo.is_callable() => Some(fo),
                other => {
                    let dsc = vm.describe(&other);
                    return Err(vm.type_error(format!("{} must be a function: {dsc}", if is_get { "Getter" } else { "Setter" })));
                }
            };
            if is_get {
                d.get = Some(f);
            } else {
                d.set = Some(f);
            }
        }
    }
    if d.is_accessor() && (d.value.is_some() || d.writable.is_some()) {
        return Err(vm.type_error("Invalid property descriptor. Cannot both specify accessors and a value or writable attribute"));
    }
    Ok(d)
}

pub fn from_property(vm: &mut Vm, p: &Property) -> Value {
    let o = vm.new_object();
    {
        let mut b = o.borrow_mut();
        match &p.slot {
            Slot::Data { value, writable } => {
                b.props.insert(PropKey::str("value"), Property::data(value.clone()));
                b.props.insert(PropKey::str("writable"), Property::data(Value::Bool(*writable)));
            }
            Slot::Accessor { get, set } => {
                b.props.insert(PropKey::str("get"), Property::data(get.clone().map(Value::Object).unwrap_or_default()));
                b.props.insert(PropKey::str("set"), Property::data(set.clone().map(Value::Object).unwrap_or_default()));
            }
        }
        b.props.insert(PropKey::str("enumerable"), Property::data(Value::Bool(p.enumerable)));
        b.props.insert(PropKey::str("configurable"), Property::data(Value::Bool(p.configurable)));
    }
    Value::Object(o)
}

