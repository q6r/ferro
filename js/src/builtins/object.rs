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

/// [[DefineOwnProperty]] (ValidateAndApplyPropertyDescriptor simplificado).
pub fn define_own_property(vm: &mut Vm, o: &ObjRef, key: PropKey, d: Descriptor) -> Result<bool, Value> {
    if super::proxy::is_proxy(o) {
        return vm.proxy_define(o, key, d);
    }
    // arrays: índices e length
    let is_array = matches!(o.borrow().kind, Kind::Array(_));
    if is_array {
        if let PropKey::Str(s) = &key
            && s.eq_str("length")
        {
            if let Some(v) = &d.value
                && !vm.set_array_length(o, v)?
            {
                return Ok(false);
            }
            if d.writable == Some(false) {
                o.borrow_mut().frozen = true;
            }
            return Ok(true);
        }
        if let Some(i) = key.as_index() {
            let plain = !d.is_accessor()
                && d.writable != Some(false)
                && d.enumerable != Some(false)
                && d.configurable != Some(false);
            let mut b = o.borrow_mut();
            let frozen = b.frozen;
            let ext = b.extensible;
            if let Kind::Array(vec) = &mut b.kind {
                let i = i as usize;
                let exists = i < vec.len() && !matches!(vec[i], Value::Empty);
                if exists && frozen {
                    return Ok(false);
                }
                if !exists && !ext {
                    return Ok(false);
                }
                if plain && (exists || (d.writable == Some(true) && d.enumerable == Some(true) && d.configurable == Some(true))) {
                    let v = d.value.clone().unwrap_or_else(|| if exists { vec[i].clone() } else { Value::Undefined });
                    if i < vec.len() {
                        vec[i] = v;
                    } else if i < vec.len() + 1_000_000 {
                        vec.resize(i, Value::Empty);
                        vec.push(v);
                    } else {
                        drop(b);
                        o.borrow_mut().props.insert(key, Property::data(v));
                    }
                    return Ok(true);
                }
                // atributos especiais: o elemento sai do vetor e vai para as propriedades
                let current = if exists { Some(std::mem::replace(&mut vec[i], Value::Empty)) } else { None };
                if i >= vec.len() && i < vec.len() + 1_000_000 {
                    vec.resize(i + 1, Value::Empty);
                }
                drop(b);
                if let Some(v) = current {
                    o.borrow_mut().props.insert(key.clone(), Property::data(v));
                }
            }
        }
    }
    let current = o.borrow().own(&key);
    let mut b = o.borrow_mut();
    match current {
        None => {
            if !b.extensible {
                return Ok(false);
            }
            let slot = if d.is_accessor() {
                Slot::Accessor {
                    get: d.get.clone().flatten(),
                    set: d.set.clone().flatten(),
                }
            } else {
                Slot::Data {
                    value: d.value.clone().unwrap_or_default(),
                    writable: d.writable.unwrap_or(false),
                }
            };
            b.props.insert(
                key,
                Property {
                    slot,
                    enumerable: d.enumerable.unwrap_or(false),
                    configurable: d.configurable.unwrap_or(false),
                },
            );
            Ok(true)
        }
        Some(cur) => {
            if !cur.configurable {
                if d.configurable == Some(true) {
                    return Ok(false);
                }
                if let Some(e) = d.enumerable
                    && e != cur.enumerable
                {
                    return Ok(false);
                }
                match &cur.slot {
                    Slot::Data { value, writable } => {
                        if d.is_accessor() {
                            return Ok(false);
                        }
                        if !*writable {
                            if d.writable == Some(true) {
                                return Ok(false);
                            }
                            if let Some(v) = &d.value
                                && !same_value(v, value)
                            {
                                return Ok(false);
                            }
                        }
                    }
                    Slot::Accessor { get, set } => {
                        if d.value.is_some() || d.writable.is_some() {
                            return Ok(false);
                        }
                        if let Some(g) = &d.get
                            && g.as_ref().map(|x| x.id()) != get.as_ref().map(|x| x.id())
                        {
                            return Ok(false);
                        }
                        if let Some(s) = &d.set
                            && s.as_ref().map(|x| x.id()) != set.as_ref().map(|x| x.id())
                        {
                            return Ok(false);
                        }
                    }
                }
            }
            let enumerable = d.enumerable.unwrap_or(cur.enumerable);
            let configurable = d.configurable.unwrap_or(cur.configurable);
            let slot = match (&cur.slot, d.is_accessor()) {
                (Slot::Accessor { get, set }, true) => Slot::Accessor {
                    get: d.get.clone().unwrap_or_else(|| get.clone()),
                    set: d.set.clone().unwrap_or_else(|| set.clone()),
                },
                (Slot::Data { .. }, true) => Slot::Accessor {
                    get: d.get.clone().flatten(),
                    set: d.set.clone().flatten(),
                },
                (Slot::Data { value, writable }, false) => Slot::Data {
                    value: d.value.clone().unwrap_or_else(|| value.clone()),
                    writable: d.writable.unwrap_or(*writable),
                },
                (Slot::Accessor { .. }, false) => {
                    if d.value.is_some() || d.writable.is_some() {
                        Slot::Data {
                            value: d.value.clone().unwrap_or_default(),
                            writable: d.writable.unwrap_or(false),
                        }
                    } else {
                        cur.slot.clone()
                    }
                }
            };
            // propriedades de string/array "virtuais" não vão para props
            if matches!(b.kind, Kind::String(_)) && key.as_index().is_some() {
                return Ok(true);
            }
            b.props.insert(
                key,
                Property {
                    slot,
                    enumerable,
                    configurable,
                },
            );
            Ok(true)
        }
    }
}

fn define_property(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(o) = a.arg(0) else {
        return Err(vm.type_error("Object.defineProperty called on non-object"));
    };
    let key = vm.to_property_key(&a.arg(1))?;
    let d = to_descriptor(vm, &a.arg(2))?;
    let existed = o.borrow().own(&key).is_some();
    if !define_own_property(vm, &o, key.clone(), d)? {
        let k = crate::ops::key_display(&key);
        if !existed && !o.borrow().extensible {
            return Err(vm.type_error(format!("Cannot define property {k}, object is not extensible")));
        }
        return Err(vm.type_error(format!("Cannot redefine property: {k}")));
    }
    Ok(Value::Object(o))
}

fn define_props_from(vm: &mut Vm, o: &ObjRef, props: &Value) -> Result<(), Value> {
    let p = vm.to_object(props)?;
    let mut descs = Vec::new();
    for k in vm.own_keys(&p)? {
        if !vm.is_enumerable_own(&p, &k)? {
            continue;
        }
        let dv = vm.get_obj(&p, &k)?;
        descs.push((k, to_descriptor(vm, &dv)?));
    }
    for (k, d) in descs {
        if !define_own_property(vm, o, k.clone(), d)? {
            return Err(vm.type_error(format!("Cannot redefine property: {}", crate::ops::key_display(&k))));
        }
    }
    Ok(())
}

fn define_properties(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(o) = a.arg(0) else {
        return Err(vm.type_error("Object.defineProperties called on non-object"));
    };
    define_props_from(vm, &o, &a.arg(1))?;
    Ok(Value::Object(o))
}

fn get_own_property_names(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = arg_obj(vm, a, 0)?;
    let ks: Vec<Value> = vm.own_keys(&o)?.into_iter().filter(|k| matches!(k, PropKey::Str(_))).map(|k| k.to_value()).collect();
    Ok(vm.array_value(ks))
}

fn get_own_property_symbols(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = arg_obj(vm, a, 0)?;
    let ks: Vec<Value> = vm.own_keys(&o)?.into_iter().filter(|k| matches!(k, PropKey::Sym(_))).map(|k| k.to_value()).collect();
    Ok(vm.array_value(ks))
}

fn get_own_property_descriptor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = arg_obj(vm, a, 0)?;
    let key = vm.to_property_key(&a.arg(1))?;
    if matches!(&key, PropKey::Sym(s) if s.is_private()) {
        return Ok(Value::Undefined);
    }
    let p = vm.get_own_property(&o, &key)?;
    Ok(match p {
        Some(p) => from_property(vm, &p),
        None => Value::Undefined,
    })
}

fn get_own_property_descriptors(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = arg_obj(vm, a, 0)?;
    let out = vm.new_object();
    for k in vm.own_keys(&o)? {
        let p = vm.get_own_property(&o, &k)?;
        if let Some(p) = p {
            let d = from_property(vm, &p);
            vm.define_data(&out, k, d);
        }
    }
    Ok(Value::Object(out))
}

fn get_prototype_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = arg_obj(vm, a, 0)?;
    Ok(vm.get_prototype_of(&o)?.map(Value::Object).unwrap_or(Value::Null))
}

pub fn set_proto_checked(vm: &mut Vm, o: &ObjRef, p: Option<ObjRef>) -> Result<bool, Value> {
    if super::proxy::is_proxy(o) {
        return vm.proxy_set_proto(o, p);
    }
    if let Some(np) = &p {
        // ciclos são proibidos
        let mut cur = Some(np.clone());
        while let Some(c) = cur {
            if c.ptr_eq(o) {
                return Err(vm.type_error("Cyclic __proto__ value"));
            }
            cur = c.proto();
        }
    }
    let same = match (&o.proto(), &p) {
        (Some(a), Some(b)) => a.ptr_eq(b),
        (None, None) => true,
        _ => false,
    };
    if same {
        return Ok(true);
    }
    if !o.borrow().extensible {
        return Ok(false);
    }
    o.borrow_mut().proto = p;
    Ok(true)
}

fn set_prototype_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let target = a.arg(0);
    if target.is_nullish() {
        return Err(vm.type_error("Object.setPrototypeOf called on null or undefined"));
    }
    let p = match a.arg(1) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return Err(vm.type_error("Object prototype may only be an Object or null")),
    };
    if let Value::Object(o) = &target
        && !set_proto_checked(vm, o, p)?
    {
        return Err(vm.type_error("#<Object> is not extensible"));
    }
    Ok(target)
}

pub fn freeze_obj(o: &ObjRef, freeze: bool) {
    let mut b = o.borrow_mut();
    b.extensible = false;
    if freeze {
        b.frozen = true;
    }
    let keys: Vec<PropKey> = b.props.iter().map(|(k, _)| k.clone()).collect();
    for k in keys {
        if let Some(p) = b.props.get_mut(&k) {
            p.configurable = false;
            if freeze && let Slot::Data { writable, .. } = &mut p.slot {
                *writable = false;
            }
        }
    }
}

fn freeze(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let v = a.arg(0);
    if let Value::Object(o) = &v {
        freeze_obj(o, true);
    }
    Ok(v)
}

fn seal(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let v = a.arg(0);
    if let Value::Object(o) = &v {
        freeze_obj(o, false);
    }
    Ok(v)
}

fn prevent_extensions(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let v = a.arg(0);
    if let Value::Object(o) = &v {
        vm.prevent_extensions(o)?;
    }
    Ok(v)
}

fn test_integrity(o: &ObjRef, frozen: bool) -> bool {
    let b = o.borrow();
    if b.extensible {
        return false;
    }
    if let Kind::Array(v) = &b.kind
        && frozen
        && !b.frozen
        && v.iter().any(|x| !matches!(x, Value::Empty))
    {
        return false;
    }
    b.props.iter().all(|(_, p)| {
        !p.configurable
            && (!frozen
                || match p.slot {
                    Slot::Data { writable, .. } => !writable,
                    _ => true,
                })
    })
}

fn is_frozen(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(match a.arg(0) {
        Value::Object(o) => test_integrity(&o, true),
        _ => true,
    }))
}

fn is_sealed(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(match a.arg(0) {
        Value::Object(o) => test_integrity(&o, false),
        _ => true,
    }))
}

fn is_extensible(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(match a.arg(0) {
        Value::Object(o) => vm.is_extensible(&o)?,
        _ => false,
    }))
}

fn is(_vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(same_value(&a.arg(0), &a.arg(1))))
}

fn from_entries(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = vm.new_object();
    let items = vm.iterate_to_vec(&a.arg(0))?;
    for item in items {
        let k = vm.get_index(&item, &Value::Number(0.0))?;
        let v = vm.get_index(&item, &Value::Number(1.0))?;
        let key = vm.to_property_key(&k)?;
        vm.define_data(&o, key, v);
    }
    Ok(Value::Object(o))
}

fn has_own(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let o = arg_obj(vm, a, 0)?;
    let key = vm.to_property_key(&a.arg(1))?;
    Ok(Value::Bool(vm.has_own(&o, &key)?))
}

fn group_by(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let items = vm.iterate_to_vec(&a.arg(0))?;
    let f = a.arg(1);
    let out = ObjRef::new(Object::new(None, Kind::Ordinary));
    for (i, item) in items.into_iter().enumerate() {
        let k = vm.call(&f, Value::Undefined, vec![item.clone(), Value::Number(i as f64)])?;
        let key = vm.to_property_key(&k)?;
        let existing = out.borrow().own(&key);
        match existing {
            Some(Property {
                slot: Slot::Data { value: Value::Object(arr), .. },
                ..
            }) => {
                if let Kind::Array(v) = &mut arr.borrow_mut().kind {
                    v.push(item);
                }
            }
            _ => {
                let arr = vm.array_value(vec![item]);
                vm.define_data(&out, key, arr);
            }
        }
    }
    Ok(Value::Object(out))
}

fn has_own_property(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let key = vm.to_property_key(&a.arg(0))?;
    let o = this_obj(vm, a)?;
    Ok(Value::Bool(vm.has_own(&o, &key)?))
}

fn is_prototype_of(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(v) = a.arg(0) else { return Ok(Value::Bool(false)) };
    let o = this_obj(vm, a)?;
    let mut cur = v.proto();
    while let Some(p) = cur {
        if p.ptr_eq(&o) {
            return Ok(Value::Bool(true));
        }
        cur = p.proto();
    }
    Ok(Value::Bool(false))
}

fn property_is_enumerable(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let key = vm.to_property_key(&a.arg(0))?;
    let o = this_obj(vm, a)?;
    let r = o.borrow().is_enumerable(&key);
    Ok(Value::Bool(r))
}

