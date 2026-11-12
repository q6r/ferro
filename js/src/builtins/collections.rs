//! Map, Set, WeakMap, WeakSet, WeakRef e FinalizationRegistry.

use super::Realm;
use super::array::iter_result;
use crate::value::*;
use crate::vm::Vm;

pub fn install(r: &Realm) {
    let mp = &r.map_proto;
    let mc = r.ctor("Map", 0, map_ctor, mp);
    r.method(&mc, "groupBy", 2, map_group_by);
    r.getter_sym(&mc, &r.sym_species, "[Symbol.species]", super::array::return_this);
    r.method(mp, "get", 1, map_get);
    r.method(mp, "set", 2, map_set);
    r.method(mp, "has", 1, coll_has);
    r.method(mp, "delete", 1, coll_delete);
    r.method(mp, "clear", 0, coll_clear);
    r.method(mp, "forEach", 1, coll_for_each);
    r.method(mp, "keys", 0, coll_keys);
    r.method(mp, "values", 0, coll_values);
    let entries = r.method(mp, "entries", 0, coll_entries);
    mp.borrow_mut().props.insert(PropKey::Sym(r.sym_iterator.clone()), Property::hidden(Value::Object(entries)));
    r.getter(mp, "size", coll_size);
    r.tag(mp, "Map");

    let sp = &r.set_proto;
    let sc = r.ctor("Set", 0, set_ctor, sp);
    r.getter_sym(&sc, &r.sym_species, "[Symbol.species]", super::array::return_this);
    r.method(sp, "add", 1, set_add);
    r.method(sp, "has", 1, coll_has);
    r.method(sp, "delete", 1, coll_delete);
    r.method(sp, "clear", 0, coll_clear);
    r.method(sp, "forEach", 1, coll_for_each);
    r.method(sp, "entries", 0, coll_entries);
    let values = r.method(sp, "values", 0, coll_values);
    sp.borrow_mut().props.insert(PropKey::str("keys"), Property::hidden(Value::Object(values.clone())));
    sp.borrow_mut().props.insert(PropKey::Sym(r.sym_iterator.clone()), Property::hidden(Value::Object(values)));
    r.getter(sp, "size", coll_size);
    r.tag(sp, "Set");

    for ip in [&r.map_iter_proto, &r.set_iter_proto] {
        r.method(ip, "next", 0, iter_next);
    }
    r.tag(&r.map_iter_proto, "Map Iterator");
    r.tag(&r.set_iter_proto, "Set Iterator");

    let wm = &r.weakmap_proto;
    r.ctor("WeakMap", 0, weakmap_ctor, wm);
    r.method(wm, "get", 1, weak_get);
    r.method(wm, "set", 2, weakmap_set);
    r.method(wm, "has", 1, weak_has);
    r.method(wm, "delete", 1, weak_delete);
    r.tag(wm, "WeakMap");

    let ws = &r.weakset_proto;
    r.ctor("WeakSet", 0, weakset_ctor, ws);
    r.method(ws, "add", 1, weakset_add);
    r.method(ws, "has", 1, weak_has);
    r.method(ws, "delete", 1, weak_delete);
    r.tag(ws, "WeakSet");

    let wr = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    r.ctor("WeakRef", 1, weakref_ctor, &wr);
    r.method(&wr, "deref", 0, weakref_deref);
    r.tag(&wr, "WeakRef");

    let fr = ObjRef::new(Object::new(Some(r.object_proto.clone()), Kind::Ordinary));
    r.ctor("FinalizationRegistry", 1, finreg_ctor, &fr);
    r.method(&fr, "register", 2, noop);
    r.method(&fr, "unregister", 1, noop_false);
    r.tag(&fr, "FinalizationRegistry");
}

fn noop(_vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Undefined)
}

fn noop_false(_vm: &mut Vm, _a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Bool(false))
}

fn require_new(vm: &mut Vm, a: &CallArgs, name: &str) -> Result<ObjRef, Value> {
    match &a.new_target {
        Some(nt) => Ok(nt.clone()),
        None => Err(vm.type_error(format!("Constructor {name} requires 'new'"))),
    }
}

fn new_coll(vm: &mut Vm, nt: &ObjRef, default: &ObjRef, set: bool) -> Result<ObjRef, Value> {
    let proto = vm.proto_from_ctor(Some(nt), default)?;
    let kind = if set { Kind::Set(Collection::default()) } else { Kind::Map(Collection::default()) };
    Ok(ObjRef::new(Object::new(Some(proto), kind)))
}

/// Preenche um Map a partir de um iterável de pares, chamando `adder`.
fn fill_from(vm: &mut Vm, o: &ObjRef, iterable: &Value, adder_name: &str, pairs: bool) -> Result<(), Value> {
    if iterable.is_nullish() {
        return Ok(());
    }
    let ov = Value::Object(o.clone());
    let adder = vm.get_str(&ov, adder_name)?;
    if !adder.is_callable() {
        return Err(vm.type_error(format!("'{adder_name}' is not a function")));
    }
    let items = vm.iterate_to_vec(iterable)?;
    for item in items {
        if pairs {
            if !matches!(item, Value::Object(_)) {
                let d = vm.describe(&item);
                return Err(vm.type_error(format!("Iterator value {d} is not an entry object")));
            }
            let k = vm.get_index(&item, &Value::Number(0.0))?;
            let v = vm.get_index(&item, &Value::Number(1.0))?;
            vm.call(&adder, ov.clone(), vec![k, v])?;
        } else {
            vm.call(&adder, ov.clone(), vec![item])?;
        }
    }
    Ok(())
}

fn map_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "Map")?;
    let def = vm.realm.map_proto.clone();
    let o = new_coll(vm, &nt, &def, false)?;
    fill_from(vm, &o, &a.arg(0), "set", true)?;
    Ok(Value::Object(o))
}

fn set_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "Set")?;
    let def = vm.realm.set_proto.clone();
    let o = new_coll(vm, &nt, &def, true)?;
    fill_from(vm, &o, &a.arg(0), "add", false)?;
    Ok(Value::Object(o))
}

fn weakmap_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "WeakMap")?;
    let def = vm.realm.weakmap_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let o = ObjRef::new(Object::new(Some(proto), Kind::WeakMap(WeakColl::default())));
    fill_from(vm, &o, &a.arg(0), "set", true)?;
    Ok(Value::Object(o))
}

fn weakset_ctor(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let nt = require_new(vm, a, "WeakSet")?;
    let def = vm.realm.weakset_proto.clone();
    let proto = vm.proto_from_ctor(Some(&nt), &def)?;
    let o = ObjRef::new(Object::new(Some(proto), Kind::WeakSet(WeakColl::default())));
    fill_from(vm, &o, &a.arg(0), "add", false)?;
    Ok(Value::Object(o))
}

/// Acessa a coleção fraca de `this` (WeakMap ou WeakSet).
fn with_weak<R>(vm: &mut Vm, a: &CallArgs, what: &str, f: impl FnOnce(&mut WeakColl) -> R) -> Result<R, Value> {
    if let Value::Object(o) = &a.this {
        let mut b = o.borrow_mut();
        if let Kind::WeakMap(c) | Kind::WeakSet(c) = &mut b.kind {
            return Ok(f(c));
        }
    }
    Err(vm.type_error(format!("Method {what} called on incompatible receiver")))
}

fn weak_get(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(with_weak(vm, a, "WeakMap.prototype.get", |c| c.get(&k))?.unwrap_or(Value::Undefined))
}

fn weak_has(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_weak(vm, a, "has", |c| c.has(&k))?))
}

fn weak_delete(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_weak(vm, a, "delete", |c| c.delete(&k))?))
}

/// Acessa a coleção de `this` (Map ou Set).
fn with_coll<R>(vm: &mut Vm, a: &CallArgs, what: &str, f: impl FnOnce(&mut Collection) -> R) -> Result<R, Value> {
    if let Value::Object(o) = &a.this {
        let mut b = o.borrow_mut();
        match &mut b.kind {
            Kind::Map(c) | Kind::Set(c) => return Ok(f(c)),
            _ => {}
        }
    }
    Err(vm.type_error(format!("Method {what} called on incompatible receiver")))
}

fn map_get(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(with_coll(vm, a, "get", |c| c.get(&k))?.unwrap_or(Value::Undefined))
}

fn map_set(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let (k, v) = (a.arg(0), a.arg(1));
    with_coll(vm, a, "set", |c| c.set(k, v))?;
    Ok(a.this.clone())
}

fn can_be_weak(v: &Value) -> bool {
    matches!(v, Value::Object(_) | Value::Symbol(_))
}

fn weakmap_set(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if !can_be_weak(&a.arg(0)) {
        let d = vm.describe(&a.arg(0));
        return Err(vm.type_error(format!("Invalid value used as weak map key: {d}")));
    }
    let (k, v) = (a.arg(0), a.arg(1));
    with_weak(vm, a, "WeakMap.prototype.set", |c| c.set(&k, v))?;
    Ok(a.this.clone())
}

fn set_add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    with_coll(vm, a, "add", |c| {
        let key = k.clone();
        c.set(key, k)
    })?;
    Ok(a.this.clone())
}

fn weakset_add(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    if !can_be_weak(&a.arg(0)) {
        let d = vm.describe(&a.arg(0));
        return Err(vm.type_error(format!("Invalid value used in weak set: {d}")));
    }
    let k = a.arg(0);
    with_weak(vm, a, "WeakSet.prototype.add", |c| c.set(&k, Value::Bool(true)))?;
    Ok(a.this.clone())
}

fn coll_has(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_coll(vm, a, "has", |c| c.has(&k))?))
}

fn coll_delete(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let k = a.arg(0);
    Ok(Value::Bool(with_coll(vm, a, "delete", |c| c.delete(&k))?))
}

fn coll_clear(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    with_coll(vm, a, "clear", |c| c.clear())?;
    Ok(Value::Undefined)
}

fn coll_size(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    Ok(Value::Number(with_coll(vm, a, "size", |c| c.size())? as f64))
}

fn coll_for_each(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let f = a.arg(0);
    if !f.is_callable() {
        let d = vm.describe(&f);
        return Err(vm.type_error(format!("{d} is not a function")));
    }
    let is_set = matches!(&a.this, Value::Object(o) if matches!(o.borrow().kind, Kind::Set(_)));
    let mut i = 0;
    loop {
        // relê a cada passo: o callback pode alterar a coleção
        let entry = with_coll(vm, a, "forEach", |c| {
            while i < c.entries.len() {
                if let Some(e) = &c.entries[i] {
                    i += 1;
                    return Some(e.clone());
                }
                i += 1;
            }
            None
        })?;
        let Some((k, v)) = entry else { break };
        let args = if is_set { vec![k.clone(), k, a.this.clone()] } else { vec![v, k, a.this.clone()] };
        vm.call(&f, a.arg(1), args)?;
    }
    Ok(Value::Undefined)
}

fn make_iter(vm: &mut Vm, a: &CallArgs, kind: IterKind) -> Result<Value, Value> {
    let Value::Object(o) = &a.this else {
        return Err(vm.type_error("Method called on incompatible receiver"));
    };
    let proto = match &o.borrow().kind {
        Kind::Map(_) => vm.realm.map_iter_proto.clone(),
        Kind::Set(_) => vm.realm.set_iter_proto.clone(),
        _ => return Err(vm.type_error("Method called on incompatible receiver")),
    };
    Ok(Value::Object(ObjRef::new(Object::new(
        Some(proto),
        Kind::MapIter {
            target: o.clone(),
            index: 0,
            kind,
        },
    ))))
}

fn coll_keys(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, IterKind::Keys)
}

fn coll_values(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, IterKind::Values)
}

fn coll_entries(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    make_iter(vm, a, IterKind::Entries)
}

fn iter_next(vm: &mut Vm, a: &mut CallArgs) -> Result<Value, Value> {
    let Value::Object(it) = &a.this else {
        return Err(vm.type_error("next method called on incompatible receiver"));
    };
    let (target, index, kind) = match &it.borrow().kind {
        Kind::MapIter { target, index, kind } => (target.clone(), *index, *kind),
        _ => return Err(vm.type_error("next method called on incompatible receiver")),
    };
    let found = {
        let b = target.borrow();
        let c = match &b.kind {
            Kind::Map(c) | Kind::Set(c) => c,
            _ => return Ok(iter_result(vm, Value::Undefined, true)),
        };
        let mut i = index;
        let mut out = None;
        while i < c.entries.len() {
            if let Some(e) = &c.entries[i] {
                out = Some((i, e.clone()));
                break;
            }
            i += 1;
        }
        out
    };
    match found {
        Some((i, (k, v))) => {
            if let Kind::MapIter { index, .. } = &mut it.borrow_mut().kind {
                *index = i + 1;
            }
            let val = match kind {
                IterKind::Keys => k,
                IterKind::Values => v,
                IterKind::Entries => vm.array_value(vec![k, v]),
            };
            Ok(iter_result(vm, val, false))
        }
        None => {
            if let Kind::MapIter { index, .. } = &mut it.borrow_mut().kind {
                *index = usize::MAX / 2;
            }
            Ok(iter_result(vm, Value::Undefined, true))
        }
    }
}

